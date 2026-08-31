//! The Sessions preview run block and footer: plain Rust, no gpui types, so
//! the content contract is assertable in plain `#[test]`s here, mirroring
//! `bottom_bar.rs`'s model/view split. `preview_view.rs` paints exactly this
//! model.
//!
//! Every identity/state fact rendered here (`run_id`, `trait_id`,
//! `elapsed_seconds`, `started_at_epoch`, the variant, the task answer) comes
//! from the one accepted `DetailBaseline` a [`crate::detail::PreviewState`]
//! carries — specifically from `DetailBaseline::row`, the exact `RunRow`
//! candidate its `LoadRequest` was issued for and committed alongside, never
//! from a `RunRow` read separately out of `CenterFace`. That is what keeps a
//! resync's new run/elapsed from ever being painted beside a stale
//! variant/task (review-verdict-1 blocker `selected-preview-not-atomic`).

use std::time::Duration;

use ctx_traits_core::procedure::activity::compact_elapsed_text;
use ctx_traits_io::center::ClaimedTaskResult;

use crate::detail::{DetailBaseline, PreviewState};
use crate::run_row::{RowState, RunRow, StateRole, task_status_presentation};

/// A value segment with its own tone. `None` marks "identity, deliberately
/// not a state" (rule 7's actual distinction) — resolved to `tokens::TEXT`
/// by the view; `Some(role)` resolves through the existing `role_color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueSegment {
    pub text: String,
    pub role: Option<StateRole>,
}

impl ValueSegment {
    fn neutral(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: None,
        }
    }

    fn toned(text: impl Into<String>, role: StateRole) -> Self {
        Self {
            text: text.into(),
            role: Some(role),
        }
    }

    /// The one separator glyph the design grammar allows, with ASCII spaces
    /// on both sides so concatenating a row's segment texts reads
    /// `identity · value`, never `identity·value`.
    fn dot() -> Self {
        Self::neutral(" \u{b7} ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValueRow {
    pub key: String,
    pub value: Vec<ValueSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedBlock {
    pub heading: String,
    pub rows: Vec<KeyValueRow>,
}

fn trait_row(row: &RunRow, baseline: &DetailBaseline) -> KeyValueRow {
    let mut value = vec![ValueSegment::neutral(row.trait_id.clone())];
    match &baseline.variant {
        Ok(Some(variant)) => {
            value.push(ValueSegment::dot());
            value.push(ValueSegment::neutral(variant.clone()));
        }
        Ok(None) => {}
        Err(_reason) => {
            value.push(ValueSegment::dot());
            value.push(ValueSegment::toned("variant unresolved", StateRole::Danger));
        }
    }
    KeyValueRow {
        key: "trait".to_string(),
        value,
    }
}

fn run_row_row(row: &RunRow) -> KeyValueRow {
    let elapsed = compact_elapsed_text(Duration::from_secs(row.elapsed_seconds));
    KeyValueRow {
        key: "run".to_string(),
        value: vec![
            ValueSegment::neutral(row.run_id.clone()),
            ValueSegment::dot(),
            ValueSegment::neutral(elapsed),
        ],
    }
}

fn task_row(baseline: &DetailBaseline) -> KeyValueRow {
    let value = match &baseline.claimed_task {
        Ok(ClaimedTaskResult::Task(task)) => {
            let mut value = vec![ValueSegment::neutral(task.key.clone())];
            if let Some(status) = task.stored_status {
                let presented = task_status_presentation(status);
                value.push(ValueSegment::dot());
                value.push(ValueSegment::toned(presented.word, presented.role));
            }
            value
        }
        Ok(ClaimedTaskResult::Unclaimed) => vec![ValueSegment::neutral("no task claimed")],
        Ok(ClaimedTaskResult::Missing) => {
            vec![ValueSegment::toned(
                "task unavailable: missing",
                StateRole::Danger,
            )]
        }
        Ok(ClaimedTaskResult::Ambiguous(_)) => vec![ValueSegment::toned(
            "task unavailable: ambiguous session",
            StateRole::Danger,
        )],
        Err(_reason) => vec![ValueSegment::toned(
            "task unavailable: center error",
            StateRole::Danger,
        )],
    };
    KeyValueRow {
        key: "task".to_string(),
        value,
    }
}

fn pending_block() -> NamedBlock {
    let pending = vec![ValueSegment::neutral("loading")];
    NamedBlock {
        heading: "run".to_string(),
        rows: vec![
            KeyValueRow {
                key: "trait".to_string(),
                value: pending.clone(),
            },
            KeyValueRow {
                key: "run".to_string(),
                value: pending.clone(),
            },
            KeyValueRow {
                key: "task".to_string(),
                value: pending,
            },
        ],
    }
}

fn unreadable_rows(text: String) -> Vec<KeyValueRow> {
    let unreadable = vec![ValueSegment::toned(text, StateRole::Danger)];
    vec![
        KeyValueRow {
            key: "trait".to_string(),
            value: unreadable.clone(),
        },
        KeyValueRow {
            key: "run".to_string(),
            value: unreadable.clone(),
        },
        KeyValueRow {
            key: "task".to_string(),
            value: unreadable,
        },
    ]
}

fn failed_block(reason: &str) -> NamedBlock {
    let text = if reason.is_empty() {
        "unreadable".to_string()
    } else {
        format!("unreadable \u{b7} {reason}")
    };
    NamedBlock {
        heading: "run".to_string(),
        rows: unreadable_rows(text),
    }
}

/// Goal 8: a projected row already flagged `RowState::Unreadable` (derived
/// from the ledger's own `parse_error`) renders unreadable — no `0s`, no
/// empty identity, no invented task — even though the ledger read itself
/// succeeded well enough to produce an accepted `DetailBaseline`.
fn unreadable_row_block(row: &RunRow) -> NamedBlock {
    let text = if row.detail_text.is_empty() {
        "unreadable".to_string()
    } else {
        format!("unreadable \u{b7} {}", row.detail_text)
    };
    NamedBlock {
        heading: "run".to_string(),
        rows: unreadable_rows(text),
    }
}

/// Compose the `run` identity block from the one accepted preview
/// projection. `Loading`/`Failed` render their own honest, non-fabricated
/// shape; only `Accepted` reaches into the baseline for trait/run/task —
/// and every one of those three rows reads the same baseline, so they can
/// never disagree about which run they describe.
/// The heading carries the posture marker: a settled, current baseline reads
/// plain `run`; a resync in flight (fingerprint move, or recovery from a
/// prior outage) reads `run · refreshing` even though `baseline` still shows
/// the last-accepted facts; a retained baseline behind a lost connection
/// reads `run · stale` — never silently identical to current (review-
/// verdict-1 blocker `selected-preview-not-atomic`).
fn accepted_heading(stale: Option<&str>, refreshing: bool) -> String {
    if stale.is_some() {
        "run \u{b7} stale".to_string()
    } else if refreshing {
        "run \u{b7} refreshing".to_string()
    } else {
        "run".to_string()
    }
}

pub fn sessions_run_block(state: Option<&PreviewState<'_>>) -> NamedBlock {
    match state {
        None | Some(PreviewState::Loading) => pending_block(),
        Some(PreviewState::Failed(reason)) => failed_block(reason),
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
        }) => {
            if baseline.row.state == RowState::Unreadable {
                return unreadable_row_block(&baseline.row);
            }
            NamedBlock {
                heading: accepted_heading(*stale, *refreshing),
                rows: vec![
                    trait_row(&baseline.row, baseline),
                    run_row_row(&baseline.row),
                    task_row(baseline),
                ],
            }
        }
    }
}

