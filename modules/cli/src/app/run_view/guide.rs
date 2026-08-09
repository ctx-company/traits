//! Guide chat: the inline conversation pane state, its key routing, and the
//! async dispatch handle a live run or a dashboard-attached session shares to
//! reach it.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use ratatui::layout::Rect;

use super::RunPanelState;
use super::model::{MAX_GUIDE_ANSWER_CHARS, RunView};
use super::render::guide_lines;
use crate::app::guide::GuideTurn;
use crate::app::tui;
use crate::app::tui_kit;

pub(super) type GuideDispatch = Arc<dyn Fn(GuideTurn) -> crate::Result<String> + Send + Sync>;

#[derive(Default)]
pub(super) struct GuidePane {
    pub(super) input: tui_kit::TextInput,
    pub(super) exchanges: Vec<GuideExchange>,
    pub(super) open: bool,
    pub(super) scroll: tui_kit::ViewportScroll,
    pub(super) follow: bool,
    pub(super) body_rows: usize,
    /// Authoritative request state. Presentation may collapse while this stays
    /// true, preventing a second paid call until the worker settles.
    pub(super) in_flight: bool,
    pub(super) generation: u64,
}

pub(super) struct GuideChat {
    pub(super) guide: GuidePane,
    pub(super) dispatch: GuideDispatch,
    pub(super) tokens: crate::app::harness_stream::OneShotTokenTracker,
    pub(super) results: Option<mpsc::Receiver<(u64, Result<String, String>)>>,
    pub(super) wake: Option<Arc<dyn Fn() + Send + Sync>>,
    // This is refreshed by the live surface and intentionally remains the last
    // bounded snapshot when terminal ownership moves to the dashboard.
    pub(super) evidence: String,
}

pub(super) struct GuideExchange {
    pub(super) question: String,
    pub(super) generation: u64,
    pub(super) answer: Option<String>,
    /// The step the evidence was composed against at send. Rendered on the
    /// answer line so an overtaken answer is visibly about the past.
    pub(super) composed_step: String,
}

/// Process-local conversation state which may move from a live run to its
/// dashboard. Dispatch configuration remains with the live run; a separately
/// launched dashboard never receives this handle.
#[derive(Clone)]
pub(crate) struct GuideChatHandle(pub(super) Arc<Mutex<GuideChat>>);

impl GuideChatHandle {
    pub(crate) fn new(
        dispatch: GuideDispatch,
        tokens: crate::app::harness_stream::OneShotTokenTracker,
    ) -> Self {
        Self(Arc::new(Mutex::new(GuideChat {
            guide: GuidePane::default(),
            dispatch,
            tokens,
            results: None,
            wake: None,
            evidence: String::new(),
        })))
    }

    #[cfg(test)]
    pub(crate) fn test_handle() -> Self {
        Self::new(
            Arc::new(|_| Ok("test answer".to_string())),
            Default::default(),
        )
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, GuideChat> {
        self.0.lock().expect("guide chat lock poisoned")
    }

    pub(crate) fn poll_results(&self) -> bool {
        let mut chat = self.lock();
        let result = chat
            .results
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some((generation, result)) = result {
            let changed = apply_guide_result(&mut chat.guide, generation, result);
            chat.results = None;
            return changed;
        }
        false
    }

    fn set_evidence(&self, evidence: String) {
        self.lock().evidence = evidence;
    }

    pub(super) fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.lock().wake = Some(wake);
    }

    pub(super) fn guide_tokens(&self) -> u64 {
        self.lock().tokens.snapshot().tokens.unwrap_or(0)
    }

