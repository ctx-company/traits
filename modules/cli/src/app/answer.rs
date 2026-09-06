//! `ctx traits answer <session>` (0253.4): the foreground submission path
//! for a run parked `awaiting-owner` on an authored `ask` step. Shares the
//! same TOCTOU re-check, schema-aware parse, and `run::set` write the
//! dashboard's `SessionAction::Answer` applier uses
//! ([`crate::app::dashboard::resolve_answer_trait_file`],
//! [`resolved_human_question_body`]) — this module only adds the
//! foreground resume, since a dashboard-driven answer resumes through the
//! center's worker instead.

use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::{CallerProvenance, Status};
use ctx_traits_core::response::{CommandOutput, Envelope};
use serde::Serialize;

use ctx_traits_io::answer::{
    AnswerDeliveryVerdict, AnswerRouteOutcome, AnswerSubmission, HeldDeliveryPolicy, route_answer,
};

use crate::app::command_handlers::print_json_report;
use crate::app::dashboard::resolve_answer_trait_file;
use crate::app::frame_prompt::summons_question;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human, wire_name,
};
use crate::app::story;

pub(crate) struct AnswerInputs<'a> {
    pub(crate) session: &'a str,
    pub(crate) session_store: Option<&'a str>,
    pub(crate) file: Option<&'a str>,
    pub(crate) value: Option<&'a str>,
    pub(crate) value_json: Option<&'a str>,
    pub(crate) no_resume: bool,
    pub(crate) json: bool,
}

/// POSIX single-quote an operand for the printed follow-up command
/// (P0253.4 blocker 2): a bare `--value <text>` hint that pastes an
/// unquoted session id or path breaks the moment either contains a space or
/// shell metacharacter (a recovery ledger path is exactly the kind of
/// operand likely to). Every character outside a small always-safe set
/// forces quoting; safe operands (plain session ids, simple paths) are left
/// bare for readability.
fn shell_quote(value: &str) -> String {
    let is_safe = !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b':' | b'-')
        });
    if is_safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AnswerReport {
    pub(crate) session: String,
    /// The exact operand this invocation resolved with — reused verbatim in
    /// the printed follow-up command, since `session` (the session id) is
    /// not resolvable through `story::resolve_run` for every session source
    /// (e.g. an explicit, unindexed ledger path).
    pub(crate) answer_operand: String,
    /// `--file`/`--session-store` this invocation itself resolved with, so
    /// the printed follow-up command stays executable for a custom store or
    /// a recovery trait file rather than only the default resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer_session_store: Option<String>,
    pub(crate) question: String,
    pub(crate) answer_slot: String,
    pub(crate) schema: Option<String>,
    /// `false` for a bare `ctx traits answer <session>` (question shown,
    /// nothing submitted).
    pub(crate) submitted: bool,
    /// Present only when `submitted` and a resume was attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resumed_status: Option<String>,
    /// A pre-existing driver accepted the answer or won the replacement-drive
    /// race, so this invocation must not start another one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) driver_continues: bool,
}

