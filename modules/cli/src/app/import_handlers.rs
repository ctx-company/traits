//! CLI import, refresh, and lifecycle handlers.

use ctx_traits_core::response::CommandOutput;

use crate::app::entry::{
    attach_assist_check_report, print_assist_candidate, print_json_report,
    trait_package_output_paths,
};
use crate::app::generate::{
    blocked_assist_candidate, canonical_json_value, decode_generate_candidate, run_builtin_trait,
    runtime_input,
};
use crate::app::import_analysis::{
    ImportSourceAnalysis, analyze_import_source, augment_draft_with_multi_file_resources,
    multi_file_evidence, multi_file_resource_mappings,
};
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportDriftEvidence {
    passed: bool,
    trait_path: String,
    report_path: String,
    trait_missing: bool,
    report_missing: bool,
    trait_drift: bool,
    report_drift: bool,
    trait_expected_digest: Option<String>,
    trait_actual_digest: Option<String>,
    report_expected_digest: Option<String>,
    report_actual_digest: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct RemoteSourceOutput<'a> {
    remote_source_evidence: Option<&'a ctx_traits_core::import::plan::RemoteSourceEvidence>,
    error: &'a str,
    status: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportCopyPlanReport<'a> {
    entries: Vec<ImportCopyPlanEntryReport<'a>>,
    warnings: Vec<ImportCopyPlanWarningReport<'a>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportCopyPlanEntryReport<'a> {
    source: &'a str,
    destination: &'a str,
    conflict: bool,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ImportCopyPlanWarningReport<'a> {
    PathConflict { destination: &'a str },
    UnsafeSourceLayout { path: &'a str },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportWriteResultReport {
    trait_path: String,
    report_path: String,
    copied_entries: usize,
    managed_overwrite: bool,
}

impl From<&ctx_traits_io::import::ImportWriteResult> for ImportWriteResultReport {
    fn from(result: &ctx_traits_io::import::ImportWriteResult) -> Self {
        Self {
            trait_path: result.trait_path.clone(),
            report_path: result.report_path.clone(),
            copied_entries: result.copied_entries,
            managed_overwrite: result.managed_overwrite,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportTargetReport {
    package_path: String,
    trait_path: String,
    report_path: String,
    written: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportAssistOutput<'a> {
    assist_candidate: &'a ctx_traits_core::assist::Candidate,
    trait_scaffold: &'a ctx_traits_core::scaffold::TraitScaffold,
    import_report: &'a ctx_traits_core::import::plan::ImportReport,
    target: ImportTargetReport,
    copy_plan: ImportCopyPlanReport<'a>,
    write_result: Option<ImportWriteResultReport>,
    check: Option<ImportDriftEvidence>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportWrittenOutput<'a> {
    import_report: &'a ctx_traits_core::import::plan::ImportReport,
    trait_scaffold: &'a ctx_traits_core::scaffold::TraitScaffold,
    copy_plan: ImportCopyPlanReport<'a>,
    write_result: ImportWriteResultReport,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportCheckOutput<'a> {
    import_report: &'a ctx_traits_core::import::plan::ImportReport,
    trait_scaffold: &'a ctx_traits_core::scaffold::TraitScaffold,
    copy_plan: ImportCopyPlanReport<'a>,
    check: ImportCheckReport<'a>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportCheckReport<'a> {
    source: &'a str,
    trait_path: String,
    report_path: String,
    passed: bool,
    trait_drift: bool,
    report_drift: bool,
    trait_expected_digest: &'a str,
    trait_actual_digest: Option<String>,
    report_expected_digest: &'a str,
    report_actual_digest: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ImportRefreshCandidateWrittenOutput {
    status: &'static str,
    out: String,
    decision: String,
    write_result: ImportWriteResultReport,
}

fn apply_assist_import_check_drift(
    mut candidate: ctx_traits_core::assist::Candidate,
    trait_path: &camino::Utf8Path,
    expected_trait: Option<&str>,
    report_path: &camino::Utf8Path,
    expected_report: &str,
) -> (ctx_traits_core::assist::Candidate, ImportDriftEvidence) {
    let trait_check = compare_expected_artifact(trait_path, expected_trait);
    let report_check = compare_expected_artifact(report_path, Some(expected_report));

    if !trait_check.passed {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate.gate_summary.check.ok = false;
        candidate
            .gate_summary
            .check
            .failed_sections
            .push("import-trait-drift".to_string());
        let code = if trait_check.missing {
            ctx_traits_core::assist::DiagnosticCode::ImportTraitMissing
        } else {
            ctx_traits_core::assist::DiagnosticCode::ImportTraitDrift
        };
        candidate
            .diagnostics
            .push(ctx_traits_core::assist::Diagnostic {
                gate: ctx_traits_core::assist::Gate::Drift,
                code,
                field: Some(trait_path.to_string()),
                message: "import trait target does not match normalized candidate".to_string(),
            });
    }
    if !report_check.passed {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate.gate_summary.check.ok = false;
        candidate
            .gate_summary
            .check
            .failed_sections
            .push("import-report-drift".to_string());
        let code = if report_check.missing {
            ctx_traits_core::assist::DiagnosticCode::ImportReportMissing
        } else {
            ctx_traits_core::assist::DiagnosticCode::ImportReportDrift
        };
        candidate
            .diagnostics
            .push(ctx_traits_core::assist::Diagnostic {
                gate: ctx_traits_core::assist::Gate::Drift,
                code,
                field: Some(report_path.to_string()),
                message: "import report target does not match deterministic import evidence"
                    .to_string(),
            });
    }
    candidate.gate_summary.check.failed_sections.sort();
    candidate.gate_summary.check.failed_sections.dedup();

    let passed = trait_check.passed && report_check.passed;
    let evidence = ImportDriftEvidence {
        passed,
        trait_path: trait_path.to_string(),
        report_path: report_path.to_string(),
        trait_missing: trait_check.missing,
        report_missing: report_check.missing,
        trait_drift: !trait_check.passed && !trait_check.missing,
        report_drift: !report_check.passed && !report_check.missing,
        trait_expected_digest: trait_check.expected_digest,
        trait_actual_digest: trait_check.actual_digest,
        report_expected_digest: report_check.expected_digest,
        report_actual_digest: report_check.actual_digest,
    };

    (candidate, evidence)
}

struct ArtifactComparison {
    passed: bool,
    missing: bool,
    expected_digest: Option<String>,
    actual_digest: Option<String>,
}

fn compare_expected_artifact(
    path: &camino::Utf8Path,
    expected_text: Option<&str>,
) -> ArtifactComparison {
    let expected_digest = expected_text.map(|text| {
        ctx_traits_core::digest::Digest::source(text)
            .as_str()
            .to_string()
    });
    let Some(expected_text) = expected_text else {
        return ArtifactComparison {
            passed: false,
            missing: true,
            expected_digest,
            actual_digest: None,
        };
    };
    match ctx_traits_io::read::read_text(path) {
        Ok(actual_text) => {
            let actual_digest = ctx_traits_core::digest::Digest::source(&actual_text)
                .as_str()
                .to_string();
            let expected_digest_value = ctx_traits_core::digest::Digest::source(expected_text)
                .as_str()
                .to_string();
            ArtifactComparison {
                passed: actual_digest == expected_digest_value,
                missing: false,
                expected_digest: Some(expected_digest_value),
                actual_digest: Some(actual_digest),
            }
        }
        Err(_) => ArtifactComparison {
            passed: false,
            missing: true,
            expected_digest,
            actual_digest: None,
        },
    }
}

fn write_multi_file_resources(
    package_root: &camino::Utf8Path,
    mf: &ctx_traits_io::import::MultiFileSkillSource,
    resource_mappings: &[ctx_traits_core::import::plan::ResourceIdMapping],
) -> crate::Result<()> {
    let checklist_resource_ids: std::collections::BTreeSet<&str> = resource_mappings
        .iter()
        .filter(|mapping| mapping.variant == "checklist")
        .map(|mapping| mapping.resource_id.as_str())
        .collect();
    let resources_dir = package_root.join("resources");
    for (rel_path, content) in &mf.included_files {
        let resource_id = ctx_traits_core::import::plan::resource_id_from_path(rel_path);
        // A file converted to a typed checklist resource has no active
        // Markdown copy: the typed items are the only canonical body, and
        // the raw file remains preserved under `imported/` (P405).
        if checklist_resource_ids.contains(resource_id.as_str()) {
            continue;
        }
        let resource_file = resources_dir.join(format!("{resource_id}.md"));
        ctx_traits_io::write::write_resource_file(&resource_file, content)?;
    }
    Ok(())
}

struct RefreshImportCandidate {
    draft_json: serde_json::Value,
    canonical_digest: Option<String>,
    output_text: String,
    report_json: String,
    multi_file_source: Option<ctx_traits_io::import::MultiFileSkillSource>,
    resource_mappings: Vec<ctx_traits_core::import::plan::ResourceIdMapping>,
}

fn resource_participating_paths(
    mappings: &[ctx_traits_core::import::plan::ResourceIdMapping],
) -> std::collections::BTreeSet<String> {
    mappings
        .iter()
        .map(|mapping| mapping.source_path.clone())
        .collect()
}

fn reject_blocked_participating_artifacts(
    snapshot: &ctx_traits_core::import::plan::TraitLockSnapshot,
    extra_participating_paths: &std::collections::BTreeSet<String>,
) -> crate::Result<()> {
    let blocked: Vec<&str> = snapshot
        .artifacts
        .iter()
        .filter(|artifact| {
            (artifact.participated_in_conversion
                || extra_participating_paths.contains(artifact.normalized_path.as_str()))
                && matches!(
                    artifact.content,
                    ctx_traits_core::import::plan::ArtifactContent::Blocked { .. }
                )
        })
        .map(|artifact| artifact.normalized_path.as_str())
        .collect();
    if blocked.is_empty() {
        return Ok(());
    }
    Err(crate::Error::Command {
        message: format!(
            "import blocked: conversion-participating artifact(s) cannot be embedded: {}",
            blocked.join(", ")
        ),
    })
}

fn staging_dir_for(
    package_root: &camino::Utf8Path,
    kind: &str,
) -> crate::Result<camino::Utf8PathBuf> {
    let parent = package_root.parent().ok_or_else(|| crate::Error::Command {
        message: format!("import target package has no parent: {package_root}"),
    })?;
    let name = package_root
        .file_name()
        .ok_or_else(|| crate::Error::Command {
            message: format!("import target package has no final path segment: {package_root}"),
        })?;
    Ok(parent.join(format!(".{name}.{kind}-{}", std::process::id())))
}

fn remove_dir_if_exists(path: &camino::Utf8Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(crate::Error::Command {
                    message: format!("staging cleanup target is not a real directory: {path}"),
                });
            }
            std::fs::remove_dir_all(path).map_err(|source| {
                ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                    path: path.to_string(),
                    source,
                })
            })?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ctx_traits_io::Error::from(
                ctx_traits_io::environment::Error::Filesystem {
                    path: path.to_string(),
                    source,
                },
            )
            .into());
        }
    }
    Ok(())
}

fn validate_existing_package_root(package_root: &camino::Utf8Path) -> crate::Result<bool> {
    match std::fs::symlink_metadata(package_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(crate::Error::Command {
                    message: format!(
                        "import package target must be an absent path or a real directory: {package_root}"
                    ),
                });
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ctx_traits_io::Error::from(
            ctx_traits_io::environment::Error::Filesystem {
                path: package_root.to_string(),
                source,
            },
        )
        .into()),
    }
}

/// Inputs for [`write_complete_import_package`], grouped to keep the
/// function under clippy's argument-count lint without an `#[allow]`.
struct CompleteImportPackageWrite<'a> {
    package_root: &'a camino::Utf8Path,
    source_path: &'a camino::Utf8Path,
    trait_text: &'a str,
    report_json: &'a str,
    lock: &'a ctx_traits_core::import::plan::TraitLock,
    multi_file_source: Option<&'a ctx_traits_io::import::MultiFileSkillSource>,
    resource_mappings: &'a [ctx_traits_core::import::plan::ResourceIdMapping],
    trait_id: &'a str,
    trait_name: &'a str,
}

