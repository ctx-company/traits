//! Pure aggregation over already-loaded run-session ledgers (P442).
//!
//! `ctx traits stats` is strictly read-only: this module never touches a
//! filesystem, clock, or process. Callers (the CLI/IO boundary) load the
//! run inventory and resolve the `--since` cutoff, then hand every readable
//! [`Session`](crate::procedure::session::Session) plus each unreadable
//! ledger's count in here for deterministic counting.
//!
//! Outcome classification prefers the session's typed [`Status`] and typed
//! [`DriveOutcomeKind`] over the free-form `last-drive-outcome.outcome`
//! string wherever a typed value is unambiguous: `Status::Completed` always
//! counts as `completed`, and the two unassignment/permission blocked
//! variants (`BlockedAgentUnassigned`, `BlockedCommandPermissionRequired`)
//! always count as `blocked`, regardless of what (if anything) the latest
//! drive outcome recorded. `Status::Blocked` whose recorded [`StopReason`]
//! is the loop runtime's own `max-iterations-exhausted` token counts as
//! `exhausted-unapproved`: a bounded loop that spent its iteration budget
//! without matching `until` and is still terminally blocked is exhaustion
//! that was never approved, distinct from a hard block. A loop that
//! exhausts under `on-exhausted = "continue"` and later completes reads as
//! `Status::Completed`, not this bucket — only a terminal blocked exhaustion
//! counts. Every other non-completed, non-blocked status checks the typed
//! `DriveOutcomeKind::Killed` marker (P551's live-TUI kill) and counts it as
//! `killed` before falling back to the raw `outcome` string: `None` is
//! `no-outcome-recorded`, `"completed"`/`"blocked"` are their buckets, and
//! anything else — including `max-frames-exhausted`,
//! `total-budget-exhausted`, or other free-form values the ledger schema
//! does not type into one of the above — is preserved verbatim in the
//! sorted `other` breakdown rather than coerced into an invented bucket.
//! Token totals sum only the evidence present in the latest recorded
//! `token_usage` per run; they are latest-drive evidence, not cumulative
//! lifetime usage. Average refinement rounds to approval is derived, not
//! separately persisted telemetry: for each completed run it is the highest
//! per-slot revision count among accepted slots whose reference ends in
//! `-verdict` (the P449 `guardedProduction` kit's one-revision-per-round
//! convention) — a name-pattern heuristic, not a typed round counter, so a
//! completed run with no such slot reads as missing coverage rather than
//! zero rounds.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::procedure::runtime::{SlotRevision, StopReason};
use crate::procedure::session::{DriveOutcomeKind, Session, Status};

/// The loop runtime's own stop-reason token for iteration-budget exhaustion
/// (`modules/core/src/procedure/runtime/state.rs`'s
/// `STOP_MAX_ITERATIONS_EXHAUSTED`, not re-exported so duplicated verbatim
/// here rather than widening that module's visibility for one string).
const STOP_MAX_ITERATIONS_EXHAUSTED: &str = "max-iterations-exhausted";

/// Suffix identifying an accepted slot as a review-verdict slot for the
/// refinement-rounds heuristic (see module doc).
const VERDICT_SLOT_SUFFIX: &str = "-verdict";

pub const STATS_SCHEMA_VERSION: &str = "2";

/// One readable ledger's evidence, extracted by the IO boundary from a
/// loaded [`Session`] before calling [`aggregate`]. Kept minimal and owned
/// so this module never depends on IO types (e.g. `InventoryOutcome`).
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub trait_id: String,
    pub canonical_digest: Option<Digest>,
    pub status: Status,
    pub stop_reason: Option<StopReason>,
    pub drive_outcome_kind: Option<DriveOutcomeKind>,
    pub outcome: Option<String>,
    pub recorded_at_epoch: Option<u64>,
    pub work_tokens: Option<u64>,
    pub narrator_tokens: Option<u64>,
    pub guide_tokens: Option<u64>,
    /// Highest per-slot revision count among accepted `-verdict`-suffixed
    /// slots, or `None` when no such slot has a recorded revision. See the
    /// module doc's refinement-rounds heuristic.
    pub verdict_rounds: Option<u64>,
}

