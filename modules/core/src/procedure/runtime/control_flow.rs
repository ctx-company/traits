// Procedure runtime control flow.
// Procedure runtime control flow.

fn refresh_runtime_status(trait_ref: &Trait, state: &mut State) -> crate::Result<()> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    state.output_ports = finalize_outputs(trait_ref, state)?;
    if state.final_state == FinalState::Rejected || state.final_state == FinalState::Failed {
        sort_state(state);
        return Ok(());
    }
    if state.stop_reason.is_some() {
        state.final_state = FinalState::Blocked;
        sort_state(state);
        return Ok(());
    }
    let missing_ports = missing_required_procedure_ports(trait_ref, state);
    if !missing_ports.is_empty() {
        state.final_state = FinalState::Blocked;
        state
            .provider_capability_reports
            .push(CapabilityReport::unsupported(
                "runtime.input-port-binding",
                format!(
                    "missing required input port value(s): {}",
                    missing_ports.join(", ")
                ),
            ));
        set_current_outer_status(
            state,
            SequenceStatusKind::Blocked,
            "missing required procedure input",
        );
        sort_state(state);
        return Ok(());
    }

    let progress_budget = MAX_SEQUENCE_NESTING_DEPTH
        .saturating_mul(4)
        .saturating_add(sequence.len())
        .saturating_add(nested_sequence_item_count(trait_ref))
        .saturating_add(control_progress_budget(trait_ref, state))
        .saturating_add(1)
        .clamp(1, MAX_CONTROL_ADVANCE_BUDGET);
    let mut exhausted_progress_budget = true;
    for _ in 0..progress_budget {
        let signal_count_before = state.emitted_signals.len();
        let failure_route_count_before = state.failure_routes.len();
        if progress_control_cursor(trait_ref, state, &sequence)? {
            if state.final_state == FinalState::Running
                && state.emitted_signals.len() > signal_count_before
                && state.failure_routes.len() == failure_route_count_before
            {
                evaluate_control_guards_after_step(trait_ref, state, &[])?;
            }
            if matches!(
                state.final_state,
                FinalState::Blocked | FinalState::Failed | FinalState::Rejected
            ) {
                // A control item may have committed a project write before a
                // guard makes this run terminal. Keep the externally returned
                // ledger synchronized with that newly committed evidence.
                state.output_ports = finalize_outputs(trait_ref, state)?;
                sort_state(state);
                return Ok(());
            }
            continue;
        }
        exhausted_progress_budget = false;
        break;
    }
    // The progress loop above can commit a barrier merge (a `parallel`
    // panel's final branch draining into the committed ledger via
    // `advance_or_complete_parallel`), which changes which slots back
    // required output ports. The entry-time `output_ports` snapshot taken
    // before that loop ran is stale by the time any blocking/completion
    // decision below is made, so re-derive it once from the now-current
    // ledger here — the same derivation every branch below (exhausted
    // budget, completed, blocked-on-missing-input) then shares, rather than
    // a barrier-specific special case recomputed only for completion.
    state.output_ports = finalize_outputs(trait_ref, state)?;

    if exhausted_progress_budget {
        stop_with_reason(
            state,
            FinalState::Blocked,
            STOP_CONTROL_ADVANCE_BUDGET_EXHAUSTED,
            state.active_path.clone(),
            None,
        );
        sort_state(state);
        return Ok(());
    }

    if state.current_run_index >= sequence.len() && state.control_stack.is_empty() {
        let missing_required_outputs = state
            .output_ports
            .iter()
            .any(|output| output.required && output.status != OutputPortStatus::Accepted);
        state.final_state = if missing_required_outputs {
            FinalState::Blocked
        } else {
            FinalState::Completed
        };
        sort_state(state);
        return Ok(());
    }

    let Some(ready) = current_ready_item(trait_ref, state, &sequence)? else {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_NO_CURRENT_EXECUTABLE_ITEM,
            state.active_path.clone(),
            None,
        );
        sort_state(state);
        return Ok(());
    };
    if ready.item.effective_kind() == SequenceKind::Ask {
        let Some(guard) = ready.item.when.as_ref() else {
            stop_with_reason(
                state,
                FinalState::Failed,
                "ask-missing-guard",
                ready.position_path.clone(),
                None,
            );
            sort_state(state);
            return Ok(());
        };
        let context = guard_context_from_stack(&state.control_stack).unwrap_or(LoopContext {
            loop_id: String::new(),
            sequence_id: None,
            iteration_index: 0,
            max_iterations: 1,
        });
        let position_path = producer_path_for_ready(&ready);
        let guard_evaluation_start_index = state.guard_evaluations.len();
        let slot_revision_watermark = latest_recorded_slot_revision_order(state);
        let (matched, evaluations) = evaluate_guard_expr(
            trait_ref,
            state,
            guard,
            &context,
            &position_path,
            &[],
        )?;
        append_guard_evaluations(state, evaluations);
        let marker_key = serde_json::to_string(&(
            ready.sequence_index,
            guard,
            &position_path,
        ))
        .map_err(|error| crate::procedure::serialization("runtime.ask.when", "ask guard", error))?;
        let guard_evaluation_index = append_guard_evaluations(
            state,
            vec![condition_evaluation(
                &format!("ask:{marker_key}"),
                None,
                &context,
                matched,
                "ask activation",
            )],
        )
        .expect("ask activation marker is not empty");
        if !state.ask_decisions.iter().any(|decision| {
            decision.sequence_index == ready.sequence_index && decision.position_path == position_path
        }) {
            state.ask_decisions.push(AskDecision {
                sequence_index: ready.sequence_index,
                position_path: position_path.clone(),
                matched,
                when: guard.clone(),
                guard_evaluation_start_index: Some(guard_evaluation_start_index),
                slot_revision_watermark: Some(slot_revision_watermark),
                guard_evaluation_index,
            });
        }
        if !matched {
            if ready.position_path.is_empty() {
                set_sequence_status(state, ready.sequence_index, SequenceStatusKind::Skipped, "ask signal guard did not match");
            } else {
                set_path_sequence_status(state, SequenceStatus {
                    sequence_index: ready.sequence_index,
                    run_index: ready.run_index,
                    item_id: ready.item.id.clone(),
                    title: ready.item.title.clone(),
                    status: SequenceStatusKind::Skipped,
                    reason: "ask signal guard did not match".to_string(),
                    position_path: ready.position_path.clone(),
                });
            }
            state.active_path = ready.position_path;
            advance_after_current_leaf(state);
            return refresh_runtime_status(trait_ref, state);
        }
    }
    record_conditional_input_decisions(trait_ref, state, &ready)?;
    let missing = missing_inputs_for_item(trait_ref, &ready, state)?;
    state.active_path = ready.position_path.clone();
    if missing.is_empty() {
        state.final_state = FinalState::Running;
        if ready.position_path.is_empty() {
            set_sequence_status(
                state,
                ready.sequence_index,
                SequenceStatusKind::Ready,
                "all current inputs available",
            );
        } else {
            set_path_sequence_status(
                state,
                SequenceStatus {
                    sequence_index: ready.sequence_index,
                    run_index: ready.run_index,
                    item_id: ready.item.id.clone(),
                    title: ready.item.title.clone(),
                    status: SequenceStatusKind::Ready,
                    reason: format!("nested item ready at {}", format_path(&ready.position_path)),
                    position_path: ready.position_path.clone(),
                },
            );
        }
    } else {
        state.final_state = FinalState::Blocked;
        if ready.position_path.is_empty() {
            set_sequence_status(
                state,
                ready.sequence_index,
                SequenceStatusKind::Blocked,
                format!("missing input(s): {}", missing.join(", ")),
            );
        } else {
            set_path_sequence_status(
                state,
                SequenceStatus {
                    sequence_index: ready.sequence_index,
                    run_index: ready.run_index,
                    item_id: ready.item.id.clone(),
                    title: ready.item.title.clone(),
                    status: SequenceStatusKind::Blocked,
                    reason: format!(
                        "nested item at {} missing input(s): {}",
                        format_path(&ready.position_path),
                        missing.join(", ")
                    ),
                    position_path: ready.position_path.clone(),
                },
            );
        }
        state
            .provider_capability_reports
            .push(CapabilityReport::unsupported(
                "runtime.sequence-input",
                format!("missing current sequence input(s): {}", missing.join(", ")),
            ));
    }
    for capability in dependency_capabilities_for_item(ready.item) {
        state.provider_capability_reports.push(capability);
    }
    sort_state(state);
    Ok(())
}

fn nested_sequence_item_count(trait_ref: &Trait) -> usize {
    trait_ref
        .sequences
        .iter()
        .map(|(_, sequence)| sequence.sequence.len())
        .sum()
}

fn control_progress_budget(trait_ref: &Trait, state: &State) -> usize {
    trait_ref.procedure.as_ref().map_or(0, |procedure| {
        sequence_control_progress_cost(
            trait_ref,
            state,
            &procedure.sequence,
            0,
            &mut BTreeSet::new(),
            &mut BTreeMap::new(),
        )
    })
}

