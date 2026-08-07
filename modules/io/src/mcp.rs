//! Thin MCP runtime/session adapter contracts.
//!
//! This module models the MCP tool boundary over the same core run-session
//! transitions used by the CLI JSON surface. It does not implement a transport
//! server; host glue can expose these functions as tools without duplicating
//! runtime validation.

use std::sync::OnceLock;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ctx_traits_core::response::{CapabilityReport, Envelope, ResponseError};

static ELAPSED_BASELINE_AND_START: OnceLock<(u64, Instant)> = OnceLock::new();

/// Initialize this MCP server process's elapsed-evidence baseline exactly
/// once, at startup: `baseline_seconds` is the cumulative active-drive
/// elapsed time the CLI drive loop already measured before spawning this
/// subprocess. The caller (`serve_stdio`) invokes this only when a trusted
/// parent drive actually supplied a baseline — a bare or persistently
/// hosted `ctx traits mcp` never calls this at all, so it never starts the
/// clock, rather than defaulting to a baseline of `0` (which would make a
/// long-lived bare host silently accrue its own idle process age as
/// elapsed evidence). Every tool call accepted by this process for the
/// rest of its lifetime then supplies `baseline_seconds + <time since this
/// call>` as its own elapsed evidence — unifying host-owned elapsed
/// observation across the CLI and MCP transports at the same accept
/// transition, while the core runtime still never reads a clock itself.
pub fn initialize_elapsed_baseline(baseline_seconds: u64) {
    let _ = ELAPSED_BASELINE_AND_START.set((baseline_seconds, Instant::now()));
}

/// Current elapsed-evidence reading for this process, or `None` if
/// [`initialize_elapsed_baseline`] was never called — a bare/persistent
/// host with no trusted baseline (see there) accrues no time for any
/// session it serves, rather than fabricating one.
fn current_elapsed_seconds() -> Option<u64> {
    ELAPSED_BASELINE_AND_START
        .get()
        .map(|(baseline, start)| baseline + start.elapsed().as_secs())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "trait")]
    pub r#trait: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trait_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default = "empty_json_object")]
    pub inputs: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    #[serde(
        default,
        rename = "agent-assignments",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_assignments: Option<Vec<ctx_traits_core::procedure::session::AgentAssignment>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct InfoRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "trait")]
    pub r#trait: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CallRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    pub submission: ctx_traits_core::procedure::session::CallSubmission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct InspectRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SetRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    pub target: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct NextRequest {
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    #[serde(default)]
    pub wait_seconds: u64,
    #[serde(default)]
    pub peek: bool,
}

pub fn ctx_traits_run_start(
    request: StartRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::Session>> {
    Ok(match ctx_traits_run_start_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, false),
    })
}

pub fn ctx_traits_run_info(request: InfoRequest) -> crate::Result<Envelope<Value>> {
    Ok(match ctx_traits_run_info_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, false),
    })
}

