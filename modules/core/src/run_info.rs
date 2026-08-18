//! Non-mutating run preflight summaries and trait argument parsing.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::procedure::runtime::{StepSlotOutput, ValueSource};
use crate::response::CapabilityReport;
use crate::r#trait::activation::{Gate, lifecycle_trust_gates_for_check};
use crate::r#trait::{PortDirection, Trait};

/// Query/direct selection status for `run-info` and query `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum RunInfoSelectionStatus {
    Selected,
    Ambiguous,
    NoMatch,
    Blocked,
    Unsupported,
}

/// Candidate evidence used when query mode cannot select exactly one runnable trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoCandidateSummary {
    pub trait_id: String,
    pub name: String,
    pub score: i32,
    pub rank_tier: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<Gate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// Selection evidence included in run-info output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoSelectionSummary {
    pub status: RunInfoSelectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_trait_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<RunInfoCandidateSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

impl RunInfoSelectionSummary {
    pub fn selected_direct(trait_id: &str) -> Self {
        Self {
            status: RunInfoSelectionStatus::Selected,
            query: None,
            selected_trait_id: Some(trait_id.to_string()),
            candidates: Vec::new(),
            reasons: Vec::new(),
        }
    }
}

/// One input argument row for starting a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoInputPort {
    pub port: String,
    pub argument: String,
    pub schema: String,
    pub required: bool,
    pub text_shorthand: bool,
    pub submission: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_evidence: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Output port row for a run-info summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoOutputPort {
    pub port: String,
    pub schema: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_slot: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Lifecycle/trust evidence for a run-info summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoLifecycleSummary {
    pub status: String,
    pub trust: String,
    pub runnable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<Gate>,
}

/// Source identity evidence when a concrete trait file is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoTraitIdentity {
    pub trait_id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,
}

/// Read-only resolved dispatch reminder. Resolution is supplied by the IO/CLI
/// boundary; core keeps this row serializable and deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoDispatchReminder {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_mode: Option<String>,
    pub assigned: bool,
    /// 1-based seat position within the role's configured
    /// `[[agent.role.<role>]]` list (P456). Absent for a legacy
    /// single-table role, so its row's serialized bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_index: Option<u32>,
    /// The configured list length this seat was selected from. Present if
    /// and only if `seat_index` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_length: Option<u32>,
}

/// Pure declaration reminder for a command the trusted runtime may execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoCommandReminder {
    pub declaration_path: String,
    pub command: String,
}

/// Complete non-mutating run-info response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunInfoSummary {
    pub trait_identity: RunInfoTraitIdentity,
    pub selection: RunInfoSelectionSummary,
    pub lifecycle: RunInfoLifecycleSummary,
    pub input_ports: Vec<RunInfoInputPort>,
    pub output_ports: Vec<RunInfoOutputPort>,
    pub dispatch_reminders: Vec<RunInfoDispatchReminder>,
    pub command_reminders: Vec<RunInfoCommandReminder>,
    pub start_examples: Vec<String>,
    pub capabilities: Vec<CapabilityReport>,
}

/// Build a pure, non-mutating summary from an already decoded trait.
///
/// `status`/`trust` are caller-resolved from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn summarize_run_info(
    trait_ref: &Trait,
    status: &crate::manifest::PackageStatus,
    trust: &crate::r#trait::TrustVerdict,
    source_path: Option<&str>,
    source_digest: Option<&str>,
    canonical_digest: Option<&str>,
    selection: Option<RunInfoSelectionSummary>,
) -> RunInfoSummary {
    let gates = lifecycle_trust_gates_for_check(trait_ref.id.as_str(), status, trust);
    let input_ports = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Input))
        .map(|port| {
            let text_shorthand = schema_accepts_text_shorthand(&port.schema);
            RunInfoInputPort {
                port: format!("port:{}", port.id),
                argument: format!("--{}", port.id),
                schema: port.schema.clone(),
                required: !port.optional,
                text_shorthand,
                submission: if text_shorthand {
                    "text-shorthand".to_string()
                } else if schema_accepts_flag_shorthand(&port.schema) {
                    "flag-shorthand".to_string()
                } else {
                    "json-value-required".to_string()
                },
                default_evidence: port.default.as_ref().map(|default| {
                    if default.command.is_some() {
                        "default.command declared; executed by the trusted local runtime"
                            .to_string()
                    } else {
                        "default declared".to_string()
                    }
                }),
                description: port.description.clone(),
                title: port.title.clone(),
                hint: None,
            }
        })
        .collect::<Vec<_>>();
    let output_ports = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Output))
        .map(|port| RunInfoOutputPort {
            port: format!("port:{}", port.id),
            schema: port.schema.clone(),
            required: !port.optional,
            value_slot: port.value.clone(),
            description: port.description.clone(),
            title: port.title.clone(),
        })
        .collect::<Vec<_>>();
    let selected = selection
        .unwrap_or_else(|| RunInfoSelectionSummary::selected_direct(trait_ref.id.as_str()));
    let start_examples = start_examples(trait_ref.id.as_str(), source_path, &input_ports);
    RunInfoSummary {
        trait_identity: RunInfoTraitIdentity {
            trait_id: trait_ref.id.as_str().to_string(),
            name: trait_ref.name.as_str().to_string(),
            version: trait_ref.version.as_str().to_string(),
            source_path: source_path.map(str::to_string),
            source_digest: source_digest.map(Digest::from_unvalidated),
            canonical_digest: canonical_digest.map(Digest::from_unvalidated),
        },
        selection: selected,
        lifecycle: RunInfoLifecycleSummary {
            status: status.display_name().to_string(),
            trust: trust.display_name().to_string(),
            runnable: gates.is_empty(),
            gates,
        },
        input_ports,
        output_ports,
        dispatch_reminders: Vec::new(),
        command_reminders: declared_command_reminders(trait_ref),
        start_examples,
        capabilities: run_info_capabilities(),
    }
}

