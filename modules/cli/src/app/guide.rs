//! Tool-less, bounded one-shot helper for the live run ask pane.
//!
//! Only the caller's handed snapshot is sent to the harness. This module does
//! not open ledgers, configure MCP, or expose any mutation/output channel.

use std::collections::BTreeMap;
use std::sync::Arc;

use ctx_traits_io::harness_config::{HarnessCliConvention, HarnessDefinition, ProfileAssignment};

use super::{agent_dispatch, harness_stream};

pub(crate) const SYSTEM_PROMPT: &str = "You are a read-only run guide. Answer only from the handed evidence. If evidence is absent or truncated, say unknown. Do not run tools, propose mutations, or claim hidden state.";
const MAX_EVIDENCE_CHARS: usize = 5_000;
const MAX_ASSIGNMENT_FIELD_CHARS: usize = 1_200;
const MAX_QUESTION_CHARS: usize = 1_000;
const MAX_PROMPT_CHARS: usize = 8_000;

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

/// One dispatched question: the fresh evidence composed at send, the prior
/// answered exchanges (rendered as a transcript), and the question itself.
#[derive(Clone, Default)]
pub(crate) struct GuideTurn {
    pub(crate) question: String,
    /// Prior answered exchanges, oldest first, one rendered "You: …\nGuide:
    /// …" block per turn — kept as whole turns so the prompt budget can trim
    /// from the front without splitting a turn's question from its answer.
    pub(crate) transcript: Vec<String>,
    pub(crate) evidence: String,
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
    let assignment_value = bound_text(
        &crate::app::run_view::session_text::input_text(session),
        MAX_ASSIGNMENT_FIELD_CHARS,
    );
    let mut fields = vec![
        field("Current step", current_step),
        field("Sequence statuses", statuses),
        format!(
            "Trait: {}",
            bound_text(&plan.trait_id, MAX_ASSIGNMENT_FIELD_CHARS)
        ),
        format!("Run inputs: {assignment_value}"),
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

pub(crate) fn dispatch(config: GuideConfig, turn: GuideTurn) -> crate::Result<GuideReply> {
    let call_total = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = ctx_traits_io::harness::AttemptTokenAccumulator::new({
        let call_total = Arc::clone(&call_total);
        move |delta| {
            call_total.fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let result = (|| {
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
        let prompt = guide_prompt(
            &instructions,
            &turn.evidence,
            &turn.transcript,
            &turn.question,
        );
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
        })?;
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
    })();
    counter.finish();
    config
        .tokens
        .end_call(call_total.load(std::sync::atomic::Ordering::Relaxed));
    result
}

fn bound_text(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// Assembles the guide prompt from instructions, evidence, prior transcript
/// turns and the question. Question and evidence are reserved first; the
/// transcript takes whatever room remains, trimmed oldest-turn-first (whole
/// turns, never mid-turn) so a long conversation cannot starve the evidence
/// that makes an answer correct.
fn guide_prompt(
    instructions: &str,
    evidence: &str,
    transcript: &[String],
    question: &str,
) -> String {
    let question = bound_text(question, MAX_QUESTION_CHARS);
    // Reserve this safety disclosure before bounding evidence.
    let notice =
        "\nEvidence may be partial or truncated; say unknown when omitted evidence is needed.";
    let prefix = format!("{instructions}Question: {question}\n\nEvidence:\n");
    let evidence_room =
        MAX_PROMPT_CHARS.saturating_sub(prefix.chars().count() + notice.chars().count());
    let bounded_evidence = bound_text(evidence, evidence_room);
    let used = prefix.chars().count() + bounded_evidence.chars().count() + notice.chars().count();
    let transcript_room = MAX_PROMPT_CHARS.saturating_sub(used);
    let transcript_section = fit_transcript(transcript, transcript_room);
    format!("{prefix}{bounded_evidence}{transcript_section}{notice}")
}

/// Keeps as many of the most recent whole turns as fit in `room` chars,
/// dropping older turns first. Returns an empty string when nothing fits.
fn fit_transcript(transcript: &[String], room: usize) -> String {
    if transcript.is_empty() || room == 0 {
        return String::new();
    }
    let header = "\n\nPrior conversation:\n";
    let mut kept: Vec<&str> = Vec::new();
    let mut used = header.chars().count();
    for turn in transcript.iter().rev() {
        let addition = turn.chars().count() + 1;
        if used + addition > room {
            break;
        }
        used += addition;
        kept.push(turn.as_str());
    }
    if kept.is_empty() {
        return String::new();
    }
    kept.reverse();
    format!("{header}{}", kept.join("\n"))
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
    use ctx_traits_io::harness_config::{HarnessRegistry, built_in_harness_definition};

    #[test]
    fn guide_tool_less_builtin_claude_is_toolless_and_output_compatible() {
        let harness = built_in_harness_definition("claude-code", &HarnessRegistry::default());
        let cli = harness.cli.as_ref().expect("Claude has a CLI convention");

        let argv =
            agent_dispatch::tool_less_standing_agent_argv(&harness, cli, None, None, SYSTEM_PROMPT)
                .expect("built-in Claude declares a guide convention");

        assert!(
            !argv
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
        assert_eq!(
            argv.windows(3)
                .find(|window| window[0] == "--disallowedTools")
                .expect("all built-in tools are denied"),
            ["--disallowedTools", "*", "mcp__*"]
        );
        assert_eq!(
            argv.windows(3)
                .find(|window| window[0] == "--output-format")
                .expect("guide output matches its parser"),
            ["--output-format", "stream-json", "--verbose"]
        );
        assert_eq!(
            response_text(
                cli.output.as_deref(),
                r#"{"type":"result","result":"final answer"}"#,
            )
            .unwrap(),
            "final answer"
        );
    }

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
        let prompt = guide_prompt(&format!("{SYSTEM_PROMPT}\n\n"), "facts", &[], "question");
        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("Question: question"));
    }

    #[test]
    fn guide_dispatch_rejects_empty_malformed_and_preserves_raw_answers() {
        assert!(response_text(None, "   ").is_err());
        assert!(response_text(Some("claude-json"), "[]").is_err());
        assert_eq!(response_text(None, "plain answer").unwrap(), "plain answer");
        let fallback = guide_prompt(&format!("{SYSTEM_PROMPT}\n\n"), "facts", &[], "why?");
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
        let prompt = guide_prompt(
            "instructions\n",
            &"x".repeat(9_000),
            &[],
            &"q".repeat(2_000),
        );
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.contains(&format!("Question: {}", "q".repeat(MAX_QUESTION_CHARS))));
        assert!(prompt.contains("say unknown"));
    }

    #[test]
    fn guide_evidence_bounds_question_and_preserves_omission_notice() {
        let prompt = guide_prompt("", "unrelated ledger secret", &[], &"文".repeat(2_000));
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
            &[],
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

    #[test]
    fn guide_prompt_carries_transcript_trimmed_oldest_first_without_starving_evidence() {
        let transcript: Vec<String> = (0..50)
            .map(|i| format!("You: q{i}\nGuide: {}", "a".repeat(200)))
            .collect();
        let prompt = guide_prompt("", "Current step: publish", &transcript, "what now?");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.contains("Current step: publish"));
        assert!(prompt.contains("Prior conversation:"));
        // Oldest turns are the first to be dropped.
        assert!(!prompt.contains("You: q0\n"));
        assert!(prompt.contains(&format!("You: q{}\n", transcript.len() - 1)));
    }

    #[test]
    fn guide_prompt_omits_transcript_section_when_empty() {
        let prompt = guide_prompt("", "evidence", &[], "question");
        assert!(!prompt.contains("Prior conversation:"));
    }

    #[test]
    fn guide_evidence_fully_populated_shape_fits_the_budget() {
        // Worst case: every field maxed at its own bound plus six maxed
        // beats — the same shape `evidence` assembles, sized without needing
        // a full `Session`/`Plan`/story fixture.
        let mut fields = vec![
            "Current step: ".to_string() + &"x".repeat(360),
            "Sequence statuses: ".to_string() + &"x".repeat(360),
            "Trait: ".to_string() + &"x".repeat(MAX_ASSIGNMENT_FIELD_CHARS),
            "Run inputs: ".to_string() + &"x".repeat(MAX_ASSIGNMENT_FIELD_CHARS),
        ];
        for _ in 0..6 {
            fields.push("Accepted beat: ".to_string() + &"x".repeat(360));
        }
        fields.push("Activity evidence: recorded".to_string());
        let text = bound_text(&fields.join("\n"), MAX_EVIDENCE_CHARS);
        assert!(text.chars().count() <= MAX_EVIDENCE_CHARS);
        // The budget must be sized to hold at least the assignment plus a
        // couple of status lines and beats without truncating them away.
        assert!(text.contains("Trait: "));
        assert!(text.contains("Run inputs: "));
        assert!(text.contains("Current step: "));
    }
}
