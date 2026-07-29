// Procedure runtime ledger contract maps.

fn sequence_status_maps<'a>(
    ledger: &'a State,
    diagnostics: &mut Vec<String>,
) -> SequenceStatusMaps<'a> {
    let mut by_run = BTreeMap::new();
    let mut by_declaration = BTreeMap::new();
    for status in &ledger.sequence_statuses {
        if !status.position_path.is_empty() {
            continue;
        }
        if by_run.insert(status.run_index, status).is_some() {
            diagnostics.push(format!(
                "duplicate sequence status for run index {}",
                status.run_index
            ));
        }
        if by_declaration
            .insert(status.sequence_index, status)
            .is_some()
        {
            diagnostics.push(format!(
                "duplicate sequence status for source index {}",
                status.sequence_index
            ));
        }
    }
    SequenceStatusMaps {
        by_run,
        by_declaration,
    }
}

struct AcceptedEvidenceMaps<'a> {
    ports: BTreeMap<String, &'a Value>,
    slots: BTreeMap<String, &'a Value>,
    output_ports: BTreeMap<String, &'a Value>,
    signals: BTreeMap<(usize, String, String, String), &'a SignalEmission>,
}

fn accepted_evidence_maps<'a>(
    ledger: &'a State,
    diagnostics: &mut Vec<String>,
) -> AcceptedEvidenceMaps<'a> {
    let mut ports = BTreeMap::new();
    let mut slots = BTreeMap::new();
    let mut output_ports = BTreeMap::new();
    let mut signals = BTreeMap::new();

    for value in ledger
        .accepted_port_values
        .iter()
        .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
    {
        if ports.insert(value.ref_text.clone(), value).is_some() {
            diagnostics.push(format!("duplicate accepted port value {}", value.ref_text));
        }
    }
    for value in ledger
        .accepted_slot_values
        .iter()
        .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
    {
        if slots.insert(value.ref_text.clone(), value).is_some() {
            diagnostics.push(format!("duplicate accepted slot value {}", value.ref_text));
        }
    }
    for value in ledger
        .accepted_output_port_values
        .iter()
        .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
    {
        if output_ports.insert(value.ref_text.clone(), value).is_some() {
            diagnostics.push(format!(
                "duplicate accepted output port value {}",
                value.ref_text
            ));
        }
    }
    // Active parallel buffers are part of the current branch's visible
    // overlay. Insert them without duplicate diagnostics against the
    // committed maps because a branch-local write intentionally shadows the
    // same ref; completed sibling buffers remain excluded until the barrier.
    for frame in ledger
        .control_stack
        .iter()
        .filter(|frame| frame.kind == ControlKind::Parallel)
    {
        for value in frame
            .parallel_buffer
            .accepted_slot_values
            .iter()
            .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
        {
            slots.insert(value.ref_text.clone(), value);
        }
        for value in frame
            .parallel_buffer
            .accepted_output_port_values
            .iter()
            .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
        {
            output_ports.insert(value.ref_text.clone(), value);
        }
    }
    let visible_signals = ledger.emitted_signals.iter().chain(
        ledger
            .control_stack
            .iter()
            .filter(|frame| frame.kind == ControlKind::Parallel)
            .flat_map(|frame| frame.parallel_buffer.emitted_signals.iter()),
    );
    for signal in visible_signals
        .filter(|signal| signal.acceptance == AcceptanceStatus::Accepted)
    {
        let key = (
            signal.sequence_index,
            signal.signal_ref.to_string(),
            signal.evidence_digest.as_str().to_string(),
            format_path(&signal.position_path),
        );
        if signals.insert(key, signal).is_some() {
            diagnostics.push(format!(
                "duplicate emitted signal record {} at source {} with evidence {}",
                signal.signal_ref, signal.sequence_index, signal.evidence_digest
            ));
        }
    }

    AcceptedEvidenceMaps {
        ports,
        slots,
        output_ports,
        signals,
    }
}

