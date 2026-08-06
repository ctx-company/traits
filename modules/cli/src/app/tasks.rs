//! `ctx tasks sync | list | show` (0060): the CLI's thin edge over the
//! files-backed `TaskProvider`. Every handler resolves a board directory
//! (`--board`, defaulting to `.internal/tasks` under the repository root),
//! opens it read-only, and either prints the provider's own typed result
//! (`--json`) or a compact panel.

use camino::Utf8PathBuf;
use ctx_traits_core::response::{CommandOutput, Envelope};
use ctx_traits_core::task::TaskStatus as TaskDocStatus;
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{TaskProvider, TaskProviderMut, TaskUpdate};
use ctx_traits_io::task_files::FilesTaskBoard;

use crate::app::command_handlers::{print_json_report, resolve_repo_root};
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::surface::cli::TaskUpdateStatus;

const DEFAULT_BOARD_DIR: &str = ".internal/tasks";

pub(crate) fn board_dir(board: Option<&str>) -> crate::Result<Utf8PathBuf> {
    match board {
        Some(path) => Ok(Utf8PathBuf::from(path)),
        None => Ok(resolve_repo_root(None)?.join(DEFAULT_BOARD_DIR)),
    }
}

pub(crate) fn status_text(derived: DerivedStatus) -> &'static str {
    match derived {
        DerivedStatus::Ready => "ready",
        DerivedStatus::Blocked => "blocked",
        DerivedStatus::Done => "done",
        DerivedStatus::Cancelled => "cancelled",
    }
}

fn status_tone(derived: DerivedStatus) -> RowTone {
    match derived {
        DerivedStatus::Done => RowTone::Pass,
        DerivedStatus::Blocked => RowTone::Warn,
        DerivedStatus::Cancelled => RowTone::Fail,
        DerivedStatus::Ready => RowTone::Default,
    }
}

