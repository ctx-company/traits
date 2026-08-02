//! Tool-less, bounded one-shot helper for the live run ask pane.
//!
//! Only the caller's handed snapshot is sent to the harness. This module does
//! not open ledgers, configure MCP, or expose any mutation/output channel.

use std::collections::BTreeMap;
use std::sync::Arc;

use ctx_traits_io::harness_config::{HarnessCliConvention, HarnessDefinition, ProfileAssignment};

use super::{agent_dispatch, harness_stream};

pub(crate) const SYSTEM_PROMPT: &str = "You are a read-only run guide. Answer only from the handed evidence. If evidence is absent or truncated, say unknown. Do not run tools, propose mutations, or claim hidden state.";
const MAX_EVIDENCE_CHARS: usize = 2_000;
const MAX_QUESTION_CHARS: usize = 1_000;
const MAX_PROMPT_CHARS: usize = 3_200;

#[derive(Clone)]
pub(crate) struct GuideConfig {
    pub(crate) harness: HarnessDefinition,
    pub(crate) cli: HarnessCliConvention,
    pub(crate) assignment: ProfileAssignment,
    pub(crate) timeout_ms: u64,
    pub(crate) exec_dir: Option<camino::Utf8PathBuf>,
    pub(crate) tokens: harness_stream::OneShotTokenTracker,
}

#[derive(Clone)]
pub(crate) struct GuideReply {
    pub(crate) text: String,
}

/// Build a bounded, read-only story projection. It intentionally reads only
/// the activity sidecar and the session snapshot, never the live ledger path.
pub(crate) fn evidence(
    session: &ctx_traits_core::procedure::session::Session,
    plan: &ctx_traits_core::procedure::run::Plan,
    ledger_path: Option<&camino::Utf8Path>,
    current_step: &str,
    statuses: &str,
) -> String {
    fn field(label: &str, value: impl AsRef<str>) -> String {
        let value = bound_text(value.as_ref(), 360);
        format!("{label}: {value}")
    }
    let activity = ledger_path.and_then(crate::app::story::load_activity);
    let story = ctx_traits_core::procedure::story::build(session, Some(plan), activity.as_ref());
    let mut fields = vec![
        field("Current step", current_step),
        field("Sequence statuses", statuses),
    ];
    if story.beats.is_empty() {
        fields.push("Accepted evidence: absent".to_string());
    }
    // Important verdicts and blockers remain available even if newer routine
    // activity would otherwise consume the small evidence budget.
    let mut beats: Vec<_> = story.beats.iter().collect();
    beats.sort_by_key(|beat| {
        let important = beat.gloss.status.is_some() || !beat.gloss.blockers.is_empty();
        !important
    });
    for beat in beats.into_iter().take(6) {
        let mut detail = vec![format!("{} ({})", beat.ref_text, beat.actor)];
        if let Some(status) = &beat.gloss.status {
            detail.push(format!("verdict/status={status}"));
        }
        if !beat.gloss.blockers.is_empty() {
            detail.push(format!("blockers={}", beat.gloss.blockers.join(", ")));
        }
        if !beat.bullets.is_empty() {
            detail.push(format!("activity={}", beat.bullets.join("; ")));
        }
        if !beat.reason.is_empty() {
            detail.push(format!("status={}", beat.reason));
        }
        fields.push(field("Accepted beat", detail.join("; ")));
    }
    match activity {
        None => fields.push("Activity evidence: absent".to_string()),
        Some(input) if input.skipped_lines > 0 => fields.push(format!(
            "Activity evidence: partial ({} unreadable entries)",
            input.skipped_lines
        )),
        Some(_) => fields.push("Activity evidence: recorded".to_string()),
    }
    let text = fields.join("\n");
    let bounded = bound_text(&text, MAX_EVIDENCE_CHARS);
    if bounded.chars().count() < text.chars().count() {
        format!("{bounded}\nEvidence truncated; say unknown when omitted evidence is needed.")
    } else {
        format!("{bounded}\nEvidence is partial; say unknown when it does not answer the question.")
    }
}

