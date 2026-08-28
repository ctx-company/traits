//! `ctx traits answer <session>` (0253.4): the foreground submission path
//! for a run parked `awaiting-owner` on an authored `ask` step. Shares the
//! same TOCTOU re-check, schema-aware parse, and `run::set` write the
//! dashboard's `SessionAction::Answer` applier uses
//! ([`crate::app::dashboard::resolve_answer_trait_file`],
//! [`resolved_human_question_body`]) — this module only adds the
//! foreground resume, since a dashboard-driven answer resumes through the
//! center's worker instead.

use camino::Utf8Path;
use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::{
    CallResponse, CallResponseKind, CallerProvenance, Status,
};
use ctx_traits_core::response::{CommandOutput, Envelope};
use serde::Serialize;

use crate::app::command_handlers::print_json_report;
use crate::app::dashboard::resolve_answer_trait_file;
use crate::app::frame_prompt::summons_question;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human, wire_name,
};
use crate::app::story;

/// The single answer-submission transaction (blocker 2, P0253.4): lock,
/// authoritative reread, derived-state/target/schema re-check, and
/// `run::set` write. Both the CLI verb and the dashboard's
/// `SessionAction::Answer` applier delegate here rather than each
/// re-implementing the TOCTOU protocol — the two copies had already
/// diverged before this extraction.
pub(crate) struct AnswerSubmission<'a> {
    pub(crate) ledger_path: &'a Utf8Path,
    pub(crate) trait_file: Option<&'a str>,
    pub(crate) session_store: Option<&'a str>,
    pub(crate) target: &'a str,
    pub(crate) schema_ref: Option<&'a str>,
    pub(crate) expected_state_digest: &'a str,
    pub(crate) value: serde_json::Value,
    pub(crate) caller: CallerProvenance,
    pub(crate) existing_input_evidence: &'static str,
    pub(crate) advance_command_frames: bool,
}

pub(crate) enum AnswerSubmissionOutcome {
    /// The lock was free, the re-check passed, and `run::set` accepted the
    /// value; `response` carries the resulting session/frame state.
    Submitted { response: Box<CallResponse> },
    /// The re-check found the question already resolved to `Cancelled`
    /// (center repair, interrupt, kill) between read and lock.
    Cancelled,
    /// The re-check found the question changed shape (different frame,
    /// slot, schema, or state digest) between read and lock.
    Stale,
    /// Another driver holds the maintenance lock.
    LockHeld,
    /// `run::set` accepted the write but rejected the value itself.
    RejectedCorrection,
    /// `run::set` routed the value to a session-level write rather than the
    /// expected call — the frame moved out from under the request.
    NotRouted,
}

/// Schema-aware text-vs-JSON parse, shared by the dashboard's single answer
/// text box (which must disambiguate by `schema_ref` alone) and reused here
/// so the two surfaces cannot drift on which schemas take a raw string
/// (P0253.4 blocker 2). Only `schema:text` takes the raw text verbatim; an
/// absent schema means `schema:any`, and — matching the dashboard's
/// pre-extraction behavior — is parsed as JSON like every other schema.
pub(crate) fn parse_schema_aware_value(
    text: &str,
    schema_ref: Option<&str>,
) -> Result<serde_json::Value, String> {
    if schema_ref == Some("schema:text") {
        Ok(serde_json::Value::String(text.to_string()))
    } else {
        serde_json::from_str(text).map_err(|error| {
            format!(
                "enter JSON for {}: {error}",
                schema_ref.unwrap_or("schema:any")
            )
        })
    }
}