pub(crate) fn handle_tasks_sync(
    board: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read(dir.clone());
    let report = provider.sync().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&report), "tasks sync report")?;
        }
        OutputMode::Human(mode) => {
            let clean = report.dangling_edges.is_empty()
                && report.parse_failures.is_empty()
                && report.duplicate_keys.is_empty();
            let status = if clean {
                PanelStatus::Passed("clean".to_string())
            } else {
                PanelStatus::Blocked("issues found".to_string())
            };
            let mut panel = Panel::new("ctx", format!("tasks sync — {dir}"), status)
                .row(PanelRow::toned(
                    "dangling edges",
                    report.dangling_edges.len().to_string(),
                    if report.dangling_edges.is_empty() {
                        RowTone::Default
                    } else {
                        RowTone::Fail
                    },
                ))
                .row(PanelRow::toned(
                    "parse failures",
                    report.parse_failures.len().to_string(),
                    if report.parse_failures.is_empty() {
                        RowTone::Default
                    } else {
                        RowTone::Fail
                    },
                ))
                .row(PanelRow::toned(
                    "duplicate keys",
                    report.duplicate_keys.len().to_string(),
                    if report.duplicate_keys.is_empty() {
                        RowTone::Default
                    } else {
                        RowTone::Fail
                    },
                ));
            for edge in &report.dangling_edges {
                panel = panel.row(PanelRow::toned(
                    edge.from.clone(),
                    format!("{} -> {} (missing)", edge.field, edge.to),
                    RowTone::Fail,
                ));
            }
            for failure in &report.parse_failures {
                panel = panel.row(PanelRow::toned(
                    "parse failure",
                    format!("{}: {}", failure.location, failure.reason),
                    RowTone::Fail,
                ));
            }
            for duplicate in &report.duplicate_keys {
                panel = panel.row(PanelRow::toned(
                    duplicate.key.clone(),
                    duplicate.locations.join(", "),
                    RowTone::Fail,
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_tasks_list(
    board: Option<&str>,
    archived: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read(dir.clone());
    let summaries = provider.list(archived).map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&summaries), "tasks list report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("tasks list — {dir}"),
                PanelStatus::Passed(format!("{} task(s)", summaries.len())),
            );
            for summary in &summaries {
                let mut value = format!(
                    "{} [{}]",
                    summary.title,
                    status_text(summary.derived_status)
                );
                if summary.archived {
                    value.push_str(" (archived)");
                }
                panel = panel.row(PanelRow::toned(
                    summary.key.clone(),
                    value,
                    status_tone(summary.derived_status),
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// `ctx tasks update <task>` (0063.5): reads the task to capture its
/// digest, builds a [`TaskUpdate`] from the flags named, and submits it
/// with `expected_digest` set — closing the read-modify-write window
/// inside this handler itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_tasks_update(
    task: &str,
    board: Option<&str>,
    title: Option<String>,
    status: Option<TaskUpdateStatus>,
    content: Option<String>,
    scope: Option<String>,
    validation: Option<String>,
    wall: Option<String>,
    clear_wall: bool,
    origin: Option<String>,
    clear_origin: bool,
    parent: Option<String>,
    clear_parent: bool,
    add_depends_on: Vec<String>,
    remove_depends_on: Vec<String>,
    step_done: Vec<String>,
    step_open: Vec<String>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read_write(dir.clone());
    let key = provider
        .resolve(task)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?
        .ok_or_else(|| crate::Error::Command {
            message: format!("no task matching {task:?} in board {dir}"),
        })?;
    let resolved = provider
        .get(&key)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?
        .ok_or_else(|| crate::Error::Command {
            message: format!("task {key} resolved but could not be read"),
        })?;

    let mut set_steps_done: Vec<(String, bool)> =
        step_done.into_iter().map(|id| (id, true)).collect();
    set_steps_done.extend(step_open.into_iter().map(|id| (id, false)));

    let update = TaskUpdate {
        title,
        status: status.map(|s| match s {
            TaskUpdateStatus::Ready => TaskDocStatus::Ready,
            TaskUpdateStatus::Done => TaskDocStatus::Done,
            TaskUpdateStatus::Cancelled => TaskDocStatus::Cancelled,
        }),
        content,
        scope,
        validation,
        add_depends_on,
        remove_depends_on,
        set_parent: if clear_parent {
            Some(None)
        } else {
            parent.map(Some)
        },
        set_wall: if clear_wall {
            Some(None)
        } else {
            wall.map(Some)
        },
        set_origin: if clear_origin {
            Some(None)
        } else {
            origin.map(Some)
        },
        set_steps_done,
        expected_digest: Some(resolved.digest),
    };

    let summary = provider
        .update(&key, update)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&summary), "tasks update report")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new(
                "ctx",
                format!("tasks update — {}", summary.key),
                PanelStatus::Passed(status_text(summary.derived_status).to_string()),
            )
            .row(PanelRow::toned("title", &summary.title, RowTone::Default))
            .row(PanelRow::toned(
                "status",
                status_text(summary.derived_status),
                status_tone(summary.derived_status),
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_tasks_show(
    task: &str,
    board: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read(dir.clone());
    let key = provider
        .resolve(task)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?
        .ok_or_else(|| crate::Error::Command {
            message: format!("no task matching {task:?} in board {dir}"),
        })?;
    let resolved = provider
        .get(&key)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?
        .ok_or_else(|| crate::Error::Command {
            message: format!("task {key} resolved but could not be read"),
        })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&resolved), "tasks show report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("tasks show — {}", resolved.document.key),
                PanelStatus::Passed(status_text(resolved.derived_status).to_string()),
            )
            .row(PanelRow::toned(
                "title",
                &resolved.document.title,
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "status",
                status_text(resolved.derived_status),
                status_tone(resolved.derived_status),
            ))
            .row(PanelRow::toned(
                "archived",
                resolved.archived.to_string(),
                RowTone::Default,
            ));
            if let Some(parent) = &resolved.relations.parent {
                panel = panel.row(PanelRow::toned(
                    "parent",
                    format!(
                        "{} ({}) [{}]",
                        parent.key,
                        parent.title,
                        status_text(parent.status)
                    ),
                    RowTone::Default,
                ));
            }
            for edge in &resolved.relations.depends_on {
                panel = panel.row(PanelRow::toned(
                    "depends on",
                    format!(
                        "{} ({}) [{}]",
                        edge.key,
                        edge.title,
                        status_text(edge.status)
                    ),
                    status_tone(edge.status),
                ));
            }
            for edge in &resolved.relations.blocks {
                panel = panel.row(PanelRow::toned(
                    "blocks",
                    format!(
                        "{} ({}) [{}]",
                        edge.key,
                        edge.title,
                        status_text(edge.status)
                    ),
                    RowTone::Default,
                ));
            }
            for edge in &resolved.relations.children {
                panel = panel.row(PanelRow::toned(
                    "child",
                    format!(
                        "{} ({}) [{}]",
                        edge.key,
                        edge.title,
                        status_text(edge.status)
                    ),
                    status_tone(edge.status),
                ));
            }
            for (label, text) in resolved.document.prose_sections() {
                if !text.is_empty() {
                    panel = panel.row(PanelRow::toned(label, text, RowTone::Default));
                }
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}
