//! Dispatch-time pre-flight for the configured `[tasks] dispatch-trait`:
//! standing walls (P414), closed-status tasks, and unmet dependencies
//! (0061).
//!
//! Task binding belongs to exactly one trait per repository: the one
//! `[tasks] dispatch-trait` names. Running THAT trait reads its `task`
//! input as a task reference (an SDK `schema:task`-typed port, validated at
//! dispatch) and binds it through the board; every other trait's `task`
//! input is plain text — binding is never inferred from a trait id, a port
//! name, or any other naming convention.
//!
//! When binding runs: before a session, worktree, or first frame exists,
//! refuse to dispatch a task whose own task file carries a typed `wall` id
//! (0063.1) when a repository-scoped ledger already records a BLOCKED
//! task-dispatched run whose typed park report cites that exact wall id —
//! and no later run of that wall's ORIGINATING task has since completed.
//! The task value resolves through the
//! [`ctx_traits_core::task::provider::TaskProvider`] interface (0060/0061)
//! against the trait's declared `task-board` directory — never a private
//! filename/stem/prefix chain of this module's own — and the whole resolved
//! document is that task's evidence. An id is never inferred from prose
//! similarity — only an explicit, identical `wall` id ever blocks a
//! sibling. [`resolve_dispatch_task`] is the single read every preflight
//! (wall, closed-status, dependency) and dispatch materialisation shares.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::procedure::session::{Session, Status};
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{ResolvedTask, TaskProvider};

const TASK_BOARD_RESOURCE_ID: &str = "task-board";

fn provider_error(error: ctx_traits_core::task::provider::ProviderError) -> crate::Error {
    crate::Error::Usage {
        message: error.to_string(),
    }
}

/// A task value fit for a refusal message: task ids are short, but a manual
/// description can be paragraphs — truncate so the refusal stays legible
/// instead of echoing the whole paragraph back mid-sentence.
fn display_task_value(task_value: &str) -> String {
    const MAX: usize = 60;
    if task_value.chars().count() <= MAX {
        return task_value.to_string();
    }
    let truncated: String = task_value.chars().take(MAX).collect();
    format!("{truncated}…")
}

/// A standing wall found among this repository's ledgers: the wall id, the
/// task that originally recorded it, and the run that blocked on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingWall {
    pub wall_id: String,
    pub origin_task: String,
    pub origin_run_id: String,
}

/// The deterministic dispatch refusal message for a standing wall.
pub fn refusal_message(standing: &StandingWall) -> String {
    format!(
        "wall {} standing since run {} (task {})",
        standing.wall_id, standing.origin_run_id, standing.origin_task
    )
}

/// A task resolved through the `TaskProvider` interface for dispatch-time
/// preflight, plus the presentation evidence the preflights and
/// materialisation both need beyond what [`ResolvedTask`] itself carries:
/// the file name (for the closed-status refusal's clearing instruction) and
/// the board directory's validated presentation path (for materialising
/// the same document into a run's worktree). Resolved exactly once per
/// dispatch and threaded through every preflight plus materialisation, so
/// an edit landing between checks can never split "what was checked" from
/// "what was run".
#[derive(Debug, Clone)]
pub struct DispatchTask {
    pub resolved: ResolvedTask,
    pub file_name: String,
    pub board_dir: Utf8PathBuf,
    /// The invocation repository root the board directory resolved under,
    /// when the `task-board` resource declares `root = "repo"` and a Git
    /// repository was discovered. `None` for a package-rooted board (no
    /// worktree image exists to materialise into) or when no invocation
    /// repository was found — materialisation fails open in both cases.
    pub invocation_repo_root: Option<Utf8PathBuf>,
}

