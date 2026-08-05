//! Line/frame rendering: turns a `RunView`/`RunPanelState` into styled
//! terminal lines and ratatui frames — the pane tree, live 2x2 grid,
//! journey/history/current rows, and the ledger-driven header/outputs box.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

use super::model::{
    ASK_FOOTER_HINT, CURRENT_MIN_OUTER_ROWS, CURRENT_PANE, HISTORY_MIN_OUTER_ROWS, HistoryOutcome,
    HistoryStep, JourneyRow, JourneyRowKind, LIVE_PANE_IDS, MergeRowState, MergeRowView,
    NARROW_WIDTH_THRESHOLD, PaneData, PaneIds, RunHeader, RunNarration, RunOutput, RunStep,
    RunView, StepState,
};
use super::planned::status_tone;
use super::projection::{active_step_index, completed_narration, display_narration};
use super::session_text::active_label;
use super::{
    AskPane, FollowTarget, GuideChatHandle, RunPanelState, StreamRow, StreamRowKind,
    apply_scroll_and_derive_follow, capped_repeat_delta, poll_and_apply_keys,
    repeat_row_scroll_key,
};
use crate::app::tui;
use crate::app::tui_kit;
use crate::app::tui_panes::{
    self, FocusRing, PaneId, PaneLayoutResult, PaneScrolls, PaneTree, TabStep,
};
use crate::app::tui_ratatui;
use crate::app::tui_select;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(super) fn render_locked(state: &mut RunPanelState) {
    // Task 0023: rebuild the ellipsized-text ledger once per frame, before
    // any truncating call site below can record into it.
    tui_select::clear_ledger();
    // Capture before draining: a wake that lands concurrently afterwards must
    // remain pending for a later tick rather than being acknowledged unseen.
    let input_generation = state.input_generation.load(Ordering::Acquire);
    state.repaint.apply_resize();
    poll_and_apply_keys(state);
    let progress_lines = progress_lines(&state.view);
    let (journey_lines, _active_row, journey_ladder) = journey_lines_with_active_row(&state.view);
    let post_run_lines = post_run_lines(&state.view);
    let history_rows = story_history_lines(&state.view);
    let mut current_rows = story_stream_lines(state);
    // P552: the trailing in-flight line is not a special overlay outside the
    // event model — it folds into the same `current_rows` set the recorded
    // stream uses, so `render_pane_body` formats every CURRENT row (recorded
    // or in-flight) through the one `event_row_line` contract.
    if let Some(overlay) = stream_overlay_line(state) {
        current_rows.push(overlay);
    }
    let title_line = title_row_line(
        state.title_state.as_ref(),
        &state.trait_name,
        state.session.provenance.started_at_epoch,
    );
    let RunPanelState {
        repaint,
        scrolls,
        progress_follow,
        journey_follow,
        history_follow,
        current_follow,
        focus,
        pending_keys,
        modal,
        ask,
        ..
    } = state;
    let modal = modal.as_ref();
    let _ = repaint.draw(|frame| {
        render_live_panes(
            frame,
            LiveFrame {
                title_line: &title_line,
                progress_lines: &progress_lines,
                journey_lines: &journey_lines,
                journey_ladder: &journey_ladder,
                history_rows: &history_rows,
                current_rows: &current_rows,
                post_run_lines: post_run_lines.as_deref(),
                scrolls,
                progress_follow,
                journey_follow,
                history_follow,
                current_follow,
                focus,
                pending_keys,
                modal,
                ask: ask.as_ref(),
            },
        );
    });
    state.last_tree_lines = journey_row_lines(&journey_lines, 80);
    state
        .handled_generation
        .fetch_max(input_generation, Ordering::Release);
}

/// One visible title row for every live or attached lifecycle state.
pub(crate) fn title_row_line(
    title_state: Option<&ctx_traits_core::procedure::session::SessionTitleState>,
    trait_name: &str,
    started_at_epoch: Option<u64>,
) -> tui::Line {
    let mut line = tui::Line::blank();
    match title_state {
        Some(ctx_traits_core::procedure::session::SessionTitleState::Resolved {
            title, ..
        }) => {
            line.push(title.clone(), tui::Tone::Bold);
            line.push(" \u{b7} ", tui::Tone::Muted);
        }
        Some(ctx_traits_core::procedure::session::SessionTitleState::Terminal { .. }) => {}
        None
        | Some(ctx_traits_core::procedure::session::SessionTitleState::InFlight { .. })
        | Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { .. }) => {
            line.push("(Generating session title…)".to_string(), tui::Tone::Muted);
            line.push(" \u{b7} ", tui::Tone::Muted);
        }
    }
    line.push(trait_name.to_string(), tui::Tone::Default);
    if let Some(epoch) = started_at_epoch {
        line.push(" \u{b7} ", tui::Tone::Muted);
        line.push(
            format!("Started at {}", epoch_clock_utc(epoch)),
            tui::Tone::Muted,
        );
    }
    line
}

