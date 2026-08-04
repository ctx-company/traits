/// Defines procedure declarations and validation constants.
/// Procedure model definitions.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::reference::{Kind, Reference};
use crate::r#trait::prompt::{PromptClassification, classify_prompt, scan_interpolations};
use crate::r#trait::{GuardExpr, PromptMap, Trait};

/// Kinds valid in procedure boundary input refs: input-direction ports only.
const PROCEDURE_INPUT_KINDS: &[Kind] = &[Kind::Port];

/// Kinds valid in procedure boundary output refs: output-direction ports only.
const PROCEDURE_OUTPUT_KINDS: &[Kind] = &[Kind::Port];

/// Kinds valid in sequence item input refs.
const SEQUENCE_INPUT_KINDS: &[Kind] = &[Kind::Port, Kind::Slot, Kind::Resource];

/// Kinds valid in sequence item output refs: local slots, terminal output ports,
/// or schema-backed ephemeral outputs.
const SEQUENCE_OUTPUT_KINDS: &[Kind] = &[Kind::Slot, Kind::Port, Kind::Schema];

/// Maximum nesting depth for named sequence/control expansion.
pub const MAX_SEQUENCE_NESTING_DEPTH: usize = 16;

/// Kinds that prompt-required interpolation/contract refs must satisfy
/// through exact presence in the sequence item input.
const PROMPT_REQUIRED_INPUT_KINDS: &[Kind] = &[Kind::Port, Kind::Slot, Kind::Resource];

// ---------------------------------------------------------------------------
// RefList — scalar-or-array normalized ref list
// ---------------------------------------------------------------------------

crate::shared::string_list_wrapper! {
    /// Normalized list of ref strings for procedure `input`, `output`, and
    /// `sequence` fields.
    ///
    /// Accepts scalar-or-array at the decode boundary but always stores a
    /// `Vec<String>`. Distinct from
    /// [`ContractRefList`](crate::trait::prompt::ContractRefList) so procedure
    /// contracts remain independent.
    #[schemars(rename = "ProcedureRefList")]
    #[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
    pub struct RefList
}

// ---------------------------------------------------------------------------
// Sequence item inputs — plain refs or guard-conditioned resource refs (P290)
// ---------------------------------------------------------------------------

/// One `procedure.sequence[].input` entry.
///
/// An ordinary entry is a plain typed ref string (`port:*`, `slot:*`,
/// `resource:*`). A resource ref may additionally carry a `when` guard,
/// authored as `{ ref = "resource:<id>", when = <GuardExpr> }`; the runtime
/// includes that resource in the step's available context and
/// `resource_evidence` only when the guard matches at readiness. A slot ref
/// may instead be authored as `{ slot = "slot:<id>", optional = true }`
/// (P447): that slot never gates validation, planning, recovery, or runtime
/// readiness, and is omitted from the frame entirely until an accepted value
/// exists. Port inputs and plain (non-decorated) slot inputs remain
/// unconditional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum SequenceInput {
    Ref(String),
    ConditionalResource {
        #[serde(rename = "ref")]
        #[schemars(rename = "ref")]
        ref_text: String,
        when: GuardExpr,
    },
    /// A slot input that never gates validation, planning, recovery, or
    /// runtime readiness (P447). Omitted from the frame entirely when no
    /// accepted value exists; presented through the normal available-input
    /// path once one does.
    OptionalSlot {
        slot: String,
        optional: bool,
    },
}

impl SequenceInput {
    pub fn ref_text(&self) -> &str {
        match self {
            Self::Ref(ref_text) => ref_text,
            Self::ConditionalResource { ref_text, .. } => ref_text,
            Self::OptionalSlot { slot, .. } => slot,
        }
    }

    pub fn guard(&self) -> Option<&GuardExpr> {
        match self {
            Self::Ref(_) | Self::OptionalSlot { .. } => None,
            Self::ConditionalResource { when, .. } => Some(when),
        }
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, Self::OptionalSlot { .. })
    }
}

