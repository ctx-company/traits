//! Pure chronological narrative builder over an already-loaded run-session
//! ledger (P383). `ctx traits story <run-id>` is strictly read-only: this
//! module never touches a filesystem, clock, or process. Callers (the
//! CLI/IO boundary) resolve the ledger and, optionally, the trait's dry
//! [`Plan`] (for producer/agent enrichment) and hand both to [`build`].
//!
//! The chronological spine is [`State::slot_revisions`], not
//! `State::accepted_slot_values`: the latter is sorted alphabetically by ref
//! and holds only each slot's final value, so a loop's superseded iteration-0
//! verdict would be silently lost. `slot_revisions` is append-only in
//! acceptance order and carries the value as written at the time, so a
//! multi-iteration arc (review → revise → apply-fixes → review again) comes
//! out in the order it actually happened. A ledger written before
//! `slot_revisions` existed (`#[serde(default)]`, so it deserializes as an
//! empty vec) falls back to `State::sequence_statuses` order instead, with
//! [`StorySpine::SequenceStatuses`] on the report saying so — this module
//! never fabricates an act the ledger does not record.
//!
//! `branch_decisions`, `conditional_input_decisions`, `failure_routes`,
//! `guard_evaluations`, and `parallel_panels` are reused verbatim from the
//! ledger's own typed evidence rather than re-modeled, and each already
//! carries its own position path. They are reported as separate ordered
//! lists (each preserves its own append/evaluation order) rather than forced
//! into one merged timeline with `beats`: the ledger has no single counter
//! that orders slot-revision acceptance against guard evaluation against
//! branch selection, so claiming a merged order would be a guess this module
//! declines to make.

use crate::digest::Digest;
use crate::procedure::activity::ActivityEvent;
use crate::procedure::run::{Plan, PlannedSequenceItem};
use crate::procedure::runtime::{
    BranchDecision, CommandExecutionEvidence, ConditionalInputDecision, FailureRouteRecord,
    FinalState, ParallelPanelRecord, PathSegment, SequenceStatus, SequenceStatusKind, SlotRevision,
    State, StopReason, ValueSource,
};
use crate::procedure::session::{
    CompletionNotification, DriveOutcome, MergeFrame, Session, Status, select_agent_assignment,
};
use crate::r#trait::condition::ConditionEvaluation;
use crate::r#trait::procedure::WriteOperation;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub const STORY_SCHEMA_VERSION: &str = "1";

/// Bound on any single free-text preview/advisory gloss, in characters (not
/// bytes, so a multi-byte-safe truncation never splits a UTF-8 sequence).
/// `pub` so the CLI shell's command-argv rendering bounds by the same rule
/// rather than inventing a second limit (P383 review round 1, blocker
/// `unbounded-command-argv-in-render`).
pub const GLOSS_CHAR_BOUND: usize = 160;

/// The requested depth of a rendered story (P550/P521). Threaded through
/// every story-consuming surface (`ctx traits story --level`, `--story` on a
/// driven run, `[run] story` config, the dashboard's `S` key). `Default`
/// renders per-step summaries and derived bullets from the ledger and,
/// where available, the persisted activity sidecar — always free. `Detailed`
/// adds the full persisted activity timeline. `Assisted` additionally spends
/// one offline narrator model call per step; it is the ONLY level that
/// spends. A caller that resolves `Detailed`/`Assisted` against a ledger
/// with no activity sidecar (predating P521, or a concurrent-wave frame —
/// wave dispatch does not attach activity observers) degrades to `Default`
/// with a stated notice — see `ctx_traits_cli::app::story::degrade_notice` —
/// rather than silently rendering the default content as if it had honored
/// the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum StoryLevel {
    Default,
    Detailed,
    Assisted,
}

impl StoryLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            StoryLevel::Default => "default",
            StoryLevel::Detailed => "detailed",
            StoryLevel::Assisted => "assisted",
        }
    }

    /// Whether resolving this level spends a narrator model call (P550's
    /// token-cost decision): only ever true for an explicit `assisted`
    /// request, never for the default-on path.
    pub fn spends_model_call(self) -> bool {
        matches!(self, StoryLevel::Assisted)
    }
}

impl std::str::FromStr for StoryLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(StoryLevel::Default),
            "detailed" => Ok(StoryLevel::Detailed),
            "assisted" => Ok(StoryLevel::Assisted),
            other => Err(format!(
                "unknown story level {other:?} (expected default, detailed, or assisted)"
            )),
        }
    }
}

impl std::fmt::Display for StoryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which ledger evidence produced [`StoryReport::beats`] — reported so a
/// reader (and `--json` consumer) knows whether the arc reflects true
/// per-iteration acceptance order or the coarser legacy fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum StorySpine {
    SlotRevisions,
    SequenceStatuses,
}