/// The footer text: `<run id> · started HH:MM`, or just the run id when
/// `started_at_epoch` is `None` — no epoch zero, no current clock, no
/// dangling separator. `Loading`/`Failed` render their own non-empty,
/// non-fabricated text rather than an empty string standing in for "nothing
/// to show".
pub fn sessions_footer(state: Option<&PreviewState<'_>>) -> String {
    match state {
        None | Some(PreviewState::Loading) => "loading".to_string(),
        Some(PreviewState::Failed(reason)) => {
            if reason.is_empty() {
                "unreadable".to_string()
            } else {
                format!("unreadable \u{b7} {reason}")
            }
        }
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
        }) => {
            if baseline.row.state == RowState::Unreadable {
                let text = if baseline.row.detail_text.is_empty() {
                    "unreadable".to_string()
                } else {
                    format!("unreadable \u{b7} {}", baseline.row.detail_text)
                };
                return text;
            }
            let base = match baseline.row.started_at_epoch {
                Some(epoch) => {
                    let offset = ctx_traits_io::clock::local_utc_offset_seconds(epoch);
                    let clock = ctx_traits_io::clock::epoch_clock_minutes(epoch, offset);
                    format!("{} \u{b7} started {clock}", baseline.row.run_id)
                }
                None => baseline.row.run_id.clone(),
            };
            if stale.is_some() {
                format!("{base} \u{b7} stale")
            } else if *refreshing {
                format!("{base} \u{b7} refreshing")
            } else {
                base
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::TaskStatus;
    use ctx_traits_core::task::provider::ClaimedTask;

    fn fixture_session(run_id: &str, trait_id: &str, elapsed_seconds: u64) -> serde_json::Value {
        serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session",
            "run-id": run_id,
            "trait-id": trait_id,
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "preview-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": run_id,
                "trait-id": trait_id,
                "current-run-index": 0,
                "final-state": "completed",
                "elapsed-seconds": elapsed_seconds,
            },
            "state-digest": "sha256:fixture",
        })
    }

    fn baseline_with(
        variant: Result<Option<String>, String>,
        claimed_task: Result<ClaimedTaskResult, String>,
    ) -> DetailBaseline {
        baseline_with_run("run-1", "implement-phase", 2_520, variant, claimed_task)
    }

    fn fixture_row(run_id: &str, trait_id: &str, elapsed_seconds: u64) -> RunRow {
        RunRow {
            ledger_path: format!("/repo/{run_id}.json"),
            session_id: "session".to_string(),
            run_id: run_id.to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: run_id.to_string(),
            trait_id: trait_id.to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: String::new(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds,
            started_at_epoch: None,
        }
    }

    fn baseline_with_run(
        run_id: &str,
        trait_id: &str,
        elapsed_seconds: u64,
        variant: Result<Option<String>, String>,
        claimed_task: Result<ClaimedTaskResult, String>,
    ) -> DetailBaseline {
        DetailBaseline {
            session: serde_json::from_value(fixture_session(run_id, trait_id, elapsed_seconds))
                .expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant,
            claimed_task,
            row: fixture_row(run_id, trait_id, elapsed_seconds),
        }
    }

    fn accepted(baseline: &DetailBaseline) -> PreviewState<'_> {
        PreviewState::Accepted {
            baseline,
            stale: None,
            refreshing: false,
        }
    }

    #[test]
    fn only_the_state_segment_carries_a_role_never_identity_or_the_dot() {
        let task = ClaimedTask {
            key: "0265.10".to_string(),
            title: "title".to_string(),
            description: String::new(),
            stored_status: Some(TaskStatus::Done),
            auto_close: None,
        };
        let baseline = baseline_with(
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Task(Box::new(task))),
        );
        let state = accepted(&baseline);
        let block = sessions_run_block(Some(&state));
        let task_row = &block.rows[2];
        assert_eq!(task_row.value[0].role, None, "the key/task-id is neutral");
        assert_eq!(task_row.value[1].role, None, "the dot is neutral");
        assert_eq!(
            task_row.value[2].role,
            Some(StateRole::Ok),
            "the state word carries the role"
        );
    }

    #[test]
    fn compound_values_join_into_the_spaced_dot_form() {
        let task = ClaimedTask {
            key: "0243.4".to_string(),
            title: "title".to_string(),
            description: String::new(),
            stored_status: Some(TaskStatus::Done),
            auto_close: None,
        };
        let baseline = baseline_with_run(
            "run-1",
            "implement-phase",
            2_520,
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Task(Box::new(task))),
        );
        let state = accepted(&baseline);
        let block = sessions_run_block(Some(&state));
        let joined = |row: &KeyValueRow| -> String {
            row.value
                .iter()
                .map(|segment| segment.text.clone())
                .collect()
        };
        assert_eq!(joined(&block.rows[0]), "implement-phase \u{b7} basic");
        assert_eq!(
            joined(&block.rows[1]),
            format!(
                "run-1 \u{b7} {}",
                compact_elapsed_text(Duration::from_secs(2_520))
            )
        );
        assert_eq!(joined(&block.rows[2]), "0243.4 \u{b7} done");
    }

    #[test]
    fn task_failure_wording_uses_no_prohibited_glyph() {
        for outcome in [
            ClaimedTaskResult::Missing,
            ClaimedTaskResult::Ambiguous(vec!["a".to_string()]),
        ] {
            let baseline = baseline_with(Ok(None), Ok(outcome));
            let row = task_row(&baseline);
            assert!(!row.value[0].text.contains('\u{2014}'), "no em dash");
        }
        let failed = baseline_with(Ok(None), Err("center down".to_string()));
        let row = task_row(&failed);
        assert!(!row.value[0].text.contains('\u{2014}'), "no em dash");
    }

    #[test]
    fn trait_row_variant_served_none_and_err_render_without_a_dangling_dot() {
        let served = baseline_with(
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let row = trait_row(&served.row, &served);
        assert_eq!(row.value.len(), 3);
        assert_eq!(row.value[2].text, "basic");

        let none = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        let row = trait_row(&none.row, &none);
        assert_eq!(row.value.len(), 1);

        let err = baseline_with(
            Err("digest mismatch".to_string()),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let row = trait_row(&err.row, &err);
        assert_eq!(row.value.len(), 3);
        assert_eq!(row.value[2].role, Some(StateRole::Danger));
    }

    #[test]
    fn run_row_elapsed_matches_the_shared_compact_formatter() {
        let baseline = baseline_with_run(
            "run-1",
            "implement-phase",
            2_520,
            Ok(None),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let composed = run_row_row(&baseline.row);
        assert_eq!(
            composed.value[2].text,
            compact_elapsed_text(Duration::from_secs(2_520))
        );
    }

    #[test]
    fn all_task_boundaries_render_the_documented_shape() {
        let unclaimed = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        assert_eq!(
            task_row(&unclaimed).value,
            vec![ValueSegment::neutral("no task claimed")]
        );

        let no_status = baseline_with(
            Ok(None),
            Ok(ClaimedTaskResult::Task(Box::new(ClaimedTask {
                key: "0265.10".to_string(),
                title: "t".to_string(),
                description: String::new(),
                stored_status: None,
                auto_close: None,
            }))),
        );
        assert_eq!(
            task_row(&no_status).value,
            vec![ValueSegment::neutral("0265.10")]
        );

        for outcome in [
            ClaimedTaskResult::Missing,
            ClaimedTaskResult::Ambiguous(vec!["a".to_string()]),
        ] {
            let baseline = baseline_with(Ok(None), Ok(outcome));
            let row = task_row(&baseline);
            assert_eq!(row.value.len(), 1);
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
        }

        let failed = baseline_with(Ok(None), Err("center down".to_string()));
        let row = task_row(&failed);
        assert_eq!(row.value[0].role, Some(StateRole::Danger));
    }

    #[test]
    fn footer_omits_started_when_epoch_is_none_with_no_dangling_separator() {
        let baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        let state = accepted(&baseline);
        let footer = sessions_footer(Some(&state));
        assert_eq!(footer, "run-1");
        assert!(!footer.contains("started"));
    }

    #[test]
    fn footer_renders_minute_precision_when_started_at_epoch_is_some() {
        let mut session = fixture_session("run-1", "implement-phase", 2_520);
        session["provenance"]["started-at-epoch"] = serde_json::json!(3_723);
        let mut row = fixture_row("run-1", "implement-phase", 2_520);
        row.started_at_epoch = Some(3_723);
        let baseline = DetailBaseline {
            session: serde_json::from_value(session).expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant: Ok(None),
            claimed_task: Ok(ClaimedTaskResult::Unclaimed),
            row,
        };
        let state = accepted(&baseline);
        let footer = sessions_footer(Some(&state));
        assert!(footer.starts_with("run-1 \u{b7} started "));
    }

    #[test]
    fn stale_and_refreshing_postures_render_a_visible_marker_never_identical_to_current() {
        let baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));

        let current = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: false,
        };
        let refreshing = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: true,
        };
        let stale = PreviewState::Accepted {
            baseline: &baseline,
            stale: Some("subscription closed"),
            refreshing: true,
        };

        let current_block = sessions_run_block(Some(&current));
        let refreshing_block = sessions_run_block(Some(&refreshing));
        let stale_block = sessions_run_block(Some(&stale));

        assert_eq!(current_block.heading, "run");
        assert_ne!(refreshing_block.heading, current_block.heading);
        assert!(refreshing_block.heading.contains("refreshing"));
        assert_ne!(stale_block.heading, current_block.heading);
        assert!(stale_block.heading.contains("stale"));

        let current_footer = sessions_footer(Some(&current));
        let refreshing_footer = sessions_footer(Some(&refreshing));
        let stale_footer = sessions_footer(Some(&stale));
        assert_ne!(refreshing_footer, current_footer);
        assert!(refreshing_footer.contains("refreshing"));
        assert_ne!(stale_footer, current_footer);
        assert!(stale_footer.contains("stale"));
    }

    #[test]
    fn unreadable_state_produces_no_zero_seconds_no_empty_identity_no_task_and_a_nonempty_footer() {
        let state = PreviewState::Failed("bad json");
        let block = sessions_run_block(Some(&state));
        for row in &block.rows {
            assert_eq!(row.value.len(), 1);
            assert!(row.value[0].text.contains("bad json"));
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
        }
        let footer = sessions_footer(Some(&state));
        assert!(
            !footer.is_empty(),
            "the unreadable footer must not be blank"
        );
        assert!(!footer.contains("started"), "no fabricated provenance");
    }

    /// A projected row already flagged `RowState::Unreadable` renders
    /// unreadable even though the ledger read succeeded and produced an
    /// `Accepted` baseline — no `0s`, no empty identity, no invented task
    /// (goal 8).
    #[test]
    fn a_committed_row_flagged_unreadable_renders_unreadable_even_when_accepted() {
        let mut baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        baseline.row.state = RowState::Unreadable;
        baseline.row.detail_text = "trailing comma".to_string();
        let state = accepted(&baseline);

        let block = sessions_run_block(Some(&state));
        for row in &block.rows {
            assert_eq!(row.value.len(), 1);
            assert!(row.value[0].text.contains("trailing comma"));
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
            assert!(!row.value[0].text.contains("0s"));
        }
        let footer = sessions_footer(Some(&state));
        assert!(footer.contains("trailing comma"));
        assert!(!footer.contains("started"), "no fabricated provenance");
    }
}
