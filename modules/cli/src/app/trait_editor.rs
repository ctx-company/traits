//! P244 §4: `ctx traits internal edit <trait>` — a modal alt-screen master-detail
//! screen for one trait package, built entirely on the landed P468 kit
//! (`tui_kit`) and the alt-screen `RatatuiPane`. Left: the package's own
//! canonical sections (metadata / intent / behavior / ports / slots /
//! procedure / resources). Right: a read-only rendered detail of whichever
//! section is selected. Every textual action suspends into `$EDITOR` and
//! reloads on exit (P423 doctrine) — there is deliberately no in-TUI text
//! buffer and no direct canonical-document write path; `e`/`b`/`c` route
//! through the authored source, `ctx traits build`, and `ctx traits check`
//! respectively, exactly like the dashboard's TRAITS screen. Gated on
//! [`super::dashboard::interactive_available`]; off a TTY it refuses with a
//! plain one-line message and a nonzero exit.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as RLine;
use ratatui::widgets::Paragraph;

use super::lifecycle_reporting::{dashboard_trait_drift, dashboard_trait_editable_source};
use super::tui_kit::{self, Focus, Modal, ModalHost, ModalOutcome, ScrollList, ViewportScroll};
use super::tui_ratatui::{RatatuiPane, render_line};

const LEFT_MIN: u16 = 28;
const RIGHT_MIN: u16 = 40;
const RIGHT_MAX: u16 = 200;
const SEPARATOR_WIDTH: u16 = 1;

/// One left-column entry: a section label plus its read-only rendered
/// detail. Built once at load (and after every reload — `e`/`b`), never
/// recomputed per render tick.
struct Section {
    label: &'static str,
    lines: Vec<RLine<'static>>,
}

#[derive(Clone)]
enum ActionTag {
    Exit,
}

struct EditorState {
    /// The path passed to `ctx traits internal edit` / resolved from the trait id —
    /// the SAME canonical-document path `dashboard_trait_drift` and
    /// `dashboard_trait_editable_source` key off, so the editor and the
    /// dashboard's TRAITS screen can never resolve a different file for the
    /// same trait (draft §4.3).
    canonical_path: String,
    trait_id: String,
    status: String,
    trust: String,
    drift: String,
    sections: Vec<Section>,
    list: ScrollList,
    /// The section list's last-rendered row budget, recorded by `draw` so
    /// key-time `ScrollList::apply` clamps against the real window instead
    /// of `usize::MAX` (which pins the selection to the bottom row once the
    /// list outgrows the rect). `0` until the first draw — `clamp` treats
    /// that like the length-only clamp.
    list_rows: usize,
    detail_scroll: ViewportScroll,
    focus: Focus,
    modal_host: ModalHost<ActionTag>,
    message: Option<String>,
    quit: bool,
}

impl EditorState {
    fn load(canonical_path: &str) -> crate::Result<Self> {
        let (trait_ref, trait_root, _source_digest, canonical_digest) =
            ctx_traits_io::run::load_trait(canonical_path)?;
        let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
            &trait_root,
            trait_ref.id.as_str(),
            canonical_digest.as_str(),
        )?;
        let drift = dashboard_trait_drift(canonical_path);
        let sections = trait_sections(&trait_ref);
        let mut list = ScrollList::new();
        list.set_len(sections.len());
        Ok(Self {
            canonical_path: canonical_path.to_string(),
            trait_id: trait_ref.id.as_str().to_string(),
            status: status.display_name().to_string(),
            trust: trust.display_name().to_string(),
            drift,
            sections,
            list,
            list_rows: 0,
            detail_scroll: ViewportScroll::new(),
            focus: Focus::Master,
            modal_host: ModalHost::new(),
            message: None,
            quit: false,
        })
    }

    /// Re-reads the trait document and rebuilds every section, preserving
    /// the current section selection where it still exists — called after
    /// `e` (editor round-trip) and `b` (rebuild) so the detail pane reflects
    /// what is actually on disk rather than a stale in-memory copy.
    fn reload(&mut self) -> crate::Result<()> {
        let selected_label = self.sections.get(self.list.selected()).map(|s| s.label);
        let reloaded = Self::load(&self.canonical_path)?;
        let selected_index = selected_label
            .and_then(|label| reloaded.sections.iter().position(|s| s.label == label))
            .unwrap_or(0);
        self.trait_id = reloaded.trait_id;
        self.status = reloaded.status;
        self.trust = reloaded.trust;
        self.drift = reloaded.drift;
        self.sections = reloaded.sections;
        self.list.set_len(self.sections.len());
        self.list.set_selected(selected_index);
        Ok(())
    }

    fn selected_section(&self) -> Option<&Section> {
        self.sections.get(self.list.selected())
    }
}

