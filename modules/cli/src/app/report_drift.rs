//! Lock, import-source, projection, and export drift reports.

use ctx_traits_core::response::CommandOutput;

use crate::app::entry::build_file_evidence_from_io;
use crate::app::report_resources;

pub(crate) struct CurrentDigestEvidence {
    pub(crate) source_digest: String,
    pub(crate) canonical_digest: String,
    pub(crate) model_visible_digest: String,
    pub(crate) resource_manifest_digest: String,
}

struct PackageImportSourceLayer {
    expected: String,
    actual: Option<String>,
    summary: String,
    unsupported: bool,
    hunks: Vec<ctx_traits_core::diff::DiffHunk>,
}

impl PackageImportSourceLayer {
    fn drift_summary(&self) -> ctx_traits_core::check::DriftSummary {
        ctx_traits_core::check::DriftSummary {
            layer: ctx_traits_core::check::DriftLayer::CanonicalSource,
            expected: self.expected.clone(),
            actual: self.actual.clone(),
            summary: self.summary.clone(),
            unsupported: self.unsupported,
        }
    }

    fn diff_entry(&self) -> ctx_traits_core::diff::DiffEntry {
        let changed = !self.unsupported && self.actual.as_deref() != Some(self.expected.as_str());
        ctx_traits_core::diff::DiffEntry {
            layer: ctx_traits_core::check::DriftLayer::CanonicalSource,
            before_digest: Some(report_digest_from_text(&self.expected)),
            after_digest: self.actual.as_deref().map(report_digest_from_text),
            changed,
            summary: self.summary.clone(),
            unsupported: self.unsupported,
            hunks: self.hunks.clone(),
        }
    }
}

fn report_digest_from_text(value: &str) -> ctx_traits_core::digest::Digest {
    ctx_traits_core::digest::Digest::parse(value)
        .unwrap_or_else(|_| ctx_traits_core::digest::Digest::source(value))
}

fn refresh_decision_name(
    decision: &ctx_traits_core::import::plan::RefreshDecision,
) -> &'static str {
    match decision {
        ctx_traits_core::import::plan::RefreshDecision::NoChange => "no-change",
        ctx_traits_core::import::plan::RefreshDecision::SourceOnlyChange => "source-only-change",
        ctx_traits_core::import::plan::RefreshDecision::TraitChange => "trait-change",
        ctx_traits_core::import::plan::RefreshDecision::Blocked => "blocked",
        ctx_traits_core::import::plan::RefreshDecision::NeedsReview => "needs-review",
        ctx_traits_core::import::plan::RefreshDecision::Unsupported => "unsupported",
    }
}

fn import_source_layer_from_message(
    expected: String,
    actual: Option<String>,
    summary: String,
    unsupported: bool,
) -> PackageImportSourceLayer {
    PackageImportSourceLayer {
        expected,
        actual,
        summary,
        unsupported,
        hunks: Vec::new(),
    }
}

