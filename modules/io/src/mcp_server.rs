//! Minimal synchronous JSON-RPC stdio transport for ctx.traits MCP tools.

use std::io::{self, BufRead, Write};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const JSONRPC_VERSION: &str = "2.0";
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_JSONRPC_LINE_BYTES: usize = 1024 * 1024;

type ServerResult<T> = std::result::Result<T, ServerError>;

#[derive(Debug)]
struct ServerError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl ServerError {
    fn parse(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {}", method.into()),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Debug)]
struct JsonRpcRequest {
    id: Option<Value>,
    is_notification: bool,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolsCallParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug)]
struct ToolSpec {
    name: &'static str,
    description: &'static str,
    input_schema: fn() -> ServerResult<Value>,
    dispatch: fn(Value) -> ServerResult<Value>,
}

enum ReadLine {
    Line(Vec<u8>),
    TooLong,
}

/// Serve MCP JSON-RPC requests from stdin and write responses to stdout.
pub fn serve_stdio() -> crate::Result<()> {
    // The CLI drive loop, when it spawns the harness that in turn spawns this
    // subprocess as its MCP server, injects the cumulative active-drive
    // elapsed seconds already measured so far via this env var (see
    // `mcp_harness_argv` in `modules/cli/src/app/drive.rs`). Absent when this
    // process was started outside a drive loop (a bare `ctx traits internal mcp`, or
    // a long-lived MCP host serving unrelated sessions) — elapsed
    // observation only ever starts when a trusted parent drive supplies a
    // baseline, so a bare/persistent host never accrues process age (idle
    // or otherwise) as elapsed evidence for any session it serves. A
    // present-but-unparsable value is treated the same as absent, never as
    // baseline `0` — a malformed baseline must not silently start the
    // clock. See this variable's contract in `crate::env_reference`.
    if let Some(elapsed_baseline_seconds) = std::env::var("CTX_TRAITS_ELAPSED_SECONDS_BASELINE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        crate::mcp::initialize_elapsed_baseline(elapsed_baseline_seconds);
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(stdin.lock(), stdout.lock())
}

fn serve<R, W>(mut reader: R, mut writer: W) -> crate::Result<()>
where
    R: BufRead,
    W: Write,
{
    while let Some(line) =
        read_limited_line(&mut reader).map_err(|source| crate::environment::Error::Filesystem {
            path: "stdin".to_string(),
            source,
        })?
    {
        let line = match line {
            ReadLine::Line(line) => line,
            ReadLine::TooLong => {
                let response = error_response(
                    Some(Value::Null),
                    ServerError::invalid_request("JSON-RPC line exceeds maximum size"),
                );
                write_response(&mut writer, &response)?;
                continue;
            }
        };
        let line =
            String::from_utf8(line).map_err(|source| crate::environment::Error::Filesystem {
                path: "stdin".to_string(),
                source: io::Error::new(io::ErrorKind::InvalidData, source),
            })?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&line) {
            write_response(&mut writer, &response)?;
        }
    }
    Ok(writer
        .flush()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: "stdout".to_string(),
            source,
        })?)
}

fn read_limited_line<R: BufRead>(reader: &mut R) -> io::Result<Option<ReadLine>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !too_long {
                return Ok(None);
            }
            return Ok(Some(if too_long {
                ReadLine::TooLong
            } else {
                ReadLine::Line(line)
            }));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consume = newline.map_or(available.len(), |index| index + 1);
        let data_len = newline.map_or(consume, |_| consume.saturating_sub(1));
        if !too_long {
            if line.len().saturating_add(data_len) > MAX_JSONRPC_LINE_BYTES {
                let remaining = MAX_JSONRPC_LINE_BYTES.saturating_sub(line.len());
                if remaining > 0 {
                    line.extend_from_slice(&available[..remaining]);
                }
                too_long = true;
            } else {
                line.extend_from_slice(&available[..data_len]);
            }
        }
        reader.consume(consume);
        if newline.is_some() {
            return Ok(Some(if too_long {
                ReadLine::TooLong
            } else {
                ReadLine::Line(line)
            }));
        }
    }
}

