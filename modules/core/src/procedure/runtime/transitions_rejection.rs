// Procedure runtime rejected transitions.
// Procedure runtime transition rejection.

fn reject_step_output(
    trait_ref: &Trait,
    mut state: State,
    sequence_index: usize,
    mut report: StepValidationReport,
) -> crate::Result<(State, StepValidationReport)> {
    let rejection_path = state.active_path.clone();
    state.rejected_attempts.extend(
        report
            .rejected_outputs
            .iter()
            .cloned()
            .map(|mut attempt| {
                if attempt.position_path.is_empty() && !rejection_path.is_empty() {
                    attempt.position_path = rejection_path.clone();
                }
                attempt
            }),
    );
    for signal in report
        .signal_validation
        .iter()
        .filter(|signal| signal.acceptance == AcceptanceStatus::Rejected)
    {
        state.rejected_attempts.push(RejectedAttempt {
            sequence_index,
            position_path: rejection_path.clone(),
            ref_text: Some(signal.signal_ref.to_string()),
            value_digest: Some(signal.evidence_digest.clone()),
            reason: signal.reason.clone(),
        });
    }
    for missing in &report.missing_required_outputs {
        state.rejected_attempts.push(RejectedAttempt {
            sequence_index,
            position_path: rejection_path.clone(),
            ref_text: Some(missing.clone()),
            value_digest: None,
            reason: "required declared slot output was not supplied".to_string(),
        });
    }
    let sequence = effective_sequence_items(procedure(trait_ref)?)?;
    let source_run_index = if state.control_stack.is_empty() {
        sequence
            .iter()
            .find(|item| item.declaration_index == sequence_index)
            .map(|item| item.run_index)
    } else {
        Some(state.current_run_index)
    };
    let nested_source = state.control_stack.last().and_then(|frame| {
        trait_ref
            .sequences
            .get(&frame.sequence_id)
            .and_then(|named| named.sequence.get(sequence_index))
    }).cloned();
    let outer_source = source_run_index
        .and_then(|run_index| sequence.iter().find(|item| item.run_index == run_index));
    let failing_item = nested_source.as_ref().or_else(|| outer_source.map(|item| item.item));
    emit_runtime_control_signal_at(
        &mut state,
        failing_item.and_then(|item| match item.on_failure.as_ref() {
            Some(FailureTarget::Signal(signal)) => Some(signal.as_str()),
            Some(FailureTarget::Route(_)) | None => None,
        }),
        sequence_index,
        rejection_path.clone(),
        None,
    )?;
    let direct_route = if let (Some(run_index), Some(item)) = (source_run_index, nested_source.as_ref()) {
        route_failure(
            trait_ref,
            &mut state,
            run_index,
            item.id.as_deref(),
            item.on_failure.as_ref(),
            rejection_path.clone(),
        )?
    } else if let Some(source) = outer_source {
        route_failure(
            trait_ref,
            &mut state,
            source.run_index,
            source.item.id.as_deref(),
            source.item.on_failure.as_ref(),
            rejection_path.clone(),
        )?
    } else {
        false
    };
    let enclosing_route = if !direct_route && !state.control_stack.is_empty() {
        let depth = state.control_stack.len();
        route_enclosing_failure(trait_ref, &mut state, depth, &rejection_path)?
    } else {
        false
    };
    if direct_route || enclosing_route {
        report.next_action = match state.final_state {
            FinalState::Running => StepNextAction::Continue,
            FinalState::Blocked => StepNextAction::Blocked,
            FinalState::Completed => StepNextAction::Completed,
            FinalState::Rejected => StepNextAction::Rejected,
            FinalState::Failed => StepNextAction::Failed,
        };
        // The failed attempt is retained in the ledger, but the routed
        // candidate is valid to advance rather than request correction.
        report.rejected_outputs.clear();
        // A route abandons the entire envelope's effects, not just the
        // output that triggered it (route_failure drops the whole active
        // `parallel` branch buffer, or the whole run for a non-Parallel
        // route). Any other output/signal this same submission produced —
        // schema-valid or not — is discarded along with it, so the report
        // must not keep claiming it as accepted; only the route's own
        // durable evidence (recorded directly by `route_failure`, never
        // through this report) survives.
        report.accepted_outputs.clear();
        report
            .signal_validation
            .retain(|signal| signal.acceptance != AcceptanceStatus::Accepted);
        state.output_ports = finalize_outputs(trait_ref, &state)?;
        sort_state(&mut state);
        return Ok((state, report));
    }
    state.final_state = FinalState::Rejected;
    if state.control_stack.is_empty() {
        set_sequence_status(
            &mut state,
            sequence_index,
            SequenceStatusKind::Rejected,
            "step output rejected",
        );
    } else {
        let current_run_index = state.current_run_index;
        stop_with_reason(
            &mut state,
            FinalState::Rejected,
            STOP_NESTED_SEQUENCE_FAILED,
            rejection_path.clone(),
            None,
        );
        let control_depth = state.control_stack.len();
        emit_enclosing_failure_signals(&mut state, control_depth, &rejection_path)?;
        set_path_sequence_status(
            &mut state,
            SequenceStatus {
                sequence_index,
                run_index: current_run_index,
                item_id: nested_source.as_ref().and_then(|item| item.id.clone()),
                title: nested_source
                    .as_ref()
                    .map_or_else(|| "nested item".to_string(), |item| item.title.clone()),
                status: SequenceStatusKind::Rejected,
                reason: format!(
                    "nested step output rejected at {}",
                    format_path(&rejection_path)
                ),
                position_path: rejection_path.clone(),
            },
        );
    }
    report.next_action = StepNextAction::Rejected;
    state.output_ports = finalize_outputs(trait_ref, &state)?;
    sort_state(&mut state);
    Ok((state, report))
}
