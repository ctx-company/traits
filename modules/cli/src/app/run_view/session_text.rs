//! Session text helpers: derive small display strings (status labels, prompt
//! context, harness summaries) from a live `Session`. Leaf module — no
//! dependencies on other `run_view` submodules.

use std::collections::BTreeMap;

use serde_json::Value;

use super::planned::structural_step_key;
use crate::app::tui;

/// P552: the trait name and accepted-input text a session-title prompt is
/// built from, independent of a live `RunPanel` — the drive-side title
/// dispatch (which must also run for panel-less surfaces such as a
/// dashboard-spawned `--progress none` drive) derives its prompt context
/// through this function rather than requiring a panel to exist first.
pub(crate) fn title_prompt_context_for(
    trait_name: &str,
    session: &ctx_traits_core::procedure::session::Session,
) -> (String, String) {
    (trait_name.to_string(), input_text(session))
}

pub(crate) fn input_text(session: &ctx_traits_core::procedure::session::Session) -> String {
    let inputs: Vec<String> = session
        .accepted_port_values
        .iter()
        .filter(|value| {
            value.acceptance == ctx_traits_core::procedure::runtime::AcceptanceStatus::Accepted
        })
        .map(|value| {
            let id = value
                .ref_text
                .strip_prefix("port:")
                .unwrap_or(&value.ref_text);
            format!("{id} {}", value_text(&value.value))
        })
        .collect();
    if inputs.is_empty() {
        "not provided".to_string()
    } else {
        inputs.join(" · ")
    }
}

pub(super) fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => clean_value_text(text),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_string()),
    }
}

pub(super) fn clean_value_text(text: &str) -> String {
    tui::clean_live_text(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Human summary of a structured control-flow stop: where the run stopped, how
/// many loop iterations ran, and which signals the stop emitted.
pub(crate) fn stop_reason_summary(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<String> {
    let stop = session.stop_reason.as_ref()?;
    let mut summary = stop.reason.clone();
    let iteration = stop
        .at
        .iter()
        .rev()
        .find_map(|segment| segment.iteration)
        .map(|iteration| iteration + 1);
    // The outermost segment names the stopped sequence item (the loop as the
    // trait declares it); deeper segments name its body internals.
    if let Some(segment) = stop.at.first() {
        let name = segment.id.as_deref().unwrap_or("unnamed");
        match iteration {
            Some(iteration) => summary.push_str(&format!(
                " — loop {name} stopped after iteration {iteration}"
            )),
            None => summary.push_str(&format!(" — at {name}")),
        }
    }
    let signals = session
        .emitted_signals
        .iter()
        .map(|signal| signal.signal_ref.as_str().to_string())
        .collect::<Vec<_>>();
    if !signals.is_empty() {
        summary.push_str(&format!(" · emitted {}", signals.join(", ")));
    }
    Some(summary)
}

pub(crate) fn phase_text(session: &ctx_traits_core::procedure::session::Session) -> String {
    match session.current_sequence_title.as_deref() {
        Some(title) if !title.trim().is_empty() => {
            format!("{} · {}", session_status(&session.status), title)
        }
        _ => session_status(&session.status).to_string(),
    }
}

pub(super) fn active_label(session: &ctx_traits_core::procedure::session::Session) -> String {
    let role = session
        .current_agent
        .as_ref()
        .map(|agent| agent.role.as_str())
        .unwrap_or(ctx_traits_io::harness_config::DEFAULT_SEAT);
    let structural_seat = session
        .current_agent
        .as_ref()
        .and_then(|agent| agent.structural_seat);
    let harness = session
        .provenance
        .agent_assignments
        .as_ref()
        .and_then(|assignments| {
            ctx_traits_core::procedure::session::select_agent_assignment(
                assignments,
                role,
                structural_seat,
            )
        })
        .map(|assignment| assignment.harness.as_str())
        .unwrap_or("unassigned");
    format!("in-progress {role}@{harness}")
}

pub(super) fn harness_summary(
    harness_by_role: &BTreeMap<String, Vec<(Option<u32>, String)>>,
) -> String {
    if harness_by_role.is_empty() {
        return "unassigned".to_string();
    }
    harness_by_role
        .iter()
        .map(|(role, rows)| format!("{role}→{}", harness_joined(rows)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Joined display text for every one of `role`'s configured seats' harness,
/// in seat order — the header's per-role summary, which (unlike a single
/// planned row's harness) legitimately reports every distinct configured
/// harness at once.
pub(super) fn harness_joined(rows: &[(Option<u32>, String)]) -> String {
    if rows.is_empty() {
        return "unassigned".to_string();
    }
    rows.iter()
        .map(|(_, harness)| harness.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn active_key(session: &ctx_traits_core::procedure::session::Session) -> Option<String> {
    session.next_frame.as_ref().map(|frame| {
        structural_step_key(
            frame.run_index.unwrap_or(session.current_run_index),
            frame.item_id.as_deref().unwrap_or(""),
            &frame.position_path,
            frame
                .assigned_agent
                .as_ref()
                .map(|agent| agent.role.as_str())
                .unwrap_or(ctx_traits_io::harness_config::DEFAULT_SEAT),
        )
    })
}

pub(crate) fn session_status(status: &ctx_traits_core::procedure::session::Status) -> &'static str {
    match status {
        ctx_traits_core::procedure::session::Status::AwaitingInput => "awaiting-input",
        ctx_traits_core::procedure::session::Status::WaitingOnHuman => "waiting-on-human",
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput => "in-progress",
        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired => {
            "blocked-command-permission-required"
        }
        ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned => {
            "blocked-agent-unassigned"
        }
        ctx_traits_core::procedure::session::Status::Rejected => "rejected",
        ctx_traits_core::procedure::session::Status::Blocked => "blocked",
        ctx_traits_core::procedure::session::Status::Completed => "completed",
        ctx_traits_core::procedure::session::Status::Failed => "failed",
    }
}

pub(super) fn output_port_status(
    status: &ctx_traits_core::procedure::runtime::OutputPortStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::runtime::OutputPortStatus::Accepted => "accepted",
        ctx_traits_core::procedure::runtime::OutputPortStatus::Missing => "missing",
        ctx_traits_core::procedure::runtime::OutputPortStatus::OptionalMissing => {
            "optional-missing"
        }
    }
}
