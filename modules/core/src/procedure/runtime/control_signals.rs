// Procedure runtime control signals.
// Procedure runtime control signal handling.

fn emit_runtime_control_signal(
    state: &mut State,
    signal_ref: Option<&str>,
    sequence_index: usize,
    identity: Option<ControlEmissionIdentity>,
    position_path: Vec<PathSegment>,
) -> crate::Result<()> {
    emit_runtime_control_signal_at(state, signal_ref, sequence_index, position_path, identity)
}

fn emit_runtime_control_signal_at(
    state: &mut State,
    signal_ref: Option<&str>,
    sequence_index: usize,
    position_path: Vec<PathSegment>,
    identity: Option<ControlEmissionIdentity>,
) -> crate::Result<()> {
    let Some(signal_ref) = signal_ref else {
        return Ok(());
    };
    let loop_context = loop_context_from_stack(&state.control_stack);
    let for_each_context = for_each_context_from_stack(&state.control_stack);
    let runtime_control = identity;
    let evidence_seed = serde_json::json!({
        "signal-ref": signal_ref,
        "sequence-index": sequence_index,
        "position-path": position_path,
        "runtime-control": runtime_control,
        "loop-id": loop_context.as_ref().map(|context| context.loop_id.as_str()),
        "iteration-index": loop_context.as_ref().map(|context| context.iteration_index),
        "for-each-id": for_each_context.as_ref().map(|context| context.for_each_id.as_str()),
        "item-index": for_each_context.as_ref().map(|context| context.item_index),
    });
    let evidence_text = serde_json::to_string(&evidence_seed).map_err(|e| {
        crate::procedure::serialization("runtime-control.evidence", "runtime-control evidence", e)
    })?;
    let evidence_digest = Digest::source(&evidence_text);
    record_emitted_signals(
        state,
        std::iter::once(SignalEmission {
            signal_ref: Reference::parse(signal_ref)?,
            sequence_index,
            evidence_digest,
            position_path,
            source: Some(SignalSource::RuntimeControl),
            runtime_control,
            loop_id: loop_context.as_ref().map(|context| context.loop_id.clone()),
            iteration_index: loop_context.as_ref().map(|context| context.iteration_index),
            for_each_id: for_each_context
                .as_ref()
                .map(|context| context.for_each_id.clone()),
            item_index: for_each_context.as_ref().map(|context| context.item_index),
            producer_agent: None,
            producer_harness: None,
            acceptance: AcceptanceStatus::Accepted,
            reason: "runtime control signal emitted".to_string(),
        }),
    );
    Ok(())
}

fn runtime_control_identity_from_frame(frame: &ControlFrame) -> ControlEmissionIdentity {
    ControlEmissionIdentity {
        kind: frame.kind.clone(),
        parent_run_index: frame.parent_run_index,
        control_item_id: frame.control_item_id.clone(),
        sequence_id: frame.sequence_id.clone(),
        iteration_index: frame.iteration_index,
        item_index: frame.item_index,
    }
}

fn emit_enclosing_failure_signals(
    state: &mut State,
    failing_control_index: usize,
    position_path: &[PathSegment],
) -> crate::Result<()> {
    let enclosing: Vec<(Option<String>, usize, ControlEmissionIdentity)> = state
        .control_stack
        .iter()
        .take(failing_control_index)
        .rev()
        .map(|frame| {
            (
                frame.on_failure.as_ref().and_then(|target| match target {
                    FailureTarget::Signal(signal) => Some(signal.as_str()),
                    FailureTarget::Route(_) => None,
                }).map(str::to_string),
                frame.parent_run_index,
                runtime_control_identity_from_frame(frame),
            )
        })
        .collect();
    for (signal, parent_run_index, identity) in enclosing {
        emit_runtime_control_signal(
            state,
            signal.as_deref(),
            parent_run_index,
            Some(identity),
            position_path.to_vec(),
        )?;
    }
    Ok(())
}

fn emit_legacy_failure_signals_before_route(
    state: &mut State,
    position_path: &[PathSegment],
) -> crate::Result<()> {
    let depth = state.control_stack.len();
    emit_enclosing_failure_signals(state, depth, position_path)
}
