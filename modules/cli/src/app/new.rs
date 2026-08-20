//! `ctx traits create` — deterministic, template-backed package scaffolding
//! (P271).
//!
//! Distinct from `ctx traits generate`: `new` is deterministic and offline
//! except for invoking the local CDK build runtime — no model call, ever.
//! `generate` remains the LLM-backed authoring boundary. Bare `new` lists
//! the five first-party templates and writes nothing; `new <name> --from
//! <template>` materializes one of them into a draft package. `new` itself
//! writes no trust verdict: the reported machine trust is resolved by the
//! canonical digest, so a recreated package identical to one already
//! checked on this machine reports that existing verdict rather than
//! unconditionally claiming "unreviewed".

use ctx_traits_core::response::CommandOutput;

use crate::app::command_handlers::print_json_report;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TemplateJson<'a> {
    id: &'a str,
    purpose: &'a str,
}

/// List the five first-party templates in their fixed, sorted order.
/// Performs no filesystem writes.
pub(crate) fn handle_list_templates(json: bool) -> crate::Result<CommandOutput<()>> {
    let mut rows = Vec::new();
    for template in ctx_traits_core::builtin_templates::templates() {
        let purpose = ctx_traits_core::builtin_templates::purpose(template).map_err(|source| {
            crate::Error::Command {
                message: source.to_string(),
            }
        })?;
        rows.push((template.id, purpose));
    }

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let templates: Vec<TemplateJson<'_>> = rows
                .iter()
                .map(|(id, purpose)| TemplateJson { id, purpose })
                .collect();
            print_json_report(&templates, "new templates output")?;
        }
        OutputMode::Human(mode) => {
            let mut template_rows = Vec::new();
            for (id, purpose) in &rows {
                template_rows.push(PanelRow::toned(*id, purpose.clone(), RowTone::Default));
            }
            let panel = Panel::new("ctx", "create", PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned(
                    "usage",
                    "ctx traits create <id> [--from <template>] [--name <display>]",
                    RowTone::Default,
                ))
                .section(PanelSection::new("templates", template_rows));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Materialize one template into a brand-new draft package at
/// `.ctx/traits/<slugified-name>/`: write the rewritten authoring files,
/// build them through the same internal path `ctx traits build` uses, then
/// lock the result through the same internal path `ctx traits vendor` uses.
///
/// Never overwrites an existing package: the root is claimed atomically
/// first, and an existing root at that path is left byte-for-byte untouched
/// and reported as an explicit error. If the build step fails (e.g. no
/// local Node/CDK runtime), the claimed root and its two authored files
/// remain on disk as a source-only draft — nothing here deletes authored
/// content or pretends a failed build succeeded.
/// `id` is the package id, slugified; `name` is the display name, which
/// defaults to `id` at the call site. They are separate because the id has
/// to be a slug and a display name does not — `create work --name "Daily
/// Work"` is one trait, not two ideas.
pub(crate) fn handle_new(
    id: &str,
    name: &str,
    from: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let template = ctx_traits_core::builtin_templates::template(from).ok_or_else(|| {
        crate::Error::Command {
            message: format!(
                "unknown template {from:?}; run `ctx traits create` with no arguments to list templates"
            ),
        }
    })?;

    let trait_id = ctx_traits_core::synth::slugify_trait_id(id)?;
    let cwd = current_utf8_dir()?;

    // Halt BEFORE writing anything. `create` scaffolds, then builds and locks
    // through the same paths `build` and `vendor` use, so a repository that
    // cannot build produces a half-materialized package and an error about
    // whatever the build tripped over — which is how a missing `node_modules`
    // used to surface as "invalid manifest at root: missing field `name`",
    // from a seed file the failed build never got to replace.
    let blocking: Vec<_> = ctx_traits_io::authoring_env::missing_for_authoring(
        &cwd,
        &ctx_traits_io::init::authoring_range_spec(),
    )
    .into_iter()
    .filter(ctx_traits_io::authoring_env::Missing::blocks_authoring)
    .collect();
    if let Some(first) = blocking.first() {
        return Err(crate::Error::Command {
            message: format!(
                "cannot scaffold {trait_id}: building a trait needs the authoring packages — {}",
                first.remedy()
            ),
        });
    }
    let package = ctx_traits_io::layout::TraitPackageRoot::new(&cwd, &trait_id)
        .map_err(ctx_traits_io::Error::from)?;

    if !ctx_traits_io::package_scaffold::claim_root(package.root(), "new trait package root")? {
        return Err(crate::Error::Command {
            message: format!(
                "{} already exists; ctx traits create never overwrites an existing package",
                package.root()
            ),
        });
    }

    let instantiated = ctx_traits_core::builtin_templates::instantiate(template, &trait_id, name)
        .map_err(|source| crate::Error::Command {
        message: source.to_string(),
    })?;

    ctx_traits_io::package_scaffold::create_new_file(
        package.package_manifest(),
        "new trait package manifest",
        &instantiated.trait_toml,
    )?;
    ctx_traits_io::package_scaffold::ensure_dir(
        package.source_dir(),
        "new trait source directory",
    )?;
    let source_path = package.source_dir().join("index.ts");
    ctx_traits_io::package_scaffold::create_new_file(
        &source_path,
        "new trait CDK source",
        &instantiated.source_ts,
    )?;
    for (relative_path, contents) in &instantiated.extra_source_files {
        ctx_traits_io::package_scaffold::create_new_file(
            &package.source_dir().join(relative_path),
            "new trait CDK source",
            contents,
        )?;
    }

    // `#trait/*` source-root alias (task 0168): without this a scaffolded
    // package cannot use the alias at all. `ctx traits dependency init`
    // later inserts name/version into this same file while preserving other
    // fields, so the two writers compose.
    ctx_traits_io::package_scaffold::create_new_file(
        &package.root().join("package.json"),
        "new trait package.json",
        "{\n  \"imports\": {\n    \"#trait/*\": \"./source/*\"\n  }\n}\n",
    )?;

    // Same internal build operation `ctx traits build` uses: produces
    // generated/index.toml + generated/index.map and enforces
    // package/source identity between trait.toml and the built trait.
    let evidence =
        crate::app::schema_synth_build::build_cdk_package(source_path.as_str(), "toml", None)?;

    // Same dependency sync/lock projection `ctx traits vendor` uses: writes
    // trait.lock from the just-built canonical output, never a
    // hand-calculated digest.
    let sync_report = ctx_traits_io::dependency::sync(ctx_traits_io::dependency::SyncRequest {
        repo_root: &cwd,
        manifest_path: None,
        trait_file: Some(evidence.target_path.as_path()),
        mode: ctx_traits_io::dependency::SyncMode::Write,
        vendor_root_override: None,
    })?;

    // Consult the same identity-bound machine trust history `ctx traits check`
    // reports from, rather than assuming "unreviewed".
    let canonical_digest = evidence
        .outcome
        .response
        .provenance
        .canonical_digest
        .as_str();
    let trust =
        ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(&trait_id, canonical_digest)?;

    let report = NewReport {
        template_id: template.id,
        trait_id: trait_id.clone(),
        name: name.to_string(),
        package_manifest: package.package_manifest().to_string(),
        source_path: source_path.clone(),
        generated: evidence.target_path.clone(),
        source_map: evidence.map_path.clone(),
        lock: sync_report.lockfile.clone(),
        trust,
    };

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&new_report_json(&report), "new output")?;
        }
        OutputMode::Human(mode) => {
            let panel = new_report_panel(&report);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

fn new_report_panel(report: &NewReport) -> Panel {
    let (status_tone, status) = match report.trust {
        ctx_traits_core::r#trait::TrustVerdict::Verified => {
            (RowTone::Pass, PanelStatus::Passed("passed".to_string()))
        }
        ctx_traits_core::r#trait::TrustVerdict::Blocked => {
            (RowTone::Fail, PanelStatus::Blocked("blocked".to_string()))
        }
        ctx_traits_core::r#trait::TrustVerdict::Unreviewed => {
            (RowTone::Default, PanelStatus::Passed("passed".to_string()))
        }
    };
    Panel::new("ctx", "create", status)
        .row(PanelRow::toned(
            "template",
            report.template_id,
            RowTone::Default,
        ))
        .row(PanelRow::toned("id", &report.trait_id, RowTone::Default))
        .row(PanelRow::toned("name", &report.name, RowTone::Default))
        .row(PanelRow::toned(
            "package-manifest",
            report.package_manifest.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "source",
            report.source_path.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "generated",
            report.generated.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "source-map",
            report.source_map.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "lock",
            report.lock.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "status",
            trust_status_line_for(report),
            status_tone,
        ))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct NewReportJson<'a> {
    template_id: &'a str,
    trait_id: &'a str,
    name: &'a str,
    package_manifest: &'a str,
    source: &'a str,
    generated: &'a str,
    source_map: &'a str,
    lock: &'a str,
    trust: ctx_traits_core::r#trait::TrustVerdict,
}

