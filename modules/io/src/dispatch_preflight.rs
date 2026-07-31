//! Dispatch-time standing-wall pre-flight for the `implement-*` family (P414).
//!
//! Before a session, worktree, or first frame exists, refuse to dispatch a
//! task whose own task file carries an explicit `**Wall:** <id>` label when
//! a repository-scoped ledger already records a BLOCKED `implement-*` run
//! whose typed park report cites that exact wall id — and no later run of
//! that wall's ORIGINATING task has since completed. The task value resolves
//! to one file among the trait's declared `task-board` directory's direct
//! children (2026-07-31 migration off `.plans/EXECUTION_PLAN.md`'s
//! checklist grammar), and the whole file is that task's section. An id is
//! never inferred from prose similarity — only an explicit, identical
//! `**Wall:**` label id ever blocks a sibling.

use std::collections::BTreeMap;

use camino::Utf8Path;

use ctx_traits_core::procedure::session::{Session, Status};

const TASK_BOARD_RESOURCE_ID: &str = "task-board";
const WALL_LABEL: &str = "**Wall:**";
const STATUS_LABEL: &str = "**Status:**";
const IMPLEMENT_FAMILY_ID: &str = "implement";
const IMPLEMENT_FAMILY_PREFIX: &str = "implement-";
const PARK_REPORT_SLOT_REF: &str = "slot:park-report";

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

/// Only `implement-*` traits participate in wall preflight — every other
/// trait dispatches unaffected.
pub fn is_implement_family(trait_id: &str) -> bool {
    trait_id == IMPLEMENT_FAMILY_ID || trait_id.starts_with(IMPLEMENT_FAMILY_PREFIX)
}

/// Resolve `task_value` to the text of its task file among the trait's
/// declared `task-board` directory's direct children, following the same
/// resolution chain [`explicit_wall_id`] and `blocked_status_marker` both
/// need: trait is `implement-*` → `task-board` resource → board directory
/// → `task_file_name_in_board` → validated `presentation_path` read. A
/// trait without a declared `task-board` resource, a missing task value,
/// an unreadable board, or a task value matching no file yields `None` —
/// never a refusal. Returns the file's text alongside its resolved file
/// name, since callers building refusal messages need the name too.
fn read_task_board_file(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    task_value: Option<&str>,
) -> crate::Result<Option<(String, String)>> {
    if !is_implement_family(trait_ref.id.as_str()) {
        return Ok(None);
    }
    let Some(task_value) = task_value else {
        return Ok(None);
    };
    let Some(resource) = trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == TASK_BOARD_RESOURCE_ID)
    else {
        return Ok(None);
    };
    let Some(relative_path) = resource.path.as_deref() else {
        return Ok(None);
    };
    let roots = crate::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    // The board must be a DIRECTORY — its presented path is the validated,
    // root-contained location to list. The chosen task FILE then goes
    // through `presentation_path` itself, so the actual read gets the full
    // containment/symlink/regular-file validation chain.
    let presented_board = crate::resource::presentation_path(&roots, resource, relative_path)?;
    if presented_board.status != crate::resource::PresentationStatus::Directory {
        return Ok(None);
    }
    let Some(file_name) = task_file_name_in_board(&presented_board.path, task_value) else {
        return Ok(None);
    };
    let presented = crate::resource::presentation_path(
        &roots,
        resource,
        &format!("{relative_path}/{file_name}"),
    )?;
    if !matches!(
        presented.status,
        crate::resource::PresentationStatus::Available
    ) {
        return Ok(None);
    }
    let text = crate::read::read_text(&presented.path)?;
    Ok(Some((text, file_name)))
}

/// Parse the explicit `**Wall:** <id>` label, if any, out of the task file
/// that `task_value` names among the trait's declared `task-board`
/// directory's direct children. A trait without a declared `task-board`
/// resource, a missing task value, an unreadable board, or a task value
/// matching no file yields `None` — never a refusal.
pub fn explicit_wall_id(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    task_value: Option<&str>,
) -> crate::Result<Option<String>> {
    let Some((text, _file_name)) = read_task_board_file(trait_ref, trait_root, task_value)? else {
        return Ok(None);
    };
    Ok(text.lines().find_map(wall_label))
}

