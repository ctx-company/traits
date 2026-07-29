//! Pure aggregation over already-loaded run-session ledgers (P442).
//!
//! `ctx traits stats` is strictly read-only: this module never touches a
//! filesystem, clock, or process. Callers (the CLI/IO boundary) load the
//! run inventory and resolve the `--since` cutoff, then hand every readable
//! [`Session`](crate::procedure::session::Session) plus each unreadable
//! ledger's count in here for deterministic counting.
//!
//! Outcome classification prefers the session's typed [`Status`] over the
//! free-form `last-drive-outcome.outcome` string wherever the typed value is
//! unambiguous: `Status::Completed` always counts as `completed`, and the
//! two unassignment/permission blocked variants (`BlockedAgentUnassigned`,
//! `BlockedCommandPermissionRequired`) always count as `blocked`, regardless
//! of what (if anything) the latest drive outcome recorded. `Status::Blocked`
//! whose recorded [`StopReason`] is the loop runtime's own
//! `max-iterations-exhausted` token is deliberately NOT counted as `blocked`:
//! a bounded loop that spent its iteration budget without matching `until`
//! is exhaustion, not a hard block, and the ledger schema has no typed
//! marker distinguishing a genuinely unapproved exhaustion from one that
//! later completed by another route — so that case falls through to the raw
//! `outcome` string like any other ambiguous status. Every other status
//! (including plain `Status::Blocked` with any other or no stop reason)
//! falls back to the raw `outcome` string: `None` is `no-outcome-recorded`,
//! `"completed"`/`"blocked"` are their buckets, and anything else —
//! including exhaustion, kill, or other free-form values the ledger schema
//! does not type — is preserved verbatim in the sorted `other` breakdown
//! rather than coerced into an invented bucket such as
//! `exhausted-unapproved` or `killed`. Token totals sum only the evidence
//! present in the latest recorded `token_usage` per run; they are latest-
//! drive evidence, not cumulative lifetime usage.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::procedure::runtime::StopReason;
use crate::procedure::session::{Session, Status};

/// The loop runtime's own stop-reason token for iteration-budget exhaustion
/// (`modules/core/src/procedure/runtime/state.rs`'s
/// `STOP_MAX_ITERATIONS_EXHAUSTED`, not re-exported so duplicated verbatim
/// here rather than widening that module's visibility for one string).
const STOP_MAX_ITERATIONS_EXHAUSTED: &str = "max-iterations-exhausted";

pub const STATS_SCHEMA_VERSION: &str = "1";

/// One readable ledger's evidence, extracted by the IO boundary from a
/// loaded [`Session`] before calling [`aggregate`]. Kept minimal and owned
/// so this module never depends on IO types (e.g. `InventoryOutcome`).
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub trait_id: String,
    pub canonical_digest: Option<Digest>,
    pub status: Status,
    pub stop_reason: Option<StopReason>,
    pub outcome: Option<String>,
    pub recorded_at_epoch: Option<u64>,
    pub work_tokens: Option<u64>,
    pub narrator_tokens: Option<u64>,
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
            outcome: last_drive_outcome.map(|outcome| outcome.outcome.as_str().to_string()),
            recorded_at_epoch: last_drive_outcome.map(|outcome| outcome.recorded_at_epoch),
            work_tokens: token_usage.and_then(|usage| usage.work_tokens),
            narrator_tokens: token_usage.and_then(|usage| usage.narrator_tokens),
        }
    }
}

/// Which outcome bucket a record belongs in, per the precedence documented
/// on this module: the typed [`Status`] wins when it unambiguously means
/// completed or blocked; otherwise the raw `outcome` string decides.
enum OutcomeBucket {
    Completed,
    Blocked,
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
        Status::Blocked if !is_exhausted_block => OutcomeBucket::Blocked,
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
    }
    TokenEvidence {
        work_tokens_total,
        work_tokens_observed_runs,
        work_tokens_missing_runs: runs - work_tokens_observed_runs,
        narrator_tokens_total,
        narrator_tokens_observed_runs,
        narrator_tokens_missing_runs: runs - narrator_tokens_observed_runs,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    pub traits: Vec<TraitRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct OutcomeCounts {
    pub completed: u64,
    pub blocked: u64,
    pub no_outcome_recorded: u64,
    pub other: u64,
    /// Every non-empty, non-`completed`/`blocked` `outcome` value actually
    /// observed among matched runs, with its exact count, sorted by value.
    /// Preserves the `other` total losslessly rather than collapsing it.
    pub other_values: Vec<OutcomeValueCount>,
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
    let mut no_outcome_recorded = 0u64;
    let mut other_counts: BTreeMap<String, u64> = BTreeMap::new();
    for record in &matched {
        match classify_outcome(record) {
            OutcomeBucket::Completed => completed += 1,
            OutcomeBucket::Blocked => blocked += 1,
            OutcomeBucket::NoOutcomeRecorded => no_outcome_recorded += 1,
            OutcomeBucket::Other(value) => *other_counts.entry(value).or_insert(0) += 1,
        }
    }
    let other = other_counts.values().sum();
    let other_values = other_counts
        .into_iter()
        .map(|(value, runs)| OutcomeValueCount { value, runs })
        .collect();

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
            no_outcome_recorded,
            other,
            other_values,
        },
        token_evidence,
        traits,
    }
}
