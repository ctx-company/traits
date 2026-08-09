//! Line/frame rendering: turns a `RunView`/`RunPanelState` into styled
//! terminal lines and ratatui frames — the pane tree, live 2x2 grid,
//! journey/history/current rows, and the ledger-driven header/outputs box.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

use super::guide::AskPane;
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
    FollowTarget, GuideChatHandle, RunPanelState, StreamRow, StreamRowKind,
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
        state.title_generation_live,
        &state.trait_name,
        state.session.provenance.started_at_epoch,
        state
            .session
            .provenance
            .started_at_epoch
            .and_then(local_utc_offset_seconds),
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
///
/// `generation_live` is the ONE authoritative signal for whether an
/// unresolved (`None`/`InFlight`/`Retryable`) claim can still advance: `true`
/// only while a driver actually holds this ledger's P423 flock (a live drive
/// or the current invocation itself). A claim left behind by a driver that
/// has since exited — the ordinary case for a resumed-later or abandoned
/// session, never advanced by anything once nothing holds the lock — renders
/// the same blank row `Terminal` gets, instead of a "(Generating…)" claim
/// that can never come true. See `close_out_unanswered_session_title` in
/// `drive.rs` for the writer-side half: it stops NEW orphans from forming by
/// closing out a claim this drive's own worker never answered.
pub(crate) fn title_row_line(
    title_state: Option<&ctx_traits_core::procedure::session::SessionTitleState>,
    generation_live: bool,
    trait_name: &str,
    started_at_epoch: Option<u64>,
    utc_offset_seconds: Option<i32>,
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
        | Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { .. })
            if generation_live =>
        {
            line.push("(Generating session title…)".to_string(), tui::Tone::Muted);
            line.push(" \u{b7} ", tui::Tone::Muted);
        }
        None
        | Some(ctx_traits_core::procedure::session::SessionTitleState::InFlight { .. })
        | Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { .. }) => {}
    }
    line.push(trait_name.to_string(), tui::Tone::Default);
    if let Some(epoch) = started_at_epoch {
        line.push(" \u{b7} ", tui::Tone::Muted);
        line.push(
            format!("Started at {}", epoch_clock(epoch, utc_offset_seconds)),
            tui::Tone::Muted,
        );
    }
    line
}

/// The reader's UTC offset, in seconds, at the moment `epoch` occurred (not
/// "now" — DST-correct for the stamp being rendered). `None` if the C
/// library cannot resolve it, in which case the caller falls back to a
/// labelled UTC display. No `chrono`/`time` dependency exists in this
/// workspace; `libc` is already a direct dependency for termios/ioctl/signal
/// (`tui.rs`, `interrupt.rs`), so this owns nothing beyond one `localtime_r`
/// call (which applies the environment's `TZ` itself, POSIX-equivalent to a
/// `tzset` call) rather than TZif parsing.
pub(super) fn local_utc_offset_seconds(epoch: u64) -> Option<i32> {
    let epoch_time = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::localtime_r(&epoch_time, &mut tm) };
    if result.is_null() {
        None
    } else {
        Some(tm.tm_gmtoff as i32)
    }
}

/// Pure `HH:MM:SS` decomposition of a UNIX epoch, shifted by
/// `utc_offset_seconds`. `None` renders the UTC fallback labelled `UTC`;
/// `Some(0)` renders unlabelled (a genuinely-UTC locale is local time, not a
/// degradation).
pub(super) fn epoch_clock(epoch: u64, utc_offset_seconds: Option<i32>) -> String {
    let (offset, suffix) = match utc_offset_seconds {
        Some(offset) => (offset as i64, ""),
        None => (0, " UTC"),
    };
    let seconds_of_day = (epoch as i64 + offset).rem_euclid(86_400);
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}{suffix}")
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

#[cfg(test)]
mod tests {
    use super::super::guide::{GuideChat, GuideExchange};
    use super::super::model::{CURRENT_PANE, HISTORY_PANE, JOURNEY_PANE, PROGRESS_PANE};
    use super::*;
    use std::sync::{Arc, Mutex};

    fn activity_event(frame_id: &str, kind: ActivityKind, text: Option<&str>) -> ActivityEvent {
        ActivityEvent {
            sequence: 0,
            frame_id: frame_id.to_string(),
            kind,
            text: text.map(str::to_string),
            tool: None,
            tokens: None,
        }
    }

    // P549: a fresh stage-boundary event (`Dispatching`) starts a Running
    // row; a `ValidatingOutput` FrameRecorded event closes that same row

    fn step(key: &str, state: StepState, summary: Option<&str>) -> RunStep {
        RunStep {
            key: key.to_string(),
            label: key.to_string(),
            role: "worker".to_string(),
            harness: None,
            tags: Vec::new(),
            status: "done".to_string(),
            state,
            active: false,
            counts_progress: true,
            inputs: Vec::new(),
            outputs: Vec::new(),
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(1_200),
            loop_key: None,
            on_active_path: false,
            position_path: Vec::new(),
            run_index: 0,
            structured_count: 0,
            summary: summary.map(str::to_string),
            summary_at: summary.map(|_| Duration::from_secs(7)),
        }
    }

    fn history_step(label: &str, elapsed: Option<Duration>, summary: Option<&str>) -> HistoryStep {
        HistoryStep {
            label: label.to_string(),
            kind: None,
            outcome: None,
            elapsed,
            output_tokens: Some(1_200),
            summary: summary.map(str::to_string),
            summary_at: summary.map(|_| Duration::from_secs(7)),
        }
    }