/// A task file carrying an explicit blocked-status marker in its
/// `**Status:**` header line — the file name (for the refusal message's
/// clearing instruction) and the offending marker line's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedStatusMarker {
    pub file_name: String,
    pub status_line: String,
}

/// The deterministic dispatch refusal message for a blocked-status marker.
pub fn blocked_status_refusal_message(marker: &BlockedStatusMarker) -> String {
    format!(
        "task file {} carries a blocked-status marker ({}); edit .internal/tasks/{} and remove the blocked marker once the dependency lands",
        marker.file_name, marker.status_line, marker.file_name
    )
}

/// Scan the task file `task_value` names for an explicit blocked marker on
/// its `**Status:**` header line — the standalone word `blocked` or the
/// token `deps-unmet` (word-boundary matched, so "unblocked" never trips).
/// Deliberately conservative, mirroring the `**Wall:**` precedent:
/// explicit-labels-only, never inferred from prose. A trait without a
/// declared `task-board` resource, a missing task value, an unreadable
/// board, a task value matching no file, or a `**Status:**` line without
/// either token yields `None` — never a refusal.
pub fn blocked_status_marker(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    task_value: Option<&str>,
) -> crate::Result<Option<BlockedStatusMarker>> {
    let Some((text, file_name)) = read_task_board_file(trait_ref, trait_root, task_value)? else {
        return Ok(None);
    };
    let Some(status_line) = text.lines().find(|line| line.contains(STATUS_LABEL)) else {
        return Ok(None);
    };
    if !status_line_has_blocked_marker(status_line) {
        return Ok(None);
    }
    Ok(Some(BlockedStatusMarker {
        file_name,
        status_line: status_line.trim().to_string(),
    }))
}

/// Whether `status_line` contains the standalone word `blocked` or the
/// token `deps-unmet`, word-boundary matched against ASCII alphanumerics
/// and `-`/`_` so "unblocked" or "deps-unmet-tracker" never trips.
fn status_line_has_blocked_marker(status_line: &str) -> bool {
    let lower = status_line.to_ascii_lowercase();
    contains_word(&lower, "blocked") || contains_word(&lower, "deps-unmet")
}

/// Whether `haystack` contains `word` as a standalone token: the match is
/// not immediately preceded or followed by an ASCII alphanumeric, `-`, or
/// `_` character.
fn contains_word(haystack: &str, word: &str) -> bool {
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut start = 0;
    while let Some(offset) = haystack[start..].find(word) {
        let match_start = start + offset;
        let match_end = match_start + word.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = match_start + 1;
    }
    false
}

/// Resolve `task_value` to a file name among the board directory's direct
/// children — the exact filename, the exact stem, or (for a bare all-digit
/// task number) the `NNNN-` prefix — mirroring how the implement family's
/// own extraction step names a task. Subdirectories (`archived/` among
/// them) never match: an archived task is not a live task. Candidates are
/// sorted so a prefix match is deterministic even if numbers ever collide.
fn task_file_name_in_board(board: &Utf8Path, task_value: &str) -> Option<String> {
    let entries = std::fs::read_dir(board.as_std_path()).ok()?;
    let numeric = !task_value.is_empty() && task_value.chars().all(|c| c.is_ascii_digit());
    let prefix = format!("{task_value}-");
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();
    names.into_iter().find(|name| {
        let stem = &name[..name.len() - 3];
        name == task_value || stem == task_value || (numeric && stem.starts_with(&prefix))
    })
}