impl<'de> Deserialize<'de> for SequenceInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum RawSequenceInput {
            Ref(String),
            ConditionalResource {
                #[serde(rename = "ref")]
                ref_text: String,
                when: GuardExpr,
            },
            OptionalSlot {
                slot: String,
                optional: bool,
            },
        }

        match RawSequenceInput::deserialize(deserializer)? {
            RawSequenceInput::Ref(ref_text) => Ok(Self::Ref(ref_text)),
            RawSequenceInput::ConditionalResource { ref_text, when } => {
                Ok(Self::ConditionalResource { ref_text, when })
            }
            RawSequenceInput::OptionalSlot { slot, optional } => {
                if !optional {
                    return Err(de::Error::custom(
                        "sequence input `optional` must be true; omit the field entirely for a required input",
                    ));
                }
                Ok(Self::OptionalSlot { slot, optional: true })
            }
        }
    }
}

/// Normalized list of `procedure.sequence[].input` entries.
///
/// Accepts scalar-or-array at the decode boundary, where each element is a
/// plain ref string, a guarded-resource table, or an optional-slot table
/// (P447). Always stores a `Vec<SequenceInput>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
pub struct SequenceInputList(Vec<SequenceInput>);

impl SequenceInputList {
    pub fn new(items: Vec<SequenceInput>) -> Self {
        Self(items)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, SequenceInput> {
        self.0.iter()
    }

    pub fn ref_texts(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(SequenceInput::ref_text)
    }

    pub fn guard_for(&self, ref_text: &str) -> Option<&GuardExpr> {
        self.0
            .iter()
            .find(|input| input.ref_text() == ref_text)
            .and_then(SequenceInput::guard)
    }

    pub fn is_optional_for(&self, ref_text: &str) -> bool {
        self.0
            .iter()
            .find(|input| input.ref_text() == ref_text)
            .is_some_and(SequenceInput::is_optional)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'de> Deserialize<'de> for SequenceInputList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SequenceInputListVisitor;

        impl<'de> Visitor<'de> for SequenceInputListVisitor {
            type Value = SequenceInputList;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a sequence input ref string, guarded-resource object, or list")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(SequenceInputList(vec![SequenceInput::Ref(
                    value.to_string(),
                )]))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(SequenceInputList(vec![SequenceInput::Ref(value)]))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<SequenceInput>()? {
                    items.push(item);
                }
                Ok(SequenceInputList(items))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let input =
                    SequenceInput::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(SequenceInputList(vec![input]))
            }
        }

        deserializer.deserialize_any(SequenceInputListVisitor)
    }
}

// ---------------------------------------------------------------------------
// Output sinks and signal emission rules
// ---------------------------------------------------------------------------

/// Runtime write operation for a slot output sink.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum WriteOperation {
    /// Replace the slot value with the supplied output value.
    #[default]
    Replace,
    /// Append the supplied output element to an array slot.
    Append,
    /// Deep-merge the supplied object delta into an object slot.
    Merge,
    /// Write the supplied output value to one named object field.
    SetField(String),
    /// Add the supplied numeric delta to a number slot.
    Increment,
}

const REPLACE_OPERATION: WriteOperation = WriteOperation::Replace;

/// A step output sink.
///
/// Existing manifests serialize a plain typed ref string. New authoring may use
/// an inline table for slot write operations, e.g.
/// `{ slot = "slot:notes", operation = "append" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum OutputSink {
    Ref(String),
    SlotOperation {
        slot: String,
        #[serde(default, skip_serializing_if = "is_replace_operation")]
        operation: WriteOperation,
    },
}

impl<'de> Deserialize<'de> for OutputSink {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum RawOutputSink {
            Ref(String),
            SlotOperation {
                slot: String,
                #[serde(default)]
                operation: WriteOperation,
            },
        }

        match RawOutputSink::deserialize(deserializer)? {
            RawOutputSink::Ref(ref_text) => Ok(Self::Ref(ref_text)),
            RawOutputSink::SlotOperation { slot, operation } => match operation {
                WriteOperation::Replace => Ok(Self::Ref(slot)),
                WriteOperation::Append
                | WriteOperation::Merge
                | WriteOperation::SetField(_)
                | WriteOperation::Increment => Ok(Self::SlotOperation { slot, operation }),
            },
        }
    }
}

impl OutputSink {
    pub fn ref_text(&self) -> &str {
        match self {
            Self::Ref(ref_text) => ref_text,
            Self::SlotOperation { slot, .. } => slot,
        }
    }

