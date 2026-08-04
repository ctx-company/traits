// Procedure runtime state.
/// Procedure runtime state definitions.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::digest::Digest;
use crate::procedure::run::{Id, effective_sequence_items};
use crate::reference::{Kind, Reference};
use crate::response::CapabilityReport;
use crate::r#trait::condition::{
    ComparisonOperandEvidence, ConditionComparisonEvidence, ConditionComparisonOperator,
    ConditionComparisonSubject, ConditionEvaluation, ConditionEvaluationScope, GuardExpr,
    GuardOutcome,
};
use crate::r#trait::procedure::{
    BranchFailureEntry, BranchFailurePolicy, CommandPlan, ExhaustionDisposition, ExhaustionTarget,
    FailureTarget, JoinPolicy, MAX_SEQUENCE_NESTING_DEPTH, OutputSink, ProjectionSource,
    SequenceKind, WriteOperation, command_plan_for_item,
};
use crate::r#trait::prompt::{PromptClassification, classify_prompt, scan_interpolations};
use crate::r#trait::{PortDirection, Trait};

const FRAME_TEXT_LIMIT: usize = 16_384;
const MAX_GUARD_EVALUATION_DEPTH: usize = 64;
const MAX_CONTROL_ADVANCE_BUDGET: usize = 1_000_000;
const MAX_GUARD_EXPLANATIONS: usize = 4;
const STOP_CONTROL_ADVANCE_BUDGET_EXHAUSTED: &str = "control-advance-budget-exhausted";
const STOP_CONTROL_INDEX_OVERFLOW: &str = "control-index-overflow";
const STOP_FOR_EACH_ITEM_BINDING_REJECTED: &str = "for-each-item-binding-rejected";
const STOP_FOR_EACH_ITEM_MISSING: &str = "for-each-item-missing";
const STOP_FOR_EACH_MISSING_OVER: &str = "for-each-missing-over";
const STOP_FOR_EACH_OVER_NOT_LIST: &str = "for-each-over-not-list";
const STOP_GUARD_CONFLICT: &str = "guard-conflict";
const STOP_ITEM_INDEX_OVERFLOW: &str = "item-index-overflow";
const STOP_ITERATION_INDEX_OVERFLOW: &str = "iteration-index-overflow";
const STOP_MAX_ITEMS_EXCEEDED: &str = "max-items-exceeded";
const STOP_MAX_ITERATIONS_EXHAUSTED: &str = "max-iterations-exhausted";
const STOP_MAX_SEQUENCE_DEPTH_EXCEEDED: &str = "max-sequence-depth-exceeded";
const STOP_NESTED_SEQUENCE_FAILED: &str = "nested-sequence-failed";
const STOP_NO_CURRENT_EXECUTABLE_ITEM: &str = "no-current-executable-item";
const STOP_RUN_INDEX_OVERFLOW: &str = "run-index-overflow";
const STOP_STOP_IF_MATCHED: &str = "stop-if-matched";
const STOP_UNRESOLVED_RUNTIME_SEQUENCE: &str = "unresolved-runtime-sequence";
const STOP_PARALLEL_QUORUM_VERDICT_FAILED: &str = "parallel-quorum-verdict-failed";
const STOP_PARALLEL_BRANCH_PARKED: &str = "parallel-branch-parked";
const RUNTIME_STOP_REASON_TOKENS: &[&str] = &[
    STOP_CONTROL_ADVANCE_BUDGET_EXHAUSTED,
    STOP_CONTROL_INDEX_OVERFLOW,
    STOP_FOR_EACH_ITEM_BINDING_REJECTED,
    STOP_FOR_EACH_ITEM_MISSING,
    STOP_FOR_EACH_MISSING_OVER,
    STOP_FOR_EACH_OVER_NOT_LIST,
    STOP_GUARD_CONFLICT,
    STOP_ITEM_INDEX_OVERFLOW,
    STOP_ITERATION_INDEX_OVERFLOW,
    STOP_MAX_ITEMS_EXCEEDED,
    STOP_MAX_ITERATIONS_EXHAUSTED,
    STOP_MAX_SEQUENCE_DEPTH_EXCEEDED,
    STOP_NESTED_SEQUENCE_FAILED,
    STOP_NO_CURRENT_EXECUTABLE_ITEM,
    STOP_RUN_INDEX_OVERFLOW,
    STOP_STOP_IF_MATCHED,
    STOP_UNRESOLVED_RUNTIME_SEQUENCE,
    STOP_PARALLEL_QUORUM_VERDICT_FAILED,
    STOP_PARALLEL_BRANCH_PARKED,
];