pub(crate) fn dispatch(
    config: GuideConfig,
    question: String,
    context: String,
) -> crate::Result<GuideReply> {
    let argv = agent_dispatch::tool_less_standing_agent_argv(
        &config.harness,
        &config.cli,
        config.assignment.model.as_deref(),
        config.assignment.reasoning_effort.as_deref(),
        SYSTEM_PROMPT,
    )?;
    let instructions = if config.cli.system_prompt_flag.is_some() {
        String::new()
    } else {
        format!("{SYSTEM_PROMPT}\n\n")
    };
    let prompt = guide_prompt(&instructions, &context, &question);
    let call_total = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = ctx_traits_io::harness::AttemptTokenAccumulator::new({
        let call_total = Arc::clone(&call_total);
        move |delta| {
            call_total.fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let outcome = agent_dispatch::run_one_shot(agent_dispatch::RunOneShotRequest {
        harness: &config.harness,
        argv,
        prompt,
        prompt_via_stdin: config.cli.prompt_via.as_deref() == Some("stdin"),
        timeout_ms: config.timeout_ms,
        exec_dir: config.exec_dir.as_deref(),
        env_overlay: &BTreeMap::new(),
        observers: agent_dispatch::RunOneShotObservers {
            stdout_observer: Some(counter.observer()),
            ..Default::default()
        },
        sandbox: None,
    });
    counter.finish();
    config
        .tokens
        .end_call(call_total.load(std::sync::atomic::Ordering::Relaxed));
    let outcome = outcome?;
    if outcome.exit_code.unwrap_or(1) != 0
        || outcome.timed_out
        || outcome.killed
        || outcome.stdout_truncated
    {
        return Err(crate::Error::Command {
            message: "guide request did not complete".to_string(),
        });
    }
    let text = response_text(config.cli.output.as_deref(), &outcome.stdout)?;
    Ok(GuideReply { text })
}

fn bound_text(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn guide_prompt(instructions: &str, context: &str, question: &str) -> String {
    let question = bound_text(question, MAX_QUESTION_CHARS);
    // Reserve this safety disclosure before bounding evidence.
    let notice =
        "\nEvidence may be partial or truncated; say unknown when omitted evidence is needed.";
    let prefix = format!("{instructions}Question: {question}\n\nEvidence:\n");
    let room = MAX_PROMPT_CHARS.saturating_sub(prefix.chars().count() + notice.chars().count());
    format!("{prefix}{}{}", bound_text(context, room), notice)
}

fn response_text(output: Option<&str>, stdout: &str) -> crate::Result<String> {
    if output.is_some_and(|id| harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id))
        && harness_stream::stream_values(stdout).is_empty()
    {
        return Err(crate::Error::Command {
            message: "guide response was not valid streaming JSON".to_string(),
        });
    }
    if matches!(output, Some("raw-json" | "claude-json"))
        && serde_json::from_str::<serde_json::Value>(stdout).is_err()
    {
        return Err(crate::Error::Command {
            message: "guide response was not valid JSON".to_string(),
        });
    }
    let text = if matches!(output, Some("raw-json" | "claude-json")) {
        guide_raw_json_text(stdout).unwrap_or_default()
    } else {
        harness_stream::narrator_stdout_text(stdout, output).unwrap_or_default()
    };
    // The narrator parser deliberately falls back to an unknown event's raw
    // bytes for display. A guide answer must instead be an actual response.
    if output.is_some_and(|id| {
        harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id)
            || matches!(id, "raw-json" | "claude-json")
    }) && text.trim() == stdout.trim()
        && stdout.trim_start().starts_with(['{', '['])
    {
        return Err(crate::Error::Command {
            message: "guide response was empty or unparseable".to_string(),
        });
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(crate::Error::Command {
            message: "guide response was empty or unparseable".to_string(),
        });
    }
    Ok(text)
}

/// Raw JSON shells are accepted by the guide convention only. The narrator's
/// stream parser deliberately requires an explicit final `type: result` event.
fn guide_raw_json_text(stdout: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(stdout).ok()? {
        serde_json::Value::String(text) => Some(text),
        value => value
            .get("result")
            .or_else(|| value.get("text"))
            .or_else(|| value.get("content"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_dispatch_parses_final_results_without_reasoning() {
        let stream = "{\"type\":\"thinking\",\"thinking\":\"private\"}\n{\"type\":\"result\",\"result\":\"answer\"}";
        assert_eq!(
            response_text(Some("opencode-json"), stream).unwrap(),
            "answer"
        );
        assert!(response_text(Some("opencode-json"), "{}").is_err());
        assert!(response_text(Some("raw-json"), "not json").is_err());
        assert_eq!(
            response_text(Some("raw-json"), r#"{"result":"raw answer"}"#).unwrap(),
            "raw answer"
        );
        assert_eq!(
            response_text(Some("raw-json"), r#""raw string""#).unwrap(),
            "raw string"
        );
    }

    #[test]
    fn guide_dispatch_parses_supported_streaming_and_raw_conventions() {
        assert_eq!(
            response_text(Some("opencode-json"), "{\"type\":\"text_delta\",\"delta\":\"one\"}\n{\"kind\":\"text\",\"content\":\" two\"}").unwrap(),
            "one  two"
        );
        assert_eq!(
            response_text(Some("claude-json"), r#"{"result":"answer"}"#).unwrap(),
            "answer"
        );
        assert!(response_text(Some("opencode-json"), "{not json").is_err());
        assert!(response_text(Some("raw-json"), r#"{}"#).is_err());
    }

    #[test]
    fn guide_dispatch_prompt_fallback_includes_fixed_instructions() {
        let prompt = guide_prompt(&format!("{SYSTEM_PROMPT}\n\n"), "facts", "question");
        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("Question: question"));
    }

    #[test]
    fn guide_dispatch_rejects_empty_malformed_and_preserves_raw_answers() {
        assert!(response_text(None, "   ").is_err());
        assert!(response_text(Some("claude-json"), "[]").is_err());
        assert_eq!(response_text(None, "plain answer").unwrap(), "plain answer");
        let fallback = guide_prompt(&format!("{SYSTEM_PROMPT}\n\n"), "facts", "why?");
        assert!(fallback.contains(SYSTEM_PROMPT));
        assert!(fallback.contains("Evidence:\nfacts"));
    }

    #[test]
    fn guide_evidence_bounds_untrusted_fields() {
        let source = "x".repeat(2_000);
        assert_eq!(bound_text(&source, 100).chars().count(), 100);
    }

    #[test]
    fn guide_evidence_prompt_has_a_final_bound_and_keeps_question() {
        let prompt = guide_prompt("instructions\n", &"x".repeat(9_000), &"q".repeat(2_000));
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.contains(&format!("Question: {}", "q".repeat(MAX_QUESTION_CHARS))));
        assert!(prompt.contains("say unknown"));
    }

    #[test]
    fn guide_evidence_bounds_question_and_preserves_omission_notice() {
        let prompt = guide_prompt("", "unrelated ledger secret", &"文".repeat(2_000));
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.contains("Evidence may be partial or truncated"));
        assert_eq!(prompt.matches('文').count(), MAX_QUESTION_CHARS);
        // This helper only receives handed evidence; it cannot add exchanges.
        assert!(!prompt.contains("previous guide answer"));
    }

    #[test]
    fn guide_evidence_retains_priority_and_omission_notices_under_global_truncation() {
        let evidence = [
            "Current step: publish",
            "Sequence statuses: publish: Failed; verify: Pending",
            "Accepted beat: review (guide); verdict/status=changes_requested; blockers=missing-proof; activity=touched src/lib.rs; retry 2",
            "Activity evidence: absent",
        ]
        .join("\n");
        let prompt = guide_prompt(
            "",
            &format!("{evidence}\n{}", "x".repeat(10_000)),
            "what failed?",
        );
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.contains("Current step: publish"));
        assert!(prompt.contains("Failed"));
        assert!(prompt.contains("changes_requested"));
        assert!(prompt.contains("missing-proof"));
        assert!(prompt.contains("say unknown"));
        assert!(!prompt.contains("previous question"));
        assert!(!prompt.contains("previous answer"));
    }
}