    fn view_with(steps: Vec<RunStep>) -> RunView {
        RunView {
            header: RunHeader {
                session_id: "session-0123456789abcdef".to_string(),
                run_id: "run-fedcba9876543210".to_string(),
                input: "not provided".to_string(),
                harnesses: "unassigned".to_string(),
                done: 0,
                total: 0,
                phase: "in-progress".to_string(),
                completed: false,
                stopped: None,
                stop_detail: None,
                state_digest: String::new(),
                harness_count: 0,
                elapsed: None,
                output_tokens: None,
                narrator_tokens: None,
                guide_tokens: None,
                structured_count: 0,
                structured_label: None,
                structured_verdict: None,
            },
            steps,
            history: Vec::new(),
            narration: None,
            outputs: Vec::new(),
            merge_rows: Vec::new(),
        }
    }

    fn line_text(line: &tui::Line) -> String {
        line.segments().map(|(text, _)| text).collect()
    }

    fn loop_path_segment(iteration: usize) -> ctx_traits_core::procedure::runtime::PathSegment {
        ctx_traits_core::procedure::runtime::PathSegment {
            kind: "loop".to_string(),
            id: Some("round".to_string()),
            index: 0,
            iteration: Some(iteration),
            item_index: None,
        }
    }

    fn item_path_segment(id: &str) -> ctx_traits_core::procedure::runtime::PathSegment {
        ctx_traits_core::procedure::runtime::PathSegment {
            kind: "item".to_string(),
            id: Some(id.to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }
    }

    fn procedure_path_segment() -> ctx_traits_core::procedure::runtime::PathSegment {
        ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: None,
            index: 0,
            iteration: None,
            item_index: None,
        }
    }

    fn sample_lines(n: usize) -> Vec<tui::Line> {
        (0..n)
            .map(|index| {
                let mut line = tui::Line::blank();
                line.push(format!("line{index}"), tui::Tone::Default);
                line
            })
            .collect()
    }

    fn sample_journey_rows(n: usize) -> Vec<JourneyRow> {
        sample_lines(n)
            .into_iter()
            .map(|line| JourneyRow(JourneyRowKind::Line(line)))
            .collect()
    }

    fn sample_event_rows(n: usize) -> Vec<EventRow> {
        (0..n)
            .map(|index| {
                EventRow::new(
                    Some(Duration::from_secs(index as u64)),
                    format!("event{index}"),
                    tui::Tone::Default,
                )
            })
            .collect()
    }

    // P552: a narrow terminal stacks every populated pane instead of
    // dropping any of them — the pre-P552 `live_pane_tree` silently omitted
    // `history` at narrow widths (reviewer blocker `live-run-pane-contract-

