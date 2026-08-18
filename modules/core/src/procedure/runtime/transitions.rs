// Procedure runtime state transitions.
// Procedure runtime state transitions.

// ---------------------------------------------------------------------------
// Public transitions
// ---------------------------------------------------------------------------

/// Start an executable procedure run from caller-supplied input values.
#[allow(clippy::too_many_arguments)]
pub fn start_procedure_run(
    trait_ref: &Trait,
    run_id: Id,
    initial_port_values: Vec<StepSlotOutput>,
    resource_evidence: Vec<ResourceEvidence>,
    provider_capability_reports: Vec<CapabilityReport>,
    source_digest: Option<Digest>,
    canonical_digest: Option<Digest>,
    resolved_settings: Vec<ResolvedSettingRecord>,
    resolved_budgets: Vec<ResolvedBudgetRecord>,
) -> crate::Result<State> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    let mut state = State {
        run_id,
        trait_id: trait_ref.id.as_str().to_string(),
        // Lenient by default; the session layer applies the caller's override.
        strict_loops: false,
        source_digest,
        canonical_digest,
        current_run_index: 0,
        sequence_statuses: sequence
            .iter()
            .map(|item| SequenceStatus {
                sequence_index: item.declaration_index,
                run_index: item.run_index,
                item_id: item.item.id.clone(),
                title: item.item.title.clone(),
                status: SequenceStatusKind::Pending,
                reason: "not reached".to_string(),
                position_path: Vec::new(),
            })
            .collect(),
        accepted_port_values: Vec::new(),
        accepted_slot_values: Vec::new(),
        accepted_output_port_values: Vec::new(),
        slot_revisions: Vec::new(),
        resource_evidence,
        emitted_signals: Vec::new(),
        rejected_attempts: Vec::new(),
        provider_capability_reports,
        output_ports: Vec::new(),
        resolved_settings,
        resolved_budgets,
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
    };

    for value in initial_port_values {
        let accepted = validate_initial_port_value(trait_ref, value)?;
        match accepted.acceptance {
            AcceptanceStatus::Accepted => state.accepted_port_values.push(accepted),
            AcceptanceStatus::Rejected => state.rejected_attempts.push(RejectedAttempt {
                sequence_index: 0,
                position_path: Vec::new(),
                ref_text: Some(accepted.ref_text.clone()),
                value_digest: Some(accepted.value_digest.clone()),
                reason: accepted.schema_validation.first().map_or_else(
                    || "initial port value rejected".to_string(),
                    |v| v.reason.clone(),
                ),
            }),
        }
    }

    refresh_runtime_status(trait_ref, &mut state)?;
    Ok(state)
}

/// Return the intro frame for a newly started run.
pub fn intro_sequence_frame(trait_ref: &Trait, state: &State) -> crate::Result<SequenceFrame> {
    let proc = procedure(trait_ref)?;
    let mut warnings = Vec::new();
    if state.final_state == FinalState::Blocked {
        warnings.push("run is blocked before first step".to_string());
    }
    let mut text = format!(
        "Model: {}\nTrait: {}\nRun: {}\n",
        proc.description,
        state.trait_id,
        state.run_id.as_str()
    );
    text.push_str("Inputs accepted:\n");
    for value in &state.accepted_port_values {
        text.push_str(&format!("- {} ({})\n", value.ref_text, value.value_digest));
    }
    if !trait_ref.agents.is_empty() {
        text.push_str("Declared agent roles (advisory until runtime assignments are supplied):\n");
        for agent in &trait_ref.agents {
            text.push_str(&format!("- agent:{}: {}\n", agent.id, agent.description));
        }
    }

    Ok(SequenceFrame {
        kind: SequenceFrameKind::Intro,
        run_id: state.run_id.as_str().to_string(),
        trait_id: state.trait_id.clone(),
        sequence_index: None,
        run_index: None,
        item_id: None,
        position_path: Vec::new(),
        loop_context: None,
        for_each_context: None,
        guard_explanations: Vec::new(),
        title: proc.description.clone(),
        frame_text: bounded(text),
        prompt: None,
        command: None,
        available_inputs: state
            .accepted_port_values
            .iter()
            .map(|value| frame_input_from_value(trait_ref, value))
            .collect(),
        resource_evidence: state.resource_evidence.clone(),
        requested_outputs: Vec::new(),
        assigned_agent: None,
        allowed_signals: Vec::new(),
        derived_signals: Vec::new(),
        call_template: None,
        warnings,
    })
}

