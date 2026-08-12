// Validates command argument interpolation contracts.
// Procedure command contract definitions.

// ---------------------------------------------------------------------------
// Command argv interpolation contract
// ---------------------------------------------------------------------------
//
// Mirrors `prompt_contract.rs` for `command` sequence items. Included into the
// `trait::procedure` module by `mod.rs`, so it shares that module's imports.

use crate::r#trait::Resource;

/// Validate a command step's `argv` interpolations against the same rules the
/// prompt interpolation contract enforces.
///
/// Each argv item is scanned with the shared [`scan_interpolations`] scanner.
/// For every interpolation that parses as a typed ref:
///
/// - the ref kind MUST be `slot`, `port`, `setting`, or `resource` — every
///   other kind is rejected;
/// - a `slot`/`port` ref MUST be local and unqualified — dependency
///   slot/port values are not resolvable at pure frame-build time — and MAY
///   appear anywhere within the argv element; the referenced value is
///   rendered into that one argv token (text/number/boolean keep their
///   scalar form, object/list/null values use compact JSON);
/// - a `setting` ref MUST name a declared setting (checked via the shared
///   [`resolve_setting_ref`] lookup, so an unknown id names itself in the
///   error the same way at every reference site) and is exempt from the
///   sequence item `input` list requirement below: settings are resolved at
///   activation, not accepted as step inputs, mirroring the loop-bound
///   branch's skip of the analogous port-only rule;
/// - a `resource` ref MUST be the entire argv element (`{resource:id}` with
///   nothing else in that element), local and unqualified, and MUST NOT
///   carry an input guard: a resource launched as a command argument is
///   verified against its declared pin at spawn time, in IO, not
///   interpolated here — permitting it embedded in a larger string or
///   behind a conditional input would blur containment and let dynamic text
///   be reinterpreted as a resource reference downstream;
/// - every non-`setting` ref MUST appear in the sequence item `input` list —
///   the same implicit-input rule prompt interpolation enforces.
///
/// Interpolation braces that do **not** parse as a typed ref (for example a
/// `jq` object filter such as `{name: .n}`) are left untouched: argv runs
/// without a shell, so those braces are literal and injection-safe.
pub(crate) fn validate_sequence_item_command_contract(
    _trait_ref: &Trait,
    item: &SequenceItem,
    argv: &[String],
    base: &str,
) -> crate::Result<()> {
    let item_input: BTreeSet<&str> = item.input.ref_texts().collect();
    for (index, arg) in argv.iter().enumerate() {
        let (interps, _diagnostics) = scan_interpolations(arg);
        for interp in &interps {
            let Ok(parsed) = Reference::parse(&interp.ref_text) else {
                // Not a typed ref; left literal (safe without a shell).
                continue;
            };
            let argv_path = format!("{base}.command.argv[{index}]");
            if !matches!(
                parsed.kind(),
                Kind::Slot | Kind::Port | Kind::Resource | Kind::Setting
            ) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: argv_path,
                    message: format!(
                        "argv interpolation {{{}}} kind {:?} not allowed; only slot, port, setting, or resource refs may be interpolated",
                        interp.ref_text,
                        parsed.kind()
                    ),
                }
                .into());
            }
            if parsed.kind() == Kind::Setting {
                resolve_setting_ref(_trait_ref, interp.ref_text.as_str(), &argv_path)?;
            }
            if parsed.is_qualified() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: argv_path,
                    message: format!(
                        "argv interpolation {{{}}} must be a local unqualified {} ref",
                        interp.ref_text,
                        argv_kind_label(parsed.kind())
                    ),
                }
                .into());
            }
            if parsed.kind() == Kind::Resource {
                let arg_char_len = arg.chars().count();
                if interp.start != 0 || interp.end != arg_char_len {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: argv_path,
                        message: format!(
                            "resource argv reference {{{}}} must be the entire argv element, not embedded in a larger string",
                            interp.ref_text
                        ),
                    }
                    .into());
                }
                if item.input.guard_for(interp.ref_text.as_str()).is_some() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.input"),
                        message: format!(
                            "command argv resource {{{}}} must not be a guarded (conditional) input",
                            interp.ref_text
                        ),
                    }
                    .into());
                }
            }
            if parsed.kind() != Kind::Setting && !item_input.contains(interp.ref_text.as_str()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.input"),
                    message: format!(
                        "command argv interpolation {{{}}} requires {:?} in sequence item input",
                        interp.ref_text, interp.ref_text
                    ),
                }
                .into());
            }
            if parsed.kind() != Kind::Setting && item.input.is_optional_for(interp.ref_text.as_str()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.input"),
                    message: format!(
                        "command argv interpolation {{{}}} requires {:?} unconditionally, but it is declared as an optional input; an absent slot would leave the argv token unresolved",
                        interp.ref_text, interp.ref_text
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn argv_kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Slot => "slot",
        Kind::Port => "port",
        Kind::Resource => "resource",
        Kind::Setting => "setting",
        _ => "unsupported",
    }
}

