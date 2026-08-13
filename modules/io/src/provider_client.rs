//! 0079: the native `transport = "api"` provider client — one blocking HTTPS
//! round trip over the existing `ureq` client (registry.rs already talks to
//! the network this way; this is one more caller of it, not a new
//! dependency), for one-shot text seats. No streaming, no tool surface: a
//! request wanting either belongs on a harness, not here.
//!
//! Two wire formats, normalized into one [`ProviderResponse`] so callers and
//! token accounting never need to know which one answered:
//! - OpenAI-compatible `/chat/completions` — the baseline; covers OpenRouter,
//!   proxies, and local model servers with zero vendor-specific code.
//! - Anthropic `/v1/messages`.

use std::time::Duration;

use crate::harness_config::ProviderWire;

/// Sized for a call that should take well under a second, not the ~20s a
/// harness spawn needs.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_READ_TIMEOUT_MS: u64 = 10_000;
/// Total attempts, not additional retries: `2` means one retry after an
/// initial transient failure.
pub const DEFAULT_RETRIES: u32 = 2;
pub const DEFAULT_MAX_TOKENS: u32 = 1024;

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// One provider call's fully resolved inputs — everything the two wire
/// formats need, with the seat/config-level defaults (timeouts, retries,
/// max-tokens) already applied by the caller.
#[derive(Debug, Clone)]
pub struct ProviderRequest<'a> {
    pub base_url: &'a str,
    pub wire: ProviderWire,
    pub model: &'a str,
    /// The resolved credential — redaction-wrapped, so this struct's `Debug`
    /// derive stays safe. Callers resolve it once via
    /// [`crate::env_reference::resolve_env_var_reference`] and pass it
    /// straight through; only the header construction below exposes it.
    pub api_key: &'a crate::secret::Secret,
    pub system: Option<&'a str>,
    pub user: &'a str,
    pub max_tokens: u32,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub retries: u32,
}

/// Both wire formats normalized to one shape: text plus usage. Usage is
/// `None`, never `0`, when a response omits it — some OpenAI-compatible
/// gateways drop or rename the `usage` object, and a silent zero would read
/// to the token panel as "this call cost nothing" rather than "unknown".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderResponse {
    pub text: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("provider request to {url} failed: {message}")]
    Request { url: String, message: String },
    #[error("provider request to {url} failed with HTTP {status}: {message}")]
    Status {
        url: String,
        status: u16,
        message: String,
    },
    #[error("provider response from {url} was not valid JSON: {message}")]
    InvalidResponse { url: String, message: String },
    #[error("provider response from {url} had no completion text")]
    EmptyCompletion { url: String },
}

