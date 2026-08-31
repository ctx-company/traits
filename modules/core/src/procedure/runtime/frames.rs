// Procedure runtime frame definitions.
// Defines frames exposed by the procedure runtime.
// Procedure runtime frame definitions.

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Kind of frame returned to a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SequenceFrameKind {
    Intro,
    Step,
    Ask,
    Command,
    Check,
}

/// Command execution plan disclosed to CLI/MCP adapters for a current command
/// frame. Core produces this evidence but never executes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CommandFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    pub argv: Vec<String>,
    #[serde(
        default,
        rename = "executable-digest",
        skip_serializing_if = "Option::is_none"
    )]
    pub executable_digest: Option<Digest>,
    /// Authored positions in `argv` that are a whole `{resource:<id>}`
    /// token, derived only from literal command-item argv (never from
    /// `argv-from` or a substituted slot/port value). IO resolves each
    /// position through the protected-resource verifier immediately before
    /// spawn and replaces only that element with the verified path, while
    /// `argv` above stays the logical text persisted in frame/evidence
    /// equality.
    #[serde(
        default,
        rename = "resource-argv",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub resource_argv: Vec<ResourceArgvRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(
        default,
        rename = "timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
    #[serde(
        default,
        rename = "idle-timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_ms: Option<u64>,
    /// See [`crate::r#trait::procedure::CommandPlan::capture_bytes`].
    #[serde(
        default,
        rename = "capture-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub capture_bytes: Option<u64>,
    #[serde(
        default,
        rename = "success-exit-code",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_exit_code: Vec<i32>,
    pub output_slot: String,
    /// Extra declared output refs beyond `output_slot` — populated only for
    /// a check with two output sinks ([slot, port]; task 0206). `output_slot`
    /// keeps the first sink's ref text unchanged for byte-compat; this field
    /// is additive and empty for every other command/check shape, so it
    /// never appears in a pre-0206 ledger.
    #[serde(
        default,
        rename = "additional-output-refs",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub additional_output_refs: Vec<String>,
    pub permission_code: String,
    pub reason: String,
}

impl CommandFrame {
    /// Every declared output ref this frame writes to, `output_slot` first.
    pub fn output_refs(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.output_slot.as_str())
            .chain(self.additional_output_refs.iter().map(String::as_str))
    }
}

/// One authored `{resource:<id>}` argv position: the zero-based `argv`
/// index it occupies and the whole local resource ref text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ResourceArgvRef {
    pub index: usize,
    #[serde(rename = "resource-ref")]
    pub resource_ref: String,
}

/// Prompt resolution evidence for a frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PromptEvidence {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_ref: Option<Reference>,
    pub digest: Digest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interpolations: Vec<String>,
}

/// One available input exposed to a step frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrameInput {
    pub ref_text: String,
    pub value_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    pub source: ValueSource,
    /// Bounded producer evidence carried over from the accepted value (for
    /// example a check's captured stdout/stderr/exit-code report), so a
    /// downstream step reading this input can inspect why it has this value
    /// without the evidence being part of the value itself.
    #[serde(
        default,
        rename = "producer-evidence",
        skip_serializing_if = "Option::is_none"
    )]
    pub producer_evidence: Option<String>,
}

/// One requested output for a step frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrameOutputRequest {
    pub slot_ref: Reference,
    #[serde(default, skip_serializing_if = "is_replace_write_operation")]
    pub operation: WriteOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    /// True when this output sink is declared optional (P105): the agent may
    /// leave it unfilled without the step being rejected. Absent/false marks
    /// an ordinary required output, unchanged from every prior frame.
    #[serde(default, skip_serializing_if = "is_false_flag")]
    pub optional: bool,
}

fn is_false_flag(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrameDerivedSignal {
    pub signal_ref: Reference,
    pub when: GuardExpr,
}

/// Abstract agent role assigned to a current frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AgentRole {
    pub role: String,
    pub ref_text: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Standing instructions for this role. Deliberately absent from the
    /// frame text: the dispatcher sends it through the harness system channel,
    /// and repeating it in the frame's user message would defeat that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Zero-based declaration-order ordinal of this authored `agent:<role>`
    /// site among every reachable site assigned to the same role (P456).
    /// Loop rounds, retries, and for-each iterations of the same authored
    /// site share one ordinal; a config-layer role list resolves the seat as
    /// `entries[ordinal % entries.len()]`. `None` only if the trait declares
    /// no procedure (never for a real assigned agent). Never serialized:
    /// whether a role is list-backed is config knowledge this pure-core type
    /// does not have, and every consumer reads this field in-process off the
    /// freshly computed frame rather than off a persisted/deserialized one,
    /// so wire output (and single-table byte-for-byte compatibility) is
    /// unaffected either way.
    #[serde(skip)]
    #[schemars(skip)]
    pub structural_seat: Option<u32>,
}

