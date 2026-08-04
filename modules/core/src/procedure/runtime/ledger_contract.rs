// Procedure runtime ledger contract.
// Procedure runtime ledger contracts.

fn validate_sequence_status_contract(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let statuses = ledger.sequence_statuses.clone();
    let mut outer_statuses: Vec<_> = statuses
        .iter()
        .filter(|status| status.position_path.is_empty())
        .cloned()
        .collect();
    outer_statuses.sort_by_key(|status| status.run_index);
    if outer_statuses.len() > sequence.len() {
        diagnostics.push(format!(
            "outer sequence-status count {} exceeds procedure sequence count {}",
            outer_statuses.len(),
            sequence.len()
        ));
        return;
    }
    for actual in &outer_statuses {
        let Some(expected) = sequence
            .iter()
            .find(|item| item.run_index == actual.run_index)
        else {
            diagnostics.push(format!(
                "sequence-status run-index {} does not match any procedure item",
                actual.run_index
            ));
            continue;
        };
        if actual.run_index != expected.run_index {
            diagnostics.push(format!(
                "sequence-status run-index {} does not match expected {}",
                actual.run_index, expected.run_index
            ));
        }
        if actual.sequence_index != expected.declaration_index {
            diagnostics.push(format!(
                "sequence-status source index {} does not match expected {}",
                actual.sequence_index, expected.declaration_index
            ));
        }
        if actual.item_id.as_ref() != expected.item.id.as_ref() {
            diagnostics.push(format!(
                "sequence-status item-id {:?} does not match expected {:?}",
                actual.item_id.as_ref(),
                expected.item.id.as_ref()
            ));
        }
        if actual.title.as_str() != expected.item.title.as_str() {
            diagnostics.push(format!(
                "sequence-status title {:?} does not match expected {:?}",
                actual.title, expected.item.title
            ));
        }
    }
    for status in statuses
        .iter()
        .filter(|status| !status.position_path.is_empty())
    {
        let is_unmatched_ask = status.status == SequenceStatusKind::Skipped
            && item_at_execution_path(trait_ref, sequence, ledger, &status.position_path)
                .is_some_and(|item| item.effective_kind() == SequenceKind::Ask);
        if matches!(status.status, SequenceStatusKind::Routed | SequenceStatusKind::Skipped)
            && !is_unmatched_ask
        {
            diagnostics.push(format!(
                "nested sequence-status for source index {} must not be routed or skipped",
                status.sequence_index
            ));
        }
        if !status
            .position_path
            .iter()
            .any(|segment| segment.kind == "item")
        {
            diagnostics.push(format!(
                "nested sequence-status for source index {} has no item segment",
                status.sequence_index
            ));
        }
    }
}

fn validate_current_run_index(
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    if ledger.current_run_index > sequence.len() {
        diagnostics.push(format!(
            "current-run-index {} exceeds sequence length {}",
            ledger.current_run_index,
            sequence.len()
        ));
    }
}

fn validate_control_state(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let mut failure_routes = BTreeSet::new();
    for route in &ledger.failure_routes {
        let key = (
            route.source_run_index,
            serde_json::to_string(&route.position_path).unwrap_or_default(),
        );
        if !failure_routes.insert(key) {
            diagnostics.push(format!(
                "duplicate failure route from run {} to {}",
                route.source_run_index, route.target_run_index
            ));
        }
        let source = sequence.iter().find(|item| item.run_index == route.source_run_index);
        let target = sequence.iter().find(|item| item.run_index == route.target_run_index);
        let declared_source = failure_route_source_at_path(trait_ref, sequence, ledger, route);
        if declared_source.is_none() {
            diagnostics.push(format!("failure route source {:?} does not match declared step", route.source_step_id));
        }
        if target.is_none_or(|item| item.item.id.as_deref() != Some(route.target_step_id.as_str())) {
            diagnostics.push(format!("failure route target {:?} does not match declared step", route.target_step_id));
        }
        if route.target_run_index <= route.source_run_index {
            diagnostics.push(format!("failure route {:?} is not forward-only", route.target_step_id));
        }
        if !outer_run_was_reached(ledger, route.target_run_index) {
            diagnostics.push(format!(
                "failure route target {:?} was not reached",
                route.target_step_id
            ));
        }
        let declared = declared_source
            .and_then(|item| item.on_failure.as_ref().and_then(FailureTarget::route));
        if declared.is_none_or(|declared| {
            declared.step != route.target_step_id || declared.signal != route.signal
        }) {
            diagnostics.push(format!(
                "failure route source {:?} does not declare its recorded target or signal",
                route.source_step_id
            ));
        }
        if !ledger.sequence_statuses.iter().any(|status| {
            status.position_path.is_empty()
                && status.run_index == route.source_run_index
                && status.status == SequenceStatusKind::Routed
        }) {
            diagnostics.push(format!("failure route source {:?} is not marked routed", route.source_step_id));
        }
        for run_index in route.source_run_index.saturating_add(1)..route.target_run_index {
            if !ledger.sequence_statuses.iter().any(|status| {
                status.position_path.is_empty()
                    && status.run_index == run_index
                    && status.status == SequenceStatusKind::Skipped
            }) {
                diagnostics.push(format!(
                    "failure route to {:?} does not mark bypassed run {} skipped",
                    route.target_step_id, run_index
                ));
            }
        }
        if let Some(signal) = route.signal.as_deref()
            && !recorded_emitted_signals(ledger).into_iter().any(|emission| {
                emission.signal_ref.as_str() == signal && emission.position_path == route.position_path
            })
        {
            diagnostics.push(format!("failure route {:?} is missing its signal emission", route.source_step_id));
        }
        if let Some(source) = source
            && route.position_path.first().is_none_or(|segment| {
                segment.kind != "procedure"
                    || segment.id != source.item.id
                    || segment.index != source.run_index
                    || segment.iteration.is_some()
                    || segment.item_index.is_some()
            }) {
                diagnostics.push(format!(
                    "failure route {:?} position-path does not bind its source procedure item",
                    route.source_step_id
                ));
            }
    }
    for status in &ledger.sequence_statuses {
        if status.position_path.is_empty() && status.status == SequenceStatusKind::Routed
            && !ledger.failure_routes.iter().any(|route| route.source_run_index == status.run_index)
        {
            diagnostics.push(format!(
                "routed sequence status at run {} is missing its failure-route receipt",
                status.run_index
            ));
        }
        if status.position_path.is_empty() && status.status == SequenceStatusKind::Skipped
            && !sequence.iter().any(|item| {
                item.run_index == status.run_index && item.item.effective_kind() == SequenceKind::Ask
            })
            && !ledger.failure_routes.iter().any(|route| {
                route.source_run_index < status.run_index && status.run_index < route.target_run_index
            })
        {
            diagnostics.push(format!(
                "skipped sequence status at run {} is not justified by a failure-route receipt",
                status.run_index
            ));
        }
    }
    let mut branch_decisions = BTreeSet::new();
    for decision in &ledger.branch_decisions {
        let key = (
            decision.parent_run_index,
            decision.branch_id.as_str(),
            serde_json::to_string(&decision.position_path).unwrap_or_default(),
        );
        if !branch_decisions.insert(key) {
            diagnostics.push(format!(
                "duplicate branch decision for parent-run-index {} branch {:?} activation",
                decision.parent_run_index, decision.branch_id
            ));
        }
        if !matches!(decision.selected_arm.as_str(), "then" | "otherwise" | "none") {
            diagnostics.push(format!(
                "branch decision {:?} has invalid selected arm {:?}",
                decision.branch_id, decision.selected_arm
            ));
        }
        if (decision.selected_arm == "none") != decision.sequence_id.is_none() {
            diagnostics.push(format!(
                "branch decision {:?} has inconsistent selected arm and sequence", decision.branch_id
            ));
        }
        if (decision.selected_arm == "then") != decision.matched {
            diagnostics.push(format!(
                "branch decision {:?} guard result does not match selected arm", decision.branch_id
            ));
        }
        let declared_branch = branch_at_decision_path(trait_ref, sequence, ledger, decision);
        let Some(branch) = declared_branch else {
            diagnostics.push(format!("branch decision {:?} does not resolve to a declared branch", decision.branch_id));
            continue;
        };
        if decision.when.as_ref() != branch.when.as_ref() {
            diagnostics.push(format!("branch decision {:?} guard does not match its declaration", decision.branch_id));
        }
        if decision.position_path.len() == 1
            && !top_level_branch_was_entered(ledger, decision)
        {
            diagnostics.push(format!(
                "branch decision {:?} does not belong to an entered top-level branch",
                decision.branch_id
            ));
        }
        if !decision_path_is_bound(trait_ref, sequence, ledger, decision) {
            diagnostics.push(format!(
                "branch decision {:?} position-path does not bind its branch activation",
                decision.branch_id
            ));
        }
        let expected_sequence = match decision.selected_arm.as_str() {
            "then" => branch.sequence.as_deref(),
            "otherwise" => branch.otherwise.as_deref(),
            "none" => None,
            _ => None,
        }
        .and_then(|reference| sequence_id_from_ref(reference).ok());
        if decision.sequence_id != expected_sequence {
            diagnostics.push(format!("branch decision {:?} selected sequence does not match its declared arm", decision.branch_id));
        }
        if decision.selected_arm == "none" && branch.otherwise.is_some() {
            diagnostics.push(format!("branch decision {:?} selected no arm despite an otherwise arm", decision.branch_id));
        }
        validate_branch_guard_decision(trait_ref, ledger, decision, branch, diagnostics);
    }
    for item in sequence {
        if item.item.effective_kind() != SequenceKind::Branch || item.run_index >= ledger.current_run_index {
            continue;
        }
        if ledger.sequence_statuses.iter().any(|status| {
            status.position_path.is_empty()
                && status.run_index == item.run_index
                && status.status == SequenceStatusKind::Skipped
        }) {
            continue;
        }
        if !ledger.branch_decisions.iter().any(|decision| {
            decision.parent_run_index == item.run_index
                && decision.branch_id == item.item.id.as_deref().unwrap_or_default()
                && decision_path_is_bound(trait_ref, sequence, ledger, decision)
        }) {
            diagnostics.push(format!(
                "entered branch {:?} at run {} is missing its immutable decision",
                item.item.id, item.run_index
            ));
        }
    }
    for status in ledger
        .sequence_statuses
        .iter()
        .filter(|status| !status.position_path.is_empty())
    {
        let Some(branch) = branch_at_status_path(trait_ref, sequence, ledger, status) else {
            continue;
        };
        if !ledger.branch_decisions.iter().any(|decision| {
            decision.parent_run_index == status.run_index
                && decision.branch_id == branch.id.as_deref().unwrap_or_default()
                && decision.position_path == status.position_path
                && decision_path_is_bound(trait_ref, sequence, ledger, decision)
        }) {
            diagnostics.push(format!(
                "entered nested branch {:?} at run {} is missing its immutable decision",
                branch.id, status.run_index
            ));
        }
    }
    for status in ledger
        .sequence_statuses
        .iter()
        .filter(|status| !status.position_path.is_empty())
    {
        if item_at_execution_path(trait_ref, sequence, ledger, &status.position_path)
            .is_none_or(|item| {
                item.id != status.item_id
                    || status.position_path.last().is_none_or(|segment| {
                        segment.kind != "item" || segment.index != status.sequence_index
                    })
            })
        {
            diagnostics.push(format!(
                "nested sequence-status for source index {} does not follow the selected structural path",
                status.sequence_index
            ));
        }
    }
    for signal in recorded_emitted_signals(ledger)
        .into_iter()
        .filter(|signal| !signal.position_path.is_empty())
    {
        if item_at_execution_path(trait_ref, sequence, ledger, &signal.position_path).is_none() {
            diagnostics.push(format!(
                "emitted signal {} does not follow the selected structural path",
                signal.signal_ref
            ));
        }
    }
    if ledger.control_stack.len() > MAX_SEQUENCE_NESTING_DEPTH {
        diagnostics.push(format!(
            "control-stack depth {} exceeds maximum {}",
            ledger.control_stack.len(),
            MAX_SEQUENCE_NESTING_DEPTH
        ));
    }
    for (index, frame) in ledger.control_stack.iter().enumerate() {
        if frame.kind != ControlKind::Parallel
            && (!frame.parallel_buffer.is_empty()
                || !frame.parallel_committed_branches.is_empty())
        {
            diagnostics.push(format!(
                "control-stack[{index}] non-parallel frame must not carry parallel effect buffers"
            ));
        }
        let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
            diagnostics.push(format!(
                "control-stack[{index}] references unknown sequence {:?}",
                frame.sequence_id
            ));
            continue;
        };
        if frame.parent_run_index >= sequence.len() {
            diagnostics.push(format!(
                "control-stack[{index}] parent-run-index {} exceeds sequence length {}",
                frame.parent_run_index,
                sequence.len()
            ));
        }
        if frame.next_index > named.sequence.len() {
            diagnostics.push(format!(
                "control-stack[{index}] next-index {} exceeds sequence {:?} length {}",
                frame.next_index,
                frame.sequence_id,
                named.sequence.len()
            ));
        }
        // P402: `concurrent` is purely CLI/IO-layer evidence (the core
        // runtime always advances `for-each` items one at a time), but a
        // hand-edited ledger must never be able to forge speculative-dispatch
        // license the trait's own author never granted. Cross-check the
        // persisted flag against the resolved canonical for-each item's
        // authored `concurrent` value (`false` for every other control-frame
        // kind, matching how a fresh frame is built — see `concurrent: kind
        // == SequenceKind::ForEach && item.concurrent` above in this module).
        let declared_concurrent = frame.kind == ControlKind::ForEach
            && control_item_for_stack_frame(trait_ref, sequence, &ledger.control_stack, index)
                .is_some_and(|item| item.concurrent);
        if frame.concurrent != declared_concurrent {
            diagnostics.push(format!(
                "control-stack[{index}] concurrent={} does not match its declared for-each concurrent intent {declared_concurrent}",
                frame.concurrent
            ));
        }
        match frame.kind {
            ControlKind::Sequence => {
                if frame.iteration_index.is_some() || frame.item_index.is_some() {
                    diagnostics.push(format!(
                        "control-stack[{index}] sequence frame must not carry iteration or item indexes"
                    ));
                }
            }
            ControlKind::Branch => {
                let Some(branch_id) = frame.control_item_id.as_deref() else {
                    diagnostics.push(format!("control-stack[{index}] branch frame missing id"));
                    continue;
                };
                let activation_path = path_for_control_frame_activation(ledger, index);
                if !ledger.branch_decisions.iter().any(|decision| {
                    decision.parent_run_index == frame.parent_run_index
                        && decision.branch_id == branch_id
                        && decision.position_path == activation_path
                        && decision.sequence_id.as_deref() == Some(frame.sequence_id.as_str())
                        && decision_path_is_bound(trait_ref, sequence, ledger, decision)
                }) {
                    diagnostics.push(format!(
                        "control-stack[{index}] branch frame has no matching branch decision"
                    ));
                }
            }
            ControlKind::Loop => {
                let Some(max_iterations) = frame.max_iterations else {
                    diagnostics.push(format!(
                        "control-stack[{index}] loop frame missing max-iterations"
                    ));
                    continue;
                };
                let Some(iteration) = frame.iteration_index else {
                    diagnostics.push(format!(
                        "control-stack[{index}] loop frame missing iteration-index"
                    ));
                    continue;
                };
                if max_iterations == 0 {
                    diagnostics.push(format!(
                        "control-stack[{index}] loop max-iterations must be greater than zero"
                    ));
                }
                if iteration >= max_iterations {
                    diagnostics.push(format!(
                        "control-stack[{index}] iteration-index {} is not less than max-iterations {}",
                        iteration, max_iterations
                    ));
                }
                let declared_bound = control_item_for_stack_frame(
                    trait_ref,
                    sequence,
                    &ledger.control_stack,
                    index,
                )
                .and_then(|item| resolved_loop_bound(item, ledger));
                if declared_bound != Some(max_iterations) {
                    diagnostics.push(format!(
                        "control-stack[{index}] frozen max-iterations {max_iterations} does not match its resolved declaration {:?}",
                        declared_bound
                    ));
                }
            }
            ControlKind::ForEach => {
                let Some(item_total) = frame.item_total else {
                    diagnostics.push(format!(
                        "control-stack[{index}] for-each frame missing item-total"
                    ));
                    continue;
                };
                let Some(item_index) = frame.item_index else {
                    diagnostics.push(format!(
                        "control-stack[{index}] for-each frame missing item-index"
                    ));
                    continue;
                };
                let Some(max_items) = frame.max_items else {
                    diagnostics.push(format!(
                        "control-stack[{index}] for-each frame missing max-items"
                    ));
                    continue;
                };
                if max_items == 0 {
                    diagnostics.push(format!(
                        "control-stack[{index}] for-each max-items must be greater than zero"
                    ));
                }
                if item_total > max_items {
                    diagnostics.push(format!(
                        "control-stack[{index}] item-total {} exceeds max-items {}",
                        item_total, max_items
                    ));
                }
                if item_index >= item_total || item_index >= max_items {
                    diagnostics.push(format!(
                        "control-stack[{index}] item-index {} is outside item-total {} / max-items {}",
                        item_index, item_total, max_items
                    ));
                }
                if let Some(over_slot) = frame.over_slot.as_deref() {
                    match accepted_value(ledger, over_slot) {
                        Some(value) => {
                            match value.value.as_array() {
                                Some(items) if items.len() == item_total => {}
                                Some(items) => diagnostics.push(format!(
                                    "control-stack[{index}] item-total {} does not match accepted list length {}",
                                    item_total,
                                    items.len()
                                )),
                                None => diagnostics.push(format!(
                                    "control-stack[{index}] over slot {over_slot:?} is not an accepted list value"
                                )),
                            }
                            match frame.list_digest.as_deref() {
                                Some(digest) if digest == value.value_digest.as_str() => {}
                                Some(digest) => diagnostics.push(format!(
                                    "control-stack[{index}] list-digest {digest:?} does not match accepted over-slot digest {:?}",
                                    value.value_digest
                                )),
                                None => diagnostics.push(format!(
                                    "control-stack[{index}] for-each frame missing list-digest"
                                )),
                            }
                        }
                        None => diagnostics.push(format!(
                            "control-stack[{index}] over slot {over_slot:?} is not an accepted list value"
                        )),
                    }
                }
            }
            ControlKind::Parallel => {
                let Some(branch_total) = frame.max_iterations else {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel frame missing max-iterations"
                    ));
                    continue;
                };
                let Some(branch_index) = frame.iteration_index else {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel frame missing iteration-index"
                    ));
                    continue;
                };
                if branch_total == 0 {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel max-iterations must be greater than zero"
                    ));
                }
                if branch_index >= branch_total {
                    diagnostics.push(format!(
                        "control-stack[{index}] branch-index {} is not less than branch count {}",
                        branch_index, branch_total
                    ));
                }
                if frame.parallel_branch_sequence_ids.len() != branch_total {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel-branch-sequence-ids length {} does not match max-iterations {}",
                        frame.parallel_branch_sequence_ids.len(),
                        branch_total
                    ));
                }
                // A `skip` branch-failure policy (P264) discards its buffer
                // instead of pushing it, so `parallel-committed-branches`
                // holds one entry per branch strictly before `branch_index`
                // whose resolved outcome is not `skipped` — not simply
                // `branch_index` itself.
                let skipped_before_branch_index = frame
                    .parallel_branch_outcomes
                    .iter()
                    .take(branch_index)
                    .filter(|outcome| matches!(outcome, Some(ParallelBranchOutcome::Skipped)))
                    .count();
                let expected_committed = branch_index.saturating_sub(skipped_before_branch_index);
                if frame.parallel_committed_branches.len() != expected_committed {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel-committed-branches length {} does not match expected {} ({} branches before branch-index {branch_index}, {skipped_before_branch_index} skipped)",
                        frame.parallel_committed_branches.len(),
                        expected_committed,
                        branch_index,
                    ));
                }
                if !frame.parallel_branch_outcomes.is_empty()
                    && frame.parallel_branch_outcomes.len() != branch_total
                {
                    diagnostics.push(format!(
                        "control-stack[{index}] parallel-branch-outcomes length {} does not match branch count {}",
                        frame.parallel_branch_outcomes.len(),
                        branch_total
                    ));
                }
                if frame
                    .parallel_branch_sequence_ids
                    .get(branch_index)
                    .is_some_and(|expected| expected != &frame.sequence_id)
                {
                    diagnostics.push(format!(
                        "control-stack[{index}] sequence-id {:?} does not match branch {} of parallel-branch-sequence-ids",
                        frame.sequence_id, branch_index
                    ));
                }
            }
        }
    }

    validate_active_path_contract(trait_ref, ledger, diagnostics);
    validate_guard_evaluations_contract(ledger, diagnostics);
    validate_parallel_panel_records_contract(ledger, diagnostics);

    let mut seen_orders = BTreeSet::new();
    let mut seen_activation_writes = BTreeSet::new();
    let mut evidence_started_by_slot = BTreeSet::new();
    let recorded_revisions = recorded_slot_revisions(ledger);
    for (index, revision) in recorded_revisions.iter().copied().enumerate() {
        if revision.slot_ref.trim().is_empty() || revision.value_digest.trim().is_empty() {
            diagnostics.push(format!(
                "slot-revisions[{index}] must carry non-empty slot-ref and value-digest"
            ));
        }
        if !seen_orders.insert(revision.acceptance_order) {
            diagnostics.push(format!(
                "slot-revisions[{index}] duplicates acceptance-order {}",
                revision.acceptance_order
            ));
        }
        if revision.acceptance_order == 0 {
            diagnostics.push(format!(
                "slot-revisions[{index}] acceptance-order must be greater than zero"
            ));
        }
        if revision.acceptance_order != index + 1 {
            diagnostics.push(format!(
                "slot-revisions[{index}] acceptance-order {} does not match append position {}",
                revision.acceptance_order,
                index + 1
            ));
        }
        if Reference::parse(&revision.slot_ref)
            .ok()
            .is_none_or(|parsed| parsed.kind() != Kind::Slot || parsed.is_qualified())
        {
            diagnostics.push(format!(
                "slot-revisions[{index}] slot-ref {:?} is not a local slot ref",
                revision.slot_ref
            ));
        }
        if revision.position_path.is_empty() {
            diagnostics.push(format!(
                "slot-revisions[{index}] must carry a producer position-path"
            ));
        } else {
            let producer = item_at_execution_path(
                trait_ref,
                sequence,
                ledger,
                &revision.position_path,
            );
            let for_each_binding =
                revision_matches_for_each_binding(trait_ref, sequence, ledger, revision);
            if producer.is_none() {
                diagnostics.push(format!(
                    "slot-revisions[{index}] does not follow the selected structural path"
                ));
            } else if producer.is_none_or(|item| {
                !item.output.ref_texts().any(|output| output == revision.slot_ref.as_str())
            }) && !for_each_binding
            {
                diagnostics.push(format!(
                    "slot-revisions[{index}] slot-ref {} is not produced by its selected structural path",
                    revision.slot_ref
                ));
            } else if !for_each_binding
                && !revision_has_accepted_producer_activation(ledger, revision)
            {
                diagnostics.push(format!(
                    "slot-revisions[{index}] has no accepted producer activation"
                ));
            }
        }
        validate_revision_evidence(
            RevisionValidationContext {
                trait_ref,
                sequence,
                ledger,
                latest_digest: latest_visible_slot_revision_before(ledger, revision)
                    .map(|prior| &prior.value_digest),
            },
            index,
            revision,
            &mut evidence_started_by_slot,
            diagnostics,
        );
        let activation_key = (
            revision.slot_ref.clone(),
            format_path(&revision.position_path),
            revision.loop_id.clone(),
            revision.iteration_index,
            revision.for_each_id.clone(),
            revision.item_index,
        );
        if !seen_activation_writes.insert(activation_key) {
            diagnostics.push(format!(
                "slot-revisions[{index}] duplicates a slot write in the same scope activation"
            ));
        }
    }

    validate_current_slot_revisions(
        &ledger.accepted_slot_values,
        &ledger.slot_revisions,
        "committed ledger",
        false,
        diagnostics,
    );
    for (index, buffer) in recorded_effect_buffers(ledger).into_iter().enumerate() {
        validate_current_slot_revisions(
            &buffer.accepted_slot_values,
            &buffer.slot_revisions,
            &format!("parallel effect buffer[{index}]"),
            true,
            diagnostics,
        );
    }
}