/// Pure `HH:MM:SS` decomposition of a UNIX epoch, UTC (no calendar handling
/// needed — only the seconds-of-day remainder). No `chrono`/`time` dependency
/// exists in this workspace for a single clock string.
pub(super) fn epoch_clock_utc(epoch: u64) -> String {
    let seconds_of_day = epoch % 86_400;
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// P552: builds a [`PaneTree`] with a leaf for exactly the panes `data`
/// supplies content for — no live/preview/attached mode flag anywhere in
/// this function. Full data (all four `Some`) produces the bounded-progress
/// 2x2 grid; preview data (`history`/`current` both `None`) naturally
/// collapses to the progress/journey left stack alone; a narrow terminal
/// stacks every populated pane instead, without omitting any of them.
/// `ids.progress`/`ids.journey` must not both be `None` in `data` — every
/// caller has at least a progress or a journey source.
pub(crate) fn pane_tree(ids: &PaneIds, area: Rect, data: &PaneData<'_>) -> PaneTree {
    let progress = || PaneTree::Leaf {
        id: ids.progress,
        title: "information".to_string(),
    };
    let journey = || PaneTree::Leaf {
        id: ids.journey,
        title: "journey".to_string(),
    };
    let history = || PaneTree::Leaf {
        id: ids.history,
        title: "history".to_string(),
    };
    let current = || PaneTree::Leaf {
        id: ids.current,
        title: "current activity".to_string(),
    };
    let post_run = || PaneTree::Leaf {
        id: ids.current,
        title: "post-run".to_string(),
    };
    let has_progress = data.progress.is_some();
    let has_journey = data.journey.is_some();
    let has_history = data.history.is_some();
    let has_current = data.current.is_some();
    let has_post_run = data.post_run.is_some();
    debug_assert!(
        has_progress || has_journey,
        "pane_tree requires at least a progress or journey source"
    );

    if area.width < NARROW_WIDTH_THRESHOLD {
        let mut children = Vec::new();
        if has_progress {
            children.push((Constraint::Min(3), progress()));
        }
        if has_journey {
            children.push((Constraint::Min(3), journey()));
        }
        if has_post_run {
            children.push((Constraint::Min(CURRENT_MIN_OUTER_ROWS), post_run()));
        } else if has_history {
            children.push((Constraint::Min(3), history()));
        }
        if has_current && !has_post_run {
            children.push((Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()));
        }
        return PaneTree::Split {
            dir: Direction::Vertical,
            children,
        };
    }

    let left = match (has_progress, has_journey) {
        (true, true) => {
            // P552: progress is bounded to its own handful of standing-fact
            // rows so journey — the pane with unbounded content — receives
            // the rest of the left column's height.
            let progress_rows = data.progress.map_or(0, <[_]>::len);
            let progress_outer = u16::try_from(progress_rows)
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            PaneTree::Split {
                dir: Direction::Vertical,
                children: vec![
                    (Constraint::Length(progress_outer), progress()),
                    (Constraint::Min(3), journey()),
                ],
            }
        }
        (true, false) => progress(),
        (false, true) => journey(),
        (false, false) => PaneTree::Split {
            dir: Direction::Vertical,
            children: Vec::new(),
        },
    };

    let right = if has_post_run {
        Some(post_run())
    } else if has_history
        && has_current
        && area.height >= HISTORY_MIN_OUTER_ROWS + CURRENT_MIN_OUTER_ROWS
    {
        let history_rows = data.history.map_or(0, <[_]>::len);
        let history_height = u16::try_from(history_rows)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .max(HISTORY_MIN_OUTER_ROWS)
            .min(area.height / 2)
            .min(area.height.saturating_sub(CURRENT_MIN_OUTER_ROWS));
        Some(PaneTree::Split {
            dir: Direction::Vertical,
            children: vec![
                (Constraint::Length(history_height), history()),
                (Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()),
            ],
        })
    } else if has_current {
        Some(current())
    } else if has_history {
        Some(history())
    } else {
        None
    };

    match right {
        Some(right) => PaneTree::Split {
            dir: Direction::Horizontal,
            children: vec![
                (Constraint::Percentage(60), left),
                (Constraint::Percentage(40), right),
            ],
        },
        None => left,
    }
}

pub(super) struct LiveFrame<'a> {
    pub(super) title_line: &'a tui::Line,
    /// The PROGRESS pane's bounded standing facts — [`progress_lines`].
    pub(super) progress_lines: &'a [tui::Line],
    /// The JOURNEY pane's full content — [`journey_lines_with_active_row`].
    pub(super) journey_lines: &'a [JourneyRow],
    /// The JOURNEY pane's follow ladder — [`journey_lines_with_active_row`].
    pub(super) journey_ladder: &'a [usize],
    /// Untruncated history/current-activity events — [`event_row_line`]
    /// truncates each to a single physical row only once this pane's inner
    /// width is known, inside [`render_pane_body`] itself.
    pub(super) history_rows: &'a [EventRow],
    /// The CURRENT pane's full row set — the recorded stream plus, when
    /// live, the trailing in-flight overlay already folded in by the
    /// caller (see [`RunPanel`]'s render path) so this module's shared
    /// [`render_pane_body`] treats every CURRENT row through the one
    /// `EventRow`/[`event_row_line`] contract, with no separate overlay
    /// case.
    pub(super) current_rows: &'a [EventRow],
    pub(super) post_run_lines: Option<&'a [tui::Line]>,
    pub(super) scrolls: &'a mut PaneScrolls,
    pub(super) progress_follow: &'a mut bool,
    pub(super) journey_follow: &'a mut bool,
    pub(super) history_follow: &'a mut bool,
    pub(super) current_follow: &'a mut bool,
    pub(super) focus: &'a mut FocusRing,
    pub(super) pending_keys: &'a mut Vec<KeyEvent>,
    pub(super) modal: Option<&'a tui_kit::Modal>,
    pub(super) ask: Option<&'a GuideChatHandle>,
}

pub(super) fn ask_lines(ask: &AskPane) -> Vec<String> {
    ask.exchanges
        .iter()
        .flat_map(|exchange| {
            let answer = exchange.answer.as_deref().unwrap_or("thinking...");
            [
                format!("You: {}", exchange.question),
                format!("Guide: {answer}"),
            ]
        })
        .collect()
}

pub(super) fn drawable_pane_ids(tree: &PaneTree, layout: &PaneLayoutResult) -> Vec<PaneId> {
    tree.leaf_ids()
        .into_iter()
        .filter(|id| {
            layout.rect(id).is_some_and(|rect| {
                let inner = tui_panes::pane_inner(rect);
                inner.width > 0 && inner.height > 0
            })
        })
        .collect()
}