/// Caller-facing template for the full run-call envelope expected for a frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceCallTemplate {
    pub session_id: String,
    pub run_id: String,
    pub state_digest: Digest,
    pub expected_run_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_source_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sequence_item_id: Option<String>,
    #[serde(
        default,
        rename = "expected-position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub expected_position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub produced_slots: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, SequenceSignalTemplate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(
        default,
        rename = "required-agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub required_agent: Option<AgentRole>,
    pub caller: SequenceCallerTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceSignalTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Schema required for a caller-authored payload, when this signal carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceCallerTemplate {
    pub surface: String,
    pub caller: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

/// Bounded frame data for intro/procedure or a concrete step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SequenceFrame {
    pub kind: SequenceFrameKind,
    pub run_id: String,
    pub trait_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(
        default,
        rename = "loop-context",
        skip_serializing_if = "Option::is_none"
    )]
    pub loop_context: Option<LoopContext>,
    #[serde(
        default,
        rename = "for-each-context",
        skip_serializing_if = "Option::is_none"
    )]
    pub for_each_context: Option<ForEachContext>,
    #[serde(
        default,
        rename = "guard-explanations",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub guard_explanations: Vec<ConditionEvaluation>,
    #[serde(default, rename = "signal-payloads", skip_serializing_if = "Vec::is_empty")]
    pub signal_payloads: Vec<FrameSignalPayload>,
    /// The highest signal `emission_order` visible when this frame's
    /// `signal_payloads`/argv were resolved — the same scan, not a second
    /// one. A command activation's evidence stamps this value verbatim as
    /// `CommandExecutionEvidence::signal_emission_ceiling`, so a later
    /// re-emission of the same signal can never retroactively change what an
    /// already-built frame's provenance replay sees.
    #[serde(default, rename = "signal-emission-ceiling", skip_serializing_if = "is_zero")]
    pub signal_emission_ceiling: usize,
    pub title: String,
    pub frame_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandFrame>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_inputs: Vec<FrameInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_evidence: Vec<ResourceEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_outputs: Vec<FrameOutputRequest>,
    #[serde(
        default,
        rename = "assigned-agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub assigned_agent: Option<AgentRole>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_signals: Vec<String>,
    #[serde(
        default,
        rename = "derived-signals",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub derived_signals: Vec<FrameDerivedSignal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_template: Option<SequenceCallTemplate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// An accepted signal payload visible at this frame's control position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrameSignalPayload {
    pub signal_ref: Reference,
    pub payload: JsonValue,
}

/// Look up a visible signal payload or one of its dotted object fields in a
/// list of ordered payloads. `payloads` must be in emission order (oldest
/// first); this takes the LAST matching entry so a signal raised more than
/// once resolves to its most recent payload, not append/sort-order luck.
///
/// The single collection-level lookup for signal payload resolution: both
/// [`SequenceFrame::signal_payload_field`] (the prompt path) and the
/// `Kind::Signal` argv-interpolation arm (`frame_builders.rs`) delegate here
/// instead of each independently reimplementing reverse matching and dotted
/// field traversal, so recency/matching semantics can never drift between
/// the two surfaces.
pub(crate) fn signal_payload_field<'a>(
    payloads: &'a [FrameSignalPayload],
    signal_ref: &str,
    field: Option<&str>,
) -> Option<&'a JsonValue> {
    let payload = payloads
        .iter()
        .rev()
        .find(|payload| payload.signal_ref.as_str() == signal_ref)
        .map(|payload| &payload.payload)?;
    field.map_or(Some(payload), |field| crate::shared::resolve_field_path(payload, field))
}

impl SequenceFrame {
    /// Look up a visible signal payload or one of its dotted object fields.
    /// See [`signal_payload_field`] (the free function) for the shared
    /// lookup this delegates to.
    pub fn signal_payload_field(&self, signal_ref: &str, field: Option<&str>) -> Option<&JsonValue> {
        signal_payload_field(&self.signal_payloads, signal_ref, field)
    }
}

