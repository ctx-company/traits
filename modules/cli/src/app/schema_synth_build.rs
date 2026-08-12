//! Schema emission and draft/CDK synthesis commands.

use ctx_traits_core::response::CommandOutput;

use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};

pub(crate) fn handle_schema(
    protocol: &str,
    format: &str,
    out: &Option<String>,
) -> crate::Result<CommandOutput<()>> {
    let protocol = ctx_traits_core::schema::Protocol::parse(protocol).ok_or_else(|| {
        crate::Error::Command {
            message: format!(
                "unsupported protocol: {protocol:?} (expected \"agent-traits\" or \"sdk-types\")"
            ),
        }
    })?;

    let format =
        ctx_traits_core::schema::Format::parse(format).ok_or_else(|| crate::Error::Command {
            message: format!("unsupported format: {format:?} (only \"json\" is supported)"),
        })?;

    let schema = match protocol {
        ctx_traits_core::schema::Protocol::AgentTraits => ctx_traits_core::schema::trait_schema()?,
        ctx_traits_core::schema::Protocol::SdkTypes => ctx_traits_core::schema::sdk_types_schema()?,
    };

    let json = match format {
        ctx_traits_core::schema::Format::Json => serde_json::to_string_pretty(&schema)
            .map_err(|e| crate::Error::json("serialize schema", e))?,
    };

    let output = CommandOutput::new(()).with_warning(ctx_traits_core::response::Warning::new(
        ctx_traits_core::response::warning_code::SCHEMA_SUPPORT_ARTIFACT,
        "generated schema describes canonical normalized JSON output for .protocol/agent-traits/; \
         it is a non-authoritative support artifact, not the authoritative protocol source, \
         and does not imply runtime permission enforcement",
    ));

    match out {
        Some(path) => {
            let path = camino::Utf8Path::new(path);
            ctx_traits_io::write::write_text(path, &json)?;
            println!("schema written to {path}");
        }
        None => {
            println!("{json}");
        }
    }

    Ok(output)
}

pub(crate) fn handle_synth(
    path: &str,
    format: &str,
    out: Option<&str>,
    check: bool,
) -> crate::Result<CommandOutput<()>> {
    let output_format = ctx_traits_core::synth::OutputFormat::parse(format).ok_or_else(|| {
        crate::Error::Command {
            message: format!("unsupported synth format {format:?}; expected toml, json, or yaml"),
        }
    })?;
    let input_path = camino::Utf8Path::new(path);
    let draft_text = ctx_traits_io::read::read_text(input_path)?;
    let draft_json: serde_json::Value = serde_json::from_str(&draft_text)
        .map_err(|e| crate::Error::json(format!("parse draft JSON {path}"), e))?;
    let response = ctx_traits_core::synth::synthesize(ctx_traits_core::synth::Request {
        document_kind: ctx_traits_core::synth::DocumentKind::Infer,
        draft_json,
        output_format,
        provenance: ctx_traits_core::synth::ProvenanceSeed {
            generator_package: Some("ctx-traits-cli".to_string()),
            generator_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            source_path: Some(path.to_string()),
            warnings: Vec::new(),
        },
    })?;

    if check {
        let target_path = synth_target_path(input_path, output_format, out);
        let existing = ctx_traits_io::read::read_optional_text(&target_path)?;
        let actual_digest = existing
            .as_deref()
            .map(ctx_traits_core::digest::Digest::source)
            .map(|digest| digest.as_str().to_string());
        let drift = existing.as_deref() != Some(response.output_text.as_str());
        print_synth_check_report(
            &response,
            path,
            target_path.as_str(),
            actual_digest.as_deref(),
            drift,
        );
        if drift {
            return Err(crate::Error::Command {
                message: format!("synth --check detected drift for {target_path}"),
            });
        }
        return Ok(CommandOutput::new(()));
    }

    if let Some(out_path) = out {
        let target_path = camino::Utf8Path::new(out_path);
        ctx_traits_io::write::write_text(target_path, &response.output_text)?;
        println!("ctx traits synth");
        println!("  source: {path}");
        println!("  target: {out_path}");
        println!(
            "  document-kind: {}",
            synth_document_kind_label(response.document_kind)
        );
        println!("  format: {}", output_format.as_str());
        print_synth_provenance("  ", &response.provenance);
        if response.warnings.is_empty() {
            println!("  warnings: none");
        } else {
            println!("  warnings:");
            for warning in &response.warnings {
                println!("    {warning}");
            }
        }
    } else {
        print!("{}", response.output_text);
    }

    Ok(CommandOutput::new(()))
}