    pub fn operation(&self) -> &WriteOperation {
        match self {
            Self::Ref(_) => &REPLACE_OPERATION,
            Self::SlotOperation { operation, .. } => operation,
        }
    }
}

/// Normalized list of sequence output sinks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
pub struct OutputSinkList(Vec<OutputSink>);

impl OutputSinkList {
    pub fn new(items: Vec<OutputSink>) -> Self {
        Self(items)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, OutputSink> {
        self.0.iter()
    }

    pub fn ref_texts(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(OutputSink::ref_text)
    }

    pub fn sink_for_ref(&self, ref_text: &str) -> Option<&OutputSink> {
        self.0.iter().find(|sink| sink.ref_text() == ref_text)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'de> Deserialize<'de> for OutputSinkList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct OutputSinkListVisitor;

        impl<'de> Visitor<'de> for OutputSinkListVisitor {
            type Value = OutputSinkList;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a sequence output sink string, object, or list")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(OutputSinkList(vec![OutputSink::Ref(value.to_string())]))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(OutputSinkList(vec![OutputSink::Ref(value)]))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<OutputSink>()? {
                    items.push(item);
                }
                Ok(OutputSinkList(items))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let sink = OutputSink::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(OutputSinkList(vec![sink]))
            }
        }

        deserializer.deserialize_any(OutputSinkListVisitor)
    }
}

/// A signal ref or a deterministic signal derivation rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum SignalEmissionRule {
    Ref(String),
    Derived {
        signal: String,
        when: Box<GuardExpr>,
    },
}

impl SignalEmissionRule {
    pub fn signal_ref(&self) -> &str {
        match self {
            Self::Ref(ref_text) => ref_text,
            Self::Derived { signal, .. } => signal,
        }
    }

    pub fn when(&self) -> Option<&GuardExpr> {
        match self {
            Self::Ref(_) => None,
            Self::Derived { when, .. } => Some(when.as_ref()),
        }
    }
}

fn is_replace_operation(operation: &WriteOperation) -> bool {
    *operation == WriteOperation::Replace
}

// ---------------------------------------------------------------------------
// Parallel barrier join policy and per-branch failure policy (P264)
// ---------------------------------------------------------------------------

/// Closed, policy-discriminated barrier join for a `parallel` sequence item.
/// Omission on `SequenceItem.join` is behaviorally identical to
/// `{ policy = "collect-in-order" }` — the P263 barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "policy", rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub enum JoinPolicy {
    /// Every branch's own effects merge into the enclosing target in authored
    /// order. Exact P263 barrier behavior.
    CollectInOrder,
    /// Fold each committed branch's `source` slot value, in authored order,
    /// into one panel-owned `destination` slot using `merge`/`set-field`
    /// write-operation semantics. `append`/`replace`/`increment` are rejected.
    ReduceMerge {
        destination: String,
        source: String,
        operation: WriteOperation,
    },
    /// Collect each committed branch's `source` slot value, in authored
    /// order, into a panel-owned list-typed `destination` slot, then evaluate
    /// `guard` against the committed ledger. A false guard is a failed
    /// quorum.
    QuorumVerdict {
        destination: String,
        source: String,
        guard: GuardExpr,
    },
}

impl JoinPolicy {
    pub fn destination(&self) -> Option<&str> {
        match self {
            Self::CollectInOrder => None,
            Self::ReduceMerge { destination, .. } | Self::QuorumVerdict { destination, .. } => {
                Some(destination.as_str())
            }
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::CollectInOrder => "collect-in-order",
            Self::ReduceMerge { .. } => "reduce-merge",
            Self::QuorumVerdict { .. } => "quorum-verdict",
        }
    }
}

/// Per-branch terminal failure policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum BranchFailurePolicy {
    /// Discard the branch's uncommitted effects and continue the panel with
    /// the remaining branches.
    Skip,
    /// Leave the run in a clean, resumable blocked state. Never escalates.
    Park,
    /// Discard all uncommitted panel effects and route through the panel's
    /// own `on-failure`, or escalate exactly like any other control kind's
    /// unrouted failure. The default for an unlisted branch.
    #[default]
    PanelFail,
}