fn is_false(value: &bool) -> bool {
    !*value
}

/// Canonical runtime-control signal reference for a terminal `stop-if` park,
/// distinct from any trait-declared `on-failure` target: nothing was
/// exhausted, so the loop's failure signal would misattribute the stop.
fn stop_if_matched_signal_ref() -> String {
    format!("signal:{STOP_STOP_IF_MATCHED}")
}

// ---------------------------------------------------------------------------
// Runtime value evidence
// ---------------------------------------------------------------------------

/// Acceptance state for supplied runtime evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum AcceptanceStatus {
    Accepted,
    Rejected,
}

/// Source of a runtime value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ValueSource {
    HostInput,
    CliSet,
    DeclaredDefault,
    TraitConfig,
    Binding,
    CommandOutput,
    ModelOutput,
    ManualOutput,
    Resource,
    Ledger,
}

/// Trusted local command execution bound to a command-backed slot value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct CommandExecutionEvidence {
    pub argv: Vec<String>,
    pub output_slot: String,
    #[serde(
        default,
        rename = "executable-digest",
        skip_serializing_if = "Option::is_none"
    )]
    pub executable_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Bounded tail of the command's output, kept so a check's verdict record
    /// can say WHY it failed and still replay from the ledger alone.
    ///
    /// The captured stdout/stderr live on the submission's evidence, which is
    /// not persisted; without this field a replayed verdict could never
    /// reproduce a value containing them. Optional and skipped when absent, so
    /// every ledger written before it existed still decodes and still replays
    /// — those runs simply carry no tail, which is what they had.
    #[serde(
        default,
        rename = "output-tail",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_tail: Option<String>,
}

/// Pure schema-validation outcome for a runtime value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SchemaStatus {
    Accepted,
    Rejected,
    IoPending,
    Unsupported,
}

/// Evidence for one schema-validation attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SchemaValidation {
    pub ref_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<Reference>,
    pub status: SchemaStatus,
    pub reason: String,
}

/// Accepted or rejected runtime value evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Value {
    pub ref_text: String,
    pub value: JsonValue,
    pub value_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<Reference>,
    pub source: ValueSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_execution: Option<CommandExecutionEvidence>,
    #[serde(
        default,
        rename = "producer-agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub producer_agent: Option<String>,
    #[serde(
        default,
        rename = "producer-harness",
        skip_serializing_if = "Option::is_none"
    )]
    pub producer_harness: Option<String>,
    /// Set exactly when this value was accepted from a `SequenceKind::Check`
    /// step's own verdict output, so downstream consumers can recover
    /// check-specific producer evidence (bounded stdout/stderr) without
    /// re-deriving "was this ref a check output" by re-scanning the trait's
    /// procedure — which cannot see check items nested inside loop/branch/
    /// parallel bodies. Stamped once at accept time from the runtime item
    /// that actually produced the value (`ready.item.effective_kind()`),
    /// which already resolves correctly regardless of nesting depth.
    #[serde(
        default,
        rename = "producer-check-verdict",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub producer_check_verdict: bool,
    pub acceptance: AcceptanceStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_validation: Vec<SchemaValidation>,
}

/// Resource evidence supplied by the IO edge before frame generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ResourceEvidence {
    pub resource_ref: Reference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    #[serde(default)]
    pub byte_size: u64,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default)]
    pub available: bool,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Sequence and final state
// ---------------------------------------------------------------------------

/// Runtime status for one effective sequence item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SequenceStatusKind {
    Pending,
    Ready,
    Blocked,
    Accepted,
    Rejected,
    Routed,
    Skipped,
    DependencyPending,
}