/// Build the next sequence frame or an explicit blocked/completed result.
pub fn next_sequence_frame(
    trait_ref: &Trait,
    state: &State,
) -> crate::Result<NextSequenceFrameResult> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    let ready = match runtime_readiness(trait_ref, state, &sequence)? {
        Readiness::Ready(ready) => ready,
        Readiness::Blocked {
            missing_inputs,
            capabilities,
        } => {
            return Ok(NextSequenceFrameResult::Blocked {
                missing_inputs,
                capabilities,
            });
        }
        Readiness::Completed => return Ok(NextSequenceFrameResult::Completed),
        Readiness::Rejected => return Ok(NextSequenceFrameResult::Rejected),
        Readiness::Failed => return Ok(NextSequenceFrameResult::Failed),
    };

    Ok(NextSequenceFrameResult::Frame(Box::new(build_sequence_frame(
        trait_ref, state, &ready,
    )?)))
}

/// Build the sequence frame for an already-resolved ready item. Shared by
/// [`next_sequence_frame`] (live control-flow-resolved position) and the
/// preview projector (synthetic declaration-order position), so frame
/// content, ordering, and resolution behavior stay identical between the two
/// callers.
fn build_sequence_frame(
    trait_ref: &Trait,
    state: &State,
    ready: &ReadyItem<'_>,
) -> crate::Result<SequenceFrame> {
    let command_plan = command_plan_for_item(
        ready.item,
        &format!("procedure.sequence[{}]", ready.sequence_index),
    )?;
    let command = command_plan
        .as_ref()
        .map(|plan| command_frame(ready.item, plan, state))
        .transpose()?;
    let prompt = if command.is_some() {
        None
    } else {
        prompt_evidence(trait_ref, ready.item)?
    };
    let assigned_agent = if ready.item.effective_kind() == SequenceKind::Ask {
        None
    } else {
        assigned_agent_role(
            trait_ref,
            ready.item.agent.as_deref(),
            &ready.position_path,
            ready.sequence_index,
        )?
    };
    let mut frame_text = format!(
        "Step [run {} / source {}]: {}\n",
        ready.run_index, ready.sequence_index, ready.item.title
    );
    if let Some(loop_context) = ready.loop_context.as_ref() {
        if loop_context.max_iterations == usize::MAX {
            frame_text.push_str(&format!(
                "Loop {} iteration {} (unbounded — exits on its own guard)\n",
                loop_context.loop_id,
                loop_context.iteration_index + 1,
            ));
        } else {
            frame_text.push_str(&format!(
                "Loop {} iteration {}/{}\n",
                loop_context.loop_id,
                loop_context.iteration_index + 1,
                loop_context.max_iterations
            ));
        }
    }
    if let Some(for_each_context) = ready.for_each_context.as_ref() {
        frame_text.push_str(&format!(
            "For-each {} item {}/{} (max {})\n",
            for_each_context.for_each_id,
            for_each_context.item_index + 1,
            for_each_context.item_total,
            for_each_context.max_items
        ));
    }
    if let Some(agent) = assigned_agent.as_ref() {
        frame_text.push_str(&format!(
            "Assigned agent: {} ({})\n",
            agent.ref_text, agent.description
        ));
    }
    for guard in &ready.guard_explanations {
        frame_text.push_str(&format!(
            "Guard {} => {} ({})\n",
            guard.predicate, guard.matched, guard.reason
        ));
    }
    if let Some(ref command) = command {
        let is_check = ready.item.effective_kind() == SequenceKind::Check;
        frame_text.push_str(if is_check {
            "Check step: runtime-only; static hosts cannot execute it.\n"
        } else {
            "Command step: runtime-only; static hosts cannot execute it.\n"
        });
        frame_text.push_str(&format!("Command argv: {}\n", command.argv.join(" ")));
        frame_text.push_str(&format!("Output slot: {}\n", command.output_slot));
        frame_text.push_str("Status: blocked-command-permission-required unless the adapter has explicit approval.\n");
    } else if let Some(ref prompt_evidence) = prompt {
        frame_text.push_str(&format!("Prompt digest: {}\n", prompt_evidence.digest));
    }
    let active_inputs = active_input_refs(trait_ref, state, ready)?;
    frame_text.push_str("Available inputs:\n");
    for input in &active_inputs {
        if let Some(value) = accepted_value(state, input) {
            frame_text.push_str(&format!("- {} ({})\n", input, value.value_digest));
        } else if let Some(resource) = accepted_resource(state, input) {
            frame_text.push_str(&format!(
                "- {} ({})\n",
                input,
                resource.digest.as_deref().map_or("no digest", |s| s)
            ));
        } else if is_soft_local_slot_ref(input) {
            frame_text.push_str(&format!("{}: (absent — no accepted value yet)\n", input));
        }
    }
    frame_text.push_str("Requested outputs:\n");
    for output in ready.item.output.iter() {
        let operation = match output.operation() {
            WriteOperation::Replace => "replace",
            WriteOperation::Append => "append",
            WriteOperation::Merge => "merge",
            WriteOperation::SetField(_) => "set-field",
            WriteOperation::Increment => "increment",
        };
        frame_text.push_str(&format!("- {} ({operation})\n", output.ref_text()));
    }

    let mut direct_signals = Vec::new();
    let mut derived_signals = Vec::new();
    for emit in &ready.item.on_complete {
        if let Some(when) = emit.when() {
            derived_signals.push(FrameDerivedSignal {
                signal_ref: Reference::parse(emit.signal_ref())?,
                when: when.clone(),
            });
        } else {
            direct_signals.push(emit.signal_ref().to_string());
        }
    }
    if !derived_signals.is_empty() {
        frame_text.push_str("Derived signals:\n");
        for derived in &derived_signals {
            frame_text.push_str(&format!(
                "- {} (runtime-derived when guard matches)\n",
                derived.signal_ref
            ));
        }
    }

    Ok(SequenceFrame {
        kind: if command.is_some() {
            if ready.item.effective_kind() == SequenceKind::Check {
                SequenceFrameKind::Check
            } else {
                SequenceFrameKind::Command
            }
        } else if ready.item.effective_kind() == SequenceKind::Ask {
            SequenceFrameKind::Ask
        } else {
            SequenceFrameKind::Step
        },
        run_id: state.run_id.as_str().to_string(),
        trait_id: state.trait_id.clone(),
        sequence_index: Some(ready.sequence_index),
        run_index: Some(ready.run_index),
        item_id: ready.item.id.clone(),
        position_path: ready.position_path.clone(),
        loop_context: ready.loop_context.clone(),
        for_each_context: ready.for_each_context.clone(),
        guard_explanations: ready.guard_explanations.clone(),
        title: ready.item.title.clone(),
        frame_text: bounded(frame_text),
        prompt,
        command,
        available_inputs: active_inputs
            .iter()
            .filter_map(|ref_text| {
                accepted_value(state, ref_text).map(|value| frame_input_from_value(trait_ref, value))
            })
            .collect(),
        resource_evidence: active_inputs
            .iter()
            .filter_map(|ref_text| accepted_resource(state, ref_text).cloned())
            .collect(),
        requested_outputs: ready
            .item
            .output
            .iter()
            .map(|sink| {
                Ok(FrameOutputRequest {
                    slot_ref: Reference::parse(sink.ref_text())?,
                    operation: sink.operation().clone(),
                    schema_ref: output_sink_schema_ref(trait_ref, sink)?,
                    optional: sink.is_optional(),
                })
            })
            .collect::<crate::Result<_>>()?,
        assigned_agent,
        allowed_signals: direct_signals,
        derived_signals,
        call_template: None,
        warnings: Vec::new(),
    })
}