/// Validate the immutable P290 guard-conditioned resource-input inclusion
/// decisions: each must resolve to a declared guarded input at its declared
/// sequence index, replay its declared guard through the exact same
/// range/watermark/replay machinery [`validate_branch_guard_decision`] uses
/// for branch selection, and appear at most once per (sequence-index, ref,
/// position). This is what stops a hand-edited ledger from claiming a
/// matched or unmatched inclusion the declared guard would not itself
/// produce. [`validate_conditional_input_decision_completeness`] closes the
/// remaining gap: a reached guarded step whose decision (and evidence) were
/// deleted wholesale, rather than tampered with, would otherwise pass this
/// per-decision authentication vacuously.
fn validate_conditional_input_decisions_contract(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for decision in &ledger.conditional_input_decisions {
        let key = (
            decision.sequence_index,
            decision.ref_text.as_str(),
            serde_json::to_string(&decision.position_path).unwrap_or_default(),
        );
        if !seen.insert(key) {
            diagnostics.push(format!(
                "duplicate conditional-input decision for sequence-index {} ref {:?}",
                decision.sequence_index, decision.ref_text
            ));
        }
        let Some(item) = item_at_execution_path(trait_ref, sequence, ledger, &decision.position_path)
        else {
            diagnostics.push(format!(
                "conditional-input decision {:?} does not resolve to a declared sequence item",
                decision.ref_text
            ));
            continue;
        };
        if expected_conditional_input_sequence_index(sequence, &decision.position_path)
            != Some(decision.sequence_index)
        {
            diagnostics.push(format!(
                "conditional-input decision {:?} sequence-index {} does not match its resolved position",
                decision.ref_text, decision.sequence_index
            ));
        }
        validate_conditional_input_guard_decision(trait_ref, ledger, decision, item, diagnostics);
    }
    validate_conditional_input_decision_completeness(trait_ref, sequence, ledger, diagnostics);
}

fn validate_ask_decisions_contract(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for decision in &ledger.ask_decisions {
        let key = (decision.sequence_index, serde_json::to_string(&decision.position_path).unwrap_or_default());
        if !seen.insert(key) {
            diagnostics.push(format!("duplicate ask decision for sequence-index {}", decision.sequence_index));
        }
        let Some(item) = item_at_execution_path(trait_ref, sequence, ledger, &decision.position_path) else {
            diagnostics.push("ask decision does not resolve to a declared sequence item".to_string());
            continue;
        };
        if item.effective_kind() != SequenceKind::Ask {
            diagnostics.push("ask decision does not resolve to an ask item".to_string());
            continue;
        }
        if expected_conditional_input_sequence_index(sequence, &decision.position_path) != Some(decision.sequence_index) {
            diagnostics.push("ask decision sequence-index does not match its resolved position".to_string());
        }
        let Some(guard) = item.when.as_ref() else {
            diagnostics.push("ask decision item has no declared guard".to_string());
            continue;
        };
        if guard != &decision.when {
            diagnostics.push("ask decision guard does not match its declaration".to_string());
        }
        let expected_marker = serde_json::to_string(&(decision.sequence_index, &decision.when, &decision.position_path))
            .ok()
            .map(|key| format!("ask:{key}"));
        let marker = ledger.guard_evaluations.get(decision.guard_evaluation_index);
        if marker.is_none_or(|evaluation| {
            evaluation.matched != decision.matched
                || evaluation.reason != "ask activation"
                || evaluation.comparison_evidence.is_some()
                || Some(&evaluation.predicate) != expected_marker.as_ref()
        }) {
            diagnostics.push("ask decision has no matching guard evaluation evidence".to_string());
        }
        match (decision.guard_evaluation_start_index, decision.slot_revision_watermark) {
            (Some(start), Some(watermark)) => {
                if start >= decision.guard_evaluation_index {
                    diagnostics.push("ask decision guard evaluation range is empty or reversed".to_string());
                    continue;
                }
                if watermark != expected_slot_revision_watermark_before(ledger, &decision.position_path) {
                    diagnostics.push("ask decision slot revision watermark does not match decision boundary".to_string());
                }
                let mut cursor = start;
                let replayed = replay_declared_guard(
                    GuardReplayContext {
                        trait_ref,
                        ledger,
                        label: "ask decision",
                        position_path: &decision.position_path,
                        watermark,
                        end: decision.guard_evaluation_index,
                    },
                    guard,
                    &mut cursor,
                    0,
                    &mut BTreeSet::new(),
                    diagnostics,
                );
                if cursor != decision.guard_evaluation_index || replayed.is_none_or(|outcome| outcome.routes_true() != decision.matched) {
                    diagnostics.push("ask decision does not replay from its declared guard".to_string());
                }
            }
            _ => diagnostics.push("ask decision is missing its replay range and slot revision watermark".to_string()),
        }
    }
    for status in &ledger.sequence_statuses {
        if status.status != SequenceStatusKind::Skipped {
            continue;
        }
        let position_path = if status.position_path.is_empty() {
            vec![PathSegment { kind: "procedure".to_string(), id: status.item_id.clone(), index: status.run_index, iteration: None, item_index: None }]
        } else {
            status.position_path.clone()
        };
        let is_ask = item_at_execution_path(trait_ref, sequence, ledger, &position_path)
            .is_some_and(|item| item.effective_kind() == SequenceKind::Ask);
        if is_ask && !ledger.ask_decisions.iter().any(|decision| {
            decision.sequence_index == status.sequence_index
                && decision.position_path == position_path
                && !decision.matched
        }) {
            diagnostics.push(format!("skipped ask at sequence-index {} is missing an unmatched ask decision", status.sequence_index));
        }
    }
}

/// The declared sequence index a conditional-input decision at `position_path`
/// must carry: the declaration index of the top-level item for an outer
/// (single-segment) path, or the terminal `item` segment's own index for a
/// nested path — mirroring how [`item_at_execution_path`] itself resolves the
/// item, so a decision cannot claim a sequence index unrelated to the
/// position that actually produced it.
fn expected_conditional_input_sequence_index(
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    position_path: &[PathSegment],
) -> Option<usize> {
    if position_path.len() == 1 {
        let segment = position_path.first()?;
        sequence
            .iter()
            .find(|item| item.run_index == segment.index)
            .map(|item| item.declaration_index)
    } else {
        position_path.last().map(|segment| segment.index)
    }
}