/// Runtime status for one effective sequence item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceStatus {
    pub sequence_index: usize,
    pub run_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub title: String,
    pub status: SequenceStatusKind,
    pub reason: String,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
}

/// One structural runtime position segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PathSegment {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
}

/// Runtime-only isolation buffer for effects accepted while a `parallel`
/// branch is active but not yet merged at the panel's barrier. Never part of
/// the committed ledger (`State.accepted_slot_values` etc.) until merge —
/// pure runtime staging, round-tripped through the ledger only so a
/// multi-call run can resume mid-branch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EffectBuffer {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_slot_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_output_port_values: Vec<Value>,
    #[serde(
        default,
        rename = "slot-revisions",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub slot_revisions: Vec<SlotRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitted_signals: Vec<SignalEmission>,
}

impl EffectBuffer {
    pub fn is_empty(&self) -> bool {
        self.accepted_slot_values.is_empty()
            && self.accepted_output_port_values.is_empty()
            && self.slot_revisions.is_empty()
            && self.emitted_signals.is_empty()
    }
}

/// Active sequence-control frame persisted in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ControlFrame {
    pub kind: ControlKind,
    pub parent_run_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_item_id: Option<String>,
    pub sequence_id: String,
    pub next_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// True only for a `Loop` frame authored with neither `max-iterations`
    /// nor `max-iterations-from` (0093). `max_iterations: None` already
    /// means "not applicable" on every non-loop frame kind, so this flag is
    /// the explicit signal that a `Loop` frame's `None` means "no bound"
    /// rather than reusing the ambiguous value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unbounded: bool,
    #[serde(default, rename = "max-items", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_digest: Option<Digest>,
    /// P402: the authored `concurrent = true` intent for a `for-each` frame,
    /// carried through unchanged from `SequenceItem::concurrent`. Purely
    /// evidence for a CLI/IO-layer conductor deciding whether it may
    /// speculatively dispatch this activation's items ahead of the cursor —
    /// the core runtime itself always advances `for-each` items one at a
    /// time, in order, regardless of this flag. `false` (the serde default)
    /// for every other control kind and for a non-concurrent `for-each`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub concurrent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<GuardExpr>,
    #[serde(default, rename = "stop-if", skip_serializing_if = "Option::is_none")]
    pub stop_if: Option<GuardExpr>,
    #[serde(
        default,
        rename = "on-exhausted",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_exhausted: Option<ExhaustionTarget>,
    /// Authored terminal signal(s) for a `stop-if` park, carried through
    /// unchanged from `SequenceItem::on_stop`. When present, the runtime
    /// emits these in place of the canonical `signal:stop-if-matched` so a
    /// trait's own name for its stop condition (e.g.
    /// `recurring-blocker-unresolved`) reaches the ledger and `run-status`.
    #[serde(default, rename = "on-stop", skip_serializing_if = "Option::is_none")]
    pub on_stop: Option<ExhaustionTarget>,
    #[serde(
        default,
        rename = "on-complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_complete: Option<String>,
    #[serde(
        default,
        rename = "on-failure",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_failure: Option<crate::r#trait::procedure::FailureTarget>,
    /// Authored-order local sequence ids for a `parallel` frame's branches,
    /// resolved once at entry. `iteration_index` indexes into this list to
    /// select the branch whose body `sequence_id`/`next_index` currently
    /// track; empty for every other control kind.
    #[serde(
        default,
        rename = "parallel-branch-sequence-ids",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parallel_branch_sequence_ids: Vec<String>,
    /// Isolation buffer for the branch currently in progress
    /// (`iteration_index`). Never visible outside this frame's own reads
    /// until the branch completes, at which point it is drained into
    /// `parallel_committed_branches`.
    #[serde(
        default,
        rename = "parallel-buffer",
        skip_serializing_if = "EffectBuffer::is_empty"
    )]
    pub parallel_buffer: EffectBuffer,
    /// Completed branch buffers, in authored branch order, held back from
    /// the enclosing target until the barrier (the last branch completes) so
    /// a sibling branch's evidence never leaks into an in-progress branch's
    /// reads.
    #[serde(
        default,
        rename = "parallel-committed-branches",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parallel_committed_branches: Vec<EffectBuffer>,
    /// `state.branch_decisions.len()`/`state.guard_evaluations.len()` at the
    /// moment the branch currently tracked by this `parallel` frame
    /// (`iteration_index`) started. Both vectors are append-only and never
    /// reordered by `sort_state`, so a rejected branch's rollback can
    /// truncate each back to its watermark to drop every nested branch
    /// decision and guard evaluation the abandoned activation produced,
    /// without disturbing evidence from earlier completed sibling branches
    /// (pushed, and therefore watermarked, before this branch started).
    /// Unused for every other control kind.
    #[serde(
        default,
        rename = "branch-decisions-watermark",
        skip_serializing_if = "is_zero"
    )]
    pub branch_decisions_watermark: usize,
    #[serde(
        default,
        rename = "guard-evaluations-watermark",
        skip_serializing_if = "is_zero"
    )]
    pub guard_evaluations_watermark: usize,
    /// Resolved barrier join policy for a `parallel` frame. Absent for every
    /// other control kind, and absent/omitted for a panel that did not
    /// declare `join` (behaviorally `collect-in-order`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinPolicy>,
    /// Resolved ordered per-branch failure policy for a `parallel` frame.
    #[serde(
        default,
        rename = "branch-failure",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub branch_failure: Vec<BranchFailureEntry>,
    /// Authored-order branch ref strings (`sequence:<id>`) for a `parallel`
    /// frame, preserved verbatim (unlike `parallel_branch_sequence_ids`,
    /// which stores resolved bare sequence ids) so ledger evidence and
    /// `branch-failure` matching can compare against the exact authored ref.
    #[serde(
        default,
        rename = "parallel-branch-refs",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parallel_branch_refs: Vec<String>,
    /// Per-branch resolved outcome, indexed like `parallel_branch_refs`.
    /// `None` for a branch not yet reached (e.g. trailing branches after a
    /// `park`).
    #[serde(
        default,
        rename = "parallel-branch-outcomes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parallel_branch_outcomes: Vec<Option<ParallelBranchOutcome>>,
}