/// The live run surface's own footer chrome, wrapping the shared
/// [`render_pane_body`] for its title row + four-pane body — the live run
/// has no standing-facts `progress` source separate from `tree_lines` today
/// (its header still folds progress-facts and journey rows into one
/// `tree_lines` vector; splitting that fully into its own `PaneData::progress`
/// source is tracked separately), so this wrapper hands the tree's own
/// content to the `journey` pane and leaves `progress` populated from the
/// same standing facts computed by [`progress_lines`]. Title reservation and
/// rendering live in [`render_pane_body`] itself (via `PaneData::title`), so
/// this wrapper only carves out the footer row.
pub(super) fn render_live_panes(frame: &mut ratatui::Frame<'_>, state: LiveFrame<'_>) {
    let LiveFrame {
        title_line,
        progress_lines,
        journey_lines: journey_rows,
        journey_ladder,
        history_rows,
        current_rows,
        post_run_lines,
        scrolls,
        progress_follow,
        journey_follow,
        history_follow,
        current_follow,
        focus,
        pending_keys,
        modal,
        ask,
    } = state;
    let full_area = frame.area();
    let regions = live_frame_regions(full_area);
    frame.render_widget(
        tui_kit::keymap_footer(
            if ask.is_some() {
                ASK_FOOTER_HINT
            } else {
                "[d] dashboard · [q] exit · [ctrl-c] kill · [up/down] scroll · [pgup/pgdn] page · [home/end] jump · [tab] cycle pane"
            },
            None,
        ),
        regions[1],
    );
    let data = PaneData {
        progress: Some(progress_lines),
        journey: Some(journey_rows),
        // Accepted statuses are durable ledger evidence. Do not reserve a pane
        // merely because the live projection currently has an empty row list.
        history: (!history_rows.is_empty()).then_some(history_rows),
        current: Some(current_rows),
        post_run: post_run_lines,
        title: PaneTitleRow::Visible(title_line),
    };
    render_pane_body(
        frame,
        regions[0],
        &LIVE_PANE_IDS,
        &data,
        journey_ladder,
        Some(CURRENT_PANE),
        PaneRenderState {
            scrolls,
            follow: PaneFollow {
                progress: progress_follow,
                journey: journey_follow,
                history: history_follow,
                current: current_follow,
            },
            focus,
            pending_keys,
            key_target: None,
        },
    );
    if let Some(modal) = modal {
        tui_kit::render_modal(frame, full_area, modal);
    } else if let Some(ask) = ask.filter(|ask| ask.is_open()) {
        ask.render(frame, full_area);
    }
}

pub(super) fn live_frame_regions(full_area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(full_area)
}

/// P552: which of the (up to) four panes' user-driven scroll state should
/// stay pinned to its own tail/anchor whenever new content lands — a live
/// run and a dashboard attach each own their own four bools; a caller with
/// fewer sources than four (a dashboard preview) still owns all four, since
/// [`render_pane_body`] only ever reads the bool for a pane `data` actually
/// populates.
pub(crate) struct PaneFollow<'a> {
    pub(crate) progress: &'a mut bool,
    pub(crate) journey: &'a mut bool,
    pub(crate) history: &'a mut bool,
    pub(crate) current: &'a mut bool,
}

/// The mutable state [`render_pane_body`] reads and updates on every call —
/// grouped into one struct (rather than four separate parameters) so the
/// shared renderer's own signature stays under clippy's argument-count
/// lint without suppressing it.
pub(crate) struct PaneRenderState<'a> {
    pub(crate) scrolls: &'a mut PaneScrolls,
    pub(crate) follow: PaneFollow<'a>,
    pub(crate) focus: &'a mut FocusRing,
    pub(crate) pending_keys: &'a mut Vec<KeyEvent>,
    /// P552 review `live-run-pane-contract-absent`: the pane a drained
    /// scroll key addresses instead of `focus.current()`, when `Some` — see
    /// [`render_pane_body`]'s own doc for why the ordinary (list-visible)
    /// SESSIONS preview needs this. `None` for a live run or a dashboard
    /// attach, whose `focus` is reconciled to the very tree being drawn.
    pub(crate) key_target: Option<PaneId>,
}

/// P552's title row, owned by [`render_pane_body`] itself so a live run and
/// a dashboard attach receive identical title behavior — a dashboard
/// preview supplies [`PaneTitleRow::None`] (no row consumed at all, per the
/// implementation draft's out-of-scope: dashboard previews never carry a
/// title), while a live run and an attach both supply [`PaneTitleRow::Reserved`],
/// which consumes its one row whether or not a title has resolved yet
/// (`None` renders blank — there is no placeholder variant, per
/// [`title_row_line`]).
pub(crate) enum PaneTitleRow<'a> {
    None,
    Visible(&'a tui::Line),
}

pub(super) fn title_row_area(area: Rect) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area)[0]
}

/// The pane body area after [`PaneTitleRow`]'s own row (if any) is carved
/// off `area` — the single source of truth both [`render_pane_body`]'s own
/// paint pass and a caller's cached pane-layout resolution (e.g. dashboard
/// attach's `state.last_pane_layout`) must agree on, so generic scroll/focus
/// handling never reads rects that are off by the title row.
pub(crate) fn pane_body_area(area: Rect, title: &PaneTitleRow<'_>) -> Rect {
    match title {
        PaneTitleRow::None => area,
        PaneTitleRow::Visible(_) => Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area)[1],
    }
}

