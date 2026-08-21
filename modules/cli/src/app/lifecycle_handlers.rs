//! CLI lifecycle transition handlers.
//!
//! Status and trust are owned by separate stores (Group 95, 2026-07-19):
//! `activate`/`deactivate` edit the package manifest's `[package].status`
//! (`draft | ready`); `ctx traits trust --approved/block` (and the hidden
//! `review --approve/--deny` compatibility alias, see
//! [`crate::app::lifecycle_reporting::handle_trust_named_update`]) records a
//! verdict in the machine trust store (`~/.config/ctx/trust.toml`) keyed by
//! the trait's current canonical digest. Neither command ever mutates the
//! canonical trait document.

use ctx_traits_core::response::CommandOutput;

use crate::app::entry::print_json_report;
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct LifecycleTransitionOutput<'a> {
    action: &'a str,
    trait_id: &'a str,
    previous_status: &'a str,
    status: &'a str,
    trust: &'a str,
    next_action: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct LifecycleStatusOutput<'a> {
    trait_id: &'a str,
    status: &'a str,
    trust: &'a str,
    next_action: &'a str,
}

pub(crate) enum LifecycleAction<'a> {
    Activate,
    Deactivate,
    /// No `deprecated` status exists in the two-state (`draft | ready`)
    /// package model; `deprecate` is a `deactivate` alias that also prints
    /// the supplied reason.
    Deprecate {
        reason: Option<&'a str>,
    },
}

fn extract_package_id_from_path(path: &camino::Utf8Path) -> Option<&str> {
    let raw = path.as_str();
    if raw.is_empty()
        || raw.contains('\\')
        || raw.contains("//")
        || path.components().any(|component| {
            !matches!(
                component,
                camino::Utf8Component::RootDir
                    | camino::Utf8Component::Normal(_)
                    // The shared id-or-path resolver (`resolve_trait_target`,
                    // used by `review`/`check`/`list`/`activate`/`deactivate`
                    // alike) joins a bare id onto `Utf8Path::new(".")`, which
                    // leaves a leading `./` component on the resolved path.
                    // That's a normalization artifact, not a meaningful path
                    // segment, so it must not disqualify an otherwise-valid
                    // `.ctx/traits/<id>/generated/index.toml` path.
                    | camino::Utf8Component::CurDir
            )
        })
    {
        return None;
    }
    let normals: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            camino::Utf8Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    if !path.is_absolute() {
        return package_id_from_normals(&normals);
    }
    let ctx_positions: Vec<usize> = normals
        .iter()
        .enumerate()
        .filter_map(|(index, component)| (*component == ".ctx").then_some(index))
        .collect();
    if ctx_positions.len() != 1 {
        return None;
    }
    let tail = normals.get(ctx_positions[0]..)?;
    package_id_from_normals(tail)
}

/// The package id in a canonical-manifest path, under either package root.
///
/// P569 moved packages from `.ctx/traits/<id>` to `.ctx/traits/packages/<id>`
/// (0179: `authored/<id>`); both shapes are accepted so a checkout that
/// predates the move can still be activated.
fn package_id_from_normals<'a>(normals: &[&'a str]) -> Option<&'a str> {
    let id = match normals {
        [
            ".ctx",
            "traits",
            "authored",
            id,
            "generated",
            "trait.toml" | "index.toml",
        ] => id,
        [
            ".ctx",
            "traits",
            id,
            "generated",
            "trait.toml" | "index.toml",
        ] => id,
        _ => return None,
    };
    (!id.is_empty()).then_some(*id)
}