/// Whether the trait/plan resolved for producer-role enrichment. `LedgerOnly`
/// is not an error: every other field of the report still renders in full,
/// only `beats[].actor` degrades to `"unrecorded"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum StoryEnrichment {
    Trait,
    LedgerOnly,
}

/// One persisted activity event, stamped with the wall-clock time the IO
/// boundary observed it (P521). Core stays clock-free — `at_epoch_ms` is
/// always supplied by the caller (the CLI/IO boundary, reading the activity
/// sidecar), never computed here.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TimedActivityEvent {
    pub at_epoch_ms: u64,
    pub event: ActivityEvent,
}

/// One persisted P455 finished-step summary (P521). `key` matches the
/// `frame_id` convention every activity event for the same step also uses
/// (`item_id`-or-title) — see the module doc on how beats stitch to it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TimedStepSummary {
    pub at_epoch_ms: u64,
    pub key: String,
    pub role: String,
    pub text: String,
}

/// The activity sidecar's contents, already read and parsed by the CLI/IO
/// boundary — core does no IO, so `build` only ever receives this typed,
/// already-tolerant-parsed value (or `None` when no sidecar was found).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ActivityInput {
    pub events: Vec<TimedActivityEvent>,
    pub step_summaries: Vec<TimedStepSummary>,
    /// Trailing lines the sidecar reader could not parse (a truncated last
    /// line from a killed process) — reported so `Detailed`/`Assisted` can
    /// say so honestly rather than silently under-counting.
    pub skipped_lines: usize,
}

/// Which evidence produced a beat's `summary_line` (P521's resolution
/// order): a recorded P455 step summary first, then a short typed output
/// reading of the beat's own accepted value, then a bullet derived purely
/// from the activity stream. Reported so `--json` consumers (and a reader)
/// can tell a authored summary from a machine-derived one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SummaryLineSource {
    StepSummary,
    TypedOutput,
    Derived,
}

/// Whether — and how completely — this report's activity evidence was
/// available. `Absent` degrades `Detailed`/`Assisted` honestly rather than
/// claiming a request was honored; `Partial` still renders everything that
/// did parse, but says how much did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ActivityProvenance {
    Recorded,
    Absent,
    Partial { skipped_lines: usize },
}

/// One conventional-field reading of a slot's JSON value: `status`/`verdict`,
/// `blockers[].id`, `escalation`, and a bounded advisory when the object
/// carries them, otherwise a bounded generic fallback. This is the single
/// place `verdict`/`blockers`/`escalation` convention-reading lives — see the
/// module doc on [`crate::procedure::story`] and
/// `ctx-traits-cli`'s `run_format::print_escalation_blockers`, which reuses
/// this instead of re-reading the convention itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ValueGloss {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<GenericGloss>,
}

/// Fallback gloss for a value that carries none of the recognized
/// conventional fields: a bounded preview of its first non-empty line (for
/// text) plus its byte length and, for an array, its element count. Never the
/// full body — see the module doc's "bounded by construction" note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct GenericGloss {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub byte_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_count: Option<usize>,
}

/// Bound `text` to [`GLOSS_CHAR_BOUND`] characters, appending an ellipsis
/// when truncated. `pub` for the same reason [`GLOSS_CHAR_BOUND`] is: the
/// CLI shell's command-argv rendering reuses this exact bound rather than
/// inventing a second one.
pub fn bounded(text: &str) -> String {
    let mut truncated: String = text.chars().take(GLOSS_CHAR_BOUND).collect();
    if text.chars().count() > GLOSS_CHAR_BOUND {
        truncated.push('…');
    }
    truncated
}

/// Read `value` for the `verdict`/`status`/`blockers[].id`/`escalation`/
/// `advisory`/`forgiveness-reason` implement-family conventions. `value` is
/// not itself a runtime type — these are conventions authored traits happen
/// to use, not something the runtime schema types — so an object carrying
/// none of them degrades to [`GenericGloss`] rather than an error.
pub fn value_gloss(value: &JsonValue) -> ValueGloss {
    let Some(object) = value.as_object() else {
        return generic_gloss(value);
    };

    let status = object
        .get("status")
        .or_else(|| object.get("verdict"))
        .and_then(|v| v.as_str())
        .map(bounded);

    let mut blockers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(entries) = object.get("blockers").and_then(|v| v.as_array()) {
        for entry in entries {
            let id = entry
                .as_object()
                .and_then(|o| o.get("id"))
                .and_then(|v| v.as_str());
            if let Some(id) = id {
                blockers.insert(id.to_string());
            }
        }
    }

    let escalation = object
        .get("escalation")
        .and_then(|v| v.as_str())
        .map(bounded);

    let advisory = object
        .get("advisory")
        .or_else(|| object.get("forgiveness-reason"))
        .and_then(|v| v.as_str())
        .map(bounded);

    if status.is_none() && blockers.is_empty() && escalation.is_none() && advisory.is_none() {
        return generic_gloss(value);
    }

    ValueGloss {
        status,
        blockers: blockers.into_iter().collect(),
        escalation,
        advisory,
        generic: None,
    }
}