fn write_complete_import_package(
    write: CompleteImportPackageWrite<'_>,
) -> crate::Result<ctx_traits_io::import::ImportWriteResult> {
    let CompleteImportPackageWrite {
        package_root,
        source_path,
        trait_text,
        report_json,
        lock,
        multi_file_source,
        resource_mappings,
        trait_id,
        trait_name,
    } = write;
    let final_trait_path = ctx_traits_io::layout::package_manifest_write_path(package_root);
    let final_report_path = package_root.join("import-report.json");
    let existing = validate_existing_package_root(package_root)?;
    let managed_overwrite = if existing {
        let managed = ctx_traits_io::import::validate_managed_import_overwrite(&final_trait_path)?;
        if !managed {
            return Err(crate::Error::Command {
                message: format!(
                    "refusing to replace existing import package without managed trait.toml: {package_root}"
                ),
            });
        }
        true
    } else {
        false
    };
    let existing_lock_document = if existing {
        ctx_traits_io::lockfile::read_lockfile(package_root)?
    } else {
        None
    };
    let _serialized_lock = serde_json::to_string_pretty(lock)
        .map_err(|e| crate::Error::json("serialize trait.lock before import write", e))?;

    let stage_dir = staging_dir_for(package_root, "ctx-traits-staging")?;
    let backup_dir = staging_dir_for(package_root, "ctx-traits-backup")?;
    if stage_dir.exists() || backup_dir.exists() {
        return Err(crate::Error::Command {
            message: format!(
                "import staging path already exists: {} or {}",
                stage_dir, backup_dir
            ),
        });
    }

    let stage_result = (|| -> crate::Result<ctx_traits_io::import::ImportWriteResult> {
        let stage_copy_plan = ctx_traits_io::import::plan_import_copy(&stage_dir, source_path)?;
        // Mirror package_manifest_write_path: canonical package roots stage
        // generated/index.toml — staging the legacy generated/trait.toml left
        // imported traits invisible to by-ID resolution (GP-1's first catch).
        let stage_trait_path = if ctx_traits_io::layout::is_canonical_package_root(package_root) {
            stage_dir
                .join(ctx_traits_io::layout::GENERATED)
                .join(ctx_traits_io::layout::CANONICAL_MANIFEST)
        } else {
            stage_dir.join(ctx_traits_io::layout::TRAIT_MANIFEST)
        };
        let result = ctx_traits_io::import::write_import_package(
            &stage_trait_path,
            trait_text,
            report_json,
            &stage_copy_plan,
        )?;
        ctx_traits_io::import::stage_package_manifest(
            package_root,
            &stage_dir,
            existing.then_some(package_root),
            trait_id,
            trait_name,
        )?;
        let mut lock_document = existing_lock_document.clone().unwrap_or_default();
        lock_document.import = Some(lock.clone());
        ctx_traits_io::lockfile::write_lockfile(&stage_dir, &mut lock_document)?;
        if let Some(mf) = multi_file_source {
            write_multi_file_resources(&stage_dir, mf, resource_mappings)?;
        }
        Ok(result)
    })();

    let stage_result = match stage_result {
        Ok(result) => result,
        Err(e) => {
            let _ = remove_dir_if_exists(&stage_dir);
            return Err(e);
        }
    };

    let mut backup_created = false;
    if existing {
        ctx_traits_io::write::rename_dir(package_root, &backup_dir)?;
        backup_created = true;
    }

    if let Err(e) = ctx_traits_io::write::rename_dir(&stage_dir, package_root) {
        let mut notes = Vec::new();
        if backup_created
            && let Err(restore_err) = ctx_traits_io::write::rename_dir(&backup_dir, package_root)
        {
            notes.push(format!(
                    "package's original bytes are still in backup directory {backup_dir}, restore failed: {restore_err}"
                ));
        }
        let _ = remove_dir_if_exists(&stage_dir);
        return Err(if notes.is_empty() {
            e.into()
        } else {
            ctx_traits_io::error::with_rollback_notes(e, notes).into()
        });
    }

    if backup_created {
        remove_dir_if_exists(&backup_dir)?;
    }

    Ok(ctx_traits_io::import::ImportWriteResult {
        trait_path: final_trait_path.to_string(),
        report_path: final_report_path.to_string(),
        copied_entries: stage_result.copied_entries,
        managed_overwrite,
    })
}