fn sequence_control_progress_cost(
    trait_ref: &Trait,
    state: &State,
    items: &[crate::r#trait::procedure::SequenceItem],
    depth: usize,
    stack: &mut BTreeSet<String>,
    memo: &mut BTreeMap<String, usize>,
) -> usize {
    if depth > MAX_SEQUENCE_NESTING_DEPTH {
        return 1;
    }
    items.iter().fold(0usize, |total, item| {
        total.saturating_add(control_item_progress_cost(
            trait_ref, state, item, depth, stack, memo,
        ))
    })
}

fn control_item_progress_cost(
    trait_ref: &Trait,
    state: &State,
    item: &crate::r#trait::procedure::SequenceItem,
    depth: usize,
    stack: &mut BTreeSet<String>,
    memo: &mut BTreeMap<String, usize>,
) -> usize {
    if is_executable_item(item) {
        return 0;
    }
    if item.effective_kind() == SequenceKind::Project {
        return 1;
    }
    let body_cost = local_sequence_id(item.sequence.as_deref()).map_or(0, |sequence_id| {
        named_sequence_control_progress_cost(
            trait_ref,
            state,
            &sequence_id,
            depth + 1,
            stack,
            memo,
        )
    });
    let body_pass_cost = body_cost.saturating_add(1);
    match item.effective_kind() {
        SequenceKind::Sequence => body_pass_cost.saturating_add(1),
        SequenceKind::Branch => {
            let otherwise_cost = local_sequence_id(item.otherwise.as_deref()).map_or(0, |sequence_id| {
                named_sequence_control_progress_cost(
                    trait_ref,
                    state,
                    &sequence_id,
                    depth + 1,
                    stack,
                    memo,
                )
            });
            body_cost.max(otherwise_cost).saturating_add(2)
        }
        SequenceKind::Loop => resolved_loop_bound(item, state)
            .unwrap_or(1)
            .max(1)
            .saturating_mul(body_pass_cost)
            .saturating_add(1),
        SequenceKind::ForEach => item
            .max_items
            .unwrap_or(1)
            .max(1)
            .saturating_mul(body_pass_cost)
            .saturating_add(1),
        // Branches run strictly sequentially (P263): the progress budget is
        // the sum of every branch's own cost, not the max of the arms a
        // branch item picks between — every declared branch is actually
        // walked, one after another, on the single cursor.
        SequenceKind::Parallel => item
            .branches
            .iter()
            .fold(0usize, |total, branch_ref| {
                total.saturating_add(local_sequence_id(Some(branch_ref)).map_or(0, |sequence_id| {
                    named_sequence_control_progress_cost(
                        trait_ref,
                        state,
                        &sequence_id,
                        depth + 1,
                        stack,
                        memo,
                    )
                }))
            })
            .saturating_add(item.branches.as_slice().len())
            .saturating_add(1),
        SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => 0,
    }
}

fn named_sequence_control_progress_cost(
    trait_ref: &Trait,
    state: &State,
    sequence_id: &str,
    depth: usize,
    stack: &mut BTreeSet<String>,
    memo: &mut BTreeMap<String, usize>,
) -> usize {
    if let Some(cost) = memo.get(sequence_id) {
        return *cost;
    }
    if !stack.insert(sequence_id.to_string()) {
        return 1;
    }
    let cost = trait_ref.sequences.get(sequence_id).map_or(0, |sequence| {
        sequence_control_progress_cost(trait_ref, state, &sequence.sequence, depth, stack, memo)
    });
    stack.remove(sequence_id);
    memo.insert(sequence_id.to_string(), cost);
    cost
}

fn progress_control_cursor(
    trait_ref: &Trait,
    state: &mut State,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'_>],
) -> crate::Result<bool> {
    if state.control_stack.is_empty() {
        let Some(item) = sequence
            .iter()
            .find(|item| item.run_index == state.current_run_index)
        else {
            return Ok(false);
        };
        if item.item.effective_kind() == SequenceKind::Project {
            execute_project_item(
                trait_ref,
                state,
                item.item,
                item.run_index,
                item.declaration_index,
            )?;
            return Ok(true);
        }
        if is_executable_item(item.item) {
            return Ok(false);
        }
        enter_control_frame(
            trait_ref,
            state,
            item.item,
            item.run_index,
            item.declaration_index,
        )?;
        return Ok(true);
    }

    if complete_or_repeat_current_control(trait_ref, state)? {
        return Ok(true);
    }

    let Some(frame) = state.control_stack.last() else {
        return Ok(true);
    };
    let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_UNRESOLVED_RUNTIME_SEQUENCE,
            state.active_path.clone(),
            None,
        );
        return Ok(true);
    };
    let Some(item) = named.sequence.get(frame.next_index) else {
        return Ok(true);
    };
    if item.effective_kind() == SequenceKind::Project {
        execute_project_item(
            trait_ref,
            state,
            item,
            frame.parent_run_index,
            frame.next_index,
        )?;
        return Ok(true);
    }
    if is_executable_item(item) {
        return Ok(false);
    }
    enter_control_frame(
        trait_ref,
        state,
        item,
        frame.parent_run_index,
        frame.next_index,
    )?;
    Ok(true)
}

fn enter_control_frame(
    trait_ref: &Trait,
    state: &mut State,
    item: &crate::r#trait::procedure::SequenceItem,
    parent_run_index: usize,
    parent_sequence_index: usize,
) -> crate::Result<()> {
    // A concurrent `for-each` (P402) enters the same control frame as any
    // other `for-each`: the core runtime remains authority for item binding,
    // ordering, and disposition and executes items sequentially, on the
    // single cursor, exactly like a non-concurrent `for-each`. `concurrent`
    // is only ever a CLI/IO-layer dispatch hint (see `ctx-cli`'s drive loop)
    // for *how many* items may be speculatively dispatched ahead of the
    // cursor — never a core scheduling behavior. Prior to P402 this was
    // rejected here as a hard "unsupported" stop; that rejection is gone.
    if state.control_stack.len() >= MAX_SEQUENCE_NESTING_DEPTH {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_MAX_SEQUENCE_DEPTH_EXCEEDED,
            state.active_path.clone(),
            None,
        );
        return Ok(());
    }
    if item.effective_kind() == SequenceKind::Parallel {
        return enter_parallel_frame(state, item, parent_run_index, parent_sequence_index);
    }
    let mut sequence_ref = item.sequence.as_deref().ok_or_else(|| {
        crate::procedure::invalid_field(
            format!("procedure.sequence[{parent_sequence_index}].sequence"),
            "sequence control item is missing sequence ref",
        )
    })?;
    if item.effective_kind() == SequenceKind::Branch {
        let when = item.when.as_ref().ok_or_else(|| {
            crate::procedure::invalid_field(
                format!("procedure.sequence[{parent_sequence_index}].when"),
                "branch control item is missing when guard",
            )
        })?;
        let branch_id = item.id.as_deref().ok_or_else(|| {
            crate::procedure::invalid_field(
                format!("procedure.sequence[{parent_sequence_index}].id"),
                "branch control item is missing id",
            )
        })?;
        let position_path = path_for_branch_item(state, parent_run_index, item);
        // The innermost control owns the guard scope. A for-each nested in a
        // loop must not observe evidence from another item in that iteration.
        let context = guard_context_from_stack(&state.control_stack)
            .unwrap_or(LoopContext {
            // An empty loop id is the explicit non-loop guard scope. Guard
            // evaluation treats signal refs in this scope as already emitted.
            loop_id: String::new(),
            sequence_id: None,
            iteration_index: 0,
            max_iterations: 1,
            });
        let guard_evaluation_start_index = state.guard_evaluations.len();
        let slot_revision_watermark = latest_recorded_slot_revision_order(state);
        let (matched, evaluations) =
            evaluate_guard_expr(trait_ref, state, when, &context, &position_path, &[])?;
        append_guard_evaluations(state, evaluations).ok_or_else(|| {
            crate::procedure::invalid_field("runtime.branch.when", "branch guard produced no evaluation evidence")
        })?;
        // Preserve a branch-specific marker after the predicate-level evidence
        // so the immutable decision cannot cite an unrelated guard result.
        let predicate = serde_json::to_string(&(when, &position_path)).map_err(|error| {
            crate::procedure::serialization("runtime.branch.when", "branch guard", error)
        })?;
        let guard_evaluation_index = append_guard_evaluations(
            state,
            vec![condition_evaluation(
                &format!("branch:{branch_id}:{predicate}"),
                None,
                &context,
                matched,
                "branch selection",
            )],
        )
        .expect("branch selection marker is not empty");
        let selected_arm = if matched {
            "then"
        } else if item.otherwise.is_some() {
            "otherwise"
        } else {
            "none"
        };
        let selected_sequence = if matched {
            item.sequence.as_deref()
        } else {
            item.otherwise.as_deref()
        }
        .and_then(|sequence| sequence_id_from_ref(sequence).ok());
        state.branch_decisions.push(BranchDecision {
            parent_run_index,
            branch_id: branch_id.to_string(),
            position_path: position_path.clone(),
            matched,
            when: Some(when.clone()),
            guard_evaluation_start_index: Some(guard_evaluation_start_index),
            slot_revision_watermark: Some(slot_revision_watermark),
            guard_evaluation_index,
            selected_arm: selected_arm.to_string(),
            sequence_id: selected_sequence,
        });
        if !state.control_stack.is_empty() {
            set_path_sequence_status(
                state,
                SequenceStatus {
                    sequence_index: parent_sequence_index,
                    run_index: parent_run_index,
                    item_id: item.id.clone(),
                    title: item.title.clone(),
                    status: SequenceStatusKind::Accepted,
                    reason: "branch decision recorded".to_string(),
                    position_path: position_path.clone(),
                },
            );
        }
        if !matched {
            let Some(otherwise) = item.otherwise.as_deref() else {
                if state.control_stack.is_empty() {
                    set_current_outer_status(state, SequenceStatusKind::Accepted, "branch completed with no selected arm");
                    state.current_run_index = state.current_run_index.saturating_add(1);
                    state.active_path.clear();
                } else {
                    state.active_path = position_path;
                    advance_after_current_leaf(state);
                    evaluate_control_guards_after_step(trait_ref, state, &[])?;
                }
                return Ok(());
            };
            sequence_ref = otherwise;
        }
    }
    let sequence_id = sequence_id_from_ref(sequence_ref)?;
    let Some(named) = trait_ref.sequences.get(&sequence_id) else {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_UNRESOLVED_RUNTIME_SEQUENCE,
            state.active_path.clone(),
            None,
        );
        return Ok(());
    };
    let kind = item.effective_kind();
    let control_kind = match kind {
        SequenceKind::Sequence => ControlKind::Sequence,
        SequenceKind::Branch => ControlKind::Branch,
        SequenceKind::Loop => ControlKind::Loop,
        SequenceKind::ForEach => ControlKind::ForEach,
        // Parallel is handled by `enter_parallel_frame` and returns above
        // before reaching this point.
        SequenceKind::Parallel
        | SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => return Ok(()),
    };

    if control_kind == ControlKind::ForEach {
        let Some(over) = item.over.as_deref() else {
            stop_with_reason(
                state,
                FinalState::Failed,
                STOP_FOR_EACH_MISSING_OVER,
                path_for_control_item(parent_run_index, item),
                None,
            );
            return Ok(());
        };
        let Some(list_value) = accepted_value(state, over) else {
            state.final_state = FinalState::Blocked;
            state.active_path.clear();
            set_current_outer_status(
                state,
                SequenceStatusKind::Blocked,
                format!("for-each over slot {over} is not accepted"),
            );
            state
                .provider_capability_reports
                .push(CapabilityReport::unsupported(
                    "runtime.sequence-input",
                    format!("missing current sequence input(s): {over}"),
                ));
            return Ok(());
        };
        let Some(items) = list_value.value.as_array() else {
            stop_with_reason(
                state,
                FinalState::Failed,
                STOP_FOR_EACH_OVER_NOT_LIST,
                state.active_path.clone(),
                None,
            );
            return Ok(());
        };
        let max_items = item.max_items.unwrap_or(items.len());
        if items.len() > max_items {
            let path = path_for_control_item_activation(state, parent_run_index, item);
            let identity = ControlEmissionIdentity {
                kind: ControlKind::ForEach,
                parent_run_index,
                control_item_id: item.id.clone(),
                sequence_id: sequence_id.clone(),
                iteration_index: None,
                item_index: None,
            };
            emit_runtime_control_signal_at(
                state,
                item.on_failure.as_ref().and_then(|target| match target {
                    FailureTarget::Signal(signal) => Some(signal.as_str()),
                    FailureTarget::Route(_) => None,
                }),
                parent_sequence_index,
                path.clone(),
                Some(identity),
            )?;
            if route_failure(
                trait_ref,
                state,
                parent_run_index,
                item.id.as_deref(),
                item.on_failure.as_ref(),
                path.clone(),
            )? {
                return Ok(());
            }
            if route_enclosing_failure(
                trait_ref,
                state,
                state.control_stack.len(),
                &path,
            )? {
                return Ok(());
            }
            stop_with_reason(
                state,
                FinalState::Blocked,
                STOP_MAX_ITEMS_EXCEEDED,
                path.clone(),
                None,
            );
            return Ok(());
        }
        if items.is_empty() || named.sequence.is_empty() {
            let identity = ControlEmissionIdentity {
                kind: ControlKind::ForEach,
                parent_run_index,
                control_item_id: item.id.clone(),
                sequence_id: sequence_id.clone(),
                iteration_index: None,
                item_index: None,
            };
            emit_runtime_control_signal_at(
                state,
                item.on_complete.first().map(|rule| rule.signal_ref()),
                parent_sequence_index,
                path_for_control_item_activation(state, parent_run_index, item),
                Some(identity),
            )?;
            if state.control_stack.is_empty() {
                set_current_outer_status(
                    state,
                    SequenceStatusKind::Accepted,
                    "for-each completed with no items",
                );
            }
            advance_after_current_leaf(state);
            return Ok(());
        }
    }

    let mut frame = ControlFrame {
        kind: control_kind,
        parent_run_index,
        control_item_id: item.id.clone(),
        sequence_id,
        next_index: 0,
        iteration_index: None,
        max_iterations: None,
        unbounded: false,
        max_items: None,
        item_index: None,
        item_total: None,
        over_slot: item.over.clone(),
        item_slot: item.item.clone(),
        list_digest: None,
        concurrent: kind == SequenceKind::ForEach && item.concurrent,
        until: item.until.clone(),
        abort_if: item.abort_if.clone(),
        on_exhausted: item.on_exhausted.clone(),
        on_abort: item.on_abort.clone(),
        on_complete: item.on_complete.first().map(|rule| rule.signal_ref().to_string()),
        on_failure: item.on_failure.clone(),
        parallel_branch_sequence_ids: Vec::new(),
        parallel_buffer: EffectBuffer::default(),
        parallel_committed_branches: Vec::new(),
        branch_decisions_watermark: 0,
        guard_evaluations_watermark: 0,
        join: None,
        branch_failure: Vec::new(),
        parallel_branch_refs: Vec::new(),
        parallel_branch_outcomes: Vec::new(),
    };
    if frame.kind == ControlKind::Loop {
        if let Some(max_iterations) = item.max_iterations {
            frame.iteration_index = Some(0);
            frame.max_iterations = Some(max_iterations);
        } else if let Some(max_iterations_from) = item.max_iterations_from.as_deref() {
            if accepted_value(state, max_iterations_from).is_none() {
                state.final_state = FinalState::Blocked;
                state.active_path.clear();
                set_current_outer_status(
                    state,
                    SequenceStatusKind::Blocked,
                    format!("dynamic loop bound {max_iterations_from} is not accepted"),
                );
                state
                    .provider_capability_reports
                    .push(CapabilityReport::unsupported(
                        "runtime.sequence-input",
                        format!("missing current sequence input(s): {max_iterations_from}"),
                    ));
                return Ok(());
            }
            let max_iterations =
                resolve_positive_usize_input(state, max_iterations_from, "max-iterations-from")?;
            frame.iteration_index = Some(0);
            frame.max_iterations = Some(max_iterations);
        } else if item.until.is_some() || item.abort_if.is_some() {
            // No bound declared (0093): validation already requires `until`
            // or `abort-if` in this case, so the loop is deliberately
            // unbounded — it exhausts never, only its own guard exits it.
            frame.iteration_index = Some(0);
            frame.max_iterations = None;
            frame.unbounded = true;
        } else {
            // Defensive: static validation already refuses a loop with
            // neither a bound nor an exit guard, so this is unreachable
            // through authored/validated procedures.
            let step = item.id.as_deref().unwrap_or("unnamed");
            return Err(crate::procedure::invalid_field(
                format!("procedure.sequence[{parent_sequence_index}].max-iterations"),
                format!("loop step {step:?} is unbounded and will not run"),
            ));
        }
    }
    if frame.kind == ControlKind::ForEach {
        let over = item.over.as_deref().unwrap_or_default();
        let Some(list_value) = accepted_value(state, over) else {
            return Ok(());
        };
        let Some(items) = list_value.value.as_array() else {
            return Ok(());
        };
        frame.item_index = Some(0);
        frame.item_total = Some(items.len());
        frame.max_items = Some(item.max_items.unwrap_or(items.len()));
        frame.list_digest = Some(list_value.value_digest.clone());
    }
    state.control_stack.push(frame);
    if state
        .control_stack
        .last()
        .is_some_and(|frame| frame.kind == ControlKind::ForEach)
    {
        bind_current_for_each_item(trait_ref, state)?;
    }
    Ok(())
}

