// Procedure run model definitions.
/// Procedure run model definitions.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reference::{Kind, Reference};
use crate::r#trait::Trait;
use crate::r#trait::port::PortDirection;
use crate::r#trait::procedure::{
    BranchFailureEntry, CommandPlan, JoinPolicy, SequenceKind, command_plan_for_item,
};
use crate::r#trait::prompt::{PromptClassification, classify_prompt};

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// A procedure run identifier. Must be non-empty if constructed from input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(crate::procedure::invalid_field(
                "procedure-run.id",
                "must not be empty",
            ));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Sequence item plan
// ---------------------------------------------------------------------------

/// How a planned sequence item's prompt is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PlannedPromptSource {
    /// Inline prompt text with typed-ref interpolations.
    Inline,
    /// A local `prompt:<id>` reference that exists in `[prompt.*]`.
    LocalPromptRef,
    /// A dependency-qualified `prompt:<dep>/<id>` reference.
    DependencyPendingPromptRef,
}

/// Planned sequence item kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PlannedSequenceKind {
    Prompt,
    Ask,
    Command,
    Check,
    Project,
    Sequence,
    Branch,
    Loop,
    ForEach,
    Parallel,
    Terminal,
}

/// A single planned branch of a `parallel` item. Branch identity (authored
/// `sequence:<id>` ref and author order) is retained rather than flattened into
/// a shared children list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PlannedParallelBranch {
    /// The authored local `sequence:<id>` ref for this branch.
    pub sequence_ref: Reference,
    /// Nested planned items for this branch's body. Structural plan only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<PlannedSequenceItem>,
}

/// The planned state of a single sequence item in a procedure run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PlannedSequenceItem {
    /// Zero-based source position in `procedure.sequence`.
    pub sequence_index: usize,
    /// Zero-based run position.
    pub run_index: usize,
    /// Optional ID from the sequence item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Human-readable title from the sequence item.
    pub title: String,
    /// The sequence item's input refs, copied from the item definition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_refs: Vec<Reference>,
    /// The sequence item's output refs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_refs: Vec<Reference>,
    /// Whether this item is prompt-backed or command-backed.
    pub kind: PlannedSequenceKind,
    /// Optional abstract agent role assigned to serve this executable item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ref: Option<Reference>,
    /// This item's zero-based role-local structural seat (P456), when it is
    /// a top-level `agent_ref` site — the same ordinal
    /// [`ctx_traits_core::procedure::runtime::AgentRole::structural_seat`]
    /// carries for a live/preview frame at this same declared site, so a
    /// static planned-item projection (a TUI/plain planned-step row) can
    /// select its own seat's exact configured harness instead of showing
    /// every seat's harness jointly. Absent for a nested `agent_ref` site
    /// (not resolvable from a declaration index alone) or when the item has
    /// no `agent_ref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural_seat: Option<u32>,
    /// Named sequence ref for sequence-control items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_ref: Option<Reference>,
    /// Alternate named sequence ref for a branch's false arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otherwise_sequence_ref: Option<Reference>,
    /// How the item's prompt is classified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_source: Option<PlannedPromptSource>,
    /// Normalized command plan for command-backed items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_plan: Option<CommandPlan>,
    /// Nested planned items for sequence-control bodies. This is a structural
    /// plan only; runtime iteration still uses the control stack.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<PlannedSequenceItem>,
    /// Nested items in a branch's false arm. Kept separate so dry plans never
    /// present either arm as unconditional work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub otherwise_children: Vec<PlannedSequenceItem>,
    /// Planned branches for a `parallel` item, in authored order. Each retains
    /// its own authored ref and children, never merged into `children`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parallel_branches: Vec<PlannedParallelBranch>,
    /// Required branch bound for a `parallel` item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_branches: Option<usize>,
    /// Declared barrier join policy for a `parallel` item. Omitted is
    /// behaviorally identical to `collect-in-order` (P263 barrier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinPolicy>,
    /// Declared ordered per-branch failure policy for a `parallel` item.
    #[serde(
        default,
        rename = "branch-failure",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub branch_failure: Vec<BranchFailureEntry>,
    /// Whether a `for-each` item requested concurrent execution. Evidence only;
    /// P262 rejects concurrent execution at runtime.
    #[serde(default, skip_serializing_if = "is_false")]
    pub concurrent: bool,
    /// The planned status of this item.
    pub status: SequenceItemStatus,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The status of a planned sequence item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SequenceItemStatus {
    /// Ready to execute (all local inputs available, no unresolved deps).
    Planned,
    /// Blocked on a local slot input that is not yet produced.
    Blocked,
    /// Waiting for dependency contents to be loaded.
    DependencyPending,
}