/// Every reached position (one with sequence-status `ready`, `blocked`,
/// `accepted`, or `rejected` — i.e. one that ever became the current
/// executable item, per `record_conditional_input_decisions`) whose declared
/// item carries a guarded input must own exactly one conditional-input
/// decision for that input. Deriving the required decisions from reached
/// positions, rather than only authenticating whichever decisions happen to
/// be stored, is what makes deleting an entire decision (and its guard
/// evidence) fail replay instead of silently passing.
fn validate_conditional_input_decision_completeness(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    diagnostics: &mut Vec<String>,
) {
    for status in &ledger.sequence_statuses {
        // `Routed` proves the item became the current executable item (its
        // conditional-input decision, if any, was recorded before failure
        // routing bypassed it) exactly as `Accepted`/`Rejected` do; excluding
        // it let a deleted decision for an already-reached routed source pass
        // vacuously.
        if !matches!(
            status.status,
            SequenceStatusKind::Ready
                | SequenceStatusKind::Blocked
                | SequenceStatusKind::Accepted
                | SequenceStatusKind::Rejected
                | SequenceStatusKind::Routed
        ) {
            continue;
        }
        let Some((item, position_path, expected_sequence_index)) = (if status.position_path.is_empty() {
            sequence
                .iter()
                .find(|effective| effective.run_index == status.run_index)
                .map(|effective| {
                    let position_path = vec![PathSegment {
                        kind: "procedure".to_string(),
                        id: effective.item.id.clone(),
                        index: effective.run_index,
                        iteration: None,
                        item_index: None,
                    }];
                    (effective.item, position_path, effective.declaration_index)
                })
        } else {
            item_at_execution_path(trait_ref, sequence, ledger, &status.position_path)
                .map(|item| (item, status.position_path.clone(), status.sequence_index))
        }) else {
            continue;
        };
        require_conditional_input_decisions(ledger, item, &position_path, expected_sequence_index, diagnostics);
    }
    // A failure route clears every nested `sequence_statuses` entry beneath
    // the routed control frame (`route_failure`'s `retain` above), including
    // ones for guarded-input items that were already reached and had their
    // decision recorded before the route fired. The loop above can no longer
    // see those positions, so a deleted decision for one would otherwise
    // never be missed. `failure_routes[].position-path` is the immutable
    // receipt of exactly which nested position the route abandoned, so use
    // it to require completeness there too.
    for route in &ledger.failure_routes {
        if route.position_path.is_empty() {
            continue;
        }
        let Some(item) = item_at_execution_path(trait_ref, sequence, ledger, &route.position_path) else {
            continue;
        };
        let Some(expected_sequence_index) =
            expected_conditional_input_sequence_index(sequence, &route.position_path)
        else {
            continue;
        };
        require_conditional_input_decisions(
            ledger,
            item,
            &route.position_path,
            expected_sequence_index,
            diagnostics,
        );
    }
}

fn require_conditional_input_decisions(
    ledger: &State,
    item: &crate::r#trait::procedure::SequenceItem,
    position_path: &[PathSegment],
    expected_sequence_index: usize,
    diagnostics: &mut Vec<String>,
) {
    for input in item.input.iter() {
        if input.guard().is_none() {
            continue;
        }
        let ref_text = input.ref_text();
        if !ledger.conditional_input_decisions.iter().any(|decision| {
            decision.sequence_index == expected_sequence_index
                && decision.ref_text == ref_text
                && decision.position_path == position_path
        }) {
            diagnostics.push(format!(
                "reached guarded input {ref_text:?} at sequence-index {expected_sequence_index} is missing its conditional-input decision"
            ));
        }
    }
}

fn validate_conditional_input_guard_decision(
    trait_ref: &Trait,
    ledger: &State,
    decision: &ConditionalInputDecision,
    item: &crate::r#trait::procedure::SequenceItem,
    diagnostics: &mut Vec<String>,
) {
    let label = format!("conditional-input decision {:?}", decision.ref_text);
    let Some(declared_guard) = item
        .input
        .iter()
        .find(|input| input.ref_text() == decision.ref_text)
        .and_then(|input| input.guard())
    else {
        diagnostics.push(format!(
            "{label} does not resolve to a guarded input declaration"
        ));
        return;
    };
    if declared_guard != &decision.when {
        diagnostics.push(format!("{label} guard does not match its declaration"));
    }

    let expected_marker = serde_json::to_string(&(
        decision.sequence_index,
        &decision.ref_text,
        &decision.when,
        &decision.position_path,
    ))
    .ok()
    .map(|key| format!("input:{}:{key}", decision.ref_text));
    let marker = ledger.guard_evaluations.get(decision.guard_evaluation_index);
    if marker.is_none_or(|evaluation| {
        evaluation.matched != decision.matched
            || evaluation.reason != "conditional input inclusion"
            || evaluation.comparison_evidence.is_some()
            || Some(&evaluation.predicate) != expected_marker.as_ref()
    }) {
        diagnostics.push(format!("{label} has no matching guard evaluation evidence"));
    }

    match (
        decision.guard_evaluation_start_index,
        decision.slot_revision_watermark,
    ) {
        (Some(start), Some(watermark)) => {
            if start >= decision.guard_evaluation_index {
                diagnostics.push(format!("{label} guard evaluation range is empty or reversed"));
                return;
            }
            let expected_watermark =
                expected_slot_revision_watermark_before(ledger, &decision.position_path);
            if watermark != expected_watermark {
                diagnostics.push(format!(
                    "{label} slot revision watermark {watermark} does not match decision boundary {expected_watermark}"
                ));
            }
            let mut cursor = start;
            let replayed = replay_declared_guard(
                GuardReplayContext {
                    trait_ref,
                    ledger,
                    label: &label,
                    position_path: &decision.position_path,
                    watermark,
                    end: decision.guard_evaluation_index,
                },
                declared_guard,
                &mut cursor,
                0,
                &mut BTreeSet::new(),
                diagnostics,
            );
            if cursor != decision.guard_evaluation_index {
                diagnostics.push(format!(
                    "{label} guard evaluation range has extra or missing entries"
                ));
            }
            if replayed.is_none_or(|outcome| outcome.routes_true() != decision.matched) {
                diagnostics.push(format!("{label} result does not replay from its declared guard"));
            }
        }
        (None, None) => diagnostics.push(format!(
            "{label} is missing its replay range and slot revision watermark"
        )),
        _ => diagnostics.push(format!(
            "{label} must carry both guard range and slot revision watermark"
        )),
    }
}

fn validate_current_slot_revisions(
    values: &[Value],
    revisions: &[SlotRevision],
    context: &str,
    require_revision_for_value: bool,
    diagnostics: &mut Vec<String>,
) {
    let mut latest_revision_by_slot: BTreeMap<&str, &SlotRevision> = BTreeMap::new();
    for revision in revisions {
        latest_revision_by_slot
            .entry(revision.slot_ref.as_str())
            .and_modify(|current| {
                if revision.acceptance_order > current.acceptance_order {
                    *current = revision;
                }
            })
            .or_insert(revision);
    }
    for (slot_ref, revision) in &latest_revision_by_slot {
        if !values.iter().any(|value| {
            value.ref_text == *slot_ref
                && value.value_digest == revision.value_digest
                && value.acceptance == AcceptanceStatus::Accepted
        }) {
            diagnostics.push(format!(
                "latest slot revision for {slot_ref} in {context} does not match its accepted current slot value"
            ));
        }
    }
    if require_revision_for_value {
        for value in values
            .iter()
            .filter(|value| value.acceptance == AcceptanceStatus::Accepted)
        {
            if latest_revision_by_slot
                .get(value.ref_text.as_str())
                .is_none_or(|revision| revision.value_digest != value.value_digest)
            {
                diagnostics.push(format!(
                    "accepted slot value {} in {context} has no matching latest slot revision",
                    value.ref_text
                ));
            }
        }
    }
}

fn control_item_for_stack_frame<'a>(
    trait_ref: &'a Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
    stack: &[ControlFrame],
    index: usize,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let frame = stack.get(index)?;
    if index == 0 {
        return sequence
            .iter()
            .find(|item| item.run_index == frame.parent_run_index)
            .map(|item| item.item);
    }
    let parent = stack.get(index - 1)?;
    trait_ref
        .sequences
        .get(&parent.sequence_id)?
        .sequence
        .get(parent.next_index)
}

#[derive(Clone, Copy)]
struct RevisionValidationContext<'a> {
    trait_ref: &'a Trait,
    sequence: &'a [crate::procedure::run::EffectiveSequenceItem<'a>],
    ledger: &'a State,
    latest_digest: Option<&'a Digest>,
}

fn validate_revision_evidence(
    context: RevisionValidationContext<'_>,
    index: usize,
    revision: &SlotRevision,
    evidence_started_by_slot: &mut BTreeSet<String>,
    diagnostics: &mut Vec<String>,
) {
    let carries_evidence = revision.operation.is_some()
        || revision.submitted_payload.is_some()
        || revision.prior_value_digest.is_some()
        || revision.prior_value.is_some()
        || revision.runtime_binding
        || revision.projection.is_some();
    if !carries_evidence {
        if evidence_started_by_slot.contains(revision.slot_ref.as_str()) {
            diagnostics.push(format!(
                "slot-revisions[{index}] strips write evidence after evidence-bearing revisions began"
            ));
        }
        return;
    }
    evidence_started_by_slot.insert(revision.slot_ref.as_str().to_string());
    let (Some(operation), Some(submitted_payload)) = (
        revision.operation.as_ref(),
        revision.submitted_payload.as_ref(),
    ) else {
        diagnostics.push(format!(
            "slot-revisions[{index}] must carry both operation and submitted-payload"
        ));
        return;
    };
    if revision.prior_value_digest.as_ref() != context.latest_digest {
        diagnostics.push(format!(
            "slot-revisions[{index}] prior-value-digest does not match the previous slot revision"
        ));
    }
    match (
        revision.prior_value_digest.as_ref(),
        revision.prior_value.as_ref(),
    ) {
        (Some(expected), Some(prior)) => match value_digest(&prior.value) {
            Ok(actual) if &actual == expected => {}
            Ok(_) => diagnostics.push(format!(
                "slot-revisions[{index}] prior-value does not match prior-value-digest"
            )),
            Err(error) => diagnostics.push(format!(
                "slot-revisions[{index}] prior-value cannot be digested: {error}"
            )),
        },
        (Some(_), None) => diagnostics.push(format!(
            "slot-revisions[{index}] has prior-value-digest without prior-value"
        )),
        (None, Some(_)) => diagnostics.push(format!(
            "slot-revisions[{index}] has prior-value without prior-value-digest"
        )),
        (None, None) => {}
    }
    if revision.runtime_binding {
        if operation != &WriteOperation::Replace {
            diagnostics.push(format!(
                "slot-revisions[{index}] runtime binding must use replace"
            ));
        }
    } else if let Some(item) = item_at_execution_path(
        context.trait_ref,
        context.sequence,
        context.ledger,
        &revision.position_path,
    ) {
        match item.output.sink_for_ref(revision.slot_ref.as_str()) {
            Some(sink) if sink.operation() == operation => {}
            Some(_) => diagnostics.push(format!(
                "slot-revisions[{index}] operation does not match its declared output sink"
            )),
            None => {}
        }
        validate_revision_command_evidence(context, index, revision, item, diagnostics);
    } else if revision.command_execution.is_some()
        || revision.source == Some(ValueSource::CommandOutput)
    {
        diagnostics.push(format!(
            "slot-revisions[{index}] claims command provenance without a declared command activation"
        ));
    }
    if let Some(projection) = revision.projection.as_ref() {
        validate_projection_revision(context, index, revision, projection, diagnostics);
    }
    let prior = revision.prior_value.as_ref().map(|value| &value.value);
    match apply_write_operation_value(operation, prior, &submitted_payload.value) {
        Ok(replayed) => match value_digest(&replayed) {
            Ok(digest) if digest == revision.value_digest => {}
            Ok(_) => diagnostics.push(format!(
                "slot-revisions[{index}] replay does not match value-digest"
            )),
            Err(error) => diagnostics.push(format!(
                "slot-revisions[{index}] replay cannot be digested: {error}"
            )),
        },
        Err(error) => diagnostics.push(format!(
            "slot-revisions[{index}] write evidence cannot be replayed: {error}"
        )),
    }
}

fn validate_revision_command_evidence(
    context: RevisionValidationContext<'_>,
    index: usize,
    revision: &SlotRevision,
    item: &crate::r#trait::procedure::SequenceItem,
    diagnostics: &mut Vec<String>,
) {
    let command_plan = match command_plan_for_item(item, "runtime.ledger.command") {
        Ok(plan) => plan,
        Err(error) => {
            diagnostics.push(format!(
                "slot-revisions[{index}] command declaration cannot be resolved: {error}"
            ));
            return;
        }
    };
    let Some(plan) = command_plan else {
        if revision.command_execution.is_some()
            || revision.source == Some(ValueSource::CommandOutput)
        {
            diagnostics.push(format!(
                "slot-revisions[{index}] non-command output claims command provenance"
            ));
        }
        return;
    };
    if revision.source != Some(ValueSource::CommandOutput) {
        diagnostics.push(format!(
            "slot-revisions[{index}] command output source must be command-output"
        ));
    }
    let Some(evidence) = revision.command_execution.as_ref() else {
        diagnostics.push(format!(
            "slot-revisions[{index}] command output is missing execution evidence"
        ));
        return;
    };
    let mut historical = context.ledger.clone();
    historical.accepted_slot_values = accepted_slot_values_before(
        context.ledger,
        revision.acceptance_order,
        &revision.position_path,
    );
    for frame in &mut historical.control_stack {
        frame.parallel_buffer.accepted_slot_values.clear();
        frame.parallel_buffer.accepted_output_port_values.clear();
    }
    match command_frame(item, &plan, &historical) {
        Ok(command) => {
            if evidence.argv != command.argv
                || evidence.output_slot != command.output_slot
                || evidence.executable_digest != command.executable_digest
            {
                diagnostics.push(format!(
                    "slot-revisions[{index}] command execution does not match its declared activation"
                ));
            }
            if evidence.output_slot != revision.slot_ref.as_str() {
                diagnostics.push(format!(
                    "slot-revisions[{index}] command execution output slot does not match the revision"
                ));
            }
            let succeeded = command_execution_succeeded(evidence, &command);
            if item.effective_kind() == SequenceKind::Check {
                // P565: replay the whole verdict record — `{ok, argv}` — through
                // the one constructor the submitting runtime and the acceptance
                // check also use, never a second local notion of the shape.
                // This is the THIRD site that has to agree; it was missed when
                // the record replaced a bare boolean, and because it only runs
                // when a ledger is re-validated, the build gates could not
                // catch it.
                let expected =
                    crate::procedure::session::check_output_value(
                        succeeded,
                        &command,
                        &crate::procedure::session::CheckEvidence::from_ledger(evidence),
                    );
                let submitted_verdict = revision
                    .submitted_payload
                    .as_ref()
                    .map(|payload| &payload.value);
                if submitted_verdict != Some(&expected) {
                    diagnostics.push(format!(
                        "slot-revisions[{index}] check verdict does not replay from its command execution"
                    ));
                }
            } else if !succeeded {
                diagnostics.push(format!(
                    "slot-revisions[{index}] command execution was not successful"
                ));
            }
        }
        Err(error) => diagnostics.push(format!(
            "slot-revisions[{index}] command activation cannot be replayed: {error}"
        )),
    }
}