impl RunRecord {
    pub fn from_session(session: &Session) -> Self {
        let last_drive_outcome = session.last_drive_outcome.as_ref();
        let token_usage = last_drive_outcome.and_then(|outcome| outcome.token_usage.as_ref());
        Self {
            trait_id: session.trait_id.clone(),
            canonical_digest: session.canonical_digest.clone(),
            status: session.status.clone(),
            stop_reason: session.stop_reason.clone(),
            drive_outcome_kind: last_drive_outcome.map(|outcome| outcome.outcome.clone()),
            outcome: last_drive_outcome.map(|outcome| outcome.outcome.as_str().to_string()),
            recorded_at_epoch: last_drive_outcome.map(|outcome| outcome.recorded_at_epoch),
            work_tokens: token_usage.and_then(|usage| usage.work_tokens),
            narrator_tokens: token_usage.and_then(|usage| usage.narrator_tokens),
            guide_tokens: token_usage.and_then(|usage| usage.guide_tokens),
            verdict_rounds: verdict_slot_rounds(&session.slot_revisions),
        }
    }
}

/// Highest per-slot revision count among `slot_revisions` whose slot
/// reference ends in [`VERDICT_SLOT_SUFFIX`], across every loop that wrote
/// one. `None` when no revision matches (missing coverage, not zero
/// rounds).
fn verdict_slot_rounds(slot_revisions: &[SlotRevision]) -> Option<u64> {
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for revision in slot_revisions {
        let slot_text = revision.slot_ref.as_str();
        if slot_text.ends_with(VERDICT_SLOT_SUFFIX) {
            *counts.entry(slot_text).or_insert(0) += 1;
        }
    }
    counts.into_values().max()
}

/// Which outcome bucket a record belongs in, per the precedence documented
/// on this module: the typed [`Status`] wins when it unambiguously means
/// completed, blocked, or exhausted-unapproved; then the typed
/// [`DriveOutcomeKind::Killed`] marker; otherwise the raw `outcome` string
/// decides.
enum OutcomeBucket {
    Completed,
    Blocked,
    ExhaustedUnapproved,
    Killed,
    NoOutcomeRecorded,
    Other(String),
}

fn classify_outcome(record: &RunRecord) -> OutcomeBucket {
    let is_exhausted_block = record.status == Status::Blocked
        && record
            .stop_reason
            .as_ref()
            .is_some_and(|stop_reason| stop_reason.reason == STOP_MAX_ITERATIONS_EXHAUSTED);
    match record.status {
        Status::Completed => OutcomeBucket::Completed,
        Status::BlockedAgentUnassigned | Status::BlockedCommandPermissionRequired => {
            OutcomeBucket::Blocked
        }
        Status::Blocked if is_exhausted_block => OutcomeBucket::ExhaustedUnapproved,
        Status::Blocked => OutcomeBucket::Blocked,
        _ if matches!(record.drive_outcome_kind, Some(DriveOutcomeKind::Killed)) => {
            OutcomeBucket::Killed
        }
        _ => match record.outcome.as_deref() {
            None => OutcomeBucket::NoOutcomeRecorded,
            Some("completed") => OutcomeBucket::Completed,
            Some("blocked") => OutcomeBucket::Blocked,
            Some(other) => OutcomeBucket::Other(other.to_string()),
        },
    }
}