/// Resolve `task_value` through the trait's declared `task-board` resource
/// via the [`TaskProvider`] interface: `task-board` resource → board
/// directory → `FilesTaskBoard::resolve`/`get`. Call this ONLY for a
/// dispatch of the configured `[tasks] dispatch-trait` —
/// never speculatively: on every other dispatch a `task` input port is
/// plain text with whatever semantics the trait gives it (0063.4). A
/// dispatch carrying no task value at all yields
/// [`DispatchTaskResolution::NotRequested`]. An explicit task value that
/// cannot bind — no declared `task-board` resource, an unreadable/absent
/// board, a task value matching no live task, or a value resolving only to
/// an archived document — yields [`DispatchTaskResolution::CannotBind`]: a
/// silently unbound task dispatch is exactly the dishonesty this product
/// exists to remove. This is the single read every preflight (wall,
/// closed-status, dependency) and dispatch materialisation shares.
pub fn resolve_dispatch_task(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    task_value: Option<&str>,
) -> crate::Result<DispatchTaskResolution> {
    let Some(task_value) = task_value else {
        return Ok(DispatchTaskResolution::NotRequested);
    };
    let cannot_bind = |reason: String| DispatchTaskResolution::CannotBind {
        trait_id: trait_ref.id.as_str().to_string(),
        reason,
    };
    let Some(resource) = trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == TASK_BOARD_RESOURCE_ID)
    else {
        return Ok(cannot_bind(format!(
            "declares no {TASK_BOARD_RESOURCE_ID} resource"
        )));
    };
    let Some(relative_path) = resource.path.as_deref() else {
        return Ok(cannot_bind(format!(
            "its {TASK_BOARD_RESOURCE_ID} resource declares no path"
        )));
    };
    let roots = crate::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    // The board must be a DIRECTORY — its presented path is the validated,
    // root-contained location the files backend opens.
    let presented_board = crate::resource::presentation_path(&roots, resource, relative_path)?;
    if presented_board.status != crate::resource::PresentationStatus::Directory {
        return Ok(cannot_bind(format!(
            "{TASK_BOARD_RESOURCE_ID} directory {} is not present",
            presented_board.path
        )));
    }
    let board = crate::task_files::FilesTaskBoard::open_read(presented_board.path.clone());
    let Some(key) = board.resolve(task_value).map_err(provider_error)? else {
        return Ok(cannot_bind(format!(
            "task {:?} does not resolve on the {TASK_BOARD_RESOURCE_ID} (manual task descriptions — virtual tasks — are not supported yet; pass a task id)",
            display_task_value(task_value)
        )));
    };
    let Some(resolved) = board.get(&key).map_err(provider_error)? else {
        return Ok(cannot_bind(format!(
            "task {key} does not resolve on the {TASK_BOARD_RESOURCE_ID}"
        )));
    };
    if resolved.archived {
        return Ok(cannot_bind(format!("task {key} is archived")));
    }
    let Some(file_name) =
        crate::task_files::task_file_name_in_dir(&presented_board.path, &resolved.document.key)
    else {
        return Ok(cannot_bind(format!(
            "task {key} has no file on the {TASK_BOARD_RESOURCE_ID}"
        )));
    };
    Ok(DispatchTaskResolution::Bound(Box::new(DispatchTask {
        resolved,
        file_name,
        board_dir: presented_board.path,
        invocation_repo_root: roots.invocation_repo_root,
    })))
}

/// The three-state outcome of [`resolve_dispatch_task`]: a dispatch never
/// requested task binding at all, a dispatch that bound successfully, or a
/// requested task dispatch that named a value it could not bind — the state
/// that must become a hard refusal at the call site rather than a silent,
/// unbound start (0063.4).
#[derive(Debug, Clone)]
pub enum DispatchTaskResolution {
    NotRequested,
    Bound(Box<DispatchTask>),
    CannotBind { trait_id: String, reason: String },
}

impl DispatchTaskResolution {
    /// The bound task, if this resolution reached one — `None` for both
    /// `NotRequested` and `CannotBind`. Convenience for callers (and tests)
    /// that only care whether a task ended up bound.
    pub fn bound(self) -> Option<DispatchTask> {
        match self {
            Self::Bound(task) => Some(*task),
            Self::NotRequested | Self::CannotBind { .. } => None,
        }
    }
}

/// The deterministic dispatch refusal message for an unbindable explicit
/// `task=` value: names the trait and the reason it cannot carry the task,
/// so the fix is legible without reading source.
pub fn cannot_bind_refusal_message(trait_id: &str, reason: &str) -> String {
    format!("trait {trait_id} cannot bind task — {reason}; dispatch refuses to start unbound")
}

/// The typed `wall` id (0063.1), if any, on `task`'s resolved document. A
/// document carrying no such field yields `None` — never a refusal.
pub fn explicit_wall_id(task: &DispatchTask) -> Option<String> {
    task.resolved.document.wall.clone()
}

