//! `ctx traits install/remove/update/outdated/info` porcelain (P438).
//!
//! Thin printing/dispatch layer over `ctx_traits_io::distribution`: all
//! staging, verification, and mutation logic lives at the IO boundary. This
//! module only resolves the invocation `repo_root`, calls through, and
//! renders plain/JSON output.

use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::distribution::{self, DistributionScope, InstallReport, OutdatedRow};

use crate::app::command_handlers::print_json_report;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};

/// Resolve the P439 distribution scope for a mutating `install`/`remove`/
/// `update` invocation: `--global` selects the per-machine tier
/// (`~/.config/ctx`), otherwise the current project (unchanged P438
/// behavior).
fn resolve_scope(global: bool) -> crate::Result<DistributionScope> {
    if global {
        return Ok(DistributionScope::global()?);
    }
    let repo_root = current_utf8_dir()?;
    Ok(DistributionScope::project(&repo_root))
}

/// Builds an install/update report's rows, shared by `handle_install`'s
/// single-package panel and `handle_update`'s per-package section so the two
/// commands can never render a package's facts differently.
fn install_report_rows(report: &InstallReport) -> Vec<PanelRow> {
    let mut rows = vec![
        PanelRow::toned("alias", report.alias.as_str(), RowTone::Default),
        PanelRow::toned("requested", report.requested.as_str(), RowTone::Default),
        PanelRow::toned(
            "resolved-version",
            report.resolved_version.as_str(),
            RowTone::Default,
        ),
        PanelRow::toned("integrity", report.integrity.as_str(), RowTone::Default),
        PanelRow::toned(
            "vendored-path",
            report.vendored_path.as_str(),
            RowTone::Default,
        ),
    ];
    if report.inherited {
        rows.push(PanelRow::toned(
            "inherited",
            "true (from extends base)",
            RowTone::Default,
        ));
    }
    rows.push(PanelRow::toned(
        "claim",
        report.claim.as_str(),
        RowTone::Default,
    ));
    if report.traits.is_empty() {
        rows.push(PanelRow::toned("traits", "none", RowTone::Default));
    } else {
        for t in &report.traits {
            rows.push(PanelRow::toned(
                t.id.as_str(),
                format!(
                    "schema-version={} canonical-path={} canonical-digest={}",
                    t.schema_version, t.canonical_path, t.canonical_digest
                ),
                RowTone::Default,
            ));
        }
    }
    rows.push(PanelRow::toned(
        "next",
        report.review_hint.as_str(),
        RowTone::Default,
    ));
    rows
}

