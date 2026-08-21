//! `ctx traits internal tui-demo` (hidden): exercises the full P505 pane-tree/chrome
//! kit in one screen — three named panes (rows/detail/notes) tiled by a
//! [`super::tui_panes::PaneTree`], visible focus movement (`Tab`/`Shift-Tab`
//! cycling plus `alt`+arrow directional movement), independent per-pane
//! scroll with scrollbars, a real `Tabs` bar cycling both ways, and
//! multi-select on the rows pane — while re-proving every P468 contract this
//! screen already proved: list scroll, modal confirm + single/multi-line
//! text input, and the `$EDITOR` round-trip preserving screen/selection/
//! scroll. Gated on [`super::dashboard::interactive_available`]; off a TTY
//! it refuses with a plain one-line message and a nonzero exit.
//!
//! Key-binding note: `Tab`/`Shift-Tab` (via
//! [`super::tui_panes::tab_cycle_key`]) drive the PANE focus ring, per the
//! P505 contract's own wording ("focus ring: Tab/Shift-Tab cycling"). The
//! `Tabs` widget's own forward/backward cycling — the capability that is new
//! this phase — is proved here on `[`/`]` instead, since a single screen
//! cannot bind the same physical key to two independent cycling targets;
//! `tab_cycle_key` itself is still exercised (by the focus ring) and unit
//! tested directly in `tui_panes`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::Paragraph;

use super::tui_kit::{self, MarkSet, Modal, ModalHost, ModalOutcome, ScrollList};
use super::tui_panes::{
    self, FocusRing, MoveDir, PaneId, PaneLayoutResult, PaneScrolls, PaneTree, TabStep,
};
use super::tui_ratatui::RatatuiPane;

const PANE_ROWS: PaneId = "rows";
const PANE_DETAIL: PaneId = "detail";
const PANE_NOTES: PaneId = "notes";

struct DemoRow {
    id: usize,
    name: String,
    notes: String,
}

#[derive(Clone)]
enum ActionTag {
    Exit,
    Delete(usize),
    Rename(usize),
    Notes(usize),
}

struct DemoState {
    rows: Vec<DemoRow>,
    /// Rows pane: selecting scroll, plus a key-keyed (row id, never index)
    /// multi-select mark set — P473's block-approve target.
    list: ScrollList,
    marks: MarkSet<usize>,
    /// Detail/notes panes: independent selection-less scroll per pane.
    pane_scrolls: PaneScrolls,
    focus: FocusRing,
    /// The pane tree's own area, as last resolved by [`draw`] — cached so
    /// directional focus movement at key-handling time reads the SAME rects
    /// a frame actually drew, rather than re-reading the terminal size and
    /// re-resolving the tree (cheaper, and immune to a resize landing
    /// between a draw and the next key).
    last_pane_layout: PaneLayoutResult,
    tab_titles: Vec<String>,
    tab_index: usize,
    modal_host: ModalHost<ActionTag>,
    message: Option<String>,
    quit: bool,
}

impl DemoState {
    fn new() -> Self {
        let rows: Vec<DemoRow> = (0..300)
            .map(|id| DemoRow {
                id,
                name: format!("demo-row-{id:04}"),
                notes: (0..20)
                    .map(|line| format!("row {id} note line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
            .collect();
        let mut list = ScrollList::new();
        list.set_len(rows.len());
        Self {
            rows,
            list,
            marks: MarkSet::new(),
            pane_scrolls: PaneScrolls::new(),
            focus: FocusRing::new(vec![PANE_ROWS, PANE_DETAIL, PANE_NOTES]),
            last_pane_layout: PaneLayoutResult::default(),
            tab_titles: vec![
                "overview".to_string(),
                "trust".to_string(),
                "history".to_string(),
            ],
            tab_index: 0,
            modal_host: ModalHost::new(),
            message: None,
            quit: false,
        }
    }

    fn selected_row(&self) -> Option<&DemoRow> {
        self.rows.get(self.list.selected())
    }
}

/// Entry point: `ctx traits internal tui-demo`. Refuses with a plain-text message and
/// a nonzero exit off a TTY (or under `CI`/`TERM=dumb`, per
/// `interactive_available`'s own checks).
pub(crate) fn run() -> crate::Result<()> {
    if !super::dashboard::interactive_available() {
        return Err(crate::Error::Command {
            message: "ctx traits internal tui-demo requires an interactive TTY".to_string(),
        });
    }
    let mut pane = RatatuiPane::new_forwarding_ctrl_c().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: "<tty>".to_string(),
            source,
        })
    })?;
    let mut state = DemoState::new();
    while !state.quit && !pane.detached() {
        draw(&mut pane, &mut state).map_err(|source| {
            ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                path: "<tty>".to_string(),
                source,
            })
        })?;
        let key = pane
            .poll_key(std::time::Duration::from_millis(250))
            .map_err(|source| {
                ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                    path: "<tty>".to_string(),
                    source,
                })
            })?;
        let Some(key) = key else { continue };
        handle_key(&mut pane, &mut state, key)?;
    }
    Ok(())
}