/// Bare `ctx traits internal state <trait>`: report the lifecycle state and the
/// machine trust standing beside it, and change nothing.
///
/// Trust is reported here because status alone never makes a trait runnable
/// — the two gates are independent, and a report that showed only one of
/// them would answer "can I run this?" wrongly half the time.
pub(crate) fn handle_lifecycle_status(file: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, trait_root, _source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;
    let status = ctx_traits_io::lifecycle::resolve_package_status(&trait_root)?;
    let status_display = status.display_name().to_string();
    let trust = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;

    let next_action = match (status, trust) {
        (ctx_traits_core::manifest::PackageStatus::Draft, _) => {
            "draft: run `ctx traits internal state --active <trait>` to make it resolver-eligible"
                .to_string()
        }
        (
            ctx_traits_core::manifest::PackageStatus::Ready,
            ctx_traits_core::r#trait::TrustVerdict::Verified,
        ) => "active and machine-trusted".to_string(),
        (ctx_traits_core::manifest::PackageStatus::Ready, _) => {
            let trust_gates = ctx_traits_core::r#trait::activation::trust_gates_for_check(
                trait_ref.id.as_str(),
                &trust,
            );
            format!(
                "status is ready, but this machine has not approved the current canonical \
                 digest; {}",
                ctx_traits_core::r#trait::activation::format_gate_refusal(&trust_gates)
            )
        }
    };

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = LifecycleStatusOutput {
                trait_id: trait_ref.id.as_str(),
                status: &status_display,
                trust: trust.display_name(),
                next_action: next_action.as_str(),
            };
            print_json_report(&output, "lifecycle status")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new("ctx", "state", PanelStatus::Passed(status_display.clone()))
                .row(PanelRow::toned(
                    "trait",
                    trait_ref.id.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "status",
                    status_display.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "trust",
                    trust.display_name(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "next",
                    next_action.as_str(),
                    RowTone::Default,
                ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// `ctx traits internal state --active`/`--draft`/`--deprecated`: edits the package
/// manifest's `[package].status`. Never touches the canonical trait
/// document.
pub(crate) fn handle_lifecycle_transition(
    file: &str,
    action: LifecycleAction,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    use ctx_traits_core::manifest::PackageStatus;

    let path = camino::Utf8Path::new(file);
    let pkg_id = extract_package_id_from_path(path).ok_or_else(|| crate::Error::Command {
        message: format!(
            "lifecycle transition refused: target must be {}/<id>/generated/index.toml",
            ctx_traits_io::layout::trait_protocol_root()
        ),
    })?;

    let (trait_ref, trait_root, _source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;

    if pkg_id != trait_ref.id.as_str() {
        return Err(crate::Error::Command {
            message: format!(
                "lifecycle transition refused: path package ID '{pkg_id}' does not match decoded trait ID '{}'",
                trait_ref.id.as_str()
            ),
        });
    }

    let prev_status = ctx_traits_io::lifecycle::resolve_package_status(&trait_root)?;
    let prev_status_display = prev_status.display_name().to_string();

    let (new_status, action_name, deprecate_reason) = match &action {
        LifecycleAction::Activate => {
            if prev_status == PackageStatus::Ready {
                return Err(crate::Error::Command {
                    message: "trait is already active (package status is already ready)"
                        .to_string(),
                });
            }
            (PackageStatus::Ready, "activate", None)
        }
        LifecycleAction::Deactivate => {
            if prev_status == PackageStatus::Draft {
                return Err(crate::Error::Command {
                    message: "trait is already inactive (package status is already draft)"
                        .to_string(),
                });
            }
            (PackageStatus::Draft, "deactivate", None)
        }
        LifecycleAction::Deprecate { reason } => {
            if prev_status == PackageStatus::Draft {
                return Err(crate::Error::Command {
                    message: "trait is already inactive (package status is already draft)"
                        .to_string(),
                });
            }
            (PackageStatus::Draft, "deprecate", *reason)
        }
    };

    if let Some(r) = deprecate_reason {
        eprintln!("  deprecation reason: {r}");
    }

    ctx_traits_io::lifecycle::write_package_status(&trait_root, new_status)?;

    let new_status_display = new_status.display_name().to_string();
    let trust = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;
    let trust_display = trust.display_name();

    // `activate` clears only the status gate; trust is an independent,
    // machine-local gate that this command never touches (Group 95). Say so
    // explicitly rather than implying the trait is runnable on status alone,
    // naming the exact remedy from the same gate builder every other refusal
    // surface uses so wording never drifts.
    let next_action = match (new_status, trust) {
        (PackageStatus::Draft, _) => "review and activate before use".to_string(),
        (PackageStatus::Ready, ctx_traits_core::r#trait::TrustVerdict::Verified) => {
            "active and machine-trusted; deactivate when no longer needed".to_string()
        }
        (PackageStatus::Ready, _) => {
            let trust_gates = ctx_traits_core::r#trait::activation::trust_gates_for_check(
                trait_ref.id.as_str(),
                &trust,
            );
            format!(
                "status is ready, but this machine has not approved the current canonical \
                 digest; {}",
                ctx_traits_core::r#trait::activation::format_gate_refusal(&trust_gates)
            )
        }
    };
    let next_action = next_action.as_str();

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = LifecycleTransitionOutput {
                action: action_name,
                trait_id: trait_ref.id.as_str(),
                previous_status: &prev_status_display,
                status: &new_status_display,
                trust: trust_display,
                next_action,
            };
            print_json_report(&output, "lifecycle transition")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new(
                "ctx",
                action_name,
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "trait",
                trait_ref.id.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "previous-status",
                prev_status_display.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "status",
                new_status_display.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned("trust", trust_display, RowTone::Default))
            .next(PanelRow::toned("next", next_action, RowTone::Default));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}