/// Resolved terminal outcome for one `parallel` branch activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ParallelBranchOutcome {
    /// The branch ran to completion and its effects were merged at the
    /// barrier (or folded into the join's aggregate write).
    Committed,
    /// The branch's terminal failure policy discarded its effects; the panel
    /// continued with the remaining branches.
    Skipped,
    /// The branch's terminal failure policy left the run in a clean,
    /// resumable blocked state.
    Parked,
}

/// The panel's overall routing disposition once its final outcome is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ParallelPanelDisposition {
    /// The barrier resolved normally (including a matched `quorum-verdict`).
    Completed,
    /// A `panel-fail` branch or failed `quorum-verdict` routed to a recovery
    /// step (this panel's own `on-failure` or an enclosing one).
    Routed,
    /// A `panel-fail` branch or failed `quorum-verdict` had no route and
    /// stopped the run.
    Stopped,
    /// A `park` branch failure policy left the run in a clean, resumable
    /// blocked state.
    Parked,
}

/// One authored branch's ledger evidence within a [`ParallelPanelRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ParallelPanelBranchRecord {
    #[serde(rename = "branch-ref")]
    pub branch_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ParallelBranchOutcome>,
}

/// Append-only evidence for one `parallel` panel activation, recorded exactly
/// once the panel's final disposition is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ParallelPanelRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_item_id: Option<String>,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(rename = "join-policy")]
    pub join_policy: String,
    pub branches: Vec<ParallelPanelBranchRecord>,
    #[serde(
        default,
        rename = "result-digest",
        skip_serializing_if = "Option::is_none"
    )]
    pub result_digest: Option<Digest>,
    #[serde(
        default,
        rename = "guard-evaluation-index",
        skip_serializing_if = "Option::is_none"
    )]
    pub guard_evaluation_index: Option<usize>,
    pub disposition: ParallelPanelDisposition,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Closed identity for runtime-control signal emissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ControlEmissionIdentity {
    pub kind: ControlKind,
    pub parent_run_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_item_id: Option<String>,
    pub sequence_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ControlKind {
    Sequence,
    Branch,
    Loop,
    ForEach,
    Parallel,
}

/// Immutable evidence of one branch selection. The record is written before
/// entering an arm so a resumed ledger cannot select a different arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct BranchDecision {
    pub parent_run_index: usize,
    pub branch_id: String,
    #[serde(default, rename = "position-path", skip_serializing_if = "Vec::is_empty")]
    pub position_path: Vec<PathSegment>,
    pub matched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<GuardExpr>,
    /// Inclusive start of this decision's contiguous guard-evaluation range.
    /// The existing `guard_evaluation_index` is the final branch marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_evaluation_start_index: Option<usize>,
    /// Greatest slot-revision acceptance order recorded when evaluation
    /// started. This binds slot operands to that decision-time boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_revision_watermark: Option<usize>,
    /// Index of the immutable final guard evaluation that selected this arm.
    pub guard_evaluation_index: usize,
    /// `then`, `otherwise`, or `none`.
    pub selected_arm: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
}

