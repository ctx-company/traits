//! gpui-free model for grammar rule 6's screen header: plain Rust, no gpui
//! types, mirroring `preview.rs`'s model/view split. `screen_header_view.rs`
//! paints exactly this model.
//!
//! Every fact rendered here comes from the one accepted
//! [`crate::detail::PreviewState`] — no second claimed-task request, model,
//! subscription or cache, and no read of `CenterFace`/the filesystem/the
//! cwd. The title is composed from `baseline.claimed_task` alone (never
//! `baseline.row.title` or `RunSummary.task_key`); the summary's description
//! leaf is composed from the selection's live-refreshed raw session title
//! alone (never the desktop's own sidecar `session_title` embellishment).

use crate::detail::PreviewState;
use crate::placeholders;
use crate::preview::{ClaimedTaskWording, claimed_task_wording, staleness_word};

/// The header's two rendered leaves. Two owned `String`s, no tone/colour/
/// enum field — `0267.2`/`0268.2`/`0269.2` all describe passing one
/// presented title and one already-composed summary, and the shared
/// component must not grow a Sessions-shaped parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenHeader {
    pub title: String,
    pub summary: String,
}

fn pending_header() -> ScreenHeader {
    ScreenHeader {
        title: "loading".to_string(),
        summary: placeholders::sessions_summary(None, None),
    }
}

fn failed_header(reason: &str) -> ScreenHeader {
    let title = if reason.is_empty() {
        "unreadable".to_string()
    } else {
        format!("unreadable \u{b7} {reason}")
    };
    ScreenHeader {
        title,
        summary: placeholders::sessions_summary(None, None),
    }
}

fn title_from_claimed_task(
    claimed_task: &Result<ctx_traits_io::center::ClaimedTaskResult, String>,
) -> String {
    match claimed_task_wording(claimed_task) {
        ClaimedTaskWording::Task(task) => format!("{} \u{2014} {}", task.key, task.title),
        ClaimedTaskWording::Wording { text, .. } => text.to_string(),
    }
}