/// The step's active input refs: every ordinary input, plus every
/// guard-conditioned resource input whose guard matches against `state`
/// right now. A false guard omits the resource entirely rather than blocking
/// the step (P290). Shared by frame text, `available_inputs`, and
/// `resource_evidence` construction so no one surface can include a resource
/// another surface omits, and by the mutating readiness settle point that
/// records the same decision into the guard-evaluation ledger — both derive
/// it from the same pure evaluation of the same accepted state, so
/// speculative (preview/lookahead) and authoritative frames cannot diverge.
fn active_input_refs<'a>(
    trait_ref: &Trait,
    state: &State,
    ready: &ReadyItem<'a>,
) -> crate::Result<Vec<&'a str>> {
    let context = guard_context_from_stack(&state.control_stack).unwrap_or(LoopContext {
        loop_id: String::new(),
        sequence_id: None,
        iteration_index: 0,
        max_iterations: 1,
    });
    let position_path = producer_path_for_ready(ready);
    active_item_input_refs(trait_ref, state, ready.item, &context, &position_path)
}

/// The item's active input refs (unconditional inputs plus every
/// guard-conditioned resource input whose guard matches `state` at
/// `position_path`). Shared by [`active_input_refs`] (live/preview frame
/// construction) and the readiness missing-input check, so a matched
/// guarded resource is promoted into the hard-input set in exactly the
/// surfaces that decide inclusion — a resource can never be presented in a
/// frame that readiness did not also require (P290).
fn active_item_input_refs<'a>(
    trait_ref: &Trait,
    state: &State,
    item: &'a crate::r#trait::procedure::SequenceItem,
    context: &LoopContext,
    position_path: &[PathSegment],
) -> crate::Result<Vec<&'a str>> {
    let mut active = Vec::new();
    for input in item.input.iter() {
        let include = match input.guard() {
            None => true,
            Some(guard) => {
                evaluate_guard_expr(trait_ref, state, guard, context, position_path, &[])?.0
            }
        };
        let include =
            include && (!input.is_optional() || accepted_value(state, input.ref_text()).is_some());
        if include {
            active.push(input.ref_text());
        }
    }
    Ok(active)
}