/// P552's one shared pane renderer: resolves `data`'s populated panes into a
/// [`PaneTree`] via [`pane_tree`], reconciles focus, formats every
/// history/current-activity row through [`event_row_line`], and renders
/// border-only content directly into [`tui_panes::pane_inner`] — the sole
/// renderer for a live run's body, dashboard preview's progress/journey
/// pair, and dashboard attach's full four-pane body. `reconcile_default`
/// is `Some` only when `focus` is scoped to exactly this pane set (a live
/// run, or a dashboard attach with its session list hidden); a dashboard
/// preview, whose `focus` also spans the sessions list, passes `None` so
/// this call never steals focus away from a pane outside this tree.
///
/// P552 review `live-run-pane-contract-absent`: `state.key_target`, when
/// `Some`, is the pane a drained scroll key addresses instead of
/// `focus.current()` — the ordinary (list-visible) SESSIONS preview queues
/// PageUp/PageDown into `pending_keys` while `focus` itself stays on the
/// sessions list (so the list's own selection and visibility never move),
/// and needs those keys to reach the journey pane anyway. A live run or a
/// dashboard attach, whose `focus` is reconciled to this very tree, pass
/// `None` and keep routing by `focus.current()` as before.
pub(crate) fn render_pane_body(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    ids: &PaneIds,
    data: &PaneData<'_>,
    journey_ladder: &[usize],
    reconcile_default: Option<PaneId>,
    state: PaneRenderState<'_>,
) {
    if let PaneTitleRow::Visible(title_line) = &data.title {
        let title_area = title_row_area(area);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(vec![tui_ratatui::render_line(title_line)]),
            title_area,
        );
    }
    let area = pane_body_area(area, &data.title);
    let PaneRenderState {
        scrolls,
        follow,
        focus,
        pending_keys,
        key_target,
    } = state;
    let PaneFollow {
        progress: progress_follow,
        journey: journey_follow,
        history: history_follow,
        current: current_follow,
    } = follow;
    let tree = pane_tree(ids, area, data);
    let layout = tree.resolve(area);
    if let Some(default) = reconcile_default {
        focus.reconcile(drawable_pane_ids(&tree, &layout), default);
    }
    let progress_outer = data.progress.and(layout.rect(ids.progress));
    let journey_outer = data.journey.and(layout.rect(ids.journey));
    let history_outer = data.history.and(layout.rect(ids.history));
    // Post-run deliberately reuses current's pane id and scroll state. Once it
    // exists, current must be absent from every renderer path, not only the tree.
    let current_outer = data
        .post_run
        .is_none()
        .then(|| data.current.and(layout.rect(ids.current)))
        .flatten();
    let post_run_outer = data.post_run.and(layout.rect(ids.current));
    let progress_inner = progress_outer.map(tui_panes::pane_inner);
    let journey_inner = journey_outer.map(tui_panes::pane_inner);
    let history_inner = history_outer.map(tui_panes::pane_inner);
    let current_inner = current_outer.map(tui_panes::pane_inner);
    let post_run_inner = post_run_outer.map(tui_panes::pane_inner);

    let progress = data.progress.map(|lines| {
        lines
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let journey = data.journey.map(|rows| {
        journey_row_lines(rows, journey_inner.map_or(0, |rect| rect.width))
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let history = data.history.map(|rows| {
        event_row_lines(rows, history_inner.map_or(0, |rect| rect.width))
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let current = data
        .post_run
        .is_none()
        .then_some(data.current)
        .flatten()
        .map(|rows| {
            event_row_lines(rows, current_inner.map_or(0, |rect| rect.width))
                .iter()
                .map(tui_ratatui::render_line)
                .collect::<Vec<_>>()
        });
    let post_run = data.post_run.map(|lines| {
        lines
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });

    let mut pending_keys = pending_keys.drain(..).peekable();
    while let Some(key) = pending_keys.next() {
        if let Some(step) = tui_panes::tab_cycle_key(&key) {
            match step {
                TabStep::Next => focus.next(),
                TabStep::Prev => focus.prev(),
            }
            continue;
        }
        let repeat_delta = repeat_row_scroll_key(&key);
        let repeats = repeat_delta.map(|delta| {
            let mut repeats = 1;
            while pending_keys
                .peek()
                .is_some_and(|next| repeat_row_scroll_key(next) == Some(delta))
            {
                pending_keys.next();
                repeats += 1;
            }
            repeats
        });
        let Some(delta) = repeat_delta.or_else(|| tui_kit::scroll_key(&key)) else {
            continue;
        };
        let Some(id) = key_target.or_else(|| focus.current()) else {
            continue;
        };
        if id == ids.progress
            && let (Some(progress), Some(inner)) = (&progress, progress_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(progress.len());
            apply_scroll_and_derive_follow(
                scroll,
                progress_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                progress.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.journey
            && let (Some(journey), Some(inner)) = (&journey, journey_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(journey.len());
            apply_scroll_and_derive_follow(
                scroll,
                journey_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                journey.len(),
                FollowTarget::Ladder(journey_ladder),
            );
        } else if id == ids.history
            && let (Some(history), Some(inner)) = (&history, history_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(history.len());
            apply_scroll_and_derive_follow(
                scroll,
                history_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                history.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.current
            && let (Some(post_run), Some(inner)) = (&post_run, post_run_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(post_run.len());
            apply_scroll_and_derive_follow(
                scroll,
                current_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                post_run.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.current
            && let (Some(current), Some(inner)) = (&current, current_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(current.len());
            apply_scroll_and_derive_follow(
                scroll,
                current_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                current.len(),
                FollowTarget::Tail,
            );
        }
    }

    if let Some(outer) = progress_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.progress).expect("progress title"),
            focus.is_focused(ids.progress),
        );
    }
    if let Some(outer) = journey_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.journey).expect("journey title"),
            focus.is_focused(ids.journey),
        );
    }
    if let Some(outer) = history_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.history).expect("history title"),
            focus.is_focused(ids.history),
        );
    }
    if let Some(outer) = post_run_outer.or(current_outer) {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.current).expect("current/post-run title"),
            focus.is_focused(ids.current),
        );
    }

    if let (Some(progress), Some(inner)) = (&progress, progress_inner) {
        follow_target(
            scrolls.get_mut(ids.progress),
            *progress_follow,
            FollowTarget::Tail,
            progress.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, progress, scrolls.get_mut(ids.progress));
    }
    if let (Some(journey), Some(inner)) = (&journey, journey_inner) {
        follow_target(
            scrolls.get_mut(ids.journey),
            *journey_follow,
            FollowTarget::Ladder(journey_ladder),
            journey.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, journey, scrolls.get_mut(ids.journey));
    }
    if let (Some(history), Some(inner)) = (&history, history_inner) {
        follow_target(
            scrolls.get_mut(ids.history),
            *history_follow,
            FollowTarget::Tail,
            history.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, history, scrolls.get_mut(ids.history));
    }
    if let (Some(current), Some(inner)) = (&current, current_inner) {
        follow_target(
            scrolls.get_mut(ids.current),
            *current_follow,
            FollowTarget::Tail,
            current.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, current, scrolls.get_mut(ids.current));
    }
    if let (Some(post_run), Some(inner)) = (&post_run, post_run_inner) {
        follow_target(
            scrolls.get_mut(ids.current),
            *current_follow,
            FollowTarget::Tail,
            post_run.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, post_run, scrolls.get_mut(ids.current));
    }
}

pub(super) fn follow_target(
    scroll: &mut tui_kit::ViewportScroll,
    follow: bool,
    target: FollowTarget<'_>,
    len: usize,
    rows: usize,
) {
    scroll.set_len(len);
    scroll.clamp(rows);
    if follow {
        align_scroll_start(scroll, target.viewport_start(len, rows), rows);
    }
}

/// Moves the persisted viewport only as far as its current rendered window is
/// from `desired_start`; `ViewportScroll` intentionally keeps its raw offset
/// private, so alignment remains coupled to the window the pane actually draws.
pub(super) fn align_scroll_start(
    scroll: &mut tui_kit::ViewportScroll,
    desired_start: usize,
    rows: usize,
) {
    let current_start = scroll.window(rows).start;
    let delta = if desired_start >= current_start {
        tui_kit::ScrollDelta::Down(desired_start - current_start)
    } else {
        tui_kit::ScrollDelta::Up(current_start - desired_start)
    };
    scroll.apply(delta, rows);
}