#[cfg(test)]
mod signal_payload_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signal_payload_field_resolves_nested_fields_without_coercion() {
        let frame = SequenceFrame {
            kind: SequenceFrameKind::Step,
            run_id: "run".to_string(),
            trait_id: "trait".to_string(),
            sequence_index: Some(0),
            run_index: Some(0),
            item_id: Some("step".to_string()),
            position_path: Vec::new(),
            loop_context: None,
            for_each_context: None,
            guard_explanations: Vec::new(),
            signal_payloads: vec![FrameSignalPayload {
                signal_ref: Reference::parse("signal:review").unwrap(),
                payload: json!({"detail": {"code": "missing-test"}}),
            }],
            signal_emission_ceiling: 0,
            title: "step".to_string(),
            frame_text: String::new(),
            prompt: None,
            command: None,
            available_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            requested_outputs: Vec::new(),
            assigned_agent: None,
            allowed_signals: Vec::new(),
            derived_signals: Vec::new(),
            call_template: None,
            warnings: Vec::new(),
        };

        assert_eq!(frame.signal_payload_field("signal:review", Some("detail.code")), Some(&json!("missing-test")));
        assert_eq!(frame.signal_payload_field("signal:review", Some("detail.missing")), None);
        assert_eq!(frame.signal_payload_field("signal:review", Some("detail.code.value")), None);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct LoopContext {
    pub loop_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    pub iteration_index: usize,
    pub max_iterations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ForEachContext {
    pub for_each_id: String,
    pub item_index: usize,
    pub item_total: usize,
    pub max_items: usize,
}

/// Result of asking for the next frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum NextSequenceFrameResult {
    Frame(Box<SequenceFrame>),
    Blocked {
        missing_inputs: Vec<String>,
        capabilities: Vec<CapabilityReport>,
    },
    Completed,
    Rejected,
    Failed,
}

// ---------------------------------------------------------------------------
// Output envelopes and reports
// ---------------------------------------------------------------------------

/// Caller/model-supplied slot value inside a step-output envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StepSlotOutput {
    #[serde(rename = "ref")]
    pub ref_text: String,
    pub value: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ValueSource>,
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
}

/// Caller/model-supplied signal emission inside a step-output envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StepSignalOutput {
    #[serde(rename = "ref")]
    pub ref_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonValue>,
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
}

/// Caller/model-supplied JSON response shape for one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StepOutputEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(
        default,
        rename = "produced-slots",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub produced_slots: Vec<StepSlotOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<StepSignalOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Validated runtime signal emission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SignalEmission {
    pub signal_ref: Reference,
    /// Monotonic emission ordinal, assigned in [`record_emitted_signals`]
    /// (control_flow.rs) from a max-existing+1 scan across the committed
    /// ledger and any open parallel-branch buffers. This is the recency
    /// signal for [`SequenceFrame::signal_payload_field`]: `sort_state`
    /// (frame_builders.rs) reorders committed emissions by sequence
    /// index/ref/digest for display, which destroys append order, so
    /// `.last()` on the stored `Vec` is not recency — this field is.
    /// `#[serde(default)]` so pre-existing ledgers deserialize to `0`; ties
    /// degrade to whatever order the vec already carries, which is
    /// acceptable since no ledger predates this field with more than one
    /// emission of the same signal in a way that recency-mattered.
    #[serde(default, rename = "emission-order")]
    pub emission_order: usize,
    pub sequence_index: usize,
    pub evidence_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonValue>,
    #[serde(default, rename = "payload-digest", skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<Digest>,
    #[serde(default, rename = "schema-validation", skip_serializing_if = "Vec::is_empty")]
    pub schema_validation: Vec<SchemaValidation>,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SignalSource>,
    #[serde(
        default,
        rename = "runtime-control",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_control: Option<ControlEmissionIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
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
    pub acceptance: AcceptanceStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SignalSource {
    RuntimeControl,
}

/// Rejected runtime attempt evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RejectedAttempt {
    pub sequence_index: usize,
    #[serde(
        default,
        rename = "position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_digest: Option<Digest>,
    pub reason: String,
}

/// Next action after step-output validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum StepNextAction {
    Continue,
    AwaitingStepOutput,
    Blocked,
    Completed,
    Rejected,
    Failed,
}

/// Step-output validation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StepValidationReport {
    pub sequence_index: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_outputs: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_outputs: Vec<RejectedAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_required_outputs: Vec<String>,
    /// Declared optional output sinks (P105) left unfilled at step completion.
    /// A signed non-failure, distinct from `missing_required_outputs`: it
    /// never contributes to `rejected` and the step completes normally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unfilled_optional_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexpected_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_validation: Vec<SchemaValidation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signal_validation: Vec<SignalEmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub next_action: StepNextAction,
}

/// Contract-validation status for a supplied runtime ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum LedgerContractStatus {
    ValidCompleted,
    ValidRunning,
    ValidBlocked,
    ValidRejected,
    ValidFailed,
    Invalid,
}

/// Pure validation report for a supplied runtime ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct LedgerValidationReport {
    pub status: LedgerContractStatus,
    pub contract_valid: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}