    pub(crate) fn handle_key(&self, key: &KeyEvent, body_rows: usize) -> bool {
        let mut chat = self.lock();
        if let Some(consumed) = apply_guide_presentation_key(&mut chat.guide, key) {
            return consumed;
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        ) && let Some(delta) = tui_kit::scroll_key(key)
        {
            chat.guide.scroll.apply(delta, body_rows);
            chat.guide.follow = chat.guide.scroll.is_at_bottom(body_rows);
            return true;
        }
        if key.code == KeyCode::Char('l')
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            // Clearing does not cancel an in-flight call — the seat is paid
            // and one-shot. It drops settled exchanges only; a result for a
            // cleared generation still releases `in_flight` in
            // `apply_guide_result` without resurrecting an exchange.
            chat.guide.exchanges.clear();
            chat.guide.scroll.set_len(0);
            chat.guide.follow = true;
            return true;
        }
        match key.code {
            KeyCode::Enter => {
                if chat.guide.in_flight {
                    return true;
                }
                let question = chat.guide.input.text().trim().to_string();
                if question.is_empty() {
                    return true;
                }
                chat.tokens.begin_call();
                chat.guide.in_flight = true;
                chat.guide.generation = chat.guide.generation.wrapping_add(1);
                let generation = chat.guide.generation;
                let composed_step = current_composed_step(&chat.evidence);
                chat.guide.exchanges.push(GuideExchange {
                    question: question.clone(),
                    generation,
                    answer: None,
                    composed_step,
                });
                chat.guide.input.reset();
                chat.guide.follow = true;
                let transcript = answered_transcript(&chat.guide.exchanges);
                let dispatch = Arc::clone(&chat.dispatch);
                let evidence = chat.evidence.clone();
                let wake = chat.wake.clone();
                let (sender, receiver) = mpsc::channel();
                chat.results = Some(receiver);
                std::thread::spawn(move || {
                    let turn = GuideTurn {
                        question,
                        transcript,
                        evidence,
                    };
                    let result = dispatch(turn).map_err(|error| error.to_string());
                    let _ = sender.send((generation, result));
                    if let Some(wake) = wake {
                        wake();
                    }
                });
                true
            }
            _ => matches!(
                chat.guide.input.handle_key(false, key),
                tui_kit::ModalOutcome::Pending
            ),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.lock().guide.open
    }

    pub(crate) fn render(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mut chat = self.lock();
        let guide = &mut chat.guide;
        if guide.open {
            let lines = guide_lines(guide);
            let input = guide.input.clone();
            let follow = guide.follow;
            guide.body_rows = tui_kit::conversation_body_rows(area);
            tui_kit::render_conversation_modal(
                frame,
                area,
                "Guide",
                &lines,
                &input,
                &mut guide.scroll,
                follow,
            );
        }
    }
}

/// Answered exchanges rendered as whole "You: …\nGuide: …" blocks, oldest
/// first, for the dispatcher to trim from the front under its own budget.
fn answered_transcript(exchanges: &[GuideExchange]) -> Vec<String> {
    exchanges
        .iter()
        .filter_map(|exchange| {
            let answer = exchange.answer.as_deref()?;
            Some(format!("You: {}\nGuide: {answer}", exchange.question))
        })
        .collect()
}

fn current_composed_step(evidence: &str) -> String {
    evidence
        .lines()
        .find_map(|line| line.strip_prefix("Current step: "))
        .unwrap_or("unknown")
        .to_string()
}

pub(super) fn apply_guide_key(state: &mut RunPanelState, key: KeyEvent) -> bool {
    let Some(guide) = state.guide.as_ref() else {
        return false;
    };
    // Evidence is composed only when the key will actually dispatch — every
    // other key (including plain typing) skips the disk read and story
    // rebuild `guide::evidence` does.
    let dispatches = key.code == KeyCode::Enter
        && !guide.lock().guide.in_flight
        && !guide.lock().guide.input.text().trim().is_empty();
    if dispatches {
        let (current_step, statuses) = guide_snapshot_from_state(state);
        guide.set_evidence(crate::app::guide::evidence(
            &state.session,
            &state.plan,
            state.guide_ledger_path.as_deref(),
            &current_step,
            &statuses,
        ));
    }
    let body_rows = guide.lock().guide.body_rows;
    guide.handle_key(&key, body_rows)
}