fn package_local_import_source_layer(
    trait_root: &camino::Utf8Path,
    trait_id: &str,
    current_package_canonical_digest: &str,
    current_canonical_json: &serde_json::Value,
) -> crate::Result<Option<PackageImportSourceLayer>> {
    let import_report_present =
        ctx_traits_io::read::read_optional_text(&trait_root.join("import-report.json"))?.is_some();

    let lock = match ctx_traits_io::import::read_trait_lock(trait_root) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            if import_report_present {
                return Ok(Some(import_source_layer_from_message(
                    "package-local trait.lock present".to_string(),
                    None,
                    "package-local import-source drift: missing trait.lock while import-report.json exists"
                        .to_string(),
                    false,
                )));
            }
            return Ok(None);
        }
        Err(e) => {
            return Ok(Some(import_source_layer_from_message(
                "readable package-local trait.lock".to_string(),
                None,
                format!("package-local import-source drift: trait.lock read error: {e}"),
                false,
            )));
        }
    };

    let Some(expected_snapshot) = lock.current_snapshot() else {
        return Ok(Some(import_source_layer_from_message(
            "current package-local import snapshot".to_string(),
            None,
            "package-local import-source drift: trait.lock has no current snapshot".to_string(),
            false,
        )));
    };

    if lock.trait_id != trait_id {
        return Ok(Some(import_source_layer_from_message(
            format!("package-local trait.lock for {trait_id}"),
            Some(lock.trait_id),
            "package-local import-source drift: trait.lock trait ID differs from decoded trait ID"
                .to_string(),
            false,
        )));
    }

    let Some(locator) = expected_snapshot.metadata.source_locator.as_deref() else {
        return Ok(Some(import_source_layer_from_message(
            expected_snapshot.snapshot_digest.to_string(),
            None,
            "package-local import-source drift: source locator missing from trait.lock metadata"
                .to_string(),
            true,
        )));
    };

    if matches!(
        ctx_traits_core::import::plan::classify_source(locator),
        ctx_traits_core::import::plan::ImportSource::Git { .. }
    ) {
        let fetch = ctx_traits_io::import::fetch_remote_source(locator);
        let evidence = fetch
            .evidence
            .as_ref()
            .map(|e| format!("kind={:?}, method={:?}", e.kind, e.fetch_method))
            .unwrap_or_else(|| "no structured remote evidence".to_string());
        return Ok(Some(import_source_layer_from_message(
            expected_snapshot.snapshot_digest.to_string(),
            None,
            format!(
                "package-local import-source drift: remote refresh unsupported/manual-download-required ({evidence})"
            ),
            true,
        )));
    }

    let source = camino::Utf8Path::new(locator);
    let (mut current_snapshot, snapshot_warnings) =
        match ctx_traits_io::import::build_artifact_snapshot_with_locator(
            source,
            &expected_snapshot.source_profile,
            locator,
        ) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return Ok(Some(import_source_layer_from_message(
                    expected_snapshot.snapshot_digest.to_string(),
                    None,
                    format!(
                        "package-local import-source drift: failed to rebuild current source snapshot from {locator}: {e}"
                    ),
                    false,
                )));
            }
        };

    let loaded_source = ctx_traits_io::import::read_agent_skill_source(source)?;
    let raw_source_digest = ctx_traits_io::import::digest_import_source(source)?;
    let import_profile =
        ctx_traits_core::import::plan::ImportProfile::parse(&expected_snapshot.source_profile);
    // Reconstructing drift against a Markdown-checklist-derived resource must
    // seed the same stable item ids `import`/`refresh` would, so prior typed
    // checklist items are pulled from the currently-checked-out canonical
    // trait before re-planning (P405).
    let prior_checklists =
        ctx_traits_core::import::plan::extract_prior_checklists(current_canonical_json);
    let mut import_plan = ctx_traits_core::import::plan::plan_agent_skills_import(
        ctx_traits_core::import::plan::AgentSkillsImportRequest {
            source: ctx_traits_core::import::plan::ImportSource::Local {
                path: locator.to_string(),
            },
            source_profile: import_profile,
            raw_source_digest: raw_source_digest.clone(),
            source_path: loaded_source.skill_path.to_string(),
            source_name: loaded_source.source_name,
            skill_markdown: loaded_source.skill_markdown,
            prior_checklists: prior_checklists.clone(),
        },
    )?;

    if source.is_dir() {
        let mf = ctx_traits_io::import::read_multi_file_skill_source(source)?;
        let (mappings, checklist_resources) =
            crate::app::import_analysis::multi_file_resource_mappings(&mf, &prior_checklists)?;
        crate::app::import_analysis::augment_draft_with_multi_file_resources(
            &mut import_plan.draft_json,
            &mappings,
            &checklist_resources,
        )?;
        current_snapshot.metadata.graph_digest = Some(mf.graph.graph_digest.clone());
        current_snapshot.metadata.resource_mappings = mappings;
    }

    let current_canonical_digest =
        ctx_traits_core::digest::canonical_digest(&import_plan.draft_json)?;
    current_snapshot.canonical_output_digest = Some(current_canonical_digest.clone());

    let refresh = ctx_traits_core::import::plan::compute_refresh_diff_full(
        trait_id,
        Some(expected_snapshot),
        &current_snapshot,
        Some(current_canonical_digest.as_str()),
        Some(current_canonical_json),
        Some(&import_plan.draft_json),
    );

    let mut hunk_text = Vec::new();
    if !refresh.artifact_diff.added.is_empty() {
        hunk_text.push(format!("added: {}", refresh.artifact_diff.added.join(", ")));
    }
    if !refresh.artifact_diff.removed.is_empty() {
        hunk_text.push(format!(
            "removed: {}",
            refresh.artifact_diff.removed.join(", ")
        ));
    }
    for modified in &refresh.artifact_diff.modified {
        hunk_text.push(format!(
            "modified: {} ({} -> {})",
            modified.path, modified.before_digest, modified.after_digest
        ));
    }
    for field in &refresh.trait_diff.field_changes {
        hunk_text.push(format!("trait-change: {field}"));
    }
    for warning in snapshot_warnings.iter().chain(refresh.warnings.iter()) {
        hunk_text.push(format!("warning: {warning}"));
    }
    let hunks = hunk_text
        .into_iter()
        .map(|text| ctx_traits_core::diff::DiffHunk {
            before_line: None,
            after_line: None,
            text,
        })
        .collect::<Vec<_>>();
    let decision = refresh_decision_name(&refresh.decision);
    let summary = format!(
        "package-local import-source drift: decision={decision}, before-snapshot={}, after-snapshot={}, before-canonical={}, after-canonical={}, added={}, removed={}, modified={}",
        refresh.before_snapshot_digest.as_deref().unwrap_or("none"),
        refresh.after_snapshot_digest,
        refresh
            .trait_diff
            .before_canonical_digest
            .as_deref()
            .unwrap_or("none"),
        refresh
            .trait_diff
            .after_canonical_digest
            .as_deref()
            .unwrap_or("none"),
        refresh.artifact_diff.added.len(),
        refresh.artifact_diff.removed.len(),
        refresh.artifact_diff.modified.len(),
    );
    let summary =
        format!("{summary}, current-package-canonical={current_package_canonical_digest}");

    Ok(Some(PackageImportSourceLayer {
        expected: expected_snapshot.snapshot_digest.to_string(),
        actual: Some(current_snapshot.snapshot_digest.to_string()),
        summary,
        unsupported: matches!(
            refresh.decision,
            ctx_traits_core::import::plan::RefreshDecision::Unsupported
        ),
        hunks,
    }))
}