/// Apply caller/model output for the current sequence item.
pub fn apply_step_output(
    trait_ref: &Trait,
    mut state: State,
    envelope: StepOutputEnvelope,
) -> crate::Result<(State, StepValidationReport)> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    let report_sequence_index = current_ready_item(trait_ref, &state, &sequence)?
        .map(|ready| ready.sequence_index)
        .or_else(|| {
            sequence
                .iter()
                .find(|item| item.run_index == state.current_run_index)
                .map(|item| item.declaration_index)
        })
        .unwrap_or(0);

    let mut report = StepValidationReport {
        sequence_index: report_sequence_index,
        accepted_outputs: Vec::new(),
        rejected_outputs: Vec::new(),
        missing_required_outputs: Vec::new(),
        unfilled_optional_outputs: Vec::new(),
        unexpected_outputs: Vec::new(),
        schema_validation: Vec::new(),
        signal_validation: Vec::new(),
        warnings: envelope.warnings,
        next_action: StepNextAction::Continue,
    };

    let ready = match runtime_readiness(trait_ref, &state, &sequence)? {
        Readiness::Ready(ready) => ready,
        Readiness::Blocked {
            missing_inputs,
            capabilities,
        } => {
            report.next_action = StepNextAction::Blocked;
            if missing_inputs.is_empty() {
                report.warnings.push("run is blocked".to_string());
            } else {
                report.warnings.push(format!(
                    "run is blocked; missing input(s): {}",
                    missing_inputs.join(", ")
                ));
            }
            for capability in capabilities {
                report.warnings.push(format!(
                    "capability {}: {}",
                    capability.capability,
                    capability
                        .reason
                        .as_deref()
                        .map_or("unsupported", |reason| reason)
                ));
            }
            return Ok((state, report));
        }
        Readiness::Completed => {
            report.next_action = StepNextAction::Completed;
            report
                .warnings
                .push("no current sequence item; run is already complete".to_string());
            return Ok((state, report));
        }
        Readiness::Rejected => {
            report.next_action = StepNextAction::Rejected;
            report
                .warnings
                .push("run is rejected; step output was not applied".to_string());
            return Ok((state, report));
        }
        Readiness::Failed => {
            report.next_action = StepNextAction::Failed;
            report
                .warnings
                .push("run is failed; step output was not applied".to_string());
            return Ok((state, report));
        }
    };
    report.sequence_index = ready.sequence_index;

    if let Some(sequence_index) = envelope.sequence_index
        && sequence_index != ready.sequence_index {
            reject_envelope(
                &mut report,
                ready.sequence_index,
                format!(
                    "step output sequence-index {sequence_index} does not match current sequence index {}",
                    ready.sequence_index
                ),
            );
        }
    if let Some(ref item_id) = envelope.item_id
        && ready.item.id.as_deref() != Some(item_id.as_str()) {
            reject_envelope(
                &mut report,
                ready.sequence_index,
                format!("step output item-id {item_id:?} does not match current item"),
            );
        }
    if !report.rejected_outputs.is_empty() {
        return reject_step_output(trait_ref, state, ready.sequence_index, report);
    }

    let is_check_item = ready.item.effective_kind() == SequenceKind::Check;
    let allowed_outputs: BTreeSet<&str> = ready.item.output.ref_texts().collect();
    let mut produced_outputs = BTreeSet::new();
    let mut accepted_slot_values = Vec::new();
    let mut accepted_output_port_values = Vec::new();
    for output in envelope.produced_slots {
        let digest = value_digest(&output.value)?;
        if !allowed_outputs.contains(output.ref_text.as_str()) {
            report.unexpected_outputs.push(output.ref_text.clone());
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: ready.position_path.clone(),
                ref_text: Some(output.ref_text),
                value_digest: Some(digest),
                reason: "slot output is not declared for the current sequence item".to_string(),
            });
            continue;
        }

        let Some(sink) = ready.item.output.sink_for_ref(&output.ref_text) else {
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: ready.position_path.clone(),
                ref_text: Some(output.ref_text),
                value_digest: Some(digest),
                reason: "declared output sink could not be resolved".to_string(),
            });
            continue;
        };
        if !produced_outputs.insert(output.ref_text.clone()) {
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: ready.position_path.clone(),
                ref_text: Some(output.ref_text),
                value_digest: Some(digest),
                reason: "slot output was submitted more than once".to_string(),
            });
            continue;
        }
        let submitted_payload = output.value.clone();
        let prior_value = accepted_value(&state, &output.ref_text).cloned();
        let mut runtime_value = runtime_value_for_output_sink(
            trait_ref,
            ready.sequence_index,
            sink,
            output,
            is_check_item,
            prior_value.as_ref().map(|value| &value.value),
        )?;
        if runtime_value.acceptance == AcceptanceStatus::Accepted {
            runtime_value = apply_runtime_write(
                trait_ref,
                sink.operation(),
                prior_value.as_ref(),
                runtime_value,
            )?;
        }
        report
            .schema_validation
            .extend(runtime_value.schema_validation.clone());
        if active_for_each_over_slot_rewrite(
            &state,
            &runtime_value.ref_text,
            &runtime_value.value_digest,
        ) {
            runtime_value.acceptance = AcceptanceStatus::Rejected;
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: ready.position_path.clone(),
                ref_text: Some(runtime_value.ref_text.clone()),
                value_digest: Some(runtime_value.value_digest.clone()),
                reason: "cannot rewrite an active for-each over slot with a different digest"
                    .to_string(),
            });
        } else if runtime_value
            .schema_validation
            .iter()
            .any(|v| v.status == SchemaStatus::Rejected)
        {
            runtime_value.acceptance = AcceptanceStatus::Rejected;
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: ready.position_path.clone(),
                ref_text: Some(runtime_value.ref_text.clone()),
                value_digest: Some(runtime_value.value_digest.clone()),
                reason: "schema validation rejected slot output".to_string(),
            });
        } else {
            report.accepted_outputs.push(runtime_value.clone());
            match Reference::parse(&runtime_value.ref_text).map(|parsed| parsed.kind()) {
                Ok(Kind::Port) => accepted_output_port_values.push(runtime_value),
                Ok(Kind::Schema) => {}
                _ => accepted_slot_values.push((
                    runtime_value,
                    sink.operation().clone(),
                    submitted_payload,
                    prior_value,
                )),
            }
        }
    }

    for expected in ready.item.output.ref_texts() {
        if !produced_outputs.contains(expected) {
            if ready.item.output.is_optional_for(expected) {
                report.unfilled_optional_outputs.push(expected.to_string());
            } else {
                report.missing_required_outputs.push(expected.to_string());
            }
        }
    }

    let allowed_signals: BTreeSet<&str> = ready
        .item
        .on_complete
        .iter()
        .filter(|emit| emit.when().is_none())
        .map(|emit| emit.signal_ref())
        .collect();
    let mut accepted_signals = Vec::new();
    for signal in envelope.signals {
        let emission = validate_signal_with_context(
            ready.sequence_index,
            &allowed_signals,
            signal,
            &ready.position_path,
            ready.loop_context.as_ref(),
            ready.for_each_context.as_ref(),
            None,
        )?;
        if emission.acceptance == AcceptanceStatus::Accepted {
            accepted_signals.push(emission.clone());
        }
        report.signal_validation.push(emission);
    }

    for emit in ready.item.on_complete.iter().filter(|emit| emit.when().is_some()) {
        let when = emit.when().expect("filtered");
        let loop_context = ready.loop_context.clone().unwrap_or(LoopContext {
            loop_id: ready.item.id.clone().unwrap_or_else(|| "step".to_string()),
            sequence_id: None,
            iteration_index: 0,
            max_iterations: 0,
        });
        let (matched, evaluations) = evaluate_guard_expr(
            trait_ref,
            &state,
            when,
            &loop_context,
            &ready.position_path,
            &report.accepted_outputs,
        )?;
        if matched {
            let allowed: BTreeSet<&str> = BTreeSet::from([emit.signal_ref()]);
            let emission = validate_signal_with_context(
                ready.sequence_index,
                &allowed,
                StepSignalOutput {
                    ref_text: emit.signal_ref().to_string(),
                    evidence: Some(derived_signal_evidence(when, &evaluations)),
                    producer_agent: None,
                    producer_harness: None,
                },
                &ready.position_path,
                ready.loop_context.as_ref(),
                ready.for_each_context.as_ref(),
                None,
            )?;
            accepted_signals.push(emission.clone());
            report.signal_validation.push(emission);
        }
    }

    let rejected_signal = report
        .signal_validation
        .iter()
        .any(|signal| signal.acceptance == AcceptanceStatus::Rejected);
    let rejected = !report.rejected_outputs.is_empty()
        || rejected_signal
        || !report.missing_required_outputs.is_empty();

    if rejected {
        return reject_step_output(trait_ref, state, ready.sequence_index, report);
    }

    let producer_path = producer_path_for_ready(&ready);
    for (runtime_value, _, _, _) in &accepted_slot_values {
        if duplicate_slot_write_in_activation(&state, &runtime_value.ref_text, &producer_path) {
            report.rejected_outputs.push(RejectedAttempt {
                sequence_index: ready.sequence_index,
                position_path: producer_path.clone(),
                ref_text: Some(runtime_value.ref_text.clone()),
                value_digest: Some(runtime_value.value_digest.clone()),
                reason: "slot was already written in the current scope activation".to_string(),
            });
            return reject_step_output(trait_ref, state, ready.sequence_index, report);
        }
    }
    // All outputs accepted by this single activation (a check's `[slot,
    // port]` pair, in particular) must replay against the same pre-activation
    // historical cutoff — one sibling must never see another sibling's write
    // as "history". Snapshot the order once, before recording either half,
    // rather than re-deriving it per output (which would let the port half
    // see the slot half's just-recorded revision as prior state).
    let activation_acceptance_order = next_acceptance_order(&state);
    for (runtime_value, operation, submitted_payload, prior_value) in accepted_slot_values {
        let revision = slot_revision_from_value(
            &runtime_value,
            SlotRevisionWrite {
                operation,
                submitted_payload,
                        prior_value: prior_value.as_ref(),
                        runtime_binding: false,
                        projection: None,
            },
            SlotRevisionContext {
                acceptance_order: activation_acceptance_order,
                position_path: &producer_path,
                loop_context: ready.loop_context.as_ref(),
                for_each_context: ready.for_each_context.as_ref(),
            },
        )?;
        record_accepted_slot_value(&mut state, runtime_value, revision);
    }
    for mut runtime_value in accepted_output_port_values {
        // Stamp where/when this direct port write was accepted (task 0206),
        // so ledger-contract replay can reconstruct the historical state
        // needed to re-verify argv-interpolated command provenance the same
        // way a `SlotRevision` does — see `Value::position_path`.
        runtime_value.position_path = producer_path.clone();
        runtime_value.acceptance_order = Some(activation_acceptance_order);
        record_accepted_output_port_value(&mut state, runtime_value);
    }
    record_emitted_signals(&mut state, accepted_signals);
    if state.control_stack.is_empty() {
        set_sequence_status(
            &mut state,
            ready.sequence_index,
            SequenceStatusKind::Accepted,
            "all declared outputs accepted",
        );
    } else {
        set_path_sequence_status(
            &mut state,
            SequenceStatus {
                sequence_index: ready.sequence_index,
                run_index: ready.run_index,
                item_id: ready.item.id.clone(),
                title: ready.item.title.clone(),
                status: SequenceStatusKind::Accepted,
                reason: "all declared outputs accepted".to_string(),
                position_path: producer_path,
            },
        );
    }
    advance_after_current_leaf(&mut state);
    evaluate_control_guards_after_step(trait_ref, &mut state, &report.accepted_outputs)?;
    refresh_runtime_status(trait_ref, &mut state)?;
    report.next_action = match state.final_state {
        FinalState::Running => StepNextAction::Continue,
        FinalState::Blocked => StepNextAction::Blocked,
        FinalState::Completed => StepNextAction::Completed,
        FinalState::Rejected => StepNextAction::Rejected,
        FinalState::Failed => StepNextAction::Failed,
    };
    Ok((state, report))
}