/// Format the blocking-gate detail suffix (e.g. `"; blocked.status.draft (...); run ..."`)
/// for a query selection that failed to select exactly one runnable trait.
/// Empty string when the selection carries no blocking gates. Shared by every
/// caller that turns a failed `RunInfoSelectionSummary` into a refusal message
/// (CLI query run, IO query run) so the gate detail is derived in one place.
pub fn selection_refusal_detail(selection: &RunInfoSelectionSummary) -> String {
    let blocking_gates: Vec<Gate> = selection
        .candidates
        .iter()
        .flat_map(|candidate| candidate.gates.clone())
        .collect();
    if blocking_gates.is_empty() {
        String::new()
    } else {
        format!(
            "; {}",
            crate::r#trait::activation::format_gate_refusal(&blocking_gates)
        )
    }
}

fn declared_command_reminders(trait_ref: &Trait) -> Vec<RunInfoCommandReminder> {
    let mut rows = Vec::new();
    if let Some(procedure) = &trait_ref.procedure {
        append_command_reminders(&mut rows, "procedure.sequence", &procedure.sequence);
    }
    for (id, sequence) in trait_ref.sequences.iter() {
        append_command_reminders(
            &mut rows,
            &format!("sequence.{id}.sequence"),
            &sequence.sequence,
        );
    }
    rows.sort_by(|left, right| left.declaration_path.cmp(&right.declaration_path));
    rows
}

fn append_command_reminders(
    rows: &mut Vec<RunInfoCommandReminder>,
    prefix: &str,
    items: &[crate::r#trait::procedure::SequenceItem],
) {
    for (index, item) in items.iter().enumerate() {
        let argv = match (&item.cmd, &item.command) {
            (Some(cmd), _) => crate::r#trait::procedure::parse_command_shorthand(
                cmd,
                &format!("{prefix}[{index}].cmd"),
            )
            .unwrap_or_else(|_| vec![cmd.clone()]),
            (None, Some(command)) => command.argv.clone(),
            (None, None) => continue,
        };
        rows.push(RunInfoCommandReminder {
            declaration_path: format!("{prefix}[{index}]"),
            command: argv.join(" "),
        });
    }
}

/// Whether a schema can be safely filled by CLI/MCP text shorthand.
pub fn schema_accepts_text_shorthand(schema: &str) -> bool {
    schema == "schema:text"
}

/// Whether a schema can be filled by bare-flag shorthand: `--<port-id>`
/// alone sets the port to `true`; `--<port-id>=false` sets it to `false`.
pub fn schema_accepts_flag_shorthand(schema: &str) -> bool {
    schema == "schema:boolean"
}