// ---------------------------------------------------------------------------
// Slot state
// ---------------------------------------------------------------------------

/// The state of a slot in the procedure run ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SlotState {
    /// Required by a sequence item input or output port but not yet available.
    Required,
    /// Planned to be produced by a sequence item in the sequence.
    PlannedProduced,
    /// Not produced and not available — the plan is incomplete.
    Missing,
}

/// A single slot's runtime state entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PlannedSlot {
    /// The slot ref string (e.g. `slot:scope`).
    pub slot_ref: Reference,
    /// The current state.
    pub state: SlotState,
}

fn slot_priority(state: &SlotState) -> u8 {
    // Global slot state is monotonic potential-production; per-item statuses
    // retain the first-pass order truth for reads before earlier producers run.
    match state {
        SlotState::PlannedProduced => 4,
        SlotState::Missing => 3,
        SlotState::Required => 1,
    }
}

fn upsert_slot(states: &mut BTreeMap<String, SlotState>, ref_text: &str, new: SlotState) {
    match states.get_mut(ref_text) {
        Some(current) => {
            if slot_priority(&new) > slot_priority(current) {
                *current = new;
            }
        }
        None => {
            states.insert(ref_text.to_string(), new);
        }
    }
}

// ---------------------------------------------------------------------------
// Producer edges
// ---------------------------------------------------------------------------

/// A producer edge: which sequence item produces which slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ProducerEdge {
    /// Zero-based source sequence index of the producing item.
    pub sequence_index: usize,
    /// Zero-based effective run position of the producing item.
    pub run_index: usize,
    /// The slot ref that is produced.
    pub slot_ref: Reference,
}

// ---------------------------------------------------------------------------
// Port requirements and output-port completion
// ---------------------------------------------------------------------------

/// The status of a port requirement in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PortRequirementStatus {
    /// Reserved `port:user-prompt` or `port:session` — runtime-provided.
    RuntimeProvided,
    /// Needs an external binding or provision artifact.
    BindingRequired,
    /// Dependency-qualified port — pending dependency load.
    DependencyPending,
}

/// A port requirement derived from `procedure.input` or sequence item inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PortRequirement {
    /// The port ref string (e.g. `port:user-prompt`).
    pub port_ref: Reference,
    /// The port ID (e.g. `user-prompt`).
    pub port_id: String,
    /// Whether the port is required (not optional).
    pub required: bool,
    /// The requirement status.
    pub status: PortRequirementStatus,
    /// Explanation.
    pub reason: String,
}

/// The completion status of an output port in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum OutputPortStatus {
    /// The output port's value slot is planned-produced by the procedure.
    PlannedProduced,
    /// The output port's value slot is missing — not produced.
    Missing,
    /// The output port is optional and its slot is not planned-produced.
    OptionalMissing,
}

/// A planned output port with its value-slot completion evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PlannedOutputPort {
    /// The port ref string (e.g. `port:finding`).
    pub port_ref: Reference,
    /// The value slot ref (e.g. `slot:finding`).
    pub value_slot_ref: Reference,
    /// Whether the port is required.
    pub required: bool,
    /// The completion status.
    pub status: OutputPortStatus,
    /// Explanation.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------

/// The acceptance state of the procedure run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum AcceptanceState {
    /// Not yet accepted or rejected.
    Pending,
    /// Accepted into the run ledger.
    Accepted,
    /// Rejected.
    Rejected,
}

// ---------------------------------------------------------------------------
// Run plan
// ---------------------------------------------------------------------------

/// A complete dry plan for a procedure run.
///
/// Produced by [`plan_procedure_run`]. Pure and deterministic. Contains no
/// actual slot values, model messages, or rendered output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Plan {
    /// The run identifier.
    pub run_id: Id,
    /// The trait ID this plan belongs to.
    pub trait_id: String,
    /// Planned sequence items in sequence order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence_items: Vec<PlannedSequenceItem>,
    /// Planned slot states.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<PlannedSlot>,
    /// Producer edges (sequence item index → slot).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub producer_edges: Vec<ProducerEdge>,
    /// Port requirements derived from procedure input and sequence items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_requirements: Vec<PortRequirement>,
    /// Planned output ports with value-slot completion evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_ports: Vec<PlannedOutputPort>,
    /// A declared `[sink.session-title]` (task 0110), surfaced so a dry-run
    /// plan shows the sink without firing it — the plan carries only the
    /// declaration itself (mode + input), never a rendered/dispatched
    /// title: dry-run performs no effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title_sink: Option<crate::r#trait::SessionTitleSink>,
    /// The acceptance state.
    pub acceptance: AcceptanceState,
}