fn validate_projection_revision(
    context: RevisionValidationContext<'_>,
    index: usize,
    revision: &SlotRevision,
    provenance: &ProjectionProvenance,
    diagnostics: &mut Vec<String>,
) {
    if revision.runtime_binding {
        diagnostics.push(format!(
            "slot-revisions[{index}] project write must not be a runtime binding"
        ));
    }
    let declared_item = item_at_execution_path(
        context.trait_ref,
        context.sequence,
        context.ledger,
        &revision.position_path,
    )
    .filter(|item| item.effective_kind() == SequenceKind::Project);

    if provenance.is_literal() {
        if provenance.source_ref.is_some()
            || provenance.source_value_digest.is_some()
            || provenance.field.is_some()
        {
            diagnostics.push(format!(
                "slot-revisions[{index}] literal-backed projection provenance must not carry a source ref, source value digest, or field"
            ));
            return;
        }
        validate_literal_projection_revision(index, revision, provenance, declared_item, diagnostics);
    } else {
        validate_slot_projection_revision(context, index, revision, provenance, declared_item, diagnostics);
    }
}

/// Literal-backed replay (P431): locate the unique declared destination/
/// operation entry, confirm it is still a literal source, bind to the
/// declared literal's digest, and confirm the submitted payload equals the
/// canonical literal exactly (no field selection is possible on a literal).
fn validate_literal_projection_revision(
    index: usize,
    revision: &SlotRevision,
    provenance: &ProjectionProvenance,
    declared_item: Option<&crate::r#trait::procedure::SequenceItem>,
    diagnostics: &mut Vec<String>,
) {
    let declared_literal = declared_item.and_then(|item| {
        item.projection.iter().find_map(|projection| {
            if projection.destination == revision.slot_ref.as_str()
                && Some(&projection.operation) == revision.operation.as_ref()
            {
                projection.source.as_literal()
            } else {
                None
            }
        })
    });
    let Some(literal) = declared_literal else {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection provenance does not match its declared literal project entry"
        ));
        return;
    };
    match value_digest(literal) {
        Ok(digest) if Some(&digest) == provenance.literal_digest.as_ref() => {}
        Ok(_) => diagnostics.push(format!(
            "slot-revisions[{index}] projection literal digest does not match the declared literal"
        )),
        Err(error) => diagnostics.push(format!(
            "slot-revisions[{index}] projection literal cannot be digested: {error}"
        )),
    }
    if revision.submitted_payload.as_ref().map(|payload| &payload.value) != Some(literal) {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection submitted payload does not replay from its declared literal"
        ));
    }
}

fn validate_slot_projection_revision(
    context: RevisionValidationContext<'_>,
    index: usize,
    revision: &SlotRevision,
    provenance: &ProjectionProvenance,
    declared_item: Option<&crate::r#trait::procedure::SequenceItem>,
    diagnostics: &mut Vec<String>,
) {
    let declared = declared_item.and_then(|item| {
        item.projection.iter().find(|projection| {
            projection.destination == revision.slot_ref.as_str()
                && Some(&projection.operation) == revision.operation.as_ref()
                && projection.source.as_slot_ref()
                    == provenance.source_ref.as_ref().map(Reference::as_str)
                && projection.field == provenance.field
        })
    });
    if declared.is_none() {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection provenance does not match its declared project entry"
        ));
        return;
    }
    let Some(source_slot_ref) = provenance.source_ref.as_ref() else {
        diagnostics.push(format!(
            "slot-revisions[{index}] slot-backed projection provenance is missing its source ref"
        ));
        return;
    };
    let Some(source_revision) = recorded_slot_revisions(context.ledger)
        .into_iter()
        .filter(|source| {
            &source.slot_ref == source_slot_ref
                && source.acceptance_order < revision.acceptance_order
                && revision_visible_at_decision(
                    &source.position_path,
                    &revision.position_path,
                )
        })
        .max_by_key(|source| source.acceptance_order)
    else {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection source has no preceding slot revision"
        ));
        return;
    };
    if Some(&source_revision.value_digest) != provenance.source_value_digest.as_ref() {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection source digest does not match the preceding source revision"
        ));
    }
    let source_value = source_revision
        .operation
        .as_ref()
        .zip(source_revision.submitted_payload.as_ref())
        .and_then(|(operation, submitted)| {
            apply_write_operation_value(
                operation,
                source_revision
                    .prior_value
                    .as_ref()
                    .map(|prior| &prior.value),
                &submitted.value,
            )
            .ok()
        });
    let selected = match (source_value.as_ref(), provenance.field.as_deref()) {
        (Some(value), Some(field)) => crate::shared::resolve_field_path(value, field),
        (Some(value), None) => Some(value),
        _ => None,
    };
    if selected
        != revision
            .submitted_payload
            .as_ref()
            .map(|payload| &payload.value)
    {
        diagnostics.push(format!(
            "slot-revisions[{index}] projection submitted payload does not replay from its source"
        ));
    }
}

fn revision_has_accepted_producer_activation(ledger: &State, revision: &SlotRevision) -> bool {
    let Some(procedure) = revision.position_path.first().filter(|segment| {
        segment.kind == "procedure" && segment.iteration.is_none() && segment.item_index.is_none()
    }) else {
        return false;
    };
    ledger.sequence_statuses.iter().any(|status| {
        status.status == SequenceStatusKind::Accepted
            && status.run_index == procedure.index
            && if revision.position_path.len() == 1 {
                status.position_path.is_empty() && status.item_id == procedure.id
            } else {
                status.position_path == revision.position_path
            }
    })
}

fn top_level_branch_was_entered(ledger: &State, decision: &BranchDecision) -> bool {
    outer_run_was_reached(ledger, decision.parent_run_index)
}

fn revision_matches_for_each_binding(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    revision: &SlotRevision,
) -> bool {
    let legacy_binding = revision.operation.is_none()
        && revision.submitted_payload.is_none()
        && revision.prior_value_digest.is_none()
        && revision.prior_value.is_none();
    if !revision.runtime_binding && !legacy_binding {
        return false;
    }
    let (Some(for_each_id), Some(item_index)) =
        (revision.for_each_id.as_deref(), revision.item_index)
    else {
        return false;
    };
    let Some(first) = revision.position_path.first() else {
        return false;
    };
    let Some(mut owner) = sequence
        .iter()
        .find(|item| item.run_index == first.index)
        .map(|item| item.item)
    else {
        return false;
    };
    let mut traversed = vec![first.clone()];
    for segment in revision.position_path.iter().skip(1) {
        if owner.effective_kind() == SequenceKind::ForEach
            && owner.id.as_deref() == Some(for_each_id)
            && owner.item.as_deref() == Some(revision.slot_ref.as_str())
            && segment.item_index == Some(item_index)
        {
            if revision.runtime_binding {
                return revision
                    .submitted_payload
                    .as_ref()
                    .and_then(|payload| value_digest(&payload.value).ok())
                    .is_some_and(|digest| digest == revision.value_digest);
            }
            return owner
                .over
                .as_deref()
                .and_then(|over| accepted_value(ledger, over))
                .and_then(|value| value.value.as_array())
                .and_then(|items| items.get(item_index))
                .and_then(|item| value_digest(item).ok())
                .is_some_and(|digest| digest == revision.value_digest);
        }
        if segment.kind == "item" {
            break;
        }
        let Some(selected_sequence) =
            selected_control_sequence(ledger, first.index, owner, &traversed, segment.iteration)
        else {
            return false;
        };
        let Some(next) = trait_ref
            .sequences
            .get(&selected_sequence)
            .and_then(|named| named.sequence.get(segment.index))
        else {
            return false;
        };
        owner = next;
        traversed.push(segment.clone());
    }
    false
}

fn outer_run_was_reached(ledger: &State, run_index: usize) -> bool {
    let status = ledger.sequence_statuses.iter().find(|status| {
        status.position_path.is_empty() && status.run_index == run_index
    });
    if status.is_some_and(|status| status.status == SequenceStatusKind::Skipped) {
        return false;
    }
    if run_index < ledger.current_run_index {
        return true;
    }
    if run_index != ledger.current_run_index {
        return false;
    }
    ledger
        .control_stack
        .first()
        .is_some_and(|frame| frame.parent_run_index == run_index)
        || status.is_some_and(|status| {
            !matches!(
                status.status,
                SequenceStatusKind::Pending | SequenceStatusKind::DependencyPending
            )
        })
}

fn item_at_execution_path<'a>(
    trait_ref: &'a Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
    ledger: &State,
    path: &[PathSegment],
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let first = path.first()?;
    if first.kind != "procedure"
        || first.iteration.is_some()
        || first.item_index.is_some()
    {
        return None;
    }
    let root = sequence
        .iter()
        .find(|item| item.run_index == first.index)?
        .item;
    if first.id != root.id {
        return None;
    }
    if path.len() == 1 {
        return Some(root);
    }

    let mut owner = root;
    let mut nearest_iteration = None;
    let mut nearest_item_index = None;
    let mut expected_item_index = None;
    let mut traversed = vec![first.clone()];
    for segment in &path[1..path.len() - 1] {
        let selected_sequence = selected_control_sequence(ledger, first.index, owner, &traversed, segment.iteration)?;
        if segment.kind != sequence_kind_name(owner.effective_kind())
            || Some(selected_sequence.as_str()) != segment.id.as_deref()
            || !control_segment_is_valid(trait_ref, ledger, owner, segment)
        {
            return None;
        }
        nearest_iteration = segment.iteration.or(nearest_iteration);
        nearest_item_index = segment.item_index.or(nearest_item_index);
        expected_item_index = Some(segment.index);
        owner = trait_ref
            .sequences
            .get(segment.id.as_deref()?)?
            .sequence
            .get(segment.index)?;
        traversed.push(segment.clone());
    }
    let terminal = path.last()?;
    (terminal.kind == "item"
        && terminal.id == owner.id
        && Some(terminal.index) == expected_item_index
        && terminal.iteration == nearest_iteration
        && terminal.item_index == nearest_item_index)
        .then_some(owner)
}

fn runtime_control_item_from_identity_at_path<'a>(
    trait_ref: &'a Trait,
    ledger: &State,
    path: &[PathSegment],
    identity: &ControlEmissionIdentity,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let sequence = trait_ref
        .procedure
        .as_ref()
        .and_then(|procedure| effective_sequence_items(procedure).ok())?;
    if path.len() > 1 {
        item_at_execution_path(trait_ref, &sequence, ledger, path)?;
    }
    let first = path.first()?;
    if first.kind != "procedure"
        || first.index != identity.parent_run_index
        || first.iteration.is_some()
        || first.item_index.is_some()
    {
        return None;
    }
    let mut owner = sequence
        .iter()
        .find(|item| item.run_index == first.index)?
        .item;
    if first.id != owner.id {
        return None;
    }

    let mut traversed = vec![first.clone()];
    for segment in &path[1..] {
        if segment.kind == "item" {
            return runtime_control_identity_matches_without_activation(
                ledger,
                first.index,
                owner,
                &traversed,
                identity,
            )
            .then_some(owner);
        }
        let selected_sequence = selected_control_sequence(ledger, first.index, owner, &traversed, segment.iteration)?;
        if segment.kind != sequence_kind_name(owner.effective_kind())
            || Some(selected_sequence.as_str()) != segment.id.as_deref()
            || !control_segment_is_valid(trait_ref, ledger, owner, segment)
        {
            return None;
        }
        if runtime_control_identity_matches(
            ledger,
            first.index,
            owner,
            &traversed,
            segment,
            identity,
        ) {
            return Some(owner);
        }
        owner = trait_ref
            .sequences
            .get(segment.id.as_deref()?)?
            .sequence
            .get(segment.index)?;
        traversed.push(segment.clone());
    }
    runtime_control_identity_matches_without_activation(
        ledger,
        first.index,
        owner,
        &traversed,
        identity,
    )
    .then_some(owner)
}

fn runtime_control_identity_matches(
    ledger: &State,
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
    traversed: &[PathSegment],
    activation: &PathSegment,
    identity: &ControlEmissionIdentity,
) -> bool {
    runtime_control_identity_matches_without_activation(
        ledger,
        parent_run_index,
        item,
        traversed,
        identity,
    ) && identity.iteration_index == activation.iteration
        && identity.item_index == activation.item_index
}

fn runtime_control_identity_matches_without_activation(
    ledger: &State,
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
    traversed: &[PathSegment],
    identity: &ControlEmissionIdentity,
) -> bool {
    let kind_matches = match identity.kind {
        ControlKind::Sequence => item.effective_kind() == SequenceKind::Sequence,
        ControlKind::Branch => item.effective_kind() == SequenceKind::Branch,
        ControlKind::Loop => item.effective_kind() == SequenceKind::Loop,
        ControlKind::ForEach => item.effective_kind() == SequenceKind::ForEach,
        ControlKind::Parallel => item.effective_kind() == SequenceKind::Parallel,
    };
    kind_matches
        && identity.parent_run_index == parent_run_index
        && item.id == identity.control_item_id
        && selected_control_sequence(ledger, parent_run_index, item, traversed, identity.iteration_index)
            .is_some_and(|sequence_id| sequence_id == identity.sequence_id)
}

fn decision_path_is_bound(
    trait_ref: &Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
    ledger: &State,
    decision: &BranchDecision,
) -> bool {
    branch_at_decision_path(trait_ref, sequence, ledger, decision).is_some()
}

fn branch_at_decision_path<'a>(
    trait_ref: &'a Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
    ledger: &State,
    decision: &BranchDecision,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let first = decision.position_path.first()?;
    if first.kind != "procedure"
        || first.index != decision.parent_run_index
        || first.iteration.is_some()
        || first.item_index.is_some()
    {
        return None;
    }
    let root = sequence
        .iter()
        .find(|item| item.run_index == decision.parent_run_index)?
        .item;
    if first.id != root.id {
        return None;
    }
    if decision.position_path.len() == 1 {
        return (root.effective_kind() == SequenceKind::Branch
            && root.id.as_deref() == Some(decision.branch_id.as_str()))
            .then_some(root);
    }

    let mut owner = root;
    let mut nearest_iteration = None;
    let mut nearest_item_index = None;
    let mut expected_item_index = None;
    let mut traversed = vec![first.clone()];
    for segment in &decision.position_path[1..decision.position_path.len() - 1] {
        let selected_sequence = selected_control_sequence(ledger, decision.parent_run_index, owner, &traversed, segment.iteration)?;
        if segment.kind != sequence_kind_name(owner.effective_kind())
            || Some(selected_sequence.as_str()) != segment.id.as_deref()
            || !control_segment_is_valid(trait_ref, ledger, owner, segment)
        {
            return None;
        }
        nearest_iteration = segment.iteration.or(nearest_iteration);
        nearest_item_index = segment.item_index.or(nearest_item_index);
        expected_item_index = Some(segment.index);
        owner = trait_ref
            .sequences
            .get(segment.id.as_deref()?)?
            .sequence
            .get(segment.index)?;
        traversed.push(segment.clone());
    }
    let terminal = decision.position_path.last()?;
    (terminal.kind == "item"
        && terminal.id.as_deref() == Some(decision.branch_id.as_str())
        && Some(terminal.index) == expected_item_index
        && terminal.iteration == nearest_iteration
        && terminal.item_index == nearest_item_index
        && owner.effective_kind() == SequenceKind::Branch
        && owner.id.as_deref() == Some(decision.branch_id.as_str()))
        .then_some(owner)
}

