// Validates procedure prompt contracts.
// Procedure prompt contract definitions.

/// Validate that every `signal:*` interpolation in prompt text names a
/// declared local signal.
///
/// Signal interpolations are exempt from the sequence-item `input` list
/// requirement (they resolve from emitted signal payloads at drive time, not
/// accepted step inputs — see [`PROMPT_REQUIRED_INPUT_KINDS`]), but an
/// undeclared or dependency-qualified base must still fail the build; this is
/// the only seam that can check that, since [`crate::trait::prompt::validate_prompts`]
/// sees only prompts and settings, not the trait's declared signals.
fn validate_prompt_signal_interpolations(
    trait_ref: &Trait,
    text: &str,
    field_path: &str,
) -> crate::Result<()> {
    let (interps, _) = scan_interpolations(text);
    for interp in &interps {
        let Ok(parsed) = Reference::parse(&interp.ref_text) else {
            continue;
        };
        if parsed.kind() != Kind::Signal {
            continue;
        }
        if parsed.is_qualified()
            || !trait_ref.signals.iter().any(|signal| signal.id == parsed.base_id())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!(
                    "signal interpolation {{{}}} requires a declared local signal {:?}",
                    interp.ref_text,
                    parsed.base_id()
                ),
            }
            .into());
        }
    }
    Ok(())
}

fn collect_prompt_required_inputs(prompt: &crate::r#trait::Prompt) -> Vec<String> {
    let mut required: Vec<String> = prompt.input.iter().cloned().collect();

    if let Some(text) = prompt.text.as_deref() {
        let (interps, _) = scan_interpolations(text);
        for interp in &interps {
            if let Ok(r) = Reference::parse(&interp.ref_text)
                && PROMPT_REQUIRED_INPUT_KINDS.contains(&r.kind()) {
                    required.push(interp.ref_text.clone());
                }
        }
    }
    if let Some(source) = prompt.source.as_deref() {
        required.push(source.to_string());
    }

    required
}

/// Validate prompt contract satisfaction for a single sequence item.
///
/// For a referenced prompt (`prompt:<id>`): require the prompt to exist
/// locally, then check that explicit `prompt.input` refs and interpolation
/// refs are present in the sequence item's `input` list, and that
/// `prompt.output` slot refs are present in the sequence item's `output` list.
///
/// For an inline prompt: check that interpolation refs are present in the
/// sequence item's `input` list.
///
/// All prompt-required refs of kind `port`, `slot`, or `resource` must be
/// listed exactly in the sequence item's input, regardless of whether the ref
/// is local or dependency-qualified.
///
/// Prompt output refs of kind `slot` must be present exactly in the sequence
/// item's output regardless of whether they are local or dependency-qualified.
///
/// Dependency-qualified prompt refs (`prompt:dep/id`) remain pending —
/// their contract cannot be checked without loaded dependency contents.
pub(crate) fn validate_sequence_item_prompt_contract(
    trait_ref: &Trait,
    base: &str,
    item_id: Option<&str>,
    item: &SequenceItem,
    prompts: &PromptMap,
) -> crate::Result<()> {
    let id_suffix = item_id.map(|id| format!(" (id={id})")).unwrap_or_default();
    let base = format!("{base}{id_suffix}");
    let item_input: BTreeSet<&str> = item.input.ref_texts().collect();
    let item_output: BTreeSet<&str> = item.output.ref_texts().collect();

    let require_unconditional_input = |ref_text: &str| -> crate::Result<()> {
        if !item_input.contains(ref_text) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input"),
                message: format!(
                    "prompt interpolation {{{ref_text}}} requires {ref_text:?} in sequence item input"
                ),
            }
            .into());
        }
        if item.input.guard_for(ref_text).is_some() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input"),
                message: format!(
                    "prompt requires {ref_text:?} unconditionally, but it is declared as a guarded input; a false guard would leave the interpolation unresolved"
                ),
            }
            .into());
        }
        // An OPTIONAL input may be interpolated (owner ruling 2026-08-11):
        // the frame renderer substitutes accepted scalar values and passes an
        // unmatched token through as a literal reference, and absent optional
        // inputs simply have no frame element — a reference to something not
        // present, never an unresolved token (see frame_prompt.rs). Only a
        // GUARDED input stays rejected above: its false-guard absence is a
        // wiring condition the prompt text cannot see.
        Ok(())
    };

    // A `setting:` interpolation is resolved against declared settings (the
    // "unknown id names itself the same way at every reference site" Watch
    // item) but exempt from the sequence-item `input` list requirement above:
    // settings are activation-resolved, not accepted step inputs, mirroring
    // the loop-bound branch's skip of the analogous port-only rule.
    let require_input_or_setting = |ref_text: &str| -> crate::Result<()> {
        if let Ok(parsed) = Reference::parse(ref_text)
            && parsed.kind() == Kind::Setting
        {
            return crate::r#trait::prompt::validate_setting_ref_exists(
                &parsed,
                ref_text,
                &trait_ref.settings,
                &format!("{base}.input"),
            );
        }
        require_unconditional_input(ref_text)
    };

    match classify_prompt(&item.prompt) {
        Ok(PromptClassification::DependencyPromptRef(_)) => Ok(()),

        Ok(PromptClassification::Inline) => {
            let (interps, _) = scan_interpolations(&item.prompt);
            for interp in &interps {
                if let Ok(r) = Reference::parse(&interp.ref_text)
                    && PROMPT_REQUIRED_INPUT_KINDS.contains(&r.kind()) {
                        require_input_or_setting(&interp.ref_text)?;
                    }
            }
            validate_prompt_signal_interpolations(trait_ref, &item.prompt, &format!("{base}.prompt"))?;
            Ok(())
        }

        Ok(PromptClassification::LocalPromptRef(parsed)) => {
            let ref_text = parsed.to_string();
            let prompt_id = parsed.id();
            let Some(prompt) = prompts.get(prompt_id) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.prompt"),
                    message: format!("unresolved local prompt ref {:?}", ref_text),
                }
                .into());
            };

            for req in collect_prompt_required_inputs(prompt) {
                require_input_or_setting(&req)?;
            }
            if let Some(text) = prompt.text.as_deref() {
                validate_prompt_signal_interpolations(trait_ref, text, &format!("{base}.prompt"))?;
            }

            for output_ref in prompt.output.iter() {
                if !item_output.contains(output_ref.as_str()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.output"),
                        message: format!(
                            "prompt {:?} requires {:?} in sequence item output",
                            ref_text, output_ref
                        ),
                    }
                    .into());
                }
            }

            Ok(())
        }

        Err(msg) => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.prompt"),
            message: msg,
        }
        .into()),
    }
}

