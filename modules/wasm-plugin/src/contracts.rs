//! Host-neutral plugin event/action contracts.
//!
//! This crate returns action plans only. Host adapters execute those actions and
//! are responsible for proving capabilities before wiring dynamic behavior.

use ctx_traits_core::response::{
    CapabilityReport, JsonAbiErrorCodes, ResponseError, decode_then as response_decode_then,
};
use serde::{Deserialize, Serialize};

pub const PLUGIN_ABI_SCHEMA_VERSION: &str = "0.1.0";

const PLUGIN_ERROR_CODES: JsonAbiErrorCodes = JsonAbiErrorCodes {
    decode_code: "plugin.decode-request",
    decode_message: "failed to decode plugin request",
    serialize_code: "plugin.serialize-envelope",
    serialize_message: "failed to serialize plugin envelope",
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString, strum::IntoStaticStr,
)]
enum HostCapability {
    #[strum(serialize = "can-static-render")]
    StaticRender,
    #[strum(serialize = "can-register-tools")]
    RegisterTools,
    #[strum(serialize = "can-hook-session-created")]
    HookSessionCreated,
    #[strum(serialize = "can-hook-tool-after")]
    HookToolAfter,
    #[strum(serialize = "can-hook-compaction")]
    HookCompaction,
    #[strum(serialize = "can-append-prompt")]
    AppendPrompt,
    #[strum(serialize = "can-persist-ledger")]
    PersistLedger,
    #[strum(serialize = "can-call-mcp")]
    CallMcp,
    #[strum(serialize = "can-load-wasm-directly")]
    LoadWasmDirectly,
}