pub(crate) fn locked_drift_summaries(
    locked: bool,
    repo_root: &camino::Utf8Path,
    trait_id: &str,
    variant: Option<&str>,
    current: &CurrentDigestEvidence,
    trait_root: &camino::Utf8Path,
    current_canonical_json: &serde_json::Value,
) -> crate::Result<Vec<ctx_traits_core::check::DriftSummary>> {
    if !locked {
        return Ok(Vec::new());
    }

    use ctx_traits_core::check::{DriftLayer, DriftSummary};

    let mut drift = Vec::new();
    let lock_path = ctx_traits_io::lockfile::lockfile_path(trait_root);
    let lockfile = ctx_traits_io::lockfile::read_lockfile(trait_root)?;
    let Some(lockfile) = lockfile else {
        drift.push(DriftSummary {
            layer: DriftLayer::Lock,
            expected: "package-local trait.lock present".to_string(),
            actual: None,
            summary: format!("missing lock evidence at {lock_path}"),
            unsupported: false,
        });
        if let Some(import_layer) = package_local_import_source_layer(
            trait_root,
            trait_id,
            &current.canonical_digest,
            current_canonical_json,
        )? {
            drift.push(import_layer.drift_summary());
        }
        return Ok(drift);
    };

    let Some(entry) = lockfile.trait_entry(trait_id, variant) else {
        drift.push(DriftSummary {
            layer: DriftLayer::Lock,
            expected: format!("trait entry {trait_id}"),
            actual: None,
            summary: format!("missing lock entry for trait {trait_id}"),
            unsupported: false,
        });
        if let Some(import_layer) = package_local_import_source_layer(
            trait_root,
            trait_id,
            &current.canonical_digest,
            current_canonical_json,
        )? {
            drift.push(import_layer.drift_summary());
        }
        return Ok(drift);
    };

    let (source_expected, source_actual, source_label) = match entry.source_digest() {
        Some(expected) => (
            Some(expected),
            current.source_digest.as_str(),
            "source digest",
        ),
        None => (
            entry.canonical_digest(),
            current.canonical_digest.as_str(),
            "canonical digest",
        ),
    };
    drift.push(compare_locked_digest(
        DriftLayer::CanonicalSource,
        source_expected,
        source_actual,
        source_label,
    ));
    drift.push(compare_locked_digest(
        DriftLayer::ModelView,
        entry.model_visible_digest(),
        &current.model_visible_digest,
        "model-visible digest",
    ));
    drift.push(compare_locked_digest(
        DriftLayer::ResourceManifest,
        entry.resource_manifest_digest(),
        &current.resource_manifest_digest,
        "resource manifest digest",
    ));
    drift.push(compare_locked_exports(repo_root, &entry.exports)?);
    drift.push(DriftSummary {
        layer: DriftLayer::PolicyManifest,
        expected: "policy manifest producer".to_string(),
        actual: None,
        summary: "policy manifest comparison not yet wired".to_string(),
        unsupported: true,
    });
    if let Some(import_layer) = package_local_import_source_layer(
        trait_root,
        trait_id,
        &current.canonical_digest,
        current_canonical_json,
    )? {
        drift.push(import_layer.drift_summary());
    }
    Ok(drift)
}

fn compare_locked_digest(
    layer: ctx_traits_core::check::DriftLayer,
    expected: Option<&str>,
    actual: &str,
    label: &str,
) -> ctx_traits_core::check::DriftSummary {
    match expected {
        Some(expected) => ctx_traits_core::check::DriftSummary {
            layer,
            expected: expected.to_string(),
            actual: Some(actual.to_string()),
            summary: if expected == actual {
                format!("{label} matches lock evidence")
            } else {
                format!("{label} differs from lock evidence")
            },
            unsupported: false,
        },
        None => ctx_traits_core::check::DriftSummary {
            layer,
            expected: format!("locked {label}"),
            actual: Some(actual.to_string()),
            summary: format!("missing locked {label}"),
            unsupported: false,
        },
    }
}