fn active_for_each_over_slot_rewrite(state: &State, ref_text: &str, value_digest: &str) -> bool {
    state.control_stack.iter().any(|frame| {
        frame.kind == ControlKind::ForEach
            && frame.over_slot.as_deref() == Some(ref_text)
            && frame
                .list_digest
                .as_deref()
                .is_some_and(|digest| digest != value_digest)
    })
}

fn apply_runtime_write(
    trait_ref: &Trait,
    operation: &WriteOperation,
    prior: Option<&Value>,
    mut value: Value,
) -> crate::Result<Value> {
    if operation == &WriteOperation::Replace {
        return Ok(value);
    }
    let Some(destination_schema) = output_schema_ref(trait_ref, &value.ref_text) else {
        return Err(crate::procedure::invalid_field(
            "procedure.sequence.output.operation",
            format!(
                "write operation output {:?} has no slot schema",
                value.ref_text
            ),
        ));
    };
    value.value = apply_write_operation_value(
        operation,
        prior.map(|existing| &existing.value),
        &value.value,
    )?;
    value.value_digest = value_digest(&value.value)?;
    value.schema_ref = runtime_schema_reference(&destination_schema)?;
    let validation =
        validate_value_schema(trait_ref, &value.ref_text, &destination_schema, &value.value)?;
    let rejected = if operation == &WriteOperation::Append {
        validation.status == SchemaStatus::Rejected
    } else {
        validation.status != SchemaStatus::Accepted
    };
    if rejected {
        value.acceptance = AcceptanceStatus::Rejected;
    }
    value.schema_validation.push(validation);
    Ok(value)
}

