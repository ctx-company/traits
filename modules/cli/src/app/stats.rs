//! `ctx traits internal stats` (P442): deterministic, read-only aggregation over this
//! repository's run session ledgers. Loads the inventory and resolves the
//! `--since` cutoff at this IO boundary, then hands everything to the pure
//! `ctx_traits_core::procedure::stats` aggregator.

use ctx_traits_core::procedure::stats::{self, OutcomeValueCount, RunRecord, StatsReport};
use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::run_session::{InventoryOutcome, current_repo_run_inventory};

use crate::app::command_handlers::print_json_report;
use crate::app::tui::write_plain_line as w;

pub(crate) fn handle_stats(
    since: Option<u64>,
    trait_id: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let inventory = current_repo_run_inventory()?;
    let total_runs = inventory.len() as u64;
    let mut unreadable_runs = 0u64;
    let mut records = Vec::with_capacity(inventory.len());
    for row in inventory {
        match row.status {
            InventoryOutcome::Readable { session, .. } => {
                records.push(RunRecord::from_session(&session));
            }
            InventoryOutcome::Unreadable { .. } => unreadable_runs += 1,
        }
    }

    let report = stats::aggregate(&records, total_runs, unreadable_runs, since, trait_id);

    if json {
        print_json_report(&report, "stats report")?;
        return Ok(CommandOutput::new(()));
    }

    // 0130: pricing/billing are config, not code — resolved here at the IO
    // boundary (same as every other run-config knob) and handed to the pure
    // printer as plain data, never read by `ctx_traits_core::procedure::stats`.
    let profile = ctx_traits_io::harness_config::resolve_runtime_assignments(&[]).ok();
    let billing = profile
        .as_ref()
        .map(crate::app::drive::current_model_billing_map)
        .unwrap_or_default();
    let pricing = profile.map(|p| p.pricing).unwrap_or_default();
    emit_plain_stats(&report, &pricing, &billing)?;
    Ok(CommandOutput::new(()))
}

fn opt_epoch(value: Option<u64>) -> String {
    value.map_or("none".to_string(), |v| v.to_string())
}

fn opt_str(value: Option<&str>) -> String {
    value.map_or("none".to_string(), str::to_string)
}

fn emit_plain_stats(
    report: &StatsReport,
    pricing: &std::collections::BTreeMap<String, ctx_traits_io::harness_config::ModelPricing>,
    billing: &std::collections::BTreeMap<String, ctx_traits_io::harness_config::BillingMode>,
) -> crate::Result<()> {
    w("ctx traits internal stats")?;
    w(format!("  schema-version: {}", report.schema_version))?;
    w(format!(
        "  applied-since-epoch: {}",
        opt_epoch(report.applied_since_epoch)
    ))?;
    w(format!(
        "  applied-trait-id: {}",
        opt_str(report.applied_trait_id.as_deref())
    ))?;
    w(format!("  total-runs: {}", report.total_runs))?;
    w(format!("  unreadable-runs: {}", report.unreadable_runs))?;
    w(format!(
        "  trait-matched-runs: {}",
        report.trait_matched_runs
    ))?;
    w(format!(
        "  timestamp-missing-runs: {}",
        report.timestamp_missing_runs
    ))?;
    w(format!("  matched-runs: {}", report.matched_runs))?;
    w("  outcomes:")?;
    w(format!("    completed: {}", report.outcomes.completed))?;
    w(format!("    blocked: {}", report.outcomes.blocked))?;
    w(format!(
        "    exhausted-unapproved: {}",
        report.outcomes.exhausted_unapproved
    ))?;
    w(format!("    killed: {}", report.outcomes.killed))?;
    w(format!(
        "    no-outcome-recorded: {}",
        report.outcomes.no_outcome_recorded
    ))?;
    w(format!("    other: {}", report.outcomes.other))?;
    for OutcomeValueCount { value, runs } in &report.outcomes.other_values {
        w(format!("      {value}: {runs}"))?;
    }
    w(
        "  refinement-rounds-to-approval (completed runs only, name-pattern heuristic over accepted `-verdict` slots):",
    )?;
    w(format!(
        "    average-rounds: {}",
        report
            .refinement_rounds
            .average_rounds
            .map_or("none".to_string(), |value| format!("{value:.2}"))
    ))?;
    w(format!(
        "    completed-runs-observed: {}",
        report.refinement_rounds.completed_runs_observed
    ))?;
    w(format!(
        "    completed-runs-missing: {}",
        report.refinement_rounds.completed_runs_missing
    ))?;
    w("  token-evidence (latest-drive evidence, not cumulative):")?;
    w(format!(
        "    work-tokens-total: {} (observed {} run(s), missing {} run(s))",
        report.token_evidence.work_tokens_total,
        report.token_evidence.work_tokens_observed_runs,
        report.token_evidence.work_tokens_missing_runs
    ))?;
    w(format!(
        "    narrator-tokens-total: {} (observed {} run(s), missing {} run(s))",
        report.token_evidence.narrator_tokens_total,
        report.token_evidence.narrator_tokens_observed_runs,
        report.token_evidence.narrator_tokens_missing_runs
    ))?;
    w(format!(
        "    guide-tokens-total: {} (observed {} run(s), missing {} run(s))",
        report.token_evidence.guide_tokens_total,
        report.token_evidence.guide_tokens_observed_runs,
        report.token_evidence.guide_tokens_missing_runs
    ))?;
    if report.tokens_by_model.is_empty() {
        w("  tokens-by-model: (none)")?;
    } else {
        w(
            "  tokens-by-model (0130; estimated $ by billing mode, resolved from the current config — subscription models always $0):",
        )?;
        for (model, tokens) in &report.tokens_by_model {
            let cost = crate::app::drive::model_cost_label(pricing, billing, model, *tokens);
            w(format!("    {model}: {tokens} tokens ({cost})"))?;
        }
    }
    if report.traits.is_empty() {
        w("  traits: (none)")?;
        return Ok(());
    }
    w("  traits:")?;
    for trait_row in &report.traits {
        w(format!(
            "    - {}: {} run(s)",
            trait_row.trait_id, trait_row.runs
        ))?;
        w(format!(
            "        work-tokens-total: {} (observed {} run(s), missing {} run(s))",
            trait_row.token_evidence.work_tokens_total,
            trait_row.token_evidence.work_tokens_observed_runs,
            trait_row.token_evidence.work_tokens_missing_runs
        ))?;
        w(format!(
            "        narrator-tokens-total: {} (observed {} run(s), missing {} run(s))",
            trait_row.token_evidence.narrator_tokens_total,
            trait_row.token_evidence.narrator_tokens_observed_runs,
            trait_row.token_evidence.narrator_tokens_missing_runs
        ))?;
        w(format!(
            "        guide-tokens-total: {} (observed {} run(s), missing {} run(s))",
            trait_row.token_evidence.guide_tokens_total,
            trait_row.token_evidence.guide_tokens_observed_runs,
            trait_row.token_evidence.guide_tokens_missing_runs
        ))?;
        for digest_row in &trait_row.digests {
            w(format!(
                "        {}: {} run(s)",
                digest_row.digest.as_deref().unwrap_or("(missing-digest)"),
                digest_row.runs
            ))?;
        }
    }
    Ok(())
}