fn branch_at_status_path<'a>(
    trait_ref: &'a Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
    ledger: &State,
    status: &SequenceStatus,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let decision = BranchDecision {
        parent_run_index: status.run_index,
        branch_id: status.item_id.clone()?,
        position_path: status.position_path.clone(),
        matched: false,
        when: None,
        guard_evaluation_start_index: None,
        slot_revision_watermark: None,
        guard_evaluation_index: 0,
        selected_arm: String::new(),
        sequence_id: None,
    };
    branch_at_decision_path(trait_ref, sequence, ledger, &decision)
}

fn sequence_kind_name(kind: SequenceKind) -> &'static str {
    match kind {
        SequenceKind::Sequence => "sequence",
        SequenceKind::Branch => "branch",
        SequenceKind::Loop => "loop",
        SequenceKind::ForEach => "for-each",
        SequenceKind::Parallel => "parallel",
        SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => "",
    }
}

/// Resolve the local sequence id `item` actually selected at this position,
/// as recorded evidence would attest — the immutable [`BranchDecision`] for a
/// `branch` item, or (for `parallel`, which always runs every branch in a
/// fixed authored order — nothing is decided at runtime) `item.branches[branch_index]`.
/// `branch_index` is the branch position a caller already knows from either
/// the traversed `PathSegment.iteration` or a [`ControlEmissionIdentity`]'s
/// `iteration_index`, both of which carry the branch index for a `parallel`
/// frame the same way they carry the loop iteration for a `loop` frame.
fn selected_control_sequence(
    ledger: &State,
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
    traversed: &[PathSegment],
    branch_index: Option<usize>,
) -> Option<String> {
    if item.effective_kind() == SequenceKind::Parallel {
        return item
            .branches
            .as_slice()
            .get(branch_index?)
            .and_then(|reference| sequence_id_from_ref(reference).ok());
    }
    if item.effective_kind() != SequenceKind::Branch {
        return item
            .sequence
            .as_deref()
            .and_then(|reference| sequence_id_from_ref(reference).ok());
    }
    let activation_path = control_activation_path(item, traversed)?;
    let decision = ledger.branch_decisions.iter().find(|decision| {
        decision.parent_run_index == parent_run_index
            && decision.branch_id == item.id.as_deref().unwrap_or_default()
            && decision.position_path == activation_path
    })?;
    match decision.selected_arm.as_str() {
        "then" => item.sequence.as_deref(),
        "otherwise" => item.otherwise.as_deref(),
        "none" => None,
        _ => return None,
    }
    .and_then(|reference| sequence_id_from_ref(reference).ok())
}

fn control_activation_path(
    item: &crate::r#trait::procedure::SequenceItem,
    traversed: &[PathSegment],
) -> Option<Vec<PathSegment>> {
    if traversed.len() == 1 {
        return Some(traversed.to_vec());
    }
    let enclosing = traversed.last()?;
    let nearest_iteration = traversed.iter().rev().find_map(|segment| segment.iteration);
    let nearest_item_index = traversed.iter().rev().find_map(|segment| segment.item_index);
    let mut activation = traversed.to_vec();
    activation.push(PathSegment {
        kind: "item".to_string(),
        id: item.id.clone(),
        index: enclosing.index,
        iteration: nearest_iteration,
        item_index: nearest_item_index,
    });
    Some(activation)
}

fn control_segment_is_valid(
    _trait_ref: &Trait,
    ledger: &State,
    item: &crate::r#trait::procedure::SequenceItem,
    segment: &PathSegment,
) -> bool {
    match item.effective_kind() {
        SequenceKind::Sequence | SequenceKind::Branch => {
            segment.iteration.is_none() && segment.item_index.is_none()
        }
        SequenceKind::Loop => {
            segment.item_index.is_none()
                && segment.iteration.is_some_and(|iteration| {
                    resolved_loop_bound(item, ledger)
                        .is_some_and(|max_iterations| iteration < max_iterations)
                })
        }
        SequenceKind::ForEach => segment.iteration.is_none()
            && segment.item_index.is_some_and(|index| index < item.max_items.unwrap_or(usize::MAX)),
        // `iteration` carries the branch index for a `parallel` segment, the
        // same way it carries the loop iteration for a `loop` segment.
        SequenceKind::Parallel => segment.item_index.is_none()
            && segment
                .iteration
                .is_some_and(|branch_index| branch_index < item.branches.as_slice().len()),
        SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => false,
    }
}

fn failure_route_source_at_path<'a>(
    trait_ref: &'a Trait,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
    ledger: &State,
    route: &FailureRouteRecord,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let first = route.position_path.first()?;
    let root = sequence
        .iter()
        .find(|item| item.run_index == route.source_run_index)?
        .item;
    if first.kind != "procedure"
        || first.index != route.source_run_index
        || first.id != root.id
        || first.iteration.is_some()
        || first.item_index.is_some()
    {
        return None;
    }
    let matches_route = |item: &'a crate::r#trait::procedure::SequenceItem| {
        item.id.as_deref() == Some(route.source_step_id.as_str())
            && item
                .on_failure
                .as_ref()
                .and_then(FailureTarget::route)
                .is_some_and(|declared| {
                    declared.step == route.target_step_id && declared.signal == route.signal
                })
    };
    let mut declared_source = matches_route(root).then_some(root);
    let mut owner = root;
    let mut traversed = vec![first.clone()];
    for segment in route
        .position_path
        .iter()
        .skip(1)
        .take(route.position_path.len().saturating_sub(2))
    {
        let selected_sequence =
            selected_control_sequence(ledger, route.source_run_index, owner, &traversed, segment.iteration)?;
        if segment.kind != sequence_kind_name(owner.effective_kind())
            || Some(selected_sequence.as_str()) != segment.id.as_deref()
            || !control_segment_is_valid(trait_ref, ledger, owner, segment)
        {
            return None;
        }
        owner = trait_ref
            .sequences
            .get(segment.id.as_deref()?)?
            .sequence
            .get(segment.index)?;
        if matches_route(owner) {
            declared_source = Some(owner);
        }
        traversed.push(segment.clone());
    }
    if route.position_path.len() > 1 {
        let terminal = route.position_path.last()?;
        let enclosing = traversed.last()?;
        let nearest_iteration = traversed.iter().rev().find_map(|segment| segment.iteration);
        let nearest_item_index = traversed.iter().rev().find_map(|segment| segment.item_index);
        if terminal.kind != "item"
            || terminal.id != owner.id
            || terminal.index != enclosing.index
            || terminal.iteration != nearest_iteration
            || terminal.item_index != nearest_item_index
        {
            return None;
        }
    }
    declared_source
}

fn validate_active_path_contract(trait_ref: &Trait, ledger: &State, diagnostics: &mut Vec<String>) {
    if let Some(reason) = active_path_empty_reason(ledger) {
        if !ledger.active_path.is_empty() {
            diagnostics.push(format!("{reason} must not carry active-path"));
        }
        return;
    }

    if ledger.final_state != FinalState::Running && ledger.stop_reason.is_some() {
        if !ledger.active_path.is_empty() {
            diagnostics.push("terminal stopped ledger active-path must be empty".to_string());
        }
        return;
    }

    let Some(expected) = expected_active_path(trait_ref, ledger, diagnostics) else {
        if !ledger.active_path.is_empty() {
            diagnostics.push(
                "ledger with no current nested executable item must not carry active-path"
                    .to_string(),
            );
        } else if ledger.final_state == FinalState::Running {
            diagnostics.push("running nested ledger has no current executable item".to_string());
        }
        return;
    };

    compare_active_path(&ledger.active_path, &expected, diagnostics);
}

fn active_path_empty_reason(ledger: &State) -> Option<&'static str> {
    if ledger.control_stack.is_empty() {
        return Some("ledger without control-stack");
    }
    None
}

fn expected_active_path(
    trait_ref: &Trait,
    ledger: &State,
    diagnostics: &mut Vec<String>,
) -> Option<Vec<PathSegment>> {
    let frame = ledger.control_stack.last()?;
    let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
        diagnostics.push(format!(
            "active-path cannot be checked because control-stack references unknown sequence {}",
            frame.sequence_id
        ));
        return None;
    };
    let item = named.sequence.get(frame.next_index)?;
    if !is_executable_item(item) {
        return None;
    }
    Some(path_for_nested_item(ledger, frame.next_index, item))
}

fn compare_active_path(
    actual: &[PathSegment],
    expected: &[PathSegment],
    diagnostics: &mut Vec<String>,
) {
    if actual.is_empty() {
        diagnostics.push("ledger with active control-stack must carry active-path".to_string());
        return;
    }
    if actual.len() != expected.len() {
        diagnostics.push(format!(
            "active-path has {} segment(s), expected {} segment(s)",
            actual.len(),
            expected.len()
        ));
    }
    for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        compare_active_path_segment(index, actual, expected, diagnostics);
    }
}

fn compare_active_path_segment(
    index: usize,
    actual: &PathSegment,
    expected: &PathSegment,
    diagnostics: &mut Vec<String>,
) {
    if actual.kind != expected.kind {
        diagnostics.push(format!(
            "active-path[{index}].kind {} does not match expected {}",
            actual.kind, expected.kind
        ));
    }
    if actual.id != expected.id {
        diagnostics.push(format!(
            "active-path[{index}].id {} does not match expected {}",
            format_optional_str(actual.id.as_deref()),
            format_optional_str(expected.id.as_deref())
        ));
    }
    if actual.index != expected.index {
        diagnostics.push(format!(
            "active-path[{index}].index {} does not match expected {}",
            actual.index, expected.index
        ));
    }
    if actual.iteration != expected.iteration {
        diagnostics.push(format!(
            "active-path[{index}].iteration {} does not match expected {}",
            format_optional_usize(actual.iteration),
            format_optional_usize(expected.iteration)
        ));
    }
    if actual.item_index != expected.item_index {
        diagnostics.push(format!(
            "active-path[{index}].item-index {} does not match expected {}",
            format_optional_usize(actual.item_index),
            format_optional_usize(expected.item_index)
        ));
    }
}

fn validate_guard_evaluations_contract(ledger: &State, diagnostics: &mut Vec<String>) {
    let mut last_elapsed_seen: Option<u64> = None;
    for (index, evaluation) in ledger.guard_evaluations.iter().enumerate() {
        if evaluation.predicate.trim().is_empty() {
            diagnostics.push(format!(
                "guard-evaluations[{index}] predicate must not be empty"
            ));
        }
        if evaluation.reason.trim().is_empty() {
            diagnostics.push(format!(
                "guard-evaluations[{index}] reason must not be empty"
            ));
        }
        if let Some(scope) = evaluation.scope.as_ref()
            && scope.loop_id.trim().is_empty() {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] scope loop-id must not be empty"
                ));
            }
        if let Some(evidence) = evaluation.comparison_evidence.as_ref() {
            validate_comparison_guard_evidence(
                ledger,
                index,
                evaluation,
                evidence,
                None,
                diagnostics,
            );
            if evidence.subject == ConditionComparisonSubject::Elapsed
                && let ComparisonOperandEvidence::Literal { value } = &evidence.lhs
                && let Some(observed) = value.as_u64()
            {
                if last_elapsed_seen.is_some_and(|previous| observed < previous) {
                    diagnostics.push(format!(
                        "guard-evaluations[{index}] elapsed-seconds evidence decreased across the run"
                    ));
                }
                last_elapsed_seen = Some(last_elapsed_seen.map_or(observed, |previous| previous.max(observed)));
            }
        }
    }
    if let Some(observed_max) = last_elapsed_seen
        && observed_max > ledger.elapsed_seconds
    {
        diagnostics.push(format!(
            "elapsed-seconds {} does not cover the greatest elapsed-seconds guard evidence {observed_max}",
            ledger.elapsed_seconds
        ));
    }
    if let Some(stop_reason) = ledger.stop_reason.as_ref() {
        validate_stop_reason_contract(ledger, stop_reason, diagnostics);
        if let Some(last_check) = stop_reason.last_check
            && last_check >= ledger.guard_evaluations.len() {
                diagnostics.push(format!(
                    "stop-reason last-check {} exceeds guard-evaluations length {}",
                    last_check,
                    ledger.guard_evaluations.len()
                ));
            }
    }
}

fn validate_branch_guard_decision(
    trait_ref: &Trait,
    ledger: &State,
    decision: &BranchDecision,
    branch: &crate::r#trait::procedure::SequenceItem,
    diagnostics: &mut Vec<String>,
) {
    let expected_marker = branch.when.as_ref().and_then(|when| {
        serde_json::to_string(&(when, &decision.position_path))
            .ok()
            .map(|when| format!("branch:{}:{when}", decision.branch_id))
    });
    let marker = ledger.guard_evaluations.get(decision.guard_evaluation_index);
    if marker.is_none_or(|evaluation| {
        evaluation.matched != decision.matched
            || evaluation.reason != "branch selection"
            || evaluation.comparison_evidence.is_some()
            || Some(&evaluation.predicate) != expected_marker.as_ref()
    }) {
        diagnostics.push(format!(
            "branch decision {:?} has no matching guard evaluation evidence",
            decision.branch_id
        ));
    }

    let Some(guard) = branch.when.as_ref() else {
        return;
    };
    match (
        decision.guard_evaluation_start_index,
        decision.slot_revision_watermark,
    ) {
        (Some(start), Some(watermark)) => {
            if start >= decision.guard_evaluation_index {
                diagnostics.push(format!(
                    "branch decision {:?} guard evaluation range is empty or reversed",
                    decision.branch_id
                ));
                return;
            }
            let expected_watermark =
                expected_slot_revision_watermark_before(ledger, &decision.position_path);
            if watermark != expected_watermark {
                diagnostics.push(format!(
                    "branch decision {:?} slot revision watermark {watermark} does not match decision boundary {expected_watermark}",
                    decision.branch_id
                ));
            }
            let mut cursor = start;
            let label = format!("branch decision {:?}", decision.branch_id);
            let replayed = replay_declared_guard(
                GuardReplayContext {
                    trait_ref,
                    ledger,
                    label: &label,
                    position_path: &decision.position_path,
                    watermark,
                    end: decision.guard_evaluation_index,
                },
                guard,
                &mut cursor,
                0,
                &mut BTreeSet::new(),
                diagnostics,
            );
            if cursor != decision.guard_evaluation_index {
                diagnostics.push(format!(
                    "branch decision {:?} guard evaluation range has extra or missing entries",
                    decision.branch_id
                ));
            }
            if replayed.is_none_or(|outcome| outcome.routes_true() != decision.matched) {
                diagnostics.push(format!(
                    "branch decision {:?} result does not replay from its declared guard",
                    decision.branch_id
                ));
            }
        }
        (None, None) => diagnostics.push(format!(
            "branch decision {:?} is missing its replay range and slot revision watermark",
            decision.branch_id
        )),
        _ => diagnostics.push(format!(
            "branch decision {:?} must carry both guard range and slot revision watermark",
            decision.branch_id
        )),
    }
}

/// Shared guard-evaluation-range replay context for both branch selection
/// decisions and P290 conditional-input inclusion decisions: both are an
/// immutable `matched` verdict tied to a declared guard, a contiguous
/// `guard_evaluations` range, and a slot-revision watermark boundary. `label`
/// only feeds diagnostics; the position path drives the actual replay/
/// visibility checks (P290).
#[derive(Clone, Copy)]
struct GuardReplayContext<'a> {
    trait_ref: &'a Trait,
    ledger: &'a State,
    label: &'a str,
    position_path: &'a [PathSegment],
    watermark: usize,
    end: usize,
}

