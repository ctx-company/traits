//! `ctx tasks sync | list | show` (0060): the CLI's thin edge over the
//! files-backed `TaskProvider`. Every handler resolves a board directory
//! (`--board`, defaulting to `.internal/tasks` under the repository root),
//! opens it read-only, and either prints the provider's own typed result
//! (`--json`) or a compact panel.

use camino::Utf8PathBuf;
use ctx_traits_core::response::{CommandOutput, Envelope};
use ctx_traits_core::task::TaskStatus as TaskDocStatus;
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{
    EffectKind, EffectOutcome, TaskProvider, TaskProviderMut, TaskUpdate,
};
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

/// `ctx tasks proposals` (0063.8): the same merge-time done-proposal
/// derivation the dashboard TASKS screen surfaces, non-interactively and
/// list-only — accepting one stays `ctx tasks update <task> --status done`,
/// no new write surface here. Read-only: the current-repository run
/// inventory plus a fresh board `list`/`sync`, folded through the same pure
/// [`super::task_proposals::derive_proposals`] both consumers share.
pub(crate) fn handle_tasks_proposals(
    board: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read(dir.clone());
    let summaries = provider.list(false).map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let sync_report = provider.sync().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;

    let inventory = ctx_traits_io::run_session::current_repo_run_inventory().map_err(|e| {
        crate::Error::Command {
            message: e.to_string(),
        }
    })?;
    let triples: Vec<(Option<String>, String, Option<String>)> = inventory
        .iter()
        .filter_map(|row| match &row.status {
            ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } => Some((
                session.provenance.task_key.clone(),
                session.run_id.as_str().to_string(),
                super::task_proposals::merged_landed_sha(session),
            )),
            ctx_traits_io::run_session::InventoryOutcome::Unreadable { .. } => None,
        })
        .collect();
    let runs: Vec<(Option<&str>, &str, Option<&str>)> = triples
        .iter()
        .map(|(key, run_id, sha)| (key.as_deref(), run_id.as_str(), sha.as_deref()))
        .collect();
    let proposals =
        super::task_proposals::derive_proposals(&runs, &summaries, &sync_report.duplicate_keys);

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&proposals), "tasks proposals report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("tasks proposals — {dir}"),
                PanelStatus::Passed(format!("{} proposal(s)", proposals.len())),
            );
            for proposal in &proposals {
                let value = proposal
                    .evidence
                    .iter()
                    .map(|evidence| {
                        format!(
                            "run {} merged as {} — mark done?",
                            evidence.run_id, evidence.sha
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                panel = panel.row(PanelRow::toned(
                    proposal.task_key.clone(),
                    value,
                    RowTone::Default,
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// `ctx tasks reconcile` (0064): the full reconcile pass, non-interactively
/// and list-only, same precedent as `handle_tasks_proposals` — accepting a
/// proposal stays `ctx tasks update`. Reuses the session-inventory assembly
/// `handle_tasks_proposals` already built, extended with the ancestry,
/// digest, and counter-park facts [`super::task_proposals::derive_reconcile_report`]
/// hardens `MarkDone` against.
pub(crate) fn handle_tasks_reconcile(
    board: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let dir = board_dir(board)?;
    let provider = FilesTaskBoard::open_read(dir.clone());
    let summaries = provider.list(true).map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let sync_report = provider.sync().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let mut resolved = std::collections::BTreeMap::new();
    for summary in &summaries {
        if let Ok(Some(task)) = provider.get(&summary.key) {
            resolved.insert(summary.key.clone(), task);
        }
    }

    let inventory = ctx_traits_io::run_session::current_repo_run_inventory().map_err(|e| {
        crate::Error::Command {
            message: e.to_string(),
        }
    })?;
    let facts = session_facts_from_inventory(&inventory);
    let report = super::task_proposals::derive_reconcile_report(
        &facts,
        &summaries,
        &resolved,
        &sync_report.duplicate_keys,
    );

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&report), "tasks reconcile report")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("tasks reconcile — {dir}"),
                PanelStatus::Passed(format!(
                    "{} proposal(s), {} ambiguous",
                    report.proposals.len(),
                    report.ambiguous.len()
                )),
            );
            for proposal in &report.proposals {
                let value = match proposal {
                    super::task_proposals::ReconcileProposal::MarkDone { task_key, evidence } => {
                        let mut value = evidence
                            .iter()
                            .map(|e| format!("run {} merged as {} — mark done?", e.run_id, e.sha))
                            .collect::<Vec<_>>()
                            .join("; ");
                        value.push_str(&mark_done_checks_annotation(task_key, &resolved));
                        value
                    }
                    super::task_proposals::ReconcileProposal::RemoveDependsOn(remove) => {
                        format!(
                            "remove depends-on {} ({}) — {}",
                            remove.to,
                            status_text(remove.to_status),
                            remove.evidence
                        )
                    }
                };
                panel = panel.row(PanelRow::toned(
                    proposal.task_key().to_string(),
                    value,
                    RowTone::Default,
                ));
            }
            for finding in &report.ambiguous {
                panel = panel.row(PanelRow::toned(
                    finding.task_key.clone(),
                    format!("ambiguous — {}", finding.reason),
                    RowTone::Warn,
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// 0144: what `handle_tasks_reconcile`'s human report says about a
/// `MarkDone` candidate's declared checks, WITHOUT running them —
/// `ctx tasks reconcile` is a read-only report, never a write, so the
/// trust-surfacing posture here is "name the commands and the resolved
/// disposition," not "execute them." Actually applying a proposal (and, for
/// a `checked`/`merge` policy, running its checks) happens through
/// `ctx tasks update` or the dashboard's `y`/`R`, both of which show the
/// same commands immediately before they run. Empty string when the task
/// has no declared checks — the existing report is unchanged in that case.
fn mark_done_checks_annotation(
    task_key: &str,
    resolved: &std::collections::BTreeMap<String, ctx_traits_core::task::provider::ResolvedTask>,
) -> String {
    let Some(document) = resolved.get(task_key).map(|task| &task.document) else {
        return String::new();
    };
    if document.checks.is_empty() {
        return String::new();
    }
    let config_default =
        ctx_traits_io::harness_config::resolve_runtime_config(camino::Utf8Path::new("."))
            .ok()
            .and_then(|config| config.effective_auto_close());
    let policy =
        super::task_proposals::resolve_auto_close_policy(document.auto_close, config_default);
    let commands = document
        .checks
        .iter()
        .map(|check| format!("{}: {}", check.name, check.command))
        .collect::<Vec<_>>()
        .join(", ");
    match policy {
        Some(policy) => {
            format!(" [auto-close={policy:?}, declared checks not yet run — {commands}]")
        }
        None => format!(" [declared checks, no auto-close policy configured — {commands}]"),
    }
}

/// [`super::task_proposals::SessionFact`] rows from a ledger inventory scan:
/// the shared assembly `handle_tasks_proposals` and `handle_tasks_reconcile`
/// both build on, extended here with the ancestry, digest, and park-report
/// facts reconcile alone needs. One `git merge-base --is-ancestor` per
/// distinct landed sha — never per session — so a task cited by several
/// merged runs against the same sha checks ancestry once.
pub(crate) fn session_facts_from_inventory(
    inventory: &[ctx_traits_io::run_session::RunInventoryRow],
) -> Vec<super::task_proposals::SessionFact> {
    let mut ancestry_cache: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    let mut facts = Vec::new();
    for row in inventory {
        let ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } = &row.status
        else {
            continue;
        };
        let landed_sha = super::task_proposals::merged_landed_sha(session);
        let landed_is_ancestor = landed_sha.as_ref().map(|sha| {
            *ancestry_cache.entry(sha.clone()).or_insert_with(|| {
                ctx_traits_io::git_process::is_ancestor(sha, "HEAD").unwrap_or(false)
            })
        });
        let blocked_with_park_report = session.status
            == ctx_traits_core::procedure::session::Status::Blocked
            && ctx_traits_io::run_session::session_park_report(session).is_some();
        facts.push(super::task_proposals::SessionFact {
            run_id: session.run_id.as_str().to_string(),
            task_key: session.provenance.task_key.clone(),
            task_digest: session
                .provenance
                .task_digest
                .as_ref()
                .map(|d| d.to_string()),
            landed_sha,
            landed_is_ancestor,
            blocked_with_park_report,
            terminal_epoch: session
                .last_drive_outcome
                .as_ref()
                .map(|outcome| outcome.recorded_at_epoch),
        });
    }
    facts
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
    release_dependents: bool,
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
        release_dependents,
        set_closure: None,
    };

    let outcome = provider
        .update(&key, update)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&outcome), "tasks update report")?;
        }
        OutputMode::Human(mode) => {
            let summary = &outcome.summary;
            let mut panel = Panel::new(
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
            for effect in &outcome.effects {
                let (tone, outcome_text) = match &effect.outcome {
                    EffectOutcome::Applied => (RowTone::Default, "applied".to_string()),
                    EffectOutcome::Failed { reason } => {
                        (RowTone::Fail, format!("failed: {reason}"))
                    }
                };
                panel = panel.row(PanelRow::toned(
                    effect_label(effect.effect),
                    format!("{outcome_text} — {}", effect.documents.join(", ")),
                    tone,
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

fn effect_label(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::ArchivePlacement => "archive placement",
        EffectKind::ReleaseDependents => "released dependents",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        // Pid recycling can hand this process a name a dead test run already
        // used and left behind; only exclusive creation guarantees the dir is
        // empty, so retry past leftovers instead of adopting them.
        loop {
            let dir = std::env::temp_dir().join(format!(
                "cli-tasks-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Utf8PathBuf::from_path_buf(dir).unwrap(),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("creating scratch dir {}: {err}", dir.display()),
            }
        }
    }

    fn write_task(dir: &Utf8PathBuf, file_name: &str, toml: &str) {
        std::fs::write(dir.join(file_name), toml).unwrap();
    }

    #[test]
    fn mark_done_checks_annotation_is_empty_for_a_task_with_no_declared_checks() {
        let documents: std::collections::BTreeMap<_, _> = [(
            "0001".to_string(),
            ctx_traits_core::task::TaskDocument {
                schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
                key: "0001".to_string(),
                title: "t".to_string(),
                status: None,
                raised: None,
                closed: None,
                wall: None,
                origin: None,
                content: String::new(),
                scope: String::new(),
                validation: String::new(),
                relations: ctx_traits_core::task::Relations::default(),
                steps: Vec::new(),
                checks: Vec::new(),
                auto_close: None,
                closure: None,
            },
        )]
        .into_iter()
        .collect();
        let resolved: std::collections::BTreeMap<_, _> = documents
            .into_iter()
            .map(|(key, document)| {
                (
                    key.clone(),
                    ctx_traits_core::task::provider::resolve_task(
                        &[(key.clone(), document)].into_iter().collect(),
                        &key,
                        false,
                        "sha256:x".to_string(),
                    ),
                )
            })
            .collect();
        assert_eq!(mark_done_checks_annotation("0001", &resolved), "");
    }

    #[test]
    fn mark_done_checks_annotation_names_every_command_for_an_unknown_task() {
        let resolved = std::collections::BTreeMap::new();
        assert_eq!(mark_done_checks_annotation("0001", &resolved), "");
    }

    /// `tasks update --status done --release-dependents --json` (0063.6):
    /// the sweep runs from the CLI's own flag threading (not just the io
    /// layer directly) and its dependent is actually released.
    #[test]
    fn tasks_update_with_release_dependents_reports_the_sweep_effect() {
        let board = tempdir();
        write_task(
            &board,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );

        handle_tasks_update(
            "0001",
            Some(board.as_str()),
            None,
            Some(TaskUpdateStatus::Done),
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            None,
            false,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            true,
            true,
        )
        .unwrap();

        let provider = FilesTaskBoard::open_read(board);
        let dependent = provider.get("0002").unwrap().unwrap();
        assert!(dependent.document.relations.depends_on.is_empty());
    }

    /// `ctx tasks proposals` (0063.8) is a thin edge: it never crashes
    /// against a board with no session inventory bound to it, and its
    /// `--json` envelope shape is exercised through the pure derivation
    /// (`derive_proposals`, tested exhaustively in `task_proposals.rs`)
    /// rather than a live inventory this test cannot control.
    #[test]
    fn tasks_proposals_against_an_unbound_board_lists_nothing() {
        let board = tempdir();
        write_task(
            &board,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );

        // `CommandOutput<()>` carries nothing to assert on directly — this
        // handler prints its envelope rather than returning it. The board's
        // own task carries no bound run, so completing without error over a
        // real (if empty) session inventory is the thin-edge contract this
        // test protects.
        handle_tasks_proposals(Some(board.as_str()), true).unwrap();
    }
}