fn ctx_traits_run_info_inner(request: InfoRequest) -> crate::Result<Envelope<Value>> {
    let trait_id = request.trait_id.as_deref().or(request.r#trait.as_deref());
    if request.trait_file.is_none() && trait_id.is_none() {
        let query = request.query.as_deref().ok_or_else(|| {
            invalid_mcp_request(
                "mcp.run-info.query",
                "MCP run-info requires trait-file, trait-id, trait, or query",
            )
        })?;
        return run_info_outcome_envelope(crate::run::run_info(None, None, Some(query))?);
    }
    run_info_outcome_envelope(crate::run::run_info(
        request.trait_file.as_deref(),
        trait_id,
        None,
    )?)
}

fn run_info_outcome_envelope(
    outcome: crate::run::RunInfoOutcome,
) -> crate::Result<Envelope<Value>> {
    match outcome {
        crate::run::RunInfoOutcome::Summary { summary, .. } => {
            Ok(mcp_envelope(to_json_value(summary)?, false, false, false))
        }
        crate::run::RunInfoOutcome::Selection(output) => {
            Ok(mcp_envelope(to_json_value(output)?, false, false, false))
        }
    }
}

fn ctx_traits_run_start_inner(
    request: StartRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::Session>> {
    let trait_id = request.trait_id.as_deref().or(request.r#trait.as_deref());
    let mut initial_values =
        ctx_traits_core::procedure::session::run_initial_values_from_json(request.inputs)?;
    initial_values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    let outcome = crate::run::start(crate::run::StartRequest {
        strict_loops: false,
        override_dependencies: false,
        task_dispatch: false,
        defer_commands: false,
        trait_file: request.trait_file.as_deref(),
        trait_id,
        query: request.query.as_deref(),
        trait_args: &request.trait_args,
        input_values: initial_values,
        out: request.out.as_deref(),
        session_store: request.session_store.as_deref(),
        ephemeral: false,
        resource_evidence: crate::run::ResourceEvidenceMode::Unavailable {
            reason: "mcp start did not include resource evidence",
        },
        assign_overrides: &[],
        agent_assignments: request.agent_assignments,
        provider_capability_reports: Vec::new(),
        provider_warnings: Vec::new(),
        harness_probes: Vec::new(),
        caller: ctx_traits_core::procedure::session::CallerProvenance::mcp(),
        state_source: "mcp-request",
        trait_arg_evidence: "ctx_traits_run_start trait_args",
        worktree: None,
        merge_rung: None,
        // An MCP host reads structured responses; stderr narration is noise.
        narrate_progress: false,
        startup_observer: None,
    })?;
    Ok(mcp_envelope(
        outcome.session,
        true,
        false,
        outcome.resource_supported,
    ))
}

pub fn ctx_traits_run_call(
    request: CallRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::CallResponse>> {
    Ok(match ctx_traits_run_call_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, true),
    })
}

fn ctx_traits_run_call_inner(
    mut request: CallRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::CallResponse>> {
    if request.submission.command_execution.is_some() {
        return Ok(unsupported_envelope(
            "runtime.command-execution",
            "MCP callers must not supply command-execution evidence; the trusted local runtime executes command frames",
            true,
        ));
    }
    request.submission.command_execution = None;
    if request.submission.caller.is_none() {
        request.submission.caller =
            Some(ctx_traits_core::procedure::session::CallerProvenance::mcp());
    }
    let outcome = crate::run::call(crate::run::CallRequest {
        trait_file: request.trait_file.as_deref(),
        trait_id: request.trait_id.as_deref(),
        session: &request.session,
        session_store: request.session_store.as_deref(),
        submission: request.submission,
        out: request.out.as_deref(),
        execution_dir: None,
        // Host-side call surface: no worktree exec_dir, so no overlay is
        // applied here (a worktree-scoped MCP subprocess already inherits the
        // overlay from the parent that spawned it).
        execution_env: &std::collections::BTreeMap::new(),
        elapsed_seconds: current_elapsed_seconds(),
        tick_observer: None,
    })?;
    Ok(mcp_envelope(
        outcome.response,
        true,
        true,
        outcome.resource_supported,
    ))
}

pub fn ctx_traits_run_set(request: SetRequest) -> crate::Result<Envelope<Value>> {
    Ok(match ctx_traits_run_set_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, true),
    })
}

fn ctx_traits_run_set_inner(request: SetRequest) -> crate::Result<Envelope<Value>> {
    let mut caller = ctx_traits_core::procedure::session::CallerProvenance::mcp();
    caller.agent = request.agent;
    caller.harness = request.harness;
    let outcome = crate::run::set(crate::run::SetRequest {
        trait_file: request.trait_file.as_deref(),
        trait_id: request.trait_id.as_deref(),
        session: &request.session,
        session_store: request.session_store.as_deref(),
        target: &request.target,
        value: request.value,
        out: request.out.as_deref(),
        caller,
        existing_input_evidence: "existing mcp run-session input",
    })?;
    match outcome {
        crate::run::SetOutcome::Session {
            session,
            resource_supported,
        } => Ok(mcp_envelope(
            to_json_value(session)?,
            true,
            true,
            resource_supported,
        )),
        crate::run::SetOutcome::Call {
            response,
            resource_supported,
        } => Ok(mcp_envelope(
            to_json_value(response)?,
            true,
            true,
            resource_supported,
        )),
    }
}

pub fn ctx_traits_run_status(
    request: InspectRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::Session>> {
    Ok(match ctx_traits_run_status_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, false),
    })
}