fn apply_write_operation_value(
    operation: &WriteOperation,
    prior: Option<&JsonValue>,
    submitted: &JsonValue,
) -> crate::Result<JsonValue> {
    match operation {
        WriteOperation::Replace => Ok(submitted.clone()),
        WriteOperation::Append => {
            let mut items = match prior {
                Some(existing) => existing.as_array().cloned().ok_or_else(|| {
                    crate::procedure::invalid_field(
                        "runtime.slot-revisions",
                        "append target does not contain an array value",
                    )
                })?,
                None => Vec::new(),
            };
            items.push(submitted.clone());
            Ok(JsonValue::Array(items))
        }
        WriteOperation::Merge => {
            let mut object = match prior {
                Some(existing) => existing.as_object().cloned().ok_or_else(|| {
                    crate::procedure::invalid_field(
                        "runtime.slot-revisions",
                        "merge target does not contain an object value",
                    )
                })?,
                None => serde_json::Map::new(),
            };
            let delta = submitted.as_object().ok_or_else(|| {
                crate::procedure::invalid_field(
                    "runtime.slot-revisions",
                    "merge payload is not an object",
                )
            })?;
            deep_merge_object(&mut object, delta);
            Ok(JsonValue::Object(object))
        }
        WriteOperation::SetField(field) => {
            let mut object = match prior {
                Some(existing) => existing.as_object().cloned().ok_or_else(|| {
                    crate::procedure::invalid_field(
                        "runtime.slot-revisions",
                        "set-field target does not contain an object value",
                    )
                })?,
                None => serde_json::Map::new(),
            };
            object.insert(field.clone(), submitted.clone());
            Ok(JsonValue::Object(object))
        }
        WriteOperation::Increment => increment_json_number(prior, submitted),
    }
}