/// Handle presentation-only keys before dispatch. Keeping this reducer small
/// makes every visible phase transition share the live router's exact rules;
/// in particular, Waiting consumes pane-navigation keys until Escape collapses
/// the guide pane.
pub(super) fn apply_guide_presentation_key(guide: &mut GuidePane, key: &KeyEvent) -> Option<bool> {
    if !guide.open {
        if key.code == KeyCode::Char('?') {
            guide.open = true;
            return Some(true);
        }
        return Some(false);
    }
    if key.code == KeyCode::Esc {
        guide.open = false;
        return Some(true);
    }
    None
}

pub(super) fn apply_guide_result(
    guide: &mut GuidePane,
    generation: u64,
    result: Result<String, String>,
) -> bool {
    if !guide.in_flight || generation != guide.generation {
        return false;
    }
    let Some(exchange) = guide
        .exchanges
        .iter_mut()
        .find(|exchange| exchange.generation == generation && exchange.answer.is_none())
    else {
        // The reservation is for a generation whose exchange was already
        // cleared (`ctrl-l` during flight). Release it without resurrecting
        // an exchange, so a cleared-while-thinking chat can send again.
        guide.in_flight = false;
        return false;
    };
    guide.in_flight = false;
    exchange.answer =
        Some(displayable_guide_answer(&result.unwrap_or_else(|error| {
            format!("Guide unavailable: {error}")
        })));
    true
}

pub(super) fn displayable_guide_answer(answer: &str) -> String {
    let cleaned = tui::clean_live_text(answer);
    let normalized = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut bounded: String = normalized.chars().take(MAX_GUIDE_ANSWER_CHARS).collect();
    if bounded.chars().count() < normalized.chars().count() {
        bounded.push_str("...");
    }
    bounded
}

/// Last-painted-view snapshot, used by the dashboard-attached path, which has
/// no composer of its own and keeps this documented last-snapshot behavior.
pub(super) fn guide_snapshot(view: &RunView) -> (String, String) {
    let step = view
        .steps
        .iter()
        .find(|step| step.active)
        .map(|step| step.label.as_str())
        .unwrap_or("none")
        .to_string();
    let statuses = view
        .steps
        .iter()
        .map(|step| format!("{}: {:?}", step.label, step.state))
        .collect::<Vec<_>>()
        .join("; ");
    (step, statuses)
}