fn compare_locked_exports(
    repo_root: &camino::Utf8Path,
    exports: &[ctx_traits_io::lockfile::LockExportEntry],
) -> crate::Result<ctx_traits_core::check::DriftSummary> {
    use ctx_traits_core::check::{DriftLayer, DriftSummary};

    if exports.is_empty() {
        return Ok(DriftSummary {
            layer: DriftLayer::Export,
            expected: "locked export evidence".to_string(),
            actual: None,
            summary: "missing locked export evidence".to_string(),
            unsupported: false,
        });
    }

    let mut mismatches = Vec::new();
    for export in exports {
        match ctx_traits_io::lockfile::digest_locked_export(repo_root, export)? {
            Some(actual) if actual.as_str() == export.digest => {}
            Some(actual) => mismatches.push(format!(
                "{} expected {} actual {}",
                export.target,
                export.digest,
                actual.as_str()
            )),
            None => mismatches.push(format!(
                "{} missing export file {}",
                export.target, export.path
            )),
        }
    }

    Ok(DriftSummary {
        layer: DriftLayer::Export,
        expected: "all locked exports match".to_string(),
        actual: if mismatches.is_empty() {
            Some("all locked exports match".to_string())
        } else {
            Some(mismatches.join("; "))
        },
        summary: if mismatches.is_empty() {
            "export digests match lock evidence".to_string()
        } else {
            "export digest drift or missing export evidence".to_string()
        },
        unsupported: false,
    })
}