/// One logical history/current-activity event, built independent of any
/// terminal width — [`event_row_line`] is the only place that turns one of
/// these into a width-truncated physical row, so this type carries the raw
/// (untruncated, uncleaned) tail text plus the tone to render it in.
/// `pub(crate)` (with a constructor rather than public fields) so a
/// dashboard-attach projection outside this module can build rows from a
/// persisted activity sidecar through the exact same type this module's own
/// live rows use — never a second event shape.
#[derive(Clone)]
pub(crate) struct EventRow {
    pub(super) at: Option<Duration>,
    pub(super) tail: String,
    pub(super) tone: tui::Tone,
}

impl EventRow {
    pub(crate) fn new(at: Option<Duration>, tail: String, tone: tui::Tone) -> Self {
        Self { at, tail, tone }
    }

    #[cfg(test)]
    pub(crate) fn tail(&self) -> &str {
        &self.tail
    }
}

/// The story column's compressed history: one event per accepted ledger
/// execution in ledger order — `<label>: <summary>` when a P455 summary landed, otherwise
/// the truthful facts fallback `<label> · elapsed · tokens` — never a
/// placeholder. `at` is when the row itself was stamped (the summary's own
/// landing time, or the step's own elapsed for the fallback). Render through
/// [`event_row_line`] for the fixed `HH:MM:SS ` prefix.
pub(super) fn story_history_lines(view: &RunView) -> Vec<EventRow> {
    view.history.iter().map(story_row_line).collect()
}

pub(super) fn story_row_line(step: &HistoryStep) -> EventRow {
    let at = step.summary_at.or(step.elapsed);
    let (tail, tone) = match (&step.summary, step.kind.as_ref(), step.outcome) {
        (
            _,
            Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Check),
            Some(HistoryOutcome::Check { ok, exit_code }),
        ) => {
            let mut tail = format!("{} · {}", step.label, if ok { "passed" } else { "failed" });
            if !ok && let Some(code) = exit_code {
                tail.push_str(&format!(" (exit {code})"));
            }
            (
                tail,
                if ok {
                    tui::Tone::Muted
                } else {
                    tui::Tone::Fail
                },
            )
        }
        (
            _,
            Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Command),
            Some(HistoryOutcome::Command {
                succeeded,
                exit_code,
            }),
        ) => {
            let mut tail = format!(
                "{} · {}",
                step.label,
                if succeeded { "succeeded" } else { "failed" }
            );
            if !succeeded && let Some(code) = exit_code {
                tail.push_str(&format!(" (exit {code})"));
            }
            (
                tail,
                if succeeded {
                    tui::Tone::Muted
                } else {
                    tui::Tone::Fail
                },
            )
        }
        (_, Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Command), None) => {
            (step.label.clone(), tui::Tone::Muted)
        }
        (Some(summary), _, _) => (format!("{}: {}", step.label, summary), tui::Tone::Default),
        (None, _, _) => {
            let mut tail = step.label.clone();
            if let Some(elapsed) = step.elapsed {
                tail.push_str(" \u{b7} ");
                tail.push_str(&tui::elapsed_text(elapsed));
            }
            if let Some(tokens) = step.output_tokens {
                tail.push_str(" \u{b7} ");
                tail.push_str(&tui::token_text(tokens));
            }
            (tail, tui::Tone::Muted)
        }
    };
    EventRow { at, tail, tone }
}

/// P552 shared one-row event formatter for history and current-activity
/// panes: a fixed `HH:MM:SS ` prefix (never truncated — a terminal
/// narrower than the prefix itself is left to clip it, per the P552
/// contract) followed by a cleaned tail, truncated by display width only in
/// whatever budget remains after the prefix. Each call produces exactly one
/// physical row; callers must not pass the result through
/// `tui_panes::wrapped_lines`.
pub(super) const EVENT_PREFIX_SEP: &str = " ";

pub(super) fn event_row_line(row: &EventRow, width: u16) -> tui::Line {
    let prefix = row
        .at
        .map(|at| format!("{}{EVENT_PREFIX_SEP}", tui::elapsed_text(at)))
        .unwrap_or_default();
    let prefix_width = tui::display_width(&prefix);
    let mut line = tui::Line::blank();
    line.push(prefix, tui::Tone::Muted);
    let tail = tui::clean_live_text(&row.tail);
    let budget = (width as usize).saturating_sub(prefix_width);
    line.push(
        tui::truncate_display_width_end_recording(&tail, budget),
        row.tone,
    );
    line
}

/// Renders a full set of [`EventRow`]s to physical rows at `width` — one row
/// per event, per [`event_row_line`].
pub(super) fn event_row_lines(rows: &[EventRow], width: u16) -> Vec<tui::Line> {
    rows.iter().map(|row| event_row_line(row, width)).collect()
}

/// The CURRENT step's verbatim recorded stream — narrations in narrated
/// mode, drained model-text deltas in passthrough mode — each rendered
/// through [`event_row_line`], the same P552 one-row formatter
/// [`story_history_lines`] uses. The trailing in-flight tail line (still
/// updating, not yet a discrete timestamped event) is handled separately by
/// [`stream_overlay_line`], since it needs its own [`narration_line`]
/// rendering and dedup check.
pub(super) fn story_stream_lines(state: &RunPanelState) -> Vec<EventRow> {
    state.current_stream.iter().map(stream_row_line).collect()
}

/// The CURRENT pane's trailing in-flight event, appended after the recorded
/// stream rows — `None` when there is no live narration or when its text
/// duplicates the last recorded row (so a just-landed narration is never
/// shown twice). Formatted through [`event_row_line`] like every other
/// history/current event, per the P552 one-formatter contract — the
/// in-flight line is not a special overlay outside that model.
pub(super) fn stream_overlay_line(state: &RunPanelState) -> Option<EventRow> {
    let narration = display_narration(&state.view)?;
    let last_text = state.current_stream.back().map(|row| row.text.as_str());
    let at = Instant::now().duration_since(state.run_started);
    overlay_event_row(narration, last_text, at)
}