fn new_report_json(report: &NewReport) -> NewReportJson<'_> {
    NewReportJson {
        template_id: report.template_id,
        trait_id: &report.trait_id,
        name: &report.name,
        package_manifest: &report.package_manifest,
        source: report.source_path.as_str(),
        generated: report.generated.as_str(),
        source_map: report.source_map.as_str(),
        lock: &report.lock,
        trust: report.trust,
    }
}

/// Everything `ctx traits create`'s success report needs, resolved once so both
/// the styled and plain renderers stay in lockstep.
struct NewReport {
    template_id: &'static str,
    trait_id: String,
    name: String,
    package_manifest: String,
    source_path: camino::Utf8PathBuf,
    generated: camino::Utf8PathBuf,
    source_map: camino::Utf8PathBuf,
    lock: String,
    trust: ctx_traits_core::r#trait::TrustVerdict,
}

fn trust_status_line_for(report: &NewReport) -> String {
    trust_status_line(&report.trait_id, report.trust, "draft")
}

/// Shared trust status line for a just-built package's report: `create`'s
/// wording, parameterized by `verb` (`"draft"` for `create`, `"forked"` for
/// `ctx traits fork`) so both commands render the exact same trust-by-digest
/// story instead of two copies of it drifting apart.
pub(crate) fn trust_status_line(
    trait_id: &str,
    trust: ctx_traits_core::r#trait::TrustVerdict,
    verb: &str,
) -> String {
    match trust {
        ctx_traits_core::r#trait::TrustVerdict::Verified
        | ctx_traits_core::r#trait::TrustVerdict::Blocked => format!(
            "status: {verb} (this machine already has a trust verdict for this exact canonical \
             digest: {}; run `ctx traits check {trait_id}` to review it)",
            trust.display_name(),
        ),
        ctx_traits_core::r#trait::TrustVerdict::Unreviewed => format!(
            "status: {verb} (no trust verdict on this machine for this canonical digest; run \
             `ctx traits check {trait_id}` then `ctx traits trust --approved {trait_id}` before \
             activating)",
        ),
    }
}