pub(crate) fn submit_answer(input: AnswerSubmission<'_>) -> crate::Result<AnswerSubmissionOutcome> {
    let Some(maintenance) = ctx_traits_io::run_control::try_acquire_maintenance(input.ledger_path)?
    else {
        return Ok(AnswerSubmissionOutcome::LockHeld);
    };
    let current = ctx_traits_io::run_session::read_run_session(input.ledger_path)?;
    let current_outcome = current.last_drive_outcome.as_ref().map(|o| &o.outcome);
    let current_state = SessionState::derive(&current.status, current_outcome, false);
    let valid = current.state_digest.as_str() == input.expected_state_digest
        && current.next_frame.as_ref().is_some_and(|frame| {
            crate::app::frame_prompt::is_live_summons(&current, frame)
                && frame.requested_outputs.first().is_some_and(|candidate| {
                    candidate.slot_ref.to_string() == input.target
                        && candidate.schema_ref.as_deref() == input.schema_ref
                })
        });
    if !valid {
        drop(maintenance);
        return Ok(if current_state == SessionState::Cancelled {
            AnswerSubmissionOutcome::Cancelled
        } else {
            AnswerSubmissionOutcome::Stale
        });
    }

    let result = ctx_traits_io::run::set(ctx_traits_io::run::SetRequest {
        trait_file: input.trait_file,
        trait_id: None,
        session: input.ledger_path.as_str(),
        session_store: input.session_store,
        target: input.target,
        value: input.value,
        out: None,
        caller: input.caller,
        existing_input_evidence: input.existing_input_evidence,
        advance_command_frames: input.advance_command_frames,
    });
    drop(maintenance);
    let response = match result? {
        ctx_traits_io::run::SetOutcome::Call { response, .. } => response,
        ctx_traits_io::run::SetOutcome::Session { .. } => {
            return Ok(AnswerSubmissionOutcome::NotRouted);
        }
    };
    if response.response_kind == CallResponseKind::RejectedCorrectionRequired {
        return Ok(AnswerSubmissionOutcome::RejectedCorrection);
    }
    Ok(AnswerSubmissionOutcome::Submitted { response })
}

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

    let outcome = submit_answer(AnswerSubmission {
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
        advance_command_frames: !inputs.no_resume,
    })?;
    let response = match outcome {
        AnswerSubmissionOutcome::Submitted { response } => response,
        AnswerSubmissionOutcome::LockHeld => {
            return Err(crate::Error::Command {
                message: format!("answer refused: {display_id}'s driver lock is held"),
            });
        }
        AnswerSubmissionOutcome::Cancelled => {
            return Err(crate::Error::Command {
                message: format!("answer refused: {display_id}'s question was cancelled"),
            });
        }
        AnswerSubmissionOutcome::Stale => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question changed; reopen it with `ctx traits answer {display_id}`"
                ),
            });
        }
        AnswerSubmissionOutcome::NotRouted => {
            return Err(crate::Error::Command {
                message: format!(
                    "answer refused: {display_id}'s question did not route to its current frame"
                ),
            });
        }
        AnswerSubmissionOutcome::RejectedCorrection => {
            return Err(crate::Error::Command {
                message: "answer rejected; correct it and re-run `ctx traits answer`".to_string(),
            });
        }
    };

    let resumed_status = if inputs.no_resume {
        None
    } else if response.session.status == Status::Completed {
        Some(wire_name(&response.session.status))
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
        Some(report.status)
    };

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
        match &report.resumed_status {
            Some(status) => panel.row(PanelRow::toned("resumed to", status, RowTone::Default)),
            None => panel.row(PanelRow::toned("resumed", "no", RowTone::Default)),
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

    /// A live, on-disk `awaiting-owner` ledger with a still-current Ask
    /// frame — the shape [`submit_answer`]'s TOCTOU re-check must accept
    /// when the caller's `expected_state_digest` still matches.
    fn awaiting_owner_session_fixture(
        run_id: &str,
    ) -> ctx_traits_core::procedure::session::Session {
        use ctx_traits_core::digest::Digest;
        use ctx_traits_core::procedure::runtime::FinalState;
        use ctx_traits_core::procedure::runtime::FrameOutputRequest;
        use ctx_traits_core::procedure::runtime::SequenceFrame;
        use ctx_traits_core::procedure::runtime::SequenceFrameKind;
        use ctx_traits_core::procedure::session::DriveOutcome;
        use ctx_traits_core::procedure::session::DriveOutcomeKind;
        use ctx_traits_core::procedure::session::SummonsRecord;
        use ctx_traits_core::reference::Reference;

        ctx_traits_core::procedure::session::Session {
            schema_version: "1".to_string(),
            session_id: ctx_traits_core::procedure::session::SessionId::new(format!(
                "session-{run_id}"
            ))
            .expect("session id"),
            run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string()).expect("run id"),
            trait_id: "test-trait".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: Status::WaitingOnHuman,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            resolved_settings: Vec::new(),
            resolved_budgets: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: Some(Box::new(SequenceFrame {
                kind: SequenceFrameKind::Ask,
                run_id: run_id.to_string(),
                trait_id: "test-trait".to_string(),
                sequence_index: Some(0),
                run_index: Some(0),
                item_id: Some("ask-owner".to_string()),
                position_path: Vec::new(),
                loop_context: None,
                for_each_context: None,
                guard_explanations: Vec::new(),
                signal_payloads: Vec::new(),
                title: "ask-owner".to_string(),
                frame_text: String::new(),
                prompt: None,
                command: None,
                available_inputs: Vec::new(),
                resource_evidence: Vec::new(),
                requested_outputs: vec![FrameOutputRequest {
                    slot_ref: Reference::parse("slot:ask-owner").expect("slot ref parses"),
                    operation: Default::default(),
                    schema_ref: Some("schema:text".to_string()),
                    optional: false,
                }],
                assigned_agent: None,
                allowed_signals: Vec::new(),
                derived_signals: Vec::new(),
                call_template: None,
                warnings: Vec::new(),
            })),
            last_validation_report: None,
            completion: None,
            last_drive_outcome: Some(DriveOutcome {
                outcome: DriveOutcomeKind::AwaitingOwner,
                recorded_at_epoch: 0,
                provider_credits_pause: None,
                effective_budget: None,
                token_usage: None,
                exit_code: None,
                rate_limit: None,
                budget_pause: None,
                tokens_by_model: None,
                summons: Some(SummonsRecord {
                    step_id: "ask-owner".to_string(),
                    title: "ask-owner".to_string(),
                    question: "What should I do next?".to_string(),
                    answer_slot: "slot:ask-owner".to_string(),
                    schema_ref: Some("schema:text".to_string()),
                }),
            }),
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: CallerProvenance {
                    surface: "test".to_string(),
                    caller: "answer-test".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
            ledger: ctx_traits_core::procedure::runtime::State {
                run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string())
                    .expect("run id"),
                trait_id: "test-trait".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                resolved_settings: Vec::new(),
                resolved_budgets: Vec::new(),
                active_path: Vec::new(),
                control_stack: Vec::new(),
                branch_decisions: Vec::new(),
                conditional_input_decisions: Vec::new(),
                ask_decisions: Vec::new(),
                failure_routes: Vec::new(),
                guard_evaluations: Vec::new(),
                parallel_panel_records: Vec::new(),
                stop_reason: None,
                elapsed_seconds: 0,
                final_state: FinalState::Running,
            },
            state_digest: Digest::source("answer-toctou-live-digest"),
        }
    }

    fn scratch_ledger_path_buf(name: &str) -> camino::Utf8PathBuf {
        camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-answer-toctou-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ))
    }

    /// P0253.4 blocker 1 (shared-answer-submission-semantics): the shared
    /// submission transaction refuses a stale re-check without touching the
    /// ledger, distinct from the `Cancelled` branch already exercised by
    /// `proof_summons.rs`'s `answer_refuses_a_cancelled_summons` — this
    /// keeps the authoritative ledger in a live `awaiting-owner` state and
    /// supplies an obsolete `expected_state_digest`, so the frame/slot/schema
    /// still match but the digest does not.
    #[test]
    fn submit_answer_refuses_a_stale_state_digest_without_writing() {
        let ledger_path = scratch_ledger_path_buf("stale-digest");
        let session = awaiting_owner_session_fixture("stale-digest-run");
        ctx_traits_io::run_session::write_run_session(&ledger_path, &session)
            .expect("write fixture session");

        let outcome = submit_answer(AnswerSubmission {
            ledger_path: &ledger_path,
            trait_file: None,
            session_store: None,
            target: "slot:ask-owner",
            schema_ref: Some("schema:text"),
            expected_state_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            value: serde_json::Value::String("do the thing".to_string()),
            caller: CallerProvenance {
                surface: "test".to_string(),
                caller: "answer-test".to_string(),
                agent: None,
                harness: None,
            },
            existing_input_evidence: "answer-test",
            advance_command_frames: true,
        })
        .expect("submit_answer must not error on a stale re-check");

        assert!(
            matches!(outcome, AnswerSubmissionOutcome::Stale),
            "expected Stale for an obsolete expected_state_digest against a live summons"
        );

        let reread = ctx_traits_io::run_session::read_run_session(&ledger_path)
            .expect("reread fixture session");
        assert!(
            reread.accepted_slot_values.is_empty(),
            "a stale re-check must never write the value: {:?}",
            reread.accepted_slot_values
        );

        let _ = std::fs::remove_file(&ledger_path);
        let _ = std::fs::remove_file(ctx_traits_io::run_control::driver_lock_path(&ledger_path));
    }

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

    #[test]
    fn parse_schema_aware_value_takes_raw_text_only_for_text_schema() {
        assert_eq!(
            parse_schema_aware_value("do the thing", Some("schema:text")),
            Ok(serde_json::Value::String("do the thing".to_string()))
        );
    }

    #[test]
    fn parse_schema_aware_value_parses_json_for_any_other_schema() {
        assert_eq!(
            parse_schema_aware_value("{\"a\":1}", Some("schema:object")),
            Ok(serde_json::json!({"a": 1}))
        );
        assert!(parse_schema_aware_value("not json", Some("schema:object")).is_err());
    }

    /// Regression (P0253.4 blocker 2): a missing schema means `schema:any`,
    /// same as the dashboard's pre-extraction behavior — it must parse as
    /// JSON, not be taken as a raw string the way `schema:text` is.
    #[test]
    fn parse_schema_aware_value_parses_json_for_no_schema() {
        assert_eq!(
            parse_schema_aware_value("{\"a\":1}", None),
            Ok(serde_json::json!({"a": 1}))
        );
        assert_eq!(
            parse_schema_aware_value("\"quoted string\"", None),
            Ok(serde_json::Value::String("quoted string".to_string()))
        );
        assert!(parse_schema_aware_value("not json", None).is_err());
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