/// Pure fold of a live [`RunNarration`] into the CURRENT pane's trailing
/// [`EventRow`], split out of [`stream_overlay_line`] so it is testable
/// without a full `RunPanelState`. `None` when the narration text duplicates
/// the last recorded stream row.
pub(super) fn overlay_event_row(
    narration: &RunNarration,
    last_recorded_text: Option<&str>,
    at: Duration,
) -> Option<EventRow> {
    if last_recorded_text == Some(narration.text.as_str()) {
        return None;
    }
    let tail = format!("{}: {}", narration.label, narration.text);
    let tone = if narration.finished {
        tui::Tone::Pass
    } else if narration.muted {
        tui::Tone::Muted
    } else {
        tui::Tone::Default
    };
    Some(EventRow {
        at: Some(at),
        tail,
        tone,
    })
}

pub(super) fn stream_row_line(row: &StreamRow) -> EventRow {
    let tone = match row.kind {
        StreamRowKind::Narration => tui::Tone::Default,
        StreamRowKind::ModelText => tui::Tone::Muted,
    };
    EventRow {
        at: Some(row.at),
        tail: row.text.clone(),
        tone,
    }
}

pub(super) fn update_live_display(state: &mut RunPanelState, display: tui::LiveDisplay) {
    state.view.narration = narration_for(&state.session, display);
}

pub(super) fn narration_for(
    session: &ctx_traits_core::procedure::session::Session,
    display: tui::LiveDisplay,
) -> Option<RunNarration> {
    Some(RunNarration {
        label: active_label(session),
        text: display.text,
        muted: display.muted,
        finished: display.finished,
    })
}

/// P552: the information pane's own bounded standing facts — session, run,
/// input, harness,
/// and one combined progress/status/current-step/elapsed/work-token/
/// narrator-token line (or, once completed, the equivalent completed-summary
/// line) — never the step journey itself, which lives in
/// [`journey_lines_with_active_row`]. Kept intentionally small so the
/// [`pane_tree`] geometry can bound this pane's height and hand the rest of
/// the left column to journey.
pub(super) fn progress_lines(view: &RunView) -> Vec<tui::Line> {
    let mut lines = Vec::new();
    render_header(&mut lines, &view.header);
    lines
}

/// P470: the journey (left) column's full, always-scrolled content — no line
/// cap, no compact-viewport degrade. Every step's mark/ports/facts renders in
/// full; the live narration itself moved to the story column's CURRENT
/// stream (see [`story_stream_lines`]) and is no longer inlined per-step.
/// Convenience wrapper over [`journey_lines_with_active_row`] for callers
/// that don't need the active row (e.g. [`render_ledger_run_view`], whose
/// ledger-only view never has an active step to anchor on).
pub(super) fn journey_lines(view: &RunView) -> Vec<JourneyRow> {
    journey_lines_with_active_row(view).0
}

/// Same output as [`journey_lines`], plus the rendered row index of the
/// ACTIVE step's [`render_step_summary`] line, and the follow ladder (0082)
/// — the one true source of the step→row mapping, since this is the only
/// place that knows how many rows each step group emits. Follow-mode
/// anchoring must derive from these row indices, never from `view.steps`'
/// own item index (a different coordinate space: each step group is 3 rows,
/// `render_step_summary` + two `render_port_line` calls, under a multi-row
/// header the caller must not hand-count either).
///
/// The ladder is the row indices of every step on the active path
/// (`RunStep::on_active_path`, e.g. an enclosing loop mid-iteration),
/// outermost first — already true of pre-order flatten order — with the
/// active row itself appended as the last (innermost) element. Empty when
/// there is no active row.
pub(super) fn journey_lines_with_active_row(
    view: &RunView,
) -> (Vec<JourneyRow>, Option<usize>, Vec<usize>) {
    let mut lines = Vec::new();
    let target_step = active_step_index(view);
    let mut active_row = None;
    let mut ladder = Vec::new();
    for (index, step) in view.steps.iter().enumerate() {
        if step.on_active_path {
            ladder.push(lines.len());
        }
        if Some(index) == target_step {
            active_row = Some(lines.len());
        }
        lines.push(JourneyRow(JourneyRowKind::Step(Box::new(step.clone()))));
    }
    if let Some(row) = active_row {
        ladder.push(row);
    } else {
        ladder.clear();
    }
    if let Some(narration) = completed_narration(view) {
        lines.push(JourneyRow(JourneyRowKind::Line(narration_line(narration))));
    }
    if view.header.completed {
        lines.push(JourneyRow(JourneyRowKind::Line(tui::Line::blank())));
        let mut line = tui::Line::blank();
        line.push("digest-stamped ", tui::Tone::Muted);
        line.push(view.header.state_digest.clone(), tui::Tone::Default);
        lines.push(JourneyRow(JourneyRowKind::Line(line)));
        lines.push(JourneyRow(JourneyRowKind::Line(tui::Line::blank())));
        let mut outputs = Vec::new();
        render_outputs_box(&mut outputs, &view.outputs);
        lines.extend(
            outputs
                .into_iter()
                .map(|line| JourneyRow(JourneyRowKind::Line(line))),
        );
    }
    (lines, active_row, ladder)
}

/// The completed-run morph requires both a completed journey and actual,
/// observed merge activity. `merge_rows` is retained for the panel lifetime,
/// so terminal transitions cannot rotate the pane tree back to history/current.
pub(super) fn post_run_lines(view: &RunView) -> Option<Vec<tui::Line>> {
    (view.header.completed && !view.merge_rows.is_empty()).then(|| {
        let mut lines = Vec::new();
        for row in &view.merge_rows {
            render_merge_row(&mut lines, row);
        }
        lines
    })
}

