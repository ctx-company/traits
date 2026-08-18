//! Host sinks: canonical declarations of trait output the host applies to
//! its own surfaces (task 0110). Sinks are a closed, typed set — the first
//! is `[sink.session-title]`; a new sink joins this module only when a
//! consumer earns it, never as an open map key.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::procedure::runtime::Value;
use crate::reference::{Kind, Reference};
use crate::r#trait::procedure::SequenceInput;
use crate::r#trait::prompt::{render_interpolation_value, scan_interpolations};

/// How a sink's declared input becomes the applied value.
///
/// `Verbatim`: authored text (a literal string or a template) rendered
/// deterministically, with zero model calls. `Generated`: material (a bare
/// slot ref or an array of slot refs) assembled and handed to the host's
/// existing generation path as context, replacing that path's default
/// context. Stored explicitly in canonical even though it is fully derived
/// from the shape of `input` at authoring time, so decoded documents never
/// need to re-derive it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SinkMode {
    Verbatim,
    Generated,
}

/// A sink's declared input: the standard input union, no new value grammar.
///
/// `Scalar` covers a literal string, a template string using the prompt
/// interpolation grammar (`{slot:<id>}`), or a bare `slot:<id>` ref — which
/// of those it is (and therefore whether the sink is verbatim or generated)
/// is classified at decode time, not by this shape. `Parts` is an array of
/// slot refs, optionally decorated `{ slot = "slot:<id>", optional = true }`
/// (P447); an array input is always generated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum SinkInput {
    Scalar(String),
    Parts(Vec<SequenceInput>),
}

impl<'de> Deserialize<'de> for SinkInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Scalar(String),
            Parts(Vec<SequenceInput>),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Scalar(s) => Ok(SinkInput::Scalar(s)),
            Raw::Parts(parts) => Ok(SinkInput::Parts(parts)),
        }
    }
}

/// One `[sink.session-title]` declaration: `effect.session.title(X)`'s
/// canonical projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct SessionTitleSink {
    pub mode: SinkMode,
    pub input: SinkInput,
}

/// The closed set of host sinks a trait may declare. One optional field per
/// sink — never an open map — so an unknown sink key is a decode-time
/// error and a duplicate declaration is structurally impossible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Sinks {
    #[serde(
        default,
        rename = "session-title",
        skip_serializing_if = "Option::is_none"
    )]
    pub session_title: Option<SessionTitleSink>,
}

impl Sinks {
    pub fn is_empty(&self) -> bool {
        self.session_title.is_none()
    }
}

/// Render a slot-interpolating `[sink.session-title]` `verbatim` template
/// against currently accepted slot values (task 0110 frame-boundary
/// firing). Returns `None` when the sink is not `verbatim`, its input is
/// not a scalar template, the template carries zero interpolations (the
/// fully-static case, resolved before the drive loop ever starts — see
/// `static_verbatim_sink_title`), or any interpolated slot has no accepted
/// value yet (not ready).
pub fn render_verbatim_sink_title(sink: &SessionTitleSink, accepted: &[Value]) -> Option<String> {
    if sink.mode != SinkMode::Verbatim {
        return None;
    }
    let SinkInput::Scalar(text) = &sink.input else {
        return None;
    };
    let (interpolations, _diagnostics) = scan_interpolations(text);
    if interpolations.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut cursor = 0;
    for interp in &interpolations {
        out.extend(chars[cursor..interp.start].iter());
        let rendered = accepted
            .iter()
            .find(|value| value.ref_text == interp.ref_text)
            .and_then(|value| render_interpolation_value(&value.value))?;
        out.push_str(&rendered);
        cursor = interp.end;
    }
    out.extend(chars[cursor..].iter());
    Some(out)
}