fn resolve_positive_usize_input(
    state: &State,
    ref_text: &str,
    field: &str,
) -> crate::Result<usize> {
    let value = accepted_value(state, ref_text).ok_or_else(|| {
        crate::procedure::invalid_field(
            format!("runtime.{field}"),
            format!("accepted input value {ref_text:?} is missing"),
        )
    })?;
    let unsigned = value.value.as_u64().ok_or_else(|| {
        crate::procedure::invalid_field(
            format!("runtime.{field}"),
            format!("accepted input value {ref_text:?} must be a positive integer"),
        )
    })?;
    let resolved = usize::try_from(unsigned).map_err(|_| {
        crate::procedure::invalid_field(
            format!("runtime.{field}"),
            format!("accepted input value {ref_text:?} does not fit usize"),
        )
    })?;
    if resolved == 0 {
        return Err(crate::procedure::invalid_field(
            format!("runtime.{field}"),
            format!("accepted input value {ref_text:?} must be greater than zero"),
        ));
    }
    Ok(resolved)
}

/// Execute one closed projection leaf. Every source and prior destination is
/// cloned before any write is recorded, and every candidate is schema-checked
/// before the first commit, so callers observe all writes or none.
fn execute_project_item(
    trait_ref: &Trait,
    state: &mut State,
    item: &crate::r#trait::procedure::SequenceItem,
    parent_run_index: usize,
    sequence_index: usize,
) -> crate::Result<()> {
    let position_path = if state.control_stack.is_empty() {
        vec![PathSegment {
            kind: "procedure".to_string(),
            id: item.id.clone(),
            index: parent_run_index,
            iteration: None,
            item_index: None,
        }]
    } else {
        path_for_nested_item(state, sequence_index, item)
    };
    let loop_context = loop_context_from_stack(&state.control_stack);
    let for_each_context = for_each_context_from_stack(&state.control_stack);
    let mut candidates = Vec::with_capacity(item.projection.len());
    let first_acceptance_order = next_acceptance_order(state);
    for (index, projection) in item.projection.iter().enumerate() {
        let (submitted, provenance, source_attribution) = match &projection.source {
            ProjectionSource::Slot(source_ref) => {
                let source = accepted_value(state, source_ref).cloned().ok_or_else(|| {
                    crate::procedure::invalid_field(
                        format!("runtime.project.projection[{index}].source"),
                        format!("project source {source_ref:?} has no accepted value"),
                    )
                })?;
                let submitted = match projection.field.as_deref() {
                    Some(field) => crate::shared::resolve_field_path(&source.value, field)
                        .cloned()
                        .ok_or_else(|| {
                            crate::procedure::invalid_field(
                                format!("runtime.project.projection[{index}].field"),
                                format!("project source field {field:?} is absent"),
                            )
                        })?,
                    None => source.value.clone(),
                };
                let provenance = ProjectionProvenance {
                    source_ref: Some(Reference::parse(source_ref)?),
                    source_value_digest: Some(source.value_digest.clone()),
                    field: projection.field.clone(),
                    literal_digest: None,
                };
                (
                    submitted,
                    provenance,
                    Some((
                        if source.source == ValueSource::CommandOutput {
                            ValueSource::Ledger
                        } else {
                            source.source
                        },
                        source.producer_evidence,
                        None,
                        source.producer_agent,
                        source.producer_harness,
                    )),
                )
            }
            ProjectionSource::Literal { literal } => {
                let provenance = ProjectionProvenance {
                    source_ref: None,
                    source_value_digest: None,
                    field: None,
                    literal_digest: Some(value_digest(literal)?),
                };
                (literal.clone(), provenance, None)
            }
        };
        let sink = if projection.operation == WriteOperation::Replace {
            OutputSink::Ref(projection.destination.clone())
        } else {
            OutputSink::SlotOperation {
                slot: projection.destination.clone(),
                operation: projection.operation.clone(),
                optional: false,
            }
        };
        let prior = accepted_value(state, &projection.destination).cloned();
        let mut runtime_value = runtime_value_for_output_sink(
            trait_ref,
            sequence_index,
            &sink,
            StepSlotOutput {
                ref_text: projection.destination.clone(),
                value: submitted.clone(),
                source: Some(source_attribution.as_ref().map_or(ValueSource::Ledger, |(source, _, _, _, _)| source.clone())),
                producer_evidence: source_attribution.as_ref().and_then(|(_, evidence, _, _, _)| evidence.clone()).or_else(|| Some(format!(
                    "project:{} projection:{index}", item.id.as_deref().unwrap_or("unnamed")
                ))),
                command_execution: source_attribution.as_ref().and_then(|(_, _, execution, _, _)| execution.clone()),
                producer_agent: source_attribution.as_ref().and_then(|(_, _, _, agent, _)| agent.clone()),
                producer_harness: source_attribution.as_ref().and_then(|(_, _, _, _, harness)| harness.clone()),
            },
            false,
        )?;
        if runtime_value.acceptance == AcceptanceStatus::Accepted {
            runtime_value = apply_runtime_write(
                trait_ref,
                &projection.operation,
                prior.as_ref(),
                runtime_value,
            )?;
        }
        if runtime_value.acceptance != AcceptanceStatus::Accepted
            || runtime_value
                .schema_validation
                .iter()
                .any(|validation| validation.status != SchemaStatus::Accepted)
        {
            return Err(crate::procedure::invalid_field(
                format!("runtime.project.projection[{index}].destination"),
                format!(
                    "project destination {:?} rejected the projected value",
                    projection.destination
                ),
            ));
        }
        let revision = slot_revision_from_value(
            &runtime_value,
            SlotRevisionWrite {
                operation: projection.operation.clone(),
                submitted_payload: submitted,
                prior_value: prior.as_ref(),
                runtime_binding: false,
                projection: Some(provenance),
            },
            SlotRevisionContext {
                acceptance_order: first_acceptance_order.saturating_add(index),
                position_path: &position_path,
                loop_context: loop_context.as_ref(),
                for_each_context: for_each_context.as_ref(),
            },
        )?;
        candidates.push((runtime_value, revision));
    }

    let projected_values: Vec<Value> = candidates
        .iter()
        .map(|(value, _)| value.clone())
        .collect();
    for (value, revision) in candidates {
        record_accepted_slot_value(state, value, revision);
    }
    if state.control_stack.is_empty() {
        set_sequence_status(
            state,
            sequence_index,
            SequenceStatusKind::Accepted,
            "atomic project writes accepted",
        );
    } else {
        set_path_sequence_status(
            state,
            SequenceStatus {
                sequence_index,
                run_index: parent_run_index,
                item_id: item.id.clone(),
                title: item.title.clone(),
                status: SequenceStatusKind::Accepted,
                reason: "atomic project writes accepted".to_string(),
                position_path: position_path.clone(),
            },
        );
    }
    state.active_path = position_path;
    advance_after_current_leaf(state);
    evaluate_control_guards_after_step(trait_ref, state, &projected_values)
}