/// Parse exact trait args (`--<port-id> value`, or bare `--<port-id>` for a
/// boolean port) into runtime initial values.
pub fn parse_trait_arguments(
    trait_ref: &Trait,
    tokens: &[String],
    producer_evidence: &str,
) -> crate::Result<Vec<StepSlotOutput>> {
    let accepted = input_port_map(trait_ref);
    let accepted_args = accepted
        .keys()
        .map(|id| format!("--{id}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if !token.starts_with("--") || token == "--" {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("trait-args[{index}]"),
                message: format!(
                    "expected trait argument --<port-id>; accepted arguments: {}",
                    empty_to_none(&accepted_args)
                ),
            }
            .into());
        }
        let (raw_name, explicit_value) = match token[2..].split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (&token[2..], None),
        };
        let Some(port) = accepted.get(raw_name) else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("trait-args[{index}]"),
                message: format!(
                    "unknown trait argument --{raw_name}; accepted arguments: {}",
                    empty_to_none(&accepted_args)
                ),
            }
            .into());
        };
        if !seen.insert(raw_name.to_string()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("trait-args[{index}]"),
                message: format!("duplicate trait argument --{raw_name}"),
            }
            .into());
        }
        let (value, consumed) = if schema_accepts_flag_shorthand(&port.schema) {
            // A boolean port is a flag: bare `--flag` means true and never
            // consumes the next token, so `--flag other-flag-value` parses
            // exactly like `--flag --other flag-value` would.
            match explicit_value {
                None => {
                    // A following non-flag token can never be valid here;
                    // refuse it now with the flag named instead of letting
                    // the next iteration report a generic parse error.
                    if let Some(next) = tokens.get(index + 1)
                        && !next.starts_with("--")
                    {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("trait-args[{index}]"),
                            message: format!(
                                "boolean argument --{raw_name} takes no separate value; pass bare --{raw_name} for true or --{raw_name}=false"
                            ),
                        }
                        .into());
                    }
                    (serde_json::Value::Bool(true), 1)
                }
                Some("true") => (serde_json::Value::Bool(true), 1),
                Some("false") => (serde_json::Value::Bool(false), 1),
                Some(other) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("trait-args[{index}]"),
                        message: format!(
                            "boolean argument --{raw_name} accepts bare --{raw_name} (true), --{raw_name}=true, or --{raw_name}=false; got {other:?}"
                        ),
                    }
                    .into());
                }
            }
        } else if schema_accepts_text_shorthand(&port.schema) {
            match explicit_value {
                Some(value) => (serde_json::Value::String(value.to_string()), 1),
                None => {
                    let Some(value) = tokens.get(index + 1) else {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("trait-args[{index}]"),
                            message: format!("argument {token:?} requires a value"),
                        }
                        .into());
                    };
                    (serde_json::Value::String(value.clone()), 2)
                }
            }
        } else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("trait-args[{index}]"),
                message: format!(
                    "argument --{raw_name} uses schema {}; provide JSON input with --input or --set instead of text shorthand",
                    port.schema
                ),
            }.into());
        };
        values.push(StepSlotOutput {
            ref_text: format!("port:{raw_name}"),
            value,
            source: Some(ValueSource::HostInput),
            producer_evidence: Some(producer_evidence.to_string()),
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        });
        index += consumed;
    }
    values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    Ok(values)
}

/// If query text is compatible with exactly one goal/task-like required text input,
/// convert it into grounded input evidence.
pub fn query_text_initial_value(trait_ref: &Trait, query: &str) -> Option<StepSlotOutput> {
    let mut candidates = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Input))
        .filter(|port| !port.optional)
        .filter(|port| schema_accepts_text_shorthand(&port.schema))
        .filter(|port| matches!(port.id.as_str(), "goal" | "task" | "request" | "user-goal"))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    if candidates.len() != 1 {
        return None;
    }
    Some(StepSlotOutput {
        ref_text: format!("port:{}", candidates[0].id),
        value: serde_json::Value::String(query.to_string()),
        source: Some(ValueSource::HostInput),
        producer_evidence: Some("query-run grounded input".to_string()),
        command_execution: None,
        producer_agent: None,
        producer_harness: None,
    })
}

fn input_port_map(trait_ref: &Trait) -> BTreeMap<String, &crate::r#trait::Port> {
    trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Input))
        .map(|port| (port.id.clone(), port))
        .collect()
}

fn start_examples(
    trait_id: &str,
    source_path: Option<&str>,
    input_ports: &[RunInfoInputPort],
) -> Vec<String> {
    let mut args = input_ports
        .iter()
        .filter(|port| port.required)
        .map(|port| {
            if port.text_shorthand {
                format!("{} <value>", port.argument)
            } else if schema_accepts_flag_shorthand(&port.schema) {
                format!("{}[=true|false]", port.argument)
            } else {
                format!("{} <json-via---input-or---set>", port.argument)
            }
        })
        .collect::<Vec<_>>();
    if args.is_empty() {
        args.push("# no required input ports".to_string());
    }
    let arg_text = args.join(" ");
    let mut examples = Vec::new();
    examples.push(format!("ctx traits run {trait_id} -- {arg_text}"));
    if let Some(path) = source_path {
        examples.push(format!("ctx traits run --file {path} -- {arg_text}"));
    }
    examples
}

pub fn run_info_capabilities() -> Vec<CapabilityReport> {
    vec![
        CapabilityReport::supported("runtime.run-info"),
        CapabilityReport::supported("runtime.session-persistence"),
        CapabilityReport::unsupported(
            "runtime.command-execution",
            "run-info never executes trait-declared commands or default commands",
        ),
        CapabilityReport::unsupported(
            "runtime.provider-calls",
            "run-info never calls providers or models",
        ),
        CapabilityReport::supported("runtime.mcp-run-info"),
        CapabilityReport::unsupported(
            "runtime.resource-access",
            "run-info does not read resource bodies or require resource evidence",
        ),
    ]
}

