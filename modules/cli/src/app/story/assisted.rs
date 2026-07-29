//! P521 ASSISTED story level: an offline LLM pass that summarizes each
//! beat's detailed activity into prose, using the machine's current
//! `[agent.narrator]` seat resolved cold (`warm: None`, no confinement,
//! `exec_dir: None`, see [`crate::app::drive::resolve_offline_narrator_config`]).
//! The ONLY story level that spends a model call, and only on an explicit
//! `--level assisted` request — `Default`/`Detailed` never call [`narrate`].
//!
//! Per-frame failure degrades that beat's own `assisted_prose` to a stated
//! notice rather than failing the whole report; total narrator tokens spent
//! (successes and failures alike) accumulate onto
//! [`StoryReport::assisted_narrator_tokens`].

use ctx_traits_core::procedure::session::Session;
use ctx_traits_core::procedure::story::{self, StoryReport};

use crate::app::drive::resolve_offline_narrator_config;
use crate::app::harness_stream::dispatch_narration;

/// Bound (characters) on the detailed-activity prompt built for one beat —
/// keeps a pathologically chatty frame's narrator prompt small and fast.
const PROMPT_TEXT_BOUND: usize = 4000;

pub(crate) fn narrate(mut report: StoryReport, session: &Session) -> StoryReport {
    if report.beats.is_empty() {
        return report;
    }
    let Some(config) = resolve_offline_narrator_config(
        session.run_id.as_str(),
        session.session_id.as_str(),
        "story-assisted",
    ) else {
        report.assisted_unavailable =
            Some("assisted narration unavailable: no [agent.narrator] seat resolved".to_string());
        return report;
    };

    let mut total_tokens: u64 = 0;
    for beat in &mut report.beats {
        let Some(frame_key) = beat.frame_key.clone() else {
            continue;
        };
        let events = events_for_frame_key(&report.detailed_timeline, &frame_key);
        if events.is_empty() {
            continue;
        }
        let prompt = build_prompt(&frame_key, &events);
        let (result, tokens) = dispatch_narration(&config, prompt);
        total_tokens += tokens;
        beat.assisted_prose = Some(match result {
            Ok(prose) => prose,
            Err(reason) => format!("(assisted narration failed for this step: {reason})"),
        });
    }
    if total_tokens > 0 {
        report.assisted_narrator_tokens = Some(total_tokens);
    }
    report
}

/// Every detailed-timeline event whose `frame_id` matches `frame_key` — the
/// one stitching convention the sidecar writes and [`StoryBeat::frame_key`]
/// exposes (`item_id`-or-title). There is no other key-derivation path here;
/// a beat with no resolved `frame_key` is skipped by the caller before this
/// is ever called (P521 review round 2, blocker
/// `assisted-prose-frame-key-mismatch`).
fn events_for_frame_key<'a>(
    timeline: &'a [story::TimedActivityEvent],
    frame_key: &str,
) -> Vec<&'a story::TimedActivityEvent> {
    timeline
        .iter()
        .filter(|timed| timed.event.frame_id == frame_key)
        .collect()
}

fn build_prompt(frame_key: &str, events: &[&story::TimedActivityEvent]) -> String {
    let mut body = String::new();
    for timed in events {
        let event = &timed.event;
        let text = event.text.as_deref().unwrap_or("");
        body.push_str(&format!("- {:?}: {text}\n", event.kind));
        if body.chars().count() > PROMPT_TEXT_BOUND {
            break;
        }
    }
    let bounded_body: String = body.chars().take(PROMPT_TEXT_BOUND).collect();
    format!(
        "Summarize this ctx.traits run step \"{frame_key}\" in one or two plain-language \
         sentences, strictly from the recorded activity below. Do not invent anything absent \
         from the record.\n\n{bounded_body}"
    )
}

#[cfg(test)]
mod tests {
    use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

    use super::*;

    fn timed(frame_id: &str) -> story::TimedActivityEvent {
        story::TimedActivityEvent {
            at_epoch_ms: 0,
            event: ActivityEvent {
                sequence: 0,
                frame_id: frame_id.to_string(),
                kind: ActivityKind::RunningTool,
                text: Some("did something".to_string()),
                tool: Some("edit".to_string()),
                tokens: None,
            },
        }
    }

    /// A beat whose item_id differs from its title must match on the
    /// item_id-keyed `frame_key`, not a title-or-ref_text re-derivation
    /// (P521 review round 2, blocker `assisted-prose-frame-key-mismatch`).
    #[test]
    fn matches_events_by_frame_key_not_title() {
        let timeline = vec![timed("item-42"), timed("some-other-step")];
        let matched = events_for_frame_key(&timeline, "item-42");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].event.frame_id, "item-42");

        // Matching against the beat's title (a different string than its
        // item_id-derived frame_key) must find nothing — proving the title
        // is not a valid stand-in key.
        let matched_by_title = events_for_frame_key(&timeline, "Human-Readable Title");
        assert!(matched_by_title.is_empty());
    }
}