/// Enter a `parallel` control item: push one coordinator frame whose
/// `sequence_id`/`next_index` track the FIRST authored branch's body,
/// exactly like a `Sequence` frame tracks its own body. `iteration_index`
/// indexes the authored `branches` list (via `parallel_branch_sequence_ids`,
/// resolved once here) to select which branch is current; branches are
/// walked strictly in authored order on the single cursor — never
/// concurrently.
fn enter_parallel_frame(
    state: &mut State,
    item: &crate::r#trait::procedure::SequenceItem,
    parent_run_index: usize,
    parent_sequence_index: usize,
) -> crate::Result<()> {
    let mut branch_sequence_ids = Vec::with_capacity(item.branches.as_slice().len());
    for branch_ref in item.branches.iter() {
        branch_sequence_ids.push(sequence_id_from_ref(branch_ref)?);
    }
    let Some(first_branch_sequence_id) = branch_sequence_ids.first().cloned() else {
        return Err(crate::procedure::invalid_field(
            format!("procedure.sequence[{parent_sequence_index}].branches"),
            "parallel sequence item must declare at least one branch",
        ));
    };
    let branch_total = branch_sequence_ids.len();
    let branch_refs: Vec<String> = item.branches.iter().cloned().collect();
    state.control_stack.push(ControlFrame {
        kind: ControlKind::Parallel,
        parent_run_index,
        control_item_id: item.id.clone(),
        sequence_id: first_branch_sequence_id,
        next_index: 0,
        iteration_index: Some(0),
        max_iterations: Some(branch_total),
        unbounded: false,
        max_items: None,
        item_index: None,
        item_total: None,
        over_slot: None,
        item_slot: None,
        list_digest: None,
        concurrent: false,
        until: None,
        abort_if: None,
        // Parallel frames reuse the loop counter for branch progress; branch
        // exhaustion is normal completion, never a policy decision.
        on_exhausted: None,
        on_abort: None,
        on_complete: None,
        // A `parallel` item's own `on-failure` is the panel-level recovery
        // route for a `panel-fail` branch policy or a failed
        // `quorum-verdict` (P264) — reused as-is by `route_failure`.
        on_failure: item.on_failure.clone(),
        parallel_branch_sequence_ids: branch_sequence_ids,
        parallel_buffer: EffectBuffer::default(),
        parallel_committed_branches: Vec::new(),
        branch_decisions_watermark: state.branch_decisions.len(),
        guard_evaluations_watermark: state.guard_evaluations.len(),
        join: item.join.clone(),
        branch_failure: item.branch_failure.clone(),
        parallel_branch_refs: branch_refs,
        parallel_branch_outcomes: vec![None; branch_total],
    });
    Ok(())
}

/// Finish the current branch of an active `parallel` frame at its body
/// barrier: drain the branch's isolation buffer into `parallel_committed_branches`,
/// then either advance to the next authored branch (fresh buffer) or, once
/// every branch is drained, merge all of them — in authored order, each
/// preserving its own internal append order — into the enclosing target
/// (the committed ledger, or the next-enclosing active `parallel` frame's own
/// buffer when this panel is nested inside another branch) and complete the
/// panel like any other control frame.
fn advance_or_complete_parallel(trait_ref: &Trait, state: &mut State) -> crate::Result<()> {
    complete_current_parallel_branch(trait_ref, state, ParallelBranchOutcome::Committed)
}

/// Finish the current branch of an active `parallel` frame at its body
/// barrier with the given resolved `outcome` — `Committed` for an ordinary
/// successful branch completion, or `Skipped`/`Parked` when a `branch-failure`
/// policy resolved this branch's terminal failure (P264; see
/// [`resolve_active_branch_failure`]). A `Committed` branch's isolation
/// buffer is drained into `parallel_committed_branches` for the eventual
/// barrier merge/join; a `Skipped` branch's buffer is discarded outright, so
/// it contributes no accepted effects and is invisible to a `reduce-merge`/
/// `quorum-verdict` join's `source` collection. Either way this either
/// advances to the next authored branch (fresh buffer) or, once every branch
/// is drained/skipped, resolves the panel's declared barrier join policy
/// ([`resolve_parallel_barrier`]).
fn complete_current_parallel_branch(
    trait_ref: &Trait,
    state: &mut State,
    outcome: ParallelBranchOutcome,
) -> crate::Result<()> {
    let Some(top) = state.control_stack.last_mut() else {
        return Ok(());
    };
    let branch_index = top.iteration_index.unwrap_or(0);
    let branch_total = top.max_iterations.unwrap_or(1);
    let finished_buffer = std::mem::take(&mut top.parallel_buffer);
    if outcome == ParallelBranchOutcome::Committed {
        top.parallel_committed_branches.push(finished_buffer);
    }
    if let Some(slot) = top.parallel_branch_outcomes.get_mut(branch_index) {
        *slot = Some(outcome);
    }
    let next_branch = branch_index.saturating_add(1);
    if next_branch < branch_total {
        let Some(next_sequence_id) = top.parallel_branch_sequence_ids.get(next_branch).cloned() else {
            stop_with_reason(
                state,
                FinalState::Failed,
                STOP_UNRESOLVED_RUNTIME_SEQUENCE,
                state.active_path.clone(),
                None,
            );
            return Ok(());
        };
        // Watermark for the branch about to start, so a rejection during its
        // activation rolls back only its own decisions/guard evaluations —
        // never the just-completed prior branch's.
        let branch_decisions_watermark = state.branch_decisions.len();
        let guard_evaluations_watermark = state.guard_evaluations.len();
        let top = state.control_stack.last_mut().expect("parallel frame present");
        top.iteration_index = Some(next_branch);
        top.next_index = 0;
        top.sequence_id = next_sequence_id;
        top.branch_decisions_watermark = branch_decisions_watermark;
        top.guard_evaluations_watermark = guard_evaluations_watermark;
        return Ok(());
    }
    let panel_position_path =
        path_for_control_frame_activation(state, state.control_stack.len() - 1);
    let completed = state.control_stack.pop().expect("parallel frame present");
    resolve_parallel_barrier(trait_ref, state, completed, panel_position_path)
}