fn generic_gloss(value: &JsonValue) -> ValueGloss {
    let byte_len = serde_json::to_string(value).map(|s| s.len()).unwrap_or(0);
    let element_count = value.as_array().map(Vec::len);
    let preview = match value {
        JsonValue::String(text) => text.lines().find(|line| !line.trim().is_empty()),
        _ => None,
    }
    .map(bounded);
    ValueGloss {
        generic: Some(GenericGloss {
            preview,
            byte_len,
            element_count,
        }),
        ..Default::default()
    }
}

/// One beat in the chronological arc: a single slot write, in acceptance (or
/// legacy status-fallback) order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StoryBeat {
    pub acceptance_order: usize,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The matched [`SequenceStatus::reason`] for this beat's position, e.g.
    /// "all declared outputs accepted" or "branch decision recorded".
    /// Empty when no status matched this position (never fabricated).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// `role@harness (model)` when a plan/producer resolved it,
    /// `role@harness`/`role (unassigned)` for a partial resolution, or
    /// `"unrecorded"` when neither the ledger nor the plan carries producer
    /// identity for this write. Never guessed.
    pub actor: String,
    pub ref_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<WriteOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ValueSource>,
    pub gloss: ValueGloss,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandExecutionEvidence>,
    /// One-line summary for the DEFAULT-level section header, resolved in
    /// order: recorded P455 step summary → short typed output → derived
    /// from the activity stream. Absent when no activity input was supplied
    /// and no typed output was short enough to stand in for one (P521).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_source: Option<SummaryLineSource>,
    /// The [`frame_key_for_status`] key this beat's position resolved to
    /// (`item_id`-or-title), i.e. the same convention every persisted
    /// activity event's `frame_id` uses. `None` when no status matched this
    /// position, in which case no activity events can be attributed to this
    /// beat. Exposed so downstream consumers (the assisted pass) stitch
    /// events to beats with the one convention the sidecar writes, rather
    /// than re-deriving a key of their own (P521 review round 2, blocker
    /// `assisted-prose-frame-key-mismatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_key: Option<String>,
    /// Derived bullets for the DEFAULT level: files touched, retries with
    /// their reason, thinking-token totals — built only from activity tool
    /// inputs (never tool results) plus this beat's own `command`. Empty
    /// when no activity input was supplied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bullets: Vec<String>,
    /// ASSISTED level only: one narrator-model prose paragraph summarizing
    /// this beat's detailed activity, or a per-beat failure notice when the
    /// call itself failed. Never populated by `build` — filled in afterward
    /// by the CLI/IO boundary's assisted pass (`build` never spends a model
    /// call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assisted_prose: Option<String>,
}

/// The full chronological narrative of one run-session ledger.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StoryReport {
    pub schema_version: String,
    pub run_id: String,
    pub trait_id: String,
    pub spine: StorySpine,
    pub enrichment: StoryEnrichment,
    pub status: Status,
    pub final_state: FinalState,
    pub elapsed_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    pub beats: Vec<StoryBeat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branch_decisions: Vec<BranchDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_input_decisions: Vec<ConditionalInputDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_routes: Vec<FailureRouteRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guard_evaluations: Vec<ConditionEvaluation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parallel_panels: Vec<ParallelPanelRecord>,
    pub emitted_signals: u64,
    pub rejected_submissions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drive: Option<DriveOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CompletionNotification>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_frames: Vec<MergeFrame>,
    /// P521: whether persisted activity evidence was available for this
    /// run at all, and how completely it parsed.
    pub activity_provenance: ActivityProvenance,
    /// DETAILED level: every persisted activity event in order with
    /// timestamps. Empty when no activity input was supplied — see
    /// `activity_provenance`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detailed_timeline: Vec<TimedActivityEvent>,
    /// ASSISTED level only: total narrator tokens spent producing every
    /// `beats[].assisted_prose`, filled in by the CLI's assisted pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assisted_narrator_tokens: Option<u64>,
    /// ASSISTED level only: set when the whole pass could not run (no
    /// narrator seat resolved) — never set for a per-beat failure, which
    /// degrades that beat's own `assisted_prose` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assisted_unavailable: Option<String>,
}

