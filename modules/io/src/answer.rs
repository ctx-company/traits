//! Shared answer-submission transaction for CLI and dashboard adapters.

use camino::Utf8Path;
use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::{CallResponse, CallResponseKind, CallerProvenance};

/// Submission data for one answered Ask frame.
pub struct AnswerSubmission<'a> {
    pub ledger_path: &'a Utf8Path,
    pub trait_file: Option<&'a str>,
    pub session_store: Option<&'a str>,
    pub target: &'a str,
    pub schema_ref: Option<&'a str>,
    pub expected_state_digest: &'a str,
    pub value: serde_json::Value,
    pub caller: CallerProvenance,
    pub existing_input_evidence: &'static str,
    pub advance_command_frames: bool,
}

/// The answer data carried over the authenticated driver-control socket.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnswerEnvelope {
    pub target: String,
    pub schema_ref: Option<String>,
    pub expected_state_digest: String,
    pub value: serde_json::Value,
}

/// The result of delivering an answer to a waiting driver.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AnswerDeliveryVerdict {
    Accepted,
    Stale,
    Cancelled,
    NotWaiting,
    RejectedCorrection { detail: String },
    NotRouted,
}

/// Result of attempting an answer-submission transaction.
pub enum AnswerSubmissionOutcome {
    /// The lock was free, the re-check passed, and `run::set` accepted the
    /// value; `response` carries the resulting session/frame state.
    Submitted { response: Box<CallResponse> },
    /// The re-check found the question already resolved to `Cancelled`.
    Cancelled,
    /// The re-check found the question changed shape between read and lock.
    Stale,
    /// Another driver holds the maintenance lock.
    LockHeld,
    /// `run::set` accepted the write but rejected the value itself.
    RejectedCorrection,
    /// `run::set` routed the value to a session-level write rather than the
    /// expected call.
    NotRouted,
}

impl From<&AnswerSubmissionOutcome> for AnswerDeliveryVerdict {
    fn from(outcome: &AnswerSubmissionOutcome) -> Self {
        match outcome {
            AnswerSubmissionOutcome::Submitted { .. } => Self::Accepted,
            AnswerSubmissionOutcome::Cancelled => Self::Cancelled,
            AnswerSubmissionOutcome::Stale => Self::Stale,
            AnswerSubmissionOutcome::LockHeld => Self::NotWaiting,
            AnswerSubmissionOutcome::RejectedCorrection => Self::RejectedCorrection {
                detail: "answer rejected; correction required".to_string(),
            },
            AnswerSubmissionOutcome::NotRouted => Self::NotRouted,
        }
    }
}