fn ctx_traits_run_status_inner(
    request: InspectRequest,
) -> crate::Result<Envelope<ctx_traits_core::procedure::session::Session>> {
    let outcome = crate::run::status(crate::run::InspectRequest {
        trait_file: request.trait_file.as_deref(),
        trait_id: request.trait_id.as_deref(),
        session: &request.session,
        session_store: request.session_store.as_deref(),
        elapsed_seconds: current_elapsed_seconds(),
    })?;
    Ok(mcp_envelope(
        outcome.session,
        false,
        false,
        outcome.resource_supported,
    ))
}

pub fn ctx_traits_run_frame(
    request: InspectRequest,
) -> crate::Result<Envelope<Option<Box<ctx_traits_core::procedure::runtime::SequenceFrame>>>> {
    Ok(match ctx_traits_run_frame_inner(request) {
        Ok(envelope) => envelope,
        Err(error) => mcp_error_envelope(&error, false),
    })
}

fn ctx_traits_run_frame_inner(
    request: InspectRequest,
) -> crate::Result<Envelope<Option<Box<ctx_traits_core::procedure::runtime::SequenceFrame>>>> {
    let status = ctx_traits_run_status_inner(request)?;
    let Some(session) = status.value else {
        return Ok(Envelope {
            schema_version: status.schema_version,
            ok: false,
            value: None,
            error: status.error,
            warnings: status.warnings,
            capabilities: status.capabilities,
        });
    };
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &session.resource_evidence,
        );
    Ok(mcp_envelope(
        session.next_frame,
        false,
        false,
        resource_supported,
    ))
}

pub fn ctx_traits_run_next(
    request: NextRequest,
) -> crate::Result<Envelope<crate::run_queue::RunNextOutput>> {
    Ok(
        match crate::run_queue::next(crate::run_queue::NextRequest {
            agent: Some(&request.agent),
            session: request.session.as_deref(),
            session_store: request.session_store.as_deref(),
            wait_seconds: request.wait_seconds,
            peek: request.peek,
        }) {
            Ok(output) => mcp_envelope(output, false, true, false),
            Err(error) => mcp_error_envelope(&error, true),
        },
    )
}

pub fn ctx_traits_mcp_resources() -> Envelope<Value> {
    unsupported_envelope(
        "runtime.mcp-resources",
        "read-only MCP trait/run resources are not implemented by this thin adapter; use ctx_traits_run_frame and ctx_traits_run_status tools",
        false,
    )
}

pub fn ctx_traits_mcp_prompts() -> Envelope<Value> {
    mcp_envelope(mcp_prompt_templates(), false, false, false)
}

pub fn ctx_traits_mcp_tool_descriptions() -> Envelope<Value> {
    mcp_envelope(mcp_tool_descriptions(), false, false, false)
}

fn invalid_mcp_request(field_path: &str, message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: message.into(),
        }
        .into(),
    )
}

fn to_json_value<T: Serialize>(value: T) -> crate::Result<Value> {
    Ok(
        serde_json::to_value(value).map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize MCP envelope value".to_string(),
            source,
        })?,
    )
}

fn mcp_envelope<T>(
    value: T,
    run_session_persistence: bool,
    call_payload: bool,
    declared_resource_evidence: bool,
) -> Envelope<T> {
    let mut envelope = Envelope::ok(value);
    for capability in ctx_traits_core::procedure::session::run_session_capability_reports(
        true,
        run_session_persistence,
        call_payload,
        declared_resource_evidence,
        true,
        false,
        false,
    ) {
        envelope = envelope.with_capability(capability);
    }
    envelope = envelope.with_capability(CapabilityReport::unsupported(
        "runtime.mcp-resources",
        "read-only MCP trait/run resources are not implemented by this thin adapter",
    ));
    envelope = envelope.with_capability(CapabilityReport::supported("runtime.mcp-prompts"));
    with_runtime_claim_gate(envelope)
}