/// One ordered `branch-failure` entry naming a declared branch and its
/// terminal failure policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct BranchFailureEntry {
    pub branch: String,
    #[serde(rename = "on-failure")]
    pub on_failure: BranchFailurePolicy,
}

// ---------------------------------------------------------------------------
// Command-backed sequence items
// ---------------------------------------------------------------------------

/// Normalized command plan for a command-backed sequence item.
///
/// This is pure declaration/validation data. Core never executes it; IO/CLI
/// adapters may execute the argv only when runtime policy explicitly allows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct CommandPlan {
    /// Explicit program argv. The first item is the executable name/path.
    /// Empty only when `argv-from` supplies the argv at runtime.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,

    /// Local input port whose accepted `[schema:text]` value supplies argv.
    #[serde(default, rename = "argv-from", skip_serializing_if = "Option::is_none")]
    pub argv_from: Option<String>,

    /// Local text slot/port containing the SHA-256 digest the IO host must
    /// verify for the resolved argv executable before spawning it.
    #[serde(
        default,
        rename = "executable-digest-from",
        skip_serializing_if = "Option::is_none"
    )]
    pub executable_digest_from: Option<String>,

    /// Optional working directory policy. MVP supports `project-root` only at
    /// the IO boundary; core stores the declared value as evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Runtime timeout in milliseconds.
    #[serde(
        default,
        rename = "timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,

    /// Runtime idle timeout in milliseconds.
    #[serde(
        default,
        rename = "idle-timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_ms: Option<u64>,

    /// Runtime stdout capture ceiling in bytes. A capture that exceeds this
    /// fails the step rather than landing a truncated value in the slot
    /// ledger (IO/CLI adapters own the actual refusal; core only carries the
    /// declared budget).
    #[serde(
        default,
        rename = "capture-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub capture_bytes: Option<u64>,

    /// Exit codes treated as success. Defaults to `[0]` semantically.
    #[serde(
        default,
        rename = "success-exit-code",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_exit_code: Vec<i32>,
}

/// Advanced command declaration table. The source shorthand `cmd = "..."`
/// remains the primary authoring path; this table carries explicit argv when
/// the author needs to avoid string parsing entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct CommandDeclaration {
    /// Explicit argv. Mutually exclusive with `argv-from`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,

    /// Local `[schema:text]` input port resolved once into concrete argv when
    /// the command frame is built. Slots are deliberately unsupported so an
    /// agent-produced value cannot select the executable.
    #[serde(default, rename = "argv-from", skip_serializing_if = "Option::is_none")]
    pub argv_from: Option<String>,

    #[serde(
        default,
        rename = "executable-digest-from",
        skip_serializing_if = "Option::is_none"
    )]
    pub executable_digest_from: Option<String>,

    /// Optional working directory policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Runtime timeout in milliseconds.
    #[serde(
        default,
        rename = "timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,

    /// Runtime idle timeout in milliseconds.
    #[serde(
        default,
        rename = "idle-timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_ms: Option<u64>,

    /// See [`CommandPlan::capture_bytes`].
    #[serde(
        default,
        rename = "capture-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub capture_bytes: Option<u64>,

    /// Exit codes treated as success. Defaults to `[0]` semantically.
    #[serde(
        default,
        rename = "success-exit-code",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_exit_code: Vec<i32>,
}

// ---------------------------------------------------------------------------
// Model body
// ---------------------------------------------------------------------------

/// Sequence item kind. Omitted kind infers prompt or command from authored fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SequenceKind {
    Prompt,
    /// A human-owned prompt frame resumed through the ordinary output path.
    Ask,
    Command,
    Check,
    /// Deterministic, runtime-owned slot projection. It never creates an agent
    /// or command frame.
    Project,
    Sequence,
    Branch,
    Loop,
    ForEach,
    Parallel,
}

/// One `procedure.sequence[].projection[].source`: either an existing local
/// slot ref, or a canonical typed literal wrapped as `{ literal = <value> }`.
/// The wrapper keeps the literal form unambiguous with the plain string
/// slot-ref shape — an unwrapped string literal would be indistinguishable
/// from `source = "slot:..."`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum ProjectionSource {
    Slot(String),
    Literal {
        literal: JsonValue,
    },
}

