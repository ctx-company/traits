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
        if item.input.is_optional_for(ref_text) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input"),
                message: format!(
                    "prompt requires {ref_text:?} unconditionally, but it is declared as an optional input; an absent slot would leave the interpolation unresolved"
                ),
            }
            .into());
        }
        Ok(())
    };

    match classify_prompt(&item.prompt) {
        Ok(PromptClassification::DependencyPromptRef(_)) => Ok(()),

        Ok(PromptClassification::Inline) => {
            let (interps, _) = scan_interpolations(&item.prompt);
            for interp in &interps {
                if let Ok(r) = Reference::parse(&interp.ref_text)
                    && PROMPT_REQUIRED_INPUT_KINDS.contains(&r.kind()) {
                        require_unconditional_input(&interp.ref_text)?;
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
                require_unconditional_input(&req)?;
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