fn handle_line(line: &str) -> Option<JsonRpcResponse> {
    let value = match serde_json::from_str::<Value>(line) {
        Ok(value) => value,
        Err(err) => {
            return Some(error_response(
                Some(Value::Null),
                ServerError::parse(format!("parse error: {err}")),
            ));
        }
    };
    let request = match parse_request(value) {
        Ok(request) => request,
        Err((id, error)) => return Some(error_response(id, error)),
    };
    match handle_request(&request) {
        Ok(Some(result)) => Some(success_response(request.id.clone(), result)),
        Ok(None) => None,
        Err(_) if request.is_notification => None,
        Err(error) => Some(error_response(request.id.clone(), error)),
    }
}

fn parse_request(
    value: Value,
) -> std::result::Result<JsonRpcRequest, (Option<Value>, ServerError)> {
    let Some(object) = value.as_object() else {
        return Err((
            Some(Value::Null),
            ServerError::invalid_request("JSON-RPC request must be an object"),
        ));
    };
    let id = object.get("id").cloned();
    let is_notification = !object.contains_key("id");
    if object
        .get("jsonrpc")
        .and_then(Value::as_str)
        .is_some_and(|version| version != JSONRPC_VERSION)
    {
        return Err((id, ServerError::invalid_request("jsonrpc must be \"2.0\"")));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err((id, ServerError::invalid_request("method is required")));
    };
    Ok(JsonRpcRequest {
        id,
        is_notification,
        method: method.to_string(),
        params: object.get("params").cloned(),
    })
}

fn handle_request(request: &JsonRpcRequest) -> ServerResult<Option<Value>> {
    match request.method.as_str() {
        "initialize" => Ok(Some(initialize_result(request.params.as_ref()))),
        "notifications/initialized" => Ok(None),
        "ping" => Ok(Some(serde_json::json!({}))),
        "tools/list" => Ok(Some(tools_list_result()?)),
        "tools/call" => Ok(Some(tools_call_result(request.params.clone())?)),
        method => Err(ServerError::method_not_found(method)),
    }
}

fn initialize_result(params: Option<&Value>) -> Value {
    let protocol_version = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(MCP_PROTOCOL_VERSION);
    serde_json::json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "ctx-traits",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn tools_list_result() -> ServerResult<Value> {
    let tools = tool_specs()
        .iter()
        .map(|tool| {
            Ok(serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.input_schema)()?,
            }))
        })
        .collect::<ServerResult<Vec<_>>>()?;
    Ok(serde_json::json!({ "tools": tools }))
}

fn tools_call_result(params: Option<Value>) -> ServerResult<Value> {
    let params = params.ok_or_else(|| ServerError::invalid_params("tools/call params required"))?;
    let params: ToolsCallParams = serde_json::from_value(params)
        .map_err(|err| ServerError::invalid_params(format!("invalid tools/call params: {err}")))?;
    let Some(tool) = tool_specs().iter().find(|tool| tool.name == params.name) else {
        return Err(ServerError::invalid_params(format!(
            "unknown tool {:?}",
            params.name
        )));
    };
    (tool.dispatch)(params.arguments.unwrap_or_else(empty_object))
}

fn tool_specs() -> &'static [ToolSpec] {
    &[
        ToolSpec {
            name: "ctx_traits_run_start",
            description: "Start exactly one ctx.traits run session with grounded inputs. Runtime validation and persistence use the same path as CLI JSON.",
            input_schema: schema_value::<crate::mcp::StartRequest>,
            dispatch: dispatch_run_start,
        },
        ToolSpec {
            name: "ctx_traits_run_call",
            description: "Submit the full current-frame call payload. Command execution evidence is refused by the thin MCP adapter.",
            input_schema: schema_value::<crate::mcp::CallRequest>,
            dispatch: dispatch_run_call,
        },
        ToolSpec {
            name: "ctx_traits_run_set",
            description: "Submit one simple value for the current awaiting input or frame target.",
            input_schema: schema_value::<crate::mcp::SetRequest>,
            dispatch: dispatch_run_set,
        },
        ToolSpec {
            name: "ctx_traits_run_status",
            description: "Read run status without advancing the session.",
            input_schema: schema_value::<crate::mcp::InspectRequest>,
            dispatch: dispatch_run_status,
        },
        ToolSpec {
            name: "ctx_traits_run_frame",
            description: "Read the current frame without advancing the session.",
            input_schema: schema_value::<crate::mcp::InspectRequest>,
            dispatch: dispatch_run_frame,
        },
        ToolSpec {
            name: "ctx_traits_run_next",
            description: "Pull the next pending frame for an attached role from one session or a session store without claiming it.",
            input_schema: schema_value::<crate::mcp::NextRequest>,
            dispatch: dispatch_run_next,
        },
    ]
}

