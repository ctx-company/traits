//! Imported package evidence for check reports.

pub(crate) fn import_evidence_section(
    trait_root: &camino::Utf8Path,
) -> crate::Result<ctx_traits_core::check::CheckSection> {
    let report_path = trait_root.join("import-report.json");

    let (lock_evidence, lock_ok) = match ctx_traits_io::import::read_trait_lock(trait_root) {
        Ok(Some(lock)) => {
            let snapshot_count = lock.snapshots.len();
            let current = lock
                .current_snapshot()
                .map(|s| {
                    format!(
                        "snapshot={}, artifacts={}, canonical-digest={}",
                        s.snapshot_digest,
                        s.artifacts.len(),
                        s.canonical_output_digest.as_deref().unwrap_or("none"),
                    )
                })
                .unwrap_or_else(|| "no current snapshot".to_string());
            (
                format!(
                    "trait.lock present ({} snapshots, {})",
                    snapshot_count, current
                ),
                true,
            )
        }
        Ok(None) => (
            "no import snapshot in package-local trait.lock".to_string(),
            true,
        ),
        Err(e) => (format!("trait.lock read error: {e}"), false),
    };

    let Some(report_text) = ctx_traits_io::read::read_optional_text(&report_path)? else {
        return Ok(ctx_traits_core::check::CheckSection {
            name: "import-evidence".to_string(),
            summary: format!("no sibling import-report.json; {lock_evidence}"),
            ok: lock_ok,
        });
    };
    let report: ctx_traits_core::import::plan::ImportReport = serde_json::from_str(&report_text)
        .map_err(|e| crate::Error::json(format!("parse import report {report_path}"), e))?;
    let report_digest = ctx_traits_core::digest::Digest::source(&report_text);
    let raw_path = trait_root.join("imported");
    let raw_present =
        ctx_traits_io::read::read_optional_text(&raw_path.join("SKILL.md"))?.is_some();

    Ok(ctx_traits_core::check::CheckSection {
        name: "import-evidence".to_string(),
        summary: format!(
            "profile={}, raw-source-digest={}, report-digest={}, unsupported-fields={}, warnings={}, review-actions={}, raw-preserved={}, default-lifecycle-status={}, lock={}",
            report.source_profile.as_str(),
            report.raw_source_digest,
            report_digest.as_str(),
            report.unsupported_fields.len(),
            report.warnings.len() + report.conversion_warnings.len(),
            report.review_actions.len(),
            raw_present,
            report.default_lifecycle.status,
            lock_evidence,
        ),
        ok: lock_ok
            && !report.hidden_content_findings.iter().any(|finding| {
                !matches!(finding.severity, ctx_traits_core::audit::Severity::Advisory)
            }),
    })
}