/// A task document carrying a closed (`done` or `cancelled`) stored
/// status — the file name (for the refusal message's clearing
/// instruction) and the offending status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedStatusMarker {
    pub file_name: String,
    pub status: ctx_traits_core::task::TaskStatus,
}

/// The deterministic dispatch refusal message for a closed-status task.
pub fn closed_status_refusal_message(marker: &ClosedStatusMarker) -> String {
    let status = match marker.status {
        ctx_traits_core::task::TaskStatus::Done => "done",
        ctx_traits_core::task::TaskStatus::Cancelled => "cancelled",
        ctx_traits_core::task::TaskStatus::Ready => "ready",
    };
    format!(
        "task file {} carries stored status {status} — dispatch refuses done/cancelled tasks; edit .internal/tasks/{} to change its status if this is wrong",
        marker.file_name, marker.file_name
    )
}

/// Refuse to dispatch a task document whose stored `status` field is
/// `done` or `cancelled` — a direct typed-field read, never a derivation.
/// Unmet-dependency (`blocked`) refusal has no stored representation under
/// 0059's schema and moves to 0060's derived status, handled by
/// [`dependency_marker`] instead. `task`'s document with `status =
/// "ready"`/absent yields `None` — never a refusal.
pub fn closed_status_marker(task: &DispatchTask) -> Option<ClosedStatusMarker> {
    match task.resolved.document.status {
        Some(
            status @ (ctx_traits_core::task::TaskStatus::Done
            | ctx_traits_core::task::TaskStatus::Cancelled),
        ) => Some(ClosedStatusMarker {
            file_name: task.file_name.clone(),
            status,
        }),
        _ => None,
    }
}

/// One `depends-on` edge on `task`'s resolved document that has not
/// resolved `Done`/`Cancelled` — met is the edge's derived status
/// `is_closed()` (0060), not the dependency's stored field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmetDependency {
    pub key: String,
    pub title: String,
    pub status: DerivedStatus,
}

/// The word a refusal/override message names an edge's derived status by.
pub fn status_word(status: DerivedStatus) -> &'static str {
    match status {
        DerivedStatus::Ready => "ready",
        DerivedStatus::Blocked => "blocked",
        DerivedStatus::Done => "done",
        DerivedStatus::Cancelled => "cancelled",
    }
}

/// Every unmet `depends-on` edge on `resolved`'s relations, or `None` when
/// every declared dependency is closed (or none are declared). A dangling
/// `depends-on` edge (the dependency key resolves to nothing) is reported by
/// `sync`, not here — it has no status to name, so it is never counted as
/// unmet. Shared by dispatch preflight ([`dependency_marker`]) and the
/// dashboard's TASKS screen refusal (0063), so both name the same edges the
/// same way.
pub fn unmet_dependencies(resolved: &ResolvedTask) -> Option<Vec<UnmetDependency>> {
    let unmet: Vec<UnmetDependency> = resolved
        .relations
        .depends_on
        .iter()
        .filter(|edge| !edge.status.is_closed())
        .map(|edge| UnmetDependency {
            key: edge.key.clone(),
            title: edge.title.clone(),
            status: edge.status,
        })
        .collect();
    if unmet.is_empty() { None } else { Some(unmet) }
}

/// Every unmet `depends-on` edge on `task`'s resolved document — see
/// [`unmet_dependencies`], which does the actual filtering.
pub fn dependency_marker(task: &DispatchTask) -> Option<Vec<UnmetDependency>> {
    unmet_dependencies(&task.resolved)
}

/// Re-read `task`'s document from its board directory's CURRENT bytes —
/// never the snapshot [`resolve_dispatch_task`] captured — so dispatch
/// materialisation can never prefer a stale read over an edit that landed
/// between preflight and worktree preparation (the whole point of 0061:
/// read from the invocation repository's working tree, write into the
/// worktree). `None` only if the task vanished from the board or turned
/// archived in that window; a genuine backend failure is a real error, not
/// fail-open, since [`resolve_dispatch_task`] already proved the board was
/// readable moments earlier in the same dispatch.
pub fn reread_current_document(task: &DispatchTask) -> crate::Result<Option<ResolvedTask>> {
    let board = crate::task_files::FilesTaskBoard::open_read(task.board_dir.clone());
    let Some(resolved) = board
        .get(&task.resolved.document.key)
        .map_err(provider_error)?
    else {
        return Ok(None);
    };
    if resolved.archived {
        return Ok(None);
    }
    Ok(Some(resolved))
}