fn dispatch_run_start(arguments: Value) -> ServerResult<Value> {
    let request = deserialize_arguments(arguments)?;
    let envelope = crate::mcp::ctx_traits_run_start(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn dispatch_run_call(arguments: Value) -> ServerResult<Value> {
    let command_execution_requested = arguments
        .get("submission")
        .and_then(|submission| submission.get("command-execution"))
        .is_some();
    let mut request: crate::mcp::CallRequest = deserialize_arguments(arguments)?;
    if command_execution_requested {
        request.submission.command_execution = Some(
            ctx_traits_core::procedure::session::CommandExecutionEvidence {
                argv: Vec::new(),
                output_slot: String::new(),
                executable_digest: None,
                exit_code: None,
                timed_out: false,
                stdout: None,
                stderr: None,
                stdout_truncated: false,
                stderr_truncated: false,
            },
        );
    }
    let envelope = crate::mcp::ctx_traits_run_call(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn dispatch_run_set(arguments: Value) -> ServerResult<Value> {
    let request = deserialize_arguments(arguments)?;
    let envelope = crate::mcp::ctx_traits_run_set(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn dispatch_run_status(arguments: Value) -> ServerResult<Value> {
    let request = deserialize_arguments(arguments)?;
    let envelope = crate::mcp::ctx_traits_run_status(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn dispatch_run_frame(arguments: Value) -> ServerResult<Value> {
    let request = deserialize_arguments(arguments)?;
    let envelope = crate::mcp::ctx_traits_run_frame(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn dispatch_run_next(arguments: Value) -> ServerResult<Value> {
    let request = deserialize_arguments(arguments)?;
    let envelope = crate::mcp::ctx_traits_run_next(request).map_err(io_error_to_server)?;
    mcp_tool_result(envelope)
}

fn deserialize_arguments<T>(arguments: Value) -> ServerResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(arguments)
        .map_err(|err| ServerError::invalid_params(format!("invalid tool arguments: {err}")))
}

fn schema_value<T>() -> ServerResult<Value>
where
    T: JsonSchema,
{
    serde_json::to_value(schema_for!(T))
        .map_err(|err| ServerError::internal(format!("failed to serialize tool schema: {err}")))
}

fn mcp_tool_result<T>(envelope: ctx_traits_core::response::Envelope<T>) -> ServerResult<Value>
where
    T: Serialize,
{
    let is_error = !envelope.ok;
    let structured = serde_json::to_value(&envelope).map_err(|err| {
        ServerError::internal(format!("failed to serialize tool envelope: {err}"))
    })?;
    let text = serde_json::to_string(&structured).map_err(|err| {
        ServerError::internal(format!("failed to serialize tool envelope text: {err}"))
    })?;
    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": structured,
        "isError": is_error
    }))
}

fn io_error_to_server(err: crate::Error) -> ServerError {
    ServerError::internal(err.to_string())
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn success_response(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: Option<Value>, error: ServerError) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: None,
        error: Some(JsonRpcError {
            code: error.code,
            message: error.message,
            data: error.data,
        }),
    }
}

fn write_response<W>(writer: &mut W, response: &JsonRpcResponse) -> crate::Result<()>
where
    W: Write,
{
    serde_json::to_writer(&mut *writer, response).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: "serialize JSON-RPC response".to_string(),
            source,
        }
    })?;
    Ok(writer
        .write_all(b"\n")
        .map_err(|source| crate::environment::Error::Filesystem {
            path: "stdout".to_string(),
            source,
        })?)
}