/// The demo's pane tree: rows on the left (deliberately `Min`, never
/// starved), detail/notes stacked on the right — the shape the sessions
/// screen will need (list left, preview right split top/bottom), so the API
/// is sanity-checked against the real P506 target without migrating
/// anything. The detail pane's title carries the selected row's name, so the
/// dynamic-title path (`attached: session-…`-style) is exercised here too.
fn build_tree(state: &DemoState) -> PaneTree {
    let detail_title = state
        .selected_row()
        .map(|row| format!("detail: {}", row.name))
        .unwrap_or_else(|| "detail".to_string());
    PaneTree::Split {
        dir: Direction::Horizontal,
        children: vec![
            (
                Constraint::Min(30),
                PaneTree::Leaf {
                    id: PANE_ROWS,
                    title: "rows".to_string(),
                },
            ),
            (
                Constraint::Percentage(50),
                PaneTree::Split {
                    dir: Direction::Vertical,
                    children: vec![
                        (
                            Constraint::Percentage(50),
                            PaneTree::Leaf {
                                id: PANE_DETAIL,
                                title: detail_title,
                            },
                        ),
                        (
                            Constraint::Percentage(50),
                            PaneTree::Leaf {
                                id: PANE_NOTES,
                                title: "notes".to_string(),
                            },
                        ),
                    ],
                },
            ),
        ],
    }
}

