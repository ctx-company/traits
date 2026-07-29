// Procedure runtime ledger contract outputs.
// Procedure runtime ledger contract outputs.

fn validate_rejected_attempts(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let declaration_indexes: BTreeSet<usize> =
        sequence.iter().map(|item| item.declaration_index).collect();
    for attempt in &ledger.rejected_attempts {
        if attempt.position_path.is_empty() {
            if !declaration_indexes.contains(&attempt.sequence_index) {
                diagnostics.push(format!(
                    "rejected attempt references unknown sequence index {}",
                    attempt.sequence_index
                ));
            }
            continue;
        }
        let resolved = item_at_execution_path(
            trait_ref,
            sequence,
            ledger,
            &attempt.position_path,
        );
        if resolved.is_none_or(|item| {
            attempt.position_path.last().is_none_or(|segment| {
                segment.kind != "item"
                    || segment.index != attempt.sequence_index
                    || segment.id != item.id
            })
        }) {
            diagnostics.push(format!(
                "rejected attempt at sequence index {} does not follow the selected structural path",
                attempt.sequence_index
            ));
        }
    }
}

fn validate_output_port_contract(
    trait_ref: &Trait,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let mut seen_rows = BTreeSet::new();
    for (index, row) in ledger.output_ports.iter().enumerate() {
        if !seen_rows.insert(row.port_ref.as_str()) {
            diagnostics.push(format!(
                "output_ports[{index}] duplicates port-ref {:?}",
                row.port_ref
            ));
        }
    }
    let expected = output_port_fact_map(finalize_outputs(trait_ref, ledger)?);
    let actual = output_port_fact_map(ledger.output_ports.clone());
    for port_ref in expected.keys() {
        if !actual.contains_key(port_ref) {
            diagnostics.push(format!(
                "output_ports is missing expected port row {port_ref}"
            ));
        }
    }
    for (port_ref, actual_fact) in actual {
        let Some(expected_fact) = expected.get(&port_ref) else {
            diagnostics.push(format!(
                "output_ports contains unexpected port row {port_ref}"
            ));
            continue;
        };
        if &actual_fact != expected_fact {
            diagnostics.push(format!(
                "output_ports row {port_ref} contradicts recomputed semantic output evidence"
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutputPortFact {
    value_slot_ref: String,
    required: bool,
    status: OutputPortStatus,
    value_digest: Option<Digest>,
}

fn output_port_fact_map(outputs: Vec<OutputPortCompletion>) -> BTreeMap<String, OutputPortFact> {
    outputs
        .into_iter()
        .map(|output| {
            (
                output.port_ref.to_string(),
                OutputPortFact {
                    value_slot_ref: output.value_slot_ref.to_string(),
                    required: output.required,
                    status: output.status,
                    value_digest: output.value_digest,
                },
            )
        })
        .collect()
}

fn validate_final_state_contract(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let sequence_len = sequence_contract.by_run.len();
    let missing_ports = missing_required_procedure_ports_from_map(trait_ref, accepted_evidence);
    let current_item = sequence_contract.by_run.get(&ledger.current_run_index);
    // Reuse the same structural resolution and guard-aware hard-input
    // derivation the live readiness path uses (`current_ready_item` +
    // `missing_inputs_for_item`), so a matched guarded resource that is
    // missing from replayed ledger evidence is recognized here exactly as
    // it would block a live run (P290).
    let mut missing_current_inputs = match current_ready_item(trait_ref, ledger, sequence)? {
        Some(ready) => missing_inputs_for_item(trait_ref, &ready, ledger)?,
        None => Vec::new(),
    };
    missing_current_inputs.sort();
    missing_current_inputs.dedup();
    let output_ports = finalize_outputs(trait_ref, ledger)?;
    let missing_required_outputs: Vec<String> = output_ports
        .iter()
        .filter(|output| output.required && output.status != OutputPortStatus::Accepted)
        .map(|output| output.port_ref.to_string())
        .collect();
    match ledger.final_state {
        FinalState::Completed => {
            if ledger.current_run_index != sequence_len {
                diagnostics.push(format!(
                    "completed ledger current-run-index {} does not equal sequence length {}",
                    ledger.current_run_index, sequence_len
                ));
            }
            for status in &ledger.sequence_statuses {
                let accepted = status.status == SequenceStatusKind::Accepted
                    || route_authorizes_status(ledger, status)
                    || ask_authorizes_status(ledger, status);
                if !accepted {
                    diagnostics.push(format!(
                        "completed ledger has non-accepted sequence status at run index {}",
                        status.run_index
                    ));
                }
            }
            if !missing_ports.is_empty() {
                diagnostics.push(format!(
                    "completed ledger is missing required input port(s): {}",
                    missing_ports.join(", ")
                ));
            }
            if !missing_required_outputs.is_empty() {
                diagnostics.push(format!(
                    "completed ledger is missing required output port(s): {}",
                    missing_required_outputs.join(", ")
                ));
            }
        }
        FinalState::Running => {
            let active_nested_current = has_active_nested_current_control(ledger);
            if ledger.current_run_index >= sequence_len {
                diagnostics.push(format!(
                    "running ledger current-run-index {} is not within sequence length {}",
                    ledger.current_run_index, sequence_len
                ));
            }
            if current_item.is_none() {
                diagnostics.push("running ledger has no current sequence item".to_string());
            }
            validate_prior_statuses_accepted(
                sequence_contract,
                status_maps,
                ledger,
                ledger.current_run_index,
                "running",
                diagnostics,
            );
            if active_nested_current {
                validate_current_status_any(
                    sequence_contract,
                    status_maps,
                    ledger.current_run_index,
                    &[SequenceStatusKind::Pending, SequenceStatusKind::Ready],
                    "running",
                    diagnostics,
                );
            } else {
                validate_current_status(
                    sequence_contract,
                    status_maps,
                    ledger.current_run_index,
                    SequenceStatusKind::Ready,
                    "running",
                    diagnostics,
                );
            }
            validate_future_statuses_after_run_index(
                sequence_contract,
                status_maps,
                ledger.current_run_index,
                "running",
                "current-run-index",
                diagnostics,
            );
            if !missing_ports.is_empty() || !missing_current_inputs.is_empty() {
                diagnostics.push(format!(
                    "running ledger is not ready; missing ports [{}], missing current inputs [{}]",
                    missing_ports.join(", "),
                    missing_current_inputs.join(", ")
                ));
            }
        }
        FinalState::Blocked => {
            if missing_ports.is_empty()
                && missing_current_inputs.is_empty()
                && missing_required_outputs.is_empty()
                && ledger.active_path.is_empty()
                && ledger.stop_reason.is_none()
            {
                diagnostics.push(
                    "blocked ledger has no identifiable missing input or output evidence"
                        .to_string(),
                );
            }
            validate_prior_statuses_accepted(
                sequence_contract,
                status_maps,
                ledger,
                ledger.current_run_index,
                "blocked",
                diagnostics,
            );
            if ledger.current_run_index < sequence_len {
                validate_current_status(
                    sequence_contract,
                    status_maps,
                    ledger.current_run_index,
                    SequenceStatusKind::Blocked,
                    "blocked",
                    diagnostics,
                );
                validate_future_statuses_after_run_index(
                    sequence_contract,
                    status_maps,
                    ledger.current_run_index,
                    "blocked",
                    "current-run-index",
                    diagnostics,
                );
            } else if missing_required_outputs.is_empty() {
                diagnostics.push(
                    "blocked ledger at completion index lacks missing required final output evidence"
                        .to_string(),
                );
            }
        }
        FinalState::Rejected => {
            if ledger.rejected_attempts.is_empty() {
                diagnostics.push("rejected ledger has no rejection evidence".to_string());
            }
            let rejected_statuses: Vec<&SequenceStatus> = ledger
                .sequence_statuses
                .iter()
                .filter(|status| status.status == SequenceStatusKind::Rejected)
                .collect();
            if rejected_statuses.is_empty() {
                diagnostics.push("rejected ledger has no rejected sequence status".to_string());
            }
            if rejected_statuses.len() > 1 {
                diagnostics
                    .push("rejected ledger has more than one rejected sequence status".to_string());
            }
            if !rejected_statuses.is_empty()
                && !rejected_statuses
                    .iter()
                    .any(|status| ledger.rejected_attempts.iter().any(|attempt| {
                        status.sequence_index == attempt.sequence_index
                            && status.position_path == attempt.position_path
                            && (attempt.position_path.is_empty()
                                || attempt.position_path.first().is_some_and(|segment| {
                                    segment.kind == "procedure"
                                        && segment.index == status.run_index
                                }))
                    }))
            {
                diagnostics.push(
                    "rejected ledger rejected sequence status does not match rejected attempt evidence"
                        .to_string(),
                );
            }
            if let Some(rejected) = rejected_statuses.first() {
                validate_prior_statuses_accepted(
                    sequence_contract,
                    status_maps,
                    ledger,
                    rejected.run_index,
                    "rejected",
                    diagnostics,
                );
                validate_future_statuses_after_run_index(
                    sequence_contract,
                    status_maps,
                    rejected.run_index,
                    "rejected",
                    "rejected sequence run-index",
                    diagnostics,
                );
                validate_rejected_sequence_evidence_absent(
                    sequence_contract,
                    status_maps,
                    accepted_evidence,
                    rejected.run_index,
                    diagnostics,
                );
            }
        }
        FinalState::Failed => {
            if ledger.rejected_attempts.is_empty() && ledger.stop_reason.is_none() {
                diagnostics.push(
                    "failed ledger has no diagnostic attempt evidence or stop reason".to_string(),
                );
            }
            if ledger.current_run_index < sequence_len {
                validate_future_statuses_after_run_index(
                    sequence_contract,
                    status_maps,
                    ledger.current_run_index,
                    "failed",
                    "current-run-index",
                    diagnostics,
                );
            }
        }
    }
    Ok(())
}

fn has_active_nested_current_control(ledger: &State) -> bool {
    let Some(first_segment) = ledger.active_path.first() else {
        return false;
    };
    if first_segment.kind != "procedure" || first_segment.index != ledger.current_run_index {
        return false;
    }
    ledger
        .control_stack
        .first()
        .is_some_and(|frame| frame.parent_run_index == ledger.current_run_index)
}

fn missing_required_procedure_ports_from_map(
    trait_ref: &Trait,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
) -> Vec<String> {
    let Some(proc) = trait_ref.procedure.as_ref() else {
        return Vec::new();
    };
    let input_ports: BTreeMap<&str, &crate::r#trait::Port> = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Input))
        .map(|port| (port.id.as_str(), port))
        .collect();
    let mut missing = Vec::new();
    for ref_text in proc.input.iter() {
        let Ok(parsed) = Reference::parse(ref_text) else {
            continue;
        };
        let Some(port) = input_ports.get(parsed.id()) else {
            continue;
        };
        if port.optional {
            continue;
        }
        if !accepted_evidence.ports.contains_key(ref_text) {
            missing.push(ref_text.clone());
        }
    }
    missing.sort();
    missing
}

fn validate_prior_statuses_accepted(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    ledger: &State,
    current_run_index: usize,
    state_label: &str,
    diagnostics: &mut Vec<String>,
) {
    for run_index in 0..current_run_index.min(sequence_contract.by_run.len()) {
        let Some(item) = sequence_contract.by_run.get(&run_index) else {
            continue;
        };
        match status_maps.by_run.get(&run_index) {
            Some(status) if status.status == SequenceStatusKind::Accepted => {}
            Some(status) if route_authorizes_status(ledger, status) || ask_authorizes_status(ledger, status) => {}
            Some(status) => diagnostics.push(format!(
                "{state_label} ledger prior sequence run {}/source {} status {:?} is not Accepted",
                item.run_index, item.declaration_index, status.status
            )),
            None => diagnostics.push(format!(
                "{state_label} ledger prior sequence run {}/source {} is missing a sequence status",
                item.run_index, item.declaration_index
            )),
        }
    }
}

fn route_authorizes_status(ledger: &State, status: &SequenceStatus) -> bool {
    if !status.position_path.is_empty() {
        return false;
    }
    ledger.failure_routes.iter().any(|route| match status.status {
        SequenceStatusKind::Routed => route.source_run_index == status.run_index,
        SequenceStatusKind::Skipped => {
            route.source_run_index < status.run_index && status.run_index < route.target_run_index
        }
        _ => false,
    })
}

/// Ask dispositions are immutable guard decisions, not failure routes. A
/// matched activation may be ready/accepted; an unmatched activation is the
/// one legitimate skipped leaf state.
fn ask_authorizes_status(ledger: &State, status: &SequenceStatus) -> bool {
    let position_path = if status.position_path.is_empty() {
        vec![PathSegment {
            kind: "procedure".to_string(),
            id: status.item_id.clone(),
            index: status.run_index,
            iteration: None,
            item_index: None,
        }]
    } else {
        status.position_path.clone()
    };
    ledger.ask_decisions.iter().any(|decision| {
        decision.sequence_index == status.sequence_index
            && decision.position_path == position_path
            && ((decision.matched
                && matches!(status.status, SequenceStatusKind::Ready | SequenceStatusKind::Accepted))
                || (!decision.matched && status.status == SequenceStatusKind::Skipped))
    })
}

fn validate_current_status(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    current_run_index: usize,
    expected: SequenceStatusKind,
    state_label: &str,
    diagnostics: &mut Vec<String>,
) {
    let Some(item) = sequence_contract.by_run.get(&current_run_index) else {
        return;
    };
    match status_maps.by_run.get(&current_run_index) {
        Some(status) if status.status == expected => {}
        Some(status) => diagnostics.push(format!(
            "{state_label} ledger current sequence run {}/source {} status {:?} is not {:?}",
            item.run_index, item.declaration_index, status.status, expected
        )),
        None => diagnostics.push(format!(
            "{state_label} ledger current sequence run {}/source {} is missing a sequence status",
            item.run_index, item.declaration_index
        )),
    }
}

fn validate_current_status_any(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    current_run_index: usize,
    expected: &[SequenceStatusKind],
    state_label: &str,
    diagnostics: &mut Vec<String>,
) {
    let Some(item) = sequence_contract.by_run.get(&current_run_index) else {
        return;
    };
    match status_maps.by_run.get(&current_run_index) {
        Some(status) if expected.contains(&status.status) => {}
        Some(status) => diagnostics.push(format!(
            "{state_label} ledger current sequence run {}/source {} status {:?} is not one of {:?}",
            item.run_index, item.declaration_index, status.status, expected
        )),
        None => diagnostics.push(format!(
            "{state_label} ledger current sequence run {}/source {} is missing a sequence status",
            item.run_index, item.declaration_index
        )),
    }
}

fn validate_future_statuses_after_run_index(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    run_index: usize,
    state_label: &str,
    index_label: &str,
    diagnostics: &mut Vec<String>,
) {
    let Some(first_future_run_index) = run_index.checked_add(1) else {
        diagnostics.push(format!(
            "{state_label} ledger {index_label} {run_index} cannot be advanced to first future run index without overflow"
        ));
        return;
    };
    validate_future_statuses_unreached(
        sequence_contract,
        status_maps,
        first_future_run_index,
        state_label,
        diagnostics,
    );
}

fn validate_future_statuses_unreached(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    first_future_run_index: usize,
    state_label: &str,
    diagnostics: &mut Vec<String>,
) {
    for run_index in first_future_run_index..sequence_contract.by_run.len() {
        let Some(item) = sequence_contract.by_run.get(&run_index) else {
            continue;
        };
        let Some(status) = status_maps.by_run.get(&run_index) else {
            diagnostics.push(format!(
                "{state_label} ledger future sequence run {}/source {} is missing a sequence status",
                item.run_index, item.declaration_index
            ));
            continue;
        };
        match status.status {
            SequenceStatusKind::Pending => {}
            SequenceStatusKind::DependencyPending => {
                // DependencyPending is allowed here only as not-yet-reached
                // dependency planning evidence. It must not represent a
                // current/reached executable sequence item.
            }
            SequenceStatusKind::Ready
            | SequenceStatusKind::Blocked
            | SequenceStatusKind::Accepted
            | SequenceStatusKind::Rejected
            | SequenceStatusKind::Routed
            | SequenceStatusKind::Skipped => diagnostics.push(format!(
                "{state_label} ledger future sequence run {}/source {}{} has reached/current status {:?}",
                item.run_index,
                item.declaration_index,
                item.item_id
                    .as_ref()
                    .map(|id| format!(" item {id}"))
                    .unwrap_or_default(),
                status.status
            )),
        }
    }
}

fn validate_rejected_sequence_evidence_absent(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
    rejected_run_index: usize,
    diagnostics: &mut Vec<String>,
) {
    for (slot_ref, value) in &accepted_evidence.slots {
        let Some(producers) = sequence_contract.slot_producers.get(slot_ref) else {
            continue;
        };
        let has_earlier_accepted_producer = producers.iter().any(|producer| {
            producer.run_index < rejected_run_index
                && status_maps
                    .by_declaration
                    .get(&producer.declaration_index)
                    .is_some_and(|status| status.status == SequenceStatusKind::Accepted)
        });
        let has_rejected_or_later_producer = producers
            .iter()
            .any(|producer| producer.run_index >= rejected_run_index);
        if has_rejected_or_later_producer && !has_earlier_accepted_producer {
            diagnostics.push(format!(
                "rejected ledger retains accepted slot {} from rejected or later sequence",
                value.ref_text
            ));
        }
    }
    for signal in accepted_evidence.signals.values() {
        let signal_run_index = status_maps
            .by_declaration
            .get(&signal.sequence_index)
            .map(|status| status.run_index);
        if signal_run_index.is_some_and(|run_index| run_index >= rejected_run_index) {
            diagnostics.push(format!(
                "rejected ledger retains accepted signal {} from rejected or later sequence",
                signal.signal_ref
            ));
        }
    }
}

fn missing_inputs_for_refs(trait_ref: &Trait, refs: &[String], state: &State) -> Vec<String> {
    let mut missing = Vec::new();
    for ref_text in refs {
        match Reference::parse(ref_text) {
            // Dependency resource evidence is materialized by the IO boundary
            // with its qualified ref intact. Core only verifies that evidence.
            Ok(parsed) if parsed.is_qualified() && parsed.kind() == Kind::Resource => {
                if accepted_resource(state, ref_text).is_none() {
                    missing.push(ref_text.clone());
                }
            }
            Ok(parsed) if parsed.is_qualified() => missing.push(ref_text.clone()),
            Ok(parsed)
                if parsed.kind() == Kind::Port
                    && trait_ref.ports.iter().any(|port| {
                        port.id == parsed.id()
                            && matches!(port.direction, PortDirection::Input)
                            && port.optional
                    }) => {}
            Ok(parsed) if parsed.kind() == Kind::Resource => {
                if accepted_resource(state, ref_text).is_none() {
                    missing.push(ref_text.clone());
                }
            }
            Ok(parsed) if parsed.kind() == Kind::Slot => {}
            Ok(_) => {
                if accepted_value(state, ref_text).is_none() {
                    missing.push(ref_text.clone());
                }
            }
            Err(_) => missing.push(ref_text.clone()),
        }
    }
    missing.sort();
    missing
}

fn missing_hard_slot_refs(refs: &[String], state: &State) -> Vec<String> {
    let mut missing: Vec<String> = refs
        .iter()
        .filter(|ref_text| accepted_value(state, ref_text).is_none())
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

fn missing_current_control_inputs(trait_ref: &Trait, state: &State) -> crate::Result<Vec<String>> {
    let Some(frame) = state.control_stack.last() else {
        return Ok(Vec::new());
    };
    let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
        return Ok(Vec::new());
    };
    let Some(item) = named.sequence.get(frame.next_index) else {
        return Ok(Vec::new());
    };
    let context = guard_context_from_stack(&state.control_stack).unwrap_or(LoopContext {
        loop_id: String::new(),
        sequence_id: None,
        iteration_index: 0,
        max_iterations: 1,
    });
    let active: BTreeSet<&str> =
        active_item_input_refs(trait_ref, state, item, &context, &state.active_path)?
            .into_iter()
            .collect();
    let (refs, hard_slots) = hard_input_refs_for_item(item, &active);
    let mut missing = missing_inputs_for_refs(trait_ref, &refs, state);
    missing.extend(missing_hard_slot_refs(&hard_slots, state));
    missing.sort();
    missing.dedup();
    Ok(missing)
}

fn hard_input_refs_for_item(
    item: &crate::r#trait::procedure::SequenceItem,
    active_guarded: &BTreeSet<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut refs = Vec::new();
    let mut hard_slots = Vec::new();
    for input in item.input.iter() {
        // An optional slot input never gates readiness (P447): it is either
        // absent (no accepted value, no requirement) or already active
        // through `active_item_input_refs`, so it is never a hard input.
        if input.is_optional() {
            continue;
        }
        // A guard-conditioned resource is a hard requirement only when its
        // guard matches: a false guard omits it without blocking the step,
        // a true guard requires its evidence like any other input (P290).
        if input.guard().is_some() && !active_guarded.contains(input.ref_text()) {
            continue;
        }
        let ref_text = input.ref_text();
        match Reference::parse(ref_text) {
            Ok(parsed) if parsed.is_qualified() => refs.push(ref_text.to_string()),
            Ok(parsed) if parsed.kind() == Kind::Slot => {}
            Ok(_) => refs.push(ref_text.to_string()),
            Err(_) => refs.push(ref_text.to_string()),
        }
    }
    if item.effective_kind() == SequenceKind::ForEach {
        if let Some(over) = item.over.as_deref() {
            if Reference::parse(over)
                .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
            {
                hard_slots.push(over.to_string());
            } else {
                refs.push(over.to_string());
            }
        }
    }
    refs.sort();
    refs.dedup();
    hard_slots.sort();
    hard_slots.dedup();
    (refs, hard_slots)
}