impl HostCapability {
    fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
enum Tool {
    #[strum(serialize = "ctx_traits_resolve")]
    Resolve,
    #[strum(serialize = "ctx_traits_pack")]
    Pack,
    #[strum(serialize = "ctx_traits_explain")]
    Explain,
    #[strum(serialize = "ctx_traits_render")]
    Render,
    #[strum(serialize = "ctx_traits_status")]
    Status,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Capabilities {
    #[serde(default)]
    pub can_static_render: bool,
    #[serde(default)]
    pub can_register_tools: bool,
    #[serde(default)]
    pub can_hook_session_created: bool,
    #[serde(default)]
    pub can_hook_tool_after: bool,
    #[serde(default)]
    pub can_hook_compaction: bool,
    #[serde(default)]
    pub can_append_prompt: bool,
    #[serde(default)]
    pub can_persist_ledger: bool,
    #[serde(default)]
    pub can_call_mcp: bool,
    #[serde(default)]
    pub can_load_wasm_directly: bool,
}

impl Capabilities {
    fn supports(&self, capability: HostCapability) -> bool {
        match capability {
            HostCapability::StaticRender => self.can_static_render,
            HostCapability::RegisterTools => self.can_register_tools,
            HostCapability::HookSessionCreated => self.can_hook_session_created,
            HostCapability::HookToolAfter => self.can_hook_tool_after,
            HostCapability::HookCompaction => self.can_hook_compaction,
            HostCapability::AppendPrompt => self.can_append_prompt,
            HostCapability::PersistLedger => self.can_persist_ledger,
            HostCapability::CallMcp => self.can_call_mcp,
            HostCapability::LoadWasmDirectly => self.can_load_wasm_directly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "event", content = "value")]
pub enum Event {
    SessionCreated {
        session_id: String,
    },
    SessionCompacted {
        session_id: String,
        reason: String,
    },
    ToolAfter {
        session_id: String,
        tool_name: String,
    },
    PromptAppend {
        session_id: String,
        prompt_id: String,
    },
    ContextStatus {
        session_id: String,
    },
    CommandInvocation {
        command: PluginCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginCommand {
    pub name: String,
    #[serde(default)]
    pub arguments_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct EventRequest {
    pub capabilities: Capabilities,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "action", content = "value")]
pub enum Action {
    Resolve { request_json: String },
    Pack { request_json: String },
    Explain { request_json: String },
    Render { request_json: String },
    Status { session_id: String },
    UnsupportedCapability { capability: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ActionPlan {
    pub schema_version: String,
    pub actions: Vec<Action>,
    pub capabilities: Vec<CapabilityReport>,
}

pub fn handle_event_json(input: &str) -> String {
    decode_then(input, |request: EventRequest| {
        Ok(plan_host_actions(request))
    })
}

pub fn plan_host_actions(request: EventRequest) -> ActionPlan {
    let mut actions = Vec::new();
    let mut capabilities = capability_reports(&request.capabilities);

    match request.event {
        Event::SessionCreated { .. } => {
            if !request
                .capabilities
                .supports(HostCapability::HookSessionCreated)
            {
                push_missing(&mut actions, HostCapability::HookSessionCreated);
            }
        }
        Event::SessionCompacted { session_id, .. } => {
            let missing = missing_requirements(
                &request.capabilities,
                &[
                    HostCapability::HookCompaction,
                    HostCapability::PersistLedger,
                ],
            );
            if missing.is_empty() {
                actions.push(Action::Status { session_id });
            } else {
                push_missing_all(&mut actions, missing);
            }
        }
        Event::ToolAfter { session_id, .. } => {
            let missing = missing_requirements(
                &request.capabilities,
                &[HostCapability::HookToolAfter, HostCapability::PersistLedger],
            );
            if missing.is_empty() {
                actions.push(Action::Status { session_id });
            } else {
                push_missing_all(&mut actions, missing);
            }
        }
        Event::PromptAppend { session_id, .. } => {
            let missing = missing_requirements(
                &request.capabilities,
                &[HostCapability::AppendPrompt, HostCapability::PersistLedger],
            );
            if missing.is_empty() {
                actions.push(Action::Status { session_id });
            } else {
                push_missing_all(&mut actions, missing);
            }
        }
        Event::ContextStatus { session_id } => {
            if request.capabilities.supports(HostCapability::PersistLedger) {
                actions.push(Action::Status { session_id });
            } else {
                push_missing(&mut actions, HostCapability::PersistLedger);
            }
        }
        Event::CommandInvocation { command } => {
            plan_command(command, &request.capabilities, &mut actions);
        }
    }

    capabilities.sort();
    capabilities.dedup();
    ActionPlan {
        schema_version: PLUGIN_ABI_SCHEMA_VERSION.to_string(),
        actions,
        capabilities,
    }
}

fn plan_command(command: PluginCommand, capabilities: &Capabilities, actions: &mut Vec<Action>) {
    if !capabilities.supports(HostCapability::RegisterTools) {
        push_missing(actions, HostCapability::RegisterTools);
        return;
    }

    match command.name.parse::<Tool>() {
        Ok(Tool::Resolve) => {
            if push_missing_wasm_or_mcp(capabilities, actions) {
                actions.push(Action::Resolve {
                    request_json: command.arguments_json,
                });
            }
        }
        Ok(Tool::Pack) => {
            if push_missing_wasm_or_mcp(capabilities, actions) {
                actions.push(Action::Pack {
                    request_json: command.arguments_json,
                });
            }
        }
        Ok(Tool::Explain) => {
            if push_missing_wasm_or_mcp(capabilities, actions) {
                actions.push(Action::Explain {
                    request_json: command.arguments_json,
                });
            }
        }
        Ok(Tool::Render) => {
            if push_missing_wasm_or_mcp(capabilities, actions) {
                actions.push(Action::Render {
                    request_json: command.arguments_json,
                });
            }
        }
        Ok(Tool::Status) => {
            if capabilities.supports(HostCapability::PersistLedger) {
                actions.push(Action::Status {
                    session_id: command.arguments_json,
                });
            } else {
                push_missing(actions, HostCapability::PersistLedger);
            }
        }
        Err(_) => actions.push(unsupported(
            "command-invocation",
            format!("unsupported plugin command {:?}", command.name),
        )),
    }
}

fn capability_reports(capabilities: &Capabilities) -> Vec<CapabilityReport> {
    let mut reports = vec![
        report(
            HostCapability::StaticRender,
            capabilities.supports(HostCapability::StaticRender),
            "static rendered skills are unavailable",
        ),
        report(
            HostCapability::RegisterTools,
            capabilities.supports(HostCapability::RegisterTools),
            "tool registration is unavailable",
        ),
        report(
            HostCapability::HookSessionCreated,
            capabilities.supports(HostCapability::HookSessionCreated),
            "session-created hooks are unavailable",
        ),
        report(
            HostCapability::HookToolAfter,
            capabilities.supports(HostCapability::HookToolAfter),
            "tool-after hooks are unavailable",
        ),
        report(
            HostCapability::HookCompaction,
            capabilities.supports(HostCapability::HookCompaction),
            "compaction hooks are unavailable",
        ),
        report(
            HostCapability::AppendPrompt,
            capabilities.supports(HostCapability::AppendPrompt),
            "prompt append is unavailable",
        ),
        report(
            HostCapability::PersistLedger,
            capabilities.supports(HostCapability::PersistLedger),
            "ledger persistence is unavailable",
        ),
        report(
            HostCapability::CallMcp,
            capabilities.supports(HostCapability::CallMcp),
            "MCP call bridge is unavailable",
        ),
        report(
            HostCapability::LoadWasmDirectly,
            capabilities.supports(HostCapability::LoadWasmDirectly),
            "direct WASM load is unavailable",
        ),
    ];
    reports.sort();
    reports
}

fn report(
    capability: HostCapability,
    supported: bool,
    unsupported_reason: &str,
) -> CapabilityReport {
    if supported {
        CapabilityReport::supported(capability.as_str())
    } else {
        CapabilityReport::unsupported(capability.as_str(), unsupported_reason)
    }
}

fn unsupported(capability: impl Into<String>, reason: impl Into<String>) -> Action {
    Action::UnsupportedCapability {
        capability: capability.into(),
        reason: reason.into(),
    }
}

fn missing_requirements(
    capabilities: &Capabilities,
    required: &[HostCapability],
) -> Vec<HostCapability> {
    required
        .iter()
        .copied()
        .filter(|capability| !capabilities.supports(*capability))
        .collect()
}

fn push_missing(actions: &mut Vec<Action>, capability: HostCapability) {
    let capability = capability.as_str();
    actions.push(unsupported(
        capability,
        format!("required host capability {capability} is not declared"),
    ));
}

fn push_missing_all(actions: &mut Vec<Action>, capabilities: Vec<HostCapability>) {
    for capability in capabilities {
        push_missing(actions, capability);
    }
}

fn push_missing_wasm_or_mcp(capabilities: &Capabilities, actions: &mut Vec<Action>) -> bool {
    if capabilities.supports(HostCapability::LoadWasmDirectly)
        || capabilities.supports(HostCapability::CallMcp)
    {
        return true;
    }
    push_missing(actions, HostCapability::LoadWasmDirectly);
    push_missing(actions, HostCapability::CallMcp);
    false
}

fn decode_then<Request, Value, F>(input: &str, f: F) -> String
where
    Request: serde::de::DeserializeOwned,
    Value: Serialize,
    F: FnOnce(Request) -> ctx_traits_core::Result<Value>,
{
    response_decode_then(input, PLUGIN_ERROR_CODES, plugin_error, f)
}

fn plugin_error(error: ctx_traits_core::Error) -> ResponseError {
    ResponseError::from_core_error(&error)
}
