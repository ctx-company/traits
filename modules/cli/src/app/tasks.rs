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

pub(crate) fn board_dir(board: Option<&str>) -> crate::Result<Utf8PathBuf> {
    match board {
        Some(path) => Ok(Utf8PathBuf::from(path)),
        None => Ok(ctx_traits_io::task_files::repo_board_dir(
            &resolve_repo_root(None)?,
        )),
    }
}

pub(crate) fn status_text(derived: DerivedStatus) -> &'static str {
    match derived {
        DerivedStatus::Draft => "draft",
        DerivedStatus::Ready => "ready",
        DerivedStatus::Blocked => "blocked",
        DerivedStatus::Done => "done",
        DerivedStatus::Cancelled => "cancelled",
    }
}

fn status_tone(derived: DerivedStatus) -> RowTone {
    match derived {
        DerivedStatus::Draft => RowTone::Default,
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
/// center snapshot plus a fresh board `list`/`sync`, folded through the same pure
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

    let repo_key = ctx_traits_io::state::current_repo_key().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let rows = ctx_traits_io::center::list(Some(&repo_key)).map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let proposals = proposals_from_center(&rows, &summaries, &sync_report.duplicate_keys);

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

    let repo_key = ctx_traits_io::state::current_repo_key().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let rows = ctx_traits_io::center::list(Some(&repo_key)).map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let facts = session_facts_from_center(&rows);
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

/// [`super::task_proposals::SessionFact`] rows from the center snapshot:
/// the shared assembly `handle_tasks_proposals` and `handle_tasks_reconcile`
/// both build on, extended here with the ancestry, digest, and park-report
/// facts reconcile alone needs. One `git merge-base --is-ancestor` per
/// distinct landed sha — never per session — so a task cited by several
/// merged runs against the same sha checks ancestry once.
pub(crate) fn session_facts_from_center(
    rows: &[ctx_traits_io::center::CenterPublicRow],
) -> Vec<super::task_proposals::SessionFact> {
    let mut ancestry_cache: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    let mut facts = Vec::new();
    for row in readable_center_rows(rows) {
        let summary = &row.summary;
        let landed_sha = super::task_proposals::merged_landed_sha_from_terminal_frame(
            summary.last_terminal_merge_frame.as_ref(),
        );
        let landed_is_ancestor = landed_sha.as_ref().map(|sha| {
            *ancestry_cache.entry(sha.clone()).or_insert_with(|| {
                ctx_traits_io::git_process::is_ancestor(sha, "HEAD").unwrap_or(false)
            })
        });
        facts.push(super::task_proposals::SessionFact {
            run_id: summary.run_id.clone(),
            task_key: summary.task_key.clone(),
            task_digest: summary.task_digest.clone(),
            landed_sha,
            landed_is_ancestor,
            blocked_with_park_report: summary.blocked_with_park_report,
            terminal_epoch: summary.terminal_epoch,
        });
    }
    facts
}

pub(crate) fn proposals_from_center(
    rows: &[ctx_traits_io::center::CenterPublicRow],
    summaries: &[ctx_traits_core::task::provider::TaskSummary],
    duplicate_keys: &[ctx_traits_core::task::provider::DuplicateKey],
) -> Vec<super::task_proposals::DoneProposal> {
    let triples: Vec<(Option<String>, String, Option<String>)> = readable_center_rows(rows)
        .into_iter()
        .map(|row| {
            (
                row.summary.task_key.clone(),
                row.summary.run_id.clone(),
                super::task_proposals::merged_landed_sha_from_terminal_frame(
                    row.summary.last_terminal_merge_frame.as_ref(),
                ),
            )
        })
        .collect();
    let runs: Vec<(Option<&str>, &str, Option<&str>)> = triples
        .iter()
        .map(|(key, run_id, sha)| (key.as_deref(), run_id.as_str(), sha.as_deref()))
        .collect();
    super::task_proposals::derive_proposals(&runs, summaries, duplicate_keys)
}

/// Preserve the inventory's observable newest-ledger-first order while sharing
/// the center's one request between proposals and reconcile. Parse failures are
/// represented by the center but never contain facts for either report.
fn readable_center_rows(
    rows: &[ctx_traits_io::center::CenterPublicRow],
) -> Vec<&ctx_traits_io::center::CenterPublicRow> {
    let mut readable: Vec<_> = rows
        .iter()
        .filter(|row| row.summary.parse_error.is_none())
        .collect();
    readable.sort_by(|left, right| {
        right
            .modified_epoch_secs
            .cmp(&left.modified_epoch_secs)
            .then_with(|| left.ledger_path.cmp(&right.ledger_path))
    });
    readable
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
            TaskUpdateStatus::Draft => TaskDocStatus::Draft,
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

fn stored_status_text(status: Option<TaskDocStatus>) -> &'static str {
    match status {
        Some(TaskDocStatus::Draft) => "draft",
        Some(TaskDocStatus::Ready) => "ready",
        Some(TaskDocStatus::Done) => "done",
        Some(TaskDocStatus::Cancelled) => "cancelled",
        None => "unset",
    }
}

fn show_claimed_task(session: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let repo_key = ctx_traits_io::state::current_repo_key().map_err(|e| crate::Error::Command {
        message: e.to_string(),
    })?;
    let claimed = ctx_traits_io::center::claimed_task(session, Some(&repo_key)).map_err(|e| {
        crate::Error::Command {
            message: e.to_string(),
        }
    })?;
    let (task, close_policy) = match claimed {
        ctx_traits_io::center::ClaimedTaskResult::Task(task, policy) => (*task, policy),
        ctx_traits_io::center::ClaimedTaskResult::Missing => {
            return Err(crate::Error::Command {
                message: format!("no run matching session {session:?}"),
            });
        }
        ctx_traits_io::center::ClaimedTaskResult::Ambiguous(ids) => {
            return Err(crate::Error::Command {
                message: format!("session {session:?} is ambiguous: {}", ids.join(", ")),
            });
        }
        ctx_traits_io::center::ClaimedTaskResult::Unclaimed => {
            return Err(crate::Error::Command {
                message: format!("run {session:?} has not claimed a task"),
            });
        }
    };
    let detail = ctx_traits_io::center::task_detail(&repo_key, &task.key).map_err(|e| {
        crate::Error::Command {
            message: e.to_string(),
        }
    })?;

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(
                &Envelope::ok(&serde_json::json!({
                    "task": task,
                    "close-policy": close_policy,
                    "detail": detail,
                })),
                "tasks show report",
            )?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                format!("tasks show — {}", task.key),
                PanelStatus::Passed(stored_status_text(task.stored_status).to_string()),
            )
            .row(PanelRow::toned("title", &task.title, RowTone::Default))
            .row(PanelRow::toned(
                "status",
                stored_status_text(task.stored_status),
                RowTone::Default,
            ));
            if let Some(auto_close) = task.auto_close {
                panel = panel.row(PanelRow::toned(
                    "auto-close",
                    format!("{auto_close:?}").to_lowercase(),
                    RowTone::Default,
                ));
            }
            panel = panel.row(PanelRow::toned(
                "close-policy",
                match &close_policy {
                    ctx_traits_io::center::ClosePolicyResolution::Effective(policy) => {
                        format!("{policy:?}").to_lowercase()
                    }
                    ctx_traits_io::center::ClosePolicyResolution::NoneConfigured => {
                        "none configured".to_string()
                    }
                    ctx_traits_io::center::ClosePolicyResolution::Unresolved(reason) => {
                        format!("unresolved: {reason}")
                    }
                },
                RowTone::Default,
            ));
            if !task.description.is_empty() {
                panel = panel.row(PanelRow::toned(
                    "description",
                    &task.description,
                    RowTone::Default,
                ));
            }
            match &detail {
                ctx_traits_io::center::TaskDetailWireResult::Missing => {
                    panel = panel.row(PanelRow::toned(
                        "detail",
                        "task unavailable: missing",
                        RowTone::Fail,
                    ));
                }
                ctx_traits_io::center::TaskDetailWireResult::Resolved { state, claim, .. } => {
                    panel = panel.row(PanelRow::toned("state", state, RowTone::Default));
                    match claim {
                        ctx_traits_io::center::TaskClaimWire::NoClaim => {
                            panel =
                                panel.row(PanelRow::toned("claimed", "no claim", RowTone::Default));
                        }
                        ctx_traits_io::center::TaskClaimWire::Ambiguous(ids) => {
                            panel = panel.row(PanelRow::toned(
                                "claimed",
                                format!("ambiguous: {}", ids.join(", ")),
                                RowTone::Fail,
                            ));
                        }
                        ctx_traits_io::center::TaskClaimWire::Claim {
                            run_id,
                            trait_id,
                            progress,
                        } => {
                            panel = panel.row(PanelRow::toned(
                                "claimed",
                                format!("{run_id} · {trait_id}"),
                                RowTone::Default,
                            ));
                            match progress {
                                Ok(ctx_traits_core::procedure::run::RunProgress::Reached {
                                    ordinal,
                                    total,
                                }) => {
                                    panel = panel.row(PanelRow::toned(
                                        "frame",
                                        format!("{ordinal} of {total}"),
                                        RowTone::Default,
                                    ));
                                }
                                Ok(ctx_traits_core::procedure::run::RunProgress::NoneReached {
                                    total,
                                }) => {
                                    panel = panel.row(PanelRow::toned(
                                        "frame",
                                        format!("none reached of {total}"),
                                        RowTone::Default,
                                    ));
                                }
                                Ok(
                                    ctx_traits_core::procedure::run::RunProgress::NoCountedFrames,
                                ) => {
                                    panel = panel.row(PanelRow::toned(
                                        "frame",
                                        "no counted frames",
                                        RowTone::Default,
                                    ));
                                }
                                Err(reason) => {
                                    panel = panel.row(PanelRow::toned(
                                        "frame",
                                        format!("unresolved: {reason}"),
                                        RowTone::Fail,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_tasks_show(
    task: Option<&str>,
    session: Option<&str>,
    board: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    if let Some(session) = session {
        return show_claimed_task(session, json);
    }
    let task = task.expect("clap enforces task or --session");
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
    use crate::app::test_support::{CenterPeer, read_center_request, write_center_response};

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
                Err(error)
                    if {
                        let error_kind = error.kind();
                        error_kind == std::io::ErrorKind::AlreadyExists
                    } =>
                {
                    continue;
                }
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

    #[test]
    fn center_task_facts_exclude_only_explicitly_unreadable_rows() {
        let readable: ctx_traits_io::center::CenterPublicRow =
            serde_json::from_value(serde_json::json!({
                "summary": {
                    "session_id": "readable-session",
                    "run_id": "readable-run",
                    "trait_id": "test",
                    "status": "completed",
                    "has_merge_frames": false,
                },
                "repo_key": "repo",
                "repo_path": "/repo",
                "ledger_path": "/runs/repo/readable.json",
                "live": false,
                "modified_epoch_secs": 0,
            }))
            .expect("readable center row");
        let corrupt: ctx_traits_io::center::CenterPublicRow =
            serde_json::from_value(serde_json::json!({
                "summary": {
                    "session_id": "corrupt-session",
                    "run_id": "corrupt-run",
                    "trait_id": "test",
                    "status": "completed",
                    "has_merge_frames": false,
                    "parse_error": "invalid JSON",
                },
                "repo_key": "repo",
                "repo_path": "/repo",
                "ledger_path": "/runs/repo/corrupt.json",
                "live": false,
                "modified_epoch_secs": 0,
            }))
            .expect("unreadable center row");

        let facts = session_facts_from_center(&[readable, corrupt]);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].run_id, "readable-run");
    }

    #[test]
    fn readable_center_rows_preserve_newest_ledger_first_order() {
        let older: ctx_traits_io::center::CenterPublicRow =
            serde_json::from_value(serde_json::json!({
                "summary": {
                    "session_id": "older-session",
                    "run_id": "older-run",
                    "trait_id": "test",
                    "status": "completed",
                    "has_merge_frames": false,
                },
                "repo_key": "repo",
                "repo_path": "/repo",
                "ledger_path": "/runs/repo/z-older.json",
                "live": false,
                "modified_epoch_secs": 10,
            }))
            .expect("older center row");
        let newer: ctx_traits_io::center::CenterPublicRow =
            serde_json::from_value(serde_json::json!({
                "summary": {
                    "session_id": "newer-session",
                    "run_id": "newer-run",
                    "trait_id": "test",
                    "status": "completed",
                    "has_merge_frames": false,
                },
                "repo_key": "repo",
                "repo_path": "/repo",
                "ledger_path": "/runs/repo/a-newer.json",
                "live": false,
                "modified_epoch_secs": 20,
            }))
            .expect("newer center row");

        let rows = [older, newer];
        let ordered = readable_center_rows(&rows);
        assert_eq!(ordered[0].summary.run_id, "newer-run");
        assert_eq!(ordered[1].summary.run_id, "older-run");
    }

    #[test]
    fn proposals_from_center_rows_match_the_shared_derivation() {
        let rows = readable_and_unreadable_center_rows();
        let board = task_board();
        let provider = FilesTaskBoard::open_read(board.clone());
        let summaries = provider.list(false).expect("list task board");
        let actual = proposals_from_center(&rows, &summaries, &[]);
        let expected = super::super::task_proposals::derive_proposals(
            &[(
                Some("0001"),
                "readable-run",
                Some("0123456789abcdef0123456789abcdef01234567"),
            )],
            &summaries,
            &[],
        );

        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 1);
        let _ = std::fs::remove_dir_all(board.as_std_path());
    }

    fn readable_and_unreadable_center_rows() -> Vec<ctx_traits_io::center::CenterPublicRow> {
        [
            serde_json::json!({
                "summary": {
                    "session_id": "readable-session", "run_id": "readable-run",
                    "trait_id": "test", "status": "completed", "task_key": "0001",
                    "has_merge_frames": true,
                    "last_terminal_merge_frame": {
                        "stage": "landing", "status": "merged",
                        "evidence": ["landed=0123456789abcdef0123456789abcdef01234567"]
                    }
                },
                "repo_key": "repo", "repo_path": "/repo", "ledger_path": "/runs/readable.json",
                "live": false, "modified_epoch_secs": 1
            }),
            serde_json::json!({
                "summary": {
                    "session_id": "unreadable-session", "run_id": "unreadable-run",
                    "trait_id": "test", "status": "completed", "task_key": "0001",
                    "has_merge_frames": true, "parse_error": "invalid JSON",
                    "last_terminal_merge_frame": {
                        "stage": "landing", "status": "merged",
                        "evidence": ["landed=0123456789abcdef0123456789abcdef01234567"]
                    }
                },
                "repo_key": "repo", "repo_path": "/repo", "ledger_path": "/runs/unreadable.json",
                "live": false, "modified_epoch_secs": 2
            }),
        ]
        .into_iter()
        .map(|value| serde_json::from_value(value).expect("center row"))
        .collect()
    }

    fn task_board() -> Utf8PathBuf {
        let board = tempdir();
        write_task(
            &board,
            "0001-task.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Task\"\nstatus = \"ready\"\n",
        );
        board
    }

    #[test]
    fn reconcile_report_from_center_rows_matches_the_shared_derivation() {
        let rows = readable_and_unreadable_center_rows();
        let board = task_board();
        let provider = FilesTaskBoard::open_read(board.clone());
        let summaries = provider.list(true).expect("list task board");
        let resolved = summaries
            .iter()
            .filter_map(|summary| {
                provider
                    .get(&summary.key)
                    .ok()
                    .flatten()
                    .map(|task| (summary.key.clone(), task))
            })
            .collect();
        let actual = super::super::task_proposals::derive_reconcile_report(
            &session_facts_from_center(&rows),
            &summaries,
            &resolved,
            &[],
        );
        let expected = super::super::task_proposals::derive_reconcile_report(
            &[super::super::task_proposals::SessionFact {
                run_id: "readable-run".to_string(),
                task_key: Some("0001".to_string()),
                task_digest: None,
                landed_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
                landed_is_ancestor: Some(false),
                blocked_with_park_report: false,
                terminal_epoch: None,
            }],
            &summaries,
            &resolved,
            &[],
        );

        assert_eq!(actual, expected);
        let _ = std::fs::remove_dir_all(board.as_std_path());
    }

    fn assert_center_request_failure_is_not_partial(
        handler: fn(Option<&str>, bool) -> crate::Result<CommandOutput<()>>,
    ) {
        let peer_server = CenterPeer::install("tasks-request-failure");
        let root = Utf8PathBuf::from_path_buf(peer_server.root().to_path_buf())
            .expect("UTF-8 center peer root");
        let board = root.join("board");
        std::fs::create_dir(&board).expect("create board");
        write_task(
            &board,
            "0001-task.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Task\"\nstatus = \"ready\"\n",
        );
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = crate::app::test_support::accept_center_client(&listener);
            let request = read_center_request(&stream);
            assert_eq!(request["kind"], "list");
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({"type": "error", "data": {"message": "test center failure"}}),
            );
        });
        let result = handler(Some(board.as_str()), true);
        peer.join().expect("join center peer");

        assert!(
            result
                .expect_err("center failure must not produce a partial report")
                .to_string()
                .contains("test center failure")
        );
    }

    #[test]
    fn proposals_fail_loudly_when_the_center_request_fails() {
        assert_center_request_failure_is_not_partial(handle_tasks_proposals);
    }

    #[test]
    fn reconcile_fails_loudly_when_the_center_request_fails() {
        assert_center_request_failure_is_not_partial(handle_tasks_reconcile);
    }

    #[test]
    fn session_show_issues_exactly_one_claimed_task_request_and_renders_it() {
        let peer_server = CenterPeer::install("tasks-show-session-happy");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = crate::app::test_support::accept_center_client(&listener);
            let request = read_center_request(&stream);
            assert_eq!(request["kind"], "claimed-task");
            assert_eq!(request["session_id"], "the-session");
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({
                    "type": "claimed-task",
                    "data": {
                        "type": "task",
                        "data": [
                            {
                                "key": "0001",
                                "title": "Fixture task",
                                "description": "the description",
                                "stored-status": "ready",
                                "auto-close": "checked"
                            },
                            {
                                "type": "effective",
                                "data": "checked"
                            }
                        ]
                    }
                }),
            );
            let mut detail_stream = crate::app::test_support::accept_center_client(&listener);
            let detail_request = read_center_request(&detail_stream);
            assert_eq!(detail_request["kind"], "task-detail");
            write_center_response(&mut detail_stream, &detail_request, fixture_task_detail());
        });
        let result = handle_tasks_show(None, Some("the-session"), None, true);
        peer.join().expect("join center peer");
        result.expect("session-addressed show succeeds");
    }

    #[test]
    fn session_show_center_error_is_a_loud_failure() {
        let peer_server = CenterPeer::install("tasks-show-session-error");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = crate::app::test_support::accept_center_client(&listener);
            let request = read_center_request(&stream);
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({"type": "error", "data": {"message": "test center failure"}}),
            );
        });
        let result = handle_tasks_show(None, Some("the-session"), None, true);
        peer.join().expect("join center peer");
        assert!(
            result
                .expect_err("center failure must not produce a partial panel")
                .to_string()
                .contains("test center failure")
        );
    }

    #[test]
    fn session_show_missing_unclaimed_and_ambiguous_are_loud_command_errors() {
        for (data, expected) in [
            (serde_json::json!({"type": "missing"}), "the-session"),
            (serde_json::json!({"type": "unclaimed"}), "the-session"),
            (
                serde_json::json!({"type": "ambiguous", "data": ["a", "b"]}),
                "the-session",
            ),
        ] {
            let peer_server = CenterPeer::install("tasks-show-session-missing-unclaimed");
            let listener = peer_server.listener();
            let peer = std::thread::spawn(move || {
                let mut stream = crate::app::test_support::accept_center_client(&listener);
                let request = read_center_request(&stream);
                write_center_response(
                    &mut stream,
                    &request,
                    serde_json::json!({"type": "claimed-task", "data": data}),
                );
            });
            let result = handle_tasks_show(None, Some("the-session"), None, true);
            peer.join().expect("join center peer");
            let error = result.expect_err("missing/unclaimed/ambiguous must be a command error");
            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn session_show_ambiguous_names_every_matching_session() {
        let peer_server = CenterPeer::install("tasks-show-session-ambiguous-names");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = crate::app::test_support::accept_center_client(&listener);
            let request = read_center_request(&stream);
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({
                    "type": "claimed-task",
                    "data": {"type": "ambiguous", "data": ["dup-a", "dup-b"]}
                }),
            );
        });
        let result = handle_tasks_show(None, Some("dup"), None, true);
        peer.join().expect("join center peer");
        let error = result.expect_err("ambiguous session must be a command error");
        let message = error.to_string();
        assert!(message.contains("dup-a"));
        assert!(message.contains("dup-b"));
    }

    #[test]
    fn session_show_renders_the_human_panel() {
        let peer_server = CenterPeer::install("tasks-show-session-human");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = crate::app::test_support::accept_center_client(&listener);
            let request = read_center_request(&stream);
            assert_eq!(request["kind"], "claimed-task");
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({
                    "type": "claimed-task",
                    "data": {
                        "type": "task",
                        "data": [
                            {
                                "key": "0001",
                                "title": "Fixture task",
                                "description": "the description",
                                "stored-status": "ready",
                                "auto-close": "checked"
                            },
                            {
                                "type": "effective",
                                "data": "checked"
                            }
                        ]
                    }
                }),
            );
            let mut detail_stream = crate::app::test_support::accept_center_client(&listener);
            let detail_request = read_center_request(&detail_stream);
            write_center_response(&mut detail_stream, &detail_request, fixture_task_detail());
        });
        let result = handle_tasks_show(None, Some("the-session"), None, false);
        peer.join().expect("join center peer");
        result.expect("session-addressed human rendering succeeds");
    }

    fn fixture_task_detail() -> serde_json::Value {
        serde_json::json!({
            "type": "task-detail",
            "data": {"type": "resolved", "data": {
                "summary": {"key": "0001", "title": "Fixture task", "stored-status": "ready", "derived-status": "ready", "archived": false},
                "content": "the description", "state": "ready", "current_activity": false,
                "claim": {"type": "no-claim"}
            }}
        })
    }

    #[test]
    fn task_addressed_show_never_opens_a_center_connection() {
        let peer_server = CenterPeer::install("tasks-show-task-addressed-no-connection");
        let listener = peer_server.listener();
        let board = task_board();
        let checker = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set listener nonblocking");
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(
                listener.accept().is_err(),
                "task-addressed show must not connect to the center"
            );
        });
        let result = handle_tasks_show(Some("0001"), None, Some(board.as_str()), true);
        checker.join().expect("join connection checker");
        result.expect("task-addressed show succeeds without a center");
        let _ = std::fs::remove_dir_all(board.as_std_path());
    }

    #[test]
    fn task_addressed_show_renders_the_human_panel_without_a_center_connection() {
        let peer_server = CenterPeer::install("tasks-show-task-addressed-human-no-connection");
        let listener = peer_server.listener();
        let board = task_board();
        let checker = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set listener nonblocking");
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(
                listener.accept().is_err(),
                "task-addressed show must not connect to the center"
            );
        });
        let result = handle_tasks_show(Some("0001"), None, Some(board.as_str()), false);
        checker.join().expect("join connection checker");
        result.expect("task-addressed human rendering succeeds without a center");
        let _ = std::fs::remove_dir_all(board.as_std_path());
    }

    #[test]
    fn task_addressed_show_missing_task_is_a_loud_error_without_a_center_connection() {
        let peer_server = CenterPeer::install("tasks-show-task-addressed-missing-no-connection");
        let listener = peer_server.listener();
        let board = task_board();
        let checker = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set listener nonblocking");
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert!(
                listener.accept().is_err(),
                "task-addressed show must not connect to the center"
            );
        });
        let result = handle_tasks_show(Some("no-such-task"), None, Some(board.as_str()), true);
        checker.join().expect("join connection checker");
        let error = result.expect_err("a missing task must fail loudly");
        assert!(
            error
                .to_string()
                .contains("no task matching \"no-such-task\"")
        );
        let _ = std::fs::remove_dir_all(board.as_std_path());
    }
}