fn replay_declared_guard(
    context: GuardReplayContext<'_>,
    guard: &GuardExpr,
    cursor: &mut usize,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    if depth > MAX_GUARD_EVALUATION_DEPTH {
        return replay_guard_leaf(context, cursor, "guard-depth", diagnostics);
    }
    match guard {
        GuardExpr::Ref(ref_text) => {
            let parsed = Reference::parse(ref_text).ok()?;
            if parsed.kind() == Kind::Condition {
                let condition_id = parsed.id().to_string();
                if !seen_conditions.insert(condition_id.clone()) {
                    return replay_guard_leaf(context, cursor, ref_text, diagnostics);
                }
                let condition = context.trait_ref.conditions.get(parsed.id())?;
                let replayed = replay_declared_guard(
                    context,
                    &condition.as_guard(),
                    cursor,
                    depth + 1,
                    seen_conditions,
                    diagnostics,
                );
                seen_conditions.remove(&condition_id);
                replayed
            } else {
                replay_guard_leaf(context, cursor, ref_text, diagnostics)
            }
        }
        GuardExpr::Any(items) => replay_guard_aggregate(
            context,
            items,
            cursor,
            depth,
            seen_conditions,
            false,
            diagnostics,
        ),
        GuardExpr::Predicate(predicate) => replay_declared_predicate(
            context,
            predicate,
            cursor,
            depth,
            seen_conditions,
            diagnostics,
        ),
    }
}

fn replay_declared_predicate(
    context: GuardReplayContext<'_>,
    predicate: &crate::r#trait::condition::GuardPredicate,
    cursor: &mut usize,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    if let Some(signal) = predicate.signal.as_deref() {
        return replay_declared_guard(
            context,
            &GuardExpr::Ref(signal.to_string()),
            cursor,
            depth,
            seen_conditions,
            diagnostics,
        );
    }
    if let Some(condition) = predicate.condition.as_deref() {
        return replay_declared_guard(
            context,
            &GuardExpr::Ref(condition.to_string()),
            cursor,
            depth,
            seen_conditions,
            diagnostics,
        );
    }
    if let Some(not) = predicate.not.as_deref() {
        let child = replay_declared_guard(
            context,
            not,
            cursor,
            depth + 1,
            seen_conditions,
            diagnostics,
        )?;
        return replay_guard_marker(context, cursor, "not[...]", child.negate(), diagnostics);
    }
    if let Some(iteration) = predicate.iteration {
        return replay_guard_leaf(context, cursor, &format!("iteration={iteration}"), diagnostics);
    }
    if let Some(iteration) = predicate.iteration_at_least {
        return replay_guard_leaf(
            context,
            cursor,
            &format!("iteration-at-least={iteration}"),
            diagnostics,
        );
    }
    if let Some(threshold) = predicate.elapsed_seconds_at_least.as_ref() {
        return replay_declared_elapsed_comparison(context, cursor, threshold, diagnostics)
            .map(GuardOutcome::from_bool);
    }
    if let Some(slot_ref) = predicate.empty.as_deref() {
        return replay_guard_leaf(context, cursor, &format!("empty({slot_ref})"), diagnostics);
    }
    if let Some(subject_ref) = predicate.present.as_deref() {
        return replay_present_predicate(context, cursor, subject_ref, predicate, diagnostics);
    }
    if let Some(slot_ref) = predicate.count.as_deref() {
        let counted = match predicate
            .field
            .as_deref()
            .zip(predicate.field_equals.as_ref())
        {
            Some((field, expected)) => format!("{slot_ref} where {field} == {expected}"),
            None => slot_ref.to_string(),
        };
        let label = if let Some(expected) = predicate.equals.as_ref() {
            format!("count({counted}) == {expected}")
        } else if let Some(expected) = predicate.at_least.as_ref() {
            format!("count({counted}) >= {expected}")
        } else {
            format!("count({counted})")
        };
        return replay_guard_leaf(context, cursor, &label, diagnostics);
    }
    if let Some(slot_ref) = predicate.slot.as_deref() {
        if let Some((operator, expected)) = runtime_comparison_modifier(predicate) {
            return replay_declared_comparison(
                context,
                cursor,
                DeclaredComparison {
                    subject: ConditionComparisonSubject::Slot,
                    ref_text: slot_ref,
                    field: predicate.field.as_deref(),
                    operator,
                    expected,
                },
                diagnostics,
            )
            .map(GuardOutcome::from_bool);
        }
        return replay_guard_leaf(
            context,
            cursor,
            &slot_predicate_label(slot_ref, predicate.field.as_deref(), None),
            diagnostics,
        );
    }
    if let Some(output_ref) = predicate.output.as_deref() {
        if let Some((operator, expected)) = runtime_comparison_modifier(predicate) {
            return replay_declared_comparison(
                context,
                cursor,
                DeclaredComparison {
                    subject: ConditionComparisonSubject::Output,
                    ref_text: output_ref,
                    field: predicate.field.as_deref(),
                    operator,
                    expected,
                },
                diagnostics,
            )
            .map(GuardOutcome::from_bool);
        }
        return replay_guard_leaf(
            context,
            cursor,
            &output_predicate_label(output_ref, predicate.field.as_deref(), None),
            diagnostics,
        );
    }
    if !predicate.all.is_empty() {
        return replay_guard_aggregate(
            context,
            &predicate.all,
            cursor,
            depth,
            seen_conditions,
            true,
            diagnostics,
        );
    }
    if !predicate.any.is_empty() {
        return replay_guard_aggregate(
            context,
            &predicate.any,
            cursor,
            depth,
            seen_conditions,
            false,
            diagnostics,
        );
    }
    replay_guard_leaf(context, cursor, "empty-predicate", diagnostics)
}

struct DeclaredComparison<'a> {
    subject: ConditionComparisonSubject,
    ref_text: &'a str,
    field: Option<&'a str>,
    operator: ConditionComparisonOperator,
    expected: &'a JsonValue,
}

fn replay_declared_comparison(
    context: GuardReplayContext<'_>,
    cursor: &mut usize,
    declared: DeclaredComparison<'_>,
    diagnostics: &mut Vec<String>,
) -> Option<bool> {
    let (index, evaluation) = take_branch_guard_evaluation(context, cursor, diagnostics)?;
    let expected_predicate = match declared.subject {
        ConditionComparisonSubject::Slot => {
            comparison_slot_predicate_label(
                declared.ref_text,
                declared.field,
                declared.operator,
                declared.expected,
            )
        }
        ConditionComparisonSubject::Output => {
            comparison_output_predicate_label(
                declared.ref_text,
                declared.field,
                declared.operator,
                declared.expected,
            )
        }
        // `replay_declared_predicate` routes `elapsed-seconds-at-least` to
        // `replay_declared_elapsed_comparison` before this function is ever
        // called with that subject.
        ConditionComparisonSubject::Elapsed => unreachable!(
            "elapsed comparisons are replayed by replay_declared_elapsed_comparison"
        ),
    };
    validate_guard_label(index, evaluation, &expected_predicate, diagnostics);
    let Some(evidence) = evaluation.comparison_evidence.as_ref() else {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison atom is missing exact operand evidence"
        ));
        return Some(evaluation.matched);
    };
    if evidence.subject != declared.subject || evidence.operator != declared.operator {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison subject/operator does not match the declared guard"
        ));
    }
    if !operand_matches_declared_ref(&evidence.lhs, declared.ref_text, declared.field) {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison LHS does not match the declared ref and field"
        ));
    }
    let rhs_matches = if declared.operator == ConditionComparisonOperator::Equals
        || declared.expected.is_number()
    {
        matches!(&evidence.rhs, ComparisonOperandEvidence::Literal { value } if value == declared.expected)
    } else if let Some(expected_ref) =
        crate::r#trait::condition::numeric_comparison_ref(declared.expected)
    {
        operand_matches_declared_ref(&evidence.rhs, expected_ref, None)
    } else {
        false
    };
    if !rhs_matches {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison RHS does not match the declared expected value"
        ));
    }
    Some(validate_comparison_guard_evidence(
        context.ledger,
        index,
        evaluation,
        evidence,
        Some((context.position_path, context.watermark)),
        diagnostics,
    ))
}

fn replay_declared_elapsed_comparison(
    context: GuardReplayContext<'_>,
    cursor: &mut usize,
    expected: &JsonValue,
    diagnostics: &mut Vec<String>,
) -> Option<bool> {
    let (index, evaluation) = take_branch_guard_evaluation(context, cursor, diagnostics)?;
    let expected_predicate = format!("elapsed-seconds >= {expected}");
    validate_guard_label(index, evaluation, &expected_predicate, diagnostics);
    let Some(evidence) = evaluation.comparison_evidence.as_ref() else {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison atom is missing exact operand evidence"
        ));
        return Some(evaluation.matched);
    };
    if evidence.subject != ConditionComparisonSubject::Elapsed
        || evidence.operator != ConditionComparisonOperator::AtLeast
    {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison subject/operator does not match the declared guard"
        ));
    }
    let rhs_matches = if expected.is_number() {
        matches!(&evidence.rhs, ComparisonOperandEvidence::Literal { value } if value == expected)
    } else if let Some(expected_ref) =
        crate::r#trait::condition::numeric_comparison_ref(expected)
    {
        operand_matches_declared_ref(&evidence.rhs, expected_ref, None)
    } else {
        false
    };
    if !rhs_matches {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison RHS does not match the declared expected value"
        ));
    }
    Some(validate_comparison_guard_evidence(
        context.ledger,
        index,
        evaluation,
        evidence,
        Some((context.position_path, context.watermark)),
        diagnostics,
    ))
}

fn replay_guard_aggregate(
    context: GuardReplayContext<'_>,
    items: &[GuardExpr],
    cursor: &mut usize,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
    all: bool,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    let mut result = if all {
        GuardOutcome::Matched
    } else {
        GuardOutcome::NotMatched
    };
    for item in items {
        let outcome = replay_declared_guard(
            context,
            item,
            cursor,
            depth + 1,
            seen_conditions,
            diagnostics,
        )?;
        result = if all {
            result.and(outcome)
        } else {
            result.or(outcome)
        };
    }
    replay_guard_marker(
        context,
        cursor,
        if all { "all[...]" } else { "any[...]" },
        result,
        diagnostics,
    )
}

/// Evaluation's recorded tri-state outcome, deriving `Matched`/`NotMatched`
/// from `matched` when `outcome` is absent (every `0.2` ledger and every
/// non-`Unmeasurable` `0.3` evaluation).
fn recorded_outcome(evaluation: &ConditionEvaluation) -> GuardOutcome {
    evaluation
        .outcome
        .unwrap_or_else(|| GuardOutcome::from_bool(evaluation.matched))
}

fn replay_guard_marker(
    context: GuardReplayContext<'_>,
    cursor: &mut usize,
    label: &str,
    expected: GuardOutcome,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    let (index, evaluation) = take_branch_guard_evaluation(context, cursor, diagnostics)?;
    validate_guard_label(index, evaluation, label, diagnostics);
    if evaluation.comparison_evidence.is_some() || recorded_outcome(evaluation) != expected {
        diagnostics.push(format!(
            "guard-evaluations[{index}] aggregate result does not replay from its children"
        ));
    }
    Some(expected)
}

fn replay_guard_leaf(
    context: GuardReplayContext<'_>,
    cursor: &mut usize,
    label: &str,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    let (index, evaluation) = take_branch_guard_evaluation(context, cursor, diagnostics)?;
    validate_guard_label(index, evaluation, label, diagnostics);
    if evaluation.comparison_evidence.is_some() {
        diagnostics.push(format!(
            "guard-evaluations[{index}] non-comparison atom carries comparison evidence"
        ));
    }
    Some(recorded_outcome(evaluation))
}

/// Replay a `present` leaf by recomputing its outcome from evidence VISIBLE
/// AT THE GUARD'S OWN DECISION POSITION rather than trusting the recorded
/// evaluation OR reading the ledger's final accumulated state — the same
/// boundary [`replay_declared_comparison`] enforces via
/// `context.position_path`/`context.watermark` for every other recomputing
/// replay path in this file. A subject accepted (or a loop iteration that
/// makes a repeated slot stale) AFTER this decision must stay invisible to
/// the recomputation exactly as it was to the live evaluator; see
/// [`present_subject_at_decision`].
fn replay_present_predicate(
    context: GuardReplayContext<'_>,
    cursor: &mut usize,
    subject_ref: &str,
    predicate: &crate::r#trait::condition::GuardPredicate,
    diagnostics: &mut Vec<String>,
) -> Option<GuardOutcome> {
    let field = predicate.field.as_deref();
    let label = match field {
        Some(field_name) => format!("present({subject_ref}).{field_name}"),
        None => format!("present({subject_ref})"),
    };
    let (index, evaluation) = take_branch_guard_evaluation(context, cursor, diagnostics)?;
    validate_guard_label(index, evaluation, &label, diagnostics);
    if evaluation.comparison_evidence.is_some() {
        diagnostics.push(format!(
            "guard-evaluations[{index}] present atom carries comparison evidence"
        ));
    }
    let subject = present_subject_at_decision(context, subject_ref);
    let replayed = if subject.stale {
        GuardOutcome::Unmeasurable
    } else {
        match (subject.accepted, field) {
            (false, None) => GuardOutcome::NotMatched,
            (false, Some(_)) => GuardOutcome::Unmeasurable,
            (true, None) => GuardOutcome::Matched,
            (true, Some(field_name)) => match subject.value {
                // The subject was accepted but its exact value could not be
                // reconstructed from recorded revision evidence — the field's
                // presence genuinely cannot be determined, not "absent".
                None => GuardOutcome::Unmeasurable,
                Some(value) => {
                    if value.as_object().is_some_and(|object| object.contains_key(field_name)) {
                        GuardOutcome::Matched
                    } else {
                        GuardOutcome::NotMatched
                    }
                }
            },
        }
    };
    if recorded_outcome(evaluation) != replayed {
        diagnostics.push(format!(
            "guard-evaluations[{index}] present does not replay from accepted evidence"
        ));
    }
    Some(replayed)
}

/// One `present` subject's boundary-resolved state: whether it was accepted
/// at all (visible to the decision), its reconstructed value if any, and
/// whether that acceptance is stale (written by an earlier iteration of the
/// evaluated loop).
struct PresentSubjectAtDecision {
    accepted: bool,
    value: Option<JsonValue>,
    stale: bool,
}

/// Resolves a `present` subject's evidence exactly as visible at the guard's
/// own decision boundary — the boundary-aware counterpart of
/// [`super::guards::accepted_value`]/[`super::guards::stale_repeated_slot`],
/// which are only correct against a *live* `State` because that state IS the
/// state at evaluation time. A port is visible unconditionally once accepted
/// (ports are never revised, matching [`validate_comparison_operand`]'s Port
/// arm, which applies no boundary check either). A slot is visible only
/// through its latest revision whose acceptance order is within
/// `context.watermark` and whose position the decision can see — exactly
/// [`latest_visible_slot_revision_at_decision`]'s own contract — with
/// staleness derived the same boundary-aware way via
/// [`slot_revision_stale_at_path`] rather than the live-only
/// `stale_repeated_slot`.
fn present_subject_at_decision(
    context: GuardReplayContext<'_>,
    subject_ref: &str,
) -> PresentSubjectAtDecision {
    let Ok(reference) = Reference::parse(subject_ref) else {
        return PresentSubjectAtDecision { accepted: false, value: None, stale: false };
    };
    if reference.kind() == Kind::Slot {
        match latest_visible_slot_revision_at_decision(
            context.ledger,
            subject_ref,
            context.position_path,
            context.watermark,
        ) {
            Some(revision) => PresentSubjectAtDecision {
                accepted: true,
                value: replayed_revision_value(revision),
                stale: slot_revision_stale_at_path(revision, context.position_path),
            },
            None => PresentSubjectAtDecision { accepted: false, value: None, stale: false },
        }
    } else {
        let accepted_port = recorded_port_values(context.ledger, false)
            .into_iter()
            .find(|value| value.ref_text == subject_ref && value.acceptance == AcceptanceStatus::Accepted);
        PresentSubjectAtDecision {
            accepted: accepted_port.is_some(),
            value: accepted_port.map(|value| value.value.clone()),
            stale: false,
        }
    }
}