pub(crate) fn handle_install(
    spec: &str,
    alias: Option<&str>,
    global: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let scope = resolve_scope(global)?;
    let report = distribution::install(
        &scope,
        spec,
        alias,
        distribution::resolve_registry_options(scope.boundary()),
    )?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "install report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("dependency add {}", report.package),
                PanelStatus::Passed("passed".to_string()),
            );
            for row in install_report_rows(&report) {
                panel = panel.row(row);
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_remove(
    operand: &str,
    global: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let scope = resolve_scope(global)?;
    let report = distribution::remove(&scope, operand)?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "remove report")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new(
                "ctx",
                format!("dependency remove {}", report.alias),
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "package",
                report.package.as_str(),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_update(
    operand: Option<&str>,
    global: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let scope = resolve_scope(global)?;
    let reports = distribution::update(
        &scope,
        operand,
        distribution::resolve_registry_options(scope.boundary()),
    )?;
    // An `update` with no operand also resolves an `extends` base fresh
    // (P443): report its evidence so a base version move is as visible as
    // any package's, not just discoverable via a `.ctx/traits.lock` diff.
    let base = if operand.is_none() {
        distribution::current_base(&scope)?
    } else {
        None
    };
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            #[derive(serde::Serialize)]
            struct UpdateOutput<'a> {
                base: &'a Option<distribution::BaseSummary>,
                packages: &'a [InstallReport],
            }
            print_json_report(
                &UpdateOutput {
                    base: &base,
                    packages: &reports,
                },
                "update reports",
            )?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                "dependency update",
                PanelStatus::Passed("passed".to_string()),
            );
            if let Some(base) = &base {
                panel = panel.section(PanelSection::new(
                    "extends base",
                    vec![
                        PanelRow::toned("package", base.package.as_str(), RowTone::Default),
                        PanelRow::toned(
                            "resolved-version",
                            base.resolved_version.as_str(),
                            RowTone::Default,
                        ),
                        PanelRow::toned("integrity", base.integrity.as_str(), RowTone::Default),
                    ],
                ));
            }
            if reports.is_empty() {
                panel = panel.row(PanelRow::toned(
                    "packages",
                    "none — no matching project dependency",
                    RowTone::Default,
                ));
            } else {
                for report in &reports {
                    panel = panel.section(PanelSection::new(
                        report.package.as_str(),
                        install_report_rows(report),
                    ));
                }
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_outdated(json: bool) -> crate::Result<CommandOutput<()>> {
    let repo_root = current_utf8_dir()?;
    let rows = distribution::outdated(
        &repo_root,
        distribution::resolve_registry_options(&repo_root),
    )?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&rows, "outdated report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                "dependency outdated",
                PanelStatus::Passed("passed".to_string()),
            );
            if rows.is_empty() {
                panel = panel.row(PanelRow::toned(
                    "packages",
                    "none — no project dependencies",
                    RowTone::Default,
                ));
            } else {
                let entry_rows = rows
                    .iter()
                    .map(|row: &OutdatedRow| {
                        PanelRow::toned(
                            format!("{} ({})", row.alias, row.package),
                            format!(
                                "current={} wanted={} latest={}",
                                row.current, row.wanted, row.latest
                            ),
                            RowTone::Default,
                        )
                    })
                    .collect();
                panel = panel.section(PanelSection::new("packages", entry_rows));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_info(spec: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let repo_root = current_utf8_dir()?;
    let report = distribution::info(
        &repo_root,
        spec,
        distribution::resolve_registry_options(&repo_root),
    )?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "info report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("dependency info {}", report.package),
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "resolved-version",
                report.resolved_version.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "claim",
                report.claim.as_str(),
                RowTone::Default,
            ));
            if report.traits.is_empty() {
                panel = panel.row(PanelRow::toned("traits", "none", RowTone::Default));
            } else {
                for t in &report.traits {
                    let resource_roots = if t.resource_roots.is_empty() {
                        "none".to_string()
                    } else {
                        t.resource_roots.join(", ")
                    };
                    let agent_roles = if t.agent_roles.is_empty() {
                        "none".to_string()
                    } else {
                        t.agent_roles.join(", ")
                    };
                    let commands = if t.commands.is_empty() {
                        "none".to_string()
                    } else {
                        t.commands
                            .iter()
                            .map(|command| format!("{command:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    panel = panel.section(PanelSection::new(
                        t.id.as_str(),
                        vec![
                            PanelRow::toned(
                                "schema-version",
                                t.schema_version.as_str(),
                                RowTone::Default,
                            ),
                            PanelRow::toned(
                                "canonical-path",
                                t.canonical_path.as_str(),
                                RowTone::Default,
                            ),
                            PanelRow::toned(
                                "canonical-digest",
                                t.canonical_digest.as_str(),
                                RowTone::Default,
                            ),
                            PanelRow::toned("resource-roots", resource_roots, RowTone::Default),
                            PanelRow::toned("agent-roles", agent_roles, RowTone::Default),
                            PanelRow::toned("commands", commands, RowTone::Default),
                        ],
                    ));
                }
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_publish(
    path: Option<&str>,
    trait_id: Option<&str>,
    dry_run: bool,
    provenance: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let target = match (path, trait_id) {
        (Some(_), Some(_)) => {
            return Err(crate::Error::Command {
                message: "publish accepts PATH or --trait, not both".to_string(),
            });
        }
        (Some(path), None) => camino::Utf8PathBuf::from(path),
        (None, Some(id)) => ctx_traits_io::run::resolve_trait_path(None, Some(id), "publish")?.0,
        (None, None) => current_utf8_dir()?,
    };
    let package_root = if target.is_file() {
        let candidate = ctx_traits_io::layout::package_root_for_manifest(&target)
            .or_else(|| target.parent())
            .ok_or_else(|| crate::Error::Command {
                message: format!("cannot determine package root for {target}"),
            })?;
        match candidate.file_name() {
            Some("generated" | "source") => candidate
                .parent()
                .ok_or_else(|| crate::Error::Command {
                    message: format!("cannot determine package root for {target}"),
                })?
                .to_path_buf(),
            _ => candidate.to_path_buf(),
        }
    } else {
        target.clone()
    };
    let root_manifest = ctx_traits_io::distribution::read_package_manifest(&package_root)?;
    if let Some(manifest) = &root_manifest {
        if manifest.package.status != ctx_traits_core::manifest::PackageStatus::Ready {
            return Err(crate::Error::Command {
                message: ctx_traits_io::publish::Error::NotReady(
                    manifest.package.status.to_string(),
                )
                .to_string(),
            });
        }
    }
    let inspection = ctx_traits_io::distribution::inspect_local_package(&package_root)?;
    if inspection.packages.len() != 1 {
        return Err(crate::Error::Command {
            message: ctx_traits_io::publish::Error::PackageCount(inspection.packages.len())
                .to_string(),
        });
    }
    let trait_package = &inspection.packages[0];
    let trait_file = &trait_package.manifest_path;
    let manifest = root_manifest
        .as_ref()
        .or(trait_package.package_manifest.as_ref())
        .ok_or_else(|| crate::Error::Command {
            message: "publish requires a package manifest at trait.toml".to_string(),
        })?;
    if manifest.package.status != ctx_traits_core::manifest::PackageStatus::Ready {
        return Err(crate::Error::Command {
            message: ctx_traits_io::publish::Error::NotReady(manifest.package.status.to_string())
                .to_string(),
        });
    }
    let check =
        crate::app::report_check::build_check_report(&crate::app::report_check::CheckInputs {
            file: trait_file.as_str(),
            locked: true,
            skip_cdk_drift: false,
            json: false,
            plain: true,
            no_animate: true,
            verbose: false,
            run_ledger: None,
            eval_reports: &[],
        })?;
    if !check.passed
        || check
            .warnings
            .iter()
            .any(|warning| warning.code == "cdk-drift-unverified")
    {
        return Err(crate::Error::Command {
            message: "publish requires a passed locked check with verified CDK drift".to_string(),
        });
    }
    let direct_package = trait_package.root == package_root;
    let (npm_name, npm_version) = if !direct_package && package_root.join("package.json").is_file()
    {
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(package_root.join("package.json")).map_err(|source| {
                crate::Error::Command {
                    message: format!("cannot read npm package metadata: {source}"),
                }
            })?,
        )
        .map_err(|source| crate::Error::Command {
            message: format!("invalid package.json: {source}"),
        })?;
        let name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| crate::Error::Command {
                message: "package.json is missing name".to_string(),
            })?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| crate::Error::Command {
                message: "package.json is missing version".to_string(),
            })?;
        (name.to_string(), version.to_string())
    } else {
        let name = format!("@ctx-traits/{}", manifest.package.id);
        if !valid_npm_name(&name) {
            return Err(crate::Error::Command {
                message: format!("package id does not produce a valid npm name: {name}"),
            });
        }
        (name, manifest.package.version.clone())
    };
    // Resolved before `package_root` moves into `PublishInput` below, so the
    // skipped-path panel can name the actual provenance of the exclude set
    // (declared `[publish] exclude` vs. the built-in defaults) rather than
    // always claiming "default excludes".
    let excludes_declared = {
        let excludes = ctx_traits_io::harness_config::resolve_pack_excludes(&package_root);
        let resolved: std::collections::BTreeSet<&str> =
            excludes.iter().map(String::as_str).collect();
        let defaults: std::collections::BTreeSet<&str> =
            ctx_traits_io::publish::PACK_DEFAULT_EXCLUDES
                .iter()
                .copied()
                .collect();
        resolved != defaults
    };
    let report = ctx_traits_io::publish::publish(
        &ctx_traits_io::publish::PublishInput {
            root: package_root,
            package: npm_name,
            version: npm_version,
            canonical_digests: inspection.canonical_digests,
            required_paths: inspection.required_paths,
            excluded: inspection.excluded,
            force_identity: direct_package,
            provenance,
            json,
        },
        dry_run,
    )?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "publish report")?;
        }
        OutputMode::Human(mode) => {
            let headline = format!(
                "publish {}@{}{}",
                report.package,
                report.version,
                if dry_run { " (dry-run)" } else { "" }
            );
            let mut panel = Panel::new("ctx", headline, PanelStatus::Passed("passed".to_string()));
            let files = report
                .files
                .iter()
                .map(|file| {
                    PanelRow::toned(file.path.as_str(), file.digest.as_str(), RowTone::Default)
                })
                .collect();
            panel = panel.section(PanelSection::new("files", files));
            if !report.skipped.is_empty() {
                let provenance = if excludes_declared {
                    "declared [publish] exclude"
                } else {
                    "default excludes"
                };
                let skipped = report
                    .skipped
                    .iter()
                    .map(|skip| {
                        PanelRow::toned(
                            skip.path.as_str(),
                            format!("skipped: {}/ ({provenance})", skip.rule),
                            RowTone::Default,
                        )
                    })
                    .collect();
                panel = panel.section(PanelSection::new("skipped", skipped));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

fn valid_npm_name(name: &str) -> bool {
    let bare = name.strip_prefix("@ctx-traits/").unwrap_or(name);
    !bare.is_empty()
        && bare.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.' | '/')
        })
        && !bare.starts_with('.')
        && !bare.starts_with('_')
}