/// Dispatch one request over the wire format the seat declared, with a small
/// bounded retry over transient failures (connect errors, 429, 5xx). A
/// non-transient failure (4xx other than 429, malformed response) returns on
/// the first attempt.
pub fn dispatch(request: &ProviderRequest<'_>) -> Result<ProviderResponse, Error> {
    let attempts = request.retries.max(1);
    let mut last_error = None;
    for attempt in 0..attempts {
        let outcome = match request.wire {
            ProviderWire::OpenaiCompat => dispatch_openai_compat(request),
            ProviderWire::Anthropic => dispatch_anthropic(request),
        };
        match outcome {
            Ok(response) => return Ok(response),
            Err(error) if attempt + 1 < attempts && is_transient(&error) => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("loop always runs at least once and only continues on Err"))
}

fn is_transient(error: &Error) -> bool {
    match error {
        Error::Request { .. } => true,
        Error::Status { status, .. } => *status == 429 || *status >= 500,
        Error::InvalidResponse { .. } | Error::EmptyCompletion { .. } => false,
    }
}

fn build_agent(request: &ProviderRequest<'_>) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(request.connect_timeout_ms)))
        .timeout_recv_response(Some(Duration::from_millis(request.read_timeout_ms)))
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

fn user_agent() -> String {
    format!("ctx-traits/{}", env!("CARGO_PKG_VERSION"))
}

fn request_error(url: &str, source: &ureq::Error) -> Error {
    Error::Request {
        url: url.to_string(),
        message: source.to_string(),
    }
}

fn read_body_json(
    url: &str,
    response: ureq::http::Response<ureq::Body>,
) -> Result<serde_json::Value, Error> {
    let status = response.status();
    let bytes = response
        .into_body()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(|source| request_error(url, &source))?;
    if !status.is_success() {
        let message = String::from_utf8_lossy(&bytes).trim().to_string();
        return Err(Error::Status {
            url: url.to_string(),
            status: status.as_u16(),
            message: if message.is_empty() {
                format!("HTTP {status}")
            } else {
                message
            },
        });
    }
    serde_json::from_slice(&bytes).map_err(|source| Error::InvalidResponse {
        url: url.to_string(),
        message: source.to_string(),
    })
}

// ---------------------------------------------------------------------------
// OpenAI-compatible `/chat/completions`
// ---------------------------------------------------------------------------

fn openai_compat_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

fn openai_compat_request_body(request: &ProviderRequest<'_>) -> serde_json::Value {
    let mut messages = Vec::new();
    if let Some(system) = request.system {
        messages.push(serde_json::json!({ "role": "system", "content": system }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": request.user }));
    serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
    })
}

fn dispatch_openai_compat(request: &ProviderRequest<'_>) -> Result<ProviderResponse, Error> {
    let url = openai_compat_url(request.base_url);
    let agent = build_agent(request);
    let body = openai_compat_request_body(request).to_string();
    let response = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .header(
            "Authorization",
            &format!("Bearer {}", request.api_key.expose()),
        )
        .header("User-Agent", &user_agent())
        .send(&body)
        .map_err(|source| request_error(&url, &source))?;
    let body = read_body_json(&url, response)?;
    parse_openai_compat_response(&url, &body)
}

fn parse_openai_compat_response(
    url: &str,
    body: &serde_json::Value,
) -> Result<ProviderResponse, Error> {
    let text = body
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string);
    let text = match text {
        Some(text) if !text.is_empty() => text,
        _ => {
            return Err(Error::EmptyCompletion {
                url: url.to_string(),
            });
        }
    };
    let usage = body.get("usage");
    let input_tokens = usage
        .and_then(|usage| usage.get("prompt_tokens"))
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(serde_json::Value::as_u64);
    Ok(ProviderResponse {
        text,
        input_tokens,
        output_tokens,
    })
}

// ---------------------------------------------------------------------------
// Anthropic `/v1/messages`
// ---------------------------------------------------------------------------

fn anthropic_url(base_url: &str) -> String {
    format!("{}/v1/messages", base_url.trim_end_matches('/'))
}

fn anthropic_request_body(request: &ProviderRequest<'_>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": [{ "role": "user", "content": request.user }],
    });
    if let Some(system) = request.system {
        body["system"] = serde_json::Value::String(system.to_string());
    }
    body
}

