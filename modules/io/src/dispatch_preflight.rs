//! Dispatch-time standing-wall pre-flight for the `implement-*` family (P414).
//!
//! Before a session, worktree, or first frame exists, refuse to dispatch a
//! phase whose own execution-plan section carries an explicit `**Wall:**
//! <id>` label when a repository-scoped ledger already records a BLOCKED
//! `implement-*` run whose typed park report cites that exact wall id — and
//! no later run of that wall's ORIGINATING phase has since completed. Parsing
//! is anchored to the same checklist/section-boundary seam the removed P375
//! dependency pre-flight used (`- [ ] **ID**` entries, `## Group <N>`
//! headings): recovered here narrowly for wall-label lookup, not P375's full
//! dependency-clause grammar. An id is never inferred from prose similarity —
//! only an explicit, identical `**Wall:**` label id ever blocks a sibling.

use std::collections::BTreeMap;

use camino::Utf8Path;

use ctx_traits_core::procedure::session::{Session, Status};

const EXECUTION_PLAN_RESOURCE_ID: &str = "execution-plan";
const WALL_LABEL: &str = "**Wall:**";
const IMPLEMENT_FAMILY_ID: &str = "implement";
const IMPLEMENT_FAMILY_PREFIX: &str = "implement-";
const PARK_REPORT_SLOT_REF: &str = "slot:park-report";

/// A standing wall found among this repository's ledgers: the wall id, the
/// phase that originally recorded it, and the run that blocked on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingWall {
    pub wall_id: String,
    pub origin_phase: String,
    pub origin_run_id: String,
}

/// The deterministic dispatch refusal message for a standing wall.
pub fn refusal_message(standing: &StandingWall) -> String {
    format!(
        "wall {} standing since run {} (phase {})",
        standing.wall_id, standing.origin_run_id, standing.origin_phase
    )
}

/// Only `implement-*` traits participate in wall preflight — every other
/// trait dispatches unaffected.
pub fn is_implement_family(trait_id: &str) -> bool {
    trait_id == IMPLEMENT_FAMILY_ID || trait_id.starts_with(IMPLEMENT_FAMILY_PREFIX)
}

/// Parse the explicit `**Wall:** <id>` label, if any, out of the section of
/// the trait's declared `execution-plan` resource that `phase_value` names
/// (a single checklist entry or a `## Group <N>` heading's whole section). A
/// trait without a declared `execution-plan` resource, a missing phase
/// value, an unreadable plan, or a phase/group reference absent from the
/// checklist yields `None` — never a refusal.
pub fn explicit_wall_id(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    phase_value: Option<&str>,
) -> crate::Result<Option<String>> {
    if !is_implement_family(trait_ref.id.as_str()) {
        return Ok(None);
    }
    let Some(phase_value) = phase_value else {
        return Ok(None);
    };
    let Some(resource) = trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == EXECUTION_PLAN_RESOURCE_ID)
    else {
        return Ok(None);
    };
    let Some(relative_path) = resource.path.as_deref() else {
        return Ok(None);
    };
    let roots = crate::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    let presented = crate::resource::presentation_path(&roots, resource, relative_path)?;
    if !matches!(
        presented.status,
        crate::resource::PresentationStatus::Available
    ) {
        return Ok(None);
    }
    let text = crate::read::read_text(&presented.path)?;
    Ok(wall_id_in_section(&text, phase_value))
}

/// Parse one checklist line into its declared id and check state, if it is a
/// `- [ ] **ID** ...` / `- [x] **ID** ...` entry. Recovered verbatim from the
/// removed P375 implementation (`e04e04b`) — the shared boundary-finding
/// seam, not its dependency-clause grammar.
fn checklist_entry(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !(trimmed.starts_with("- [x] ")
        || trimmed.starts_with("- [X] ")
        || trimmed.starts_with("- [ ] "))
    {
        return None;
    }
    let after_checkbox = &trimmed[6..];
    let after_bold_open = after_checkbox.strip_prefix("**")?;
    let bold_end = after_bold_open.find("**")?;
    let bold_text = &after_bold_open[..bold_end];
    let id = bold_text.split_whitespace().next()?;
    let id = id.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '-'));
    if id.is_empty() { None } else { Some(id) }
}