/// Renders one section of the parsed canonical document as pretty-printed
/// JSON — the same canonical data `ctx traits check`/`inspect` operate on,
/// never a second hand-authored summary of it. Read-only: this is display
/// only, never a write path (draft §4.2).
fn section_lines<T: serde::Serialize>(value: &T) -> Vec<RLine<'static>> {
    match serde_json::to_string_pretty(value) {
        Ok(text) => text
            .lines()
            .map(|line| RLine::from(line.to_string()))
            .collect(),
        Err(err) => vec![RLine::from(format!("(unable to render: {err})"))],
    }
}

fn trait_sections(trait_ref: &ctx_traits_core::Trait) -> Vec<Section> {
    vec![
        Section {
            label: "metadata",
            lines: section_lines(&trait_ref.metadata),
        },
        Section {
            label: "intent",
            lines: section_lines(&trait_ref.intent),
        },
        Section {
            label: "behavior",
            lines: section_lines(&trait_ref.behavior),
        },
        Section {
            label: "ports",
            lines: section_lines(&trait_ref.ports),
        },
        Section {
            label: "slots",
            lines: section_lines(&trait_ref.slots),
        },
        Section {
            label: "procedure",
            lines: section_lines(&trait_ref.procedure),
        },
        Section {
            label: "resources",
            lines: section_lines(&trait_ref.resources),
        },
    ]
}

/// Entry point: `ctx traits internal edit <trait>`. Refuses with a plain-text message
/// and a nonzero exit off a TTY, mirroring `tui_demo::run`.
pub(crate) fn run(trait_arg: &str) -> crate::Result<()> {
    if !super::dashboard::interactive_available() {
        return Err(crate::Error::Command {
            message: "ctx traits internal edit requires an interactive TTY".to_string(),
        });
    }
    let canonical_path =
        super::command_handlers::resolve_trait_target(Some(trait_arg), None, "edit")?;
    let mut state = EditorState::load(&canonical_path)?;
    let mut pane = RatatuiPane::new_forwarding_ctrl_c().map_err(io_err)?;
    while !state.quit && !pane.detached() {
        draw(&mut pane, &mut state).map_err(io_err)?;
        let key = pane
            .poll_key(std::time::Duration::from_millis(250))
            .map_err(io_err)?;
        let Some(key) = key else { continue };
        handle_key(&mut pane, &mut state, key)?;
    }
    Ok(())
}

fn io_err(source: std::io::Error) -> crate::Error {
    ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
        path: "<tty>".to_string(),
        source,
    })
    .into()
}