/// `ctx traits trust approve <package>`: resolve `operand` against installed
/// packages (project scope, when the invocation is inside a repository,
/// then global) and atomically approve every current trait digest of the
/// one matching package.
pub(crate) fn handle_approve(
    operand: &str,
    reason: Option<String>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let repo_root = match ctx_traits_io::state::discover_invocation_root()? {
        ctx_traits_io::state::InvocationRoot::Repo(root) => Some(root),
        ctx_traits_io::state::InvocationRoot::Adhoc(_) => None,
    };
    let report = distribution::approve_package(repo_root.as_deref(), operand, reason)?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "trust approve report")?;
        }
        OutputMode::Human(mode) => {
            let digests = if report.digests.is_empty() {
                "none".to_string()
            } else {
                report.digests.join(", ")
            };
            let mut panel = Panel::new(
                "ctx",
                format!("trust approve {}", report.package),
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "alias",
                report.alias.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "scope",
                report.scope.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned("digests", digests, RowTone::Default));
            let supersession_rows: Vec<PanelRow> = report
                .updates
                .iter()
                .filter_map(|update| {
                    let supersedes = update.supersedes.as_ref()?;
                    Some(PanelRow::toned(
                        update.trait_id.as_deref().unwrap_or("(digest-only)"),
                        format!("supersedes {}", supersedes.digest),
                        RowTone::Default,
                    ))
                })
                .collect();
            if !supersession_rows.is_empty() {
                panel = panel.section(PanelSection::new("supersedes", supersession_rows));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}