fn unsupported_envelope<T>(capability: &str, message: &str, call_payload: bool) -> Envelope<T> {
    let mut envelope = Envelope::err_response(
        ResponseError::new("unsupported-capability", message).with_detail("capability", capability),
    );
    for report in ctx_traits_core::procedure::session::run_session_capability_reports(
        true,
        false,
        call_payload,
        false,
        true,
        false,
        false,
    ) {
        envelope = envelope.with_capability(report);
    }
    envelope = envelope.with_capability(CapabilityReport::unsupported(
        "runtime.mcp-resources",
        "read-only MCP trait/run resources are not implemented by this thin adapter",
    ));
    envelope = envelope.with_capability(CapabilityReport::supported("runtime.mcp-prompts"));
    with_runtime_claim_gate(envelope)
}

fn mcp_error_envelope<T>(error: &crate::Error, call_payload: bool) -> Envelope<T> {
    let response_error = match error {
        crate::Error::TransientGitLock { reason } => ResponseError::new(
            "io.transient-git-lock",
            "transient git lock retry exhausted",
        )
        .with_detail("reason", reason.code().to_string()),
        crate::Error::Core(core) => ResponseError::from_core_error(core),
        crate::Error::Environment(environment) => match environment {
            crate::environment::Error::Filesystem { path, source } => {
                ResponseError::new("io.filesystem", "filesystem error")
                    .with_detail("path", path.clone())
                    .with_detail("message", source.to_string())
            }
            crate::environment::Error::Git {
                command,
                path,
                exit_status,
                timed_out,
                message,
            } => {
                let mut response = ResponseError::new("io.git", "git error")
                    .with_detail("message", message.clone());
                if let Some(command) = command {
                    response = response.with_detail("command", command.clone());
                }
                if let Some(path) = path {
                    response = response.with_detail("path", path.clone());
                }
                if let Some(exit_status) = exit_status {
                    response = response.with_detail("exit-status", exit_status.to_string());
                }
                if *timed_out {
                    response = response.with_detail("timed-out", "true".to_string());
                }
                response
            }
            crate::environment::Error::Process {
                command,
                path,
                exit_status,
                timed_out,
                message,
            } => {
                let mut response = ResponseError::new("io.process", "process error")
                    .with_detail("message", message.clone());
                if let Some(command) = command {
                    response = response.with_detail("command", command.clone());
                }
                if let Some(path) = path {
                    response = response.with_detail("path", path.clone());
                }
                if let Some(exit_status) = exit_status {
                    response = response.with_detail("exit-status", exit_status.to_string());
                }
                if *timed_out {
                    response = response.with_detail("timed-out", "true".to_string());
                }
                response
            }
            crate::environment::Error::HostInstall {
                host,
                path,
                message,
            } => {
                let mut response = ResponseError::new("io.host-install", "host install error")
                    .with_detail("message", message.clone());
                if let Some(host) = host {
                    response = response.with_detail("host", host.clone());
                }
                if let Some(path) = path {
                    response = response.with_detail("path", path.clone());
                }
                response
            }
        },
        crate::Error::Parse(parse) => match parse {
            crate::parse::Error::JsonDeserialize { .. }
            | crate::parse::Error::JsonSerialize { .. } => {
                ResponseError::new("io.serde", "serde error")
                    .with_detail("context", parse.context().to_string())
                    .with_detail("message", parse.source_message())
            }
            crate::parse::Error::TomlDecode { .. } => {
                ResponseError::new("io.toml-decode", "toml decode error")
                    .with_detail("context", parse.context().to_string())
                    .with_detail("message", parse.source_message())
            }
            crate::parse::Error::TomlEncode { .. } => {
                ResponseError::new("io.toml-encode", "toml encode error")
                    .with_detail("context", parse.context().to_string())
                    .with_detail("message", parse.source_message())
            }
            crate::parse::Error::TomlEditDecode { .. } => {
                ResponseError::new("io.toml-decode", "toml decode error")
                    .with_detail("context", parse.context().to_string())
                    .with_detail("message", parse.source_message())
            }
        },
        crate::Error::Layout(crate::layout::Error::InvalidTraitId { id, reason }) => {
            ResponseError::new("io.invalid-trait-id", "invalid trait id")
                .with_detail("id", id.clone())
                .with_detail("reason", reason.clone())
        }
        crate::Error::Layout(crate::layout::Error::ConflictingCdkSources { id }) => {
            ResponseError::new("io.conflicting-cdk-sources", "conflicting CDK sources")
                .with_detail("id", id.clone())
        }
        crate::Error::Export(export) => ResponseError::new("io.export", "export error")
            .with_detail("path", export.path().to_string())
            .with_detail("message", export.to_string()),
        crate::Error::Registry(registry) => ResponseError::new("io.registry", "registry error")
            .with_detail("message", registry.to_string()),
        crate::Error::Publish(publish) => ResponseError::new("io.publish", "publish error")
            .with_detail("message", publish.to_string()),
        crate::Error::Usage { message } => {
            ResponseError::new("io.usage", "usage error").with_detail("message", message.clone())
        }
        crate::Error::RollbackIncomplete { primary, notes } => {
            let mut response = ResponseError::new("io.rollback-incomplete", "rollback incomplete")
                .with_detail("message", primary.to_string());
            for (index, note) in notes.iter().enumerate() {
                response = response.with_detail(format!("note-{index}"), note.clone());
            }
            response
        }
    };
    let mut envelope = Envelope::err_response(response_error);
    for report in ctx_traits_core::procedure::session::run_session_capability_reports(
        true,
        true,
        call_payload,
        false,
        true,
        false,
        false,
    ) {
        envelope = envelope.with_capability(report);
    }
    envelope = envelope.with_capability(CapabilityReport::unsupported(
        "runtime.mcp-resources",
        "read-only MCP trait/run resources are not implemented by this thin adapter",
    ));
    envelope = envelope.with_capability(CapabilityReport::supported("runtime.mcp-prompts"));
    with_runtime_claim_gate(envelope)
}

