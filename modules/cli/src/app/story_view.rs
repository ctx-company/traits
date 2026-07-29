//! P550: the run-story pane opened at run termination (`--story`) and from
//! the dashboard's `S` key on a Sessions row. Smallest possible surface on
//! the existing P505 pane kit: a single bordered alt-screen lines pane over
//! the shared [`super::story::story_document`] — the alt screen is correct
//! here (not inline): the run's scrollback record was already committed by
//! the inline live pane and the plain final output, so the story is a
//! document you enter and leave, not a continuation of that scrollback.
//! Read-only: `q`/`Esc`/ctrl-c all close immediately, with no confirm modal
//! and no mutation of any ledger.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::text::Line as RLine;

use ctx_traits_core::procedure::session::Session;
use ctx_traits_core::procedure::story::{StoryLevel, StoryReport};

use super::story::story_document;
use super::tui_kit::{self, ViewportScroll};
use super::tui_panes;
use super::tui_ratatui::{RatatuiPane, render_line};

fn io_err(source: std::io::Error) -> crate::Error {
    ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
        path: "<tty>".to_string(),
        source,
    })
    .into()
}

/// Wraps the shared `story_document` to `width` display columns so the
/// pane's scrollbar position matches what actually renders — the same
/// physical-row discipline `tui_panes::wrapped_lines`'s own doc calls out
/// for story panes specifically.
fn wrapped_document(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
    width: u16,
) -> Vec<RLine<'static>> {
    let styled = story_document(session, report, level);
    let rendered: Vec<RLine<'static>> = styled.iter().map(render_line).collect();
    tui_panes::wrapped_lines(&rendered, width)
}

/// Opens a fresh alt-screen pane and drives it until the user closes it.
/// Entry point for the `--story` run-termination hook (`run.rs`); the
/// dashboard's `S` key instead renders the same document into its own
/// already-open pane (`dashboard.rs`), reusing [`wrapped_document`] rather
/// than opening a second `RatatuiPane` over the dashboard's own.
pub(crate) fn run(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
    title: &str,
) -> crate::Result<()> {
    let mut pane = RatatuiPane::new_forwarding_ctrl_c().map_err(io_err)?;
    let mut scroll = ViewportScroll::new();
    while !pane.detached() {
        let mut rendered_len = 0usize;
        pane.draw(|frame| {
            let area = frame.area();
            let inner = tui_panes::render_pane(frame, area, title, true);
            let lines = wrapped_document(session, report, level, inner.width.max(1));
            rendered_len = lines.len();
            tui_panes::render_lines_pane(frame, inner, &lines, scroll);
        })
        .map_err(io_err)?;
        scroll.set_len(rendered_len);

        let Some(key) = pane
            .poll_key(std::time::Duration::from_millis(250))
            .map_err(io_err)?
        else {
            continue;
        };
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            _ => {
                if let Some(delta) = tui_kit::scroll_key(&key) {
                    scroll.apply(delta, usize::MAX);
                }
            }
        }
    }
    Ok(())
}