impl<'de> Deserialize<'de> for ProjectionSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum RawProjectionSource {
            Slot(String),
            Literal { literal: JsonValue },
        }

        match RawProjectionSource::deserialize(deserializer)? {
            RawProjectionSource::Slot(slot) => Ok(Self::Slot(slot)),
            RawProjectionSource::Literal { literal } => Ok(Self::Literal { literal }),
        }
    }
}

impl ProjectionSource {
    pub fn as_slot_ref(&self) -> Option<&str> {
        match self {
            Self::Slot(slot) => Some(slot),
            Self::Literal { .. } => None,
        }
    }

    pub fn as_literal(&self) -> Option<&JsonValue> {
        match self {
            Self::Slot(_) => None,
            Self::Literal { literal } => Some(literal),
        }
    }
}

/// One ordered write in a closed `project` sequence item.
///
/// Runtime reads every slot source from one pre-step snapshot, optionally
/// selects one top-level object field, validates every destination write,
/// then commits all writes atomically. A literal source (P431) is a canonical
/// value declared on the projection itself rather than read from a slot;
/// literal-backed projections contribute no sequence input and reject
/// `field`. Destinations are local typed slots; `replace`, `append`, and
/// `increment` are the supported operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Projection {
    pub source: ProjectionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub destination: String,
    #[serde(default, skip_serializing_if = "is_replace_operation")]
    pub operation: WriteOperation,
}

/// A control failure either emits a legacy signal or abandons the current path
/// and schedules a later top-level recovery step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FailureTarget {
    Signal(String),
    Route(FailureRoute),
}

impl FailureTarget {
    pub fn signal_ref(&self) -> Option<&str> {
        match self {
            Self::Signal(signal) => Some(signal),
            Self::Route(route) => route.signal.as_deref(),
        }
    }

    pub fn route(&self) -> Option<&FailureRoute> {
        match self {
            Self::Signal(_) => None,
            Self::Route(route) => Some(route),
        }
    }
}

/// A loop's `on-exhausted` declaration: the keywords `"continue"`/`"block"`,
/// or one or more signal refs emitted when the loop continues past
/// exhaustion. A one-element sequence collapses to `One` on decode so each
/// meaning has exactly one canonical spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ExhaustionTarget {
    One(String),
    Many(Vec<String>),
}

/// Disposition an `ExhaustionTarget` resolves to: stop the run blocked, or
/// continue and emit the given signals (possibly none).
pub enum ExhaustionDisposition<'a> {
    Block,
    Continue { signals: &'a [String] },
}

impl ExhaustionTarget {
    /// Classify this declaration once so validation, runtime, and the ledger
    /// contract share a single keyword/signal split.
    pub fn disposition(&self) -> ExhaustionDisposition<'_> {
        match self {
            Self::One(value) if value == "block" => ExhaustionDisposition::Block,
            Self::One(value) if value == "continue" => ExhaustionDisposition::Continue { signals: &[] },
            Self::One(signal) => ExhaustionDisposition::Continue {
                signals: std::slice::from_ref(signal),
            },
            Self::Many(signals) => ExhaustionDisposition::Continue { signals },
        }
    }

    /// The declared signal refs this target names, regardless of
    /// disposition: empty for `Block` (and for the bare `"continue"`
    /// keyword), the named signal(s) otherwise. The single accessor every
    /// consumer that only wants "which signals does this declaration name"
    /// (never the block/continue distinction itself) should call, rather
    /// than re-matching on `disposition()`.
    pub fn signals(&self) -> &[String] {
        match self.disposition() {
            ExhaustionDisposition::Block => &[],
            ExhaustionDisposition::Continue { signals } => signals,
        }
    }
}

impl<'de> Deserialize<'de> for ExhaustionTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let items = crate::shared::deserialize_string_list(deserializer)?;
        let mut items = items.into_iter();
        let Some(first) = items.next() else {
            return Ok(Self::Many(Vec::new()));
        };
        let Some(second) = items.next() else {
            return Ok(Self::One(first));
        };
        let mut rest = vec![first, second];
        rest.extend(items);
        Ok(Self::Many(rest))
    }
}

