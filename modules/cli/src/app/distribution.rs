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
    let mut rows = vec![PanelRow::toned(
        "alias",
        report.alias.as_str(),
        RowTone::Default,
    )];
    if report.transport == "path" {
        // A path install has no npm registry evidence at all: never print
        // "requested"/"resolved-version"/"integrity" rows that would imply
        // one.
        rows.push(PanelRow::toned("source", "path", RowTone::Default));
        rows.push(PanelRow::toned(
            "path",
            report.path.as_deref().unwrap_or(""),
            RowTone::Default,
        ));
    } else {
        rows.push(PanelRow::toned("source", "npm", RowTone::Default));
        rows.push(PanelRow::toned(
            "requested",
            report.requested.as_deref().unwrap_or(""),
            RowTone::Default,
        ));
        rows.push(PanelRow::toned(
            "resolved-version",
            report.resolved_version.as_deref().unwrap_or(""),
            RowTone::Default,
        ));
        rows.push(PanelRow::toned(
            "integrity",
            report.integrity.as_deref().unwrap_or(""),
            RowTone::Default,
        ));
    }
    rows.push(PanelRow::toned(
        "tree-digest",
        report.tree_digest.as_str(),
        RowTone::Default,
    ));
    rows.push(PanelRow::toned(
        "vendored-path",
        report.vendored_path.as_str(),
        RowTone::Default,
    ));
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

/// The install/update panel headline identity: the npm package identifier,
/// or `path:<path>` for a path-transport report — never a blank title.
fn report_headline(report: &InstallReport) -> String {
    if report.transport == "path" {
        format!("path:{}", report.path.as_deref().unwrap_or(""))
    } else {
        report.package.clone().unwrap_or_default()
    }
}

/// `ctx traits dependency add`: an npm/`path:` spec, a single git spec
/// (`owner/repo/trait[@ref]` shorthand or explicit form), or a git
/// collection spec combined with `--trait <id>` (repeatable) / `--all`
/// (task 0191 CLI surface). `trait_ids`/`all` only apply to a git spec that
/// does not already name a trait; usage conflicts are refused locally
/// before any network call.
pub(crate) fn handle_install(
    spec: &str,
    alias: Option<&str>,
    global: bool,
    trait_ids: &[String],
    all: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    if all && !trait_ids.is_empty() {
        return Err(crate::Error::Command {
            message: "--all cannot be combined with --trait".to_string(),
        });
    }
    if alias.is_some() && trait_ids.len() > 1 {
        return Err(crate::Error::Command {
            message: "--alias cannot be combined with more than one --trait".to_string(),
        });
    }

    let parsed = ctx_traits_core::distribution::parse_install_spec(spec)
        .map_err(ctx_traits_core::Error::from)?;
    let names_a_trait = matches!(
        &parsed,
        ctx_traits_core::distribution::InstallSpec::Git(git_spec)
            if git_spec.trait_selector.is_some()
    );
    if names_a_trait && (!trait_ids.is_empty() || all) {
        return Err(crate::Error::Command {
            message:
                "spec already names a trait; --trait/--all only apply to a bare git collection spec"
                    .to_string(),
        });
    }
    let git_spec = match &parsed {
        ctx_traits_core::distribution::InstallSpec::Git(git_spec) => Some(git_spec.clone()),
        _ => None,
    };
    if git_spec.is_none() && (!trait_ids.is_empty() || all) {
        return Err(crate::Error::Command {
            message: "--trait/--all only apply to a git collection spec".to_string(),
        });
    }

    let scope = resolve_scope(global)?;
    let registry = distribution::resolve_registry_options(scope.boundary());

    if all {
        let git_spec = git_spec.expect("checked above");
        let listing = distribution::list_git_collection(&git_spec)?;
        for entry in &listing.entries {
            let entry_spec = format!(
                "git+{}#ref={}&path={}",
                git_spec.url, listing.resolved_commit, entry.trait_path
            );
            let report = distribution::install(&scope, &entry_spec, None, registry)?;
            emit_install_report(&report, json)?;
        }
        return Ok(CommandOutput::new(()));
    }

    if !trait_ids.is_empty() {
        let git_spec = git_spec.expect("checked above");
        for trait_id in trait_ids {
            let entry_spec = match &git_spec.requested_ref {
                Some(git_ref) => format!("git+{}#ref={}&path={}", git_spec.url, git_ref, trait_id),
                None => format!("git+{}#path={}", git_spec.url, trait_id),
            };
            let entry_alias = if trait_ids.len() == 1 { alias } else { None };
            let report = distribution::install(&scope, &entry_spec, entry_alias, registry)?;
            emit_install_report(&report, json)?;
        }
        return Ok(CommandOutput::new(()));
    }

    // A bare git collection spec with no selection is DISCOVERY, not an
    // error (0191 DX centerpiece): list the contained traits with copyable
    // add commands and exit 0. Only explicit selections vendor anything.
    if let Some(git_spec) = &git_spec
        && git_spec.trait_selector.is_none()
    {
        let listing = distribution::list_git_collection(git_spec)?;
        if json {
            print_json_report(&listing, "collection listing")?;
        } else {
            println!(
                "collection {} @ {} — pass one of its traits (or --all):\n{}",
                git_spec.url,
                listing.resolved_commit,
                listing.render()
            );
        }
        return Ok(CommandOutput::new(()));
    }

    let report = distribution::install(&scope, spec, alias, registry)?;
    emit_install_report(&report, json)?;
    Ok(CommandOutput::new(()))
}