pub(crate) fn handle_diff(
    file: &str,
    from_lock: bool,
    model_view: bool,
    exports: bool,
    resources: bool,
    json: bool,
    verbose: bool,
) -> crate::Result<CommandOutput<()>> {
    let path = camino::Utf8Path::new(file);
    let (trait_ref, trait_root, source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;
    let trait_id = trait_ref.id.as_str().to_string();
    let trait_variant = trait_ref.variant.as_deref();
    let trait_root = trait_root.as_path();
    let repo_root = ctx_traits_io::export::infer_repo_root_from_trait_file(path);

    let mut entries = Vec::new();
    let include_all_optional_layers = !model_view && !exports && !resources;

    let current_canonical_json = serde_json::to_value(&trait_ref)
        .map_err(|e| crate::Error::json("serialize current canonical trait for diff", e))?;
    let roots = ctx_traits_io::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    let manifest = ctx_traits_io::resource::digest_resources(
        &roots,
        trait_ref.id.as_str(),
        &trait_ref.resources,
    )?;
    let file_evidence = build_file_evidence_from_io(&manifest);
    let lockfile = if from_lock {
        ctx_traits_io::lockfile::read_lockfile(trait_root)?
    } else {
        None
    };
    let projection_evidence =
        ctx_traits_io::lockfile::read_lockfile(trait_root)?.and_then(|document| {
            document
                .trait_entry(trait_ref.id.as_str(), trait_variant)
                .and_then(|entry| entry.projections.first().cloned())
        });
    let (mut resource_body_evidence, body_read_warnings) =
        report_resources::scan_resource_bodies(&roots, &trait_ref)?;
    let dependency_evidence = crate::app::report_check::dependency_evidence(
        repo_root,
        trait_root,
        &trait_ref,
        lockfile.as_ref(),
        from_lock,
    )?;
    resource_body_evidence.extend(dependency_evidence.body_evidence);
    let projection_model_view_digest =
        projection_model_view_digest(ProjectionModelViewDigestInput {
            projection: projection_evidence.as_ref(),
            trait_ref: &trait_ref,
            source_digest: source_digest.as_str(),
            file_evidence: &file_evidence,
            resource_body_evidence: &resource_body_evidence,
            dependency_resource_decls: &dependency_evidence.resource_decls,
            resource_manifest_digest: Some(manifest.manifest_digest.as_str()),
            manifest_warnings: &manifest.warnings,
            body_read_warnings: &body_read_warnings,
        })?;
    let mut resource_read_warnings =
        report_resources::resource_read_warning_strings(&manifest.warnings);
    resource_read_warnings.extend(report_resources::resource_read_warning_strings(
        &body_read_warnings,
    ));
    let render_plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
        &trait_ref,
        ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
        source_digest.as_str(),
        ctx_traits_core::render::ResourceEvidenceInputs {
            file_evidence: &file_evidence,
            body_evidence: &resource_body_evidence,
            dependency_resources: &dependency_evidence.resource_decls,
            manifest_digest: Some(manifest.manifest_digest.as_str()),
            read_warnings: resource_read_warnings,
        },
    );
    let lock_entry = lockfile
        .as_ref()
        .and_then(|document| document.trait_entry(&trait_id, trait_variant));
    let missing_lock_reason = if from_lock {
        let lock_path = ctx_traits_io::lockfile::lockfile_path(trait_root);
        if lockfile.is_none() {
            Some(format!("missing lockfile {lock_path}"))
        } else if lock_entry.is_none() {
            Some(format!("missing lock entry for trait {trait_id}"))
        } else {
            None
        }
    } else {
        None
    };

    let locked_source_digest = lock_entry.and_then(|entry| entry.source_digest());
    let locked_canonical_digest = lock_entry.and_then(|entry| entry.canonical_digest());
    let (source_expected, source_current, source_label) = if locked_source_digest.is_some() {
        (
            locked_source_digest,
            source_digest.as_str(),
            "source digest",
        )
    } else {
        (
            locked_canonical_digest,
            canonical_digest.as_str(),
            "canonical digest",
        )
    };
    entries.push(diff_digest_with_optional_lock(
        ctx_traits_core::check::DriftLayer::CanonicalSource,
        from_lock,
        source_expected,
        source_current,
        missing_lock_reason.as_deref(),
        source_label,
    ));

    if model_view || include_all_optional_layers {
        entries.push(diff_digest_with_optional_lock(
            ctx_traits_core::check::DriftLayer::ModelView,
            from_lock,
            lock_entry.and_then(|entry| entry.model_visible_digest()),
            render_plan.model_view.content_digest.as_str(),
            missing_lock_reason.as_deref(),
            "model-visible digest",
        ));
        entries.push(projection_lock_diff(
            projection_evidence.as_ref(),
            source_digest.as_str(),
            canonical_digest.as_str(),
            &projection_model_view_digest,
        )?);
    }

    if exports || include_all_optional_layers {
        let export_entries = lock_entry.map_or(&[][..], |entry| entry.exports.as_slice());
        entries.push(export_diff_with_optional_lock(
            repo_root,
            from_lock,
            export_entries,
            missing_lock_reason.as_deref(),
        )?);
    }

    if resources || include_all_optional_layers {
        entries.push(diff_digest_with_optional_lock(
            ctx_traits_core::check::DriftLayer::ResourceManifest,
            from_lock,
            lock_entry.and_then(|entry| entry.resource_manifest_digest()),
            manifest.manifest_digest.as_str(),
            missing_lock_reason.as_deref(),
            "resource manifest digest",
        ));
        entries.push(ctx_traits_core::diff::unsupported_layer(
            ctx_traits_core::check::DriftLayer::PolicyManifest,
            "policy manifest comparison not yet wired",
        ));
    }

    if from_lock {
        entries.push(eval_result_diff_with_optional_lock(
            lock_entry,
            missing_lock_reason.as_deref(),
            source_digest.as_str(),
        )?);
        entries.push(lock_diff_with_optional_entry(
            lock_entry,
            missing_lock_reason.as_deref(),
        )?);
    }

    if from_lock
        && let Some(import_layer) = package_local_import_source_layer(
            trait_root,
            &trait_id,
            canonical_digest.as_str(),
            &current_canonical_json,
        )?
    {
        entries.push(import_layer.diff_entry());
    }

    let report = ctx_traits_core::diff::DiffReport { trait_id, entries };

    use crate::app::presentation::{
        HumanOutputMode, OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
    };

    match OutputMode::select(json, verbose) {
        OutputMode::Json => {
            crate::app::command_handlers::print_json_report(&report, "diff report")?;
        }
        OutputMode::Human(mode) => {
            let changed = report.entries.iter().filter(|e| e.changed).count();
            let mut panel = Panel::new(
                "ctx",
                "diff",
                PanelStatus::Passed(if changed == 0 {
                    "no change".to_string()
                } else {
                    "changed".to_string()
                }),
            )
            .row(PanelRow::toned("trait", &report.trait_id, RowTone::Default));
            for entry in &report.entries {
                let status = diff_entry_status(entry);
                panel = panel.row(PanelRow::toned(
                    layer_label(&entry.layer),
                    status,
                    if entry.changed {
                        RowTone::Fail
                    } else {
                        RowTone::Pass
                    },
                ));
            }
            if mode == HumanOutputMode::Compact {
                panel = panel.next(PanelRow::toned(
                    "next",
                    "ctx traits diff --verbose for full hunk detail",
                    RowTone::Default,
                ));
            }
            emit_human(false, &panel, mode, || {
                crate::app::tui::emit_report(
                    false,
                    || styled_diff_lines(&report),
                    || emit_plain_diff_report(&report),
                )
            })?;
        }
    }

    Ok(CommandOutput::new(()))
}