fn validate_accepted_port_values(
    trait_ref: &Trait,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    for value in &ledger.accepted_port_values {
        if value.acceptance != AcceptanceStatus::Accepted {
            diagnostics.push(format!(
                "accepted port value {} is not marked accepted",
                value.ref_text
            ));
        }
        validate_value_digest(value, "accepted port", diagnostics)?;
        if value
            .schema_validation
            .iter()
            .any(|validation| validation.status == SchemaStatus::Rejected)
        {
            diagnostics.push(format!(
                "accepted port value {} contains rejected schema validation evidence",
                value.ref_text
            ));
        }
        let Ok(parsed) = Reference::parse(&value.ref_text) else {
            diagnostics.push(format!(
                "accepted port value {:?} is not a valid ref",
                value.ref_text
            ));
            continue;
        };
        if parsed.kind() != Kind::Port || parsed.is_qualified() {
            diagnostics.push(format!(
                "accepted port value {} must be a local port:* ref",
                value.ref_text
            ));
            continue;
        }
        let Some(port) = trait_ref
            .ports
            .iter()
            .find(|port| port.id == parsed.id() && matches!(port.direction, PortDirection::Input))
        else {
            diagnostics.push(format!(
                "accepted port value {} is not a declared input port",
                value.ref_text
            ));
            continue;
        };
        let expected_schema = runtime_schema_reference(&port.schema)?;
        if value.schema_ref != expected_schema {
            diagnostics.push(format!(
                "accepted port value {} schema {:?} does not match expected {:?}",
                value.ref_text, value.schema_ref, port.schema
            ));
        }
        validate_union_value_evidence(trait_ref, value, &port.schema, true, "accepted port", diagnostics)?;
    }
    Ok(())
}

fn validate_accepted_slot_values(
    trait_ref: &Trait,
    _proc: &crate::r#trait::procedure::Model,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let produced_slots: BTreeSet<String> = runtime_produced_refs(trait_ref)
        .into_iter()
        .filter(|ref_text| {
            Reference::parse(ref_text)
                .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
        })
        .collect();
    validate_accepted_slot_value_collection(
        trait_ref,
        &produced_slots,
        &ledger.accepted_slot_values,
        "accepted slot",
        false,
        diagnostics,
    )?;
    for (index, buffer) in recorded_effect_buffers(ledger).into_iter().enumerate() {
        validate_accepted_slot_value_collection(
            trait_ref,
            &produced_slots,
            &buffer.accepted_slot_values,
            &format!("parallel effect buffer[{index}] accepted slot"),
            true,
            diagnostics,
        )?;
    }
    Ok(())
}

fn validate_accepted_slot_value_collection(
    trait_ref: &Trait,
    produced_slots: &BTreeSet<String>,
    values: &[Value],
    context: &str,
    check_duplicates: bool,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if check_duplicates && !seen.insert(value.ref_text.as_str()) {
            diagnostics.push(format!(
                "{context} values contain duplicate ref {}",
                value.ref_text
            ));
        }
        if value.acceptance != AcceptanceStatus::Accepted {
            diagnostics.push(format!(
                "{context} value {} is not marked accepted",
                value.ref_text
            ));
        }
        validate_value_digest(value, context, diagnostics)?;
        if value
            .schema_validation
            .iter()
            .any(|validation| validation.status == SchemaStatus::Rejected)
        {
            diagnostics.push(format!(
                "{context} value {} contains rejected schema validation evidence",
                value.ref_text
            ));
        }
        let Ok(parsed) = Reference::parse(&value.ref_text) else {
            diagnostics.push(format!(
                "{context} value {:?} is not a valid ref",
                value.ref_text
            ));
            continue;
        };
        if parsed.kind() != Kind::Slot || parsed.is_qualified() {
            diagnostics.push(format!(
                "{context} value {} must be a local slot:* ref",
                value.ref_text
            ));
            continue;
        }
        if !trait_ref.slots.iter().any(|slot| slot.id == parsed.id()) {
            diagnostics.push(format!(
                "{context} value {} is not a declared slot",
                value.ref_text
            ));
        }
        if !produced_slots.contains(value.ref_text.as_str()) {
            diagnostics.push(format!(
                "{context} value {} is not produced by any procedure sequence output",
                value.ref_text
            ));
        }
        let expected_schema = slot_schema_ref(trait_ref, &value.ref_text);
        let expected_schema_ref = expected_schema
            .as_deref()
            .map(runtime_schema_reference)
            .transpose()?
            .flatten();
        if value.schema_ref != expected_schema_ref {
            diagnostics.push(format!(
                "{context} value {} schema {:?} does not match expected {:?}",
                value.ref_text, value.schema_ref, expected_schema
            ));
        }
        if let Some(expected_schema) = expected_schema.as_deref() {
            validate_union_value_evidence(
                trait_ref,
                value,
                expected_schema,
                false,
                context,
                diagnostics,
            )?;
        }
    }
    Ok(())
}