/// Pure fold of one [`ActivityEvent`] into `rows`, split out of
/// [`RunPanel::merge_event`] so the P549 stage/status→row-state mapping is
/// directly unit-testable without a live terminal pane.
pub(super) fn fold_merge_event(rows: &mut Vec<MergeRowView>, event: &ActivityEvent, now: Instant) {
    let terminal = match event.kind {
        ActivityKind::ValidatingOutput => Some(MergeRowState::Done),
        ActivityKind::Stalled if event.frame_id != "merge:lock" => Some(MergeRowState::Failed),
        _ => None,
    };
    let label = event
        .frame_id
        .strip_prefix("merge:")
        .unwrap_or(event.frame_id.as_str())
        .to_string();
    let detail = event.text.clone().or_else(|| event.tool.clone());
    let is_new_row = rows.iter().all(|row| row.frame_id != event.frame_id);
    if is_new_row {
        // P549: most stages record a ledger frame only on failure — a
        // stage that succeeds silently would otherwise leave its row
        // "Running" forever once a later stage's first event arrives. The
        // sole progression signal a live merge actually has is "a new
        // stage's row just appeared", so that is what closes every prior
        // still-open row to Done — a park/failure closes its own row via
        // `terminal` above and is left untouched here.
        for row in rows
            .iter_mut()
            .filter(|row| row.state == MergeRowState::Running)
        {
            row.state = MergeRowState::Done;
            row.finished.get_or_insert(now);
        }
    }
    match rows.iter_mut().find(|row| row.frame_id == event.frame_id) {
        Some(row) => {
            if detail.is_some() {
                row.detail = detail;
            }
            if let Some(terminal) = terminal {
                row.state = terminal;
                row.finished.get_or_insert(now);
            }
        }
        None => {
            rows.push(MergeRowView {
                frame_id: event.frame_id.clone(),
                label,
                state: terminal.unwrap_or(MergeRowState::Running),
                detail,
                started: now,
                finished: terminal.map(|_| now),
            });
        }
    }
}

pub(super) fn render_merge_row(lines: &mut Vec<tui::Line>, row: &MergeRowView) {
    let (mark, tone) = match row.state {
        MergeRowState::Done => ("✓", tui::Tone::Pass),
        MergeRowState::Running => ("~", tui::Tone::Warn),
        MergeRowState::Failed => ("×", tui::Tone::Fail),
    };
    let mut line = tui::Line::blank();
    line.push(mark, tone);
    line.push(" ", tui::Tone::Muted);
    line.push(row.label.clone(), tone);
    if let Some(detail) = &row.detail {
        line.push("   ", tui::Tone::Muted);
        line.push(detail.clone(), tui::Tone::Default);
    }
    let elapsed = row.finished.unwrap_or_else(Instant::now) - row.started;
    line.push(" (", tui::Tone::Muted);
    line.push(tui::elapsed_text(elapsed), tui::Tone::Muted);
    line.push(")", tui::Tone::Muted);
    lines.push(line);
}

