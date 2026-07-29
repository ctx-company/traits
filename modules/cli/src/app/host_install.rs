//! CLI orchestration for the P441 host-placement lifecycle
//! (`host-install`/`host-update`/`host-remove`).
//!
//! Rendering happens here (via [`crate::app::report_render::build_export_artifact`]);
//! every placement/manifest/lock/audit mechanic — including the
//! install/update/remove transaction and its required audit event — is
//! delegated to `ctx_traits_io::host_install`, which never renders.

use crate::app::command_handlers::{print_json_report, resolve_trait_target};
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};
use crate::app::report_render::{ExportArtifact, RenderTrustPosture, build_export_artifact};
use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::host_install::{ArtifactInput, Scope};

fn host_install_error(error: ctx_traits_io::host_install::Error) -> crate::Error {
    crate::Error::Command {
        message: error.to_string(),
    }
}

fn scope_and_paths(
    global: bool,
) -> crate::Result<(Scope, camino::Utf8PathBuf, camino::Utf8PathBuf)> {
    if global {
        let root = ctx_traits_io::state::home_dir()?;
        let manifest = ctx_traits_io::state::global_host_placements_manifest_path()?;
        Ok((Scope::Global, root, manifest))
    } else {
        let root = ctx_traits_io::repository::discover_repo_root()?;
        let manifest = ctx_traits_io::layout::project_host_placements_manifest_path(&root);
        Ok((Scope::Project, root, manifest))
    }
}

fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Normalize a trait file locator to an absolute, canonical path before it
/// is persisted as a placement's stable source. A `--file` argument (or a
/// resolved package-shadow/built-in path) is otherwise relative to the
/// current working directory at install time; persisting it as-is means
/// `host-update` can only reload the source when invoked from that same
/// directory. Canonicalizing once here — the locator's underlying file is
/// already known to exist, since [`build_export_artifact`] just loaded it —
/// makes the recorded source resolvable from any working directory.
fn canonicalize_source_locator(file_locator: &str) -> crate::Result<String> {
    let path = camino::Utf8Path::new(file_locator);
    let absolute =
        std::fs::canonicalize(path.as_std_path()).map_err(|source| crate::Error::Command {
            message: format!("cannot resolve a stable source path for {file_locator:?}: {source}"),
        })?;
    camino::Utf8PathBuf::from_path_buf(absolute)
        .map(|path| path.to_string())
        .map_err(|_| crate::Error::Command {
            message: format!("trait source path {file_locator:?} is not valid UTF-8"),
        })
}

fn audit_root() -> crate::Result<camino::Utf8PathBuf> {
    ctx_traits_io::state::global_audit_root().map_err(crate::Error::from)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct InstallJsonOutput<'a> {
    trait_id: &'a str,
    host: &'a str,
    scope: &'a str,
    reinstalled: bool,
    path: &'a str,
    content_digest: &'a str,
    archive_path: Option<&'a str>,
    resource_manifest_digest: Option<&'a str>,
    export_partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle_advisory: Option<&'a [ctx_traits_core::r#trait::activation::Gate]>,
}

pub(crate) struct InstallInputs<'a> {
    pub(crate) trait_arg: Option<&'a str>,
    pub(crate) file: Option<&'a str>,
    pub(crate) host: &'a str,
    pub(crate) global: bool,
    pub(crate) format: Option<&'a str>,
    pub(crate) archive: Option<&'a str>,
    pub(crate) allow_unreviewed: bool,
    pub(crate) allow_draft: bool,
    pub(crate) json: bool,
}