/// Closed recovery-route declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct FailureRoute {
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// A `[[procedure.sequence]]` item: one item in the procedure, owned by the
/// procedure rather than by a separate top-level table.
///
/// `[[procedure.sequence]]` definition order is the default execution order.
/// An optional `id` allows `procedure.sequence-order` to customize ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceItem {
    /// Optional stable identifier for sequence-order references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Human-readable title for this sequence step.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,

    /// Explicit item kind. When omitted, `prompt`, `cmd`, and `command` fields
    /// provide the current authoring sugar for prompt/command inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SequenceKind>,

    /// Optional abstract agent role that should serve this executable item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,

    /// Prompt ref (`prompt:<id>`) or inline prompt text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,

    /// Simple no-shell command shorthand for a command-backed sequence item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,

    /// Explicit command declaration. `cmd` and `command` are mutually
    /// exclusive source forms and lower to the same command plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandDeclaration>,

    /// Ordered deterministic writes for `kind = "project"`. All source values
    /// are read before any destination is committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projection: Vec<Projection>,

    /// Runtime timeout in milliseconds for simple `cmd` shorthand.
    #[serde(
        default,
        rename = "timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,

    /// Runtime idle timeout in milliseconds, for simple `cmd` shorthand.
    #[serde(
        default,
        rename = "idle-timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_ms: Option<u64>,

    /// See [`CommandPlan::capture_bytes`], for simple `cmd` shorthand.
    #[serde(
        default,
        rename = "capture-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub capture_bytes: Option<u64>,

    /// Exit codes treated as success for simple `cmd` shorthand.
    #[serde(
        default,
        rename = "success-exit-code",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_exit_code: Vec<i32>,

    /// Input refs (e.g. `port:user-prompt`, `slot:scope`, `resource:data`).
    /// A resource entry may additionally be authored as a guarded table
    /// (`{ ref = "resource:<id>", when = <GuardExpr> }`); the runtime
    /// includes it in this step's frame only when the guard matches.
    #[serde(default, skip_serializing_if = "SequenceInputList::is_empty")]
    pub input: SequenceInputList,

    /// Output sinks (e.g. `port:finding`, `slot:result`, `schema:verdict`, or
    /// `{ slot = "slot:notes", operation = "append" }`).
    #[serde(default, skip_serializing_if = "OutputSinkList::is_empty")]
    pub output: OutputSinkList,

    /// Optional format preferences. Accepted as a format-preference declaration on prompt/command items; rejected on control items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<crate::shared::SlugList>,

    /// Declared signal refs this item may emit, either directly as refs or
    /// derived from deterministic output predicates.
    #[serde(default, rename = "emits", skip_serializing_if = "Vec::is_empty")]
    pub emits: Vec<SignalEmissionRule>,

    /// Named sequence ref (`sequence:<id>`) for sequence/loop/for-each items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<String>,

    /// Branch entry guard. A true guard selects `sequence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<GuardExpr>,

    /// Optional named sequence ref selected when a branch guard is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otherwise: Option<String>,

    /// Loop success guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<GuardExpr>,

    /// Loop terminal guard.
    #[serde(default, rename = "stop-if", skip_serializing_if = "Option::is_none")]
    pub stop_if: Option<GuardExpr>,

    /// Required loop bound.
    #[serde(
        default,
        rename = "max-iterations",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_iterations: Option<usize>,

    /// Local `schema:integer` input port resolved once at loop entry and frozen
    /// into the runtime control frame. Mutually exclusive with
    /// `max-iterations`.
    #[serde(
        default,
        rename = "max-iterations-from",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_iterations_from: Option<String>,

    /// Loop exhaustion policy: `"continue"` (the default when omitted),
    /// `"block"`, or one or more `signal:<id>` refs.
    ///
    /// Continuing treats exhaustion as a normal outcome — the sequence
    /// proceeds to the next item, and any declared signals are emitted as
    /// recorded evidence a later guard may read (or ignore). Spending a
    /// bounded budget without matching `until` means the work is unfinished,
    /// which the following step is responsible for reading; it does not mean
    /// the run is broken.
    ///
    /// `"block"` stops the run at exhaustion, for procedures where an unmet
    /// exit condition invalidates every step after the loop; no signal is
    /// emitted in that case. A run started with strict loops blocks every
    /// loop regardless of its declared policy, and suppresses signal
    /// emission even when the declaration names one.
    #[serde(
        default,
        rename = "on-exhausted",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_exhausted: Option<ExhaustionTarget>,

    /// Names the signal(s) to emit when `stop-if` is the arm that halts the
    /// loop, so the trait's own terminal reason (e.g.
    /// `recurring-blocker-unresolved`) is distinguishable from exhaustion in
    /// the ledger, the receipt, and `run-status` — the runtime still reports
    /// the accurate mechanism (`stop-if-matched`) alongside it. Requires
    /// `stop-if`; the `"continue"`/`"block"` keywords are meaningless here
    /// (a `stop-if` match always halts the loop) and are rejected.
    #[serde(default, rename = "on-stop", skip_serializing_if = "Option::is_none")]
    pub on_stop: Option<ExhaustionTarget>,

    /// `for-each` list slot ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,

    /// `for-each` per-item slot ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,

    /// Required `for-each` bound.
    #[serde(default, rename = "max-items", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,

    /// Runtime signal emitted when a for-each completes.
    #[serde(
        default,
        rename = "on-complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_complete: Option<String>,

    /// Legacy failure signal or structured forward recovery route.
    #[serde(
        default,
        rename = "on-failure",
        skip_serializing_if = "Option::is_none"
    )]
    pub on_failure: Option<FailureTarget>,

    /// Ordered, author-order list of local `sequence:<id>` refs run as
    /// independent branches of a `kind = "parallel"` item. Never sorted.
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub branches: RefList,

    /// Required positive upper bound on `branches` length for a `parallel`
    /// item. `branches.len()` must not exceed this bound.
    #[serde(
        default,
        rename = "max-branches",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_branches: Option<usize>,

    /// `for-each` concurrency request. Additive: `false`/absent is byte-identical
    /// to documents authored before concurrency existed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub concurrent: bool,

    /// `parallel` barrier join policy. Omitted is behaviorally identical to
    /// `{ policy = "collect-in-order" }` (P263 barrier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinPolicy>,

    /// `parallel` ordered per-branch terminal failure policy. A branch with
    /// no entry defaults to `panel-fail`.
    #[serde(
        default,
        rename = "branch-failure",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub branch_failure: Vec<BranchFailureEntry>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl SequenceItem {
    /// Effective kind after applying prompt/command authoring inference.
    pub fn effective_kind(&self) -> SequenceKind {
        if let Some(kind) = self.kind {
            return kind;
        }
        if self.cmd.is_some() || self.command.is_some() {
            SequenceKind::Command
        } else {
            SequenceKind::Prompt
        }
    }
}