/// Assemble the material context for a `generated` `[sink.session-title]`
/// sink from currently accepted slot values (task 0110 frame-boundary
/// firing). Returns `None` when the sink is not `generated`, or a required
/// part has no accepted value yet; a `.optional()` part is included only
/// when present and otherwise skipped rather than blocking readiness.
pub fn assemble_generated_sink_material(
    sink: &SessionTitleSink,
    accepted: &[Value],
) -> Option<String> {
    if sink.mode != SinkMode::Generated {
        return None;
    }
    match &sink.input {
        SinkInput::Scalar(ref_text) => accepted
            .iter()
            .find(|value| value.ref_text == *ref_text)
            .and_then(|value| render_interpolation_value(&value.value)),
        SinkInput::Parts(parts) => {
            let mut rendered_parts = Vec::new();
            for part in parts {
                let found = accepted
                    .iter()
                    .find(|value| value.ref_text == part.ref_text())
                    .and_then(|value| render_interpolation_value(&value.value));
                match found {
                    Some(rendered) => rendered_parts.push(rendered),
                    None if part.is_optional() => {}
                    None => return None,
                }
            }
            if rendered_parts.is_empty() {
                None
            } else {
                Some(rendered_parts.join("\n\n"))
            }
        }
    }
}

fn invalid_field(field_path: impl Into<String>, message: impl Into<String>) -> crate::Error {
    crate::manifest::Error::InvalidField {
        field_path: field_path.into(),
        message: message.into(),
    }
    .into()
}

fn check_slot_declared(id: &str, slot_ids: &BTreeSet<&str>, field: &str) -> crate::Result<()> {
    if slot_ids.contains(id) {
        Ok(())
    } else {
        Err(invalid_field(
            field,
            format!("slot ref \"slot:{id}\" does not resolve to a declared [[slot]]"),
        ))
    }
}

/// Decode-time validation for `[sink.*]` declarations (task 0110).
///
/// Slot refs must resolve to declared slots; an X whose parts are entirely
/// optional is a build error (a sink that may never have material is a
/// declaration mistake); mode must agree with the shape of `input` — a
/// string or template (including one with zero interpolations, which is
/// verbatim by definition, no special case) is `verbatim`, a bare slot ref
/// or an array of slot refs is `generated`.
pub fn validate_sinks(sinks: &Sinks, slot_ids: &BTreeSet<&str>) -> crate::Result<()> {
    let Some(sink) = sinks.session_title.as_ref() else {
        return Ok(());
    };
    validate_session_title_sink(sink, slot_ids)
}