/// The authoritative check for an Ask frame a caller may display or answer.
pub fn is_live_summons(
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> bool {
    if frame.kind != ctx_traits_core::procedure::runtime::SequenceFrameKind::Ask {
        return false;
    }
    let outcome = session.last_drive_outcome.as_ref().map(|o| &o.outcome);
    let state = SessionState::derive(&session.status, outcome, false);
    state == SessionState::WaitingOnHuman
        && matches!(
            outcome,
            Some(ctx_traits_core::procedure::session::DriveOutcomeKind::AwaitingOwner)
        )
}

/// Schema-aware text-vs-JSON parse shared by answer entry points.
pub fn parse_schema_aware_value(
    text: &str,
    schema_ref: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
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

/// Re-reads, validates, then applies one Ask answer without acquiring a lock.
///
/// Callers must hold the ledger's maintenance lock for the entire transaction.
pub fn apply_answer_transaction(
    input: AnswerSubmission<'_>,
) -> crate::Result<AnswerSubmissionOutcome> {
    let current = crate::run_session::read_run_session(input.ledger_path)?;
    let current_outcome = current.last_drive_outcome.as_ref().map(|o| &o.outcome);
    let current_state = SessionState::derive(&current.status, current_outcome, false);
    let valid = current.state_digest.as_str() == input.expected_state_digest
        && current.next_frame.as_ref().is_some_and(|frame| {
            is_live_summons(&current, frame)
                && frame.requested_outputs.first().is_some_and(|candidate| {
                    candidate.slot_ref.to_string() == input.target
                        && candidate.schema_ref.as_deref() == input.schema_ref
                })
        });
    if !valid {
        return Ok(if current_state == SessionState::Cancelled {
            AnswerSubmissionOutcome::Cancelled
        } else {
            AnswerSubmissionOutcome::Stale
        });
    }

    let response = match crate::run::set(crate::run::SetRequest {
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
    })? {
        crate::run::SetOutcome::Call { response, .. } => response,
        crate::run::SetOutcome::Session { .. } => return Ok(AnswerSubmissionOutcome::NotRouted),
    };
    if response.response_kind == CallResponseKind::RejectedCorrectionRequired {
        return Ok(AnswerSubmissionOutcome::RejectedCorrection);
    }
    Ok(AnswerSubmissionOutcome::Submitted { response })
}

/// Locks, re-reads, validates, then applies one Ask answer.
pub fn submit_answer(input: AnswerSubmission<'_>) -> crate::Result<AnswerSubmissionOutcome> {
    let Some(_maintenance) = crate::run_control::try_acquire_maintenance(input.ledger_path)? else {
        return Ok(AnswerSubmissionOutcome::LockHeld);
    };
    apply_answer_transaction(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::session::Status;

    fn awaiting_owner_session_fixture(
        run_id: &str,
    ) -> ctx_traits_core::procedure::session::Session {
        use ctx_traits_core::digest::Digest;
        use ctx_traits_core::procedure::runtime::{
            FinalState, FrameOutputRequest, SequenceFrame, SequenceFrameKind,
        };
        use ctx_traits_core::procedure::session::{DriveOutcome, DriveOutcomeKind, SummonsRecord};
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
                signal_emission_ceiling: 0,
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
                disk_full: None,
                tokens_by_model: None,
                summons: Some(SummonsRecord {
                    step_id: "ask-owner".to_string(),
                    title: "ask-owner".to_string(),
                    question: "What should I do next?".to_string(),
                    answer_slot: "slot:ask-owner".to_string(),
                    schema_ref: Some("schema:text".to_string()),
                }),
                reclaim: None,
                interruption_cause: None,
                interruption_position: None,
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

    #[test]
    fn submit_answer_refuses_a_stale_state_digest_without_writing() {
        let ledger_path = scratch_ledger_path_buf("stale-digest");
        let session = awaiting_owner_session_fixture("stale-digest-run");
        crate::run_session::write_run_session(&ledger_path, &session)
            .expect("write fixture session");
        let outcome = submit_answer(AnswerSubmission {
            ledger_path: &ledger_path, trait_file: None, session_store: None, target: "slot:ask-owner",
            schema_ref: Some("schema:text"),
            expected_state_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            value: serde_json::Value::String("do the thing".to_string()),
            caller: CallerProvenance { surface: "test".to_string(), caller: "answer-test".to_string(), agent: None, harness: None },
            existing_input_evidence: "answer-test", advance_command_frames: true,
        }).expect("submit_answer must not error on a stale re-check");
        assert!(matches!(outcome, AnswerSubmissionOutcome::Stale));
        let reread =
            crate::run_session::read_run_session(&ledger_path).expect("reread fixture session");
        assert!(reread.accepted_slot_values.is_empty());
        let _ = std::fs::remove_file(&ledger_path);
        let _ = std::fs::remove_file(crate::run_control::driver_lock_path(&ledger_path));
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

    #[test]
    fn answer_delivery_verdict_maps_submission_outcomes() {
        assert_eq!(
            AnswerDeliveryVerdict::from(&AnswerSubmissionOutcome::Cancelled),
            AnswerDeliveryVerdict::Cancelled
        );
        assert_eq!(
            AnswerDeliveryVerdict::from(&AnswerSubmissionOutcome::Stale),
            AnswerDeliveryVerdict::Stale
        );
        assert_eq!(
            AnswerDeliveryVerdict::from(&AnswerSubmissionOutcome::LockHeld),
            AnswerDeliveryVerdict::NotWaiting
        );
        assert_eq!(
            AnswerDeliveryVerdict::from(&AnswerSubmissionOutcome::RejectedCorrection),
            AnswerDeliveryVerdict::RejectedCorrection {
                detail: "answer rejected; correction required".to_string(),
            }
        );
        assert_eq!(
            AnswerDeliveryVerdict::from(&AnswerSubmissionOutcome::NotRouted),
            AnswerDeliveryVerdict::NotRouted
        );
    }

    #[test]
    fn answer_wire_types_round_trip_through_json() {
        let envelope = AnswerEnvelope {
            target: "slot:ask-owner".to_string(),
            schema_ref: Some("schema:object".to_string()),
            expected_state_digest: "sha256:digest".to_string(),
            value: serde_json::json!({"answer": true}),
        };
        assert_eq!(
            serde_json::from_str::<AnswerEnvelope>(
                &serde_json::to_string(&envelope).expect("serialize envelope")
            )
            .expect("deserialize envelope"),
            envelope
        );
    }
}