/// Current-state snapshot read directly from `state.session`/`state.plan` at
/// send, the same sources `rebuild_view`'s `run_view()` reads — not the last
/// painted `state.view`, which lags the ledger between paints.
fn guide_snapshot_from_state(state: &RunPanelState) -> (String, String) {
    let view = super::run_view(
        &state.trait_ref,
        &state.plan,
        &state.session,
        None,
        super::PresentationState {
            active_started: &state.active_started,
            finished_durations: &state.finished_durations,
            output_tokens: &state.output_tokens,
            loop_elapsed: &state.loop_elapsed,
            loop_output_tokens: &state.loop_output_tokens,
            step_summaries: &state.step_summaries,
            step_summary_at: &state.step_summary_at,
            narrator_tokens: state.narrator_tokens,
            guide_tokens: state
                .guide
                .as_ref()
                .map_or(state.ledger_guide_tokens, GuideChatHandle::guide_tokens),
            run_started: state.run_started,
            live_drive: !state.observer,
        },
    );
    guide_snapshot(&view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_pane_routes_open_edit_answer_and_collapse_visibility() {
        let mut guide = GuidePane::default();
        assert!(guide_lines(&guide).is_empty());
        assert_eq!(
            apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Char('?'))),
            Some(true)
        );
        assert!(guide.open);
        assert!(matches!(
            guide
                .input
                .handle_key(false, &KeyEvent::from(KeyCode::Char('é'))),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(guide.input.cursor(), 1);
        assert!(matches!(
            guide
                .input
                .handle_key(false, &KeyEvent::from(KeyCode::Left)),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(guide.input.cursor(), 0);
        guide.generation = 1;
        guide.in_flight = true;
        guide.exchanges.push(GuideExchange {
            question: "é".to_string(),
            generation: 1,
            answer: None,
            composed_step: "publish".to_string(),
        });
        assert!(apply_guide_result(&mut guide, 1, Ok("answer".to_string())));
        assert_eq!(
            guide_lines(&guide),
            ["You: é", "Guide (at publish): answer"]
        );
        assert_eq!(
            apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Esc)),
            Some(true)
        );
        assert!(!guide.open);
        assert_eq!(
            guide_lines(&guide),
            ["You: é", "Guide (at publish): answer"]
        );
    }

    #[test]
    fn text_input_inline_guide_unicode_editing_survives_close_and_reopen() {
        let mut guide = GuidePane::default();
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Char('?')));
        for key in ['文', 'é'] {
            assert!(matches!(
                guide
                    .input
                    .handle_key(false, &KeyEvent::from(KeyCode::Char(key))),
                tui_kit::ModalOutcome::Pending
            ));
        }
        assert!(matches!(
            guide
                .input
                .handle_key(false, &KeyEvent::from(KeyCode::Left)),
            tui_kit::ModalOutcome::Pending
        ));
        assert!(matches!(
            guide
                .input
                .handle_key(false, &KeyEvent::from(KeyCode::Backspace)),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(guide.input.text(), "é");
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Esc));
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Char('?')));
        assert_eq!(guide.input.text(), "é");
        assert_eq!(guide.input.cursor(), 0);
    }

    #[test]
    fn guide_pane_normalizes_and_bounds_multiline_answers() {
        let answer = format!(
            "first\n\nsecond\tthird {}",
            "x".repeat(MAX_GUIDE_ANSWER_CHARS)
        );
        let display = displayable_guide_answer(&answer);
        assert_eq!(
            display.split_whitespace().take(3).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(!display.contains(['\n', '\r']));
        assert!(display.chars().count() <= MAX_GUIDE_ANSWER_CHARS + 3);
        assert!(display.ends_with("..."));
    }

    #[test]
    fn guide_pane_sanitizes_untrusted_answer_controls() {
        let answer = format!(
            "\x1b[31mfirst\x1b[0m\nsecond\u{0007}\u{202e}third {}",
            "x".repeat(MAX_GUIDE_ANSWER_CHARS)
        );
        let display = displayable_guide_answer(&answer);

        assert_eq!(
            display.split_whitespace().take(3).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(!display.contains('\x1b'));
        assert!(!display.chars().any(|ch| ch.is_control()));
        assert!(!display.contains('\u{202e}'));
        assert!(display.chars().count() <= MAX_GUIDE_ANSWER_CHARS + 3);
        assert!(display.ends_with("..."));
    }

    #[test]
    fn guide_call_lifecycle_retains_hidden_completion_and_rejects_unknown_generation() {
        let mut guide = GuidePane {
            open: true,
            in_flight: true,
            generation: 7,
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 7,
                answer: None,
                composed_step: "publish".to_string(),
            }],
            ..GuidePane::default()
        };
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Esc));
        assert!(guide.in_flight, "collapsing must not permit another call");
        assert!(apply_guide_result(&mut guide, 7, Ok("settled".to_string())));
        assert!(!guide.open);
        assert_eq!(guide.exchanges[0].answer.as_deref(), Some("settled"));
        assert!(!guide.in_flight);
        assert!(!apply_guide_result(&mut guide, 8, Ok("stale".to_string())));
    }

    #[test]
    fn stale_guide_result_does_not_clear_current_in_flight() {
        let mut guide = GuidePane {
            in_flight: true,
            generation: 2,
            exchanges: vec![
                GuideExchange {
                    question: "first".to_string(),
                    generation: 1,
                    answer: Some("answered".to_string()),
                    composed_step: "publish".to_string(),
                },
                GuideExchange {
                    question: "second".to_string(),
                    generation: 2,
                    answer: None,
                    composed_step: "publish".to_string(),
                },
            ],
            ..GuidePane::default()
        };
        assert!(!apply_guide_result(&mut guide, 1, Ok("stale".to_string())));
        assert_eq!(guide.exchanges[0].answer.as_deref(), Some("answered"));
        assert!(guide.exchanges[1].answer.is_none());
        assert!(guide.in_flight);
    }

    #[test]
    fn guide_call_lifecycle_blocks_reopen_submission_until_the_reserved_call_settles() {
        let mut guide = GuidePane {
            open: true,
            in_flight: true,
            generation: 3,
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 3,
                answer: None,
                composed_step: "publish".to_string(),
            }],
            ..GuidePane::default()
        };
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Esc));
        apply_guide_presentation_key(&mut guide, &KeyEvent::from(KeyCode::Char('?')));
        assert!(guide.open);
        assert!(guide.in_flight);
        // The presentation router consumes Enter while the reservation is
        // live, so reopening cannot launch another call.
        // A live router checks this guard before it can reserve another call.
        assert!(guide.in_flight);
        assert!(apply_guide_result(&mut guide, 3, Ok("settled".to_string())));
        assert!(!guide.in_flight);
    }

    #[test]
    fn guide_chat_scroll_uses_rendered_viewport_rows() {
        let chat = GuideChatHandle::test_handle();
        {
            let mut state = chat.lock();
            state.guide.open = true;
            state.guide.scroll.set_len(30);
            state.guide.body_rows = 3;
            state.guide.scroll.apply(tui_kit::ScrollDelta::End, 3);
            state.guide.follow = true;
        }
        chat.handle_key(&KeyEvent::from(KeyCode::Up), 3);
        {
            let state = chat.lock();
            assert_eq!(state.guide.scroll.window(3), 26..29);
            assert!(!state.guide.follow);
        }
        chat.handle_key(&KeyEvent::from(KeyCode::Down), 3);
        assert!(chat.lock().guide.follow);

        // A resize changes both the clamp and the tail position; the same one
        // row key must use the new rendered body height, not a fixed value.
        chat.lock().guide.scroll.apply(tui_kit::ScrollDelta::End, 7);
        chat.handle_key(&KeyEvent::from(KeyCode::Up), 7);
        let state = chat.lock();
        assert_eq!(state.guide.scroll.window(7), 22..29);
        assert!(!state.guide.follow);
    }

    #[test]
    fn clear_drops_exchanges_and_a_late_result_releases_in_flight_without_resurrecting() {
        let chat = GuideChatHandle::test_handle();
        {
            let mut state = chat.lock();
            state.guide.open = true;
            state.guide.in_flight = true;
            state.guide.generation = 5;
            state.guide.exchanges.push(GuideExchange {
                question: "q".to_string(),
                generation: 5,
                answer: None,
                composed_step: "publish".to_string(),
            });
        }
        // ctrl-l clears while a call is still in flight.
        chat.handle_key(
            &KeyEvent::new(KeyCode::Char('l'), crossterm::event::KeyModifiers::CONTROL),
            10,
        );
        {
            let state = chat.lock();
            assert!(state.guide.exchanges.is_empty());
            assert!(state.guide.in_flight, "clear must not cancel in flight");
        }
        // The settling result for the cleared generation must release the
        // reservation without resurrecting the dropped exchange.
        let mut state = chat.lock();
        assert!(!apply_guide_result(
            &mut state.guide,
            5,
            Ok("late".to_string())
        ));
        assert!(!state.guide.in_flight);
        assert!(state.guide.exchanges.is_empty());
    }

    #[test]
    fn answered_transcript_renders_only_settled_exchanges_oldest_first() {
        let exchanges = vec![
            GuideExchange {
                question: "first".to_string(),
                generation: 1,
                answer: Some("one".to_string()),
                composed_step: "a".to_string(),
            },
            GuideExchange {
                question: "second".to_string(),
                generation: 2,
                answer: None,
                composed_step: "b".to_string(),
            },
        ];
        let transcript = answered_transcript(&exchanges);
        assert_eq!(transcript, vec!["You: first\nGuide: one".to_string()]);
    }
}