/// Proves `replay_present_predicate`/`present_subject_at_decision` resolve a
/// `present`+`field` subject through the decision boundary rather than the
/// ledger's final accumulated state: a slot accepted AFTER the guard's own
/// decision position must stay invisible to the recomputation, exactly as it
/// was to the live evaluator that recorded `Unmeasurable`.
#[cfg(test)]
mod present_replay_boundary_tests {
    use super::*;

    fn decision_position() -> Vec<PathSegment> {
        vec![PathSegment {
            kind: "item".to_string(),
            id: Some("check-cap".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }]
    }

    fn later_position() -> Vec<PathSegment> {
        vec![PathSegment {
            kind: "item".to_string(),
            id: Some("produce-report".to_string()),
            index: 1,
            iteration: None,
            item_index: None,
        }]
    }

    fn present_field_predicate() -> crate::r#trait::condition::GuardPredicate {
        crate::r#trait::condition::GuardPredicate {
            present: Some("slot:evaluator-result".to_string()),
            field: Some("cost-microusd".to_string()),
            ..Default::default()
        }
    }

    /// A `slot:evaluator-result` write accepted AFTER the guard's own
    /// decision position (`acceptance_order` 1, position `later_position()`)
    /// — the §5 fail-closed cap pattern's exact shape: the report container
    /// is produced by a LATER step in the same run.
    fn late_slot_revision() -> SlotRevision {
        let value = serde_json::json!({ "cost-microusd": 3 });
        SlotRevision {
            slot_ref: Reference::parse("slot:evaluator-result").expect("valid ref"),
            value_digest: crate::digest::canonical_digest(&value).expect("digest"),
            acceptance_order: 1,
            operation: Some(WriteOperation::Replace),
            submitted_payload: Some(RevisionValue { value }),
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: None,
            runtime_binding: false,
            projection: None,
            position_path: later_position(),
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    fn ledger_with(evaluation: ConditionEvaluation) -> State {
        State {
            run_id: Id::new("run-present-replay-test").expect("id"),
            trait_id: "present-replay-test".to_string(),
            strict_loops: false,
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            sequence_statuses: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: vec![late_slot_revision()],
            resource_evidence: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_attempts: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            branch_decisions: Vec::new(),
            conditional_input_decisions: Vec::new(),
            ask_decisions: Vec::new(),
            failure_routes: Vec::new(),
            guard_evaluations: vec![evaluation],
            parallel_panel_records: Vec::new(),
            stop_reason: None,
            elapsed_seconds: 0,
            final_state: FinalState::Running,
        }
    }

    /// Exactly what the live evaluator recorded at the decision: the report
    /// container was not yet supplied, so `present(...).cost-microusd` is
    /// `Unmeasurable`, not `NotMatched`.
    fn unmeasurable_evaluation() -> ConditionEvaluation {
        ConditionEvaluation {
            predicate: "present(slot:evaluator-result).cost-microusd".to_string(),
            evidence_ref: Some("slot:evaluator-result".to_string()),
            scope: Some(ConditionEvaluationScope {
                loop_id: String::new(),
                sequence_id: None,
                iteration_index: 0,
                max_iterations: Some(1),
            }),
            comparison_evidence: None,
            outcome: Some(GuardOutcome::Unmeasurable),
            matched: false,
            reason: "container was not supplied; field presence is unmeasurable".to_string(),
        }
    }

    fn minimal_trait() -> crate::r#trait::Trait {
        crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            "id = \"present-replay-test\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Present replay test\"\nsummary = \"Minimal fixture.\"\n",
        )
        .expect("minimal trait decodes")
    }

    #[test]
    fn present_replay_ignores_evidence_accepted_after_the_decision() {
        let trait_ref = minimal_trait();
        let ledger = ledger_with(unmeasurable_evaluation());
        let position = decision_position();
        let context = GuardReplayContext {
            trait_ref: &trait_ref,
            ledger: &ledger,
            label: "present-replay-test",
            position_path: &position,
            // The late write's acceptance-order (1) is past this watermark —
            // it must stay invisible to the recomputation.
            watermark: 0,
            end: 1,
        };
        let mut cursor = 0;
        let mut diagnostics = Vec::new();
        let outcome = replay_present_predicate(
            context,
            &mut cursor,
            "slot:evaluator-result",
            &present_field_predicate(),
            &mut diagnostics,
        );
        assert!(diagnostics.is_empty(), "expected zero diagnostics, got {diagnostics:?}");
        assert_eq!(outcome, Some(GuardOutcome::Unmeasurable));
    }

    #[test]
    fn present_subject_at_decision_sees_the_later_write_only_once_the_watermark_admits_it() {
        // Proves the boundary argument is load-bearing, not decorative:
        // raising the watermark past the later write's acceptance order
        // flips the same subject to accepted — exactly the false mismatch
        // an unbounded (pre-fix) recomputation would have raised against a
        // legitimate `Unmeasurable` recorded evaluation.
        let trait_ref = minimal_trait();
        let ledger = ledger_with(unmeasurable_evaluation());
        let position = decision_position();
        let bounded = present_subject_at_decision(
            GuardReplayContext {
                trait_ref: &trait_ref,
                ledger: &ledger,
                label: "present-replay-test",
                position_path: &position,
                watermark: 0,
                end: 1,
            },
            "slot:evaluator-result",
        );
        assert!(!bounded.accepted, "the later write must be invisible below its acceptance order");
        let admitted = present_subject_at_decision(
            GuardReplayContext {
                trait_ref: &trait_ref,
                ledger: &ledger,
                label: "present-replay-test",
                position_path: &position,
                watermark: 1,
                end: 1,
            },
            "slot:evaluator-result",
        );
        assert!(admitted.accepted, "raising the watermark should make the later write visible");
    }

    #[test]
    fn present_replay_rejects_a_tampered_recorded_outcome() {
        let trait_ref = minimal_trait();
        let mut tampered = unmeasurable_evaluation();
        tampered.outcome = None;
        tampered.matched = true; // claims Matched even though the boundary still says Unmeasurable
        let ledger = ledger_with(tampered);
        let position = decision_position();
        let context = GuardReplayContext {
            trait_ref: &trait_ref,
            ledger: &ledger,
            label: "present-replay-test",
            position_path: &position,
            watermark: 0,
            end: 1,
        };
        let mut cursor = 0;
        let mut diagnostics = Vec::new();
        replay_present_predicate(
            context,
            &mut cursor,
            "slot:evaluator-result",
            &present_field_predicate(),
            &mut diagnostics,
        );
        assert!(
            diagnostics.iter().any(|message| message.contains("present does not replay")),
            "expected a replay mismatch diagnostic, got {diagnostics:?}"
        );
    }
}

fn take_branch_guard_evaluation<'a>(
    context: GuardReplayContext<'a>,
    cursor: &mut usize,
    diagnostics: &mut Vec<String>,
) -> Option<(usize, &'a ConditionEvaluation)> {
    if *cursor >= context.end {
        diagnostics.push(format!(
            "{} guard evaluation range is missing an entry",
            context.label
        ));
        return None;
    }
    let index = *cursor;
    *cursor = cursor.saturating_add(1);
    context
        .ledger
        .guard_evaluations
        .get(index)
        .map(|evaluation| (index, evaluation))
        .or_else(|| {
            diagnostics.push(format!(
                "{} guard evaluation range exceeds the ledger",
                context.label
            ));
            None
        })
}

fn validate_guard_label(
    index: usize,
    evaluation: &ConditionEvaluation,
    expected: &str,
    diagnostics: &mut Vec<String>,
) {
    if evaluation.predicate != expected {
        diagnostics.push(format!(
            "guard-evaluations[{index}] predicate label does not match the declared guard"
        ));
    }
}

fn operand_matches_declared_ref(
    operand: &ComparisonOperandEvidence,
    expected_ref: &str,
    expected_field: Option<&str>,
) -> bool {
    match operand {
        ComparisonOperandEvidence::Ref {
            ref_text, field, ..
        }
        | ComparisonOperandEvidence::MissingRef { ref_text, field } => {
            ref_text == expected_ref && field.as_deref() == expected_field
        }
        ComparisonOperandEvidence::Literal { .. } => false,
    }
}

fn validate_comparison_guard_evidence(
    ledger: &State,
    index: usize,
    evaluation: &ConditionEvaluation,
    evidence: &ConditionComparisonEvidence,
    boundary: Option<(&[PathSegment], usize)>,
    diagnostics: &mut Vec<String>,
) -> bool {
    let lhs_ref = comparison_operand_ref(&evidence.lhs).unwrap_or_default();
    let lhs_kind = Reference::parse(lhs_ref).ok().map(|reference| reference.kind());
    let valid_lhs_kind = match evidence.subject {
        ConditionComparisonSubject::Slot => lhs_kind == Some(Kind::Slot),
        ConditionComparisonSubject::Output => {
            matches!(lhs_kind, Some(Kind::Slot | Kind::Port | Kind::Schema))
        }
        // The elapsed LHS has no backing ref — it is the exact runtime
        // evidence value embedded as a literal at evaluation time.
        ConditionComparisonSubject::Elapsed => {
            matches!(&evidence.lhs, ComparisonOperandEvidence::Literal { value } if value.as_u64().is_some())
        }
    };
    if !valid_lhs_kind {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison LHS ref does not match its subject"
        ));
    }
    if evidence.subject != ConditionComparisonSubject::Elapsed
        && evaluation.scope.is_some()
        && evaluation.evidence_ref.as_deref() != Some(lhs_ref)
    {
        diagnostics.push(format!(
            "guard-evaluations[{index}] evidence-ref does not match its comparison LHS ref"
        ));
    }
    let valid_rhs = match (&evidence.operator, &evidence.rhs) {
        (ConditionComparisonOperator::Equals, ComparisonOperandEvidence::Literal { .. }) => true,
        (ConditionComparisonOperator::Equals, _) => false,
        (_, ComparisonOperandEvidence::Literal { value }) => value.is_number(),
        (
            _,
            ComparisonOperandEvidence::Ref { ref_text, field, .. }
            | ComparisonOperandEvidence::MissingRef { ref_text, field },
        ) => {
            field.is_none()
                && Reference::parse(ref_text).is_ok_and(|reference| {
                    !reference.is_qualified()
                        && matches!(reference.kind(), Kind::Slot | Kind::Port)
                })
        }
    };
    if !valid_rhs {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison RHS form does not match its operator"
        ));
    }
    validate_comparison_operand(
        ledger,
        index,
        &evidence.lhs,
        evidence.subject == ConditionComparisonSubject::Output,
        boundary,
        diagnostics,
    );
    validate_comparison_operand(
        ledger,
        index,
        &evidence.rhs,
        false,
        boundary,
        diagnostics,
    );
    let replayed_result =
        comparison_result(evidence.operator, &evidence.lhs, &evidence.rhs, evidence.subject);
    if evidence.result != replayed_result {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison result does not match its operands and operator"
        ));
    }
    let matched = !evidence.stale && replayed_result;
    if evaluation.matched != matched {
        diagnostics.push(format!(
            "guard-evaluations[{index}] matched does not replay from comparison evidence"
        ));
    }
    validate_comparison_freshness(ledger, index, evaluation, evidence, boundary, diagnostics);

    let lhs_field = comparison_operand_field(&evidence.lhs);
    let lhs_label = match (evidence.subject, lhs_field) {
        (ConditionComparisonSubject::Slot, Some(field)) => format!("{lhs_ref}.{field}"),
        (ConditionComparisonSubject::Slot, None) => lhs_ref.to_string(),
        (ConditionComparisonSubject::Output, Some(field)) => {
            format!("output({lhs_ref}).{field}")
        }
        (ConditionComparisonSubject::Output, None) => format!("output({lhs_ref})"),
        (ConditionComparisonSubject::Elapsed, _) => "elapsed-seconds".to_string(),
    };
    let rhs_label = match &evidence.rhs {
        ComparisonOperandEvidence::Ref { ref_text, .. }
        | ComparisonOperandEvidence::MissingRef { ref_text, .. } => {
            serde_json::json!({ "ref": ref_text }).to_string()
        }
        ComparisonOperandEvidence::Literal { value } => value.to_string(),
    };
    if evaluation.predicate
        != format!("{lhs_label} {} {rhs_label}", evidence.operator.symbol())
    {
        diagnostics.push(format!(
            "guard-evaluations[{index}] predicate does not match its comparison evidence"
        ));
    }
    matched
}

fn validate_comparison_operand(
    ledger: &State,
    index: usize,
    operand: &ComparisonOperandEvidence,
    allow_output_source: bool,
    boundary: Option<(&[PathSegment], usize)>,
    diagnostics: &mut Vec<String>,
) {
    let ComparisonOperandEvidence::Ref {
        ref_text,
        source_value_digest,
        source_value,
        field,
        selected_value,
        slot_revision_acceptance_order,
    } = operand
    else {
        if let ComparisonOperandEvidence::MissingRef { ref_text, .. } = operand
            && let Some((decision_path, watermark)) = boundary
            && Reference::parse(ref_text).is_ok_and(|reference| reference.kind() == Kind::Slot)
            && latest_visible_slot_revision_at_decision(ledger, ref_text, decision_path, watermark)
                .is_some()
        {
            diagnostics.push(format!(
                "guard-evaluations[{index}] records missing slot evidence despite a visible accepted revision"
            ));
        }
        return;
    };
    let Ok(reference) = Reference::parse(ref_text) else {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison operand ref {ref_text:?} is invalid"
        ));
        return;
    };
    if reference.is_qualified() {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison operand ref must be local"
        ));
        return;
    }
    let embedded_matches = source_value.as_ref().is_some_and(|value| {
        value_digest(value).is_ok_and(|digest| &digest == source_value_digest)
            && select_json_value(value, field.as_deref()) == selected_value.as_ref()
    });
    match reference.kind() {
        Kind::Slot => {
            let matching_revisions: Vec<_> = slot_revision_acceptance_order.map_or_else(Vec::new, |order| {
                recorded_slot_revisions(ledger)
                    .into_iter()
                    .filter(|revision| revision.acceptance_order == order)
                    .collect()
            });
            if matching_revisions.len() != 1
                || matching_revisions.first().is_none_or(|revision| {
                    revision.slot_ref.as_str() != ref_text
                        || &revision.value_digest != source_value_digest
                        || !replayed_revision_value(revision).is_some_and(|value| {
                            value_digest(&value)
                                .is_ok_and(|digest| &digest == source_value_digest)
                                && select_json_value(&value, field.as_deref())
                                    == selected_value.as_ref()
                        })
                })
            {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] slot operand does not match its exact revision order, digest, field, and value"
                ));
            }
            if source_value.is_some() && (!allow_output_source || !embedded_matches) {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] embedded slot source value is invalid"
                ));
            }
            if let Some((decision_path, watermark)) = boundary
                && latest_visible_slot_revision_at_decision(ledger, ref_text, decision_path, watermark)
                    .map(|revision| revision.acceptance_order)
                    != *slot_revision_acceptance_order
            {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] slot operand is not the latest visible revision at the branch decision"
                ));
            }
        }
        Kind::Port => {
            if slot_revision_acceptance_order.is_some() {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] port operand must not carry slot revision order"
                ));
            }
            let recorded_matches = recorded_port_values(ledger, allow_output_source)
                .into_iter()
                .any(|value| {
                    value.acceptance == AcceptanceStatus::Accepted
                        && value.ref_text == *ref_text
                        && value.value_digest == *source_value_digest
                        && value_digest(&value.value)
                            .is_ok_and(|digest| digest == *source_value_digest)
                        && select_json_value(&value.value, field.as_deref())
                            == selected_value.as_ref()
                });
            if !(recorded_matches || allow_output_source && embedded_matches) {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] port operand does not match accepted runtime evidence"
                ));
            }
            if source_value.is_some() && (!allow_output_source || !embedded_matches) {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] embedded port source value is invalid"
                ));
            }
        }
        Kind::Schema if allow_output_source => {
            if slot_revision_acceptance_order.is_some() || !embedded_matches {
                diagnostics.push(format!(
                    "guard-evaluations[{index}] schema output operand has invalid embedded source evidence"
                ));
            }
        }
        _ => diagnostics.push(format!(
            "guard-evaluations[{index}] comparison operand ref kind is not valid for this atom"
        )),
    }
}