/// Build the full story for `session`, optionally enriched with the trait's
/// dry [`Plan`] for producer-role resolution. `plan` is `None` whenever the
/// CLI boundary could not resolve the trait source (moved, drifted digest,
/// or simply not requested) — every other field still renders in full; see
/// the module doc.
pub fn build(
    session: &Session,
    plan: Option<&Plan>,
    activity: Option<&ActivityInput>,
) -> StoryReport {
    let state = &session.ledger;
    let (spine, beats) = if !state.slot_revisions.is_empty() {
        (
            StorySpine::SlotRevisions,
            beats_from_slot_revisions(state, session, plan, activity),
        )
    } else {
        (
            StorySpine::SequenceStatuses,
            beats_from_sequence_statuses(state, session, plan, activity),
        )
    };
    let activity_provenance = match activity {
        None => ActivityProvenance::Absent,
        Some(activity) if activity.skipped_lines > 0 => ActivityProvenance::Partial {
            skipped_lines: activity.skipped_lines,
        },
        Some(_) => ActivityProvenance::Recorded,
    };
    let detailed_timeline = activity
        .map(|activity| activity.events.clone())
        .unwrap_or_default();

    StoryReport {
        schema_version: STORY_SCHEMA_VERSION.to_string(),
        run_id: session.run_id.as_str().to_string(),
        trait_id: session.trait_id.clone(),
        spine,
        enrichment: if plan.is_some() {
            StoryEnrichment::Trait
        } else {
            StoryEnrichment::LedgerOnly
        },
        status: session.status.clone(),
        final_state: state.final_state.clone(),
        elapsed_seconds: state.elapsed_seconds,
        stop_reason: state.stop_reason.clone(),
        beats,
        branch_decisions: state.branch_decisions.clone(),
        conditional_input_decisions: state.conditional_input_decisions.clone(),
        failure_routes: state.failure_routes.clone(),
        guard_evaluations: state.guard_evaluations.clone(),
        parallel_panels: state.parallel_panel_records.clone(),
        emitted_signals: session.emitted_signals.len() as u64,
        rejected_submissions: session.rejected_submissions.len() as u64,
        last_drive: session.last_drive_outcome.clone(),
        completion: session.completion.clone(),
        merge_frames: session.provenance.merge_frames.clone(),
        activity_provenance,
        detailed_timeline,
        assisted_narrator_tokens: None,
        assisted_unavailable: None,
    }
}

fn beats_from_slot_revisions(
    state: &State,
    session: &Session,
    plan: Option<&Plan>,
    activity: Option<&ActivityInput>,
) -> Vec<StoryBeat> {
    let mut revisions: Vec<&SlotRevision> = state.slot_revisions.iter().collect();
    revisions.sort_by_key(|revision| revision.acceptance_order);
    revisions
        .into_iter()
        .map(|revision| {
            let ref_text = revision.slot_ref.as_str().to_string();
            let gloss = revision
                .submitted_payload
                .as_ref()
                .map(|payload| value_gloss(&payload.value))
                .unwrap_or_default();
            let matched_status = match_status(&state.sequence_statuses, &revision.position_path);
            let actor = resolve_actor(
                plan,
                session,
                matched_status,
                &ref_text,
                revision.value_digest.as_str(),
            );
            let frame_key = matched_status.map(frame_key_for_status);
            let (summary_line, summary_source, bullets) = enrich_from_activity(
                activity,
                frame_key.as_deref(),
                typed_summary_candidate(&gloss),
                revision.command_execution.as_ref(),
            );
            StoryBeat {
                acceptance_order: revision.acceptance_order,
                position_path: revision.position_path.clone(),
                title: matched_status.map(|status| status.title.clone()),
                reason: matched_status
                    .map(|status| status.reason.clone())
                    .unwrap_or_default(),
                actor,
                ref_text,
                value_digest: Some(revision.value_digest.clone()),
                operation: revision.operation.clone(),
                source: revision.source.clone(),
                gloss,
                command: revision.command_execution.clone(),
                summary_line,
                summary_source,
                frame_key: frame_key.clone(),
                bullets,
                assisted_prose: None,
            }
        })
        .collect()
}

