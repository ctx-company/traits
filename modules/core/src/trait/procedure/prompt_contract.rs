// Validates procedure prompt contracts.
// Procedure prompt contract definitions.

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