/// Immutable evidence of one guard-conditioned resource-input inclusion
/// decision (P290). Mirrors [`BranchDecision`]'s replay-authenticated shape
/// so ledger validation can reuse the same guard-replay machinery instead of
/// trusting the plain `ConditionEvaluation` marker text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ConditionalInputDecision {
    pub sequence_index: usize,
    #[serde(rename = "ref")]
    pub ref_text: String,
    #[serde(default, rename = "position-path", skip_serializing_if = "Vec::is_empty")]
    pub position_path: Vec<PathSegment>,
    pub matched: bool,
    pub when: GuardExpr,
    /// Inclusive start of this decision's contiguous guard-evaluation range.
    /// The existing `guard_evaluation_index` is the final inclusion marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_evaluation_start_index: Option<usize>,
    /// Greatest slot-revision acceptance order recorded when evaluation
    /// started. This binds slot operands to that decision-time boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_revision_watermark: Option<usize>,
    /// Index of the immutable final guard evaluation that recorded inclusion.
    pub guard_evaluation_index: usize,
}

/// Immutable evidence for a signal-gated human ask activation. This is kept
/// separate from guarded inputs because it governs cursor advancement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AskDecision {
    pub sequence_index: usize,
    #[serde(default, rename = "position-path", skip_serializing_if = "Vec::is_empty")]
    pub position_path: Vec<PathSegment>,
    pub matched: bool,
    pub when: GuardExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_evaluation_start_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_revision_watermark: Option<usize>,
    pub guard_evaluation_index: usize,
}

/// Immutable evidence that a failed control path was abandoned for recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FailureRouteRecord {
    pub source_run_index: usize,
    pub source_step_id: String,
    pub target_run_index: usize,
    pub target_step_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, rename = "position-path", skip_serializing_if = "Vec::is_empty")]
    pub position_path: Vec<PathSegment>,
}

/// Append-only slot revision evidence for repeated writes across scopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct RevisionValue {
    pub value: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SlotRevision {
    pub slot_ref: Reference,
    pub value_digest: Digest,
    pub acceptance_order: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<WriteOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_payload: Option<RevisionValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_value_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_value: Option<RevisionValue>,
    /// The [`ValueSource`] the written value actually carried at acceptance
    /// time (e.g. `model-output`, `manual-output`, `command-output`) —
    /// preserved so historical reconstruction (`preview_historical_frame`)
    /// can restate a slot's true provenance instead of fabricating one.
    /// Absent only for ledgers written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ValueSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_execution: Option<CommandExecutionEvidence>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub runtime_binding: bool,
    /// Deterministic internal projection that produced this revision. Present
    /// only for `kind = "project"` writes and sufficient to replay the source
    /// field selection against the preceding source revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionProvenance>,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
}