pub(crate) fn handle_install(inputs: InstallInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let InstallInputs {
        trait_arg,
        file,
        host,
        global,
        format,
        archive,
        allow_unreviewed,
        allow_draft,
        json,
    } = inputs;
    let file_locator = resolve_trait_target(trait_arg, file, "host-install")?;
    let overrides =
        ctx_traits_io::harness_config::resolve_runtime_config(camino::Utf8Path::new("."))?.host;
    let mut spec = ctx_traits_io::host_install::resolve_host_spec(host, &overrides)
        .map_err(host_install_error)?;
    if let Some(format) = format {
        spec.format = match ctx_traits_io::export::Format::parse(format) {
            Some(ctx_traits_io::export::Format::Stub) => ctx_traits_io::export::Format::Stub,
            Some(ctx_traits_io::export::Format::Skill) => ctx_traits_io::export::Format::Skill,
            _ => {
                return Err(crate::Error::Command {
                    message: format!("unsupported --format: {format:?} (expected stub or skill)"),
                });
            }
        };
    }

    let posture = RenderTrustPosture::host_install(allow_unreviewed, allow_draft, json);
    let ExportArtifact {
        trait_ref,
        source_digest,
        canonical_digest,
        content,
        resource_manifest_digest,
        unplaced_resource_count,
        companions,
        lifecycle_advisory,
        ..
    } = build_export_artifact(&file_locator, spec.profile, spec.format, &posture)?;
    let source = canonicalize_source_locator(&file_locator)?;

    let content = match ctx_traits_io::host_install::builtin_content_frontmatter(host) {
        Some(frontmatter) => format!("{frontmatter}\n\n{content}"),
        None => content,
    };

    let (scope, root, manifest_path) = scope_and_paths(global)?;
    let template =
        ctx_traits_io::host_install::target_template(&spec, global).map_err(host_install_error)?;
    let relative_target =
        ctx_traits_io::host_install::resolve_template(host, template, trait_ref.id.as_str())
            .map_err(host_install_error)?;

    let input = ArtifactInput {
        trait_id: trait_ref.id.as_str(),
        source: &source,
        profile: spec.profile,
        format: spec.format,
        source_digest: &source_digest,
        canonical_digest: &canonical_digest,
        content: &content,
        companions: &companions,
    };
    let outcome = ctx_traits_io::host_install::install(
        ctx_traits_io::host_install::InstallRequest {
            manifest_path: &manifest_path,
            scope,
            root: &root,
            host,
            relative_target: &relative_target,
            audit_root: &audit_root()?,
            unix_seconds: unix_seconds_now(),
            archive_path: archive,
        },
        &input,
    )
    .map_err(host_install_error)?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = InstallJsonOutput {
                trait_id: trait_ref.id.as_str(),
                host,
                scope: scope.as_str(),
                reinstalled: outcome.reinstalled,
                path: outcome.artifact.path.as_str(),
                content_digest: &outcome.artifact.content_digest,
                archive_path: archive,
                resource_manifest_digest: resource_manifest_digest.as_deref(),
                export_partial: unplaced_resource_count > 0,
                lifecycle_advisory: (!lifecycle_advisory.is_empty())
                    .then_some(&lifecycle_advisory[..]),
            };
            print_json_report(&output, "host-install output")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                "host-install",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "trait",
                trait_ref.id.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned("host", host, RowTone::Default))
            .row(PanelRow::toned("scope", scope.as_str(), RowTone::Default))
            .row(PanelRow::toned(
                "action",
                if outcome.reinstalled {
                    "reinstalled"
                } else {
                    "installed"
                },
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "path",
                outcome.artifact.path.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "digest",
                outcome.artifact.content_digest.as_str(),
                RowTone::Default,
            ));
            if let Some(archive_path) = archive {
                panel = panel.row(PanelRow::toned("archive", archive_path, RowTone::Default));
            }
            if unplaced_resource_count > 0 {
                eprintln!(
                    "warning: {unplaced_resource_count} declared resource(s) could not be placed \
                     alongside {} (binary, missing, symlinked, or inline)",
                    outcome.artifact.path
                );
                panel = panel.row(PanelRow::toned(
                    "export-partial",
                    format!("true ({unplaced_resource_count} resource(s) not placed)"),
                    RowTone::Fail,
                ));
            }
            if !lifecycle_advisory.is_empty() {
                panel = panel.row(PanelRow::toned(
                    "lifecycle-advisory",
                    ctx_traits_core::r#trait::activation::format_gate_refusal(&lifecycle_advisory),
                    RowTone::Fail,
                ));
            }
            panel = panel.row(PanelRow::toned(
                "resource-manifest-digest",
                resource_manifest_digest.as_deref().unwrap_or("none"),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct UpdateEntryJsonOutput {
    host: String,
    trait_id: String,
    outcome: String,
    path: Option<String>,
    content_digest: Option<String>,
    error: Option<String>,
}

pub(crate) fn handle_update(
    global: bool,
    force: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (scope, root, manifest_path) = scope_and_paths(global)?;
    let records =
        ctx_traits_io::host_install::list_placements(&manifest_path).map_err(host_install_error)?;
    let audit_root = audit_root()?;

    let mut reports = Vec::with_capacity(records.len());
    for record in records {
        let host = record.host.clone();
        let trait_id = record.trait_id.clone();
        let entry = (|| -> crate::Result<UpdateEntryJsonOutput> {
            let profile = ctx_traits_core::render::ExtendedRenderProfile::parse(&record.profile)
                .ok_or_else(|| crate::Error::Command {
                    message: format!(
                        "recorded profile {:?} is no longer recognized",
                        record.profile
                    ),
                })?;
            let format = ctx_traits_io::export::Format::parse(&record.format).ok_or_else(|| {
                crate::Error::Command {
                    message: format!(
                        "recorded format {:?} is no longer recognized",
                        record.format
                    ),
                }
            })?;
            let posture = RenderTrustPosture::host_update();
            let artifact = build_export_artifact(&record.source, profile, format, &posture)?;
            let content = match ctx_traits_io::host_install::builtin_content_frontmatter(&host) {
                Some(frontmatter) => format!("{frontmatter}\n\n{}", artifact.content),
                None => artifact.content.clone(),
            };
            let input = ArtifactInput {
                trait_id: &trait_id,
                source: &record.source,
                profile,
                format,
                source_digest: &artifact.source_digest,
                canonical_digest: &artifact.canonical_digest,
                content: &content,
                companions: &artifact.companions,
            };
            match ctx_traits_io::host_install::apply_update(
                &manifest_path,
                &root,
                &record,
                &input,
                &audit_root,
                unix_seconds_now(),
                force,
            )
            .map_err(host_install_error)?
            {
                ctx_traits_io::host_install::UpdateOutcome::Skipped { .. } => {
                    Ok(UpdateEntryJsonOutput {
                        host: host.clone(),
                        trait_id: trait_id.clone(),
                        outcome: "current".to_string(),
                        path: None,
                        content_digest: None,
                        error: None,
                    })
                }
                ctx_traits_io::host_install::UpdateOutcome::LocallyModified { .. } => {
                    Ok(UpdateEntryJsonOutput {
                        host: host.clone(),
                        trait_id: trait_id.clone(),
                        outcome: "locally-modified".to_string(),
                        path: None,
                        content_digest: None,
                        error: Some(
                            "a recorded path was locally modified since it was placed; pass --force to overwrite"
                                .to_string(),
                        ),
                    })
                }
                ctx_traits_io::host_install::UpdateOutcome::Updated { artifact, .. } => {
                    Ok(UpdateEntryJsonOutput {
                        host: host.clone(),
                        trait_id: trait_id.clone(),
                        outcome: "updated".to_string(),
                        path: Some(artifact.path.to_string()),
                        content_digest: Some(artifact.content_digest),
                        error: None,
                    })
                }
            }
        })()
        .unwrap_or_else(|error| UpdateEntryJsonOutput {
            host: host.clone(),
            trait_id: trait_id.clone(),
            outcome: "error".to_string(),
            path: None,
            content_digest: None,
            error: Some(error.to_string()),
        });
        reports.push(entry);
    }

    let failed = reports
        .iter()
        .filter(|entry| entry.outcome == "error")
        .count();

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&reports, "host-update output")?;
        }
        OutputMode::Human(mode) => {
            let status = if failed > 0 {
                PanelStatus::Blocked("blocked".to_string())
            } else {
                PanelStatus::Passed("passed".to_string())
            };
            let mut panel = Panel::new("ctx", "host-update", status).row(PanelRow::toned(
                "scope",
                scope.as_str(),
                RowTone::Default,
            ));
            for entry in &reports {
                let mut value = format!("outcome={}", entry.outcome);
                if let Some(path) = &entry.path {
                    value.push_str(&format!(" path={path}"));
                }
                if let Some(digest) = &entry.content_digest {
                    value.push_str(&format!(" digest={digest}"));
                }
                if let Some(error) = &entry.error {
                    value.push_str(&format!(" error={error}"));
                }
                panel = panel.row(PanelRow::toned(
                    format!("{} {}", entry.host, entry.trait_id),
                    value,
                    if matches!(entry.outcome.as_str(), "error" | "locally-modified") {
                        RowTone::Fail
                    } else {
                        RowTone::Default
                    },
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    if failed > 0 {
        return Err(crate::Error::Command {
            message: format!(
                "host-update: {failed} of {} recorded placement(s) failed to rebuild",
                reports.len()
            ),
        });
    }

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct StatusEntryJsonOutput {
    host: String,
    trait_id: String,
    outcome: String,
    detail: Option<String>,
}

/// Read-only drift/ownership report for every recorded placement in a
/// manifest, without mutating anything (`host update` is the verb that
/// fixes what this names). Per record: `current` (on disk, matches, and a
/// fresh render agrees), `stale-source` (on disk, matches the record, but a
/// fresh render differs — `host update` will rewrite), `locally-modified`
/// (a recorded path's bytes no longer match its recorded digest — a human
/// edited a generated file), `missing` (a recorded path is gone),
/// `unmanaged`/`ownership-mismatch` (the leaf's marker is gone or owned by
/// another trait), or `error` (source no longer loadable, or now
/// trust-blocked/unreviewed/draft — reported the same way `host update`
/// reports it).
pub(crate) fn handle_status(global: bool, json: bool) -> crate::Result<CommandOutput<()>> {
    let (scope, root, manifest_path) = scope_and_paths(global)?;
    let records =
        ctx_traits_io::host_install::list_placements(&manifest_path).map_err(host_install_error)?;

    let mut reports = Vec::with_capacity(records.len());
    for record in &records {
        let host = record.host.clone();
        let trait_id = record.trait_id.clone();
        let entry = (|| -> crate::Result<StatusEntryJsonOutput> {
            let path_states = ctx_traits_io::host_install::inspect_placement_paths(&root, record)
                .map_err(host_install_error)?;
            if path_states.iter().any(|state| {
                matches!(
                    state,
                    ctx_traits_io::host_install::PathState::LocallyModified
                )
            }) {
                return Ok(StatusEntryJsonOutput {
                    host: host.clone(),
                    trait_id: trait_id.clone(),
                    outcome: "locally-modified".to_string(),
                    detail: None,
                });
            }
            if path_states
                .iter()
                .any(|state| matches!(state, ctx_traits_io::host_install::PathState::Missing))
            {
                return Ok(StatusEntryJsonOutput {
                    host: host.clone(),
                    trait_id: trait_id.clone(),
                    outcome: "missing".to_string(),
                    detail: None,
                });
            }
            if let Some(leaf_state) = path_states.first() {
                let outcome = match leaf_state {
                    ctx_traits_io::host_install::PathState::UnmanagedTarget => Some("unmanaged"),
                    // Distinct from plain `unmanaged`: these name exactly
                    // what a user needs to act on (a symlink to remove, or
                    // a non-regular file blocking the leaf) rather than
                    // collapsing both into one generic outcome.
                    ctx_traits_io::host_install::PathState::LeafSymlink => Some("leaf-symlink"),
                    ctx_traits_io::host_install::PathState::LeafNotRegularFile => {
                        Some("leaf-not-regular-file")
                    }
                    ctx_traits_io::host_install::PathState::OwnershipMismatch => {
                        Some("ownership-mismatch")
                    }
                    _ => None,
                };
                if let Some(outcome) = outcome {
                    return Ok(StatusEntryJsonOutput {
                        host: host.clone(),
                        trait_id: trait_id.clone(),
                        outcome: outcome.to_string(),
                        detail: None,
                    });
                }
            }

            let profile = ctx_traits_core::render::ExtendedRenderProfile::parse(&record.profile)
                .ok_or_else(|| crate::Error::Command {
                    message: format!(
                        "recorded profile {:?} is no longer recognized",
                        record.profile
                    ),
                })?;
            let format = ctx_traits_io::export::Format::parse(&record.format).ok_or_else(|| {
                crate::Error::Command {
                    message: format!(
                        "recorded format {:?} is no longer recognized",
                        record.format
                    ),
                }
            })?;
            let posture = RenderTrustPosture::host_update();
            let artifact = build_export_artifact(&record.source, profile, format, &posture)?;
            let content = match ctx_traits_io::host_install::builtin_content_frontmatter(&host) {
                Some(frontmatter) => format!("{frontmatter}\n\n{}", artifact.content),
                None => artifact.content.clone(),
            };
            let fresh_digest = ctx_traits_core::digest::Digest::from_bytes(content.as_bytes());
            let recorded_leaf_digest = record.content_digests.first().map(String::as_str);
            let outcome = if Some(fresh_digest.as_str()) == recorded_leaf_digest {
                "current"
            } else {
                "stale-source"
            };
            Ok(StatusEntryJsonOutput {
                host: host.clone(),
                trait_id: trait_id.clone(),
                outcome: outcome.to_string(),
                detail: None,
            })
        })()
        .unwrap_or_else(|error| StatusEntryJsonOutput {
            host: host.clone(),
            trait_id: trait_id.clone(),
            outcome: "error".to_string(),
            detail: Some(error.to_string()),
        });
        reports.push(entry);
    }

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&reports, "host-status output")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                "host-status",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned("scope", scope.as_str(), RowTone::Default));
            for entry in &reports {
                let mut value = format!("outcome={}", entry.outcome);
                if let Some(detail) = &entry.detail {
                    value.push_str(&format!(" detail={detail}"));
                }
                panel = panel.row(PanelRow::toned(
                    format!("{} {}", entry.host, entry.trait_id),
                    value,
                    match entry.outcome.as_str() {
                        "current" => RowTone::Pass,
                        "error"
                        | "locally-modified"
                        | "missing"
                        | "unmanaged"
                        | "leaf-symlink"
                        | "leaf-not-regular-file"
                        | "ownership-mismatch" => RowTone::Fail,
                        _ => RowTone::Default,
                    },
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct RemoveJsonOutput<'a> {
    trait_id: &'a str,
    host: &'a str,
    scope: &'a str,
    removed_paths: &'a [String],
}

pub(crate) fn handle_remove(
    trait_id: &str,
    host: &str,
    global: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (scope, root, manifest_path) = scope_and_paths(global)?;
    let record = ctx_traits_io::host_install::remove(
        &manifest_path,
        &root,
        scope,
        host,
        trait_id,
        &audit_root()?,
        unix_seconds_now(),
    )
    .map_err(host_install_error)?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = RemoveJsonOutput {
                trait_id,
                host,
                scope: scope.as_str(),
                removed_paths: &record.paths,
            };
            print_json_report(&output, "host-remove output")?;
        }
        OutputMode::Human(mode) => {
            let removed = record
                .paths
                .iter()
                .map(|path| PanelRow::toned("removed", path.as_str(), RowTone::Pass))
                .collect();
            let panel = Panel::new(
                "ctx",
                "host-remove",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned("trait", trait_id, RowTone::Default))
            .row(PanelRow::toned("host", host, RowTone::Default))
            .row(PanelRow::toned("scope", scope.as_str(), RowTone::Default))
            .section(PanelSection::new("removed", removed));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}