fn diff_entry_status(entry: &ctx_traits_core::diff::DiffEntry) -> &'static str {
    if entry.unsupported {
        "unsupported"
    } else if entry.changed {
        "changed"
    } else {
        "no change"
    }
}

fn emit_plain_diff_report(report: &ctx_traits_core::diff::DiffReport) -> crate::Result<()> {
    use crate::app::tui::write_plain_line as w;
    w("ctx traits diff")?;
    w(format!("  trait: {}", report.trait_id))?;
    for entry in &report.entries {
        let status = diff_entry_status(entry);
        w(format!("  {:>16}: {status}", layer_label(&entry.layer)))?;
        w(format!("    summary: {}", entry.summary))?;
        w(format!(
            "    before-digest: {}",
            entry.before_digest.as_deref().unwrap_or("none")
        ))?;
        w(format!(
            "    after-digest: {}",
            entry.after_digest.as_deref().unwrap_or("none")
        ))?;
        w(format!("    unsupported: {}", entry.unsupported))?;
        if !entry.hunks.is_empty() {
            for hunk in &entry.hunks {
                w(format!("    {hunk}", hunk = hunk.text))?;
            }
        }
    }
    Ok(())
}

fn styled_diff_lines(report: &ctx_traits_core::diff::DiffReport) -> Vec<crate::app::tui::Line> {
    use crate::app::tui::{Line, Tone};

    let mut lines = Vec::new();
    lines.push(crate::app::tui::command_line("ctx traits diff"));
    lines.push(Line::blank());
    let mut trait_line = Line::blank();
    trait_line.push("  trait: ", Tone::Muted);
    trait_line.push(report.trait_id.clone(), Tone::Default);
    lines.push(trait_line);
    for entry in &report.entries {
        let status = diff_entry_status(entry);
        let status_tone = if entry.unsupported {
            Tone::Warn
        } else if entry.changed {
            Tone::Fail
        } else {
            Tone::Pass
        };
        let mut header = Line::blank();
        header.push(
            format!("  {:>16}: ", layer_label(&entry.layer)),
            Tone::Muted,
        );
        header.push(status, status_tone);
        lines.push(header);

        let mut summary = Line::blank();
        summary.push("    summary: ", Tone::Muted);
        summary.push(entry.summary.clone(), Tone::Default);
        lines.push(summary);

        let mut before = Line::blank();
        before.push("    before-digest: ", Tone::Muted);
        before.push(
            entry.before_digest.as_deref().unwrap_or("none"),
            Tone::Default,
        );
        lines.push(before);

        let mut after = Line::blank();
        after.push("    after-digest: ", Tone::Muted);
        after.push(
            entry.after_digest.as_deref().unwrap_or("none"),
            Tone::Default,
        );
        lines.push(after);

        let mut unsupported = Line::blank();
        unsupported.push("    unsupported: ", Tone::Muted);
        unsupported.push(entry.unsupported.to_string(), Tone::Default);
        lines.push(unsupported);

        for hunk in &entry.hunks {
            let mut hunk_line = Line::blank();
            hunk_line.push("    ", Tone::Muted);
            hunk_line.push(hunk.text.clone(), Tone::Default);
            lines.push(hunk_line);
        }
    }
    lines
}

pub(crate) fn load_trait_files(paths: &[String]) -> crate::Result<Vec<ctx_traits_core::Trait>> {
    let mut traits = Vec::new();
    for path_str in paths {
        let (trait_ref, _trait_root, _source_digest, _canonical_digest) =
            ctx_traits_io::run::load_trait(path_str)?;
        traits.push(trait_ref);
    }
    Ok(traits)
}

fn diff_digest_with_optional_lock(
    layer: ctx_traits_core::check::DriftLayer,
    from_lock: bool,
    expected: Option<&str>,
    current: &str,
    missing_lock_reason: Option<&str>,
    label: &str,
) -> ctx_traits_core::diff::DiffEntry {
    if !from_lock {
        return ctx_traits_core::diff::missing_baseline(
            layer,
            Some(current),
            "no baseline supplied; pass --from-lock to compare lock evidence",
        );
    }

    if let Some(reason) = missing_lock_reason {
        return ctx_traits_core::diff::missing_baseline(layer, Some(current), reason);
    }

    match expected {
        Some(expected) => ctx_traits_core::diff::digest_diff(layer, Some(expected), Some(current)),
        None => ctx_traits_core::diff::missing_baseline(
            layer,
            Some(current),
            &format!("lock entry lacks {label}"),
        ),
    }
}