/// The deterministic dispatch refusal message for an unmet dependency,
/// naming the dependency and its current status in one sentence.
pub fn dependency_refusal_message(task_key: &str, unmet: &[UnmetDependency]) -> String {
    let deps = unmet
        .iter()
        .map(|dep| format!("{} ({})", dep.key, status_word(dep.status)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "task {task_key} depends on {deps} — dispatch refuses tasks with unmet dependencies; pass --override-dependencies to record an override and dispatch anyway"
    )
}

/// Scan this repository's session ledgers for a BLOCKED run whose park
/// report cites `wall_id`, and that has not since been cleared by
/// a later completed run of the same originating task. Ledgers with no
/// typed park report (legacy blocked runs, or runs blocked for unrelated
/// reasons) are ignored. `dispatched_task` is the task value about to be
/// dispatched: a wall never refuses re-dispatching the SAME task that
/// originally recorded it — that self-retry is the only way a wall is ever
/// cleared, so treating it as a "sibling" would make every park permanent.
///
/// Every dispatch calls this fresh (no cache reuse across calls, unlike the
/// dashboard's ticking `InventoryCache`), so it deep-parses this
/// repository's whole ledger store on every single invocation unless
/// something cheaper filters first. Per P510 §3.6's resolution order, each
/// ledger is first resolved to a [`crate::run_summary::RunSummary`] (a fresh
/// sidecar answers in two `stat`s plus a small JSON read; a missing or
/// stale one falls back to a full parse, so behavior is identical whether
/// or not a sidecar exists) and only rows whose summary is
/// Completed-or-Blocked — the only two states this preflight ever inspects
/// — are deep-parsed a second time for the frame history (`session_task`,
/// park report, terminal epoch) a summary cannot carry. A summary carries
/// no task evidence, so status is the only cheap filter; a deep-parsed
/// session that never carried a `port:task` value simply matches no wall in
/// [`standing_wall_in_sessions`].
pub fn find_standing_wall(
    wall_id: &str,
    dispatched_task: &str,
) -> crate::Result<Option<StandingWall>> {
    let mut sessions = Vec::new();
    for path in crate::run_session::session_store_paths(None)? {
        let Ok(summary) = crate::run_summary::read_summary_or_ledger(&path) else {
            continue;
        };
        if !matches!(summary.status, Status::Completed | Status::Blocked) {
            continue;
        }
        let Ok(session) = crate::run_session::read_run_session(&path) else {
            continue;
        };
        sessions.push(session);
    }
    Ok(standing_wall_in_sessions(
        &sessions,
        wall_id,
        dispatched_task,
    ))
}

/// The persisted terminal timestamp for a ledger's last drive outcome, if
/// any — `last_drive_outcome.recorded_at_epoch`, the drive's own record of
/// when this session last reached a stop, not the ledger FILE's mtime.
/// Ledger files can be rewritten (e.g. re-serialized on a later, unrelated
/// read) without the session reaching a new terminal state, so mtime alone
/// cannot order block/clear events; a session with no recorded drive outcome
/// yet sorts as never-terminal (`None`, ordered before any recorded epoch).
fn terminal_epoch(session: &Session) -> Option<u64> {
    session
        .last_drive_outcome
        .as_ref()
        .map(|outcome| outcome.recorded_at_epoch)
}

/// `sessions` is already filtered to Completed-or-Blocked by
/// [`find_standing_wall`]'s summary-first scan; this function does not
/// re-check that condition, since every element it receives already
/// satisfies it.
fn standing_wall_in_sessions(
    sessions: &[Session],
    wall_id: &str,
    dispatched_task: &str,
) -> Option<StandingWall> {
    // Latest completion epoch per task_value, used to decide whether a
    // blocked run's wall has since been cleared — keyed on task alone (not
    // `(trait_id, task_value)`): the wall marks a TASK as blocked, and an
    // approved completion of that same task through any trait clears it,
    // since the task is not tied to which trait last ran it.
    let mut latest_completed: BTreeMap<String, u64> = BTreeMap::new();
    for session in sessions {
        if session.status != Status::Completed {
            continue;
        }
        let Some(task_value) = crate::run_session::session_task(session) else {
            continue;
        };
        let Some(epoch) = terminal_epoch(session) else {
            continue;
        };
        let entry = latest_completed.entry(task_value).or_insert(0);
        *entry = (*entry).max(epoch);
    }

    for session in sessions {
        if session.status != Status::Blocked {
            continue;
        }
        let Some(park_report) = crate::run_session::session_park_report(session) else {
            continue;
        };
        let Some(entry_wall_id) = park_report.get("wall-id").and_then(|v| v.as_str()) else {
            continue;
        };
        if entry_wall_id != wall_id {
            continue;
        }
        let origin_task = crate::run_session::session_task(session).unwrap_or_default();
        if origin_task == dispatched_task {
            continue;
        }
        let Some(blocked_epoch) = terminal_epoch(session) else {
            // No persisted terminal timestamp for this block: never treat it
            // as clearable by epoch comparison, but it still stands as a wall.
            return Some(StandingWall {
                wall_id: wall_id.to_string(),
                origin_task,
                origin_run_id: session.run_id.as_str().to_string(),
            });
        };
        let cleared = latest_completed
            .get(&origin_task)
            .is_some_and(|completed_epoch| *completed_epoch >= blocked_epoch);
        if cleared {
            continue;
        }
        return Some(StandingWall {
            wall_id: wall_id.to_string(),
            origin_task,
            origin_run_id: session.run_id.as_str().to_string(),
        });
    }
    None
}

/// A declared command that cannot run in this repository, found before any
/// budget is spent on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrunnableCommand {
    pub sequence_id: String,
    pub argv: Vec<String>,
    pub reason: String,
    pub remedy: String,
}