/// Legacy fallback for a ledger predating `slot_revisions`: one beat per
/// [`SequenceStatus`] that actually accepted a value, attributing the
/// `accepted_slot_values` entry the plan's `producer_edges` say that
/// position produced. Never fabricates an act: a status that has not
/// reached [`SequenceStatusKind::Accepted`] (pending, ready, blocked,
/// dependency-pending) is skipped outright rather than rendered as though
/// it ran, and a producer edge whose slot has no matching accepted value is
/// skipped rather than rendered with an empty/`"unrecorded"` placeholder —
/// a run with nothing accepted yet renders zero beats, not a synthesized
/// arc (P383 review round 1, blocker `status-fallback-fabricates-unexecuted-beats`).
fn beats_from_sequence_statuses(
    state: &State,
    session: &Session,
    plan: Option<&Plan>,
    activity: Option<&ActivityInput>,
) -> Vec<StoryBeat> {
    let mut beats = Vec::new();
    for status in &state.sequence_statuses {
        if status.status != SequenceStatusKind::Accepted {
            continue;
        }
        let refs: Vec<String> = plan
            .map(|plan| {
                plan.producer_edges
                    .iter()
                    .filter(|edge| edge.run_index == status.run_index)
                    .map(|edge| edge.slot_ref.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();

        for ref_text in refs {
            let Some(value) = session
                .accepted_slot_values
                .iter()
                .find(|value| value.ref_text == ref_text)
            else {
                continue;
            };
            let gloss = value_gloss(&value.value);
            let actor = resolve_legacy_actor(value);
            let frame_key = frame_key_for_status(status);
            let (summary_line, summary_source, bullets) = enrich_from_activity(
                activity,
                Some(&frame_key),
                typed_summary_candidate(&gloss),
                value.command_execution.as_ref(),
            );
            beats.push(StoryBeat {
                acceptance_order: beats.len(),
                position_path: status.position_path.clone(),
                title: Some(status.title.clone()),
                reason: status.reason.clone(),
                actor,
                ref_text,
                value_digest: Some(value.value_digest.clone()),
                operation: None,
                source: Some(value.source.clone()),
                gloss,
                command: value.command_execution.clone(),
                summary_line,
                summary_source,
                frame_key: Some(frame_key.clone()),
                bullets,
                assisted_prose: None,
            });
        }
    }
    beats
}

/// The key an activity event/step-summary's `frame_id`/`key` uses for this
/// status's step: `item_id`-or-title, the same convention
/// `ActivityRecorder`'s CLI-side `frame_id` construction uses (drive.rs), so
/// the story can stitch events to beats with no new convention.
fn frame_key_for_status(status: &SequenceStatus) -> String {
    status
        .item_id
        .clone()
        .unwrap_or_else(|| status.title.clone())
}

/// A short standalone reading of the beat's own accepted value, usable as a
/// summary line when no step summary was recorded: the gloss's `status` (a
/// verdict word) if present, otherwise a generic preview line. Neither is
/// guaranteed short — both are already bounded by [`bounded`]/
/// [`GLOSS_CHAR_BOUND`] at gloss-construction time, so no extra truncation
/// is needed here.
fn typed_summary_candidate(gloss: &ValueGloss) -> Option<&str> {
    gloss
        .status
        .as_deref()
        .or_else(|| gloss.generic.as_ref().and_then(|g| g.preview.as_deref()))
}

/// Resolve one beat's `summary_line`/`summary_source`/`bullets` from
/// activity evidence, in P521's stated order: recorded step summary → short
/// typed output → derived from the activity stream. `command` is this
/// beat's own ledger `CommandExecutionEvidence` (not activity — the command
/// bullet always comes from the trusted ledger evidence, never a tool-input
/// echo).
fn enrich_from_activity(
    activity: Option<&ActivityInput>,
    frame_key: Option<&str>,
    typed_summary: Option<&str>,
    command: Option<&CommandExecutionEvidence>,
) -> (Option<String>, Option<SummaryLineSource>, Vec<String>) {
    let bullets = match (activity, frame_key) {
        (Some(activity), Some(frame_key)) => derive_bullets(activity, frame_key, command),
        _ => command.map(command_bullet).into_iter().collect(),
    };

    if let (Some(activity), Some(frame_key)) = (activity, frame_key)
        && let Some(step) = activity
            .step_summaries
            .iter()
            .rev()
            .find(|step| step.key == frame_key)
    {
        return (
            Some(step.text.clone()),
            Some(SummaryLineSource::StepSummary),
            bullets,
        );
    }

    if let Some(typed) = typed_summary {
        return (
            Some(typed.to_string()),
            Some(SummaryLineSource::TypedOutput),
            bullets,
        );
    }

    if !bullets.is_empty() {
        return (
            Some(bullets.join("; ")),
            Some(SummaryLineSource::Derived),
            bullets,
        );
    }

    (None, None, bullets)
}

/// Whether a `RunningTool` event's tool name is a known file-editing tool,
/// so its bounded input text is safe to present under the default level's
/// "touched N file(s)" bullet. Every other tool (`bash`, `read`, an unknown
/// or absent name) is never labeled a touched file — the bullet inventory
/// promises only edit/write evidence, never a raw tool-input echo (P521
/// review round 2, blocker `derived-bullets-fabricate-touched-files`).
fn is_edit_tool(tool: Option<&str>) -> bool {
    let Some(tool) = tool else {
        return false;
    };
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "edit"
            | "write"
            | "multiedit"
            | "multi_edit"
            | "notebookedit"
            | "notebook_edit"
            | "edit_file"
            | "write_file"
            | "create_file"
            | "apply_patch"
            | "str_replace_editor"
            | "str_replace_based_edit_tool"
    )
}

fn command_bullet(command: &CommandExecutionEvidence) -> String {
    let argv = bounded(&command.argv.join(" "));
    match command.exit_code {
        Some(0) => format!("ran `{argv}` (exit 0)"),
        Some(code) => format!("ran `{argv}` (exit {code})"),
        None => format!("ran `{argv}` (no exit code)"),
    }
}

/// Derived bullets from activity alone (tool inputs, never tool results):
/// files touched, retries with their reason, and a thinking-token total —
/// plus this beat's own command evidence. Never fabricates: a beat with no
/// matching activity events yields an empty vec.
fn derive_bullets(
    activity: &ActivityInput,
    frame_key: &str,
    command: Option<&CommandExecutionEvidence>,
) -> Vec<String> {
    let mut files = std::collections::BTreeSet::new();
    let mut retries = Vec::new();
    let mut thinking_tokens: u64 = 0;
    for timed in &activity.events {
        let event = &timed.event;
        if event.frame_id != frame_key {
            continue;
        }
        match event.kind {
            crate::procedure::activity::ActivityKind::RunningTool => {
                if is_edit_tool(event.tool.as_deref())
                    && let Some(text) = &event.text
                {
                    files.insert(bounded(text));
                }
            }
            crate::procedure::activity::ActivityKind::Retrying => {
                if let Some(text) = &event.text {
                    retries.push(bounded(text));
                }
            }
            crate::procedure::activity::ActivityKind::Thinking => {
                thinking_tokens += event.tokens.unwrap_or(0);
            }
            _ => {}
        }
    }
    let mut bullets = Vec::new();
    if !files.is_empty() {
        bullets.push(format!(
            "touched {} file(s): {}",
            files.len(),
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    for retry in retries {
        bullets.push(format!("retried: {retry}"));
    }
    if thinking_tokens > 0 {
        bullets.push(format!("thinking: ~{thinking_tokens} tokens"));
    }
    if let Some(command) = command {
        bullets.push(command_bullet(command));
    }
    bullets
}

fn resolve_legacy_actor(value: &crate::procedure::runtime::Value) -> String {
    let Some(agent) = &value.producer_agent else {
        return "unrecorded".to_string();
    };
    let role = normalize_producer_agent(agent);
    match &value.producer_harness {
        Some(harness) => format!("{role}@{}", normalize_producer_harness(harness)),
        None => role.to_string(),
    }
}

/// Strip the ledger's `agent:` producer-role prefix so it reads identically
/// to the plan-resolved `role@harness` shape.
fn normalize_producer_agent(agent: &str) -> &str {
    agent.strip_prefix("agent:").unwrap_or(agent)
}

/// The ledger's recorded `producer-harness` is the caller's raw evidence
/// string (e.g. `harness=claude-code version=2.1.220 (Claude Code)
/// transport=cli duration-ms=83385`), not the short harness id the
/// plan-resolved path renders. Extract just the id so both resolution paths
/// produce one actor shape (P383 review round 1 advisory).
fn normalize_producer_harness(harness: &str) -> String {
    let after_prefix = harness.strip_prefix("harness=").unwrap_or(harness);
    after_prefix
        .split_whitespace()
        .next()
        .unwrap_or(after_prefix)
        .to_string()
}

/// Resolve the acting role for one slot write. Prefers the accepted value's
/// own recorded `producer-agent`/`producer-harness` (present only for the
/// slot's *final* value) when the digest matches this exact revision, then
/// falls back to the plan's declared `agent_ref` for the sequence item at
/// `matched_status`'s own position (path-addressed, not `run_index`
/// -addressed — see [`plan_item_for_path`]'s doc), then `"unrecorded"`.
/// Never guessed.
fn resolve_actor(
    plan: Option<&Plan>,
    session: &Session,
    matched_status: Option<&SequenceStatus>,
    ref_text: &str,
    value_digest: &str,
) -> String {
    if let Some(value) = session
        .accepted_slot_values
        .iter()
        .find(|value| value.ref_text == ref_text && value.value_digest.as_str() == value_digest)
        && let Some(agent) = &value.producer_agent
    {
        let role = normalize_producer_agent(agent);
        return match &value.producer_harness {
            Some(harness) => format!("{role}@{}", normalize_producer_harness(harness)),
            None => role.to_string(),
        };
    }

    let (Some(plan), Some(status)) = (plan, matched_status) else {
        return "unrecorded".to_string();
    };
    let Some(item) = plan_item_for_path(&plan.sequence_items, &effective_status_path(status))
    else {
        return "unrecorded".to_string();
    };
    let Some(agent_ref) = &item.agent_ref else {
        return "unrecorded".to_string();
    };
    let role = agent_ref.id();
    let assignments = session
        .provenance
        .agent_assignments
        .as_deref()
        .unwrap_or(&[]);
    match select_agent_assignment(assignments, role, item.structural_seat) {
        Some(assignment) => match &assignment.model {
            Some(model) => format!("{}@{} ({model})", assignment.role, assignment.harness),
            None => format!("{}@{}", assignment.role, assignment.harness),
        },
        None => format!("{role} (unassigned)"),
    }
}

/// [`SequenceStatus::position_path`] is empty for a top-level (non-nested)
/// sequence item — the runtime never stamps a root segment on it — while
/// [`SlotRevision::position_path`] always carries one. Reconstruct the
/// equivalent root-only path from the status's own `item_id`/`run_index` so
/// both sides compare on the same shape.
fn effective_status_path(status: &SequenceStatus) -> Vec<PathSegment> {
    if !status.position_path.is_empty() {
        return status.position_path.clone();
    }
    vec![PathSegment {
        kind: "procedure".to_string(),
        id: status.item_id.clone(),
        index: status.run_index,
        iteration: None,
        item_index: None,
    }]
}

fn path_segments_equal(a: &PathSegment, b: &PathSegment) -> bool {
    a.kind == b.kind
        && a.id == b.id
        && a.index == b.index
        && a.iteration == b.iteration
        && a.item_index == b.item_index
}

fn common_prefix_len(a: &[PathSegment], b: &[PathSegment]) -> usize {
    a.iter()
        .zip(b)
        .take_while(|(left, right)| path_segments_equal(left, right))
        .count()
}

/// Find the [`SequenceStatus`] whose position path identifies the same
/// ledger position as `target` (a [`SlotRevision`]'s own position path):
/// exact match first, falling back to the status with the longest common
/// path prefix when no exact match exists (never a bare 0-length "match" —
/// that is no match at all). This is the load-bearing join that replaced a
/// slot-ref-keyed lookup: a slot with more than one producer in the ledger
/// (`slot:work-summary` from both `implement` and `apply-fixes`;
/// `slot:park-report` from both a clear and an append) has no unique
/// producer edge, but every revision's own position path identifies exactly
/// one status (P383 review round 1, blocker
/// `beat-attribution-ignores-sequence-statuses`).
fn match_status<'a>(
    statuses: &'a [SequenceStatus],
    target: &[PathSegment],
) -> Option<&'a SequenceStatus> {
    if let Some(exact) = statuses
        .iter()
        .find(|status| effective_status_path(status).as_slice() == target)
    {
        return Some(exact);
    }
    statuses
        .iter()
        .map(|status| {
            (
                common_prefix_len(&effective_status_path(status), target),
                status,
            )
        })
        .filter(|(len, _)| *len > 0)
        .max_by_key(|(len, _)| *len)
        .map(|(_, status)| status)
}

/// Locate the [`PlannedSequenceItem`] a ledger position `path` identifies,
/// walking the plan's authored tree structurally rather than by
/// `run_index`: a top-level control item's nested children, the arm of a
/// branch, and a loop's own body all legitimately share their enclosing
/// item's `run_index` (confirmed on a real ledger: `single-review`,
/// `review`, every `park-report-*` item, and `maybe-apply-fixes` are all
/// `run-index: 2`), so `run_index` alone cannot identify a nested item.
/// `path`'s first segment locates the top-level item by `run_index`; every
/// following non-`item`-kind segment is a further descent by the child's
/// authored `sequence_index` into whichever of `children`/
/// `otherwise_children`/a parallel branch's `children` contains it; the
/// final `item`-kind segment is a same-node confirmation, not an additional
/// hop, and is never followed by another descent (P383 review round 1,
/// blocker `beat-attribution-ignores-sequence-statuses`, root cause (b)).
fn plan_item_for_path<'a>(
    items: &'a [PlannedSequenceItem],
    path: &[PathSegment],
) -> Option<&'a PlannedSequenceItem> {
    let (root, rest) = path.split_first()?;
    let mut current = items.iter().find(|item| item.run_index == root.index)?;
    for segment in rest {
        if segment.kind == "item" {
            break;
        }
        current = descend_by_sequence_index(current, segment.index)?;
    }
    Some(current)
}

fn descend_by_sequence_index(
    item: &PlannedSequenceItem,
    sequence_index: usize,
) -> Option<&PlannedSequenceItem> {
    item.children
        .iter()
        .find(|child| child.sequence_index == sequence_index)
        .or_else(|| {
            item.otherwise_children
                .iter()
                .find(|child| child.sequence_index == sequence_index)
        })
        .or_else(|| {
            item.parallel_branches.iter().find_map(|branch| {
                branch
                    .children
                    .iter()
                    .find(|child| child.sequence_index == sequence_index)
            })
        })
}

#[cfg(test)]
mod activity_enrichment_tests {
    use super::*;
    use crate::procedure::activity::ActivityKind;

    fn timed(
        frame_id: &str,
        kind: ActivityKind,
        text: Option<&str>,
        tokens: Option<u64>,
    ) -> TimedActivityEvent {
        timed_tool(frame_id, kind, text, None, tokens)
    }

    fn timed_tool(
        frame_id: &str,
        kind: ActivityKind,
        text: Option<&str>,
        tool: Option<&str>,
        tokens: Option<u64>,
    ) -> TimedActivityEvent {
        TimedActivityEvent {
            at_epoch_ms: 0,
            event: ActivityEvent {
                sequence: 0,
                frame_id: frame_id.to_string(),
                kind,
                text: text.map(str::to_string),
                tool: tool.map(str::to_string),
                tokens,
                rate_limit: None,
            },
        }
    }

    #[test]
    fn resolution_order_prefers_step_summary_over_typed_over_derived() {
        let activity = ActivityInput {
            events: vec![timed_tool(
                "step-a",
                ActivityKind::RunningTool,
                Some("path=/tmp/a.rs"),
                Some("edit"),
                None,
            )],
            step_summaries: vec![TimedStepSummary {
                at_epoch_ms: 1,
                key: "step-a".to_string(),
                role: "agent:worker".to_string(),
                text: "wrote a.rs".to_string(),
            }],
            skipped_lines: 0,
        };
        let (summary, source, bullets) = enrich_from_activity(
            Some(&activity),
            Some("step-a"),
            Some("typed fallback"),
            None,
        );
        assert_eq!(summary.as_deref(), Some("wrote a.rs"));
        assert_eq!(source, Some(SummaryLineSource::StepSummary));
        assert!(!bullets.is_empty());
    }

    #[test]
    fn falls_back_to_typed_output_when_no_step_summary_recorded() {
        let activity = ActivityInput {
            events: Vec::new(),
            step_summaries: Vec::new(),
            skipped_lines: 0,
        };
        let (summary, source, _) = enrich_from_activity(
            Some(&activity),
            Some("step-a"),
            Some("typed fallback"),
            None,
        );
        assert_eq!(summary.as_deref(), Some("typed fallback"));
        assert_eq!(source, Some(SummaryLineSource::TypedOutput));
    }

    #[test]
    fn falls_back_to_derived_bullets_when_nothing_else_recorded() {
        let activity = ActivityInput {
            events: vec![
                timed_tool(
                    "step-a",
                    ActivityKind::RunningTool,
                    Some("edited x.rs"),
                    Some("edit"),
                    None,
                ),
                timed("step-a", ActivityKind::Thinking, Some("plan"), Some(3)),
                timed("step-b", ActivityKind::RunningTool, Some("unrelated"), None),
            ],
            step_summaries: Vec::new(),
            skipped_lines: 0,
        };
        let (summary, source, bullets) =
            enrich_from_activity(Some(&activity), Some("step-a"), None, None);
        assert_eq!(source, Some(SummaryLineSource::Derived));
        assert!(summary.is_some());
        assert!(bullets.iter().any(|b| b.contains("edited x.rs")));
        assert!(bullets.iter().any(|b| b.contains("tokens")));
        assert!(!bullets.iter().any(|b| b.contains("unrelated")));
    }

    #[test]
    fn derived_bullets_only_label_known_edit_tools_as_touched_files() {
        let activity = ActivityInput {
            events: vec![
                timed_tool(
                    "step-a",
                    ActivityKind::RunningTool,
                    Some("{\"command\":\"cargo test\"}"),
                    Some("bash"),
                    None,
                ),
                timed_tool(
                    "step-a",
                    ActivityKind::RunningTool,
                    Some("path=/tmp/a.rs"),
                    Some("edit"),
                    None,
                ),
            ],
            step_summaries: Vec::new(),
            skipped_lines: 0,
        };
        let bullets = derive_bullets(&activity, "step-a", None);
        let files_bullet = bullets
            .iter()
            .find(|bullet| bullet.starts_with("touched"))
            .expect("a touched-files bullet");
        assert!(files_bullet.contains("path=/tmp/a.rs"));
        assert!(!files_bullet.contains("cargo test"));
    }

    #[test]
    fn no_activity_input_yields_no_summary_and_no_bullets() {
        let (summary, source, bullets) = enrich_from_activity(None, Some("step-a"), None, None);
        assert!(summary.is_none());
        assert!(source.is_none());
        assert!(bullets.is_empty());
    }
}
