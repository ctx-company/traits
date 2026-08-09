//! Native harness streams normalized at the IO boundary.
//!
//! This module deliberately recognizes only documented event shapes. Unknown
//! input produces no inferred activity, and tool result payloads are ignored.

use std::collections::BTreeMap;

use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind, RateLimitObservation};
use serde_json::Value;

const MAX_TEXT_CHARS: usize = 512;
const MAX_TOOL_INPUT_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessActivityAdapterKind {
    ClaudeCode,
    OpenCode,
    Pi,
    Codex,
}

impl HarnessActivityAdapterKind {
    pub fn from_harness_kind(kind: &str) -> Option<Self> {
        match kind {
            "claude-code" => Some(Self::ClaudeCode),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

/// Stateful because native NDJSON and Claude tool input both cross chunks.
pub struct HarnessActivityAdapter {
    kind: HarnessActivityAdapterKind,
    frame_id: String,
    sequence: u64,
    line_buffer: String,
    tool_inputs: BTreeMap<u64, ToolInput>,
}

struct ToolInput {
    name: String,
    input: String,
}

impl HarnessActivityAdapter {
    pub fn new(kind: HarnessActivityAdapterKind, frame_id: impl Into<String>) -> Self {
        Self {
            kind,
            frame_id: frame_id.into(),
            sequence: 0,
            line_buffer: String::new(),
            tool_inputs: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<ActivityEvent> {
        self.line_buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(newline) = self.line_buffer.find('\n') {
            let line = self.line_buffer[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.line_buffer.drain(..=newline);
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                self.map_value(&value, &mut events);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Vec<ActivityEvent> {
        let trailing = std::mem::take(&mut self.line_buffer);
        if trailing.trim().is_empty() {
            Vec::new()
        } else {
            self.push(format!("{trailing}\n").as_bytes())
        }
    }

    fn map_value(&mut self, value: &Value, events: &mut Vec<ActivityEvent>) {
        match self.kind {
            HarnessActivityAdapterKind::ClaudeCode => self.map_claude(value, events),
            HarnessActivityAdapterKind::OpenCode => self.map_common(value, events, true),
            HarnessActivityAdapterKind::Pi | HarnessActivityAdapterKind::Codex => {
                self.map_common(value, events, false)
            }
        }
    }

    fn map_claude(&mut self, value: &Value, events: &mut Vec<ActivityEvent>) {
        let event = value.get("event").unwrap_or(value);
        let kind = event.get("type").and_then(Value::as_str);
        match kind {
            Some("content_block_delta") => {
                let delta = event.get("delta").unwrap_or(event);
                match delta.get("type").and_then(Value::as_str) {
                    Some("thinking_delta") => self.emit_text_with_tokens(
                        events,
                        ActivityKind::Thinking,
                        delta.get("thinking").and_then(Value::as_str),
                    ),
                    Some("text_delta") => self.emit_text(
                        events,
                        ActivityKind::StreamingOutput,
                        delta.get("text").and_then(Value::as_str),
                    ),
                    Some("input_json_delta") => {
                        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                        if let Some(input) = self.tool_inputs.get_mut(&index) {
                            append_bounded(
                                &mut input.input,
                                delta
                                    .get("partial_json")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                                MAX_TOOL_INPUT_CHARS,
                            );
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let block = event.get("content_block").unwrap_or(event);
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    self.tool_inputs.insert(
                        index,
                        ToolInput {
                            name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string(),
                            input: String::new(),
                        },
                    );
                }
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(input) = self.tool_inputs.remove(&index) {
                    self.emit_tool(events, input.name, input.input);
                }
            }
            Some("compaction") | Some("compact") => {
                self.emit(events, ActivityKind::Compacting, None, None, None)
            }
            Some("rate_limit_event") => {
                if let Some(observation) = event
                    .get("rate_limit_info")
                    .and_then(RateLimitObservation::decode)
                {
                    self.emit_rate_limit(events, observation);
                }
            }
            _ => {}
        }
    }

    fn map_common(&mut self, value: &Value, events: &mut Vec<ActivityEvent>, opencode: bool) {
        let kind = value
            .get("type")
            .or_else(|| value.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind.contains("result") || kind.contains("tool_result") {
            return;
        }
        if kind.contains("compact") {
            self.emit(events, ActivityKind::Compacting, None, None, None);
        } else if kind.contains("reasoning") || kind.contains("thinking") {
            self.emit_text_with_tokens(events, ActivityKind::Thinking, text_from(value));
        } else if kind.contains("text") || kind.contains("message") || kind.contains("output") {
            self.emit_text(events, ActivityKind::StreamingOutput, text_from(value));
        } else if kind.contains("tool") {
            let tool = value
                .get("name")
                .or_else(|| value.pointer("/part/tool/name"))
                .and_then(Value::as_str)
                .unwrap_or(if opencode { "tool" } else { "tool-call" });
            let input = value
                .get("input")
                .or_else(|| value.pointer("/part/state/input"))
                .map(salient_value);
            self.emit(
                events,
                ActivityKind::RunningTool,
                input,
                Some(tool.to_string()),
                None,
            );
        }
    }

    fn emit_text(
        &mut self,
        events: &mut Vec<ActivityEvent>,
        kind: ActivityKind,
        text: Option<&str>,
    ) {
        self.emit(events, kind, text.map(bounded_text), None, None);
    }

    /// Like [`Self::emit_text`], but also stamps an estimated token count
    /// (chars/4, the same rough ratio the token counters elsewhere in the IO
    /// boundary use for a live estimate) — used only for `Thinking` deltas,
    /// where the raw text length is a reasonable proxy and the sidecar wants
    /// a per-frame thinking-token total (P521).
    fn emit_text_with_tokens(
        &mut self,
        events: &mut Vec<ActivityEvent>,
        kind: ActivityKind,
        text: Option<&str>,
    ) {
        let tokens = text
            .filter(|t| !t.is_empty())
            .map(|t| (t.chars().count() as u64).div_ceil(4).max(1));
        self.emit(events, kind, text.map(bounded_text), None, tokens);
    }

    fn emit_tool(&mut self, events: &mut Vec<ActivityEvent>, tool: String, input: String) {
        self.emit(
            events,
            ActivityKind::RunningTool,
            (!input.is_empty()).then(|| bounded_text(&input)),
            Some(tool),
            None,
        );
    }

    fn emit(
        &mut self,
        events: &mut Vec<ActivityEvent>,
        kind: ActivityKind,
        text: Option<String>,
        tool: Option<String>,
        tokens: Option<u64>,
    ) {
        self.sequence += 1;
        events.push(ActivityEvent {
            sequence: self.sequence,
            frame_id: self.frame_id.clone(),
            kind,
            text,
            tool,
            tokens,
            rate_limit: None,
        });
    }

    /// Emit a decoded `rate_limit_event` observation (P556/0117). The human
    /// text names the absolute reset instant (an epoch, never a duration) so
    /// live surfaces never render a stale "resets in Xh" that drifts as the
    /// event ages.
    fn emit_rate_limit(
        &mut self,
        events: &mut Vec<ActivityEvent>,
        observation: RateLimitObservation,
    ) {
        self.sequence += 1;
        let limit_type = observation.limit_type.as_deref().unwrap_or("unknown");
        let text = format!(
            "rate limit {} ({limit_type}) — resets at epoch {}",
            observation.status, observation.resets_at_epoch
        );
        events.push(ActivityEvent {
            sequence: self.sequence,
            frame_id: self.frame_id.clone(),
            kind: ActivityKind::RateLimited,
            text: Some(text),
            tool: None,
            tokens: None,
            rate_limit: Some(observation),
        });
    }
}

fn text_from(value: &Value) -> Option<&str> {
    ["text", "delta", "content", "reasoning", "thinking"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
}

fn salient_value(value: &Value) -> String {
    match value {
        Value::Object(object) => object
            .iter()
            .next()
            .map(|(key, value)| format!("{key}={}", bounded_text(&value.to_string())))
            .unwrap_or_default(),
        _ => bounded_text(&value.to_string()),
    }
}

fn append_bounded(target: &mut String, text: &str, cap: usize) {
    if target.chars().count() < cap {
        target.extend(text.chars().take(cap - target.chars().count()));
    }
}

fn bounded_text(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_reassembles_tool_input_and_ignores_results() {
        let mut adapter =
            HarnessActivityAdapter::new(HarnessActivityAdapterKind::ClaudeCode, "frame");
        let first = br#"{"event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","name":"read"}}}
{"event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}}
"#;
        assert!(adapter.push(first).is_empty());
        let events = adapter.push(br#"{"event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}}
{"event":{"type":"content_block_stop","index":0}}
{"type":"user","content":{"type":"tool_result","content":"secret"}}
"#);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::RunningTool);
        assert_eq!(events[0].text.as_deref(), Some("{\"path\":\"a\"}"));
    }

    #[test]
    fn provider_streams_share_the_normalized_vocabulary() {
        let mut claude =
            HarnessActivityAdapter::new(HarnessActivityAdapterKind::ClaudeCode, "frame");
        let claude_events = claude.push(
            br#"{"event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"plan"}}}
{"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"answer"}}}
{"event":{"type":"compaction"}}
"#,
        );
        let mut opencode =
            HarnessActivityAdapter::new(HarnessActivityAdapterKind::OpenCode, "frame");
        let opencode_events = opencode.push(
            br#"{"type":"reasoning","text":"plan"}
{"type":"text","text":"answer"}
{"type":"compaction"}
"#,
        );
        assert_eq!(
            claude_events
                .iter()
                .map(|event| &event.kind)
                .collect::<Vec<_>>(),
            opencode_events
                .iter()
                .map(|event| &event.kind)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pi_and_codex_map_without_reading_results() {
        for kind in [
            HarnessActivityAdapterKind::Pi,
            HarnessActivityAdapterKind::Codex,
        ] {
            let mut adapter = HarnessActivityAdapter::new(kind, "frame");
            let events = adapter.push(
                br#"{"type":"tool_call","name":"read","input":{"path":"a"}}
{"type":"tool_result","content":"do not retain"}
"#,
            );
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].kind, ActivityKind::RunningTool);
            assert!(
                !events[0]
                    .text
                    .as_deref()
                    .unwrap_or_default()
                    .contains("retain")
            );
        }
    }

    #[test]
    fn claude_decodes_a_rejected_rate_limit_event_split_across_chunks() {
        let mut adapter =
            HarnessActivityAdapter::new(HarnessActivityAdapterKind::ClaudeCode, "frame");
        let first = br#"{"type":"rate_limit_event","rate_limit_info":{"status":"rej"#;
        let second = br#"ected","resetsAt":1784836800,"rateLimitType":"five_hour"},"uuid":"u1","session_id":"s1"}
"#;
        assert!(adapter.push(first).is_empty());
        let events = adapter.push(second);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::RateLimited);
        let observation = events[0].rate_limit.as_ref().expect("rate_limit set");
        assert_eq!(observation.status, "rejected");
        assert_eq!(observation.limit_type.as_deref(), Some("five_hour"));
        assert_eq!(observation.resets_at_epoch, 1_784_836_800);
        assert!(
            events[0]
                .text
                .as_deref()
                .unwrap_or_default()
                .contains("1784836800")
        );
        assert!(
            !events[0]
                .text
                .as_deref()
                .unwrap_or_default()
                .contains("in ")
        );
    }

    #[test]
    fn claude_tolerates_a_rate_limit_event_with_no_rate_limit_type() {
        let mut adapter =
            HarnessActivityAdapter::new(HarnessActivityAdapterKind::ClaudeCode, "frame");
        let events = adapter.push(
            br#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":100}}
"#,
        );
        assert_eq!(events.len(), 1);
        let observation = events[0].rate_limit.as_ref().expect("rate_limit set");
        assert_eq!(observation.limit_type, None);
    }
}