/// The deterministic dispatch refusal message for an unrunnable command.
pub fn unrunnable_refusal_message(found: &[UnrunnableCommand]) -> String {
    found
        .iter()
        .map(|entry| {
            format!(
                "sequence {} declares `{}`, which cannot run here: {}. {}",
                entry.sequence_id,
                entry.argv.join(" "),
                entry.reason,
                entry.remedy
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Refuse at dispatch when a trait declares a command this repository cannot
/// execute.
///
/// A command step's argv lives in the TRAIT, not in the run, so an agent can
/// never repair one that does not resolve — it simply fails identically every
/// round while the reviewer, seeing only a false verdict, attributes the
/// failure to the work. Measured across this machine's ctx-gate ledgers, 45%
/// of runs exhausted their full round budget and 38% carried a blocker that
/// repeated three or more rounds; the `just test` recipe missing from a
/// consuming repository produced a byte-identical failed gate for six straight
/// rounds. Refusing here costs milliseconds and replaces an entire wasted run
/// with one actionable sentence.
///
/// Deliberately conservative — this refuses only what it can prove:
///
/// * A **path-shaped** program (containing a separator) is never refused: it
///   may legitimately be built by a setup command, or by an earlier step, and
///   not yet exist when this runs.
/// * A **bare** program name must resolve on `PATH`.
/// * `argv-from` commands supply their argv at runtime and are skipped.
///
/// `just` is additionally checked for the named recipe, because a launcher
/// that exists while its subcommand does not is the exact shape that cost
/// those six rounds, and `just` is this project's launcher. Other launchers
/// (`npm run`, `make`) would each need their own probe and get none here
/// rather than a guess.
pub fn unrunnable_check_commands(
    trait_ref: &ctx_traits_core::Trait,
    repo_root: &Utf8Path,
) -> Vec<UnrunnableCommand> {
    let mut found = Vec::new();
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        for (index, item) in sequence.sequence.iter().enumerate() {
            let field_path = format!("sequence.{sequence_id}.sequence[{index}]");
            let Ok(Some(plan)) =
                ctx_traits_core::r#trait::procedure::command::plan_for_item(item, &field_path)
            else {
                continue;
            };
            if plan.argv_from.is_some() {
                continue;
            }
            let Some(program) = plan.argv.first() else {
                continue;
            };
            if program.contains('/') || program.contains('\\') {
                continue;
            }
            if !program_on_path(program) {
                found.push(UnrunnableCommand {
                    sequence_id: sequence_id.to_string(),
                    argv: plan.argv.clone(),
                    reason: format!("`{program}` is not on PATH"),
                    remedy: format!(
                        "install `{program}`, or point the step at a program this repository has"
                    ),
                });
                continue;
            }
            if program == "just"
                && let Some(recipe) = plan.argv.get(1)
                && !recipe.starts_with('-')
                && !just_recipe_exists(repo_root, recipe)
            {
                found.push(UnrunnableCommand {
                    sequence_id: sequence_id.to_string(),
                    argv: plan.argv.clone(),
                    reason: format!("the Justfile has no recipe `{recipe}`"),
                    remedy: format!(
                        "add a `{recipe}:` recipe to this repository's Justfile so the step has something to run"
                    ),
                });
            }
        }
    }
    found
}

/// Whether a bare program name resolves to an executable file on `PATH`.
fn program_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return true;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_file())
    })
}