/// The id following a `**Wall:**` label on `line`, if present — the first
/// whitespace-delimited token after the label.
fn wall_label(line: &str) -> Option<String> {
    let pos = line.find(WALL_LABEL)?;
    let body = line[pos + WALL_LABEL.len()..].trim();
    let id = body.split_whitespace().next()?;
    let id = id.trim_end_matches(['.', ',']);
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// The typed park-report entry a blocked session's final review round
/// accepted onto `slot:park-report`, if any — read from `accepted_slot_values`
/// (the SAME evidence a completed session's `port:park-report` final output
/// is itself projected from at completion time), never a re-derivation from
/// `completion.final-outputs` (a blocked session never reaches normal
/// completion, so that projection never runs for it). A slot's LATEST
/// accepted value replaces the prior round's per `parkReport`'s own
/// "Replaced by each review step" contract, so the last matching entry in
/// this append-ordered evidence list is the one that stood when the run
/// blocked.
fn session_park_report(session: &Session) -> Option<serde_json::Value> {
    let value = session
        .accepted_slot_values
        .iter()
        .rev()
        .find(|value| value.ref_text == PARK_REPORT_SLOT_REF)?;
    value.value.as_array()?.first().cloned()
}

/// Scan this repository's session ledgers for a BLOCKED `implement-*` run
/// whose park report cites `wall_id`, and that has not since been cleared by
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
/// or not a sidecar exists) and only rows whose summary is genuinely
/// `implement-*` and Completed-or-Blocked — the only two states this
/// preflight ever inspects — are deep-parsed a second time for the frame
/// history (`session_task`, park report, terminal epoch) a summary cannot
/// carry. Every other row (typically most of a diverse ledger store) is
/// skipped without ever touching its frame history.
pub fn find_standing_wall(
    wall_id: &str,
    dispatched_task: &str,
) -> crate::Result<Option<StandingWall>> {
    let mut sessions = Vec::new();
    for path in crate::run_session::session_store_paths(None)? {
        let Ok(summary) = crate::run_summary::read_summary_or_ledger(&path) else {
            continue;
        };
        if !is_implement_family(&summary.trait_id) {
            continue;
        }
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

/// `sessions` is already filtered to `implement-*` + Completed-or-Blocked by
/// [`find_standing_wall`]'s summary-first scan; this function does not
/// re-check either condition, since every element it receives already
/// satisfies both.
fn standing_wall_in_sessions(
    sessions: &[Session],
    wall_id: &str,
    dispatched_task: &str,
) -> Option<StandingWall> {
    // Latest completion epoch per task_value, used to decide whether a
    // blocked run's wall has since been cleared — keyed on task alone (not
    // `(trait_id, task_value)`): the wall marks a TASK as blocked, and an
    // approved completion of that same task through any implement-family
    // variant (quick, default, smart, strict, phase) clears it, since the
    // task is not tied to which variant last ran it.
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
        let Some(park_report) = session_park_report(session) else {
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

    #[test]
    fn blocked_status_line_is_flagged() {
        assert!(status_line_has_blocked_marker("**Status:** blocked"));
        assert!(status_line_has_blocked_marker(
            "**Status:** deps-unmet · waiting on 0046"
        ));
        assert!(status_line_has_blocked_marker("**Status:** Blocked"));
    }

    #[test]
    fn ready_status_line_is_not_flagged() {
        assert!(!status_line_has_blocked_marker(
            "**Status:** ready to implement"
        ));
        assert!(!status_line_has_blocked_marker("**Status:** in progress"));
    }

    #[test]
    fn unblocked_does_not_trip_the_blocked_token() {
        assert!(!status_line_has_blocked_marker(
            "**Status:** unblocked, ready to implement"
        ));
    }

    #[test]
    fn deps_unmet_prefix_does_not_trip_on_unrelated_token() {
        assert!(!status_line_has_blocked_marker(
            "**Status:** deps-unmet-tracker-closed"
        ));
    }

    #[test]
    fn contains_word_matches_only_at_boundaries() {
        assert!(contains_word("status blocked here", "blocked"));
        assert!(!contains_word("status unblocked here", "blocked"));
        assert!(!contains_word("status blockedout here", "blocked"));
        assert!(contains_word("blocked", "blocked"));
    }
}