fn deep_merge_object(
    target: &mut serde_json::Map<String, JsonValue>,
    delta: &serde_json::Map<String, JsonValue>,
) {
    for (key, delta_value) in delta {
        match (target.get_mut(key), delta_value) {
            (Some(JsonValue::Object(target_object)), JsonValue::Object(delta_object)) => {
                deep_merge_object(target_object, delta_object);
            }
            _ => {
                target.insert(key.clone(), delta_value.clone());
            }
        }
    }
}

fn increment_json_number(
    prior: Option<&JsonValue>,
    submitted: &JsonValue,
) -> crate::Result<JsonValue> {
    let zero = JsonValue::from(0);
    let current = prior.unwrap_or(&zero);
    let current_number = current.as_number().ok_or_else(|| {
        crate::procedure::invalid_field(
            "runtime.slot-revisions",
            "increment target does not contain a number value",
        )
    })?;
    let delta_number = submitted.as_number().ok_or_else(|| {
        crate::procedure::invalid_field(
            "runtime.slot-revisions",
            "increment payload is not a number",
        )
    })?;
    if (current_number.is_i64() || current_number.is_u64())
        && (delta_number.is_i64() || delta_number.is_u64())
    {
        let sum = json_integer_as_i128(current_number)
            .checked_add(json_integer_as_i128(delta_number))
            .ok_or_else(|| {
                crate::procedure::invalid_field("runtime.slot-revisions", "increment overflow")
            })?;
        if sum < 0 {
            let value = i64::try_from(sum).map_err(|_| {
                crate::procedure::invalid_field("runtime.slot-revisions", "increment overflow")
            })?;
            return Ok(JsonValue::from(value));
        }
        let value = u64::try_from(sum).map_err(|_| {
            crate::procedure::invalid_field("runtime.slot-revisions", "increment overflow")
        })?;
        return Ok(JsonValue::from(value));
    }
    let sum = exact_json_f64(current_number)? + exact_json_f64(delta_number)?;
    let number = serde_json::Number::from_f64(sum).ok_or_else(|| {
        crate::procedure::invalid_field(
            "runtime.slot-revisions",
            "increment produced a non-finite number",
        )
    })?;
    Ok(JsonValue::Number(number))
}

fn json_integer_as_i128(number: &serde_json::Number) -> i128 {
    number
        .as_i64()
        .map(i128::from)
        .unwrap_or_else(|| i128::from(number.as_u64().unwrap_or(0)))
}

fn exact_json_f64(number: &serde_json::Number) -> crate::Result<f64> {
    const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_992;
    if let Some(value) = number.as_i64() {
        if value.unsigned_abs() > MAX_EXACT_INTEGER {
            return Err(crate::procedure::invalid_field(
                "runtime.slot-revisions",
                "increment would lose integer precision",
            ));
        }
        return Ok(value as f64);
    }
    if let Some(value) = number.as_u64() {
        if value > MAX_EXACT_INTEGER {
            return Err(crate::procedure::invalid_field(
                "runtime.slot-revisions",
                "increment would lose integer precision",
            ));
        }
        return Ok(value as f64);
    }
    number.as_f64().ok_or_else(|| {
        crate::procedure::invalid_field("runtime.slot-revisions", "increment payload is not finite")
    })
}