fn handle_key(pane: &mut RatatuiPane, state: &mut EditorState, key: KeyEvent) -> crate::Result<()> {
    if let Some((tag, outcome)) = state.modal_host.handle_key(&key) {
        return apply_modal_outcome(state, tag, outcome);
    }
    if state.modal_host.is_open() {
        return Ok(());
    }

    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.modal_host.open(
            ActionTag::Exit,
            Modal::confirm("exit editor", "Quit ctx traits internal edit?"),
        );
        return Ok(());
    }
    match key.code {
        KeyCode::Char('q') => {
            state.modal_host.open(
                ActionTag::Exit,
                Modal::confirm("exit editor", "Quit ctx traits internal edit?"),
            );
        }
        KeyCode::Enter | KeyCode::Esc => {
            let moved = state.focus.handle_key(&key);
            if moved && state.focus == Focus::Detail {
                state.detail_scroll = ViewportScroll::new();
            }
        }
        KeyCode::Char('e') => edit_source(pane, state)?,
        KeyCode::Char('b') => rebuild_from_source(state)?,
        KeyCode::Char('c') => run_check(state),
        _ if state.focus == Focus::Detail => {
            if let Some(delta) = tui_kit::scroll_key(&key) {
                let len = state
                    .selected_section()
                    .map(|section| section.lines.len())
                    .unwrap_or(0);
                state.detail_scroll.set_len(len);
                state.detail_scroll.apply(delta, usize::MAX);
            }
        }
        _ if state.focus == Focus::Master => {
            if let Some(delta) = tui_kit::scroll_key(&key) {
                let rows = if state.list_rows == 0 {
                    usize::MAX
                } else {
                    state.list_rows
                };
                state.list.apply(delta, rows);
                state.detail_scroll = ViewportScroll::new();
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_modal_outcome(
    state: &mut EditorState,
    tag: ActionTag,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    match (tag, outcome) {
        (_, ModalOutcome::Cancelled) => {
            state.message = Some("cancelled".to_string());
        }
        (ActionTag::Exit, ModalOutcome::Confirmed) => {
            state.quit = true;
        }
        _ => {}
    }
    Ok(())
}

/// `e`: suspends the pane and opens the resolved authored source (CDK
/// `index.ts`/`.mjs`, or the canonical file itself for a hand-authored
/// package — the SAME resolution `dashboard`'s `e` uses) in `$EDITOR`,
/// reloading every section from disk on return. No in-TUI text buffer, per
/// §4.2.
fn edit_source(pane: &mut RatatuiPane, state: &mut EditorState) -> crate::Result<()> {
    let Some(path) = dashboard_trait_editable_source(&state.canonical_path) else {
        state.message = Some("no editable authored source for this package".to_string());
        return Ok(());
    };
    let ok = tui_kit::edit_file(pane, &path)?;
    if ok {
        state.reload()?;
        state.message = Some(format!("edited {path}"));
    } else {
        state.message = Some("editor round-trip cancelled".to_string());
    }
    Ok(())
}

/// `b`: rebuilds from CDK authoring source via the exact same
/// `build_cdk_package` path `ctx traits build` drives, only when the
/// resolved editable source is a CDK source file (`.ts`/`.mjs`) rather than
/// the canonical document itself — a hand-authored package has no CDK
/// source to rebuild from.
fn rebuild_from_source(state: &mut EditorState) -> crate::Result<()> {
    let Some(source_path) = dashboard_trait_editable_source(&state.canonical_path) else {
        state.message = Some("no editable authored source for this package".to_string());
        return Ok(());
    };
    let is_cdk_source = matches!(source_path.extension(), Some("ts") | Some("mjs"));
    if !is_cdk_source {
        state.message = Some("not a CDK-authored package; nothing to rebuild".to_string());
        return Ok(());
    }
    let format = camino::Utf8Path::new(&state.canonical_path)
        .extension()
        .unwrap_or("toml");
    match super::schema_synth_build::build_cdk_package(source_path.as_str(), format, None) {
        Ok(evidence) => {
            state.message = Some(format!("rebuilt {}", evidence.target_path));
            state.reload()?;
        }
        Err(err) => {
            state.message = Some(format!("build failed: {err}"));
        }
    }
    Ok(())
}

/// `c`: runs `ctx traits check` against the same canonical path this editor
/// opened, and renders the report into the detail pane through the existing
/// `Line`/`Tone` → `render_line` bridge (`report_check::styled_check_lines`)
/// — never a second summary of the same [`ctx_traits_core::check::CheckReport`].
fn run_check(state: &mut EditorState) {
    let report = super::report_check::build_check_report(&super::report_check::CheckInputs {
        file: &state.canonical_path,
        locked: true,
        skip_cdk_drift: false,
        json: false,
        plain: true,
        no_animate: true,
        verbose: true,
        run_ledger: None,
        eval_reports: &[],
    });
    let lines = match report {
        Ok(report) => {
            let passed = report.passed;
            let styled =
                super::report_check::styled_check_lines(&report, true, &state.canonical_path, true);
            state.message = Some(if passed {
                "check passed".to_string()
            } else {
                "check failed — see the check section".to_string()
            });
            styled.into_iter().map(|line| render_line(&line)).collect()
        }
        Err(err) => {
            state.message = Some(format!("check error: {err}"));
            vec![RLine::from(format!("check error: {err}"))]
        }
    };
    let index = if let Some(index) = state
        .sections
        .iter()
        .position(|section| section.label == "check")
    {
        state.sections[index].lines = lines;
        index
    } else {
        state.sections.push(Section {
            label: "check",
            lines,
        });
        state.list.set_len(state.sections.len());
        state.sections.len() - 1
    };
    // Select the section the report just landed in — advisory fix: the
    // message said "see the check section" without ever putting it on
    // screen.
    state.list.set_selected(index);
    state.detail_scroll = ViewportScroll::new();
}

fn draw(pane: &mut RatatuiPane, state: &mut EditorState) -> std::io::Result<()> {
    pane.draw(|frame| {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(2),
            ])
            .split(area);
        frame.render_widget(
            tui_kit::title_bar(format!(
                "ctx traits internal edit — {} [{}/{}/{}]",
                state.trait_id, state.status, state.trust, state.drift
            )),
            layout[0],
        );

        let bounds = tui_kit::SplitBounds {
            left_min: LEFT_MIN,
            right_min: RIGHT_MIN,
            right_max: RIGHT_MAX,
            separator: SEPARATOR_WIDTH,
        };
        match bounds.compute(layout[1], LEFT_MIN) {
            tui_kit::SplitLayout::Full(rect) => {
                state.list_rows = rect.height as usize;
                render_sections(frame, rect, state);
            }
            tui_kit::SplitLayout::Split {
                left,
                separator,
                right,
            } => {
                state.list_rows = left.height as usize;
                render_sections(frame, left, state);
                tui_kit::render_separator_column(frame, separator);
                render_detail(frame, right, state);
            }
        }

        frame.render_widget(footer(state), layout[2]);

        if let Some(modal) = state.modal_host.modal() {
            tui_kit::render_modal(frame, area, modal);
        }
    })
}

fn render_sections(
    frame: &mut ratatui::Frame<'_>,
    rect: ratatui::layout::Rect,
    state: &EditorState,
) {
    let selected = state.list.selected();
    let window = state.list.window(rect.height as usize);
    let items: Vec<ratatui::widgets::ListItem<'static>> = state
        .sections
        .iter()
        .enumerate()
        .skip(window.start)
        .take(window.len())
        .map(|(idx, section)| {
            let item = ratatui::widgets::ListItem::new(section.label);
            if idx == selected && state.focus == Focus::Master {
                item.style(Style::default().add_modifier(Modifier::BOLD))
            } else {
                item
            }
        })
        .collect();
    frame.render_widget(
        ratatui::widgets::List::new(items)
            .block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::NONE)),
        rect,
    );
}

fn render_detail(frame: &mut ratatui::Frame<'_>, rect: ratatui::layout::Rect, state: &EditorState) {
    let style = if state.focus == Focus::Detail {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let Some(section) = state.selected_section() else {
        frame.render_widget(Paragraph::new("(no sections)").style(style), rect);
        return;
    };
    let mut scroll = state.detail_scroll;
    scroll.set_len(section.lines.len());
    let window = scroll.window(rect.height as usize);
    let visible: Vec<RLine<'static>> = section.lines[window].to_vec();
    frame.render_widget(Paragraph::new(visible).style(style), rect);
}

fn footer(state: &EditorState) -> Paragraph<'static> {
    let hint = format!(
        "enter focus  esc back  e $EDITOR  b rebuild  c check  ctrl-c/q exit  [{}]",
        state.list.position_text()
    );
    tui_kit::keymap_footer(hint, state.message.as_deref())
}