fn dispatch_anthropic(request: &ProviderRequest<'_>) -> Result<ProviderResponse, Error> {
    let url = anthropic_url(request.base_url);
    let agent = build_agent(request);
    let body = anthropic_request_body(request).to_string();
    let response = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .header("x-api-key", request.api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("User-Agent", &user_agent())
        .send(&body)
        .map_err(|source| request_error(&url, &source))?;
    let body = read_body_json(&url, response)?;
    parse_anthropic_response(&url, &body)
}

fn parse_anthropic_response(
    url: &str,
    body: &serde_json::Value,
) -> Result<ProviderResponse, Error> {
    let text = body
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|block| block.get("text"))
        .and_then(|text| text.as_str())
        .map(str::to_string);
    let text = match text {
        Some(text) if !text.is_empty() => text,
        _ => {
            return Err(Error::EmptyCompletion {
                url: url.to_string(),
            });
        }
    };
    let usage = body.get("usage");
    let input_tokens = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(serde_json::Value::as_u64);
    Ok(ProviderResponse {
        text,
        input_tokens,
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> crate::secret::Secret {
        crate::secret::Secret::new("key".to_string())
    }

    fn request<'a>(wire: ProviderWire, api_key: &'a crate::secret::Secret) -> ProviderRequest<'a> {
        ProviderRequest {
            base_url: "https://example.invalid",
            wire,
            model: "test-model",
            api_key,
            system: Some("system prompt"),
            user: "user prompt",
            max_tokens: DEFAULT_MAX_TOKENS,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            read_timeout_ms: DEFAULT_READ_TIMEOUT_MS,
            retries: DEFAULT_RETRIES,
        }
    }

    #[test]
    fn openai_compat_request_body_carries_system_and_user() {
        let body = openai_compat_request_body(&request(ProviderWire::OpenaiCompat, &test_key()));
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "system prompt");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "user prompt");
    }

    #[test]
    fn openai_compat_request_body_omits_system_when_absent() {
        let key = test_key();
        let mut req = request(ProviderWire::OpenaiCompat, &key);
        req.system = None;
        let body = openai_compat_request_body(&req);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn openai_compat_url_strips_trailing_slash() {
        assert_eq!(
            openai_compat_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            openai_compat_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn parse_openai_compat_response_extracts_text_and_usage() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "hello there" } }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 3 },
        });
        let response = parse_openai_compat_response("url", &body).unwrap();
        assert_eq!(response.text, "hello there");
        assert_eq!(response.input_tokens, Some(12));
        assert_eq!(response.output_tokens, Some(3));
    }

    #[test]
    fn parse_openai_compat_response_missing_usage_is_none_not_zero() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "hello" } }],
        });
        let response = parse_openai_compat_response("url", &body).unwrap();
        assert_eq!(response.input_tokens, None);
        assert_eq!(response.output_tokens, None);
    }

    #[test]
    fn parse_openai_compat_response_empty_content_is_empty_completion() {
        let body = serde_json::json!({ "choices": [{ "message": { "content": "" } }] });
        let error = parse_openai_compat_response("url", &body).unwrap_err();
        assert!(matches!(error, Error::EmptyCompletion { .. }));
    }

    #[test]
    fn parse_openai_compat_response_missing_choices_is_empty_completion() {
        let body = serde_json::json!({});
        let error = parse_openai_compat_response("url", &body).unwrap_err();
        assert!(matches!(error, Error::EmptyCompletion { .. }));
    }

    #[test]
    fn anthropic_request_body_carries_system_separately() {
        let body = anthropic_request_body(&request(ProviderWire::Anthropic, &test_key()));
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["system"], "system prompt");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "user prompt");
        assert!(body.get("messages").unwrap()[0].get("system").is_none());
    }

    #[test]
    fn anthropic_url_appends_v1_messages() {
        assert_eq!(
            anthropic_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn parse_anthropic_response_extracts_text_and_usage() {
        let body = serde_json::json!({
            "content": [{ "type": "text", "text": "hi" }],
            "usage": { "input_tokens": 7, "output_tokens": 2 },
        });
        let response = parse_anthropic_response("url", &body).unwrap();
        assert_eq!(response.text, "hi");
        assert_eq!(response.input_tokens, Some(7));
        assert_eq!(response.output_tokens, Some(2));
    }

    #[test]
    fn parse_anthropic_response_missing_usage_is_none_not_zero() {
        let body = serde_json::json!({ "content": [{ "type": "text", "text": "hi" }] });
        let response = parse_anthropic_response("url", &body).unwrap();
        assert_eq!(response.input_tokens, None);
        assert_eq!(response.output_tokens, None);
    }

    #[test]
    fn is_transient_retries_connect_errors_and_429_5xx() {
        assert!(is_transient(&Error::Request {
            url: "u".into(),
            message: "connect failed".into(),
        }));
        assert!(is_transient(&Error::Status {
            url: "u".into(),
            status: 429,
            message: "rate limited".into(),
        }));
        assert!(is_transient(&Error::Status {
            url: "u".into(),
            status: 503,
            message: "unavailable".into(),
        }));
    }

    #[test]
    fn is_transient_does_not_retry_client_errors_or_malformed_bodies() {
        assert!(!is_transient(&Error::Status {
            url: "u".into(),
            status: 400,
            message: "bad request".into(),
        }));
        assert!(!is_transient(&Error::Status {
            url: "u".into(),
            status: 401,
            message: "unauthorized".into(),
        }));
        assert!(!is_transient(&Error::InvalidResponse {
            url: "u".into(),
            message: "not json".into(),
        }));
        assert!(!is_transient(&Error::EmptyCompletion { url: "u".into() }));
    }

    #[test]
    fn dispatch_stops_after_max_attempts_on_persistent_transient_failure() {
        // A connect failure against an address nothing listens on is
        // transient by classification; the retry loop must still terminate
        // (not spin forever) once `retries` attempts are exhausted.
        let req = ProviderRequest {
            base_url: "http://127.0.0.1:1",
            wire: ProviderWire::OpenaiCompat,
            model: "m",
            api_key: &test_key(),
            system: None,
            user: "hi",
            max_tokens: 16,
            connect_timeout_ms: 200,
            read_timeout_ms: 200,
            retries: 2,
        };
        let result = dispatch(&req);
        assert!(result.is_err());
    }
}