/// Whether `just` recognises `recipe` in `repo_root`. Any failure to ask
/// (no Justfile, `just` erroring for an unrelated reason) answers `true`, so
/// this can only ever refuse on a definite negative.
fn just_recipe_exists(repo_root: &Utf8Path, recipe: &str) -> bool {
    std::process::Command::new("just")
        .arg("--show")
        .arg(recipe)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `implement-quick` trait declaring `task-board` at
    /// `tasks` (package-rooted), for exercising [`resolve_dispatch_task`]
    /// end to end without a Git repository.
    fn implement_trait_with_board() -> ctx_traits_core::Trait {
        let text = "id = \"implement-quick\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Fixture\"\nsummary = \"Minimal fixture.\"\n\n[[resource]]\nid = \"task-board\"\npath = \"tasks\"\n";
        ctx_traits_core::encoding::decode_trait(ctx_traits_core::encoding::Encoding::Toml, text)
            .expect("fixture trait decodes")
    }

    fn write_task(dir: &std::path::Path, file_name: &str, toml: &str) {
        std::fs::write(dir.join(file_name), toml).unwrap();
    }

    const TASK_0050: &str =
        "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Example\"\nstatus = \"ready\"\n";

    #[test]
    fn resolve_dispatch_task_matches_toml_not_markdown() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(&board_dir, "0050-example.toml", TASK_0050);
        std::fs::write(board_dir.join("0050-example.md"), "").unwrap();
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0050"))
            .unwrap()
            .bound()
            .expect("resolves the toml document, ignoring the markdown sibling");
        assert_eq!(task.file_name, "0050-example.toml");
        assert_eq!(task.resolved.document.key, "0050");
    }

    #[test]
    fn resolve_dispatch_task_matches_exact_stem() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(&board_dir, "0050-example.toml", TASK_0050);
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0050-example"))
            .unwrap()
            .bound()
            .expect("resolves by exact stem");
        assert_eq!(task.file_name, "0050-example.toml");
    }

    #[test]
    fn resolve_dispatch_task_never_surfaces_an_archived_task() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(board_dir.join("archived")).unwrap();
        write_task(
            &board_dir.join("archived"),
            "0050-example.toml",
            "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Example\"\nstatus = \"done\"\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        assert!(
            matches!(
                resolve_dispatch_task(&trait_ref, trait_root, Some("0050")).unwrap(),
                DispatchTaskResolution::CannotBind { .. }
            ),
            "an archived-only task named explicitly cannot bind — dispatch must refuse, not fail open"
        );
    }

    #[test]
    fn resolve_dispatch_task_not_requested_without_a_task_value() {
        let dir = tempfile_dir();
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        assert!(matches!(
            resolve_dispatch_task(&trait_ref, trait_root, None).unwrap(),
            DispatchTaskResolution::NotRequested
        ));
    }

    /// Binding is caller-requested, never trait-id-inferred: the resolver
    /// treats every trait identically, and a boardless trait named a task
    /// value refuses regardless of what the trait is called.
    #[test]
    fn resolve_dispatch_task_cannot_bind_on_any_boardless_trait_regardless_of_id() {
        for id in ["other", "implement-quick"] {
            let dir = tempfile_dir();
            let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
            let text = format!(
                "id = \"{id}\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Fixture\"\nsummary = \"Minimal fixture.\"\n"
            );
            let trait_ref = ctx_traits_core::encoding::decode_trait(
                ctx_traits_core::encoding::Encoding::Toml,
                &text,
            )
            .expect("fixture trait decodes");
            match resolve_dispatch_task(&trait_ref, trait_root, Some("0050")).unwrap() {
                DispatchTaskResolution::CannotBind { trait_id, reason } => {
                    assert_eq!(trait_id, id);
                    assert!(reason.contains("task-board"));
                    let message = cannot_bind_refusal_message(&trait_id, &reason);
                    assert!(message.contains(id));
                    assert!(message.contains("task-board"));
                }
                other => panic!("expected CannotBind for {id}, got {other:?}"),
            }
        }
    }

    #[test]
    fn resolve_dispatch_task_cannot_bind_when_the_task_value_does_not_resolve() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        match resolve_dispatch_task(&trait_ref, trait_root, Some("9999")).unwrap() {
            DispatchTaskResolution::CannotBind { trait_id, reason } => {
                assert_eq!(trait_id, "implement-quick");
                assert!(reason.contains("9999"));
            }
            other => panic!("expected CannotBind, got {other:?}"),
        }
    }

    #[test]
    fn explicit_wall_id_reads_the_typed_field() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0050-example.toml",
            "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Example\"\nstatus = \"ready\"\nwall = \"wall-42\"\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0050"))
            .unwrap()
            .bound()
            .expect("task resolves");
        assert_eq!(explicit_wall_id(&task), Some("wall-42".to_string()));
    }

    #[test]
    fn explicit_wall_id_is_none_without_the_typed_field() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0050-example.toml",
            "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Example\"\nstatus = \"ready\"\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0050"))
            .unwrap()
            .bound()
            .expect("task resolves");
        assert_eq!(explicit_wall_id(&task), None);
    }

    #[test]
    fn closed_status_refusal_message_names_the_status() {
        let marker = ClosedStatusMarker {
            file_name: "0050-example.toml".to_string(),
            status: ctx_traits_core::task::TaskStatus::Done,
        };
        let message = closed_status_refusal_message(&marker);
        assert!(message.contains("done"));
        assert!(message.contains("0050-example.toml"));
    }

    #[test]
    fn dependency_marker_none_when_every_dependency_is_closed() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0001-dep.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Dep\"\nstatus = \"done\"\n",
        );
        write_task(
            &board_dir,
            "0002-dependent.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Dependent\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0002"))
            .unwrap()
            .bound()
            .expect("resolves");
        assert!(dependency_marker(&task).is_none());
    }

    #[test]
    fn dependency_marker_names_every_unmet_edge() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0001-dep.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Dep\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-dependent.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Dependent\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0002"))
            .unwrap()
            .bound()
            .expect("resolves");
        let unmet = dependency_marker(&task).expect("0001 is ready, not closed");
        assert_eq!(unmet.len(), 1);
        assert_eq!(unmet[0].key, "0001");
        assert_eq!(unmet[0].status, DerivedStatus::Ready);

        let message = dependency_refusal_message(&task.resolved.document.key, &unmet);
        assert!(message.contains("0002 depends on 0001 (ready)"));
        assert!(message.contains("--override-dependencies"));
    }

    #[test]
    fn unmet_dependencies_matches_dependency_marker_on_the_same_fixture() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0001-dep.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Dep\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-dependent.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Dependent\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0002"))
            .unwrap()
            .bound()
            .expect("resolves");
        assert_eq!(unmet_dependencies(&task.resolved), dependency_marker(&task));
    }

    #[test]
    fn dependency_marker_ignores_a_dangling_edge() {
        let dir = tempfile_dir();
        let board_dir = dir.join("tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        write_task(
            &board_dir,
            "0002-dependent.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Dependent\"\nstatus = \"ready\"\nrelations.depends-on = [\"9999\"]\n",
        );
        let trait_root = Utf8Path::from_path(dir.as_path()).unwrap();
        let trait_ref = implement_trait_with_board();
        let task = resolve_dispatch_task(&trait_ref, trait_root, Some("0002"))
            .unwrap()
            .bound()
            .expect("resolves");
        assert!(
            dependency_marker(&task).is_none(),
            "a dangling edge has no status to name and is `sync`'s job, not dispatch's"
        );
    }

    fn tempfile_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dispatch-preflight-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