/// Evidence produced by one `ctx traits build` run: the resolved target and
/// source-map paths, and the underlying CDK synthesis outcome. Extracted so
/// `ctx traits new` (P271) can build a freshly instantiated template's
/// source through the exact same path `ctx traits build` uses — including
/// package/source identity enforcement — without duplicating any of this
/// logic or driving the CLI recursively.
pub(crate) struct BuildEvidence {
    pub(crate) target_path: camino::Utf8PathBuf,
    pub(crate) map_path: camino::Utf8PathBuf,
    pub(crate) output_format: ctx_traits_core::synth::OutputFormat,
    pub(crate) outcome: crate::app::cdk_build::CdkSynthOutcome,
}

/// Build a CDK authoring source into canonical output and a source map,
/// enforcing package/source identity, and write both to disk. Shared by
/// `ctx traits new`'s package materialization and `trait_editor.rs`'s
/// preview path — both single-trait-only callers that must synthesize the
/// source exactly once. [`handle_build`] instead calls
/// [`route_cdk_build`] directly, since it must also accept native family
/// sources without a second synthesis.
pub(crate) fn build_cdk_package(
    path: &str,
    format: &str,
    out: Option<&str>,
) -> crate::Result<BuildEvidence> {
    let output_format = ctx_traits_core::synth::OutputFormat::parse(format).ok_or_else(|| {
        crate::Error::Command {
            message: format!("unsupported build format {format:?}; expected toml, json, or yaml"),
        }
    })?;
    let source_path = camino::Utf8Path::new(path);
    crate::app::cdk_build::ensure_unambiguous_cdk_source(source_path)?;
    let outcome = crate::app::cdk_build::synthesize_cdk_source(source_path, output_format)?;
    finish_cdk_package_build(source_path, output_format, out, outcome)
}