/// P552: the information pane's standing facts, including compact session and
/// run identity. The completion digest stamp remains in
/// [`journey_lines_with_active_row`] as a terminal-outputs fact.
pub(super) fn render_header(lines: &mut Vec<tui::Line>, header: &RunHeader) {
    identifier_line(lines, "session", &header.session_id, "session-");
    identifier_line(lines, "run", &header.run_id, "run-");

    let mut line = tui::Line::blank();
    line.push("input ", tui::Tone::Muted);
    line.push(header.input.clone(), tui::Tone::Default);
    lines.push(line);

    let mut line = tui::Line::blank();
    line.push("harness ", tui::Tone::Muted);
    line.push(header.harnesses.clone(), tui::Tone::Default);
    lines.push(line);

    if header.completed {
        let mut line = tui::Line::blank();
        line.push("✓ complete", tui::Tone::Pass);
        line.push(
            format!(
                " · {} steps · {} harnesses",
                header.total, header.harness_count
            ),
            tui::Tone::Default,
        );
        if let Some(elapsed) = header.elapsed {
            line.push(
                format!(" · {}", tui::elapsed_text(elapsed)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.output_tokens {
            line.push(format!(" · {}", tui::token_text(tokens)), tui::Tone::Muted);
        }
        if let Some(tokens) = header.narrator_tokens {
            line.push(
                format!(" · narrator {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.guide_tokens {
            line.push(
                format!(" · guide {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(stopped) = &header.stopped {
            line.push(format!(" · {stopped}"), tui::Tone::Muted);
        }
        if header.structured_count > 0 {
            line.push(
                format!(
                    " · completed{} {}: {} open",
                    header
                        .structured_verdict
                        .as_deref()
                        .map_or(String::new(), |verdict| format!(" - {verdict},")),
                    header
                        .structured_label
                        .as_deref()
                        .unwrap_or("structured output"),
                    header.structured_count
                ),
                tui::Tone::Default,
            );
        }
        lines.push(line);
    } else {
        let mut line = tui::Line::blank();
        line.push("progress ", tui::Tone::Muted);
        line.push(
            format!("{}/{}", header.done, header.total),
            tui::Tone::Default,
        );
        line.push(" · ", tui::Tone::Muted);
        line.push(header.phase.clone(), tui::Tone::Default);
        if let Some(elapsed) = header.elapsed {
            line.push(
                format!(" · {}", tui::elapsed_text(elapsed)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.output_tokens {
            line.push(format!(" · {}", tui::token_text(tokens)), tui::Tone::Muted);
        }
        if let Some(tokens) = header.narrator_tokens {
            line.push(
                format!(" · narrator {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.guide_tokens {
            line.push(
                format!(" · guide {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        lines.push(line);
        if let Some(detail) = header.stop_detail.as_deref() {
            let mut line = tui::Line::blank();
            line.push("■ stopped ", tui::Tone::Fail);
            line.push("— ", tui::Tone::Muted);
            line.push(detail.to_string(), tui::Tone::Default);
            lines.push(line);
        }
    }
}

pub(super) fn identifier_line(lines: &mut Vec<tui::Line>, label: &str, id: &str, prefix: &str) {
    let mut line = tui::Line::blank();
    line.push(format!("{label} "), tui::Tone::Muted);
    line.push(compact_identifier(id, prefix), tui::Tone::Default);
    lines.push(line);
}

pub(super) fn compact_identifier(id: &str, prefix: &str) -> String {
    id.strip_prefix(prefix)
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

pub(super) fn narration_line(narration: &RunNarration) -> tui::Line {
    let mut line = tui::Line::blank();
    line.push("    ", tui::Tone::Muted);
    line.push(
        format!("{}:", narration.label),
        if narration.finished {
            tui::Tone::Pass
        } else {
            tui::Tone::Warn
        },
    );
    line.push(" ", tui::Tone::Muted);
    line.push(
        narration.text.clone(),
        if narration.finished {
            tui::Tone::Pass
        } else if narration.muted {
            tui::Tone::Muted
        } else {
            tui::Tone::Default
        },
    );
    line
}

pub(super) fn journey_row_lines(rows: &[JourneyRow], width: u16) -> Vec<tui::Line> {
    rows.iter()
        .map(|row| match &row.0 {
            JourneyRowKind::Step(step) => journey_step_line(step, width),
            JourneyRowKind::Line(line) => line.clone(),
        })
        .collect()
}

/// Loop-nesting depth for a step row, from its `position_path` — never a
/// render-local counter, so a resumed run indents identically (0033). A
/// container's own `position_path` carries only its *ancestor* loop
/// segments (its own loop segment is pushed onto its children's paths by
/// `child_location`, not its own), so counting segments directly nests
/// headers and bodies monotonically with no container special-case.
/// Clamped at two levels so a deeply nested procedure doesn't walk off a
/// narrow pane.
pub(super) fn journey_step_depth(step: &RunStep) -> usize {
    step.position_path
        .iter()
        .filter(|segment| segment.kind == "loop" || segment.kind == "for-each")
        .count()
        .min(2)
}

pub(super) fn journey_step_line(step: &RunStep, width: u16) -> tui::Line {
    let (mark, tone) = match step.state {
        StepState::Done => ("✓", tui::Tone::Pass),
        StepState::Running => ("~", tui::Tone::Warn),
        StepState::Pending => ("○", tui::Tone::Muted),
        StepState::Failed => ("×", tui::Tone::Fail),
    };
    let agent = step.harness.as_deref().map_or_else(
        || step.role.clone(),
        |harness| format!("{}@{harness}", step.role),
    );
    let variant = step.tags.join(" ∙ ");
    let state = step.status.clone();
    let mut suffixes = Vec::new();
    if step.structured_count > 0 {
        suffixes.push(format!("({} open)", step.structured_count));
    }
    let metrics = step.elapsed.map(|elapsed| {
        let mut text = tui::elapsed_text(elapsed);
        if let Some(tokens) = step.output_tokens {
            text.push_str(" · ");
            text.push_str(&tui::token_text(tokens));
        }
        text
    });
    let elapsed_only = step.elapsed.map(tui::elapsed_text);
    let full = [
        Some(format!("[agent] {agent}")),
        (!variant.is_empty()).then(|| format!("[variant] {variant}")),
        Some(state.clone()),
        metrics.clone(),
    ]
    .into_iter()
    .flatten()
    .chain(suffixes.iter().cloned())
    .collect::<Vec<_>>();
    let compact = [
        Some(agent.clone()),
        (!variant.is_empty()).then_some(variant.clone()),
        Some(state.clone()),
        elapsed_only,
    ]
    .into_iter()
    .flatten()
    .chain(suffixes.iter().cloned())
    .collect::<Vec<_>>();
    let without_variant_or_metrics = std::iter::once(agent.clone())
        .chain(std::iter::once(state.clone()))
        .chain(suffixes.iter().cloned())
        .collect::<Vec<_>>();
    let without_agent = std::iter::once(state.clone())
        .chain(suffixes.iter().cloned())
        .collect::<Vec<_>>();
    let candidates = [
        full,
        compact,
        without_variant_or_metrics,
        without_agent.clone(),
    ];
    let indent = 4 * journey_step_depth(step);
    let indented_width = (width as usize).saturating_sub(indent);
    let (indent, label, fields) = candidates
        .iter()
        .find(|fields| journey_text_width(mark, &step.label, fields) <= indented_width)
        .cloned()
        .map(|fields| (indent, step.label.clone(), fields))
        .or_else(|| {
            // Indentation is the first thing to drop, so a step that only
            // fits at the full width renders flat rather than truncating
            // its label to preserve a nesting cue (0033).
            candidates
                .iter()
                .find(|fields| journey_text_width(mark, &step.label, fields) <= width as usize)
                .cloned()
                .map(|fields| (0, step.label.clone(), fields))
        })
        .unwrap_or_else(|| {
            let tail = without_agent;
            let tail_width = tail
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" · ");
            let budget = (width as usize)
                .saturating_sub(tui::display_width(mark) + 1 + 3 + tui::display_width(&tail_width));
            (
                0,
                tui::truncate_display_width_end_recording(&step.label, budget),
                tail,
            )
        });
    let mut line = tui::Line::blank();
    if indent > 0 {
        line.push(" ".repeat(indent), tui::Tone::Muted);
    }
    line.push(mark, tone);
    line.push(" ", tui::Tone::Muted);
    line.push(label, tone);
    for field in fields {
        line.push(" · ", tui::Tone::Muted);
        let field_tone = if field == step.status {
            status_tone(step)
        } else if field.starts_with('(') && field.ends_with(" open)") {
            tui::Tone::Default
        } else {
            tui::Tone::Muted
        };
        line.push(field, field_tone);
    }
    line
}

pub(super) fn journey_text_width(mark: &str, label: &str, fields: &[String]) -> usize {
    tui::display_width(mark)
        + 1
        + tui::display_width(label)
        + fields
            .iter()
            .map(|field| 3 + tui::display_width(field))
            .sum::<usize>()
}

pub(super) fn render_outputs_box(lines: &mut Vec<tui::Line>, outputs: &[RunOutput]) {
    let row_width = outputs
        .iter()
        .map(|output| output.slug.len() + 1 + output.status.len())
        .max()
        .unwrap_or(4)
        .max("outputs".len());
    let inner_width = row_width + 2;
    let label = " outputs ";
    let rule = "─".repeat(inner_width.saturating_sub(label.len()));
    let mut top = tui::Line::blank();
    top.push(format!("┌{label}{rule}┐"), tui::Tone::Muted);
    lines.push(top);

    if outputs.is_empty() {
        let mut line = tui::Line::blank();
        line.push("│ ", tui::Tone::Muted);
        line.push(format!("{:row_width$}", "none"), tui::Tone::Muted);
        line.push(" │", tui::Tone::Muted);
        lines.push(line);
    } else {
        for output in outputs {
            let text = format!("{} {}", output.slug, output.status);
            let mut line = tui::Line::blank();
            line.push("│ ", tui::Tone::Muted);
            line.push(
                format!("{text:row_width$}"),
                if output.accepted {
                    tui::Tone::Pass
                } else {
                    tui::Tone::Muted
                },
            );
            line.push(" │", tui::Tone::Muted);
            lines.push(line);
        }
    }

    let mut bottom = tui::Line::blank();
    bottom.push(format!("└{}┘", "─".repeat(inner_width)), tui::Tone::Muted);
    lines.push(bottom);
}