fn runtime_produced_refs(trait_ref: &Trait) -> BTreeSet<String> {
    let mut produced = BTreeSet::new();
    if let Some(procedure) = trait_ref.procedure.as_ref() {
        collect_runtime_produced_refs_from_items(
            trait_ref,
            &procedure.sequence,
            &mut produced,
            &mut BTreeSet::new(),
        );
    }
    produced
}

fn collect_runtime_produced_refs_from_items(
    trait_ref: &Trait,
    items: &[crate::r#trait::procedure::SequenceItem],
    produced: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
) {
    for item in items {
        produced.extend(
            item.output
                .ref_texts()
                .filter(|ref_text| {
                    Reference::parse(ref_text)
                        .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
                })
                .map(ToString::to_string),
        );
        if item.effective_kind() == SequenceKind::ForEach
            && let Some(item_slot) = item.item.as_ref() {
                produced.insert(item_slot.clone());
            }
        for sequence_id in [
            local_sequence_id(item.sequence.as_deref()),
            local_sequence_id(item.otherwise.as_deref()),
        ]
        .into_iter()
        .flatten()
        .chain(
            item.branches
                .iter()
                .filter_map(|branch_ref| local_sequence_id(Some(branch_ref))),
        )
        {
            if !stack.insert(sequence_id.clone()) {
                continue;
            }
            if let Some(named) = trait_ref.sequences.get(&sequence_id) {
                collect_runtime_produced_refs_from_items(trait_ref, &named.sequence, produced, stack);
            }
            stack.remove(&sequence_id);
        }
    }
}

fn local_sequence_id(ref_text: Option<&str>) -> Option<String> {
    let parsed = Reference::parse(ref_text?).ok()?;
    (parsed.kind() == Kind::Sequence && !parsed.is_qualified()).then(|| parsed.id().to_string())
}

fn validate_accepted_output_port_values(
    trait_ref: &Trait,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let produced_output_ports: BTreeSet<String> = runtime_produced_refs(trait_ref)
        .into_iter()
        .filter(|ref_text| {
            Reference::parse(ref_text)
                .is_ok_and(|parsed| parsed.kind() == Kind::Port && !parsed.is_qualified())
        })
        .collect();
    validate_accepted_output_port_value_collection(
        trait_ref,
        &produced_output_ports,
        &ledger.accepted_output_port_values,
        "accepted output port",
        false,
        diagnostics,
    )?;
    for (index, buffer) in recorded_effect_buffers(ledger).into_iter().enumerate() {
        validate_accepted_output_port_value_collection(
            trait_ref,
            &produced_output_ports,
            &buffer.accepted_output_port_values,
            &format!("parallel effect buffer[{index}] accepted output port"),
            true,
            diagnostics,
        )?;
    }
    Ok(())
}

fn validate_accepted_output_port_value_collection(
    trait_ref: &Trait,
    produced_output_ports: &BTreeSet<String>,
    values: &[Value],
    context: &str,
    check_duplicates: bool,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if check_duplicates && !seen.insert(value.ref_text.as_str()) {
            diagnostics.push(format!(
                "{context} values contain duplicate ref {}",
                value.ref_text
            ));
        }
        if value.acceptance != AcceptanceStatus::Accepted {
            diagnostics.push(format!(
                "{context} value {} is not marked accepted",
                value.ref_text
            ));
        }
        validate_value_digest(value, context, diagnostics)?;
        if value
            .schema_validation
            .iter()
            .any(|validation| validation.status == SchemaStatus::Rejected)
        {
            diagnostics.push(format!(
                "{context} value {} contains rejected schema validation evidence",
                value.ref_text
            ));
        }
        let Ok(parsed) = Reference::parse(&value.ref_text) else {
            diagnostics.push(format!(
                "{context} value {:?} is not a valid ref",
                value.ref_text
            ));
            continue;
        };
        if parsed.kind() != Kind::Port || parsed.is_qualified() {
            diagnostics.push(format!(
                "{context} value {} must be a local port:* ref",
                value.ref_text
            ));
            continue;
        }
        let Some(port) = trait_ref
            .ports
            .iter()
            .find(|port| port.id == parsed.id() && matches!(port.direction, PortDirection::Output))
        else {
            diagnostics.push(format!(
                "{context} value {} is not a declared output port",
                value.ref_text
            ));
            continue;
        };
        let expected_schema = runtime_schema_reference(&port.schema)?;
        if value.schema_ref != expected_schema {
            diagnostics.push(format!(
                "{context} value {} schema {:?} does not match expected {:?}",
                value.ref_text, value.schema_ref, port.schema
            ));
        }
        validate_union_value_evidence(
            trait_ref,
            value,
            &port.schema,
            false,
            context,
            diagnostics,
        )?;
        if !produced_output_ports.contains(value.ref_text.as_str()) {
            diagnostics.push(format!(
                "{context} value {} is not produced as a terminal sequence output",
                value.ref_text
            ));
        }
    }
    Ok(())
}

