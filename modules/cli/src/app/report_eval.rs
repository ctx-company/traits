//! Evaluation evidence for check reports.

pub(crate) fn read_eval_reports(
    paths: &[String],
) -> crate::Result<Vec<ctx_traits_core::eval_run::Report>> {
    let mut reports = Vec::new();
    for path in paths {
        let text = ctx_traits_io::read::read_text(camino::Utf8Path::new(path))?;
        let report = serde_json::from_str(&text)
            .map_err(|e| crate::Error::json(format!("parse eval report JSON {path}"), e))?;
        reports.push(report);
    }
    Ok(reports)
}

pub(crate) fn eval_evidence_section(
    trait_ref: &ctx_traits_core::Trait,
    source_digest: &str,
    lock_entry: Option<&ctx_traits_io::lockfile::LockTraitEntry>,
    reports: &[ctx_traits_core::eval_run::Report],
    evidence_required: bool,
) -> ctx_traits_core::check::CheckSection {
    let mut passing_results: Vec<ctx_traits_core::r#trait::EvalResult> = lock_entry
        .map(|entry| entry.eval_results.clone())
        .unwrap_or_default();
    for report in reports {
        passing_results.extend(report.passing_results());
    }
    passing_results.sort();
    let mut missing = 0usize;
    let mut stale = 0usize;
    let mut unsupported = 0usize;
    for eval in &trait_ref.evals {
        if !eval.variant.is_mvp_supported() {
            unsupported += 1;
            continue;
        }
        let results: Vec<&ctx_traits_core::r#trait::EvalResult> = passing_results
            .iter()
            .filter(|result| result.eval_id == eval.id)
            .collect();
        if results.is_empty() {
            missing += 1;
        } else if !results
            .iter()
            .any(|result| result.input_digest.as_str() == source_digest)
        {
            stale += 1;
        }
    }
    ctx_traits_core::check::CheckSection {
        name: "eval-evidence".to_string(),
        summary: format!(
            "declared={}, passing-results={}, missing={}, stale={}, unsupported={}",
            trait_ref.evals.len(),
            passing_results.len(),
            missing,
            stale,
            unsupported,
        ),
        ok: !evidence_required || (missing == 0 && stale == 0),
    }
}