fn emit_install_report(report: &InstallReport, json: bool) -> crate::Result<()> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(report, "install report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("dependency add {}", report_headline(report)),
                PanelStatus::Passed("passed".to_string()),
            );
            for row in install_report_rows(report) {
                panel = panel.row(row);
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(())
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
                        report_headline(report),
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
                        if row.transport == "path" {
                            let path = row.path.as_deref().unwrap_or("");
                            let drift = match row.drift {
                                Some(true) => "yes — run `ctx traits dependency update` to accept",
                                Some(false) => "no",
                                None => "unknown — source unreadable",
                            };
                            PanelRow::toned(
                                format!("{} (path:{path})", row.alias),
                                format!(
                                    "locked-tree-digest={} current-tree-digest={} drift={drift}",
                                    row.locked_tree_digest.as_deref().unwrap_or(""),
                                    row.current_tree_digest.as_deref().unwrap_or(""),
                                ),
                                RowTone::Default,
                            )
                        } else {
                            PanelRow::toned(
                                format!("{} ({})", row.alias, row.package.as_deref().unwrap_or("")),
                                format!(
                                    "current={} wanted={} latest={}",
                                    row.current.as_deref().unwrap_or(""),
                                    row.wanted.as_deref().unwrap_or(""),
                                    row.latest.as_deref().unwrap_or(""),
                                ),
                                RowTone::Default,
                            )
                        }
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
            let headline = if report.transport == "path" {
                format!("path:{}", report.path.as_deref().unwrap_or(""))
            } else {
                report.package.clone().unwrap_or_default()
            };
            let mut panel = Panel::new(
                "ctx",
                format!("dependency info {headline}"),
                PanelStatus::Passed("passed".to_string()),
            );
            if report.transport != "path" {
                panel = panel.row(PanelRow::toned(
                    "resolved-version",
                    report.resolved_version.as_deref().unwrap_or(""),
                    RowTone::Default,
                ));
            }
            panel = panel.row(PanelRow::toned(
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

/// `ctx traits dependency init`: give a trait package a publishable identity.
///
/// Sets `[publish]` in `trait.toml` (ctx's record of the npm name, registry,
/// and access); the npm wrapper `package.json` is generated from it at build
/// time, never hand-authored. Without this a package has no name it can
/// actually publish under — the fallback is `@ctx-traits/<id>`, a scope only
/// this project owns.
pub(crate) fn handle_dependency_init(
    path: Option<&str>,
    name: Option<&str>,
    registry: Option<&str>,
    access: Option<&str>,
    force: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let package_root = match path {
        Some(path) => camino::Utf8PathBuf::from(path),
        None => current_utf8_dir()?,
    };
    let manifest_path = ctx_traits_io::layout::package_manifest_path(&package_root);
    let Some(manifest) = ctx_traits_io::distribution::read_package_manifest(&package_root)? else {
        return Err(crate::Error::Command {
            message: format!(
                "no trait package at {package_root}: expected a trait.toml here (run `ctx traits new` first)"
            ),
        });
    };

    if let Some(access) = access
        && !matches!(access, "public" | "restricted")
    {
        return Err(crate::Error::Command {
            message: format!("--access must be `public` or `restricted`, not {access}"),
        });
    }

    let existing = manifest.publish.as_ref();
    if !force
        && let Some(existing) = existing
        && existing.name.is_some()
        && name.is_some()
    {
        return Err(crate::Error::Command {
            message: format!(
                "{manifest_path} already declares [publish] name = {:?}; pass --force to replace it",
                existing.name.as_deref().unwrap_or_default()
            ),
        });
    }

    let resolved_name = name
        .map(str::to_string)
        .or_else(|| existing.and_then(|publish| publish.name.clone()))
        .unwrap_or_else(|| format!("@ctx-traits/{}", manifest.package.id));
    if !valid_npm_name(&resolved_name) {
        return Err(crate::Error::Command {
            message: format!("not a valid npm package name: {resolved_name}"),
        });
    }
    let resolved_registry = registry
        .map(str::to_string)
        .or_else(|| existing.and_then(|publish| publish.registry.clone()));
    let resolved_access = access
        .map(str::to_string)
        .or_else(|| existing.and_then(|publish| publish.access.clone()));

    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|source| crate::Error::Command {
            message: format!("cannot read {manifest_path}: {source}"),
        })?;
    let patched = upsert_publish_table(
        &manifest_text,
        &resolved_name,
        resolved_registry.as_deref(),
        resolved_access.as_deref(),
    );
    // Re-decode before writing, exactly as the status edit does: a text patch
    // that produces an unparseable manifest must never reach disk.
    let Some(decoded) =
        ctx_traits_core::manifest::decode_package_manifest(&patched, manifest_path.as_str())?
    else {
        return Err(crate::Error::Command {
            message: format!(
                "[publish] edit produced an unreadable {manifest_path}; refusing to write"
            ),
        });
    };
    if decoded
        .publish
        .as_ref()
        .and_then(|publish| publish.name.as_deref())
        != Some(resolved_name.as_str())
    {
        return Err(crate::Error::Command {
            message: format!(
                "[publish] edit did not take effect in {manifest_path}; refusing to write"
            ),
        });
    }
    ctx_traits_io::write::write_package_manifest(&manifest_path, &patched)?;

    // The npm wrapper. No `exports`/`main`/`module`: a trait package is read
    // by ctx through trait.toml, never imported as JavaScript. An existing
    // wrapper keeps every field it already had except name and version.
    let wrapper_path = package_root.join("package.json");
    let mut wrapper: serde_json::Value = if wrapper_path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(&wrapper_path).map_err(|source| {
            crate::Error::Command {
                message: format!("cannot read {wrapper_path}: {source}"),
            }
        })?)
        .map_err(|source| crate::Error::Command {
            message: format!("invalid {wrapper_path}: {source}"),
        })?
    } else {
        serde_json::json!({})
    };
    let object = wrapper
        .as_object_mut()
        .ok_or_else(|| crate::Error::Command {
            message: format!("{wrapper_path} must be a JSON object"),
        })?;
    object.insert(
        "name".to_string(),
        serde_json::Value::String(resolved_name.clone()),
    );
    object.insert(
        "version".to_string(),
        serde_json::Value::String(manifest.package.version.clone()),
    );
    if let Some(description) = manifest.package.description.as_ref() {
        object
            .entry("description")
            .or_insert_with(|| serde_json::Value::String(description.clone()));
    }
    // `#trait/*` source-root alias (task 0168): insert when absent so a
    // publishable package can use it. A present-but-different value is left
    // alone — the build walk is the single enforcement point for the
    // mapping, not this writer.
    object
        .entry("imports")
        .or_insert_with(|| serde_json::json!({ "#trait/*": "./source/*" }));
    let wrapper_text = format!(
        "{}\n",
        serde_json::to_string_pretty(&wrapper).map_err(|source| crate::Error::Command {
            message: format!("cannot render {wrapper_path}: {source}"),
        })?
    );
    std::fs::write(&wrapper_path, wrapper_text).map_err(|source| crate::Error::Command {
        message: format!("cannot write {wrapper_path}: {source}"),
    })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(
                &serde_json::json!({
                    "package-root": package_root.as_str(),
                    "manifest": manifest_path.as_str(),
                    "wrapper": wrapper_path.as_str(),
                    "name": resolved_name,
                    "version": manifest.package.version,
                    "registry": resolved_registry,
                    "access": resolved_access,
                }),
                "dependency init report",
            )?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("dependency init {resolved_name}"),
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "package",
                package_root.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "name",
                resolved_name.as_str(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "version",
                manifest.package.version.as_str(),
                RowTone::Default,
            ));
            if let Some(registry) = resolved_registry.as_deref() {
                panel = panel.row(PanelRow::toned("registry", registry, RowTone::Default));
            }
            if let Some(access) = resolved_access.as_deref() {
                panel = panel.row(PanelRow::toned("access", access, RowTone::Default));
            } else {
                // npm defaults a NEW scoped package to restricted, so silence
                // here is the difference between a public package and one
                // nobody can install.
                panel = panel.row(PanelRow::toned(
                    "access",
                    "unset — npm defaults a new scoped package to restricted",
                    RowTone::Warn,
                ));
            }
            panel = panel.row(PanelRow::toned(
                "wrapper",
                wrapper_path.as_str(),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Replace the `[publish]` table, or append one, preserving every other byte
/// of the manifest — comments, ordering, and the `[family]`/`[dependencies]`
/// tables an author maintains by hand.
fn upsert_publish_table(
    text: &str,
    name: &str,
    registry: Option<&str>,
    access: Option<&str>,
) -> String {
    let mut rendered = format!("[publish]\nname = \"{name}\"\n");
    if let Some(registry) = registry {
        rendered.push_str(&format!("registry = \"{registry}\"\n"));
    }
    if let Some(access) = access {
        rendered.push_str(&format!("access = \"{access}\"\n"));
    }

    let mut out = String::with_capacity(text.len() + rendered.len() + 2);
    let mut lines = text.lines().peekable();
    let mut replaced = false;
    while let Some(line) = lines.next() {
        if line.trim() == "[publish]" {
            replaced = true;
            out.push_str(&rendered);
            // Drop the old table's key/value lines; stop at the next table
            // header so a following `[dependencies]` survives untouched.
            while let Some(next) = lines.peek() {
                if next.trim_start().starts_with('[') {
                    break;
                }
                lines.next();
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&rendered);
    }
    out
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
    if let Some(manifest) = &root_manifest
        && manifest.package.status != ctx_traits_core::manifest::PackageStatus::Ready
    {
        return Err(crate::Error::Command {
            message: ctx_traits_io::publish::Error::NotReady(manifest.package.status.to_string())
                .to_string(),
        });
    }
    let inspection = ctx_traits_io::distribution::inspect_local_package(&package_root)?;
    // Count PACKAGES, not canonical traits. A native family is one publishable
    // package that legitimately contains several canonicals, and since P535
    // `inspect_local_package` expands it into one entry per leaf — so the old
    // `packages.len() != 1` guard refused every folded family (`implement`
    // reported "found 5", `plan` "found 4"), which is exactly the set worth
    // publishing. Distinct roots is the question the guard was always asking.
    let distinct_roots: std::collections::BTreeSet<&camino::Utf8Path> = inspection
        .packages
        .iter()
        .map(|package| package.root.as_path())
        .collect();
    if distinct_roots.len() != 1 {
        return Err(crate::Error::Command {
            message: ctx_traits_io::publish::Error::PackageCount(distinct_roots.len()).to_string(),
        });
    }
    let trait_package = &inspection.packages[0];
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
    // Check EVERY canonical in the package, not just the first. A family
    // publishes as one unit, so a package whose default variant passes while
    // another leaf is stale would ship exactly the drift this check exists to
    // catch. Fails on the first bad leaf and names it — "the check failed" is
    // useless when five canonicals could be the reason.
    for package in &inspection.packages {
        let check =
            crate::app::report_check::build_check_report(&crate::app::report_check::CheckInputs {
                file: package.manifest_path.as_str(),
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
            let leaf = package
                .variant
                .as_deref()
                .map(|variant| format!(" (variant {variant})"))
                .unwrap_or_default();
            return Err(crate::Error::Command {
                message: format!(
                    "publish requires a passed locked check with verified CDK drift: {}{leaf} failed",
                    package.manifest_path
                ),
            });
        }
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
    } else if let Some(declared) = manifest
        .publish
        .as_ref()
        .and_then(|publish| publish.name.clone())
    {
        // Declared `[publish] name` wins over any derived default. Without
        // this the scope below is imposed on every publisher, and `@ctx-traits`
        // is a scope nobody but us can publish to.
        if !valid_npm_name(&declared) {
            return Err(crate::Error::Command {
                message: format!("[publish] name is not a valid npm package name: {declared}"),
            });
        }
        (declared, manifest.package.version.clone())
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

/// Whether `name` is a publishable npm package name, scoped or unscoped.
///
/// Previously this stripped only the literal `@ctx-traits/` prefix, so every
/// other scope failed validation — `@acme/review` was rejected as malformed.
/// Together with the derived `@ctx-traits/<id>` default that made third-party
/// publication impossible twice over: the name we invented was one nobody else
/// could use, and any name they chose instead was refused here.
fn valid_npm_name(name: &str) -> bool {
    let bare = match name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            // A scope must be non-empty and is validated by the same character
            // rule as the package part.
            Some((scope, package)) if !scope.is_empty() && valid_npm_segment(scope) => package,
            _ => return false,
        },
        None => name,
    };
    valid_npm_segment(bare)
}

/// One npm name segment: a scope without its `@`, or the package part.
fn valid_npm_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        })
        && !segment.starts_with('.')
        && !segment.starts_with('_')
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