/// The write/identity-enforcement tail shared by every single-trait build
/// path, factored out so a caller that already holds a computed
/// [`crate::app::cdk_build::CdkSynthOutcome`] (routed off a single Node
/// synthesis) never has to synthesize a second time to reach it.
fn finish_cdk_package_build(
    source_path: &camino::Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
    out: Option<&str>,
    outcome: crate::app::cdk_build::CdkSynthOutcome,
) -> crate::Result<BuildEvidence> {
    let (target_path, map_path) =
        crate::app::cdk_build::package_build_paths(source_path, output_format, out)?;
    crate::app::cdk_build::ensure_distinct_build_paths(source_path, &target_path, &map_path)?;
    validate_package_manifest_identity(&target_path, &outcome.response.output_text, output_format)?;
    ctx_traits_io::write::write_build_output(&target_path, &outcome.response.output_text)?;
    crate::app::cdk_build::write_generated_package_json_if_packaged(source_path)?;
    let map_json = serde_json::to_string_pretty(&outcome.source_map)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|e| crate::Error::json("serialize CDK source map", e))?;
    ctx_traits_io::write::write_build_output(&map_path, &map_json)?;

    Ok(BuildEvidence {
        target_path,
        map_path,
        output_format,
        outcome,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct BuildReportJson<'a> {
    source: &'a str,
    target: &'a str,
    source_map: &'a str,
    source_map_anchors: usize,
    runner: &'a str,
    argv: String,
    document_kind: &'static str,
    format: &'a str,
    provenance: &'a ctx_traits_core::synth::Provenance,
    warnings: &'a [String],
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct FamilyVariantReportJson {
    name: String,
    target: String,
    source_map: String,
    aliases: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct FamilyBuildReportJson {
    source: String,
    family: String,
    version: String,
    manifest: String,
    variants: Vec<FamilyVariantReportJson>,
}

/// Either shape `handle_build` writes: an ordinary single-trait package or a
/// native family's published variants. Produced by [`route_cdk_build`], which
/// synthesizes `path` exactly once and branches on the result — never both.
enum CdkBuildRouted {
    Single(Box<BuildEvidence>),
    Family(FamilyBuildReportJson),
}

/// Synthesize `path` exactly once via `synthesize_cdk_source_any`, then
/// route the single Node/core synthesis result to the right publish path:
/// a native family (`trait(id, { variants })`) publishes every variant's
/// canonical output under `generated/<name>/` and writes the package's
/// `[family]` manifest table; an ordinary single-trait source finishes
/// through the same write/identity-enforcement tail [`build_cdk_package`]
/// uses. `handle_build` is the only caller — it must accept both source
/// shapes without a second synthesis of the same source.
fn route_cdk_build(path: &str, format: &str, out: Option<&str>) -> crate::Result<CdkBuildRouted> {
    let output_format = ctx_traits_core::synth::OutputFormat::parse(format).ok_or_else(|| {
        crate::Error::Command {
            message: format!("unsupported build format {format:?}; expected toml, json, or yaml"),
        }
    })?;
    let source_path = camino::Utf8Path::new(path);
    crate::app::cdk_build::ensure_unambiguous_cdk_source(source_path)?;
    let synth = crate::app::cdk_build::synthesize_cdk_source_any(source_path, output_format)?;
    let family = match synth {
        crate::app::cdk_build::CdkSynthResult::Single(outcome) => {
            let evidence = finish_cdk_package_build(source_path, output_format, out, *outcome)?;
            return Ok(CdkBuildRouted::Single(Box::new(evidence)));
        }
        crate::app::cdk_build::CdkSynthResult::Family(family) => family,
    };
    if out.is_some() {
        return Err(crate::Error::Command {
            message: "--out is not supported for native trait family sources; every variant is \
                      published under generated/<name>/ by convention"
                .to_string(),
        });
    }
    let evidence = crate::app::cdk_build::publish_cdk_family(source_path, output_format, &family)?;
    Ok(CdkBuildRouted::Family(FamilyBuildReportJson {
        source: source_path.to_string(),
        family: evidence.family_id,
        version: evidence.family_version,
        manifest: evidence.manifest_path.to_string(),
        variants: evidence
            .variants
            .into_iter()
            .map(|variant| FamilyVariantReportJson {
                name: variant.name,
                target: variant.target_path.to_string(),
                source_map: variant.map_path.to_string(),
                aliases: variant.aliases,
            })
            .collect(),
    }))
}

/// P566: record the just-built canonical digests in the package's
/// `trait.lock`.
///
/// `build` is the only command that knows a rebuilt canonical digest — it
/// just computed it — and until now it was the one command that did not
/// record it. The lock therefore kept the PREVIOUS build's digests, and
/// `review --approve`, which compares built output against the lock, refused
/// with "rebuild before approving" immediately after a rebuild. The only way
/// to reconcile was `ctx traits vendor`, a dependency command with no
/// dependencies to resolve, used purely because it is the other caller of
/// this same projection.
///
/// `ctx traits new` has always done build-then-lock in one command
/// (`new.rs`), which is why scaffolding never hit this; `build` simply
/// skipped the second half. The projection upserts by `(id, variant)`, so
/// calling it once per family variant accumulates the whole family into one
/// package-root lock rather than overwriting.
///
/// Skipped, not failed, when the build did not write into a package the lock
/// belongs to: an `--out` redirect targets a caller-chosen path with no
/// package around it, and a built-in store package is read-only. Neither is
/// an error in `build`'s own terms.
fn record_lock_evidence(target: &camino::Utf8Path, relock: bool) -> crate::Result<()> {
    let repo_root = crate::app::lifecycle_reporting::current_utf8_dir()?;
    ctx_traits_io::dependency::sync(ctx_traits_io::dependency::SyncRequest {
        repo_root: &repo_root,
        manifest_path: None,
        trait_file: Some(target),
        mode: ctx_traits_io::dependency::SyncMode::Write,
    })?;
    ctx_traits_io::dependency::sync_derived_dependency_locks(target, relock)?;
    Ok(())
}

pub(crate) fn handle_build(
    path: &str,
    format: &str,
    out: Option<&str>,
    json: bool,
    relock: bool,
) -> crate::Result<CommandOutput<()>> {
    let source_path = if matches!(camino::Utf8Path::new(path).extension(), Some("ts" | "mjs")) {
        camino::Utf8PathBuf::from(path)
    } else {
        let (trait_path, _) = ctx_traits_io::run::resolve_trait_path(None, Some(path), "build")?;
        crate::app::cdk_build::package_cdk_source(&trait_path)?.ok_or_else(|| {
            crate::Error::Command {
                message: format!(
                    "cannot rebuild named trait {path:?}: it resolves to {trait_path}, but its package has no TypeScript or JavaScript authoring source"
                ),
            }
        })?
    };
    let evidence = match route_cdk_build(source_path.as_str(), format, out)? {
        CdkBuildRouted::Single(evidence) => *evidence,
        CdkBuildRouted::Family(family_report) => {
            // Every variant, so a family's lock carries the whole topology
            // the approval guard and the drift gate both read (P532).
            for variant in &family_report.variants {
                record_lock_evidence(camino::Utf8Path::new(&variant.target), relock)?;
            }
            match OutputMode::select(json, false) {
                OutputMode::Json => {
                    crate::app::command_handlers::print_json_report(
                        &family_report,
                        "build output",
                    )?;
                }
                OutputMode::Human(mode) => {
                    let mut panel =
                        Panel::new("ctx", "build", PanelStatus::Passed("passed".to_string()))
                            .row(PanelRow::toned(
                                "family",
                                &family_report.family,
                                RowTone::Default,
                            ))
                            .row(PanelRow::toned(
                                "manifest",
                                &family_report.manifest,
                                RowTone::Default,
                            ));
                    for variant in &family_report.variants {
                        panel = panel.row(PanelRow::toned(
                            "variant",
                            format!("{} -> {}", variant.name, variant.target),
                            RowTone::Default,
                        ));
                    }
                    emit_human(false, &panel, mode, || Ok(()))?;
                }
            }
            return Ok(CommandOutput::new(()));
        }
    };
    let BuildEvidence {
        target_path,
        map_path,
        output_format,
        outcome,
    } = evidence;

    // An `--out` build writes outside the package, so there is no package
    // lock the digests belong to; see `record_lock_evidence`.
    if out.is_none() {
        record_lock_evidence(&target_path, relock)?;
    }

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let report = BuildReportJson {
                source: outcome.source_path.as_str(),
                target: target_path.as_str(),
                source_map: map_path.as_str(),
                source_map_anchors: outcome.source_map.len(),
                runner: outcome.run.source_kind.as_str(),
                argv: build_argv_evidence(&outcome.run),
                document_kind: synth_document_kind_label(outcome.response.document_kind),
                format: output_format.as_str(),
                provenance: &outcome.response.provenance,
                warnings: &outcome.response.warnings,
            };
            crate::app::command_handlers::print_json_report(&report, "build output")?;
        }
        OutputMode::Human(mode) => {
            let provenance = &outcome.response.provenance;
            let warnings = if outcome.response.warnings.is_empty() {
                "none".to_string()
            } else {
                outcome.response.warnings.join("; ")
            };
            let panel = Panel::new("ctx", "build", PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned(
                    "source",
                    outcome.source_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "target",
                    target_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "source-map",
                    map_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "source-map-anchors",
                    outcome.source_map.len().to_string(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "runner",
                    outcome.run.source_kind.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "argv",
                    build_argv_evidence(&outcome.run),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "document-kind",
                    synth_document_kind_label(outcome.response.document_kind),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "format",
                    output_format.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "generator-package",
                    provenance.generator_package.as_deref().unwrap_or("none"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "generator-version",
                    provenance.generator_version.as_deref().unwrap_or("none"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "source-path",
                    provenance.source_path.as_deref().unwrap_or("none"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "source-path-digest",
                    provenance.source_path_digest.as_deref().unwrap_or("none"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "draft-digest",
                    provenance.draft_digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "canonical-digest",
                    provenance.canonical_digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "output-digest",
                    provenance.output_digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "warnings",
                    warnings,
                    if outcome.response.warnings.is_empty() {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// Enforce `[package]` identity when the package declares a root manifest:
/// the built trait's id and version must match `package.id`/`package.version`.
fn validate_package_manifest_identity(
    target_path: &camino::Utf8Path,
    output_text: &str,
    output_format: ctx_traits_core::synth::OutputFormat,
) -> crate::Result<()> {
    let Some(package_root) = ctx_traits_io::layout::package_root_for_manifest(target_path) else {
        return Ok(());
    };
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    if !manifest_path.is_file() {
        return Ok(());
    }
    let manifest_text = ctx_traits_io::read::read_text(&manifest_path)?;
    let Some(manifest) =
        ctx_traits_core::manifest::decode_package_manifest(&manifest_text, manifest_path.as_str())?
    else {
        return Ok(());
    };
    let encoding = match output_format {
        ctx_traits_core::synth::OutputFormat::Toml => ctx_traits_core::encoding::Encoding::Toml,
        ctx_traits_core::synth::OutputFormat::Json => ctx_traits_core::encoding::Encoding::Json,
        ctx_traits_core::synth::OutputFormat::Yaml => ctx_traits_core::encoding::Encoding::Yaml,
    };
    let (built, decode_warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, output_text)?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(
        manifest_path.as_str(),
        &decode_warnings,
    );
    if built.id.as_str() != manifest.package.id
        || built.version.as_str() != manifest.package.version
    {
        return Err(crate::Error::Command {
            message: format!(
                "package manifest {manifest_path} declares [package] {}@{} but the built trait is {}@{}; align trait source and manifest",
                manifest.package.id,
                manifest.package.version,
                built.id.as_str(),
                built.version.as_str()
            ),
        });
    }
    Ok(())
}

fn build_argv_evidence(run: &ctx_traits_io::cdk_build::CdkBuildOutcome) -> String {
    let mut argv = run.argv.clone();
    match run.source_kind {
        ctx_traits_io::cdk_build::CdkSourceKind::TypeScript
        | ctx_traits_io::cdk_build::CdkSourceKind::JavaScriptModule => {
            for index in 0..argv.len().saturating_sub(1) {
                if argv[index] == "--import" {
                    argv[index + 1] = "<ctx-cdk-node-loader>".to_string();
                } else if argv[index] == "--eval" {
                    argv[index + 1] = "<ctx-cdk-node-emitter>".to_string();
                }
            }
        }
    }
    argv.join(" ")
}

fn synth_target_path(
    input_path: &camino::Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
    out: Option<&str>,
) -> camino::Utf8PathBuf {
    match out {
        Some(out_path) => camino::Utf8Path::new(out_path).to_path_buf(),
        None => input_path.with_extension(output_format.extension()),
    }
}

fn print_synth_check_report(
    response: &ctx_traits_core::synth::Response,
    source_path: &str,
    target_path: &str,
    actual_digest: Option<&str>,
    drift: bool,
) {
    println!("ctx traits synth --check");
    println!("  source: {source_path}");
    println!("  target: {target_path}");
    println!(
        "  document-kind: {}",
        synth_document_kind_label(response.document_kind)
    );
    println!("  format: {}", response.provenance.output_format.as_str());
    println!("  drift: {}", if drift { "yes" } else { "no" });
    println!("  expected-digest: {}", response.provenance.output_digest);
    println!("  actual-digest: {}", actual_digest.unwrap_or("missing"));
    print_synth_provenance("  ", &response.provenance);
    if response.warnings.is_empty() {
        println!("  warnings: none");
    } else {
        println!("  warnings:");
        for warning in &response.warnings {
            println!("    {warning}");
        }
    }
}

pub(crate) fn print_synth_provenance(
    prefix: &str,
    provenance: &ctx_traits_core::synth::Provenance,
) {
    // Broken-pipe-safe: route through write_plain_line and drop the result
    // (matching println!'s fire-and-forget semantics without its panic).
    let write = |text: String| {
        let _ = crate::app::tui::write_plain_line(text);
    };
    write(format!("{prefix}provenance:"));
    write(format!(
        "{prefix}  generator-package: {}",
        provenance.generator_package.as_deref().unwrap_or("none")
    ));
    write(format!(
        "{prefix}  generator-version: {}",
        provenance.generator_version.as_deref().unwrap_or("none")
    ));
    write(format!(
        "{prefix}  source-path: {}",
        provenance.source_path.as_deref().unwrap_or("none")
    ));
    write(format!(
        "{prefix}  source-path-digest: {}",
        provenance.source_path_digest.as_deref().unwrap_or("none")
    ));
    write(format!(
        "{prefix}  draft-digest: {}",
        provenance.draft_digest
    ));
    write(format!(
        "{prefix}  canonical-digest: {}",
        provenance.canonical_digest
    ));
    write(format!(
        "{prefix}  output-digest: {}",
        provenance.output_digest
    ));
}

fn synth_document_kind_label(kind: ctx_traits_core::synth::DocumentKind) -> &'static str {
    match kind {
        ctx_traits_core::synth::DocumentKind::Infer => "infer",
        ctx_traits_core::synth::DocumentKind::Trait => "trait",
        ctx_traits_core::synth::DocumentKind::ProjectManifest => "project-manifest",
    }
}