fn derived_signal_evidence(guard: &GuardExpr, evaluations: &[ConditionEvaluation]) -> String {
    let guard_text = serde_json::to_string(guard).unwrap_or_else(|_| "<guard>".to_string());
    let matched = evaluations
        .iter()
        .filter(|evaluation| evaluation.matched)
        .map(|evaluation| evaluation.predicate.as_str())
        .collect::<Vec<_>>()
        .join(",");
    if matched.is_empty() {
        format!("derived-output:{guard_text}")
    } else {
        format!("derived-output:{guard_text}:matched={matched}")
    }
}

/// Compute final output-port completion from accepted slot values.
pub fn finalize_outputs(
    trait_ref: &Trait,
    state: &State,
) -> crate::Result<Vec<OutputPortCompletion>> {
    let proc = procedure(trait_ref)?;
    let mut completions = Vec::new();
    for ref_text in proc.output.iter() {
        let parsed = Reference::parse(ref_text).map_err(|_| {
            crate::procedure::invalid_field(
                "procedure.output",
                format!("invalid output port ref {ref_text:?}"),
            )
        })?;
        let port_id = parsed.id();
        let Some(port) = trait_ref.ports.iter().find(|port| port.id == port_id) else {
            continue;
        };
        let value_ref = port
            .value
            .clone()
            .unwrap_or_else(|| format!("port:{port_id}"));
        let accepted = if port.value.is_some() {
            state
                .accepted_slot_values
                .iter()
                .find(|value| value.ref_text == value_ref)
        } else {
            state
                .accepted_output_port_values
                .iter()
                .find(|value| value.ref_text == value_ref)
        };
        let port_schema_validation = accepted
            .map(|value| validate_value_schema(trait_ref, ref_text, &port.schema, &value.value))
            .transpose()?;
        let required = !port.optional;
        let status = if accepted.is_some() {
            OutputPortStatus::Accepted
        } else if required {
            OutputPortStatus::Missing
        } else {
            OutputPortStatus::OptionalMissing
        };
        completions.push(OutputPortCompletion {
            port_ref: Reference::parse(ref_text)?,
            value_slot_ref: Reference::parse(&value_ref)?,
            required,
            status,
            value_digest: accepted.map(|value| value.value_digest.clone()),
            reason: if accepted.is_some() {
                if port.value.is_some() {
                    match port_schema_validation {
                        Some(validation)
                            if validation.status != SchemaStatus::Accepted =>
                        {
                            format!(
                                "accepted slot value is available; output port schema recheck reported {}",
                                validation.reason
                            )
                        }
                        _ => "accepted slot value is available and matches output port schema"
                            .to_string(),
                    }
                } else {
                    "accepted direct output port value is available".to_string()
                }
            } else if required {
                "required output value is not accepted".to_string()
            } else {
                "optional output value is absent".to_string()
            },
        });
    }
    completions.sort_by(|a, b| a.port_ref.cmp(&b.port_ref));
    Ok(completions)
}

#[cfg(test)]
mod resolved_settings_ledger_tests {
    use super::*;

    fn minimal_trait() -> crate::r#trait::Trait {
        crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            r#"
id = "resolved-settings-ledger-test"
schema-version = "0.3"
version = "0.1.0"
name = "Resolved settings ledger test"
summary = "Minimal fixture."

[[agent]]
id = "worker"
description = "Runs the single step."
summary = "Worker role."

[[slot]]
id = "note"
schema = "schema:text"
description = "Step output."

[prompt.write-note]
text = "Write a note."

[procedure]
description = "One step, no loop."

[[procedure.sequence]]
id = "write-note"
title = "Write note"
agent = "agent:worker"
prompt = "prompt:write-note"
output = ["slot:note"]
"#,
        )
        .expect("minimal trait decodes")
    }

    #[test]
    fn start_procedure_run_records_resolved_settings_as_ledger_evidence() {
        let trait_ref = minimal_trait();
        let records = vec![ResolvedSettingRecord {
            id: "review-rounds".to_string(),
            value: serde_json::json!(5),
            source: SettingSourceLayer::Variant,
        }];
        let state = start_procedure_run(
            &trait_ref,
            Id::new("run-resolved-settings-test").expect("run id"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            records.clone(),
            Vec::new(),
        )
        .expect("run starts");
        assert_eq!(state.resolved_settings, records);
    }
}