#[cfg(test)]
mod prompt_contract_setting_tests {
    use crate::encoding::{Encoding, decode_trait};

    const HEADER: &str = r#"
id = "prompt-contract-setting-test"
schema-version = "0.3"
version = "0.1.0"
name = "Prompt contract setting test"
summary = "Minimal fixture."

[[setting]]
id = "review-rounds"
schema = "number"
description = "Rounds."
default = 3
"#;

    #[test]
    fn inline_prompt_interpolating_a_declared_setting_builds_without_requiring_input() {
        let text = format!(
            "{HEADER}\n[procedure]\ndescription = \"Go.\"\n\n[[procedure.sequence]]\nid = \"go\"\ntitle = \"Go\"\nkind = \"prompt\"\nprompt = \"Do {{setting:review-rounds}} rounds.\"\n"
        );
        decode_trait(Encoding::Toml, &text)
            .expect("a setting interpolation builds without being listed in `input`");
    }

    #[test]
    fn inline_prompt_interpolating_an_undeclared_setting_fails_naming_the_id() {
        let text = format!(
            "{HEADER}\n[procedure]\ndescription = \"Go.\"\n\n[[procedure.sequence]]\nid = \"go\"\ntitle = \"Go\"\nkind = \"prompt\"\nprompt = \"Do {{setting:not-declared}} rounds.\"\n"
        );
        let err = decode_trait(Encoding::Toml, &text)
            .expect_err("an undeclared setting id must fail the build");
        assert!(
            err.to_string().contains("setting:not-declared"),
            "error must name the resolved id: {err}"
        );
    }
}

#[cfg(test)]
mod prompt_contract_signal_tests {
    use crate::encoding::{Encoding, decode_trait};

    const HEADER: &str = r#"
id = "prompt-contract-signal-test"
schema-version = "0.3"
version = "0.1.0"
name = "Prompt contract signal test"
summary = "Minimal fixture."

[[signal]]
id = "needs-owner"
description = "Owner input is needed."
schema = "schema:owner-payload"

[[schema]]
id = "owner-payload"

[schema.fields.question]
schema = "schema:text"
required = true
"#;

    #[test]
    fn inline_prompt_interpolating_a_declared_signal_field_builds_without_requiring_input() {
        let text = format!(
            "{HEADER}\n[procedure]\ndescription = \"Go.\"\n\n[[procedure.sequence]]\nid = \"go\"\ntitle = \"Go\"\nkind = \"prompt\"\nprompt = \"{{signal:needs-owner.question}}\"\n"
        );
        decode_trait(Encoding::Toml, &text)
            .expect("a signal payload-field interpolation builds without being listed in `input`");
    }

    #[test]
    fn inline_prompt_interpolating_an_undeclared_signal_base_fails_naming_the_id() {
        let text = format!(
            "{HEADER}\n[procedure]\ndescription = \"Go.\"\n\n[[procedure.sequence]]\nid = \"go\"\ntitle = \"Go\"\nkind = \"prompt\"\nprompt = \"{{signal:not-declared.question}}\"\n"
        );
        let err = decode_trait(Encoding::Toml, &text)
            .expect_err("an undeclared signal base must fail the build");
        assert!(
            err.to_string().contains("signal:not-declared.question"),
            "error must name the resolved ref: {err}"
        );
    }

    #[test]
    fn referenced_prompt_interpolating_a_declared_signal_field_builds() {
        let text = format!(
            "{HEADER}\n[prompt.ask]\ntext = \"{{signal:needs-owner.question}}\"\n\n[procedure]\ndescription = \"Go.\"\n\n[[procedure.sequence]]\nid = \"go\"\ntitle = \"Go\"\nkind = \"prompt\"\nprompt = \"prompt:ask\"\n"
        );
        decode_trait(Encoding::Toml, &text)
            .expect("a referenced prompt's signal payload-field interpolation builds");
    }
}