/// Replayable provenance for one atomic `project` write. Exactly one of
/// `source_ref`/`source_value_digest` (a slot-backed source) or
/// `literal_digest` (a canonical literal source, P431) is present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct ProjectionProvenance {
    /// Absent only for a literal-backed projection (P431). Ledgers written
    /// before P431 always carry this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<Reference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_value_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Digest of the canonical literal declared on the projection. Present
    /// only for a literal-backed projection; absent (and defaulted) on every
    /// ledger written before P431.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal_digest: Option<Digest>,
}

impl ProjectionProvenance {
    pub fn is_literal(&self) -> bool {
        self.literal_digest.is_some()
    }
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn is_replace_write_operation(operation: &WriteOperation) -> bool {
    operation == &WriteOperation::Replace
}

/// Structured terminal stop reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StopReason {
    pub reason: String,
    #[serde(default, rename = "at", skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<PathSegment>,
    #[serde(
        default,
        rename = "last-check",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_check: Option<usize>,
}

/// Output-port completion state from accepted runtime slot values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum OutputPortStatus {
    Accepted,
    Missing,
    OptionalMissing,
}

/// Completion evidence for one declared procedure output port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct OutputPortCompletion {
    pub port_ref: Reference,
    pub value_slot_ref: Reference,
    pub required: bool,
    pub status: OutputPortStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_digest: Option<Digest>,
    pub reason: String,
}

/// Final state of a procedure run ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum FinalState {
    Running,
    Blocked,
    Completed,
    Failed,
    Rejected,
}

/// Executable procedure run ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct State {
    pub run_id: Id,
    pub trait_id: String,
    /// Run-level user override: when true, every loop stops the run blocked at
    /// exhaustion regardless of its own `on-exhausted` policy, whose default is
    /// to continue. Recorded in the ledger so the receipt shows which policy
    /// governed.
    #[serde(
        default,
        rename = "strict-loops",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub strict_loops: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,
    pub current_run_index: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence_statuses: Vec<SequenceStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_port_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_slot_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_output_port_values: Vec<Value>,
    #[serde(
        default,
        rename = "slot-revisions",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub slot_revisions: Vec<SlotRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_evidence: Vec<ResourceEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitted_signals: Vec<SignalEmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_attempts: Vec<RejectedAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_capability_reports: Vec<CapabilityReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_ports: Vec<OutputPortCompletion>,
    #[serde(default, rename = "active-path", skip_serializing_if = "Vec::is_empty")]
    pub active_path: Vec<PathSegment>,
    #[serde(
        default,
        rename = "control-stack",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub control_stack: Vec<ControlFrame>,
    #[serde(default, rename = "branch-decisions", skip_serializing_if = "Vec::is_empty")]
    pub branch_decisions: Vec<BranchDecision>,
    #[serde(
        default,
        rename = "conditional-input-decisions",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub conditional_input_decisions: Vec<ConditionalInputDecision>,
    #[serde(default, rename = "ask-decisions", skip_serializing_if = "Vec::is_empty")]
    pub ask_decisions: Vec<AskDecision>,
    #[serde(default, rename = "failure-routes", skip_serializing_if = "Vec::is_empty")]
    pub failure_routes: Vec<FailureRouteRecord>,
    #[serde(
        default,
        rename = "guard-evaluations",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub guard_evaluations: Vec<ConditionEvaluation>,
    /// Append-only `parallel` panel activation evidence — exactly one entry
    /// per panel activation once its final disposition is known.
    #[serde(
        default,
        rename = "parallel-panel-records",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parallel_panel_records: Vec<ParallelPanelRecord>,
    #[serde(
        default,
        rename = "stop-reason",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_reason: Option<StopReason>,
    /// Cumulative active-drive elapsed seconds, supplied by the IO layer
    /// (the CLI drive owns measurement; the core runtime never reads a
    /// clock) and monotonically non-decreasing across resumes. Defaults to
    /// zero on ledgers written before this field existed, and while paused
    /// or unattached — only active drive time accrues.
    #[serde(default, rename = "elapsed-seconds")]
    pub elapsed_seconds: u64,
    pub final_state: FinalState,
}