fn validate_union_value_evidence(
    trait_ref: &Trait,
    value: &Value,
    expected_schema: &str,
    require_accepted: bool,
    context: &str,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    if !matches!(
        crate::schema::form::Schema::try_from_str(expected_schema),
        Ok(crate::schema::form::Schema::Union(_))
    ) {
        return Ok(());
    }

    let expected_validation =
        validate_value_schema(trait_ref, &value.ref_text, expected_schema, &value.value)?;
    let valid_status = if require_accepted {
        expected_validation.status == SchemaStatus::Accepted
    } else {
        expected_validation.status != SchemaStatus::Rejected
    };
    if !valid_status {
        diagnostics.push(format!(
            "{context} value {} does not satisfy expected union schema {:?}: {}",
            value.ref_text, expected_schema, expected_validation.reason
        ));
    }
    if !value.schema_validation.contains(&expected_validation) {
        diagnostics.push(format!(
            "{context} value {} is missing current union schema validation evidence for {:?}",
            value.ref_text, expected_schema
        ));
    }
    Ok(())
}

fn validate_accepted_sequence_outputs(
    sequence_contract: &SequenceContract,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    for status in ledger.sequence_statuses.iter().filter(|status| {
        status.status == SequenceStatusKind::Accepted && status.position_path.is_empty()
    }) {
        let Some(item) = sequence_contract.by_declaration.get(&status.sequence_index) else {
            diagnostics.push(format!(
                "accepted sequence run {}/source {} is not declared by the current procedure",
                status.run_index, status.sequence_index
            ));
            continue;
        };
        for output_ref in item.output_refs.iter().collect::<BTreeSet<_>>() {
            let accepted =
                Reference::parse(output_ref)
                    .ok()
                    .is_some_and(|parsed| match parsed.kind() {
                        Kind::Slot => accepted_evidence.slots.contains_key(output_ref.as_str()),
                        Kind::Port => accepted_evidence
                            .output_ports
                            .contains_key(output_ref.as_str()),
                        _ => false,
                    });
            if !accepted {
                diagnostics.push(format!(
                    "accepted sequence run {}/source {}{} is missing accepted output {}",
                    item.run_index,
                    item.declaration_index,
                    item.item_id
                        .as_ref()
                        .map(|id| format!(" item {id}"))
                        .unwrap_or_default(),
                    output_ref
                ));
            }
            if item.executable
                && Reference::parse(output_ref)
                    .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
            {
                let revisions = ledger
                    .slot_revisions
                    .iter()
                    .filter(|revision| {
                        !revision.runtime_binding
                            && revision.slot_ref.as_str() == output_ref.as_str()
                            && revision.position_path.len() == 1
                            && revision.position_path.first().is_some_and(|segment| {
                                segment.kind == "procedure" && segment.index == status.run_index
                            })
                    })
                    .count();
                if revisions != 1 {
                    diagnostics.push(format!(
                        "accepted sequence run {}/source {} output {} has {revisions} revisions, expected one",
                        item.run_index, item.declaration_index, output_ref
                    ));
                }
            }
        }
    }
}

fn validate_accepted_slot_producer_statuses(
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    for slot_ref in accepted_evidence.slots.keys() {
        let producers = sequence_contract
            .slot_producers
            .get(slot_ref)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let has_accepted_producer = producers.iter().any(|producer| {
            status_maps
                .by_declaration
                .get(&producer.declaration_index)
                .is_some_and(|status| status.status == SequenceStatusKind::Accepted)
        });
        if !has_accepted_producer {
            let has_revision = recorded_slot_revisions(ledger).into_iter().any(|revision| {
                revision.slot_ref.as_str() == slot_ref.as_str()
                    && !revision.position_path.is_empty()
            });
            if !has_revision {
                diagnostics.push(format!(
                    "accepted slot {slot_ref} has no accepted producer sequence; producer statuses: {}",
                    producer_statuses(producers, status_maps)
                ));
            }
        }
    }
}