/// Resolve a completed `parallel` panel's declared barrier join policy
/// (P264): merge every committed branch buffer into the enclosing target in
/// authored order (unconditionally — a `reduce-merge`/`quorum-verdict` join
/// is an ADDITIONAL panel-owned aggregate write, not a replacement for the
/// P263 barrier merge), then, for a declared join, fold (`reduce-merge`) or
/// collect-and-guard (`quorum-verdict`) each committed branch's own `source`
/// value into the join's panel-owned `destination` slot. Records exactly one
/// append-only [`ParallelPanelRecord`] once the panel's final disposition is
/// known.
fn resolve_parallel_barrier(
    trait_ref: &Trait,
    state: &mut State,
    completed: ControlFrame,
    panel_position_path: Vec<PathSegment>,
) -> crate::Result<()> {
    let branch_records: Vec<ParallelPanelBranchRecord> = completed
        .parallel_branch_refs
        .iter()
        .enumerate()
        .map(|(index, branch_ref)| ParallelPanelBranchRecord {
            branch_ref: branch_ref.clone(),
            outcome: completed.parallel_branch_outcomes.get(index).copied().flatten(),
        })
        .collect();
    let join_label = completed
        .join
        .as_ref()
        .map_or("collect-in-order", JoinPolicy::label)
        .to_string();

    match completed.join.clone() {
        None | Some(JoinPolicy::CollectInOrder) => {
            merge_parallel_branches_into_target(state, completed.parallel_committed_branches.clone());
            finish_control_completion(state, &completed.kind);
            state.parallel_panel_records.push(ParallelPanelRecord {
                control_item_id: completed.control_item_id.clone(),
                position_path: panel_position_path,
                join_policy: join_label,
                branches: branch_records,
                result_digest: None,
                guard_evaluation_index: None,
                disposition: ParallelPanelDisposition::Completed,
            });
        }
        Some(JoinPolicy::ReduceMerge { destination, source, operation }) => {
            let prior = accepted_value(state, &destination).cloned();
            let mut accumulator = prior.as_ref().map(|value| value.value.clone());
            for buffer in &completed.parallel_committed_branches {
                if let Some(value) =
                    buffer.accepted_slot_values.iter().find(|value| value.ref_text == source)
                {
                    accumulator = Some(apply_write_operation_value(
                        &operation,
                        accumulator.as_ref(),
                        &value.value,
                    )?);
                }
            }
            merge_parallel_branches_into_target(state, completed.parallel_committed_branches.clone());
            let result_digest = if let Some(final_value) = accumulator {
                let digest = value_digest(&final_value)?;
                let runtime_value = Value {
                    ref_text: destination.clone(),
                    value: final_value.clone(),
                    value_digest: digest.clone(),
                    schema_ref: runtime_schema_reference(
                        &output_schema_ref(trait_ref, &destination).unwrap_or_default(),
                    )?,
                    source: ValueSource::Ledger,
                    producer_evidence: Some("parallel reduce-merge join".to_string()),
                    command_execution: None,
                    producer_agent: None,
                    producer_harness: None,
                    producer_check_verdict: false,
                    acceptance: AcceptanceStatus::Accepted,
                    schema_validation: Vec::new(),
                };
                let revision = slot_revision_from_value(
                    &runtime_value,
                    SlotRevisionWrite {
                        operation,
                        submitted_payload: final_value,
                        prior_value: prior.as_ref(),
                        runtime_binding: true,
                        projection: None,
                    },
                    SlotRevisionContext {
                        acceptance_order: next_acceptance_order(state),
                        position_path: &panel_position_path,
                        loop_context: None,
                        for_each_context: None,
                    },
                )?;
                record_accepted_slot_value(state, runtime_value, revision);
                Some(digest)
            } else {
                None
            };
            finish_control_completion(state, &completed.kind);
            state.parallel_panel_records.push(ParallelPanelRecord {
                control_item_id: completed.control_item_id.clone(),
                position_path: panel_position_path,
                join_policy: join_label,
                branches: branch_records,
                result_digest,
                guard_evaluation_index: None,
                disposition: ParallelPanelDisposition::Completed,
            });
        }
        Some(JoinPolicy::QuorumVerdict { destination, source, guard }) => {
            let mut collected = Vec::new();
            for buffer in &completed.parallel_committed_branches {
                if let Some(value) =
                    buffer.accepted_slot_values.iter().find(|value| value.ref_text == source)
                {
                    collected.push(value.value.clone());
                }
            }
            merge_parallel_branches_into_target(state, completed.parallel_committed_branches.clone());
            let staged_value = JsonValue::Array(collected);
            let digest = value_digest(&staged_value)?;
            let prior = accepted_value(state, &destination).cloned();
            let staged_runtime_value = Value {
                ref_text: destination.clone(),
                value: staged_value.clone(),
                value_digest: digest.clone(),
                schema_ref: runtime_schema_reference(
                    &output_schema_ref(trait_ref, &destination).unwrap_or_default(),
                )?,
                source: ValueSource::Ledger,
                producer_evidence: Some("parallel quorum-verdict join".to_string()),
                command_execution: None,
                producer_agent: None,
                producer_harness: None,
                producer_check_verdict: false,
                acceptance: AcceptanceStatus::Accepted,
                schema_validation: Vec::new(),
            };
            // Stage the candidate destination value at the now-current active
            // target (this panel's own frame has already been popped and its
            // branches merged above, so this is whichever frame/ledger now
            // encloses) so the guard's `count`/`empty` predicates over
            // `destination` see it — without yet appending a durable
            // `SlotRevision`. Reverted below if the guard does not match, so
            // only a matched guard ever actually commits.
            match active_parallel_frame_index(state) {
                Some(index) => upsert_runtime_value(
                    &mut state.control_stack[index].parallel_buffer.accepted_slot_values,
                    staged_runtime_value.clone(),
                ),
                None => upsert_runtime_value(&mut state.accepted_slot_values, staged_runtime_value.clone()),
            }
            let loop_context = LoopContext {
                loop_id: String::new(),
                sequence_id: None,
                iteration_index: 0,
                max_iterations: 0,
            };
            let (matched, evaluations) = evaluate_guard_expr(
                trait_ref,
                state,
                &guard,
                &loop_context,
                &panel_position_path,
                &[],
            )?;
            let guard_evaluation_index = append_guard_evaluations(state, evaluations);
            if matched {
                let revision = slot_revision_from_value(
                    &staged_runtime_value,
                    SlotRevisionWrite {
                        operation: WriteOperation::Replace,
                        submitted_payload: staged_value,
                        prior_value: prior.as_ref(),
                        runtime_binding: true,
                        projection: None,
                    },
                    SlotRevisionContext {
                        acceptance_order: next_acceptance_order(state),
                        position_path: &panel_position_path,
                        loop_context: None,
                        for_each_context: None,
                    },
                )?;
                record_accepted_slot_value(state, staged_runtime_value, revision);
                finish_control_completion(state, &completed.kind);
                state.parallel_panel_records.push(ParallelPanelRecord {
                    control_item_id: completed.control_item_id.clone(),
                    position_path: panel_position_path,
                    join_policy: join_label,
                    branches: branch_records,
                    result_digest: Some(digest),
                    guard_evaluation_index,
                    disposition: ParallelPanelDisposition::Completed,
                });
            } else {
                // Never commits: revert the staged value so a failed quorum
                // leaves the destination slot exactly as it was before this
                // activation.
                match active_parallel_frame_index(state) {
                    Some(index) => {
                        let values = &mut state.control_stack[index].parallel_buffer.accepted_slot_values;
                        match prior.clone() {
                            Some(prior) => upsert_runtime_value(values, prior),
                            None => values.retain(|value| value.ref_text != destination),
                        }
                    }
                    None => match prior.clone() {
                        Some(prior) => upsert_runtime_value(&mut state.accepted_slot_values, prior),
                        None => state.accepted_slot_values.retain(|value| value.ref_text != destination),
                    },
                }
                let path = panel_position_path.clone();
                let routed = route_failure(
                    trait_ref,
                    state,
                    completed.parent_run_index,
                    completed.control_item_id.as_deref(),
                    completed.on_failure.as_ref(),
                    path.clone(),
                )? || route_enclosing_failure(trait_ref, state, state.control_stack.len(), &path)?;
                if routed {
                    state.parallel_panel_records.push(ParallelPanelRecord {
                        control_item_id: completed.control_item_id.clone(),
                        position_path: path,
                        join_policy: join_label,
                        branches: branch_records,
                        result_digest: None,
                        guard_evaluation_index,
                        disposition: ParallelPanelDisposition::Routed,
                    });
                } else {
                    stop_with_reason(
                        state,
                        FinalState::Blocked,
                        STOP_PARALLEL_QUORUM_VERDICT_FAILED,
                        path.clone(),
                        guard_evaluation_index,
                    );
                    emit_runtime_control_signal(
                        state,
                        completed.on_failure.as_ref().and_then(FailureTarget::signal_ref),
                        completed.parent_run_index,
                        Some(runtime_control_identity_from_frame(&completed)),
                        path.clone(),
                    )?;
                    emit_enclosing_failure_signals(state, state.control_stack.len(), &path)?;
                    state.parallel_panel_records.push(ParallelPanelRecord {
                        control_item_id: completed.control_item_id.clone(),
                        position_path: path,
                        join_policy: join_label,
                        branches: branch_records,
                        result_digest: None,
                        guard_evaluation_index,
                        disposition: ParallelPanelDisposition::Stopped,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Merge completed branch buffers (already in authored order) into whichever
/// target is currently enclosing this panel — see [`advance_or_complete_parallel`].
fn merge_parallel_branches_into_target(state: &mut State, branches: Vec<EffectBuffer>) {
    match state.control_stack.iter().rposition(|frame| frame.kind == ControlKind::Parallel) {
        Some(index) => {
            for buffer in branches {
                merge_effect_buffer_into_buffer(&mut state.control_stack[index].parallel_buffer, buffer);
            }
        }
        None => {
            for buffer in branches {
                merge_effect_buffer_into_ledger(state, buffer);
            }
        }
    }
}

/// Append one drained branch buffer's effects onto an enclosing `parallel`
/// frame's own in-progress buffer. Revision acceptance order is global to the
/// run and remains stable across isolation-buffer merges.
fn merge_effect_buffer_into_buffer(target: &mut EffectBuffer, buffer: EffectBuffer) {
    for value in buffer.accepted_slot_values {
        upsert_runtime_value(&mut target.accepted_slot_values, value);
    }
    for value in buffer.accepted_output_port_values {
        upsert_runtime_value(&mut target.accepted_output_port_values, value);
    }
    for revision in buffer.slot_revisions {
        target.slot_revisions.push(revision);
    }
    target.emitted_signals.extend(buffer.emitted_signals);
}

/// Append one drained branch buffer's effects onto the committed ledger while
/// preserving the run-global revision acceptance order.
fn merge_effect_buffer_into_ledger(state: &mut State, buffer: EffectBuffer) {
    for value in buffer.accepted_slot_values {
        upsert_runtime_value(&mut state.accepted_slot_values, value);
    }
    for value in buffer.accepted_output_port_values {
        upsert_runtime_value(&mut state.accepted_output_port_values, value);
    }
    for revision in buffer.slot_revisions {
        state.slot_revisions.push(revision);
    }
    state.emitted_signals.extend(buffer.emitted_signals);
}

/// The control frame currently accepting writes — the innermost active
/// `parallel` frame's own branch buffer, if any is on the stack. `None` means
/// writes go straight to the committed ledger (no active panel).
fn active_parallel_frame_index(state: &State) -> Option<usize> {
    state
        .control_stack
        .iter()
        .rposition(|frame| frame.kind == ControlKind::Parallel)
}

/// Roll a rejected submission's active `parallel` branch back to a clean
/// branch-entry state: drop every nested control pushed since entering the
/// branch, discard the branch's isolation buffer (accepted values,
/// output-port values, revisions, and ordinary signals staged by any earlier
/// accepted call in this same branch activation — not just this rejected
/// call), and reset the branch's cursor to its first step so a corrected
/// replay resubmits the whole branch from a coherent position. A `parallel`
/// branch is one all-or-nothing unit until its barrier: buffering a step's
/// effects across separate accepted calls, then persisting them in
/// `session.ledger` before the branch as a whole is known-good, only ever
/// gave call-level atomicity — this restores branch-level atomicity to match.
/// No-op when the rejection did not occur inside an active branch. Only the
/// branch's own nested per-position status display entries are pruned; the
/// panel's own top-level status is untouched (still `Pending`/`Running`, not
/// a terminal outcome).
/// P402: apply a terminal dispatch-level failure (harness retries exhausted,
/// timeout, or a concurrent-wave worker panic/IO error) for the CURRENT
/// ready item — the counterpart, for a call that never produced a usable
/// step-output submission at all, to what a rejected submission's content
/// already triggers via [`reject_step_output`]. Reused (not reimplemented)
/// here: this function builds exactly the synthetic
/// [`StepValidationReport`] a rejected submission would have carried and
/// hands it to the SAME `reject_step_output`, so the SAME nested-recovery /
/// P264 `skip`/`park`/`panel-fail` branch-failure policy in
/// [`route_enclosing_failure`]/[`resolve_active_branch_failure`] applies —
/// CLI never re-implements that policy for a concurrent branch/item's
/// terminal failure.
///
/// Returns `Ok(None)` when there is no current ready item to fail (the run
/// already completed, is blocked for an unrelated reason, or is already
/// rejected/failed) — the caller must not silently claim to have routed
/// anything in that case.
pub fn apply_terminal_frame_failure(
    trait_ref: &Trait,
    state: State,
    reason: &str,
) -> crate::Result<Option<(State, StepValidationReport)>> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    let Some(ready) = current_ready_item(trait_ref, &state, &sequence)? else {
        return Ok(None);
    };
    let sequence_index = ready.sequence_index;
    let position_path = ready.position_path.clone();
    let value_digest = crate::digest::Digest::source(reason);
    let report = StepValidationReport {
        sequence_index,
        accepted_outputs: Vec::new(),
        rejected_outputs: vec![RejectedAttempt {
            sequence_index,
            position_path: position_path.clone(),
            ref_text: None,
            value_digest: Some(value_digest),
            reason: reason.to_string(),
        }],
        missing_required_outputs: Vec::new(),
        unfilled_optional_outputs: Vec::new(),
        unexpected_outputs: Vec::new(),
        schema_validation: Vec::new(),
        signal_validation: Vec::new(),
        warnings: Vec::new(),
        next_action: StepNextAction::Rejected,
    };
    let (state, report) = reject_step_output(trait_ref, state, sequence_index, report)?;
    Ok(Some((state, report)))
}

pub fn rollback_active_parallel_branch(trait_ref: &Trait, state: &mut State) -> crate::Result<()> {
    let Some(index) = active_parallel_frame_index(state) else {
        return Ok(());
    };
    // Restore the branch-entry checkpoint recorded when this branch started
    // (`enter_parallel_frame`/`advance_or_complete_parallel`): everything the
    // abandoned activation produced — nested control frames, staged effects,
    // branch decisions, and guard evaluations — is discarded back to that
    // single boundary, rather than a list of unrelated per-field filters.
    let frame = &state.control_stack[index];
    let branch_decisions_watermark = frame.branch_decisions_watermark;
    let guard_evaluations_watermark = frame.guard_evaluations_watermark;
    let control_item_id = frame.control_item_id.clone();
    let branch_index = frame.iteration_index;

    state.control_stack.truncate(index + 1);
    let frame = &mut state.control_stack[index];
    frame.parallel_buffer = EffectBuffer::default();
    frame.next_index = 0;
    // `branch_decisions`/`guard_evaluations` are append-only and never
    // reordered, so truncating to the watermark drops exactly this branch
    // activation's evidence and nothing from earlier completed siblings
    // (whose entries were pushed, and therefore watermarked, before this
    // branch started).
    state.branch_decisions.truncate(branch_decisions_watermark);
    state.guard_evaluations.truncate(guard_evaluations_watermark);
    // `sequence_statuses` is sorted by `run_index` in `sort_state` after
    // every transition, so — unlike the two vectors above — "everything
    // pushed since the watermark" is not "the tail of the vector"; filter by
    // content (this branch's own position-path prefix) instead of truncating
    // by length.
    state.sequence_statuses.retain(|status| {
        status
            .position_path
            .first()
            .is_none_or(|segment| segment.id != control_item_id)
            || status
                .position_path
                .get(1)
                .is_none_or(|segment| segment.iteration != branch_index)
    });
    // The branch's first authored item is not necessarily executable — it
    // can itself be a nested Sequence/Branch/Loop/ForEach control, which
    // must be *entered* (pushing its own frame) before there is any current
    // leaf to describe. Reuse the same control-progression/readiness
    // machinery every ordinary control entry goes through (the accept path
    // always ends by calling this) rather than hand-reconstructing
    // `active_path` from the raw item, which cannot tell a control item from
    // a leaf and previously left `active_path` claiming a current nested
    // item that had never actually been entered.
    refresh_runtime_status(trait_ref, state)
}

/// The next run-global [`SlotRevision`] acceptance order. Isolation buffers
/// preserve this order when they are merged, so guard evidence can retain an
/// immutable revision coordinate.
fn next_acceptance_order(state: &State) -> usize {
    latest_recorded_slot_revision_order(state).saturating_add(1)
}

/// Record an accepted slot value and its revision at the active effect
/// target: the innermost active `parallel` branch's isolation buffer, or the
/// committed ledger when no panel is active. The single place both the
/// step-output path (`apply_step_output`) and the for-each item-binding path
/// (`bind_current_for_each_item`) route writes through, so a branch's
/// evidence is staged identically regardless of which caller produced it.
fn record_accepted_slot_value(state: &mut State, value: Value, revision: SlotRevision) {
    match active_parallel_frame_index(state) {
        Some(index) => {
            let buffer = &mut state.control_stack[index].parallel_buffer;
            upsert_runtime_value(&mut buffer.accepted_slot_values, value);
            buffer.slot_revisions.push(revision);
        }
        None => {
            upsert_runtime_value(&mut state.accepted_slot_values, value);
            state.slot_revisions.push(revision);
        }
    }
}

/// Record an accepted output-port value at the active effect target — see
/// [`record_accepted_slot_value`].
fn record_accepted_output_port_value(state: &mut State, value: Value) {
    match active_parallel_frame_index(state) {
        Some(index) => upsert_runtime_value(
            &mut state.control_stack[index].parallel_buffer.accepted_output_port_values,
            value,
        ),
        None => upsert_runtime_value(&mut state.accepted_output_port_values, value),
    }
}

/// Record accepted signal emissions at the active effect target — see
/// [`record_accepted_slot_value`].
fn record_emitted_signals(state: &mut State, signals: impl IntoIterator<Item = SignalEmission>) {
    match active_parallel_frame_index(state) {
        Some(index) => state.control_stack[index]
            .parallel_buffer
            .emitted_signals
            .extend(signals),
        None => state.emitted_signals.extend(signals),
    }
}

fn complete_or_repeat_current_control(trait_ref: &Trait, state: &mut State) -> crate::Result<bool> {
    let Some(frame) = state.control_stack.last() else {
        return Ok(false);
    };
    let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_UNRESOLVED_RUNTIME_SEQUENCE,
            state.active_path.clone(),
            None,
        );
        return Ok(true);
    };
    if frame.next_index < named.sequence.len() {
        return Ok(false);
    }

    match frame.kind {
        ControlKind::Sequence => {
            complete_current_control_success(state);
            Ok(true)
        }
        ControlKind::Branch => {
            let activation_path =
                path_for_control_frame_activation(state, state.control_stack.len() - 1);
            complete_current_control_success(state);
            if !state.control_stack.is_empty() {
                state.active_path = activation_path;
                evaluate_control_guards_after_step(trait_ref, state, &[])?;
            }
            Ok(true)
        }
        ControlKind::Loop => {
            let iteration = frame.iteration_index.unwrap_or(0);
            let Some(next_iteration) = iteration.checked_add(1) else {
                stop_with_reason(
                    state,
                    FinalState::Failed,
                    STOP_ITERATION_INDEX_OVERFLOW,
                    state.active_path.clone(),
                    None,
                );
                return Ok(true);
            };
            if frame.unbounded {
                // No bound to exhaust (0093): the guard path
                // (`evaluate_control_guards_after_step`) is the only exit —
                // this frame always just advances to the next iteration.
                if let Some(frame) = state.control_stack.last_mut() {
                    frame.iteration_index = Some(next_iteration);
                    frame.next_index = 0;
                }
                return Ok(true);
            }
            let Some(max_iterations) = frame.max_iterations else {
                let step = frame.control_item_id.as_deref().unwrap_or("unnamed");
                return Err(crate::procedure::invalid_field(
                    "runtime.control-stack.max-iterations",
                    format!("loop step {step:?} is unbounded and will not run"),
                ));
            };
            if next_iteration >= max_iterations {
                let failing_index = state.control_stack.len().saturating_sub(1);
                let path = path_for_control_frame_activation(state, failing_index);
                // Exhaustion is a normal outcome, not a failure: a bounded loop
                // that spends its budget without matching `until` ends like any
                // completed control item and the sequence proceeds. A budget is
                // a limit on effort, and reaching it says the work is
                // unfinished — not that the run is broken. What the loop
                // actually produced is in its slots, and the step after it is
                // responsible for reading them rather than assuming success.
                //
                // A loop declares no `on-failure`: it has no failure of its own
                // to route, and the items inside its body route theirs.
                //
                // `on-exhausted = "abort"` opts a loop back into stopping, for
                // procedures where an unmet exit condition genuinely invalidates
                // everything after it. `--strict-loops` does the same for every
                // loop in a run, from the caller's side, and also suppresses
                // signal emission on a continuing loop: the loop did not
                // continue, so emitting "the sequence moved on" would be false
                // evidence.
                let disposition = frame
                    .on_exhausted
                    .as_ref()
                    .map(ExhaustionTarget::disposition)
                    .unwrap_or(ExhaustionDisposition::Continue { signals: &[] });
                let should_continue =
                    matches!(disposition, ExhaustionDisposition::Continue { .. }) && !state.strict_loops;
                let signals_to_emit: Vec<String> = match disposition {
                    ExhaustionDisposition::Continue { signals } if should_continue => {
                        signals.to_vec()
                    }
                    _ => Vec::new(),
                };
                let parent_run_index = frame.parent_run_index;
                let identity = runtime_control_identity_from_frame(frame);
                if should_continue {
                    for signal in &signals_to_emit {
                        emit_runtime_control_signal(
                            state,
                            Some(signal),
                            parent_run_index,
                            Some(identity.clone()),
                            path.clone(),
                        )?;
                    }
                    complete_current_control_success(state);
                    return Ok(true);
                }
                stop_with_reason(
                    state,
                    FinalState::Blocked,
                    STOP_MAX_ITERATIONS_EXHAUSTED,
                    path.clone(),
                    None,
                );
            } else if let Some(frame) = state.control_stack.last_mut() {
                frame.iteration_index = Some(next_iteration);
                frame.next_index = 0;
            }
            Ok(true)
        }
        ControlKind::ForEach => {
            let item_total = frame.item_total.unwrap_or(0);
            let item_index = frame.item_index.unwrap_or(0);
            let Some(next_item_index) = item_index.checked_add(1) else {
                stop_with_reason(
                    state,
                    FinalState::Failed,
                    STOP_ITEM_INDEX_OVERFLOW,
                    state.active_path.clone(),
                    None,
                );
                return Ok(true);
            };
            if next_item_index >= item_total {
                let on_complete = frame.on_complete.clone();
                let parent = frame.parent_run_index;
                let control_identity = runtime_control_identity_from_frame(frame);
                if on_complete.is_some() {
                    emit_runtime_control_signal(
                        state,
                        on_complete.as_deref(),
                        parent,
                        Some(control_identity),
                        state.active_path.clone(),
                    )?;
                }
                complete_current_control_success(state);
            } else {
                if let Some(frame) = state.control_stack.last_mut() {
                    frame.item_index = Some(next_item_index);
                    frame.next_index = 0;
                }
                bind_current_for_each_item(trait_ref, state)?;
            }
            Ok(true)
        }
        ControlKind::Parallel => {
            advance_or_complete_parallel(trait_ref, state)?;
            Ok(true)
        }
    }
}

fn complete_current_control_success(state: &mut State) {
    let Some(completed) = state.control_stack.pop() else {
        return;
    };
    finish_control_completion(state, &completed.kind);
}

/// Shared post-pop completion: advance the enclosing frame past this control
/// item, or — with no enclosing frame — mark the top-level item accepted and
/// move the outer cursor forward. Shared by [`complete_current_control_success`]
/// and [`advance_or_complete_parallel`]'s barrier, which pops its own frame
/// first (to extract the completed branch buffers for merge) and then calls
/// this directly.
fn finish_control_completion(state: &mut State, completed_kind: &ControlKind) {
    if state.control_stack.last_mut().is_some() {
        advance_after_current_leaf(state);
    } else {
        set_current_outer_status(
            state,
            SequenceStatusKind::Accepted,
            format!(
                "control item completed: {}",
                control_kind_name(completed_kind)
            ),
        );
        if let Some(next) = state.current_run_index.checked_add(1) {
            state.current_run_index = next;
        } else {
            stop_with_reason(
                state,
                FinalState::Failed,
                STOP_RUN_INDEX_OVERFLOW,
                state.active_path.clone(),
                None,
            );
        }
        state.active_path.clear();
    }
}

pub(crate) fn bind_current_for_each_item(trait_ref: &Trait, state: &mut State) -> crate::Result<()> {
    let Some(frame) = state.control_stack.last() else {
        return Ok(());
    };
    if frame.kind != ControlKind::ForEach {
        return Ok(());
    }
    let over_slot = frame.over_slot.clone().unwrap_or_default();
    let item_slot = frame.item_slot.clone().unwrap_or_default();
    let item_index = frame.item_index.unwrap_or(0);
    let control_item_id = frame
        .control_item_id
        .clone()
        .unwrap_or_else(|| frame.sequence_id.clone());
    if visible_slot_revisions(state).into_iter().any(|revision| {
        revision.slot_ref.as_str() == item_slot.as_str()
            && revision.for_each_id.as_deref() == Some(control_item_id.as_str())
            && revision.item_index == Some(item_index)
    }) {
        return Ok(());
    }
    let Some(list_value) = accepted_value(state, &over_slot) else {
        return Ok(());
    };
    let Some(value) = list_value
        .value
        .as_array()
        .and_then(|items| items.get(item_index))
        .cloned()
    else {
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_FOR_EACH_ITEM_MISSING,
            state.active_path.clone(),
            None,
        );
        return Ok(());
    };
    let sink = OutputSink::Ref(item_slot.clone());
    let runtime_value = runtime_value_for_output_sink(
        trait_ref,
        frame.parent_run_index,
        &sink,
        StepSlotOutput {
            ref_text: item_slot,
            value,
            source: Some(ValueSource::Ledger),
            producer_evidence: Some("for-each item binding".to_string()),
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        },
        false,
    )?;
    if runtime_value.acceptance != AcceptanceStatus::Accepted {
        let rejection_path = for_each_binding_path(trait_ref, state)
            .unwrap_or_else(|| state.active_path.clone());
        let rejection_index = rejection_path
            .last()
            .filter(|segment| segment.kind == "item")
            .map_or(frame.next_index, |segment| segment.index);
        stop_with_reason(
            state,
            FinalState::Failed,
            STOP_FOR_EACH_ITEM_BINDING_REJECTED,
            rejection_path.clone(),
            None,
        );
        state.rejected_attempts.push(RejectedAttempt {
            sequence_index: rejection_index,
            position_path: rejection_path,
            ref_text: Some(runtime_value.ref_text.clone()),
            value_digest: Some(runtime_value.value_digest.clone()),
            reason: runtime_value.schema_validation.first().map_or_else(
                || "for-each item binding rejected".to_string(),
                |v| v.reason.clone(),
            ),
        });
        return Ok(());
    }
    let loop_context = loop_context_from_stack(&state.control_stack);
    let for_each_context = for_each_context_from_stack(&state.control_stack);
    let position_path =
        for_each_binding_path(trait_ref, state).unwrap_or_else(|| state.active_path.clone());
    let revision = slot_revision_from_value(
        &runtime_value,
        SlotRevisionWrite {
            operation: WriteOperation::Replace,
            submitted_payload: runtime_value.value.clone(),
            prior_value: accepted_value(state, &runtime_value.ref_text),
            runtime_binding: true,
            projection: None,
        },
        SlotRevisionContext {
            acceptance_order: next_acceptance_order(state),
            position_path: &position_path,
            loop_context: loop_context.as_ref(),
            for_each_context: for_each_context.as_ref(),
        },
    )?;
    record_accepted_slot_value(state, runtime_value, revision);
    Ok(())
}

fn for_each_binding_path(trait_ref: &Trait, state: &State) -> Option<Vec<PathSegment>> {
    let frame = state.control_stack.last()?;
    if frame.kind != ControlKind::ForEach {
        return None;
    }
    let named = trait_ref.sequences.get(&frame.sequence_id)?;
    let item = named.sequence.get(frame.next_index)?;
    Some(path_for_nested_item(state, frame.next_index, item))
}

fn stop_with_reason(
    state: &mut State,
    final_state: FinalState,
    reason: &str,
    at: Vec<PathSegment>,
    last_check: Option<usize>,
) {
    if final_state == FinalState::Blocked
        && (at.is_empty()
            || at.first().is_some_and(|segment| {
                segment.kind == "procedure" && segment.index == state.current_run_index
            }))
    {
        set_current_outer_status(state, SequenceStatusKind::Blocked, reason.to_string());
    }
    state.final_state = final_state;
    state.stop_reason = Some(StopReason {
        reason: reason.to_string(),
        at,
        last_check,
    });
    state.active_path.clear();
}

/// Move the single cursor to a validated later recovery step. This is a
/// control-flow transition, not a retry: the failed stack is discarded.
fn route_failure(
    trait_ref: &Trait,
    state: &mut State,
    source_run_index: usize,
    source_step_id: Option<&str>,
    on_failure: Option<&FailureTarget>,
    position_path: Vec<PathSegment>,
) -> crate::Result<bool> {
    let Some(route) = on_failure.and_then(FailureTarget::route) else {
        return Ok(false);
    };
    let Some(source_step_id) = source_step_id else {
        return Ok(false);
    };
    let sequence = effective_sequence_items(procedure(trait_ref)?)?;
    let Some(source) = sequence.iter().find(|item| item.run_index == source_run_index) else {
        return Ok(false);
    };
    let Some(target) = sequence.iter().find(|item| item.item.id.as_deref() == Some(route.step.as_str())) else {
        return Ok(false);
    };
    if target.run_index <= source_run_index {
        return Ok(false);
    }
    let position_path = if position_path.is_empty() {
        path_for_control_item(source_run_index, source.item)
    } else {
        position_path
    };
    if state.failure_routes.iter().any(|record| {
        record.source_run_index == source_run_index
            && record.position_path == position_path
    }) {
        return Ok(false);
    }
    // This route always clears `control_stack` below, which would silently
    // discard any active `parallel` branch's isolation buffer. The failure
    // route's own evidence signals (unlike the branch's ordinary staged
    // effects) must survive that clear, so watermark the active buffer here
    // and drain only what this route emits into the committed ledger before
    // clearing.
    let parallel_signal_watermark = active_parallel_frame_index(state)
        .map(|index| state.control_stack[index].parallel_buffer.emitted_signals.len());
    emit_legacy_failure_signals_before_route(state, &position_path)?;
    let source_sequence_index = position_path
        .last()
        .filter(|segment| segment.kind == "item")
        .map_or(source.declaration_index, |segment| segment.index);
    emit_runtime_control_signal_at(
        state,
        route.signal.as_deref(),
        source_sequence_index,
        position_path.clone(),
        None,
    )?;
    state.failure_routes.push(FailureRouteRecord {
        source_run_index,
        source_step_id: source_step_id.to_string(),
        target_run_index: target.run_index,
        target_step_id: route.step.clone(),
        signal: route.signal.clone(),
        position_path,
    });
    set_sequence_status(
        state,
        source.declaration_index,
        SequenceStatusKind::Routed,
        "failure routed to recovery step",
    );
    for skipped in sequence.iter().filter(|item| {
        item.run_index > source_run_index && item.run_index < target.run_index
    }) {
        set_sequence_status(
            state,
            skipped.declaration_index,
            SequenceStatusKind::Skipped,
            "bypassed by failure recovery route",
        );
    }
    if let (Some(index), Some(watermark)) =
        (active_parallel_frame_index(state), parallel_signal_watermark)
    {
        let buffer = &mut state.control_stack[index].parallel_buffer;
        if buffer.emitted_signals.len() > watermark {
            let route_evidence = buffer.emitted_signals.split_off(watermark);
            state.emitted_signals.extend(route_evidence);
        }
    }
    state.control_stack.clear();
    state.active_path.clear();
    state.stop_reason = None;
    state.current_run_index = target.run_index;
    state.final_state = FinalState::Running;
    // Clearing the control stack abandons every nested position this panel
    // (or any control nested inside it) was tracking. Their per-position
    // status entries are display evidence for a scope that no longer exists
    // and, unlike the top-level source status just marked `Routed` above,
    // can never become `Accepted` or route-authorized (`route_authorizes_status`
    // only recognizes top-level entries) — left behind, a mid-flight nested
    // `Ready`/`Pending` entry permanently fails the completed-ledger
    // all-statuses-accepted check once the run reaches the recovery target.
    state
        .sequence_statuses
        .retain(|status| status.position_path.is_empty());
    refresh_runtime_status(trait_ref, state)?;
    Ok(true)
}

fn route_enclosing_failure(
    trait_ref: &Trait,
    state: &mut State,
    failing_control_index: usize,
    position_path: &[PathSegment],
) -> crate::Result<bool> {
    for index in (0..failing_control_index.min(state.control_stack.len())).rev() {
        let frame = state.control_stack[index].clone();
        // Ordinary nested recovery (every frame strictly inside this
        // `parallel` branch's own body) is tried first, by the caller and by
        // this same loop's inner-to-outer walk, before this point is ever
        // reached for a `Parallel` frame. Only once that is exhausted does a
        // declared `branch-failure` policy for the CURRENTLY active branch
        // apply (P264) — `skip`/`park` resolve the failure here without ever
        // trying the panel's own `on-failure`; the default `panel-fail`
        // policy falls through to the ordinary `route_failure` call below,
        // reusing the panel's `on-failure` exactly like any other control
        // kind's unrouted failure.
        if frame.kind == ControlKind::Parallel
            && let Some(handled) = resolve_active_branch_failure(trait_ref, state, index, position_path)?
        {
            return Ok(handled);
        }
        if route_failure(
            trait_ref,
            state,
            frame.parent_run_index,
            frame.control_item_id.as_deref(),
            frame.on_failure.as_ref(),
            position_path.to_vec(),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve the declared `branch-failure` policy (P264) for the branch
/// currently active on the `parallel` frame at `parallel_frame_index`, once
/// ordinary nested recovery inside that branch's own body is exhausted.
/// Returns `Some(true)` when the failure was fully resolved right here
/// (`skip` or `park`, which never escalates further) — the caller must not
/// also try this frame's own `on-failure`. Returns `None` for the default
/// `panel-fail` policy (or no declared policy at all), so the caller falls
/// through to the ordinary `route_failure` escalation.
fn resolve_active_branch_failure(
    trait_ref: &Trait,
    state: &mut State,
    parallel_frame_index: usize,
    position_path: &[PathSegment],
) -> crate::Result<Option<bool>> {
    let frame = &state.control_stack[parallel_frame_index];
    let branch_index = frame.iteration_index.unwrap_or(0);
    let Some(branch_ref) = frame.parallel_branch_refs.get(branch_index).cloned() else {
        return Ok(None);
    };
    let policy = frame
        .branch_failure
        .iter()
        .find(|entry| entry.branch == branch_ref)
        .map(|entry| entry.on_failure)
        .unwrap_or(BranchFailurePolicy::PanelFail);
    match policy {
        BranchFailurePolicy::PanelFail => Ok(None),
        BranchFailurePolicy::Skip => {
            reset_active_branch_to_clean_entry_state(state, parallel_frame_index);
            complete_current_parallel_branch(trait_ref, state, ParallelBranchOutcome::Skipped)?;
            // Unlike `route_failure` (which calls this itself before
            // returning), neither `complete_current_parallel_branch` nor this
            // function's caller (`reject_step_output`'s `enclosing_route`
            // branch) refreshes readiness for whatever position `skip`
            // advanced the cursor to next (the panel's next branch, or past
            // the panel entirely) — without this call that new current
            // position is left at its stale `Pending` sequence-status
            // instead of `Ready`, which the ledger contract then rejects.
            refresh_runtime_status(trait_ref, state)?;
            Ok(Some(true))
        }
        BranchFailurePolicy::Park => {
            // Never escalates and never truncates: the branch's own position
            // (and every nested frame above it) is left exactly as it stands
            // so a later resumed call can retry from the same failing point,
            // exactly like an ordinary blocked stop elsewhere in the runtime.
            let panel_position_path =
                path_for_control_frame_activation(state, parallel_frame_index);
            let control_item_id = frame.control_item_id.clone();
            let join_label = frame
                .join
                .as_ref()
                .map_or("collect-in-order", JoinPolicy::label)
                .to_string();
            let branches: Vec<ParallelPanelBranchRecord> = frame
                .parallel_branch_refs
                .iter()
                .enumerate()
                .map(|(index, branch_ref)| ParallelPanelBranchRecord {
                    branch_ref: branch_ref.clone(),
                    outcome: if index == branch_index {
                        Some(ParallelBranchOutcome::Parked)
                    } else {
                        frame.parallel_branch_outcomes.get(index).copied().flatten()
                    },
                })
                .collect();
            stop_with_reason(
                state,
                FinalState::Blocked,
                STOP_PARALLEL_BRANCH_PARKED,
                position_path.to_vec(),
                None,
            );
            state.parallel_panel_records.push(ParallelPanelRecord {
                control_item_id,
                position_path: panel_position_path,
                join_policy: join_label,
                branches,
                result_digest: None,
                guard_evaluation_index: None,
                disposition: ParallelPanelDisposition::Parked,
            });
            Ok(Some(true))
        }
    }
}

/// Discard the currently active `parallel` branch's own uncommitted progress
/// (every nested frame pushed since it started, its isolation buffer, and the
/// branch decisions/guard evaluations it produced) back to a clean state, so
/// [`complete_current_parallel_branch`] can treat it as a resolved `Skipped`
/// branch rather than retrying it. Mirrors [`rollback_active_parallel_branch`]
/// but is driven by a declared `branch-failure` policy rather than a
/// corrected-resubmission rollback, and the caller (not this function)
/// decides the branch's resolved outcome and whether to advance.
fn reset_active_branch_to_clean_entry_state(state: &mut State, parallel_frame_index: usize) {
    let frame = &state.control_stack[parallel_frame_index];
    let branch_decisions_watermark = frame.branch_decisions_watermark;
    let guard_evaluations_watermark = frame.guard_evaluations_watermark;
    let control_item_id = frame.control_item_id.clone();
    let branch_index = frame.iteration_index;
    state.control_stack.truncate(parallel_frame_index + 1);
    let frame = &mut state.control_stack[parallel_frame_index];
    frame.parallel_buffer = EffectBuffer::default();
    state.branch_decisions.truncate(branch_decisions_watermark);
    state.guard_evaluations.truncate(guard_evaluations_watermark);
    state.sequence_statuses.retain(|status| {
        status
            .position_path
            .first()
            .is_none_or(|segment| segment.id != control_item_id)
            || status
                .position_path
                .get(1)
                .is_none_or(|segment| segment.iteration != branch_index)
    });
}

fn path_for_control_item(
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
) -> Vec<PathSegment> {
    vec![PathSegment {
        kind: "procedure".to_string(),
        id: item.id.clone(),
        index: parent_run_index,
        iteration: None,
        item_index: None,
    }]
}

fn path_for_branch_item(
    state: &State,
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
) -> Vec<PathSegment> {
    let Some(frame) = state.control_stack.last() else {
        return path_for_control_item(parent_run_index, item);
    };
    // Nested branch ids are local to their named sequence. Preserve the normal
    // nested activation path so the decision resolves to that exact occurrence.
    path_for_nested_item(state, frame.next_index, item)
}

fn path_for_control_item_activation(
    state: &State,
    parent_run_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
) -> Vec<PathSegment> {
    let Some(frame) = state.control_stack.last() else {
        return path_for_control_item(parent_run_index, item);
    };
    path_for_nested_item(state, frame.next_index, item)
}

fn path_for_control_frame_activation(state: &State, frame_index: usize) -> Vec<PathSegment> {
    let Some(frame) = state.control_stack.get(frame_index) else {
        return Vec::new();
    };
    let Some(first) = state.control_stack.first() else {
        return Vec::new();
    };
    if frame_index == 0 {
        return vec![PathSegment {
            kind: "procedure".to_string(),
            id: frame.control_item_id.clone(),
            index: frame.parent_run_index,
            iteration: None,
            item_index: None,
        }];
    }
    let mut path = vec![PathSegment {
        kind: "procedure".to_string(),
        id: first.control_item_id.clone(),
        index: first.parent_run_index,
        iteration: None,
        item_index: None,
    }];
    for enclosing in state.control_stack.iter().take(frame_index) {
        path.push(PathSegment {
            kind: control_kind_name(&enclosing.kind).to_string(),
            id: Some(enclosing.sequence_id.clone()),
            index: enclosing.next_index,
            iteration: enclosing.iteration_index,
            item_index: enclosing.item_index,
        });
    }
    let enclosing = &state.control_stack[frame_index - 1];
    path.push(PathSegment {
        kind: "item".to_string(),
        id: frame.control_item_id.clone(),
        index: enclosing.next_index,
        iteration: state
            .control_stack
            .iter()
            .take(frame_index)
            .rev()
            .find_map(|frame| frame.iteration_index),
        item_index: state
            .control_stack
            .iter()
            .take(frame_index)
            .rev()
            .find_map(|frame| frame.item_index),
    });
    path
}