fn empty_to_none(value: &str) -> String {
    if value.is_empty() {
        "none".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod trait_argument_tests {
    use super::*;

    /// Scratch fixture: one text input, one boolean input, one prompt step
    /// consuming them — the smallest trait the argument parser can run
    /// against.
    const FIXTURE: &str = r#"
id = "flag-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Flag fixture"
description = "Trait-argument parser fixture with a boolean input port."

[[port]]
id = "goal"
direction = "input"
schema = "schema:text"
description = "What to do."

[[port]]
id = "dry-run"
direction = "input"
schema = "schema:boolean"
optional = true
description = "Plan without acting."

[[agent]]
id = "worker"
description = "Does the work."
summary = "Worker role."

[[slot]]
id = "result"
schema = "schema:text"
description = "The outcome."

[prompt.work]
text = "Do the goal."

[procedure]
description = "One step."

[[procedure.sequence]]
id = "work"
title = "Work"
agent = "agent:worker"
prompt = "prompt:work"
input = ["port:goal", "port:dry-run"]
output = ["slot:result"]
"#;

    fn fixture() -> Trait {
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, FIXTURE)
            .expect("fixture decodes")
    }

    fn parse(tokens: &[&str]) -> crate::Result<Vec<StepSlotOutput>> {
        let tokens: Vec<String> = tokens.iter().map(|token| token.to_string()).collect();
        parse_trait_arguments(&fixture(), &tokens, "test")
    }

    #[test]
    fn bare_flag_sets_boolean_port_true() {
        let values = parse(&["--dry-run"]).expect("bare flag parses");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].ref_text, "port:dry-run");
        assert_eq!(values[0].value, serde_json::Value::Bool(true));
    }

    #[test]
    fn bare_flag_does_not_consume_the_next_argument() {
        let values = parse(&["--dry-run", "--goal", "ship it"]).expect("mixed args parse");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].value, serde_json::Value::Bool(true));
        assert_eq!(
            values[1].value,
            serde_json::Value::String("ship it".to_string())
        );
    }

    #[test]
    fn explicit_false_sets_boolean_port_false() {
        let values = parse(&["--dry-run=false"]).expect("explicit false parses");
        assert_eq!(values[0].value, serde_json::Value::Bool(false));
    }

    #[test]
    fn explicit_true_sets_boolean_port_true() {
        let values = parse(&["--dry-run=true"]).expect("explicit true parses");
        assert_eq!(values[0].value, serde_json::Value::Bool(true));
    }

    #[test]
    fn non_boolean_flag_value_is_refused() {
        let error = parse(&["--dry-run=maybe"]).expect_err("bad flag value refused");
        assert!(
            error.to_string().contains("--dry-run=false"),
            "error names the accepted forms: {error}"
        );
    }

    #[test]
    fn separate_value_after_flag_is_refused_naming_the_flag() {
        let error = parse(&["--dry-run", "true"]).expect_err("separate value refused");
        assert!(
            error.to_string().contains("takes no separate value"),
            "error explains flag form: {error}"
        );
    }

    #[test]
    fn unknown_bare_argument_reports_unknown_not_missing_value() {
        let error = parse(&["--nonsense"]).expect_err("unknown argument refused");
        assert!(
            error.to_string().contains("unknown trait argument"),
            "unknown port beats missing-value: {error}"
        );
    }

    /// The parsed Bool survives the runtime's initial-value schema gate:
    /// a bare flag lands in accepted-port-values as a real boolean, not a
    /// rejected attempt.
    #[test]
    fn parsed_flag_value_is_accepted_at_run_start() {
        let trait_ref = fixture();
        let values = parse(&["--dry-run", "--goal", "ship it"]).expect("args parse");
        let state = crate::procedure::runtime::start_procedure_run(
            &trait_ref,
            crate::procedure::run::Id::new("run-flag-shorthand-test").expect("run id"),
            values,
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
        )
        .expect("run starts");
        assert!(
            state.rejected_attempts.is_empty(),
            "no initial value rejected"
        );
        let flag = state
            .accepted_port_values
            .iter()
            .find(|value| value.ref_text == "port:dry-run")
            .expect("flag port accepted");
        assert_eq!(flag.value, serde_json::Value::Bool(true));
    }

    #[test]
    fn text_port_still_requires_a_value() {
        let error = parse(&["--goal"]).expect_err("text port without value refused");
        assert!(
            error.to_string().contains("requires a value"),
            "text shorthand unchanged: {error}"
        );
    }
}