pub(crate) fn handle_answer(
    inputs: AnswerInputs<'_>,
) -> crate::Result<CommandOutput<AnswerReport>> {
    let (ledger_path, session) = story::resolve_run(inputs.session, inputs.session_store)?;
    let display_id = session.session_id.as_str().to_string();
    // The operand this invocation itself resolved with — not `display_id`,
    // which is unresolvable through `story::resolve_run` for a session
    // reached only by an explicit ledger path (e.g. this proof's `run.json`,
    // never indexed by session id). Reusing the caller's own operand in the
    // printed follow-up command keeps it resolvable for every session
    // source (P0253.4 blocker 2).
    let answer_operand = inputs.session.to_string();
    let repo_root = current_utf8_dir()?;
    let trait_file = inputs
        .file
        .map(str::to_string)
        .or_else(|| resolve_answer_trait_file(&session, Some(repo_root.as_str())));
    // Derived state, not raw `Status::WaitingOnHuman` (P0253.4, mirrors the
    // TOCTOU re-check below and the dashboard's `SessionAction::Answer`
    // applier): `record_interrupted_outcome` never rewrites `status` away
    // from `WaitingOnHuman`, so a bare query against raw status would show a
    // cancelled question as if it were still live.
    let session_outcome = session.last_drive_outcome.as_ref().map(|o| &o.outcome);
    let session_state = SessionState::derive(&session.status, session_outcome, false);
    let frame = session
        .next_frame
        .as_ref()
        .filter(|frame| crate::app::frame_prompt::is_live_summons(&session, frame))
        .ok_or_else(|| crate::Error::Command {
            message: if session_state == SessionState::Cancelled {
                format!("{display_id}'s question was cancelled")
            } else {
                format!("{display_id} is not waiting for a summons answer")
            },
        })?;
    let output = frame
        .requested_outputs
        .first()
        .ok_or_else(|| crate::Error::Command {
            message: format!("{display_id}'s question has no answer slot"),
        })?;
    // Stored evidence first (P0253.4 blocker 1): a durable summons record
    // lets this inspection answer without paying for `load_trait_for_session`
    // at all — only a ledger parked before that field existed (or one whose
    // stored record no longer names the current frame) falls back to
    // resolving and re-rendering the trait.
    let question = match crate::app::frame_prompt::stored_summons_question(&session, frame) {
        Some(question) => question,
        None => {
            let loaded = ctx_traits_io::run::load_trait_for_session(
                trait_file.as_deref(),
                None,
                &session,
                "answer",
            )?;
            summons_question(&loaded, &session, frame)?
        }
    };
    let target = output.slot_ref.to_string();
    let schema_ref = output.schema_ref.clone();

    let raw_value = match (inputs.value, inputs.value_json) {
        (Some(_), Some(_)) => {
            return Err(crate::Error::Command {
                message: "pass only one of --value or --value-json".to_string(),
            });
        }
        (Some(text), None) => Some(text.to_string()),
        (None, Some(json_text)) => Some(json_text.to_string()),
        (None, None) => {
            return Ok(CommandOutput::new(print_and_return(
                inputs.json,
                AnswerReport {
                    session: display_id,
                    answer_operand,
                    answer_file: inputs.file.map(str::to_string),
                    answer_session_store: inputs.session_store.map(str::to_string),
                    question,
                    answer_slot: target,
                    schema: schema_ref,
                    submitted: false,
                    resumed_status: None,
                    driver_continues: false,
                },
            )?));
        }
    };
    let value = if inputs.value_json.is_some() {
        serde_json::from_str(&raw_value.expect("value-json branch set raw_value")).map_err(
            |error| crate::Error::Command {
                message: format!(
                    "--value-json is not valid JSON for {}: {error}",
                    schema_ref.as_deref().unwrap_or("schema:any")
                ),
            },
        )?
    } else {
        serde_json::Value::String(raw_value.expect("value branch set raw_value"))
    };

    let outcome = route_answer(
        session.session_id.as_str(),
        AnswerSubmission {
            ledger_path: &ledger_path,
            trait_file: trait_file.as_deref(),
            session_store: inputs.session_store,
            target: &target,
            schema_ref: schema_ref.as_deref(),
            expected_state_digest: session.state_digest.as_str(),
            value,
            caller: CallerProvenance {
                surface: "cli".to_string(),
                caller: "ctx traits answer".to_string(),
                agent: None,
                harness: None,
            },
            existing_input_evidence: "ctx traits answer",
            advance_command_frames: false,
        },
        if inputs.no_resume {
            HeldDeliveryPolicy::Refuse
        } else {
            HeldDeliveryPolicy::Allow
        },
    )?;
    let (response, driver_continues) = match outcome {
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::Accepted) => (None, true),
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::Stale) => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question changed; reopen it with `ctx traits answer {display_id}`"
                ),
            });
        }
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::Cancelled) => {
            return Err(crate::Error::Command {
                message: format!("answer refused: {display_id}'s question was cancelled"),
            });
        }
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::NotWaiting) => {
            return Err(crate::Error::Command {
                message: format!("answer refused: {display_id}'s driver is no longer waiting"),
            });
        }
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::RejectedCorrection { detail }) => {
            return Err(crate::Error::Command { message: detail });
        }
        AnswerRouteOutcome::Delivered(AnswerDeliveryVerdict::NotRouted) => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question did not route to its current frame"
                ),
            });
        }
        AnswerRouteOutcome::Submitted { response } => (Some(response), false),
        AnswerRouteOutcome::Cancelled => {
            return Err(crate::Error::Command {
                message: format!("answer refused: {display_id}'s question was cancelled"),
            });
        }
        AnswerRouteOutcome::Stale => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question changed; reopen it with `ctx traits answer {display_id}`"
                ),
            });
        }
        AnswerRouteOutcome::RejectedCorrection => {
            return Err(crate::Error::Command {
                message: "answer rejected; correct it and re-run `ctx traits answer`".to_string(),
            });
        }
        AnswerRouteOutcome::NotRouted => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question did not route to its current frame"
                ),
            });
        }
        AnswerRouteOutcome::HeldDeliveryRefused => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s live driver owns advancement; rerun without --no-resume"
                ),
            });
        }
        AnswerRouteOutcome::HolderUnverifiable => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s driver lock holder could not be verified"
                ),
            });
        }
        AnswerRouteOutcome::Undelivered => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s verified driver could not receive the answer"
                ),
            });
        }
        AnswerRouteOutcome::DriverAppeared => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s driver appeared while applying the answer; retry"
                ),
            });
        }
    };

    let resumed_status = if driver_continues || inputs.no_resume {
        None
    } else if response
        .as_ref()
        .expect("unheld route has response")
        .session
        .status
        == Status::Completed
    {
        Some(wire_name(
            &response
                .as_ref()
                .expect("unheld route has response")
                .session
                .status,
        ))
    } else {
        let panel_handoff = crate::app::drive::PanelHandoff::new();
        let report = crate::app::drive::drive(crate::app::drive::DriveInputs {
            file: trait_file.as_deref(),
            session: ledger_path.as_str(),
            session_store: inputs.session_store,
            assignments: &[],
            max_frames: None,
            frame_seconds: None,
            total_seconds: None,
            max_retries: None,
            attach_wait_seconds: None,
            idle_seconds: None,
            max_in_flight: 1,
            wait: false,
            progress: crate::app::drive::resolve_progress(None, inputs.json, false),
            worktree: None,
            execution_dir: None,
            clear_merge_intent: false,
            retain_panel_on_failure: false,
            panel_handoff: Some(panel_handoff),
            startup: None,
            frame_observer: None,
        })?;
        if report.status == "driver-lock-busy" {
            None
        } else {
            Some(report.status)
        }
    };
    let driver_continues = driver_continues || (!inputs.no_resume && resumed_status.is_none());

    print_and_return(
        inputs.json,
        AnswerReport {
            session: display_id,
            answer_operand,
            answer_file: inputs.file.map(str::to_string),
            answer_session_store: inputs.session_store.map(str::to_string),
            question,
            answer_slot: target,
            schema: schema_ref,
            submitted: true,
            resumed_status,
            driver_continues,
        },
    )
    .map(CommandOutput::new)
}