/// Sum work/narrator token evidence and observed/missing coverage across
/// `records`, shared verbatim by both the report-wide and per-trait
/// aggregation paths so token semantics are defined in exactly one place.
fn accumulate_token_evidence<'a>(
    records: impl IntoIterator<Item = &'a RunRecord>,
) -> TokenEvidence {
    let mut runs = 0u64;
    let mut work_tokens_total = 0u64;
    let mut work_tokens_observed_runs = 0u64;
    let mut narrator_tokens_total = 0u64;
    let mut narrator_tokens_observed_runs = 0u64;
    let mut guide_tokens_total = 0u64;
    let mut guide_tokens_observed_runs = 0u64;
    for record in records {
        runs += 1;
        if let Some(tokens) = record.work_tokens {
            work_tokens_total += tokens;
            work_tokens_observed_runs += 1;
        }
        if let Some(tokens) = record.narrator_tokens {
            narrator_tokens_total += tokens;
            narrator_tokens_observed_runs += 1;
        }
        if let Some(tokens) = record.guide_tokens {
            guide_tokens_total += tokens;
            guide_tokens_observed_runs += 1;
        }
    }
    TokenEvidence {
        work_tokens_total,
        work_tokens_observed_runs,
        work_tokens_missing_runs: runs - work_tokens_observed_runs,
        narrator_tokens_total,
        narrator_tokens_observed_runs,
        narrator_tokens_missing_runs: runs - narrator_tokens_observed_runs,
        guide_tokens_total,
        guide_tokens_observed_runs,
        guide_tokens_missing_runs: runs - guide_tokens_observed_runs,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StatsReport {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_since_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_trait_id: Option<String>,
    pub total_runs: u64,
    pub unreadable_runs: u64,
    pub trait_matched_runs: u64,
    pub timestamp_missing_runs: u64,
    pub matched_runs: u64,
    pub outcomes: OutcomeCounts,
    pub token_evidence: TokenEvidence,
    pub refinement_rounds: RefinementRoundsEvidence,
    pub traits: Vec<TraitRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct OutcomeCounts {
    pub completed: u64,
    pub blocked: u64,
    pub exhausted_unapproved: u64,
    pub killed: u64,
    pub no_outcome_recorded: u64,
    pub other: u64,
    /// Every non-empty, non-`completed`/`blocked` `outcome` value actually
    /// observed among matched runs, with its exact count, sorted by value.
    /// Preserves the `other` total losslessly rather than collapsing it.
    pub other_values: Vec<OutcomeValueCount>,
}

/// Average refinement rounds to approval, derived from
/// [`RunRecord::verdict_rounds`] and averaged only over `Completed` matched
/// runs (see the module doc's heuristic). `average_rounds` is `None` when no
/// completed run carries verdict-slot evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RefinementRoundsEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_rounds: Option<f64>,
    pub completed_runs_observed: u64,
    pub completed_runs_missing: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct OutcomeValueCount {
    pub value: String,
    pub runs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TokenEvidence {
    pub work_tokens_total: u64,
    pub work_tokens_observed_runs: u64,
    pub work_tokens_missing_runs: u64,
    pub narrator_tokens_total: u64,
    pub narrator_tokens_observed_runs: u64,
    pub narrator_tokens_missing_runs: u64,
    pub guide_tokens_total: u64,
    pub guide_tokens_observed_runs: u64,
    pub guide_tokens_missing_runs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitRow {
    pub trait_id: String,
    pub runs: u64,
    pub token_evidence: TokenEvidence,
    pub digests: Vec<DigestRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DigestRow {
    /// `None` is the explicit missing-digest bucket: a readable ledger with
    /// no persisted `canonical-digest` (e.g. written before that field
    /// existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    pub runs: u64,
}

/// Aggregate already-filtered-by-nothing run records into a [`StatsReport`].
/// `since_epoch` and `trait_id` are applied here (trait filter first, then
/// the inclusive timestamp cutoff) so a caller need only load records and
/// resolve the cutoff; every other count is deterministic from the input
/// order-independent of iteration order (all groupings are explicitly
/// sorted below).
pub fn aggregate(
    records: &[RunRecord],
    total_runs: u64,
    unreadable_runs: u64,
    since_epoch: Option<u64>,
    trait_id: Option<&str>,
) -> StatsReport {
    let trait_filtered: Vec<&RunRecord> = records
        .iter()
        .filter(|record| trait_id.is_none_or(|wanted| record.trait_id == wanted))
        .collect();
    let trait_matched_runs = trait_filtered.len() as u64;

    let timestamp_missing_runs = trait_filtered
        .iter()
        .filter(|record| record.recorded_at_epoch.is_none())
        .count() as u64;

    let matched: Vec<&RunRecord> = trait_filtered
        .iter()
        .copied()
        .filter(|record| match since_epoch {
            None => true,
            Some(cutoff) => record
                .recorded_at_epoch
                .is_some_and(|epoch| epoch >= cutoff),
        })
        .collect();
    let matched_runs = matched.len() as u64;

    let mut completed = 0u64;
    let mut blocked = 0u64;
    let mut exhausted_unapproved = 0u64;
    let mut killed = 0u64;
    let mut no_outcome_recorded = 0u64;
    let mut other_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut refinement_rounds_total = 0u64;
    let mut completed_runs_observed = 0u64;
    let mut completed_runs_missing = 0u64;
    for record in &matched {
        let bucket = classify_outcome(record);
        if matches!(bucket, OutcomeBucket::Completed) {
            match record.verdict_rounds {
                Some(rounds) => {
                    refinement_rounds_total += rounds;
                    completed_runs_observed += 1;
                }
                None => completed_runs_missing += 1,
            }
        }
        match bucket {
            OutcomeBucket::Completed => completed += 1,
            OutcomeBucket::Blocked => blocked += 1,
            OutcomeBucket::ExhaustedUnapproved => exhausted_unapproved += 1,
            OutcomeBucket::Killed => killed += 1,
            OutcomeBucket::NoOutcomeRecorded => no_outcome_recorded += 1,
            OutcomeBucket::Other(value) => *other_counts.entry(value).or_insert(0) += 1,
        }
    }
    let other = other_counts.values().sum();
    let other_values = other_counts
        .into_iter()
        .map(|(value, runs)| OutcomeValueCount { value, runs })
        .collect();
    let refinement_rounds = RefinementRoundsEvidence {
        average_rounds: (completed_runs_observed > 0)
            .then(|| refinement_rounds_total as f64 / completed_runs_observed as f64),
        completed_runs_observed,
        completed_runs_missing,
    };

    let token_evidence = accumulate_token_evidence(matched.iter().copied());

    let mut by_trait: BTreeMap<String, Vec<&RunRecord>> = BTreeMap::new();
    for record in &matched {
        by_trait
            .entry(record.trait_id.clone())
            .or_default()
            .push(record);
    }
    let traits = by_trait
        .into_iter()
        .map(|(trait_id, trait_records)| {
            let runs = trait_records.len() as u64;
            let mut digest_counts: BTreeMap<Option<String>, u64> = BTreeMap::new();
            for record in &trait_records {
                let digest_key = record
                    .canonical_digest
                    .as_ref()
                    .map(|digest| digest.as_str().to_string());
                *digest_counts.entry(digest_key).or_insert(0) += 1;
            }
            let digests = digest_counts
                .into_iter()
                .map(|(digest, runs)| DigestRow { digest, runs })
                .collect();
            let token_evidence = accumulate_token_evidence(trait_records.iter().copied());
            TraitRow {
                trait_id,
                runs,
                token_evidence,
                digests,
            }
        })
        .collect();

    StatsReport {
        schema_version: STATS_SCHEMA_VERSION.to_string(),
        applied_since_epoch: since_epoch,
        applied_trait_id: trait_id.map(str::to_string),
        total_runs,
        unreadable_runs,
        trait_matched_runs,
        timestamp_missing_runs,
        matched_runs,
        outcomes: OutcomeCounts {
            completed,
            blocked,
            exhausted_unapproved,
            killed,
            no_outcome_recorded,
            other,
            other_values,
        },
        token_evidence,
        refinement_rounds,
        traits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::{Kind, Reference};

    fn record(
        trait_id: &str,
        status: Status,
        stop_reason: Option<&str>,
        drive_outcome_kind: Option<DriveOutcomeKind>,
        outcome: Option<&str>,
        recorded_at_epoch: Option<u64>,
        verdict_rounds: Option<u64>,
    ) -> RunRecord {
        RunRecord {
            trait_id: trait_id.to_string(),
            canonical_digest: None,
            status,
            stop_reason: stop_reason.map(|reason| StopReason {
                reason: reason.to_string(),
                at: Vec::new(),
                last_check: None,
            }),
            drive_outcome_kind,
            outcome: outcome.map(str::to_string),
            recorded_at_epoch,
            work_tokens: None,
            narrator_tokens: None,
            guide_tokens: None,
            verdict_rounds,
        }
    }

    fn verdict_revision(slot_id: &str) -> SlotRevision {
        SlotRevision {
            slot_ref: Reference::local(Kind::Slot, slot_id).expect("valid slot ref"),
            value_digest: Digest::source(slot_id),
            acceptance_order: 0,
            operation: None,
            submitted_payload: None,
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: None,
            runtime_binding: false,
            projection: None,
            position_path: Vec::new(),
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    #[test]
    fn empty_input_is_a_clean_zero_report() {
        let report = aggregate(&[], 0, 0, None, None);
        assert_eq!(report.schema_version, STATS_SCHEMA_VERSION);
        assert_eq!(report.total_runs, 0);
        assert_eq!(report.unreadable_runs, 0);
        assert_eq!(report.matched_runs, 0);
        assert_eq!(report.outcomes.completed, 0);
        assert_eq!(report.outcomes.killed, 0);
        assert_eq!(report.outcomes.exhausted_unapproved, 0);
        assert_eq!(report.refinement_rounds.average_rounds, None);
        assert!(report.traits.is_empty());
    }

    #[test]
    fn outcome_bucket_precedence_hand_counted() {
        let records = vec![
            // Status::Completed always wins, regardless of outcome string.
            record(
                "t",
                Status::Completed,
                None,
                None,
                Some("interrupted"),
                Some(1),
                Some(2),
            ),
            // Typed unassignment/permission-blocked statuses are `blocked`.
            record(
                "t",
                Status::BlockedAgentUnassigned,
                None,
                None,
                None,
                Some(1),
                None,
            ),
            // Status::Blocked with the loop runtime's own exhaustion stop
            // reason is `exhausted-unapproved`, not `blocked`.
            record(
                "t",
                Status::Blocked,
                Some("max-iterations-exhausted"),
                None,
                None,
                Some(1),
                None,
            ),
            // Status::Blocked with any other stop reason stays `blocked`.
            record(
                "t",
                Status::Blocked,
                Some("some-other-reason"),
                None,
                None,
                Some(1),
                None,
            ),
            // A typed Killed drive outcome kind counts as `killed` even
            // though the raw outcome string also says "killed".
            record(
                "t",
                Status::AwaitingAgentOutput,
                None,
                Some(DriveOutcomeKind::Killed),
                Some("killed"),
                Some(1),
                None,
            ),
            // No recorded drive outcome at all.
            record(
                "t",
                Status::AwaitingAgentOutput,
                None,
                None,
                None,
                Some(1),
                None,
            ),
            // An untyped, non-completed/blocked outcome string is preserved
            // verbatim in `other`, never coerced into an invented bucket.
            record(
                "t",
                Status::Failed,
                None,
                None,
                Some("max-frames-exhausted"),
                Some(1),
                None,
            ),
        ];
        let report = aggregate(&records, 7, 0, None, None);
        assert_eq!(report.outcomes.completed, 1);
        assert_eq!(report.outcomes.blocked, 2);
        assert_eq!(report.outcomes.exhausted_unapproved, 1);
        assert_eq!(report.outcomes.killed, 1);
        assert_eq!(report.outcomes.no_outcome_recorded, 1);
        assert_eq!(report.outcomes.other, 1);
        assert_eq!(
            report.outcomes.other_values,
            vec![OutcomeValueCount {
                value: "max-frames-exhausted".to_string(),
                runs: 1,
            }]
        );
    }

    #[test]
    fn since_and_trait_filters_compose() {
        let records = vec![
            record(
                "alpha",
                Status::Completed,
                None,
                None,
                None,
                Some(100),
                None,
            ),
            record("alpha", Status::Completed, None, None, None, Some(50), None),
            record("beta", Status::Completed, None, None, None, Some(100), None),
        ];
        let report = aggregate(&records, 3, 0, Some(100), Some("alpha"));
        assert_eq!(report.trait_matched_runs, 2);
        assert_eq!(report.matched_runs, 1);
        assert_eq!(report.outcomes.completed, 1);
        assert_eq!(report.traits.len(), 1);
        assert_eq!(report.traits[0].trait_id, "alpha");
        assert_eq!(report.traits[0].runs, 1);
    }

    #[test]
    fn timestamp_missing_runs_are_counted_but_excluded_from_since_matches() {
        let records = vec![
            record("t", Status::Completed, None, None, None, None, None),
            record("t", Status::Completed, None, None, None, Some(10), None),
        ];
        let report = aggregate(&records, 2, 0, Some(0), None);
        assert_eq!(report.trait_matched_runs, 2);
        assert_eq!(report.timestamp_missing_runs, 1);
        assert_eq!(report.matched_runs, 1);
    }

    #[test]
    fn refinement_rounds_average_over_completed_runs_with_missing_coverage() {
        let records = vec![
            record("t", Status::Completed, None, None, None, Some(1), Some(3)),
            record("t", Status::Completed, None, None, None, Some(1), Some(1)),
            // Completed but no verdict-slot evidence: missing, not zero.
            record("t", Status::Completed, None, None, None, Some(1), None),
            // Not completed: never counted toward the average either way.
            record(
                "t",
                Status::Blocked,
                Some("some-other-reason"),
                None,
                None,
                Some(1),
                Some(5),
            ),
        ];
        let report = aggregate(&records, 4, 0, None, None);
        assert_eq!(report.refinement_rounds.completed_runs_observed, 2);
        assert_eq!(report.refinement_rounds.completed_runs_missing, 1);
        assert_eq!(report.refinement_rounds.average_rounds, Some(2.0));
    }

    #[test]
    fn verdict_slot_rounds_takes_the_max_revision_count_across_loops_and_ignores_non_verdict_slots()
    {
        let revisions = vec![
            verdict_revision("review-a-verdict"),
            verdict_revision("review-a-verdict"),
            verdict_revision("review-b-verdict"),
            verdict_revision("review-a-verdict"),
            verdict_revision("draft"),
        ];
        assert_eq!(verdict_slot_rounds(&revisions), Some(3));
        assert_eq!(verdict_slot_rounds(&[verdict_revision("draft")]), None);
        assert_eq!(verdict_slot_rounds(&[]), None);
    }

    #[test]
    fn unreadable_runs_pass_through_total_and_unreadable_counts_untouched() {
        let report = aggregate(&[], 5, 3, None, None);
        assert_eq!(report.total_runs, 5);
        assert_eq!(report.unreadable_runs, 3);
        assert_eq!(report.trait_matched_runs, 0);
        assert_eq!(report.matched_runs, 0);
    }
}