pub(crate) struct ImportInputs<'a> {
    pub(crate) source: &'a str,
    pub(crate) source_profile: Option<&'a str>,
    pub(crate) out: Option<&'a str>,
    pub(crate) check: bool,
    pub(crate) llm_assisted: bool,
    pub(crate) model: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) candidate_path: Option<&'a str>,
    /// `--run-profile <PATH>`: caller-selected LLM runtime routing/budget
    /// for the harness-backed `import-trait` runner (P403). Distinct from
    /// `source_profile`, which selects the source-format parser. Has no
    /// effect when `llm_assisted` is false or `candidate_path` is set, since
    /// neither path dispatches a harness.
    pub(crate) run_profile: Option<&'a str>,
    pub(crate) json: bool,
    pub(crate) verbose: bool,
}

pub(crate) fn handle_import(input: ImportInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let ImportInputs {
        source,
        source_profile,
        out,
        check,
        llm_assisted,
        model,
        assignments,
        candidate_path,
        run_profile,
        json,
        verbose,
    } = input;
    if matches!(
        ctx_traits_core::import::plan::classify_source(source),
        ctx_traits_core::import::plan::ImportSource::Git { .. }
    ) {
        let fetch_result = ctx_traits_io::import::fetch_remote_source(source);
        let error_message = fetch_result
            .error
            .clone()
            .unwrap_or_else(|| "remote import failed".to_string());

        if json {
            let output = RemoteSourceOutput {
                remote_source_evidence: fetch_result.evidence.as_ref(),
                error: &error_message,
                status: "manual-download-required",
            };
            print_json_report(&output, "remote import")?;
        } else {
            println!("ctx traits import — remote source");
            println!("  status: manual-download-required");
            if let Some(ref evidence) = fetch_result.evidence {
                println!(
                    "  source-kind: {}",
                    serde_json::to_string(&evidence.kind).unwrap_or_default()
                );
                println!("  original-url: {}", evidence.original_url);
                println!("  canonical-url: {}", evidence.canonical_url);
                println!("  requested-ref: {}", evidence.requested_ref);
                println!("  floating: {}", evidence.is_floating);
                println!(
                    "  fetch-method: {}",
                    serde_json::to_string(&evidence.fetch_method).unwrap_or_default()
                );
                for w in &evidence.warnings {
                    println!("  warning: {w}");
                }
            }
            println!("  guidance: {error_message}");
        }
        return Err(crate::Error::Command {
            message: error_message,
        });
    }
    let source_path = camino::Utf8Path::new(source);
    let import_profile_text = source_profile.map_or("agent-skills", |profile| profile);
    let import_profile = ctx_traits_core::import::plan::ImportProfile::parse(import_profile_text);

    let no_prior_checklists = std::collections::BTreeMap::new();
    let mut analysis = analyze_import_source(
        source_path,
        import_profile.clone(),
        "ctx-traits-import",
        "agent-skills-import",
        &no_prior_checklists,
    )?;

    let (package_path, output_path) = trait_package_output_paths(&analysis.trait_id, out);
    let package_root = camino::Utf8Path::new(&package_path);

    // A repeated managed import over an existing package must reconcile
    // checklist item identity against what is already canonical, so a
    // reworded criterion keeps its id instead of reading as remove/add
    // (P405). Re-plan once prior checklist state is known.
    if let Some(existing_manifest) = ctx_traits_io::layout::resolve_package_manifest(package_root) {
        let (old_trait, ..) = ctx_traits_io::run::load_trait(existing_manifest.as_str())?;
        let old_trait_json = serde_json::to_value(&old_trait).map_err(|e| {
            crate::Error::json("serialize existing trait for import reconciliation", e)
        })?;
        let prior_checklists =
            ctx_traits_core::import::plan::extract_prior_checklists(&old_trait_json);
        if !prior_checklists.is_empty() {
            analysis = analyze_import_source(
                source_path,
                import_profile,
                "ctx-traits-import",
                "agent-skills-import",
                &prior_checklists,
            )?;
        }
    }

    let ImportSourceAnalysis {
        trait_id,
        trait_name,
        summary: _analysis_summary,
        raw_source_digest,
        loaded_source,
        multi_file_source,
        multi_file_resource_mappings,
        draft_json,
        mut report,
        synth_output_text,
        scaffold: trait_scaffold,
        decode_warnings,
    } = analysis;
    ctx_traits_io::decode_diagnostics::print_decode_warnings("import scaffold", &decode_warnings);

    let trait_path = camino::Utf8Path::new(&output_path);
    let report_path = package_root.join("import-report.json");
    let copy_plan = ctx_traits_io::import::plan_import_copy(package_root, source_path)?;

    if copy_plan.warnings.iter().any(|warning| {
        matches!(
            warning,
            ctx_traits_io::import::ImportCopyWarning::UnsafeSourceLayout { .. }
        )
    }) {
        report
            .warnings
            .push(ctx_traits_core::import::plan::ImportWarning::IncompletePreservation);
        report
            .conversion_warnings
            .push("raw source preservation has unsafe source layout warnings".to_string());
    }
    let report_json = import_report_json(&report)?;

    let assist_candidate = if llm_assisted {
        let candidate = ctx_traits_core::assist::plan_assist_boundary(
            ctx_traits_core::assist::BoundaryRequest {
                operation: ctx_traits_core::assist::Operation::Import,
                source_trait_ids: vec![trait_id.clone()],
                source_paths: vec![source.to_string()],
                source_digests: vec![raw_source_digest.clone()],
                user_request: format!("llm-assisted import enrichment for {}", trait_id),
                model: model.map(str::to_string),
                target_path: output_path.clone(),
                provider_available: candidate_path.is_none(),
                context: serde_json::json!({
                    "source-profile": report.source_profile.as_str(),
                    "trait-id": trait_id,
                    "raw-source-digest": raw_source_digest.as_str(),
                    "target-schema": "agent-traits/canonical-trait",
                }),
            },
        )?;

        let mut final_report = report.clone();
        let mut write_result_report = None;
        let mut check_report = None;
        let (raw, raw_encoding) = match candidate_path {
            Some(cand_path) => {
                let cand_utf8 = camino::Utf8Path::new(cand_path);
                (
                    ctx_traits_io::read::read_text(cand_utf8)?,
                    ctx_traits_core::encoding::Encoding::from_path(cand_utf8)?,
                )
            }
            None => (
                {
                    let run_profile_document = run_profile
                        .map(camino::Utf8Path::new)
                        .map(ctx_traits_io::harness_config::load_run_profile_document)
                        .transpose()?;
                    match run_builtin_trait(
                        "import-trait",
                        vec![
                            runtime_input("scaffold", synth_output_text.as_str()),
                            runtime_input("source", loaded_source.skill_markdown.as_str()),
                            runtime_input("source-profile", report.source_profile.as_str()),
                            runtime_input("trait-id", trait_id.as_str()),
                            runtime_input("target-schema", "agent-traits/canonical-trait"),
                        ],
                        assignments,
                        model,
                        run_profile_document.as_ref(),
                    ) {
                        Ok(raw) => raw,
                        Err(error) => {
                            return blocked_assist_candidate(candidate, json, error.to_string());
                        }
                    }
                },
                ctx_traits_core::encoding::Encoding::Json,
            ),
        };
        let (candidate_text, wrapped_draft) = match decode_generate_candidate(&raw) {
            Ok(decoded) => decoded,
            Err(error) => return blocked_assist_candidate(candidate, json, error.to_string()),
        };
        let candidate_encoding = if wrapped_draft.is_some() {
            ctx_traits_core::encoding::Encoding::Json
        } else {
            raw_encoding
        };
        let mut evaluation = ctx_traits_core::assist::evaluate_supplied_candidate(
            candidate,
            &candidate_text,
            candidate_encoding,
        );
        if wrapped_draft.is_some() {
            // The runtime emits a typed wrapper, but the candidate gate remains the
            // single trait validation path. Its audit identity must name the full
            // model output rather than the extracted trait alone.
            evaluation.candidate.raw_output_digest =
                Some(ctx_traits_core::digest::Digest::source(&raw));
            evaluation.candidate.parsed_candidate_digest = Some(
                ctx_traits_core::digest::Digest::canonical(&canonical_json_value(&raw)?),
            );
            evaluation.candidate = ctx_traits_core::assist::audit_wrapper_output(
                evaluation.candidate,
                &raw,
                &trait_id,
            );
        }

        let final_candidate = {
            let mut c = attach_assist_check_report(
                evaluation.candidate,
                evaluation.normalized_trait.as_ref(),
                evaluation.normalized_output_text.as_deref(),
                package_root,
            )?;

            if let Some(ref cand_id) = c.candidate_trait_id
                && cand_id != &trait_id
            {
                c.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                c.warnings.push(format!(
                    "candidate trait ID {cand_id} does not match import trait ID {}",
                    trait_id
                ));
            }
            if let Some(text) = evaluation.normalized_output_text.as_deref() {
                let trait_digest = ctx_traits_core::digest::Digest::source(text);
                final_report.managed_import =
                    Some(ctx_traits_core::import::plan::ManagedImportArtifact::new(
                        trait_id.clone(),
                        final_report.source_profile.clone(),
                        raw_source_digest.clone(),
                        trait_digest,
                    ));
            }
            let final_report_json = import_report_json(&final_report)?;
            if check {
                let (checked_candidate, evidence) = apply_assist_import_check_drift(
                    c,
                    trait_path,
                    evaluation.normalized_output_text.as_deref(),
                    &report_path,
                    &final_report_json,
                );
                c = checked_candidate;
                check_report = Some(evidence);
            }

            // Import persists the default draft package by default (Group
            // 95): `--out` only overrides where it lands, `--check` is the
            // sole non-mutating mode, so the write no longer requires
            // `out.is_some()`.
            if c.gate_summary.all_passed()
                && c.status != ctx_traits_core::assist::CandidateStatus::Blocked
                && !check
            {
                match evaluation.normalized_output_text.as_deref() {
                    Some(text) => {
                        let (cand_snapshot, cand_warnings) =
                            ctx_traits_io::import::build_artifact_snapshot_with_locator(
                                source_path,
                                import_profile_text,
                                source,
                            )?;

                        let participating_paths = resource_participating_paths(
                            multi_file_resource_mappings.as_deref().unwrap_or(&[]),
                        );
                        reject_blocked_participating_artifacts(
                            &cand_snapshot,
                            &participating_paths,
                        )?;

                        let cand_canonical_digest = evaluation
                            .normalized_trait
                            .as_ref()
                            .map(ctx_traits_core::digest::canonical_digest)
                            .transpose()?;

                        let mut lock = ctx_traits_io::import::read_trait_lock(package_root)?
                            .unwrap_or_else(|| {
                                ctx_traits_core::import::plan::TraitLock::new(&trait_id)
                            });
                        let mut cand_snapshot = cand_snapshot;
                        cand_snapshot.canonical_output_digest = cand_canonical_digest;
                        cand_snapshot.metadata.frontmatter_mapping =
                            final_report.frontmatter.clone();
                        if let Some(ref mf) = multi_file_source {
                            cand_snapshot.metadata.graph_digest =
                                Some(mf.graph.graph_digest.clone());
                        }
                        cand_snapshot.metadata.resource_mappings =
                            multi_file_resource_mappings.clone().unwrap_or_default();
                        lock.insert_snapshot(cand_snapshot);

                        match write_complete_import_package(CompleteImportPackageWrite {
                            package_root,
                            source_path,
                            trait_text: text,
                            report_json: &final_report_json,
                            lock: &lock,
                            multi_file_source: multi_file_source.as_ref(),
                            resource_mappings: multi_file_resource_mappings
                                .as_deref()
                                .unwrap_or(&[]),
                            trait_id: &trait_id,
                            trait_name: &trait_name,
                        }) {
                            Ok(result) => {
                                write_result_report = Some(ImportWriteResultReport::from(&result));

                                for w in &cand_warnings {
                                    eprintln!("warning: trait.lock artifact: {w}");
                                }

                                ctx_traits_core::assist::mark_written(c, &output_path)
                            }
                            Err(_) => {
                                c.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                                c.warnings.push(
                                    "write gate failed: managed import writer rejected target path"
                                        .to_string(),
                                );
                                c
                            }
                        }
                    }
                    None => c,
                }
            } else {
                c
            }
        };

        let combined = ImportAssistOutput {
            assist_candidate: &final_candidate,
            trait_scaffold: &trait_scaffold,
            import_report: &final_report,
            target: ImportTargetReport {
                package_path: package_path.clone(),
                trait_path: output_path.clone(),
                report_path: report_path.to_string(),
                written: final_candidate.write_status
                    != ctx_traits_core::assist::WriteStatus::NotWritten,
            },
            copy_plan: import_copy_plan_report(&copy_plan),
            write_result: write_result_report,
            check: check_report,
        };
        if json {
            print_json_report(&combined, "combined import output")?;
        } else {
            print_assist_candidate(&final_candidate, false)?;
            println!();
            println!("import-report:");
            println!("  trait-id: {}", trait_id);
            println!("  source-profile: {}", final_report.source_profile.as_str());
            println!(
                "  default-lifecycle: status={}, trust={}",
                final_report.default_lifecycle.status, final_report.default_lifecycle.trust
            );
            println!(
                "  trait-scaffold: declarations={} check-required={}",
                trait_scaffold.declarations.len(),
                trait_scaffold.check.required
            );
            println!("  target-path: {output_path}");
            if let Some(check) = &combined.check {
                let check_json = serde_json::to_string(check)
                    .map_err(|e| crate::Error::json("serialize import check", e))?;
                println!("import-check: {check_json}");
            }
        }

        if final_candidate.status == ctx_traits_core::assist::CandidateStatus::UnsupportedProvider {
            return Err(crate::Error::Command {
                message: "import --llm-assisted failed: model provider adapter is unavailable; no candidate was produced or written".to_string(),
            });
        }
        if final_candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked {
            return Err(crate::Error::Command {
                message: if check {
                    "import --llm-assisted --check failed: candidate was blocked by validation gates".to_string()
                } else {
                    "import --llm-assisted failed: candidate was blocked by validation gates; no file was written".to_string()
                },
            });
        }
        return Ok(CommandOutput::new(()));
    } else {
        None::<ctx_traits_core::assist::Candidate>
    };

    if let Some(ref _candidate) = assist_candidate {}

    if check {
        return handle_import_check(ImportCheckInputs {
            source,
            trait_path,
            report_path: &report_path,
            expected_trait: &synth_output_text,
            expected_report: &report_json,
            report: &report,
            trait_scaffold: &trait_scaffold,
            copy_plan: &copy_plan,
            json,
        });
    }

    // Import persists the default draft package by default (Group 95):
    // `--out` only overrides where the package lands, `--check` (handled
    // above) is the sole non-mutating mode.
    let (snapshot, lock_warnings) = ctx_traits_io::import::build_artifact_snapshot_with_locator(
        source_path,
        import_profile_text,
        source,
    )?;

    let participating_paths =
        resource_participating_paths(multi_file_resource_mappings.as_deref().unwrap_or(&[]));
    reject_blocked_participating_artifacts(&snapshot, &participating_paths)?;

    let canonical_digest = Some(ctx_traits_core::digest::canonical_digest(&draft_json)?);

    let mut lock = ctx_traits_io::import::read_trait_lock(package_root)?
        .unwrap_or_else(|| ctx_traits_core::import::plan::TraitLock::new(&trait_id));

    let mut snapshot = snapshot;
    snapshot.canonical_output_digest = canonical_digest;
    snapshot.metadata.frontmatter_mapping = report.frontmatter.clone();
    if let Some(ref mf) = multi_file_source {
        snapshot.metadata.graph_digest = Some(mf.graph.graph_digest.clone());
    }
    snapshot.metadata.resource_mappings = multi_file_resource_mappings.clone().unwrap_or_default();
    lock.insert_snapshot(snapshot);

    let write_result = write_complete_import_package(CompleteImportPackageWrite {
        package_root,
        source_path,
        trait_text: &synth_output_text,
        report_json: &report_json,
        lock: &lock,
        multi_file_source: multi_file_source.as_ref(),
        resource_mappings: multi_file_resource_mappings.as_deref().unwrap_or(&[]),
        trait_id: &trait_id,
        trait_name: &trait_name,
    })?;
    for w in &lock_warnings {
        eprintln!("warning: trait.lock artifact: {w}");
    }

    match OutputMode::select(json, verbose) {
        OutputMode::Json => {
            let output = ImportWrittenOutput {
                import_report: &report,
                trait_scaffold: &trait_scaffold,
                copy_plan: import_copy_plan_report(&copy_plan),
                write_result: ImportWriteResultReport::from(&write_result),
            };
            print_json_report(&output, "import output")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new("ctx", "import", PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned("source", source, RowTone::Default))
                .row(PanelRow::toned(
                    "profile",
                    report.source_profile.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "target",
                    output_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned("status", "written", RowTone::Pass))
                .row(PanelRow::toned(
                    "copied-entries",
                    write_result.copied_entries.to_string(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "managed-overwrite",
                    write_result.managed_overwrite.to_string(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "findings",
                    format!(
                        "inferred: {} · unsupported: {} · warnings: {} · hidden-content: {}",
                        report.inferred_fields.len(),
                        report.unsupported_fields.len(),
                        report.warnings.len(),
                        report.hidden_content_findings.len()
                    ),
                    if report.warnings.is_empty() && report.hidden_content_findings.is_empty() {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                ))
                .next(PanelRow::toned(
                    "next",
                    format!(
                        "package status is draft and this machine has not reviewed the imported \
                         digest; run `ctx traits check {}`, then when the team is ready `ctx \
                         traits activate {}` and `ctx traits trust approve {}`",
                        trait_id, trait_id, trait_id
                    ),
                    RowTone::Default,
                ));
            emit_human(false, &panel, mode, || {
                print_import_summary(
                    source,
                    &output_path,
                    Some(write_result.report_path.as_str()),
                    &report,
                    &copy_plan,
                );
                println!("  status: written");
                println!("  copied-entries: {}", write_result.copied_entries);
                println!("  managed-overwrite: {}", write_result.managed_overwrite);
                println!(
                    "  next-action: package status is draft and this machine has not reviewed \
                     the imported digest; run `ctx traits check {}`, then when the team is \
                     ready `ctx traits activate {}` and `ctx traits trust approve {}`",
                    trait_id, trait_id, trait_id
                );
                Ok(())
            })?;
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_import_refresh(
    trait_id_or_package: &str,
    source_override: Option<&str>,
    check: bool,
    out: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let package_root = camino::Utf8Path::new(trait_id_or_package);
    let trait_path = if ctx_traits_io::layout::resolve_package_manifest(package_root).is_some() {
        package_root.to_path_buf()
    } else {
        camino::Utf8Path::new(ctx_traits_io::layout::trait_protocol_root())
            .join(trait_id_or_package)
    };

    let existing_lock = ctx_traits_io::import::read_trait_lock(&trait_path)?
        .unwrap_or_else(|| ctx_traits_core::import::plan::TraitLock::new(trait_id_or_package));

    let old_snapshot = existing_lock.current_snapshot();

    let source_path = match source_override {
        Some(s) => s.to_string(),
        None => match old_snapshot.and_then(|s| s.metadata.source_locator.as_ref()) {
            Some(loc) => loc.clone(),
            None => {
                return Err(crate::Error::Command {
                    message: "no source path in trait.lock; pass --source to refresh".to_string(),
                });
            }
        },
    };

    if matches!(
        ctx_traits_core::import::plan::classify_source(&source_path),
        ctx_traits_core::import::plan::ImportSource::Git { .. }
    ) {
        let fetch_result = ctx_traits_io::import::fetch_remote_source(&source_path);
        let error_message = fetch_result
            .error
            .clone()
            .unwrap_or_else(|| "remote refresh failed".to_string());
        if json {
            let output = RemoteSourceOutput {
                remote_source_evidence: fetch_result.evidence.as_ref(),
                error: &error_message,
                status: "manual-download-required",
            };
            print_json_report(&output, "remote refresh")?;
        } else {
            println!("ctx traits import refresh — remote source");
            println!("  status: manual-download-required");
            if let Some(ref evidence) = fetch_result.evidence {
                println!("  source-kind: {:?}", evidence.kind);
                println!("  original-url: {}", evidence.original_url);
            }
            println!("  guidance: {error_message}");
        }
        return Err(crate::Error::Command {
            message: error_message,
        });
    }

    let source = camino::Utf8Path::new(&source_path);
    let (mut new_snapshot, warnings) = ctx_traits_io::import::build_artifact_snapshot_with_locator(
        source,
        "agent-skills",
        &source_path,
    )?;

    let old_canonical_json =
        if let Some(old_manifest) = ctx_traits_io::layout::resolve_package_manifest(&trait_path) {
            let (old_trait, _old_root, _old_source_digest, _old_canonical_digest) =
                ctx_traits_io::run::load_trait(old_manifest.as_str())?;
            serde_json::to_value(&old_trait).ok()
        } else {
            None
        };

    let prior_checklists = old_canonical_json
        .as_ref()
        .map(ctx_traits_core::import::plan::extract_prior_checklists)
        .unwrap_or_default();

    let refresh_candidate = {
        let loaded_source = ctx_traits_io::import::read_agent_skill_source(source)?;
        let raw_source_digest = ctx_traits_io::import::digest_import_source(source)?;
        let mut import_plan = ctx_traits_core::import::plan::plan_agent_skills_import(
            ctx_traits_core::import::plan::AgentSkillsImportRequest {
                source: ctx_traits_core::import::plan::ImportSource::Local {
                    path: source_path.clone(),
                },
                source_profile: ctx_traits_core::import::plan::ImportProfile::AgentSkills,
                raw_source_digest: raw_source_digest.clone(),
                source_path: loaded_source.skill_path.to_string(),
                source_name: loaded_source.source_name.clone(),
                skill_markdown: loaded_source.skill_markdown,
                prior_checklists: prior_checklists.clone(),
            },
        )?;

        let refresh_multi_file_source = if source.is_dir() {
            Some(ctx_traits_io::import::read_multi_file_skill_source(source)?)
        } else {
            None
        };
        let (refresh_resource_mappings, refresh_checklist_resources) =
            match refresh_multi_file_source.as_ref() {
                Some(mf) => multi_file_resource_mappings(mf, &prior_checklists)?,
                None => (Vec::new(), std::collections::BTreeMap::new()),
            };
        if let Some(ref mf) = refresh_multi_file_source {
            augment_draft_with_multi_file_resources(
                &mut import_plan.draft_json,
                &refresh_resource_mappings,
                &refresh_checklist_resources,
            )?;
            import_plan.report.conversion_warnings.push(format!(
                "multi-file refresh: {} support file(s) discovered, {} edge(s) in artifact graph",
                mf.included_files.len(),
                mf.graph.edges.len()
            ));
            for w in &mf.graph_warnings {
                import_plan.report.conversion_warnings.push(w.clone());
            }
        }

        let new_digest = Some(ctx_traits_core::digest::canonical_digest(
            &import_plan.draft_json,
        )?);
        new_snapshot.canonical_output_digest = new_digest.clone();
        if let Some(ref mf) = refresh_multi_file_source {
            new_snapshot.metadata.graph_digest = Some(mf.graph.graph_digest.clone());
        }
        new_snapshot.metadata.resource_mappings = refresh_resource_mappings.clone();

        let synth_response = ctx_traits_core::synth::synthesize(ctx_traits_core::synth::Request {
            document_kind: ctx_traits_core::synth::DocumentKind::Trait,
            draft_json: import_plan.draft_json.clone(),
            output_format: ctx_traits_core::synth::OutputFormat::Toml,
            provenance: ctx_traits_core::synth::ProvenanceSeed {
                generator_package: Some("ctx-traits-import-refresh".to_string()),
                generator_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                source_path: Some(loaded_source.skill_path.to_string()),
                warnings: import_plan.report.conversion_warnings.clone(),
            },
        })?;
        let trait_digest = ctx_traits_core::digest::Digest::source(&synth_response.output_text);
        let mut report = import_plan.report;
        report.synth_provenance = Some(synth_response.provenance.clone());
        report.managed_import = Some(ctx_traits_core::import::plan::ManagedImportArtifact::new(
            existing_lock.trait_id.clone(),
            report.source_profile.clone(),
            raw_source_digest.clone(),
            trait_digest,
        ));
        if let Some(ref mf) = refresh_multi_file_source {
            report.multi_file_evidence =
                Some(multi_file_evidence(mf, refresh_resource_mappings.clone()));
        }
        let report_json = import_report_json(&report)?;

        RefreshImportCandidate {
            draft_json: import_plan.draft_json,
            canonical_digest: new_digest.as_ref().map(ToString::to_string),
            output_text: synth_response.output_text,
            report_json,
            multi_file_source: refresh_multi_file_source,
            resource_mappings: refresh_resource_mappings,
        }
    };

    let diff = ctx_traits_core::import::plan::compute_refresh_diff_full(
        &existing_lock.trait_id,
        old_snapshot,
        &new_snapshot,
        refresh_candidate.canonical_digest.as_deref(),
        old_canonical_json.as_ref(),
        Some(&refresh_candidate.draft_json),
    );

    let drift = !matches!(
        diff.decision,
        ctx_traits_core::import::plan::RefreshDecision::NoChange
    );

    if json {
        print_json_report(&diff, "import refresh")?;
    } else {
        println!("ctx traits import refresh");
        println!("  trait: {}", existing_lock.trait_id);
        println!("  decision: {:?}", diff.decision);
        if !diff.artifact_diff.added.is_empty() {
            println!("  added: {}", diff.artifact_diff.added.join(", "));
        }
        if !diff.artifact_diff.removed.is_empty() {
            println!("  removed: {}", diff.artifact_diff.removed.join(", "));
        }
        for m in &diff.artifact_diff.modified {
            println!(
                "  modified: {} ({} -> {})",
                m.path, m.before_digest, m.after_digest
            );
        }
        for fc in &diff.trait_diff.field_changes {
            println!("  trait-change: {fc}");
        }
        for md in &diff.mapping_diff {
            println!(
                "  mapping: {} -> {}",
                md.canonical_field, md.source_attribution
            );
        }
        for w in &warnings {
            println!("  warning: {w}");
        }
        for w in &diff.warnings {
            println!("  blocked: {w}");
        }
    }

    if drift && check {
        return Err(crate::Error::Command {
            message: "import refresh detected drift; --check exits nonzero".to_string(),
        });
    }

    if drift && out.is_some() && !check {
        if matches!(
            diff.decision,
            ctx_traits_core::import::plan::RefreshDecision::Blocked
                | ctx_traits_core::import::plan::RefreshDecision::Unsupported
        ) {
            return Err(crate::Error::Command {
                message: format!(
                    "import refresh --out refused: decision is {:?}; \
                     cannot write candidate package for blocked/unsupported refresh",
                    diff.decision
                ),
            });
        }
        if let Some(out_path) = out {
            let out_dir = camino::Utf8Path::new(out_path);
            let mut lock = ctx_traits_io::import::read_trait_lock(out_dir)?.unwrap_or_else(|| {
                ctx_traits_core::import::plan::TraitLock::new(&existing_lock.trait_id)
            });
            lock.insert_snapshot(new_snapshot);
            let write_result = write_complete_import_package(CompleteImportPackageWrite {
                package_root: out_dir,
                source_path: source,
                trait_text: &refresh_candidate.output_text,
                report_json: &refresh_candidate.report_json,
                lock: &lock,
                multi_file_source: refresh_candidate.multi_file_source.as_ref(),
                resource_mappings: &refresh_candidate.resource_mappings,
                trait_id: &existing_lock.trait_id,
                trait_name: &existing_lock.trait_id,
            })?;

            if json {
                let output = ImportRefreshCandidateWrittenOutput {
                    status: "candidate-written",
                    out: out_dir.to_string(),
                    decision: format!("{:?}", diff.decision),
                    write_result: ImportWriteResultReport::from(&write_result),
                };
                print_json_report(&output, "import refresh candidate output")?;
            } else {
                println!("  candidate-package: {}", out_dir);
                println!("  candidate-decision: {:?}", diff.decision);
                println!("  candidate-report: {}", write_result.report_path);
                println!("  copied-entries: {}", write_result.copied_entries);
            }
        }
    }

    Ok(CommandOutput::new(()))
}

struct ImportCheckInputs<'a> {
    source: &'a str,
    trait_path: &'a camino::Utf8Path,
    report_path: &'a camino::Utf8Path,
    expected_trait: &'a str,
    expected_report: &'a str,
    report: &'a ctx_traits_core::import::plan::ImportReport,
    trait_scaffold: &'a ctx_traits_core::scaffold::TraitScaffold,
    copy_plan: &'a ctx_traits_io::import::ImportCopyPlan,
    json: bool,
}

fn handle_import_check(input: ImportCheckInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let existing_trait = ctx_traits_io::read::read_optional_text(input.trait_path)?;
    let existing_report = ctx_traits_io::read::read_optional_text(input.report_path)?;
    let trait_drift = existing_trait.as_deref() != Some(input.expected_trait);
    let report_drift = existing_report.as_deref() != Some(input.expected_report);
    let passed = !trait_drift && !report_drift;
    let trait_expected_digest = ctx_traits_core::digest::Digest::source(input.expected_trait);
    let report_expected_digest = ctx_traits_core::digest::Digest::source(input.expected_report);
    let trait_actual_digest = existing_trait
        .as_deref()
        .map(ctx_traits_core::digest::Digest::source)
        .map(|digest| digest.as_str().to_string());
    let report_actual_digest = existing_report
        .as_deref()
        .map(ctx_traits_core::digest::Digest::source)
        .map(|digest| digest.as_str().to_string());

    match OutputMode::select(input.json, false) {
        OutputMode::Json => {
            let output = ImportCheckOutput {
                import_report: input.report,
                trait_scaffold: input.trait_scaffold,
                copy_plan: import_copy_plan_report(input.copy_plan),
                check: ImportCheckReport {
                    source: input.source,
                    trait_path: input.trait_path.to_string(),
                    report_path: input.report_path.to_string(),
                    passed,
                    trait_drift,
                    report_drift,
                    trait_expected_digest: trait_expected_digest.as_str(),
                    trait_actual_digest,
                    report_expected_digest: report_expected_digest.as_str(),
                    report_actual_digest,
                },
            };
            print_json_report(&output, "import check output")?;
        }
        OutputMode::Human(mode) => {
            let status = if passed {
                PanelStatus::Passed("passed".to_string())
            } else {
                PanelStatus::Blocked("blocked".to_string())
            };
            let panel = Panel::new("ctx", "import --check", status)
                .row(PanelRow::toned("source", input.source, RowTone::Default))
                .row(PanelRow::toned(
                    "trait-path",
                    input.trait_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "report-path",
                    input.report_path.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "trait-drift",
                    trait_drift.to_string(),
                    if trait_drift {
                        RowTone::Fail
                    } else {
                        RowTone::Pass
                    },
                ))
                .row(PanelRow::toned(
                    "report-drift",
                    report_drift.to_string(),
                    if report_drift {
                        RowTone::Fail
                    } else {
                        RowTone::Pass
                    },
                ))
                .row(PanelRow::toned(
                    "trait-expected-digest",
                    trait_expected_digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "trait-actual-digest",
                    trait_actual_digest.as_deref().unwrap_or("missing"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "report-expected-digest",
                    report_expected_digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "report-actual-digest",
                    report_actual_digest.as_deref().unwrap_or("missing"),
                    RowTone::Default,
                ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    if passed {
        Ok(CommandOutput::new(()))
    } else {
        Err(crate::Error::Command {
            message: format!(
                "import --check detected drift for {} or {}",
                input.trait_path, input.report_path
            ),
        })
    }
}

fn import_report_json(
    report: &ctx_traits_core::import::plan::ImportReport,
) -> crate::Result<String> {
    serde_json::to_string_pretty(report)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|e| crate::Error::json("serialize import report", e))
}

fn print_import_summary(
    source: &str,
    trait_path: &str,
    report_path: Option<&str>,
    report: &ctx_traits_core::import::plan::ImportReport,
    copy_plan: &ctx_traits_io::import::ImportCopyPlan,
) {
    println!("ctx traits import");
    println!("  source: {source}");
    println!("  profile: {}", report.source_profile.as_str());
    println!("  target: {trait_path}");
    if let Some(report_path) = report_path {
        println!("  import-report: {report_path}");
    }
    println!("  raw-source-digest: {}", report.raw_source_digest);
    println!(
        "  default-lifecycle: status={}, trust={}",
        report.default_lifecycle.status, report.default_lifecycle.trust
    );
    println!("  inferred-fields: {}", report.inferred_fields.len());
    for inferred in &report.inferred_fields {
        println!(
            "    {} = {:?} ({:?})",
            inferred.field_path, inferred.value, inferred.method
        );
    }
    println!("  unsupported-fields: {}", report.unsupported_fields.len());
    for unsupported in &report.unsupported_fields {
        println!(
            "    {}: {} ({})",
            unsupported.source_field, unsupported.value, unsupported.reason
        );
    }
    if let Some(frontmatter) = &report.frontmatter {
        println!("  frontmatter:");
        println!(
            "    raw-digest: {}",
            frontmatter.raw_digest.as_deref().unwrap_or("none")
        );
        println!("    trusted-policy: {}", frontmatter.trusted_policy);
        println!("    mapped-keys: {}", frontmatter.mapped_keys.len());
        for mapped in &frontmatter.mapped_keys {
            println!("      {} -> {}", mapped.source_key, mapped.target_field);
        }
        println!(
            "    unsupported-keys: {}",
            if frontmatter.unsupported_keys.is_empty() {
                "none".to_string()
            } else {
                frontmatter.unsupported_keys.join(", ")
            }
        );
    } else {
        println!("  frontmatter: none");
    }
    println!(
        "  conversion-warnings: {}",
        report.conversion_warnings.len()
    );
    for warning in &report.conversion_warnings {
        println!("    {warning}");
    }
    println!(
        "  hidden-content-findings: {}",
        report.hidden_content_findings.len()
    );
    for finding in &report.hidden_content_findings {
        println!(
            "    [{:?}] {:?}: {}",
            finding.severity, finding.code, finding.message
        );
    }
    if report.warnings.is_empty() {
        println!("  warnings: none");
    } else {
        println!("  warnings:");
        for warning in &report.warnings {
            println!("    {warning:?}");
        }
    }
    println!("  review-actions: {}", report.review_actions.len());
    for action in &report.review_actions {
        println!(
            "    {:?} {}: {}",
            action.action, action.target, action.detail
        );
    }
    println!("  raw-copy-entries: {}", copy_plan.entries.len());
    for entry in &copy_plan.entries {
        println!("    {} -> {}", entry.source, entry.destination);
    }
    for warning in &copy_plan.warnings {
        println!("  raw-copy-warning: {warning:?}");
    }
}

fn import_copy_plan_report(
    plan: &ctx_traits_io::import::ImportCopyPlan,
) -> ImportCopyPlanReport<'_> {
    let entries = plan
        .entries
        .iter()
        .map(|entry| ImportCopyPlanEntryReport {
            source: &entry.source,
            destination: &entry.destination,
            conflict: entry.conflict,
        })
        .collect::<Vec<_>>();

    let warnings = plan
        .warnings
        .iter()
        .map(|warning| match warning {
            ctx_traits_io::import::ImportCopyWarning::PathConflict { destination } => {
                ImportCopyPlanWarningReport::PathConflict { destination }
            }
            ctx_traits_io::import::ImportCopyWarning::UnsafeSourceLayout { path } => {
                ImportCopyPlanWarningReport::UnsafeSourceLayout { path }
            }
        })
        .collect::<Vec<_>>();

    ImportCopyPlanReport { entries, warnings }
}