/// Whole-token `{resource:<id>}` positions in literal command argv: the
/// authoritative definition of what counts as a command-argv resource
/// reference, matching exactly what
/// [`validate_sequence_item_command_contract`] statically accepts. Runtime
/// frame-building and `ctx traits check`'s unpinned-argv scan both reuse this
/// one function rather than each re-implementing the whole-token/local rule,
/// so they can never disagree on which argv positions are resource refs.
pub(crate) fn whole_token_resource_argv_refs(argv: &[String]) -> Vec<(usize, String)> {
    let mut refs = Vec::new();
    for (index, arg) in argv.iter().enumerate() {
        let (interps, _diagnostics) = scan_interpolations(arg);
        if interps.len() != 1 {
            continue;
        }
        let interp = &interps[0];
        if interp.start != 0 || interp.end != arg.chars().count() {
            continue;
        }
        let Ok(parsed) = Reference::parse(&interp.ref_text) else {
            continue;
        };
        if parsed.kind() == Kind::Resource && !parsed.is_qualified() {
            refs.push((index, parsed.id().to_string()));
        }
    }
    refs
}

/// One authored command/check argv resource reference whose declared
/// resource carries no pin, found while walking every sequence item across
/// the trait's procedure and named sequences.
pub struct UnpinnedCommandResourceArgv {
    pub field_path: String,
    pub resource_ref: String,
}

/// Collect every unpinned resource reference used as command/check argv
/// across the trait's procedure and named sequences.
///
/// Only literal (non-`argv-from`) argv is scanned — dynamic argv is never a
/// resource reference (see [`whole_token_resource_argv_refs`]). A reference
/// to an undeclared resource is skipped here: `validate_resources`/sequence
/// input validation already reject that at decode time, so it cannot reach a
/// decoded `Trait`.
///
/// Retired (P516, 2026-07-26): letting a caller-supplied `argv-from` token
/// select which pinned resource runs would put a port value in control of
/// which authored executable spawns — the one use case, and speculative (no
/// trait in this repo does it, and a trait can only reference resources it
/// already declares, which it could reference literally instead). Runtime
/// frame-building now refuses any `{resource:x}`-shaped `argv-from` token
/// with a typed error instead of silently passing it through as inert text
/// (`frame_builders.rs`'s `command_frame`).
pub fn unpinned_command_resource_argv(trait_ref: &Trait) -> Vec<UnpinnedCommandResourceArgv> {
    let mut found = Vec::new();
    let mut visit = |item: &SequenceItem, base: &str| {
        let Ok(Some(plan)) = command_plan_for_item(item, base) else {
            return;
        };
        if plan.argv_from.is_some() {
            return;
        }
        for (index, resource_id) in whole_token_resource_argv_refs(&plan.argv) {
            let is_pinned = trait_ref
                .resources
                .iter()
                .find(|resource| resource.id == resource_id)
                .is_some_and(Resource::is_protected);
            if !is_pinned {
                found.push(UnpinnedCommandResourceArgv {
                    field_path: format!("{base}.command.argv[{index}]"),
                    resource_ref: format!("resource:{resource_id}"),
                });
            }
        }
    };

    if let Some(proc) = trait_ref.procedure.as_ref() {
        for (i, item) in proc.sequence.iter().enumerate() {
            visit(item, &format!("procedure.sequence[{i}]"));
        }
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        for (index, item) in sequence.sequence.iter().enumerate() {
            visit(item, &format!("sequence.{sequence_id}.sequence[{index}]"));
        }
    }

    found
}

#[cfg(test)]
mod command_contract_tests {
    use super::*;
    use crate::r#trait::{Setting, SettingSchema};

    fn minimal_trait_with_setting(setting: Setting) -> Trait {
        let mut trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            "id = \"command-contract-setting-test\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Command contract setting test\"\nsummary = \"Minimal fixture.\"\n",
        )
        .expect("minimal trait decodes");
        trait_ref.settings.push(setting);
        trait_ref
    }

    fn number_setting(id: &str) -> Setting {
        Setting {
            id: id.to_string(),
            schema: SettingSchema::Number,
            description: "A test setting.".to_string(),
            default: serde_json::json!(3),
            min: None,
            max: None,
        }
    }

    fn command_item() -> SequenceItem {
        toml::from_str(
            "id = \"run\"\ntitle = \"Run\"\nkind = \"command\"\ncmd = \"true\"\n",
        )
        .expect("test fixture must be a valid sequence item")
    }

    #[test]
    fn argv_setting_ref_to_a_declared_setting_builds() {
        let trait_ref = minimal_trait_with_setting(number_setting("review-rounds"));
        let item = command_item();
        let argv = vec!["--rounds".to_string(), "{setting:review-rounds}".to_string()];
        validate_sequence_item_command_contract(&trait_ref, &item, &argv, "procedure.sequence[0]")
            .expect("a setting ref naming a declared setting builds without requiring it in `input`");
    }

    #[test]
    fn argv_setting_ref_to_an_undeclared_setting_fails_naming_the_id() {
        let trait_ref = minimal_trait_with_setting(number_setting("review-rounds"));
        let item = command_item();
        let argv = vec!["{setting:not-declared}".to_string()];
        let error = validate_sequence_item_command_contract(
            &trait_ref,
            &item,
            &argv,
            "procedure.sequence[0]",
        )
        .expect_err("an undeclared setting id must fail the build");
        assert!(
            format!("{error}").contains("setting:not-declared"),
            "error must name the resolved id: {error}"
        );
    }
}