/// A `[procedure]` section: deterministic sequence orchestration.
///
/// Optional top-level section. When present, `description` must be non-empty
/// and `sequence` must be non-empty. The `input` and `output` contract fields
/// declare procedure-level boundary ports. `sequence` holds inline
/// `[[procedure.sequence]]` items; `sequence-order` optionally reorders them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case", rename = "Procedure")]
pub struct Model {
    /// Human-readable description of what the procedure accomplishes.
    pub description: String,

    /// Require prepared worktree provenance before the procedure can expose
    /// its first prompt or command frame.
    #[serde(default, skip_serializing_if = "is_false")]
    pub worktree_required: bool,

    /// Model-level input refs (e.g. `port:user-prompt`, `slot:scope`).
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub input: RefList,

    /// Model-level output refs (e.g. `port:finding`, `slot:result`).
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub output: RefList,

    /// Ordered `[[procedure.sequence]]` items. Definition order is the default
    /// execution order unless `sequence-order` is present.
    #[serde(default, rename = "sequence", skip_serializing_if = "Vec::is_empty")]
    pub sequence: Vec<SequenceItem>,

    /// Optional explicit ordering of sequence item IDs. If present, every
    /// sequence item must have a unique ID, every listed ID must exist,
    /// duplicate IDs are invalid, and unlisted items are invalid.
    #[serde(
        default,
        rename = "sequence-order",
        skip_serializing_if = "Option::is_none"
    )]
    pub sequence_order: Option<Vec<String>>,
}