fn with_runtime_claim_gate<T>(envelope: Envelope<T>) -> Envelope<T> {
    let (warning, capability) = ctx_traits_core::launch::runtime_posture();
    envelope.with_warning(warning).with_capability(capability)
}

fn empty_json_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn mcp_tool_descriptions() -> Value {
    serde_json::json!([
        {
            "name": "ctx_traits_run_info",
            "description": "Non-mutating preflight. Inspect trait/query inputs, accepted --<port> args, lifecycle gates, outputs, and start examples before any run session is created."
        },
        {
            "name": "ctx_traits_run_start",
            "description": "Start exactly one ctx.traits run session after run-info. Fill only grounded required inputs via inputs or trait_args; query start refuses ambiguous/no-match selections."
        },
        {
            "name": "ctx_traits_run_set",
            "description": "Submit one simple value for the current awaiting input or current frame target. The runtime validates schema and advances to the next frame/status."
        },
        {
            "name": "ctx_traits_run_call",
            "description": "Submit the full current-frame call payload. Submit only requested outputs/signals; do not execute command frames through this thin MCP adapter."
        },
        {
            "name": "ctx_traits_run_frame",
            "description": "Read the current frame without advancing. Use it to know exactly one current prompt/outputs contract."
        },
        {
            "name": "ctx_traits_run_status",
            "description": "Read run status without advancing. Loops, conditions, blocked states, and completion are controlled by ctx.traits runtime evidence."
        },
        {
            "name": "ctx_traits_run_next",
            "description": "Pull the next pending frame for an attached agent role from one session or the session store without claiming or mutating it."
        }
    ])
}

fn mcp_prompt_templates() -> Value {
    let contract = "Before starting a trait run, inspect it with ctx_traits_run_info. Fill required inputs only when grounded in the user request, conversation, selected context, or tool results. Ask for missing or ambiguous required inputs. Do exactly one current frame. Submit only requested outputs with ctx_traits_run_set or ctx_traits_run_call. Wait for ctx.traits to route to the next frame, blocked status, or completion. Loops and completion are controlled by the trait runtime, not by this prompt.";
    serde_json::json!([
        {
            "name": "use_trait",
            "description": "Select and use a ctx.traits runtime trait without bypassing runtime gates.",
            "template": contract
        },
        {
            "name": "start_trait_run",
            "description": "Inspect run-info, fill grounded inputs or ask, then start a run.",
            "template": contract
        },
        {
            "name": "continue_trait_run",
            "description": "Read current status/frame and complete exactly one frame.",
            "template": contract
        }
    ])
}