fn producer_statuses(
    producers: &[SequenceProducer],
    status_maps: &SequenceStatusMaps<'_>,
) -> String {
    if producers.is_empty() {
        return "none".to_string();
    }
    producers
        .iter()
        .map(|producer| {
            let status = status_maps
                .by_declaration
                .get(&producer.declaration_index)
                .map(|status| format!("{:?}", status.status))
                .unwrap_or_else(|| "Absent".to_string());
            format!(
                "run {}/source {}{} {status}",
                producer.run_index,
                producer.declaration_index,
                producer
                    .item_id
                    .as_ref()
                    .map(|id| format!(" item {id}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_value_digest(
    value: &Value,
    label: &str,
    diagnostics: &mut Vec<String>,
) -> crate::Result<()> {
    let digest = value_digest(&value.value)?;
    if value.value_digest != digest {
        diagnostics.push(format!(
            "{label} value {} digest {:?} does not match recomputed {:?}",
            value.ref_text, value.value_digest, digest
        ));
    }
    Ok(())
}

fn validate_emitted_signals(
    trait_ref: &Trait,
    sequence_contract: &SequenceContract,
    status_maps: &SequenceStatusMaps<'_>,
    accepted_evidence: &AcceptedEvidenceMaps<'_>,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let declared_local_signals: BTreeSet<&str> = trait_ref
        .signals
        .iter()
        .map(|signal| signal.id.as_str())
        .collect();
    validate_emitted_signal_collection(
        trait_ref,
        sequence_contract,
        ledger,
        &declared_local_signals,
        &ledger.emitted_signals,
        "committed ledger",
        false,
        diagnostics,
    );
    for (index, buffer) in recorded_effect_buffers(ledger).into_iter().enumerate() {
        validate_emitted_signal_collection(
            trait_ref,
            sequence_contract,
            ledger,
            &declared_local_signals,
            &buffer.emitted_signals,
            &format!("parallel effect buffer[{index}]"),
            true,
            diagnostics,
        );
    }

    for signal in accepted_evidence.signals.values() {
        if !signal.position_path.is_empty() || signal.source == Some(SignalSource::RuntimeControl) {
            continue;
        }
        let Some(status) = status_maps.by_declaration.get(&signal.sequence_index) else {
            diagnostics.push(format!(
                "emitted signal {} belongs to unknown sequence source {}",
                signal.signal_ref, signal.sequence_index
            ));
            continue;
        };
        if status.status != SequenceStatusKind::Accepted {
            diagnostics.push(format!(
                "emitted signal {} belongs to non-accepted sequence source {} status {:?}",
                signal.signal_ref, signal.sequence_index, status.status
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_emitted_signal_collection(
    trait_ref: &Trait,
    sequence_contract: &SequenceContract,
    ledger: &State,
    declared_local_signals: &BTreeSet<&str>,
    signals: &[SignalEmission],
    context: &str,
    check_duplicates: bool,
    diagnostics: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for signal in signals {
        let key = (
            signal.sequence_index,
            signal.signal_ref.as_str(),
            signal.evidence_digest.as_str(),
            format_path(&signal.position_path),
        );
        if check_duplicates && !seen.insert(key) {
            diagnostics.push(format!(
                "{context} contains duplicate emitted signal {} at source {} with evidence {}",
                signal.signal_ref, signal.sequence_index, signal.evidence_digest
            ));
        }
        if signal.acceptance != AcceptanceStatus::Accepted {
            diagnostics.push(format!(
                "emitted signal {} is not marked accepted",
                signal.signal_ref
            ));
        }
        if signal.evidence_digest.trim().is_empty() {
            diagnostics.push(format!(
                "emitted signal {} has an empty evidence digest",
                signal.signal_ref
            ));
        }
        if signal.source != Some(SignalSource::RuntimeControl) {
            if signal.position_path.is_empty() {
                let Some(item) = sequence_contract.by_declaration.get(&signal.sequence_index)
                else {
                    diagnostics.push(format!(
                        "emitted signal {} references unknown sequence index {}",
                        signal.signal_ref, signal.sequence_index
                    ));
                    continue;
                };
                if !item.emits.contains(signal.signal_ref.as_str()) {
                    diagnostics.push(format!(
                        "emitted signal {} is not listed in sequence item emits",
                        signal.signal_ref
                    ));
                }
            } else if !nested_signal_allowed(trait_ref, signal) {
                diagnostics.push(format!(
                    "nested emitted signal {} is not listed in the nested sequence item emits",
                    signal.signal_ref
                ));
            }
        } else if !runtime_control_signal_allowed(trait_ref, ledger, signal) {
            diagnostics.push(format!(
                "runtime-control signal {} is not declared by the referenced control item",
                signal.signal_ref
            ));
        }
        let Ok(parsed) = Reference::parse(&signal.signal_ref) else {
            diagnostics.push(format!(
                "emitted signal {:?} is not a valid signal ref",
                signal.signal_ref
            ));
            continue;
        };
        let is_canonical_stop_if_matched = signal.source == Some(SignalSource::RuntimeControl)
            && signal.signal_ref.as_str() == stop_if_matched_signal_ref().as_str();
        if parsed.kind() != Kind::Signal {
            diagnostics.push(format!(
                "emitted signal {} must use signal:* kind",
                signal.signal_ref
            ));
        } else if !is_canonical_stop_if_matched
            && !parsed.is_qualified()
            && !declared_local_signals.contains(parsed.id())
        {
            diagnostics.push(format!(
                "emitted signal {} is not declared locally",
                signal.signal_ref
            ));
        }
    }
}

/// True when the given `ExhaustionTarget` field (`on-exhausted` or
/// `on-stop`) names `signal_ref` among its declared signals. One shared
/// check for both loop-terminal signal declarations, since each is exactly
/// "is this ref among the target's `signals()`".
fn declares_signal(target: Option<&ExhaustionTarget>, signal_ref: &str) -> bool {
    target.is_some_and(|target| target.signals().iter().any(|s| s == signal_ref))
}

fn runtime_control_signal_allowed(
    trait_ref: &Trait,
    ledger: &State,
    signal: &SignalEmission,
) -> bool {
    if let Some(identity) = signal.runtime_control.as_ref() {
        return runtime_control_item_from_identity_at_path(
            trait_ref,
            ledger,
            &signal.position_path,
            identity,
        )
        .is_some_and(|item| {
            item.on_complete.as_deref() == Some(signal.signal_ref.as_str())
                || item.on_failure.as_ref().and_then(FailureTarget::signal_ref) == Some(signal.signal_ref.as_str())
                || (item.stop_if.is_some()
                    && item.on_stop.is_none()
                    && signal.signal_ref.as_str() == stop_if_matched_signal_ref().as_str())
                || declares_signal(item.on_exhausted.as_ref(), signal.signal_ref.as_str())
                || declares_signal(item.on_stop.as_ref(), signal.signal_ref.as_str())
        });
    }
    if signal.position_path.is_empty() {
        return trait_ref
            .procedure
            .as_ref()
            .and_then(|procedure| procedure.sequence.get(signal.sequence_index))
            .is_some_and(|item| {
                item.on_complete.as_deref() == Some(signal.signal_ref.as_str())
                    || item.on_failure.as_ref().and_then(FailureTarget::signal_ref) == Some(signal.signal_ref.as_str())
                    || declares_signal(item.on_exhausted.as_ref(), signal.signal_ref.as_str())
            });
    }
    if ledger.failure_routes.iter().any(|route| {
        route.signal.as_deref() == Some(signal.signal_ref.as_str())
            && route.position_path == signal.position_path
    }) {
        return true;
    }
    let sequence = trait_ref
        .procedure
        .as_ref()
        .and_then(|procedure| effective_sequence_items(procedure).ok());
    if let Some(sequence) = sequence
        && item_at_execution_path(trait_ref, &sequence, ledger, &signal.position_path)
            .is_some_and(|item| {
                matches!(
                    item.on_failure.as_ref(),
                    Some(FailureTarget::Signal(declared)) if declared == signal.signal_ref.as_str()
                )
            })
    {
        return true;
    }
    false
}

fn nested_signal_allowed(trait_ref: &Trait, signal: &SignalEmission) -> bool {
    let Some(item_segment) = signal
        .position_path
        .iter()
        .rev()
        .find(|segment| segment.kind == "item")
    else {
        return false;
    };
    let Some(sequence_segment) = signal
        .position_path
        .iter()
        .rev()
        .find(|segment| {
            matches!(
                segment.kind.as_str(),
                "sequence" | "branch" | "loop" | "for-each" | "parallel"
            )
        })
    else {
        return false;
    };
    let Some(sequence_id) = sequence_segment.id.as_deref() else {
        return false;
    };
    trait_ref
        .sequences
        .get(sequence_id)
        .and_then(|sequence| sequence.sequence.get(item_segment.index))
        .is_some_and(|item| {
            item.emits
                .iter()
                .any(|item_signal| item_signal.signal_ref() == signal.signal_ref.as_str())
        })
}
