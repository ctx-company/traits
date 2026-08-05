//! Ask/guide chat: the inline conversation pane state, its key routing,
//! and the async dispatch handle a live run or a dashboard-attached
//! session shares to reach it.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use ratatui::layout::Rect;

use super::RunPanelState;
use super::model::{MAX_GUIDE_ANSWER_CHARS, RunView};
use super::render::ask_lines;
use crate::app::tui;
use crate::app::tui_kit;

pub(super) type GuideDispatch = Arc<dyn Fn(String, String) -> crate::Result<String> + Send + Sync>;

#[derive(Default)]
pub(super) struct AskPane {
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
    pub(super) ask: AskPane,
    pub(super) dispatch: GuideDispatch,
    pub(super) tokens: crate::app::harness_stream::OneShotTokenTracker,
    pub(super) results: Option<mpsc::Receiver<(u64, Result<String, String>)>>,
    pub(super) wake: Option<Arc<dyn Fn() + Send + Sync>>,
    // This is refreshed by the live surface and intentionally remains the last
    // bounded snapshot when terminal ownership moves to the dashboard.
    pub(super) context: String,
}

pub(super) struct GuideExchange {
    pub(super) question: String,
    pub(super) generation: u64,
    pub(super) answer: Option<String>,
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
            ask: AskPane::default(),
            dispatch,
            tokens,
            results: None,
            wake: None,
            context: String::new(),
        })))
    }

    #[cfg(test)]
    pub(crate) fn test_handle() -> Self {
        Self::new(
            Arc::new(|_, _| Ok("test answer".to_string())),
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
            let changed = apply_ask_result(&mut chat.ask, generation, result);
            chat.results = None;
            return changed;
        }
        false
    }

    fn set_context(&self, context: String) {
        self.lock().context = context;
    }

    pub(super) fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.lock().wake = Some(wake);
    }

    pub(super) fn guide_tokens(&self) -> u64 {
        self.lock().tokens.snapshot().tokens.unwrap_or(0)
    }

    pub(crate) fn handle_key(&self, key: &KeyEvent, body_rows: usize) -> bool {
        let mut chat = self.lock();
        if let Some(consumed) = apply_ask_presentation_key(&mut chat.ask, key) {
            return consumed;
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        ) && let Some(delta) = tui_kit::scroll_key(key)
        {
            chat.ask.scroll.apply(delta, body_rows);
            chat.ask.follow = chat.ask.scroll.is_at_bottom(body_rows);
            return true;
        }
        match key.code {
            KeyCode::Enter => {
                if chat.ask.in_flight {
                    return true;
                }
                let question = chat.ask.input.text().trim().to_string();
                if question.is_empty() {
                    return true;
                }
                chat.tokens.begin_call();
                chat.ask.in_flight = true;
                chat.ask.generation = chat.ask.generation.wrapping_add(1);
                let generation = chat.ask.generation;
                chat.ask.exchanges.push(GuideExchange {
                    question: question.clone(),
                    generation,
                    answer: None,
                });
                chat.ask.input.reset();
                chat.ask.follow = true;
                let dispatch = Arc::clone(&chat.dispatch);
                let context = chat.context.clone();
                let wake = chat.wake.clone();
                let (sender, receiver) = mpsc::channel();
                chat.results = Some(receiver);
                std::thread::spawn(move || {
                    let result = dispatch(question, context).map_err(|error| error.to_string());
                    let _ = sender.send((generation, result));
                    if let Some(wake) = wake {
                        wake();
                    }
                });
                true
            }
            _ => matches!(
                chat.ask.input.handle_key(false, key),
                tui_kit::ModalOutcome::Pending
            ),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.lock().ask.open
    }

    pub(crate) fn render(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mut chat = self.lock();
        let ask = &mut chat.ask;
        if ask.open {
            let lines = ask_lines(ask);
            let input = ask.input.clone();
            let follow = ask.follow;
            ask.body_rows = tui_kit::conversation_body_rows(area);
            tui_kit::render_conversation_modal(
                frame,
                area,
                "Guide",
                &lines,
                &input,
                &mut ask.scroll,
                follow,
            );
        }
    }
}

pub(super) fn apply_ask_key(state: &mut RunPanelState, key: KeyEvent) -> bool {
    let Some(ask) = state.ask.as_ref() else {
        return false;
    };
    let (current_step, statuses) = guide_snapshot(&state.view);
    ask.set_context(crate::app::guide::evidence(
        &state.session,
        &state.plan,
        state.guide_ledger_path.as_deref(),
        &current_step,
        &statuses,
    ));
    let body_rows = ask.lock().ask.body_rows;
    ask.handle_key(&key, body_rows)
}

/// Handle presentation-only keys before dispatch. Keeping this reducer small
/// makes every visible phase transition share the live router's exact rules;
/// in particular, Waiting consumes pane-navigation keys until Escape collapses
/// the ask pane.
pub(super) fn apply_ask_presentation_key(ask: &mut AskPane, key: &KeyEvent) -> Option<bool> {
    if !ask.open {
        if key.code == KeyCode::Char('?') {
            ask.open = true;
            return Some(true);
        }
        return Some(false);
    }
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        ask.open = false;
        return Some(true);
    }
    None
}

pub(super) fn apply_ask_result(
    ask: &mut AskPane,
    generation: u64,
    result: Result<String, String>,
) -> bool {
    if !ask.in_flight || generation != ask.generation {
        return false;
    }
    let Some(exchange) = ask
        .exchanges
        .iter_mut()
        .find(|exchange| exchange.generation == generation && exchange.answer.is_none())
    else {
        return false;
    };
    ask.in_flight = false;
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