fn group_heading_number(line: &str) -> Option<u64> {
    let after = line.trim_start().strip_prefix("## Group ")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn requested_group_number(phase_value: &str) -> Option<u64> {
    let after = phase_value.strip_prefix("Group ")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Bound the plan text to the section named by `phase_value` (a group's
/// whole section, or one checklist entry's own indented body), then return
/// the first `**Wall:**` label's id found in that bound section, if any.
fn wall_id_in_section(plan_text: &str, phase_value: &str) -> Option<String> {
    let lines: Vec<&str> = plan_text.lines().collect();
    let (start, end) = if let Some(group_number) = requested_group_number(phase_value) {
        let start = lines
            .iter()
            .position(|line| group_heading_number(line) == Some(group_number))?;
        let end = lines[start + 1..]
            .iter()
            .position(|line| line.trim_start().starts_with("## "))
            .map(|offset| start + 1 + offset)
            .unwrap_or(lines.len());
        (start, end)
    } else {
        let start = lines
            .iter()
            .position(|line| checklist_entry(line) == Some(phase_value))?;
        let end = lines[start + 1..]
            .iter()
            .position(|line| checklist_entry(line).is_some() || line.trim_start().starts_with('#'))
            .map(|offset| start + 1 + offset)
            .unwrap_or(lines.len());
        (start, end)
    };
    lines[start..end].iter().find_map(|line| wall_label(line))
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
/// a later completed run of the same originating phase. Ledgers with no
/// typed park report (legacy blocked runs, or runs blocked for unrelated
/// reasons) are ignored. `dispatched_phase` is the phase value about to be
/// dispatched: a wall never refuses re-dispatching the SAME phase that
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
/// history (`session_phase`, park report, terminal epoch) a summary cannot
/// carry. Every other row (typically most of a diverse ledger store) is
/// skipped without ever touching its frame history.
pub fn find_standing_wall(
    wall_id: &str,
    dispatched_phase: &str,
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
        dispatched_phase,
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
    dispatched_phase: &str,
) -> Option<StandingWall> {
    // Latest completion epoch per phase_value, used to decide whether a
    // blocked run's wall has since been cleared — keyed on phase alone (not
    // `(trait_id, phase_value)`): the wall marks a PHASE as blocked, and an
    // approved completion of that same phase through any implement-family
    // variant (quick, default, smart, strict, phase) clears it, since the
    // workflow phase is not tied to which variant last ran it.
    let mut latest_completed: BTreeMap<String, u64> = BTreeMap::new();
    for session in sessions {
        if session.status != Status::Completed {
            continue;
        }
        let Some(phase_value) = crate::run_session::session_phase(session) else {
            continue;
        };
        let Some(epoch) = terminal_epoch(session) else {
            continue;
        };
        let entry = latest_completed.entry(phase_value).or_insert(0);
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
        let origin_phase = crate::run_session::session_phase(session).unwrap_or_default();
        if origin_phase == dispatched_phase {
            continue;
        }
        let Some(blocked_epoch) = terminal_epoch(session) else {
            // No persisted terminal timestamp for this block: never treat it
            // as clearable by epoch comparison, but it still stands as a wall.
            return Some(StandingWall {
                wall_id: wall_id.to_string(),
                origin_phase,
                origin_run_id: session.run_id.as_str().to_string(),
            });
        };
        let cleared = latest_completed
            .get(&origin_phase)
            .is_some_and(|completed_epoch| *completed_epoch >= blocked_epoch);
        if cleared {
            continue;
        }
        return Some(StandingWall {
            wall_id: wall_id.to_string(),
            origin_phase,
            origin_run_id: session.run_id.as_str().to_string(),
        });
    }
    None
}