    // P549: a fresh stage-boundary event (`Dispatching`) starts a Running
    // row; a `ValidatingOutput` FrameRecorded event closes that same row
    // (by `frame_id`) as Done, freezing its elapsed time.
    #[test]
    fn fold_merge_event_opens_running_then_closes_done_on_the_same_row() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let started = Instant::now();
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::Dispatching,
                Some("starting gates"),
            ),
            started,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, MergeRowState::Running);
        assert!(rows[0].finished.is_none());

        let finished = started + Duration::from_secs(5);
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::ValidatingOutput,
                Some("gates passed"),
            ),
            finished,
        );
        assert_eq!(rows.len(), 1, "same frame_id must fold into one row");
        assert_eq!(rows[0].state, MergeRowState::Done);
        assert_eq!(rows[0].detail.as_deref(), Some("gates passed"));
        assert_eq!(rows[0].finished, Some(finished));
    }

    // P549: `Stalled` on any frame_id OTHER than "merge:lock" is a terminal
    // park/failure — the row must read Failed, never Running.
    #[test]
    fn fold_merge_event_stalled_marks_failed_except_at_the_lock_frame() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let now = Instant::now();
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::Stalled,
                Some("post-run gate failed"),
            ),
            now,
        );
        assert_eq!(rows[0].state, MergeRowState::Failed);

        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:lock",
                ActivityKind::Stalled,
                Some("waiting for the merge lock"),
            ),
            now,
        );
        let lock_row = rows
            .iter()
            .find(|row| row.frame_id == "merge:lock")
            .unwrap();
        assert_eq!(lock_row.state, MergeRowState::Running);
    }

    // P549 blocker `merge-rows-do-not-track-stage-progression`: on a
    // happy-path merge, most stages record no ledger frame until the NEXT
    // stage's first event arrives — the only progression signal a live
    // merge has. A stage entered earlier must read Done once a later
    // stage's row appears, not stay Running forever; `merge:gates` (the
    // final `StageEntered` before this trace's terminal `Merged`-frame
    // stand-in) is left Running here on purpose — this test traces
    // `StageEntered` events only, exactly what a park-free happy path emits
    // for every stage but the last, whose own terminal frame closes it.
    #[test]
    fn fold_merge_event_closes_the_prior_stage_row_when_the_next_stage_starts() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let now = Instant::now();
        let stage_order = [
            "merge:lock",
            "merge:preflight",
            "merge:rebase",
            "merge:reconciliation",
            "merge:gates",
            "merge:post-run",
            "merge:cleanup",
        ];
        for (index, frame_id) in stage_order.iter().enumerate() {
            fold_merge_event(
                &mut rows,
                &activity_event(
                    frame_id,
                    ActivityKind::Dispatching,
                    Some(&format!("starting {frame_id}")),
                ),
                now + Duration::from_secs(index as u64),
            );
        }
        assert_eq!(rows.len(), stage_order.len());
        for frame_id in &stage_order[..stage_order.len() - 1] {
            let row = rows.iter().find(|row| &row.frame_id == frame_id).unwrap();
            assert_eq!(
                row.state,
                MergeRowState::Done,
                "{frame_id} must read Done once a later stage's row appeared"
            );
            assert!(row.finished.is_some());
        }
        let last = rows.last().unwrap();
        assert_eq!(last.frame_id, "merge:cleanup");
        assert_eq!(
            last.state,
            MergeRowState::Running,
            "the most recently entered stage stays Running until its own terminal frame"
        );
        assert!(last.finished.is_none());
    }

    #[test]
    fn story_row_line_keeps_typed_verdicts_and_omits_unknown_command_facts() {
        use ctx_traits_core::procedure::run::PlannedSequenceKind;

        let check = HistoryStep {
            label: "check".to_string(),
            kind: Some(PlannedSequenceKind::Check),
            outcome: Some(HistoryOutcome::Check {
                ok: false,
                exit_code: Some(7),
            }),
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(10),
            summary: Some("summary must not suppress verdict".to_string()),
            summary_at: Some(Duration::from_secs(9)),
        };
        let rendered = story_row_line(&check);
        assert_eq!(rendered.tone, tui::Tone::Fail);
        assert!(rendered.tail.contains("failed (exit 7)"));
        assert!(!rendered.tail.contains("summary"));

        let command = HistoryStep {
            label: "command".to_string(),
            kind: Some(PlannedSequenceKind::Command),
            outcome: None,
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(10),
            summary: None,
            summary_at: None,
        };
        let rendered = story_row_line(&command);
        assert_eq!(rendered.tail, "command");
        assert!(!rendered.tail.contains("00:00:05"));

        let check = |ok, exit_code, summary: Option<&str>| {
            story_row_line(&HistoryStep {
                label: "check".to_string(),
                kind: Some(PlannedSequenceKind::Check),
                outcome: Some(HistoryOutcome::Check { ok, exit_code }),
                elapsed: Some(Duration::from_secs(5)),
                output_tokens: Some(10),
                summary: summary.map(str::to_string),
                summary_at: None,
            })
        };
        let passed = check(true, Some(0), None);
        assert_eq!(passed.tone, tui::Tone::Muted);
        assert!(passed.tail.contains("passed"));
        assert!(!passed.tail.contains("exit"));
        let failed = check(false, Some(9), Some("must not replace verdict"));
        assert_eq!(failed.tone, tui::Tone::Fail);
        assert!(failed.tail.contains("failed (exit 9)"));
        assert!(!failed.tail.contains("replace verdict"));

        let unknown_check = story_row_line(&HistoryStep {
            label: "check".to_string(),
            kind: Some(PlannedSequenceKind::Check),
            outcome: None,
            elapsed: None,
            output_tokens: None,
            summary: None,
            summary_at: None,
        });
        assert_eq!(unknown_check.tone, tui::Tone::Muted);
        assert_eq!(unknown_check.tail, "check");

        let command = |succeeded, exit_code| {
            story_row_line(&HistoryStep {
                label: "command".to_string(),
                kind: Some(PlannedSequenceKind::Command),
                outcome: Some(HistoryOutcome::Command {
                    succeeded,
                    exit_code,
                }),
                elapsed: Some(Duration::from_secs(5)),
                output_tokens: Some(10),
                summary: Some("summary".to_string()),
                summary_at: None,
            })
        };
        let succeeded = command(true, Some(0));
        assert_eq!(succeeded.tone, tui::Tone::Muted);
        assert!(succeeded.tail.contains("succeeded"));
        let failed = command(false, Some(2));
        assert_eq!(failed.tone, tui::Tone::Fail);
        assert!(failed.tail.contains("failed (exit 2)"));
    }

    #[test]
    fn progress_lines_show_compact_session_and_run_identifiers() {
        let lines = progress_lines(&view_with(Vec::new()));
        assert_eq!(line_text(&lines[0]), "session 0123456789ab");
        assert_eq!(line_text(&lines[1]), "run fedcba987654");
    }

    #[test]
    fn post_run_morph_requires_completion_and_observed_merge_work() {
        let merge_row = MergeRowView {
            frame_id: "merge:post-run".to_string(),
            label: "post-run".to_string(),
            state: MergeRowState::Done,
            detail: None,
            started: Instant::now(),
            finished: Some(Instant::now()),
        };
        let mut incomplete = view_with(Vec::new());
        incomplete.merge_rows.push(merge_row.clone());
        assert!(post_run_lines(&incomplete).is_none());

        let mut no_work = view_with(Vec::new());
        no_work.header.completed = true;
        assert!(post_run_lines(&no_work).is_none());

        incomplete.header.completed = true;
        assert!(post_run_lines(&incomplete).is_some());
    }

    #[test]
    fn compact_identifier_handles_short_and_nonstandard_values() {
        assert_eq!(compact_identifier("session-short", "session-"), "short");
        assert_eq!(compact_identifier("custom-id", "session-"), "custom-id");
    }

    // P470's plan projection still only supplies journey rows; history is
    // now populated from accepted ledger executions.
    #[test]
    fn story_history_lines_only_covers_done_steps_in_plan_order() {
        let mut view = view_with(vec![
            step("a", StepState::Done, None),
            step("b", StepState::Running, None),
            step("c", StepState::Done, Some("finished cleanly")),
            step("d", StepState::Pending, None),
        ]);
        view.history = vec![
            history_step("a", Some(Duration::from_secs(5)), None),
            history_step("c", Some(Duration::from_secs(7)), Some("finished cleanly")),
        ];
        let lines = story_history_lines(&view);
        assert_eq!(lines.len(), 2);
    }

    // A step with a landed P455 summary joins its row; one without falls
    // back to the truthful facts line — never a placeholder.
    #[test]
    fn story_row_line_prefers_summary_over_facts_fallback() {
        let with_summary = story_row_line(&history_step(
            "a",
            Some(Duration::from_secs(5)),
            Some("did the thing"),
        ));
        assert_eq!(with_summary.at, Some(Duration::from_secs(7)));
        assert!(with_summary.tail.contains("did the thing"));

        let without_summary =
            story_row_line(&history_step("b", Some(Duration::from_secs(5)), None));
        assert_eq!(without_summary.at, Some(Duration::from_secs(5)));
        assert!(without_summary.tail.contains('5')); // elapsed
        assert!(without_summary.tail.contains("tok")); // tokens
    }

    #[test]
    fn stream_row_line_tones_narration_and_model_text_differently() {
        let narration = stream_row_line(&StreamRow {
            at: Duration::from_secs(3),
            kind: StreamRowKind::Narration,
            text: "thinking about it".to_string(),
        });
        let model_text = stream_row_line(&StreamRow {
            at: Duration::from_secs(3),
            kind: StreamRowKind::ModelText,
            text: "raw delta".to_string(),
        });
        assert_eq!(narration.at, Some(Duration::from_secs(3)));
        assert_eq!(model_text.at, Some(Duration::from_secs(3)));
        assert_eq!(narration.tone, tui::Tone::Default);
        assert_eq!(model_text.tone, tui::Tone::Muted);
    }

    // P552: every history/current-activity row shares one formatter
    // (`event_row_line`) that reserves the fixed `HH:MM:SS ` prefix and
    // truncates only the tail, by display width, so wide/combining Unicode
    // never desyncs the truncation point from a plain byte/char count.
    #[test]
    fn event_row_line_truncates_only_the_tail_by_display_width() {
        tui_select::clear_ledger();
        let row = EventRow {
            at: Some(Duration::from_secs(5)),
            tail: "a".repeat(50),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 20);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:00:05 "));
        assert!(rendered.ends_with("..."));
        assert!(tui::display_width(&rendered) <= 20);
        // Task 0023: the truncation was recorded, so a selection spanning
        // this row expands back to the full untruncated tail on copy.
        assert_eq!(
            tui_select::substitute_ledger(&rendered),
            format!("00:00:05 {}", row.tail)
        );
        tui_select::clear_ledger();
    }

    #[test]
    fn event_row_line_wide_unicode_tail_truncates_by_display_width_not_char_count() {
        tui_select::clear_ledger();
        // Each "文" is 2 display columns; a char-count truncation would
        // overflow the requested width, a display-width one will not.
        let row = EventRow {
            at: Some(Duration::from_secs(1)),
            tail: "文".repeat(30),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 25);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(tui::display_width(&rendered) <= 25);
        assert!(rendered.ends_with("..."));
        assert_eq!(
            tui_select::substitute_ledger(&rendered),
            format!("00:00:01 {}", row.tail)
        );
        tui_select::clear_ledger();
    }

    #[test]
    fn event_row_line_leaves_a_short_tail_unmarked() {
        let row = EventRow {
            at: Some(Duration::from_secs(9)),
            tail: "short".to_string(),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 80);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:00:09 short"));
        assert!(!rendered.ends_with("..."));
    }

    #[test]
    fn event_row_line_without_timestamp_uses_the_full_tail_budget() {
        let row = EventRow {
            at: None,
            tail: "a".repeat(30),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 20);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(!rendered.contains("00:00:00"));
        assert!(!rendered.starts_with(EVENT_PREFIX_SEP));
        assert_eq!(rendered, "a".repeat(17) + "...");
        assert_eq!(tui::display_width(&rendered), 20);
    }

    // P552: the CURRENT pane's in-flight line is not a special overlay
    // outside the event model — it folds into an `EventRow` using the
    // current run-relative timestamp, so it renders through the exact same
    // `event_row_line` prefix/truncation contract as every recorded event.
    #[test]
    fn overlay_event_row_uses_the_shared_event_contract() {
        let narration = RunNarration {
            label: "narrator".to_string(),
            text: "文".repeat(30),
            muted: false,
            finished: false,
        };
        let row = overlay_event_row(&narration, None, Duration::from_secs(65))
            .expect("narration text differs from last recorded row");
        assert_eq!(row.at, Some(Duration::from_secs(65)));
        assert!(row.tail.starts_with("narrator: "));
        let line = event_row_line(&row, 25);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:01:05 "));
        assert!(tui::display_width(&rendered) <= 25);
        assert!(rendered.ends_with("..."));
    }

    #[test]
    fn overlay_event_row_suppresses_a_duplicate_of_the_last_recorded_row() {
        let narration = RunNarration {
            label: "narrator".to_string(),
            text: "same text".to_string(),
            muted: false,
            finished: true,
        };
        assert!(overlay_event_row(&narration, Some("same text"), Duration::from_secs(1)).is_none());
    }

    // P552: each logical history/current-activity input becomes exactly one
    // physical row — no `wrapped_lines` pass may re-split it.
    #[test]
    fn event_row_lines_produces_exactly_one_row_per_input() {
        let rows = vec![
            EventRow {
                at: Some(Duration::from_secs(1)),
                tail: "one".to_string(),
                tone: tui::Tone::Default,
            },
            EventRow {
                at: Some(Duration::from_secs(2)),
                tail: "x".repeat(200),
                tone: tui::Tone::Default,
            },
        ];
        let lines = event_row_lines(&rows, 30);
        assert_eq!(lines.len(), rows.len());
    }

    // P470 blocker `tree-follow-anchor-unit-mismatch`: the follow anchor
    // handed to the pane must be the RENDERED ROW index of the active step's
    // `render_step_summary` line, never its `view.steps` item index — each
    // step group is 3 rows (summary + two port lines) under a multi-row
    // header, so the two coordinate spaces disagree for any step past the
    // first.
    #[test]
    fn render_tree_lines_with_active_row_returns_a_row_index_not_a_step_index() {
        let mut active_step = step("c", StepState::Running, None);
        active_step.active = true;
        let view = view_with(vec![
            step("a", StepState::Done, None),
            step("b", StepState::Done, None),
            active_step,
            step("d", StepState::Pending, None),
        ]);
        let (lines, active_row, _ladder) = journey_lines_with_active_row(&view);
        let active_row = active_row.expect("an active step must yield a row anchor");
        assert_eq!(active_row, 2, "each preceding step contributes one row");
        let rendered = journey_row_lines(&lines, 120);
        let label = rendered[active_row]
            .segments()
            .nth(2)
            .map(|(text, _)| text.to_string());
        assert_eq!(
            label,
            Some("c".to_string()),
            "row {active_row} must be the active step's own render_step_summary line"
        );
    }

    #[test]
    fn journey_content_omits_the_pane_title() {
        let lines = journey_lines(&view_with(vec![step("work", StepState::Running, None)]));
        assert!(
            journey_row_lines(&lines, 120)
                .iter()
                .all(|line| line_text(line) != "journey"),
            "the pane border is the only journey title"
        );
    }

    #[test]
    fn journey_step_full_line_labels_agent_variant_and_facts() {
        let mut step = step("deploy", StepState::Running, None);
        step.role = "reviewer".to_string();
        step.harness = Some("codex".to_string());
        step.tags = vec!["fast".to_string(), "branch-a".to_string()];
        step.status = "running".to_string();
        step.structured_count = 2;
        assert_eq!(
            line_text(&journey_step_line(&step, 120)),
            "~ deploy · [agent] reviewer@codex · [variant] fast ∙ branch-a · running · 00:00:05 · 1.2k tok · (2 open)"
        );
    }

    #[test]
    fn journey_step_indents_by_loop_nesting_depth_from_position_path() {
        // Container paths never carry their own loop segment — a loop
        // pushes its segment onto its *children's* paths, not its own
        // (`child_location`) — so a top-level container sits flush left.
        let mut top_level_container = step("round", StepState::Done, None);
        top_level_container.loop_key = Some("round".to_string());
        top_level_container.position_path = vec![procedure_path_segment()];
        assert!(
            !line_text(&journey_step_line(&top_level_container, 120)).starts_with(' '),
            "a top-level loop container renders flat"
        );

        // A body step under that loop carries the loop's segment and
        // indents one level.
        let mut body_step = step("produce", StepState::Done, None);
        body_step.position_path = vec![procedure_path_segment(), loop_path_segment(1)];
        assert!(line_text(&journey_step_line(&body_step, 120)).starts_with("    ✓"));

        // A nested loop container one level in carries its ancestor's
        // segment and indents one level too — headers nest under their
        // enclosing loop just like bodies do.
        let mut nested_container = step("inner round", StepState::Done, None);
        nested_container.loop_key = Some("inner round".to_string());
        nested_container.position_path = vec![procedure_path_segment(), loop_path_segment(1)];
        assert!(
            line_text(&journey_step_line(&nested_container, 120)).starts_with("    ✓"),
            "a nested loop container indents under its enclosing loop"
        );
    }

    #[test]
    fn journey_step_nesting_clamps_at_two_levels() {
        let mut doubly_nested = step("inner", StepState::Done, None);
        doubly_nested.position_path = vec![
            loop_path_segment(0),
            loop_path_segment(0),
            item_path_segment("inner"),
        ];
        assert!(line_text(&journey_step_line(&doubly_nested, 120)).starts_with("        ✓"));

        let mut triply_nested = step("deepest", StepState::Done, None);
        triply_nested.position_path = vec![
            loop_path_segment(0),
            loop_path_segment(0),
            loop_path_segment(0),
            item_path_segment("deepest"),
        ];
        assert!(
            line_text(&journey_step_line(&triply_nested, 120)).starts_with("        ✓"),
            "a third loop level must clamp at the same indent as two levels"
        );
    }

    #[test]
    fn journey_step_drops_indentation_before_truncating_the_label() {
        let mut nested = step("deploy the fleet", StepState::Running, None);
        nested.position_path = vec![loop_path_segment(0), item_path_segment("deploy")];
        nested.status = "running".to_string();
        let rendered = line_text(&journey_step_line(&nested, 30));
        assert!(
            !rendered.starts_with(' '),
            "indentation must drop before the label is truncated: {rendered}"
        );
        assert!(
            rendered.contains("deploy the fleet"),
            "the label must survive untruncated once indentation drops: {rendered}"
        );
    }

    #[test]
    fn journey_step_degrades_by_display_width_without_losing_state() {
        let mut step = step("文文 deploy", StepState::Running, None);
        step.harness = Some("codex".to_string());
        step.tags = vec!["fast".to_string(), "branch-a".to_string()];
        step.status = "running".to_string();
        step.structured_count = 2;
        for (width, absent) in [
            (120, ""),
            (65, "[agent]"),
            (45, "branch-a"),
            (30, "worker@codex"),
            (25, "文文 deploy"),
        ] {
            let rendered = line_text(&journey_step_line(&step, width));
            assert!(rendered.contains('~'));
            assert!(rendered.contains("running"));
            assert!(rendered.contains("(2 open)"));
            assert!(tui::display_width(&rendered) <= width as usize);
            if !absent.is_empty() {
                assert!(!rendered.contains(absent), "{rendered}");
            }
        }
    }

    #[test]
    fn journey_steps_are_one_row_and_never_render_ports() {
        let view = view_with(vec![
            step("first", StepState::Done, None),
            step("second", StepState::Pending, None),
        ]);
        let rows = journey_lines(&view);
        let rendered = journey_row_lines(&rows, 120);
        assert_eq!(rendered.len(), 2);
        assert!(rendered.iter().all(|line| {
            let text = line_text(line);
            !text.starts_with("    in ") && !text.starts_with("    out ")
        }));
    }

    #[test]
    fn render_tree_lines_with_active_row_is_none_when_no_step_is_selectable() {
        let (_, active_row, ladder) = journey_lines_with_active_row(&view_with(Vec::new()));
        assert_eq!(active_row, None);
        assert!(
            ladder.is_empty(),
            "no active step must yield an empty ladder"
        );
    }

    // P552: a narrow terminal stacks every populated pane instead of
    // dropping any of them — the pre-P552 `live_pane_tree` silently omitted
    // `history` at narrow widths (reviewer blocker `live-run-pane-contract-
    // absent`); this proves the replacement never does.
    #[test]
    fn narrow_full_data_stacks_every_pane_without_omission() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(10);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::Visible(&title_row_line(None, true, "trait", None, None)),
        };
        let area = Rect::new(0, 0, 80, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
        );
    }

    // Normal-width full data: the bounded-progress 2x2 grid — progress is
    // bounded to its own content height, journey receives the rest of the
    // left column, history retains up to half the right column, and current
    // receives the remainder while keeping its content floor.
    #[test]
    fn wide_full_data_produces_bounded_progress_2x2_grid() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
        );
        let layout = tree.resolve(area);
        let progress_rect = layout.rect(PROGRESS_PANE).expect("progress");
        let journey_rect = layout.rect(JOURNEY_PANE).expect("journey");
        let history_rect = layout.rect(HISTORY_PANE).expect("history");
        let current_rect = layout.rect(CURRENT_PANE).expect("current");
        assert_eq!(progress_rect.height, 5, "3 content rows + 2 borders");
        assert!(
            journey_rect.height > progress_rect.height,
            "journey must receive the rest of the left column"
        );
        assert_eq!(history_rect.height, area.height / 2);
        assert_eq!(current_rect.height, area.height - history_rect.height);
        assert!(history_rect.height >= HISTORY_MIN_OUTER_ROWS);
        assert!(current_rect.height >= CURRENT_MIN_OUTER_ROWS);
        assert!(tui_panes::pane_inner(current_rect).height >= 6);
        assert_eq!(progress_rect.x, journey_rect.x, "left column shares an x");
        assert_eq!(history_rect.x, current_rect.x, "right column shares an x");
        assert!(
            history_rect.x > progress_rect.x,
            "right column is to the right"
        );
        assert_eq!(journey_rect.y + journey_rect.height, area.y + area.height);
        assert_eq!(current_rect.y + current_rect.height, area.y + area.height);
        assert_eq!(journey_rect.x + journey_rect.width, history_rect.x);
        assert_eq!(current_rect.x + current_rect.width, area.x + area.width);
    }

    #[test]
    fn post_run_replaces_the_complete_right_column_after_completion() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(100);
        let post_run = sample_lines(20);
        let area = Rect::new(0, 0, 120, 24);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: Some(&post_run),
            title: PaneTitleRow::None,
        };
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        let layout = tree.resolve(area);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, CURRENT_PANE]
        );
        assert_eq!(tree.title(CURRENT_PANE), Some("post-run"));
        assert!(layout.rect(HISTORY_PANE).is_none());
        let post_run_rect = layout.rect(CURRENT_PANE).expect("post-run");
        assert_eq!(post_run_rect.y, area.y);
        assert_eq!(post_run_rect.height, area.height);
        assert_eq!(
            post_run_rect.x,
            layout.rect(PROGRESS_PANE).expect("progress").x + 72
        );
    }

    #[test]
    fn wide_short_history_yields_unused_rows_to_current_activity() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(4);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);

        assert_eq!(layout.rect(HISTORY_PANE).expect("history").height, 6);
        assert_eq!(layout.rect(CURRENT_PANE).expect("current").height, 18);
    }

    #[test]
    fn wide_current_activity_length_does_not_change_history_allocation() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let short_current = sample_event_rows(1);
        let long_current = sample_event_rows(100);
        let area = Rect::new(0, 0, 120, 24);
        let short_data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&short_current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let long_data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&long_current),
            post_run: None,
            title: PaneTitleRow::None,
        };

        let short = pane_tree(&LIVE_PANE_IDS, area, &short_data).resolve(area);
        let long = pane_tree(&LIVE_PANE_IDS, area, &long_data).resolve(area);
        assert_eq!(short.rect(HISTORY_PANE), long.rect(HISTORY_PANE));
    }

    #[test]
    fn smallest_supported_wide_body_keeps_history_and_current_floors() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(100);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, HISTORY_MIN_OUTER_ROWS + CURRENT_MIN_OUTER_ROWS);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);
        let history = layout.rect(HISTORY_PANE).expect("history");
        let current = layout.rect(CURRENT_PANE).expect("current");

        assert_eq!(history.height, HISTORY_MIN_OUTER_ROWS);
        assert_eq!(current.height, CURRENT_MIN_OUTER_ROWS);
        assert_eq!(history.y + history.height, current.y);
        assert_eq!(current.y + current.height, area.y + area.height);
    }

    // Dashboard preview supplies only progress/journey — this must produce
    // exactly those two leaves, never four.
    #[test]
    fn preview_data_produces_exactly_progress_and_journey() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: None,
            current: None,
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(tree.leaf_ids(), vec![PROGRESS_PANE, JOURNEY_PANE]);
        assert_eq!(tree.title(PROGRESS_PANE), Some("information"));
        assert_eq!(tree.title(JOURNEY_PANE), Some("journey"));
    }

    #[test]
    fn information_title_and_pane_ids_survive_the_narrow_breakpoint() {
        let progress = sample_lines(5);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(5);
        let current = sample_event_rows(5);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        for width in [108, 109] {
            let area = Rect::new(0, 0, width, 30);
            let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
            assert_eq!(tree.title(PROGRESS_PANE), Some("information"));
            assert_eq!(
                tree.leaf_ids(),
                vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
            );
            let layout = tree.resolve(area);
            for id in tree.leaf_ids() {
                assert!(
                    layout
                        .rect(id)
                        .is_some_and(|rect| rect.width > 0 && rect.height > 0)
                );
            }
        }
    }

    #[test]
    fn narrow_full_data_tiles_the_supplied_area_to_its_edges() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(10);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 80, 24);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);
        let panes = [PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
            .map(|id| layout.rect(id).expect("all supplied panes resolve"));
        assert!(
            panes
                .iter()
                .all(|rect| rect.x + rect.width == area.x + area.width)
        );
        assert_eq!(panes[0].y, area.y);
        assert_eq!(panes[3].y + panes[3].height, area.y + area.height);
        assert!(
            panes
                .windows(2)
                .all(|pair| pair[0].y + pair[0].height == pair[1].y)
        );
    }

    // Source-driven omission (a legacy session with no activity sidecar):
    // only the missing pane is dropped, and focus reconciliation never
    // lands on an undrawn pane.
    #[test]
    fn source_omission_drops_only_the_missing_pane_leaving_focus_valid() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: None,
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, CURRENT_PANE]
        );
        let layout = tree.resolve(area);
        let ids = drawable_pane_ids(&tree, &layout);
        let mut focus = FocusRing::new(vec![HISTORY_PANE]);
        focus.reconcile(ids.clone(), CURRENT_PANE);
        assert!(focus.current().is_some_and(|id| ids.contains(&id)));
    }

    #[test]
    fn focus_ring_contains_only_drawable_panes_at_small_sizes() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        for area in [
            Rect::new(0, 0, 80, 6),
            Rect::new(0, 0, 80, 7),
            Rect::new(0, 0, 120, 6),
            Rect::new(0, 0, 120, 7),
        ] {
            let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
            let layout = tree.resolve(area);
            let ids = drawable_pane_ids(&tree, &layout);
            let mut focus = FocusRing::new(vec![CURRENT_PANE]);
            focus.reconcile(ids.clone(), CURRENT_PANE);
            for _ in 0..ids.len() {
                assert!(ids.contains(&focus.current().expect("drawable focus")));
                focus.next();
            }
        }
    }

    #[test]
    fn tiny_narrow_frame_keeps_current_activity_drawable() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let overlay = EventRow {
            at: Some(Duration::from_secs(0)),
            tail: "unique current activity".to_string(),
            tone: tui::Tone::Default,
        };
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = Vec::new();
        // P552: narrow now stacks all four panes (never omitting any),
        // so this terminal must be tall enough for all four Min-bounded
        // constraints (3+3+3+`CURRENT_MIN_OUTER_ROWS`) plus the title/
        // footer rows — a 6-row terminal (the pre-P552 two-pane fixture)
        // could no longer fit even the four panes' own minimums.
        let mut terminal = Terminal::new(TestBackend::new(80, 22)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &[],
                        journey_lines: &[],
                        journey_ladder: &[],
                        history_rows: &[],
                        current_rows: std::slice::from_ref(&overlay),
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                )
            })
            .expect("draw");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("unique current"));
    }

    /// P552: pending titles use a stable visible row; resolved titles render
    /// bold with the trait name and start clock following them.
    #[test]
    fn title_row_is_pending_before_success_and_bold_after() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![PROGRESS_PANE]);
        let mut keys = Vec::new();
        let pending_title_line =
            title_row_line(None, true, "implement-phase", Some(3_723), Some(0));
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &pending_title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        journey_ladder: &[],
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let pending_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(
            pending_row.trim_end(),
            "(Generating session title…) · implement-phase · Started at 01:02:03"
        );

        let title_state = ctx_traits_core::procedure::session::SessionTitleState::Resolved {
            attempts: 1,
            title: "Refactor the merge story".to_string(),
        };
        let title_line = title_row_line(
            Some(&title_state),
            true,
            "implement-phase",
            Some(3_723),
            Some(0),
        );
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        journey_ladder: &[],
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let rendered_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert!(rendered_row.contains("Refactor the merge story"));
        assert!(rendered_row.contains("implement-phase"));
        assert!(rendered_row.contains("Started at 01:02:03"));
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((0, 0))
                .expect("title cell")
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        );

        let title_state = ctx_traits_core::procedure::session::SessionTitleState::Terminal {
            attempts: 3,
            reason: "attempt-limit-exhausted".to_string(),
        };
        let title_line = title_row_line(
            Some(&title_state),
            true,
            "implement-phase",
            Some(3_723),
            Some(0),
        );
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        journey_ladder: &[],
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let terminal_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(
            terminal_row.trim_end(),
            "implement-phase · Started at 01:02:03"
        );
    }

    // 0143: an orphaned claim (worker never answered, no driver left to
    // answer it) must render the same blank row `Terminal` gets, never a
    // "(Generating…)" claim that can never come true.
    #[test]
    fn dead_driver_hides_generating_claim_for_every_unresolved_state() {
        let dead_states = [
            None,
            Some(
                ctx_traits_core::procedure::session::SessionTitleState::InFlight {
                    owner: "stale-owner".to_string(),
                    attempts: 1,
                },
            ),
            Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { attempts: 1 }),
        ];
        for state in &dead_states {
            let line = title_row_line(state.as_ref(), false, "implement-phase", None, None);
            assert_eq!(
                line_text(&line),
                "implement-phase",
                "state {state:?} with a dead driver must render the blank/terminal row"
            );
        }
    }

    // A live driver still gets the pending claim shown — this is the only
    // case that can still actually resolve.
    #[test]
    fn live_driver_still_shows_generating_claim_for_every_unresolved_state() {
        let live_states = [
            None,
            Some(
                ctx_traits_core::procedure::session::SessionTitleState::InFlight {
                    owner: "driver-owner".to_string(),
                    attempts: 1,
                },
            ),
            Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { attempts: 1 }),
        ];
        for state in &live_states {
            let line = title_row_line(state.as_ref(), true, "implement-phase", None, None);
            assert!(
                line_text(&line).starts_with("(Generating session title…)"),
                "state {state:?} with a live driver must still show the pending claim"
            );
        }
    }

    #[test]
    fn epoch_clock_wraps_seconds_of_day() {
        assert_eq!(epoch_clock(3_723, Some(0)), "01:02:03");
        assert_eq!(epoch_clock(86_400, Some(0)), "00:00:00");
    }

    #[test]
    fn epoch_clock_applies_offset() {
        // Negative offset crossing midnight backwards: 01:02:03 UTC - 2h.
        assert_eq!(epoch_clock(3_723, Some(-2 * 3_600)), "23:02:03");
        // Non-whole-hour positive offset: 01:02:03 UTC + 5:45.
        assert_eq!(epoch_clock(3_723, Some(5 * 3_600 + 45 * 60)), "06:47:03");
        // Unknown offset degrades to a labelled UTC fallback.
        assert_eq!(epoch_clock(3_723, None), "01:02:03 UTC");
    }

    #[test]
    fn ask_pane_layout_regions_are_disjoint_at_wide_narrow_and_short_sizes() {
        for area in [
            Rect::new(0, 0, 160, 40),
            Rect::new(0, 0, 70, 20),
            Rect::new(0, 0, 70, 6),
        ] {
            let regions = live_frame_regions(area);
            assert_eq!(regions[1].width, area.width);
            assert!(regions[0].height >= 3, "body must retain usable rows");
            assert_eq!(regions[1].height, 1);
            assert!(regions[0].bottom() <= regions[1].y);
        }
    }

    #[test]
    fn ask_footer_renders_every_hint_at_narrow_width_boundary() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for width in [NARROW_WIDTH_THRESHOLD - 1, NARROW_WIDTH_THRESHOLD] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("test terminal");
            let mut scrolls = PaneScrolls::new();
            let mut progress_follow = true;
            let mut journey_follow = true;
            let mut history_follow = true;
            let mut current_follow = true;
            let mut focus = FocusRing::new(vec![CURRENT_PANE]);
            let mut keys = Vec::new();
            let ask = AskPane::default();
            terminal
                .draw(|frame| {
                    render_live_panes(
                        frame,
                        LiveFrame {
                            title_line: &tui::Line::blank(),
                            progress_lines: &[],
                            journey_lines: &[],
                            journey_ladder: &[],
                            history_rows: &[],
                            current_rows: &[],
                            post_run_lines: None,
                            scrolls: &mut scrolls,
                            progress_follow: &mut progress_follow,
                            journey_follow: &mut journey_follow,
                            history_follow: &mut history_follow,
                            current_follow: &mut current_follow,
                            focus: &mut focus,
                            pending_keys: &mut keys,
                            modal: None,
                            ask: Some(&GuideChatHandle(Arc::new(Mutex::new(GuideChat {
                                ask,
                                dispatch: Arc::new(|_, _| Ok(String::new())),
                                tokens: Default::default(),
                                results: None,
                                wake: None,
                                context: String::new(),
                            })))),
                        },
                    );
                })
                .expect("draw");
            let rendered: String = (0..width)
                .map(|x| terminal.backend().buffer().cell((x, 11)).unwrap().symbol())
                .collect();
            for hint in [
                "[?] ask",
                "[up/down] scroll",
                "[pg] page",
                "[home/end] jump",
                "[tab] pane",
                "[d] dash",
                "[q] exit",
                "[ctrl-c] kill",
            ] {
                assert!(
                    rendered.contains(hint),
                    "width {width} omits {hint}: {rendered}"
                );
            }
            assert!(!rendered.contains("Ask the guide"));
        }
    }

    #[test]
    fn pending_guide_exchange_has_single_label() {
        let ask = AskPane {
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 1,
                answer: None,
            }],
            ..AskPane::default()
        };
        assert_eq!(ask_lines(&ask), ["You: question", "Guide: thinking..."]);
    }

    #[test]
    fn guide_header_tokens_labels_live_and_reconstructed_usage() {
        let mut view = view_with(Vec::new());
        view.header.output_tokens = Some(1_000);
        view.header.narrator_tokens = Some(2_000);
        view.header.guide_tokens = Some(3_000);
        let mut lines = Vec::new();
        render_header(&mut lines, &view.header);
        let rendered = lines.iter().map(line_text).collect::<String>();
        assert!(rendered.contains("1.0k"));
        assert!(rendered.contains("narrator 2.0k"));
        assert!(rendered.contains("guide 3.0k"));
        view.header.completed = true;
        let mut completed = Vec::new();
        render_header(&mut completed, &view.header);
        assert!(
            completed
                .iter()
                .map(line_text)
                .collect::<String>()
                .contains("guide 3.0k")
        );
    }
}