fn validate_session_title_sink(
    sink: &SessionTitleSink,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    const FIELD: &str = "sink.session-title";
    match &sink.input {
        SinkInput::Scalar(text) => {
            if let Ok(r) = Reference::parse(text) {
                if r.try_kind()? != Kind::Slot {
                    return Err(invalid_field(
                        format!("{FIELD}.input"),
                        format!(
                            "input {text:?} parses as a {} ref; a bare typed-ref sink input must be a slot ref",
                            r.try_kind()?
                        ),
                    ));
                }
                if sink.mode != SinkMode::Generated {
                    return Err(invalid_field(
                        format!("{FIELD}.mode"),
                        "a bare slot ref input requires mode = \"generated\"",
                    ));
                }
                return check_slot_declared(r.id(), slot_ids, &format!("{FIELD}.input"));
            }

            if sink.mode != SinkMode::Verbatim {
                return Err(invalid_field(
                    format!("{FIELD}.mode"),
                    "a string or template input requires mode = \"verbatim\"",
                ));
            }
            let (interpolations, diagnostics) = scan_interpolations(text);
            if let Some(diagnostic) = diagnostics.first() {
                return Err(invalid_field(format!("{FIELD}.input"), diagnostic.clone()));
            }
            for interpolation in &interpolations {
                let parsed = Reference::parse(&interpolation.ref_text).map_err(|_| {
                    invalid_field(
                        format!("{FIELD}.input"),
                        format!(
                            "interpolation {{{}}} is not a valid typed ref",
                            interpolation.ref_text
                        ),
                    )
                })?;
                if parsed.try_kind()? != Kind::Slot {
                    return Err(invalid_field(
                        format!("{FIELD}.input"),
                        format!(
                            "interpolation {{{}}} kind {} not allowed; only slot refs may interpolate into a sink",
                            interpolation.ref_text,
                            parsed.try_kind()?
                        ),
                    ));
                }
                check_slot_declared(parsed.id(), slot_ids, &format!("{FIELD}.input"))?;
            }
            Ok(())
        }
        SinkInput::Parts(parts) => {
            if sink.mode != SinkMode::Generated {
                return Err(invalid_field(
                    format!("{FIELD}.mode"),
                    "an array input requires mode = \"generated\"",
                ));
            }
            if parts.is_empty() {
                return Err(invalid_field(
                    format!("{FIELD}.input"),
                    "array input must declare at least one part",
                ));
            }
            if parts.iter().all(SequenceInput::is_optional) {
                return Err(invalid_field(
                    format!("{FIELD}.input"),
                    "every part is optional; a sink that may never have material is a declaration mistake",
                ));
            }
            for part in parts {
                let parsed = Reference::parse(part.ref_text()).map_err(|_| {
                    invalid_field(
                        format!("{FIELD}.input"),
                        format!("{:?} is not a valid typed ref", part.ref_text()),
                    )
                })?;
                if parsed.try_kind()? != Kind::Slot {
                    return Err(invalid_field(
                        format!("{FIELD}.input"),
                        format!(
                            "{:?} is a {} ref; only slot refs are valid sink array parts",
                            part.ref_text(),
                            parsed.try_kind()?
                        ),
                    ));
                }
                check_slot_declared(parsed.id(), slot_ids, &format!("{FIELD}.input"))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_ids(ids: &[&'static str]) -> BTreeSet<&'static str> {
        ids.iter().copied().collect()
    }

    #[test]
    fn string_input_is_verbatim() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("Fixed session title".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&[])).is_ok());
    }

    #[test]
    fn template_with_zero_interpolations_is_verbatim_no_special_case() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("plain title, no braces".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&[])).is_ok());
    }

    #[test]
    fn template_with_declared_slot_interpolation_is_verbatim() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("Working on {slot:topic}".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["topic"])).is_ok());
    }

    #[test]
    fn template_with_undeclared_slot_interpolation_is_rejected() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("Working on {slot:topic}".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&[])).is_err());
    }

    #[test]
    fn bare_slot_ref_requires_generated_mode() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("slot:draft".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft"])).is_err());

        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Scalar("slot:draft".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft"])).is_ok());
    }

    #[test]
    fn bare_slot_ref_must_resolve_to_declared_slot() {
        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Scalar("slot:missing".to_string()),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft"])).is_err());
    }

    #[test]
    fn array_input_requires_generated_mode() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Parts(vec![SequenceInput::Ref("slot:draft".to_string())]),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft"])).is_err());
    }

    #[test]
    fn array_input_all_optional_is_a_build_error() {
        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Parts(vec![SequenceInput::OptionalSlot {
                slot: "slot:draft".to_string(),
                optional: true,
            }]),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft"])).is_err());
    }

    #[test]
    fn array_input_mixed_required_and_optional_is_ok() {
        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Parts(vec![
                SequenceInput::Ref("slot:draft".to_string()),
                SequenceInput::OptionalSlot {
                    slot: "slot:context".to_string(),
                    optional: true,
                },
            ]),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&["draft", "context"])).is_ok());
    }

    #[test]
    fn array_input_non_slot_part_is_rejected() {
        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Parts(vec![SequenceInput::Ref("port:user-prompt".to_string())]),
        };
        assert!(validate_session_title_sink(&sink, &slot_ids(&[])).is_err());
    }

    fn accepted(ref_text: &str, value: serde_json::Value) -> Value {
        Value {
            ref_text: ref_text.to_string(),
            value_digest: crate::digest::canonical_digest(&value).unwrap(),
            value,
            schema_ref: None,
            source: crate::procedure::runtime::ValueSource::ModelOutput,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
            producer_check_verdict: false,
            acceptance: crate::procedure::runtime::AcceptanceStatus::Accepted,
            position_path: Vec::new(),
            acceptance_order: None,
            schema_validation: Vec::new(),
        }
    }

    #[test]
    fn render_verbatim_template_waits_for_every_interpolated_slot() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("Working on {slot:topic}".to_string()),
        };
        assert_eq!(render_verbatim_sink_title(&sink, &[]), None);
        let accepted = [accepted(
            "slot:topic",
            serde_json::Value::String("the launch plan".to_string()),
        )];
        assert_eq!(
            render_verbatim_sink_title(&sink, &accepted),
            Some("Working on the launch plan".to_string())
        );
    }

    #[test]
    fn render_verbatim_template_with_multiple_slots_requires_all_of_them() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("{slot:a} / {slot:b}".to_string()),
        };
        let only_a = [accepted(
            "slot:a",
            serde_json::Value::String("A".to_string()),
        )];
        assert_eq!(render_verbatim_sink_title(&sink, &only_a), None);
        let both = [
            accepted("slot:a", serde_json::Value::String("A".to_string())),
            accepted("slot:b", serde_json::Value::String("B".to_string())),
        ];
        assert_eq!(
            render_verbatim_sink_title(&sink, &both),
            Some("A / B".to_string())
        );
    }

    #[test]
    fn render_verbatim_returns_none_for_generated_mode() {
        let sink = SessionTitleSink {
            mode: SinkMode::Generated,
            input: SinkInput::Scalar("slot:draft".to_string()),
        };
        let accepted = [accepted(
            "slot:draft",
            serde_json::Value::String("x".to_string()),
        )];
        assert_eq!(render_verbatim_sink_title(&sink, &accepted), None);
    }

    #[test]
    fn render_verbatim_returns_none_for_zero_interpolation_static_text() {
        let sink = SessionTitleSink {
            mode: SinkMode::Verbatim,
            input: SinkInput::Scalar("Fixed session title".to_string()),
        };
        assert_eq!(render_verbatim_sink_title(&sink, &[]), None);
    }

    fn decode(text: &str) -> crate::Result<crate::r#trait::Trait> {
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, text)
    }

    #[test]
    fn decode_verbatim_string_sink_round_trips() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."