fn projection_lock_diff(
    lock: Option<&ctx_traits_core::lockfile::LockProjection>,
    source_digest: &str,
    canonical_digest: &str,
    model_view_digest: &str,
) -> crate::Result<ctx_traits_core::diff::DiffEntry> {
    let status = ctx_traits_core::lockfile::projection_drift_status(
        lock,
        source_digest,
        canonical_digest,
        model_view_digest,
    );
    let before = lock.map(|lock| lock.digests.model_visible.as_str());
    let mut entry = match status {
        ctx_traits_core::lockfile::ProjectionDriftStatus::MissingLock => {
            ctx_traits_core::diff::missing_baseline(
                ctx_traits_core::check::DriftLayer::ProjectionLock,
                Some(model_view_digest),
                "missing projection evidence",
            )
        }
        _ => ctx_traits_core::diff::digest_diff(
            ctx_traits_core::check::DriftLayer::ProjectionLock,
            before,
            Some(model_view_digest),
        ),
    };
    entry.summary = projection_lock_summary(status);
    Ok(entry)
}

pub(crate) struct ProjectionModelViewDigestInput<'a> {
    pub(crate) projection: Option<&'a ctx_traits_core::lockfile::LockProjection>,
    pub(crate) trait_ref: &'a ctx_traits_core::Trait,
    pub(crate) source_digest: &'a str,
    pub(crate) file_evidence: &'a [ctx_traits_core::resource_plan::FileEvidence],
    pub(crate) resource_body_evidence: &'a [ctx_traits_core::resource_plan::BodyEvidence],
    pub(crate) dependency_resource_decls:
        &'a [ctx_traits_core::resource_plan::DependencyResourceDecl],
    pub(crate) resource_manifest_digest: Option<&'a str>,
    pub(crate) manifest_warnings: &'a [ctx_traits_io::resource::ResourceReadWarning],
    pub(crate) body_read_warnings: &'a [ctx_traits_io::resource::ResourceReadWarning],
}

pub(crate) fn projection_model_view_digest(
    input: ProjectionModelViewDigestInput<'_>,
) -> crate::Result<String> {
    let profile = match input.projection {
        Some(projection) => {
            ctx_traits_core::render::ExtendedRenderProfile::parse(&projection.target_profile)
                .ok_or_else(|| crate::Error::Command {
                    message: format!(
                        "trait.lock projection has unsupported target-profile {:?}",
                        projection.target_profile
                    ),
                })?
        }
        None => ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
    };
    let mut resource_read_warnings =
        report_resources::resource_read_warning_strings(input.manifest_warnings);
    resource_read_warnings.extend(report_resources::resource_read_warning_strings(
        input.body_read_warnings,
    ));
    let plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
        input.trait_ref,
        profile,
        input.source_digest,
        ctx_traits_core::render::ResourceEvidenceInputs {
            file_evidence: input.file_evidence,
            body_evidence: input.resource_body_evidence,
            dependency_resources: input.dependency_resource_decls,
            manifest_digest: input.resource_manifest_digest,
            read_warnings: resource_read_warnings,
        },
    );
    Ok(plan.model_view.content_digest.to_string())
}

pub(crate) fn projection_lock_summary(
    status: ctx_traits_core::lockfile::ProjectionDriftStatus,
) -> String {
    match status {
        ctx_traits_core::lockfile::ProjectionDriftStatus::InSync => {
            "projection evidence in sync with current source/model view".to_string()
        }
        ctx_traits_core::lockfile::ProjectionDriftStatus::SourceDrifted => {
            "projection source/canonical/model-visible digest drifted".to_string()
        }
        ctx_traits_core::lockfile::ProjectionDriftStatus::MissingLock => {
            "missing projection evidence in trait.lock".to_string()
        }
        ctx_traits_core::lockfile::ProjectionDriftStatus::OutputNotWritten => {
            "projection has no static output path recorded".to_string()
        }
    }
}