fn print_and_return(json: bool, report: AnswerReport) -> crate::Result<AnswerReport> {
    match OutputMode::select(json, false) {
        OutputMode::Json => print_json_report(&Envelope::ok(report.clone()), "answer")?,
        OutputMode::Human(mode) => {
            let panel = answer_report_panel(&report);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(report)
}

fn answer_report_panel(report: &AnswerReport) -> Panel {
    let status = if report.submitted {
        PanelStatus::Passed("answer accepted".to_string())
    } else {
        PanelStatus::Passed("awaiting answer".to_string())
    };
    let mut panel = Panel::new("ctx", "answer", status)
        .row(PanelRow::toned(
            "session",
            &report.session,
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "question",
            &report.question,
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "answer slot",
            &report.answer_slot,
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "schema",
            report.schema.as_deref().unwrap_or("schema:any"),
            RowTone::Default,
        ));
    panel = if report.submitted {
        match (&report.resumed_status, report.driver_continues) {
            (_, true) => panel.row(PanelRow::toned(
                "resumed",
                "driver continues",
                RowTone::Default,
            )),
            (Some(status), false) => {
                panel.row(PanelRow::toned("resumed to", status, RowTone::Default))
            }
            (None, false) => panel.row(PanelRow::toned("resumed", "no", RowTone::Default)),
        }
    } else {
        panel.row(PanelRow::toned(
            "submit with",
            format!("{}   # or --value-json <json>", submit_command_hint(report)),
            RowTone::Default,
        ))
    };
    panel
}

/// The exact, shell-safe `ctx traits answer` invocation that reproduces this
/// inspection (P0253.4 blocker 2): carries the same `--file`/
/// `--session-store` this invocation itself resolved with, so the hint stays
/// executable for a custom session store or an explicit recovery trait file,
/// not only the default resolution — and every operand is quoted, so a
/// session id or path containing a space or shell metacharacter still pastes
/// as one argument.
fn submit_command_hint(report: &AnswerReport) -> String {
    let mut command = format!("ctx traits answer {}", shell_quote(&report.answer_operand));
    if let Some(file) = &report.answer_file {
        command.push_str(&format!(" --file {}", shell_quote(file)));
    }
    if let Some(store) = &report.answer_session_store {
        command.push_str(&format!(" --session-store {}", shell_quote(store)));
    }
    command.push_str(" --value <text>");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_a_plain_operand_bare() {
        assert_eq!(shell_quote("run-2953-abcd"), "run-2953-abcd");
        assert_eq!(shell_quote("./run.json"), "./run.json");
        assert_eq!(shell_quote("recovery/session:one"), "recovery/session:one");
    }

    #[test]
    fn shell_quote_wraps_a_metacharacter_operand() {
        assert_eq!(shell_quote("has space"), "'has space'");
        assert_eq!(shell_quote("has'quote"), "'has'\\''quote'");
        assert_eq!(shell_quote(""), "''");
    }

    fn base_report() -> AnswerReport {
        AnswerReport {
            session: "session-1".to_string(),
            answer_operand: "run-1".to_string(),
            answer_file: None,
            answer_session_store: None,
            question: "what next?".to_string(),
            answer_slot: "slot:ask".to_string(),
            schema: None,
            submitted: false,
            resumed_status: None,
            driver_continues: false,
        }
    }

    #[test]
    fn submit_command_hint_omits_absent_file_and_store() {
        let report = base_report();
        assert_eq!(
            submit_command_hint(&report),
            "ctx traits answer run-1 --value <text>"
        );
    }

    #[test]
    fn submit_command_hint_carries_the_resolved_file_and_store() {
        let mut report = base_report();
        report.answer_file = Some("recovery/index.toml".to_string());
        report.answer_session_store = Some("my store".to_string());
        assert_eq!(
            submit_command_hint(&report),
            "ctx traits answer run-1 --file recovery/index.toml --session-store 'my store' --value <text>"
        );
    }
}