[sink.session-title]
mode = "verbatim"
input = "Fixed session title"
"#;
        let t = decode(FIXTURE).expect("decode");
        assert_eq!(
            t.sinks.session_title.as_ref().unwrap().mode,
            SinkMode::Verbatim
        );
        let encoded = crate::encoding::encode(crate::encoding::Encoding::Toml, &t).unwrap();
        let round_tripped = decode(&encoded).expect("re-decode");
        assert_eq!(t, round_tripped);
    }

    #[test]
    fn decode_generated_slot_sink_round_trips() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."

[[slot]]
id = "draft"
schema = "schema:text"

[sink.session-title]
mode = "generated"
input = "slot:draft"
"#;
        let t = decode(FIXTURE).expect("decode");
        assert_eq!(
            t.sinks.session_title.as_ref().unwrap().mode,
            SinkMode::Generated
        );
    }

    #[test]
    fn decode_rejects_undeclared_slot_ref() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."

[sink.session-title]
mode = "generated"
input = "slot:missing"
"#;
        assert!(decode(FIXTURE).is_err());
    }

    #[test]
    fn decode_rejects_unknown_sink_key() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."

[sink.notify]
mode = "verbatim"
input = "hello"
"#;
        assert!(decode(FIXTURE).is_err());
    }

    #[test]
    fn decode_rejects_all_optional_array_input() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."

[[slot]]
id = "draft"
schema = "schema:text"

[sink.session-title]
mode = "generated"
input = [{ slot = "slot:draft", optional = true }]
"#;
        assert!(decode(FIXTURE).is_err());
    }

    #[test]
    fn no_declaration_decodes_with_empty_sinks() {
        const FIXTURE: &str = r#"
id = "sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sink Fixture"
description = "Scratch fixture for the session-title sink."
"#;
        let t = decode(FIXTURE).expect("decode");
        assert!(t.sinks.is_empty());
    }
}