fn export_diff_with_optional_lock(
    repo_root: &camino::Utf8Path,
    from_lock: bool,
    exports: &[ctx_traits_io::lockfile::LockExportEntry],
    missing_lock_reason: Option<&str>,
) -> crate::Result<ctx_traits_core::diff::DiffEntry> {
    use ctx_traits_core::check::DriftLayer;

    if !from_lock {
        return Ok(ctx_traits_core::diff::unsupported_layer(
            DriftLayer::Export,
            "pass --from-lock to compare generated export evidence",
        ));
    }

    if let Some(reason) = missing_lock_reason {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Export,
            None,
            reason,
        ));
    }

    if exports.is_empty() {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Export,
            None,
            "lock entry lacks export evidence",
        ));
    }

    let mut before_lines = Vec::new();
    let mut after_lines = Vec::new();
    let mut hunks = Vec::new();

    for export in exports {
        before_lines.push(format!(
            "{}:{}:{}",
            export.target, export.path, export.digest
        ));
        match ctx_traits_io::lockfile::digest_locked_export(repo_root, export)? {
            Some(actual) => {
                after_lines.push(format!(
                    "{}:{}:{}",
                    export.target,
                    export.path,
                    actual.as_str()
                ));
                if actual.as_str() != export.digest {
                    hunks.push(ctx_traits_core::diff::DiffHunk {
                        before_line: None,
                        after_line: None,
                        text: format!(
                            "{}: locked {} current {}",
                            export.target,
                            export.digest,
                            actual.as_str()
                        ),
                    });
                }
            }
            None => {
                after_lines.push(format!("{}:{}:missing", export.target, export.path));
                hunks.push(ctx_traits_core::diff::DiffHunk {
                    before_line: None,
                    after_line: None,
                    text: format!(
                        "{}: current export missing or unreadable at {}",
                        export.target, export.path
                    ),
                });
            }
        }
    }

    let before_text = before_lines.join("\n");
    let after_text = after_lines.join("\n");
    let before_digest = ctx_traits_core::digest::Digest::from_bytes(before_text.as_bytes());
    let after_digest = ctx_traits_core::digest::Digest::from_bytes(after_text.as_bytes());
    let mut entry = ctx_traits_core::diff::digest_diff(
        DriftLayer::Export,
        Some(before_digest.as_str()),
        Some(after_digest.as_str()),
    );
    entry.summary = if hunks.is_empty() {
        format!("export: {} locked export digest(s) match", exports.len())
    } else {
        format!(
            "export: {} locked export digest(s) differ or are missing",
            hunks.len()
        )
    };
    entry.hunks = hunks;
    Ok(entry)
}

fn eval_result_diff_with_optional_lock(
    entry: Option<&ctx_traits_io::lockfile::LockTraitEntry>,
    missing_lock_reason: Option<&str>,
    current_source_digest: &str,
) -> crate::Result<ctx_traits_core::diff::DiffEntry> {
    use ctx_traits_core::check::DriftLayer;

    if let Some(reason) = missing_lock_reason {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Lock,
            Some(current_source_digest),
            reason,
        ));
    }

    let Some(entry) = entry else {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Lock,
            Some(current_source_digest),
            "lock entry missing for eval-result evidence",
        ));
    };
    if entry.eval_results.is_empty() {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Lock,
            Some(current_source_digest),
            "lock entry lacks eval-result evidence",
        ));
    }
    let text = serde_json::to_string(&entry.eval_results)
        .map_err(|e| crate::Error::json("serialize eval-result lock evidence", e))?;
    let before = ctx_traits_core::digest::Digest::source(&text);
    let stale = entry
        .eval_results
        .iter()
        .any(|result| result.input_digest.as_str() != current_source_digest);
    Ok(ctx_traits_core::diff::DiffEntry {
        layer: DriftLayer::Lock,
        before_digest: Some(before),
        after_digest: Some(ctx_traits_core::digest::Digest::parse(
            current_source_digest,
        )?),
        changed: stale,
        summary: if stale {
            "eval-result lock evidence contains stale input digest(s)".to_string()
        } else {
            "eval-result lock evidence input digests match current source".to_string()
        },
        unsupported: false,
        hunks: Vec::new(),
    })
}

fn lock_diff_with_optional_entry(
    entry: Option<&ctx_traits_io::lockfile::LockTraitEntry>,
    missing_lock_reason: Option<&str>,
) -> crate::Result<ctx_traits_core::diff::DiffEntry> {
    use ctx_traits_core::check::DriftLayer;

    if let Some(reason) = missing_lock_reason {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Lock,
            None,
            reason,
        ));
    }

    let Some(entry) = entry else {
        return Ok(ctx_traits_core::diff::missing_baseline(
            DriftLayer::Lock,
            None,
            "lock entry missing",
        ));
    };
    let text = serde_json::to_string(entry)
        .map_err(|e| crate::Error::json("serialize lock entry evidence", e))?;
    let digest = ctx_traits_core::digest::Digest::from_bytes(text.as_bytes());
    Ok(ctx_traits_core::diff::DiffEntry {
        layer: DriftLayer::Lock,
        before_digest: Some(digest),
        after_digest: None,
        changed: false,
        summary: "lock: lock entry loaded; no independent current lock producer to compare"
            .to_string(),
        unsupported: true,
        hunks: Vec::new(),
    })
}

pub(crate) fn layer_label(layer: &ctx_traits_core::check::DriftLayer) -> &str {
    use ctx_traits_core::check::DriftLayer;
    match layer {
        DriftLayer::CanonicalSource => "canonical-source",
        DriftLayer::ModelView => "model-view",
        DriftLayer::ResourceManifest => "resource-manifest",
        DriftLayer::PolicyManifest => "policy-manifest",
        DriftLayer::Export => "export",
        DriftLayer::Lock => "lock",
        DriftLayer::ProjectionLock => "projection-lock",
        DriftLayer::Dependency => "dependency",
    }
}