fn handle_key(pane: &mut RatatuiPane, state: &mut DemoState, key: KeyEvent) -> crate::Result<()> {
    if let Some((tag, outcome)) = state.modal_host.handle_key(&key) {
        return apply_modal_outcome(pane, state, tag, outcome);
    }
    if state.modal_host.is_open() {
        // The modal is still open and consumed this key as an edit
        // keystroke (`ModalOutcome::Pending`) — every other key routes
        // through it exclusively (the focus trap), never falling through to
        // screen-level handling below.
        return Ok(());
    }

    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.modal_host.open(
            ActionTag::Exit,
            Modal::confirm("exit demo", "Quit the kit demo?"),
        );
        return Ok(());
    }
    if key.code == KeyCode::Char('q') {
        state.modal_host.open(
            ActionTag::Exit,
            Modal::confirm("exit demo", "Quit the kit demo?"),
        );
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::ALT) {
        let dir = match key.code {
            KeyCode::Up => Some(MoveDir::Up),
            KeyCode::Down => Some(MoveDir::Down),
            KeyCode::Left => Some(MoveDir::Left),
            KeyCode::Right => Some(MoveDir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            state.focus.move_dir(dir, &state.last_pane_layout);
            return Ok(());
        }
    }

    if let Some(step) = tui_panes::tab_cycle_key(&key) {
        match step {
            TabStep::Next => state.focus.next(),
            TabStep::Prev => state.focus.prev(),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Char('[') => {
            state.tab_index =
                (state.tab_index + state.tab_titles.len() - 1) % state.tab_titles.len();
            return Ok(());
        }
        KeyCode::Char(']') => {
            state.tab_index = (state.tab_index + 1) % state.tab_titles.len();
            return Ok(());
        }
        _ => {}
    }

    if let Some(delta) = tui_kit::scroll_key(&key) {
        match state.focus.current() {
            Some(id) if id == PANE_ROWS => state.list.apply(delta, usize::MAX),
            Some(id) if id == PANE_DETAIL => {
                let len = state
                    .selected_row()
                    .map(|row| detail_lines(row).len())
                    .unwrap_or(0);
                let scroll = state.pane_scrolls.get_mut(PANE_DETAIL);
                scroll.set_len(len);
                scroll.apply(delta, usize::MAX);
            }
            Some(id) if id == PANE_NOTES => {
                let len = state
                    .selected_row()
                    .map(|row| row.notes.split('\n').count())
                    .unwrap_or(0);
                let scroll = state.pane_scrolls.get_mut(PANE_NOTES);
                scroll.set_len(len);
                scroll.apply(delta, usize::MAX);
            }
            _ => {}
        }
        return Ok(());
    }

    if state.focus.current() == Some(PANE_ROWS) {
        match key.code {
            KeyCode::Char(' ') => {
                if let Some(row) = state.selected_row() {
                    state.marks.toggle(row.id);
                }
            }
            KeyCode::Char('C') if !state.marks.is_empty() => {
                state.marks.clear();
                state.message = Some("cleared marks".to_string());
            }
            KeyCode::Char('d') => {
                if let Some(row) = state.selected_row() {
                    state.modal_host.open(
                        ActionTag::Delete(row.id),
                        Modal::confirm("delete row", format!("Delete {}?", row.name)),
                    );
                }
            }
            KeyCode::Char('r') => {
                if let Some(row) = state.selected_row() {
                    state.modal_host.open(
                        ActionTag::Rename(row.id),
                        Modal::text_input("rename row", row.name.clone(), false),
                    );
                }
            }
            KeyCode::Char('n') => {
                if let Some(row) = state.selected_row() {
                    state.modal_host.open(
                        ActionTag::Notes(row.id),
                        Modal::text_input("edit notes (multi-line)", row.notes.clone(), true),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn apply_modal_outcome(
    pane: &mut RatatuiPane,
    state: &mut DemoState,
    tag: ActionTag,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    let _ = pane;
    match (tag, outcome) {
        (_, ModalOutcome::Cancelled) => {
            state.message = Some("cancelled".to_string());
        }
        (ActionTag::Exit, ModalOutcome::Confirmed) => {
            state.quit = true;
        }
        (ActionTag::Delete(id), ModalOutcome::Confirmed) => {
            state.rows.retain(|row| row.id != id);
            state.list.set_len(state.rows.len());
            let remaining: Vec<usize> = state.rows.iter().map(|row| row.id).collect();
            state.marks.retain_existing(&remaining);
            state.message = Some(format!("deleted row {id}"));
        }
        (ActionTag::Rename(id), ModalOutcome::Submitted(text)) => {
            if let Some(row) = state.rows.iter_mut().find(|row| row.id == id) {
                row.name = text;
            }
            state.message = Some(format!("renamed row {id}"));
        }
        (ActionTag::Notes(id), ModalOutcome::Submitted(text)) => {
            if let Some(row) = state.rows.iter_mut().find(|row| row.id == id) {
                row.notes = text;
            }
            state.message = Some(format!("saved notes for row {id}"));
        }
        _ => {}
    }
    Ok(())
}

fn draw(pane: &mut RatatuiPane, state: &mut DemoState) -> std::io::Result<()> {
    pane.draw(|frame| {
        let area = frame.area();
        let regions = tui_panes::screen_regions(area);
        frame.render_widget(
            tui_panes::tab_bar(&state.tab_titles, state.tab_index),
            regions[0],
        );

        let tree = build_tree(state);
        let resolved = tree.resolve(regions[1]);
        // Cached so `handle_key`'s directional focus movement reads the SAME
        // rects this frame just drew, instead of re-reading the terminal
        // size and re-resolving the tree outside a draw pass.
        state.last_pane_layout = resolved.clone();
        for id in tree.leaf_ids() {
            let Some(rect) = resolved.rect(id) else {
                continue;
            };
            let focused = state.focus.is_focused(id);
            let title = tree.title(id).unwrap_or(id).to_string();
            let inner = tui_panes::render_pane(frame, rect, &title, focused);
            if id == PANE_ROWS {
                render_rows_pane(frame, inner, state);
            } else if id == PANE_DETAIL {
                render_detail_pane(frame, inner, state);
            } else if id == PANE_NOTES {
                render_notes_pane(frame, inner, state);
            }
        }

        frame.render_widget(footer(state), regions[2]);

        if let Some(modal) = state.modal_host.modal() {
            tui_kit::render_modal(frame, area, modal);
        }
    })
}

fn render_rows_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &DemoState) {
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.rows,
        &state.list,
        |row| row.name.clone(),
        |row| state.marks.contains(&row.id),
    );
}

/// The detail pane's full content, uncapped — the pane's own
/// [`super::tui_panes::PaneScrolls`] entry windows it down to `rect.height`
/// rows at render time, and [`handle_key`] uses this same builder to know
/// the scroll length a `PgUp`/`PgDn`/arrow key clamps against.
fn detail_lines(row: &DemoRow) -> Vec<RLine<'static>> {
    let mut lines = vec![
        RLine::from(Span::styled(
            row.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        RLine::from(format!("id: {}", row.id)),
        RLine::default(),
    ];
    lines.extend(
        (0..20).map(|field| RLine::from(format!("field {field}: value-{}-{field}", row.id))),
    );
    lines
}

fn render_detail_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &DemoState) {
    let Some(row) = state.selected_row() else {
        frame.render_widget(Paragraph::new("(no rows)"), inner);
        return;
    };
    let lines = detail_lines(row);
    let scroll = state.pane_scrolls.get(PANE_DETAIL);
    tui_panes::render_lines_pane(frame, inner, &lines, scroll);
}

fn render_notes_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &DemoState) {
    let Some(row) = state.selected_row() else {
        frame.render_widget(Paragraph::new("(no rows)"), inner);
        return;
    };
    let lines: Vec<RLine<'static>> = row
        .notes
        .split('\n')
        .map(|line| RLine::from(line.to_string()))
        .collect();
    let scroll = state.pane_scrolls.get(PANE_NOTES);
    tui_panes::render_lines_pane(frame, inner, &lines, scroll);
}

fn footer(state: &DemoState) -> Paragraph<'static> {
    let hint = format!(
        "tab/shift-tab focus  alt+arrows move  [/] cycle tabs  space mark  C clear marks  d delete  r rename  n notes  ctrl-c/q exit  [{} marked {}]",
        state.list.position_text(),
        state.marks.len()
    );
    tui_kit::keymap_footer(hint, state.message.as_deref())
}