/// The Sessions screen's composer: `pub fn sessions_header(state: ...)`
/// matches the `sessions_run_block`/`sessions_footer` signature convention
/// (`preview.rs:227,257`). `counter` is the 0265.14 `frame N of M` value,
/// derived once by the caller (`shell.rs`) and passed straight into the
/// already-reserved `placeholders::sessions_summary(_, counter)` slot; the
/// `Loading`/`Failed` arms keep passing `None`.
pub fn sessions_header(state: Option<&PreviewState<'_>>, counter: Option<&str>) -> ScreenHeader {
    match state {
        None | Some(PreviewState::Loading) => pending_header(),
        Some(PreviewState::Failed(reason)) => failed_header(reason),
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
            session_title,
            ..
        }) => {
            let mut title = title_from_claimed_task(&baseline.claimed_task);
            if let Some(word) = staleness_word(*stale, *refreshing) {
                title = format!("{title} \u{b7} {word}");
            }
            let summary = placeholders::sessions_summary(*session_title, counter);
            ScreenHeader { title, summary }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::TaskStatus;
    use ctx_traits_core::task::provider::ClaimedTask;
    use ctx_traits_io::center::ClaimedTaskResult;

    use crate::detail::DetailBaseline;
    use crate::run_row::{RowState, RunRow};

    fn fixture_row(session_title: Option<&str>) -> RunRow {
        RunRow {
            ledger_path: "/repo/run-1.json".to_string(),
            session_id: "session".to_string(),
            run_id: "run-1".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: "run-1".to_string(),
            session_title: session_title.map(str::to_string),
            trait_id: "implement-phase".to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: String::new(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 10,
            started_at_epoch: None,
        }
    }

    fn fixture_session() -> serde_json::Value {
        serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session",
            "run-id": "run-1",
            "trait-id": "implement-phase",
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "screen-header-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": "run-1",
                "trait-id": "implement-phase",
                "current-run-index": 0,
                "final-state": "completed",
            },
            "state-digest": "sha256:fixture",
        })
    }

    fn baseline(
        claimed_task: Result<ClaimedTaskResult, String>,
        session_title: Option<&str>,
    ) -> DetailBaseline {
        DetailBaseline {
            session: serde_json::from_value(fixture_session()).expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant: Ok(None),
            claimed_task,
            progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            row: fixture_row(session_title),
        }
    }

    fn accepted<'a>(
        baseline: &'a DetailBaseline,
        session_title: Option<&'a str>,
    ) -> PreviewState<'a> {
        PreviewState::Accepted {
            baseline,
            stale: None,
            refreshing: false,
            live: baseline.row.live,
            session_title,
        }
    }

    #[test]
    fn title_carries_the_served_task_answer_s_key_and_title() {
        let task = ClaimedTask {
            key: "0265.13".to_string(),
            title: "render the sessions header".to_string(),
            description: String::new(),
            stored_status: Some(TaskStatus::Ready),
            auto_close: None,
        };
        let baseline = baseline(Ok(ClaimedTaskResult::Task(Box::new(task))), None);
        let state = accepted(&baseline, None);
        let header = sessions_header(Some(&state), None);
        assert_eq!(header.title, "0265.13 \u{2014} render the sessions header");
    }

    #[test]
    fn every_non_success_claimed_task_outcome_renders_its_own_visible_title() {
        for (outcome, expected) in [
            (ClaimedTaskResult::Unclaimed, "no task claimed"),
            (ClaimedTaskResult::Missing, "task unavailable: missing"),
            (
                ClaimedTaskResult::Ambiguous(vec!["a".to_string()]),
                "task unavailable: ambiguous session",
            ),
        ] {
            let baseline = baseline(Ok(outcome), None);
            let state = accepted(&baseline, None);
            let header = sessions_header(Some(&state), None);
            assert_eq!(header.title, expected);
        }

        let failed = baseline(Err("center down".to_string()), None);
        let state = accepted(&failed, None);
        let header = sessions_header(Some(&state), None);
        assert_eq!(header.title, "task unavailable: center error");
    }

    #[test]
    fn present_description_carries_through_the_summary() {
        let baseline = baseline(Ok(ClaimedTaskResult::Unclaimed), Some("review the plan"));
        let state = accepted(&baseline, Some("review the plan"));
        let header = sessions_header(Some(&state), None);
        assert_eq!(header.summary, "review the plan");
    }

    #[test]
    fn absent_description_renders_one_non_empty_module_phrased_summary() {
        let baseline = baseline(Ok(ClaimedTaskResult::Unclaimed), None);
        let state = accepted(&baseline, None);
        let header = sessions_header(Some(&state), None);
        assert!(!header.summary.is_empty());
        assert_eq!(header.summary, "no run description yet");
    }

    #[test]
    fn none_loading_and_failed_states_render_visible_non_fabricated_text() {
        assert_eq!(sessions_header(None, None).title, "loading");
        assert_eq!(
            sessions_header(Some(&PreviewState::Loading), None).title,
            "loading"
        );
        let failed = sessions_header(Some(&PreviewState::Failed("bad json")), None);
        assert!(failed.title.contains("bad json"));
        assert!(!failed.title.is_empty());
        assert!(!failed.summary.is_empty());
    }

    #[test]
    fn the_frame_counter_reaches_the_summary_through_the_reserved_slot() {
        let baseline = baseline(Ok(ClaimedTaskResult::Unclaimed), Some("review the plan"));
        let state = accepted(&baseline, Some("review the plan"));
        let header = sessions_header(Some(&state), Some("frame 2 of 5"));
        assert_eq!(header.summary, "review the plan \u{b7} frame 2 of 5");
        assert_eq!(header.summary.matches('\u{b7}').count(), 1);
    }

    #[test]
    fn loading_and_failed_headers_never_render_a_counter_even_when_supplied() {
        assert_eq!(
            sessions_header(Some(&PreviewState::Loading), Some("frame 2 of 5")).summary,
            "no run description yet"
        );
        assert_eq!(
            sessions_header(
                Some(&PreviewState::Failed("bad json")),
                Some("frame 2 of 5")
            )
            .summary,
            "no run description yet"
        );
    }

    #[test]
    fn stale_and_refreshing_postures_render_a_visible_marker_on_the_title() {
        let baseline = baseline(Ok(ClaimedTaskResult::Unclaimed), None);
        let current = accepted(&baseline, None);
        let refreshing = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: true,
            live: baseline.row.live,
            session_title: None,
        };
        let stale = PreviewState::Accepted {
            baseline: &baseline,
            stale: Some("subscription closed"),
            refreshing: true,
            live: baseline.row.live,
            session_title: None,
        };

        let current_header = sessions_header(Some(&current), None);
        let refreshing_header = sessions_header(Some(&refreshing), None);
        let stale_header = sessions_header(Some(&stale), None);

        assert_ne!(refreshing_header.title, current_header.title);
        assert!(refreshing_header.title.contains("refreshing"));
        assert_ne!(stale_header.title, current_header.title);
        assert!(stale_header.title.contains("stale"));
    }
}