fn validate_comparison_freshness(
    ledger: &State,
    index: usize,
    evaluation: &ConditionEvaluation,
    evidence: &ConditionComparisonEvidence,
    boundary: Option<(&[PathSegment], usize)>,
    diagnostics: &mut Vec<String>,
) {
    const STALE_REASON: &str =
        "accepted slot evidence is stale (written in an earlier iteration of this loop)";
    if matches!(
        evidence.subject,
        ConditionComparisonSubject::Output | ConditionComparisonSubject::Elapsed
    ) {
        if evidence.stale {
            diagnostics.push(format!(
                "guard-evaluations[{index}] output/elapsed comparison cannot be stale"
            ));
        }
        return;
    }
    if evidence.stale != (evaluation.reason == STALE_REASON) {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison freshness does not match its reason"
        ));
    }
    let Some(order) = comparison_operand_slot_order(&evidence.lhs) else {
        if evidence.stale {
            diagnostics.push(format!(
                "guard-evaluations[{index}] missing slot evidence cannot be stale"
            ));
        }
        return;
    };
    let revision = recorded_slot_revisions(ledger)
        .into_iter()
        .find(|revision| revision.acceptance_order == order);
    let replayed_stale = match (revision, boundary, evaluation.scope.as_ref()) {
        (Some(revision), Some((decision_path, _)), _) => {
            slot_revision_stale_at_path(revision, decision_path)
        }
        (Some(revision), None, Some(scope)) => {
            revision.loop_id.as_deref() == Some(scope.loop_id.as_str())
                && revision.iteration_index != Some(scope.iteration_index)
        }
        _ => false,
    };
    if replayed_stale != evidence.stale {
        diagnostics.push(format!(
            "guard-evaluations[{index}] comparison stale state does not match its exact slot revision"
        ));
    }
}

fn comparison_operand_ref(operand: &ComparisonOperandEvidence) -> Option<&str> {
    match operand {
        ComparisonOperandEvidence::Ref { ref_text, .. }
        | ComparisonOperandEvidence::MissingRef { ref_text, .. } => Some(ref_text),
        ComparisonOperandEvidence::Literal { .. } => None,
    }
}

fn comparison_operand_field(operand: &ComparisonOperandEvidence) -> Option<&str> {
    match operand {
        ComparisonOperandEvidence::Ref { field, .. }
        | ComparisonOperandEvidence::MissingRef { field, .. } => field.as_deref(),
        ComparisonOperandEvidence::Literal { .. } => None,
    }
}

fn comparison_operand_slot_order(operand: &ComparisonOperandEvidence) -> Option<usize> {
    match operand {
        ComparisonOperandEvidence::Ref {
            slot_revision_acceptance_order,
            ..
        } => *slot_revision_acceptance_order,
        ComparisonOperandEvidence::MissingRef { .. }
        | ComparisonOperandEvidence::Literal { .. } => None,
    }
}

fn select_json_value<'a>(value: &'a JsonValue, field: Option<&str>) -> Option<&'a JsonValue> {
    match field {
        Some(field) => crate::shared::resolve_field_path(value, field),
        None => Some(value),
    }
}

fn replayed_revision_value(revision: &SlotRevision) -> Option<JsonValue> {
    let operation = revision.operation.as_ref()?;
    let submitted = revision.submitted_payload.as_ref()?;
    apply_write_operation_value(
        operation,
        revision.prior_value.as_ref().map(|prior| &prior.value),
        &submitted.value,
    )
    .ok()
}

fn recorded_slot_revisions(ledger: &State) -> Vec<&SlotRevision> {
    let mut revisions: Vec<&SlotRevision> = ledger.slot_revisions.iter().collect();
    for frame in &ledger.control_stack {
        revisions.extend(frame.parallel_buffer.slot_revisions.iter());
        for branch in &frame.parallel_committed_branches {
            revisions.extend(branch.slot_revisions.iter());
        }
    }
    revisions.sort_by_key(|revision| revision.acceptance_order);
    revisions
}

/// Every persisted isolation buffer, including completed sibling branches.
/// Callers validate each buffer independently so equal refs in isolated
/// siblings are not mistaken for duplicate currently visible values.
fn recorded_effect_buffers(ledger: &State) -> Vec<&EffectBuffer> {
    let mut buffers = Vec::new();
    for frame in &ledger.control_stack {
        buffers.push(&frame.parallel_buffer);
        buffers.extend(frame.parallel_committed_branches.iter());
    }
    buffers
}

fn recorded_emitted_signals(ledger: &State) -> Vec<&SignalEmission> {
    let mut signals: Vec<&SignalEmission> = ledger.emitted_signals.iter().collect();
    for buffer in recorded_effect_buffers(ledger) {
        signals.extend(buffer.emitted_signals.iter());
    }
    signals
}

fn latest_visible_slot_revision_before<'a>(
    ledger: &'a State,
    revision: &SlotRevision,
) -> Option<&'a SlotRevision> {
    recorded_slot_revisions(ledger)
        .into_iter()
        .filter(|candidate| {
            candidate.slot_ref == revision.slot_ref
                && candidate.acceptance_order < revision.acceptance_order
                && revision_visible_at_decision(
                    &candidate.position_path,
                    &revision.position_path,
                )
        })
        .max_by_key(|candidate| candidate.acceptance_order)
}

fn recorded_port_values(ledger: &State, allow_output_port: bool) -> Vec<&Value> {
    let mut values: Vec<&Value> = ledger.accepted_port_values.iter().collect();
    if !allow_output_port {
        return values;
    }
    values.extend(ledger.accepted_output_port_values.iter());
    for frame in &ledger.control_stack {
        values.extend(frame.parallel_buffer.accepted_output_port_values.iter());
        for branch in &frame.parallel_committed_branches {
            values.extend(branch.accepted_output_port_values.iter());
        }
    }
    values
}

/// The greatest slot-revision acceptance order visible before entering
/// `decision_path` — the boundary a branch selection or P290
/// conditional-input decision's own comparison operands must not exceed.
fn expected_slot_revision_watermark_before(ledger: &State, decision_path: &[PathSegment]) -> usize {
    recorded_slot_revisions(ledger)
        .into_iter()
        .filter(|revision| revision_precedes_decision_position(&revision.position_path, decision_path))
        .map(|revision| revision.acceptance_order)
        .max()
        .unwrap_or(0)
}

/// Structural paths put prior siblings/iterations before the decision's own
/// position and put its children (e.g. a branch's selected arm) after it.
/// This derives the boundary independently instead of trusting a tampered
/// watermark.
fn revision_precedes_decision_position(
    revision_path: &[PathSegment],
    decision_path: &[PathSegment],
) -> bool {
    for (index, (revision, decision)) in revision_path.iter().zip(decision_path).enumerate() {
        if revision == decision {
            continue;
        }
        if index + 1 == decision_path.len()
            && decision.kind == "item"
            && revision.kind != "item"
        {
            return false;
        }
        let revision_order = (
            revision.iteration.unwrap_or(0),
            revision.item_index.unwrap_or(0),
            revision.index,
        );
        let decision_order = (
            decision.iteration.unwrap_or(0),
            decision.item_index.unwrap_or(0),
            decision.index,
        );
        return revision_order < decision_order;
    }
    revision_path.len() < decision_path.len()
}

fn latest_visible_slot_revision_at_decision<'a>(
    ledger: &'a State,
    slot_ref: &str,
    decision_path: &[PathSegment],
    watermark: usize,
) -> Option<&'a SlotRevision> {
    recorded_slot_revisions(ledger)
        .into_iter()
        .filter(|revision| {
            revision.slot_ref.as_str() == slot_ref
                && revision.acceptance_order <= watermark
                && revision_visible_at_decision(&revision.position_path, decision_path)
        })
        .max_by_key(|revision| revision.acceptance_order)
}

/// Parallel siblings execute earlier but remain isolated until the barrier;
/// only revisions from the decision's own branch are visible inside a panel.
fn revision_visible_at_decision(
    revision_path: &[PathSegment],
    decision_path: &[PathSegment],
) -> bool {
    for (index, decision_segment) in decision_path.iter().enumerate() {
        if decision_segment.kind != "parallel" || revision_path.get(..index) != decision_path.get(..index) {
            continue;
        }
        if let Some(revision_segment) = revision_path.get(index)
            && revision_segment.kind == "parallel"
            && revision_segment.iteration != decision_segment.iteration
        {
            return false;
        }
    }
    true
}

fn slot_revision_stale_at_path(revision: &SlotRevision, decision_path: &[PathSegment]) -> bool {
    let revision_scope = repeated_activation_scope(&revision.position_path);
    let decision_scope = repeated_activation_scope(decision_path);
    for (revision, current) in revision_scope.iter().zip(&decision_scope) {
        if !same_repeated_control(revision, current) {
            return false;
        }
        if revision.iteration != current.iteration || revision.item_index != current.item_index {
            return true;
        }
    }
    false
}

/// Validate append-only `parallel-panel-records` (P264) invariants: a result
/// digest only ever accompanies a `completed` disposition and links to a
/// recorded slot revision, a `quorum-verdict` record's stored guard
/// evaluation agrees with its disposition (a matched guard is `completed`
/// only; an unmatched one never is), and `guard-evaluation-index` is present
/// only for `quorum-verdict` records.
fn validate_parallel_panel_records_contract(ledger: &State, diagnostics: &mut Vec<String>) {
    for (index, record) in ledger.parallel_panel_records.iter().enumerate() {
        if record.branches.is_empty() {
            diagnostics.push(format!(
                "parallel-panel-records[{index}] must declare at least one authored branch"
            ));
        }
        if record.disposition != ParallelPanelDisposition::Completed && record.result_digest.is_some() {
            diagnostics.push(format!(
                "parallel-panel-records[{index}] result-digest must be absent unless disposition is completed"
            ));
        }
        if let Some(digest) = record.result_digest.as_ref()
            && !recorded_slot_revisions(ledger)
                .into_iter()
                .any(|revision| &revision.value_digest == digest)
        {
            diagnostics.push(format!(
                "parallel-panel-records[{index}] result-digest does not match any recorded slot revision"
            ));
        }
        if record.join_policy == "quorum-verdict" {
            match record.guard_evaluation_index {
                None => diagnostics.push(format!(
                    "parallel-panel-records[{index}] quorum-verdict record must carry guard-evaluation-index"
                )),
                Some(guard_index) => match ledger.guard_evaluations.get(guard_index) {
                    None => diagnostics.push(format!(
                        "parallel-panel-records[{index}] guard-evaluation-index {guard_index} exceeds guard-evaluations length {}",
                        ledger.guard_evaluations.len()
                    )),
                    Some(evaluation) => {
                        if evaluation.matched && record.disposition != ParallelPanelDisposition::Completed {
                            diagnostics.push(format!(
                                "parallel-panel-records[{index}] matched quorum-verdict guard must have disposition completed"
                            ));
                        }
                        if !evaluation.matched && record.disposition == ParallelPanelDisposition::Completed {
                            diagnostics.push(format!(
                                "parallel-panel-records[{index}] unmatched quorum-verdict guard must not have disposition completed"
                            ));
                        }
                    }
                },
            }
        } else if record.guard_evaluation_index.is_some() {
            diagnostics.push(format!(
                "parallel-panel-records[{index}] guard-evaluation-index is valid only for quorum-verdict records"
            ));
        }
    }
}

fn validate_stop_reason_contract(
    ledger: &State,
    stop_reason: &StopReason,
    diagnostics: &mut Vec<String>,
) {
    if !RUNTIME_STOP_REASON_TOKENS.contains(&stop_reason.reason.as_str()) {
        diagnostics.push(format!(
            "stop-reason.reason {} is not a known runtime stop reason",
            stop_reason.reason
        ));
    }
    if matches!(
        ledger.final_state,
        FinalState::Running | FinalState::Completed
    ) {
        diagnostics.push(format!(
            "{} ledger must not carry stop-reason",
            final_state_label(&ledger.final_state)
        ));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceContract {
    by_run: BTreeMap<usize, SequenceContractItem>,
    by_declaration: BTreeMap<usize, SequenceContractItem>,
    slot_producers: BTreeMap<String, Vec<SequenceProducer>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceContractItem {
    run_index: usize,
    declaration_index: usize,
    item_id: Option<String>,
    executable: bool,
    output_refs: Vec<String>,
    emits: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceProducer {
    run_index: usize,
    declaration_index: usize,
    item_id: Option<String>,
}

impl SequenceContract {
    fn new(sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>]) -> Self {
        let mut by_run = BTreeMap::new();
        let mut by_declaration = BTreeMap::new();
        let mut slot_producers: BTreeMap<String, Vec<SequenceProducer>> = BTreeMap::new();

        for item in sequence {
            let output_refs: Vec<String> =
                item.item.output.ref_texts().map(str::to_string).collect();
            let contract_item = SequenceContractItem {
                run_index: item.run_index,
                declaration_index: item.declaration_index,
                item_id: item.item.id.clone(),
                executable: is_executable_item(item.item),
                output_refs: output_refs.clone(),
                emits: item
                    .item
                    .emits
                    .iter()
                    .map(|emit| emit.signal_ref().to_string())
                    .collect(),
            };
            by_run.insert(item.run_index, contract_item.clone());
            by_declaration.insert(item.declaration_index, contract_item);
            for slot_ref in output_refs.into_iter().filter(|ref_text| {
                Reference::parse(ref_text)
                    .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
            }) {
                slot_producers
                    .entry(slot_ref)
                    .or_default()
                    .push(SequenceProducer {
                        run_index: item.run_index,
                        declaration_index: item.declaration_index,
                        item_id: item.item.id.clone(),
                    });
            }
        }

        Self {
            by_run,
            by_declaration,
            slot_producers,
        }
    }
}

struct SequenceStatusMaps<'a> {
    by_run: BTreeMap<usize, &'a SequenceStatus>,
    by_declaration: BTreeMap<usize, &'a SequenceStatus>,
}
