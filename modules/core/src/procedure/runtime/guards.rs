// Procedure runtime guards.
// Procedure runtime guards.

fn evaluate_control_guards_after_step(
    trait_ref: &Trait,
    state: &mut State,
    current_outputs: &[Value],
) -> crate::Result<()> {
    let loop_indexes: Vec<usize> = state
        .control_stack
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| (frame.kind == ControlKind::Loop).then_some(index))
        .collect();
    for loop_index in loop_indexes.into_iter().rev() {
        let frame = state.control_stack[loop_index].clone();
        // An unbounded loop (0093) has no iteration ceiling to report; guard
        // evaluation only reads `iteration_index`, so `usize::MAX` stands in
        // as "no bound" rather than erroring a step that is only here to
        // check `until`/`abort-if` — those guards are this frame's only exit.
        let max_iterations = match frame.max_iterations {
            Some(value) => value,
            None if frame.unbounded => usize::MAX,
            None => {
                let step = frame.control_item_id.as_deref().unwrap_or("unnamed");
                return Err(crate::procedure::invalid_field(
                    "runtime.control-stack.max-iterations",
                    format!("loop step {step:?} is unbounded and will not run"),
                ));
            }
        };
        let loop_context = LoopContext {
            loop_id: frame
                .control_item_id
                .clone()
                .unwrap_or_else(|| frame.sequence_id.clone()),
            sequence_id: Some(frame.sequence_id.clone()),
            iteration_index: frame.iteration_index.unwrap_or(0),
            max_iterations,
        };
        let guard_path_len = (loop_index + 2).min(state.active_path.len());
        let guard_path = state.active_path[..guard_path_len].to_vec();
        let mut stop_check = None;
        let stop_match = if let Some(abort_if) = frame.abort_if.as_ref() {
            let (matched, evaluations) =
                evaluate_guard_expr(
                    trait_ref,
                    state,
                    abort_if,
                    &loop_context,
                    &guard_path,
                    current_outputs,
                )?;
            stop_check = append_guard_evaluations(state, evaluations);
            matched
        } else {
            false
        };
        let mut until_check = None;
        let until_match = if let Some(until) = frame.until.as_ref() {
            let (matched, evaluations) =
                evaluate_guard_expr(
                    trait_ref,
                    state,
                    until,
                    &loop_context,
                    &guard_path,
                    current_outputs,
                )?;
            until_check = append_guard_evaluations(state, evaluations);
            matched
        } else {
            false
        };

        if stop_match && until_match {
            let path = state.active_path.clone();
            if route_enclosing_failure(trait_ref, state, loop_index, &path)? {
                return Ok(());
            }
            state.control_stack.truncate(loop_index + 1);
            stop_with_reason(
                state,
                FinalState::Blocked,
                STOP_GUARD_CONFLICT,
                path.clone(),
                stop_check.or(until_check),
            );
            emit_enclosing_failure_signals(state, loop_index, &path)?;
            return Ok(());
        } else if stop_match {
            let path = state.active_path.clone();
            if route_enclosing_failure(trait_ref, state, loop_index, &path)? {
                return Ok(());
            }
            state.control_stack.truncate(loop_index + 1);
            stop_with_reason(
                state,
                FinalState::Blocked,
                STOP_STOP_IF_MATCHED,
                path.clone(),
                stop_check,
            );
            // The runtime's stop *reason* stays the accurate mechanism
            // (`abort-if-matched`) regardless of authoring; a declared
            // `on-abort` only substitutes which signal(s) are emitted, so a
            // trait's own name for its terminal condition (e.g.
            // `recurring-blocker-unresolved`) reaches the ledger instead of
            // the canonical signal.
            let declared_signals = frame.on_abort.as_ref().map(ExhaustionTarget::signals).unwrap_or_default();
            let signals_to_emit: Vec<String> = if declared_signals.is_empty() {
                vec![abort_if_matched_signal_ref()]
            } else {
                declared_signals.to_vec()
            };
            for signal in &signals_to_emit {
                emit_runtime_control_signal(
                    state,
                    Some(signal),
                    frame.parent_run_index,
                    Some(runtime_control_identity_from_frame(&frame)),
                    path.clone(),
                )?;
            }
            return Ok(());
        } else if until_match {
            state.control_stack.truncate(loop_index + 1);
            complete_current_control_success(state);
            continue;
        }
    }
    Ok(())
}

fn append_guard_evaluations(
    state: &mut State,
    evaluations: Vec<ConditionEvaluation>,
) -> Option<usize> {
    if evaluations.is_empty() {
        return None;
    }
    state.guard_evaluations.extend(evaluations);
    state.guard_evaluations.len().checked_sub(1)
}

/// Record the runtime's per-guarded-resource-input inclusion decision for the
/// item the cursor has just settled on (P290), reusing the same guard
/// evaluation, marker, and replay-authenticated decision discipline
/// `enter_control_frame` uses for branch selection. Idempotent by
/// construction: a decision keyed on the sequence index, ref, and runtime
/// position means a repeat settle at the same position (e.g. a second
/// `refresh_runtime_status` pass that does not move the cursor) never
/// appends a duplicate decision, so a reconstructed session replaying the
/// same transitions reaches the same ledger.
fn record_conditional_input_decisions(
    trait_ref: &Trait,
    state: &mut State,
    ready: &ReadyItem<'_>,
) -> crate::Result<()> {
    let guarded: Vec<(String, GuardExpr)> = ready
        .item
        .input
        .iter()
        .filter_map(|input| {
            input
                .guard()
                .map(|guard| (input.ref_text().to_string(), guard.clone()))
        })
        .collect();
    if guarded.is_empty() {
        return Ok(());
    }
    let context = guard_context_from_stack(&state.control_stack).unwrap_or(LoopContext {
        loop_id: String::new(),
        sequence_id: None,
        iteration_index: 0,
        max_iterations: 1,
    });
    let position_path = producer_path_for_ready(ready);
    for (ref_text, guard) in guarded {
        if state.conditional_input_decisions.iter().any(|decision| {
            decision.sequence_index == ready.sequence_index
                && decision.ref_text == ref_text
                && decision.position_path == position_path
        }) {
            continue;
        }
        let marker_key = serde_json::to_string(&(ready.sequence_index, &ref_text, &guard, &position_path))
            .map_err(|error| {
                crate::procedure::serialization("runtime.input.when", "conditional input guard", error)
            })?;
        let marker_predicate = format!("input:{ref_text}:{marker_key}");
        let guard_evaluation_start_index = state.guard_evaluations.len();
        let slot_revision_watermark = latest_recorded_slot_revision_order(state);
        let (matched, evaluations) =
            evaluate_guard_expr(trait_ref, state, &guard, &context, &position_path, &[])?;
        append_guard_evaluations(state, evaluations);
        let guard_evaluation_index = append_guard_evaluations(
            state,
            vec![condition_evaluation(
                &marker_predicate,
                None,
                &context,
                matched,
                "conditional input inclusion",
            )],
        )
        .expect("conditional input inclusion marker is not empty");
        state
            .conditional_input_decisions
            .push(ConditionalInputDecision {
                sequence_index: ready.sequence_index,
                ref_text,
                position_path: position_path.clone(),
                matched,
                when: guard,
                guard_evaluation_start_index: Some(guard_evaluation_start_index),
                slot_revision_watermark: Some(slot_revision_watermark),
                guard_evaluation_index,
            });
    }
    Ok(())
}

fn evaluate_guard_expr(
    trait_ref: &Trait,
    state: &State,
    guard: &GuardExpr,
    loop_context: &LoopContext,
    position_path: &[PathSegment],
    current_outputs: &[Value],
) -> crate::Result<(bool, Vec<ConditionEvaluation>)> {
    let repeated_scope = repeated_activation_scope(position_path);
    let evidence = GuardEvidence {
        repeated_scope: &repeated_scope,
        current_outputs,
    };
    let (outcome, evaluations) = evaluate_guard_expr_with_seen(
        trait_ref,
        state,
        guard,
        loop_context,
        &evidence,
        0,
        &mut BTreeSet::new(),
    )?;
    Ok((outcome.routes_true(), evaluations))
}

struct GuardEvidence<'a> {
    repeated_scope: &'a [RepeatedActivation],
    current_outputs: &'a [Value],
}

/// Internal tri-state walk. Only [`evaluate_guard_expr`] converts to `bool`,
/// at the single routing boundary — every recursive/combinator step here
/// stays [`GuardOutcome`] so `not`'s fail-closed rule (`not(Unmeasurable)`
/// stays `Unmeasurable`, never `Matched`) cannot be lost partway through a
/// nested guard.
fn evaluate_guard_expr_with_seen(
    trait_ref: &Trait,
    state: &State,
    guard: &GuardExpr,
    loop_context: &LoopContext,
    evidence: &GuardEvidence<'_>,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
) -> crate::Result<(GuardOutcome, Vec<ConditionEvaluation>)> {
    if depth > MAX_GUARD_EVALUATION_DEPTH {
        return Ok((
            GuardOutcome::NotMatched,
            vec![condition_evaluation(
                "guard-depth",
                Some(loop_evidence_ref(loop_context)),
                loop_context,
                false,
                "guard evaluation exceeded maximum depth",
            )],
        ));
    }
    match guard {
        GuardExpr::Ref(ref_text) => evaluate_guard_ref(
            trait_ref,
            state,
            ref_text,
            loop_context,
            evidence,
            depth,
            seen_conditions,
        ),
        GuardExpr::Any(items) => {
            let mut all_evaluations = Vec::new();
            let mut outcome = GuardOutcome::NotMatched;
            for item in items {
                let (item_outcome, evaluations) = evaluate_guard_expr_with_seen(
                    trait_ref,
                    state,
                    item,
                    loop_context,
                    evidence,
                    depth + 1,
                    seen_conditions,
                )?;
                outcome = outcome.or(item_outcome);
                all_evaluations.extend(evaluations);
            }
            all_evaluations.push(condition_evaluation_outcome(
                "any[...]",
                Some(loop_evidence_ref(loop_context)),
                loop_context,
                outcome,
                match outcome {
                    GuardOutcome::Matched => "at least one guard matched",
                    GuardOutcome::Unmeasurable => "no guard matched; at least one is unmeasurable",
                    GuardOutcome::NotMatched => "no guard matched",
                },
            ));
            Ok((outcome, all_evaluations))
        }
        GuardExpr::Predicate(predicate) => evaluate_guard_predicate(
            trait_ref,
            state,
            predicate,
            loop_context,
            evidence,
            depth,
            seen_conditions,
        ),
    }
}

fn evaluate_guard_ref(
    trait_ref: &Trait,
    state: &State,
    ref_text: &str,
    loop_context: &LoopContext,
    evidence: &GuardEvidence<'_>,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
) -> crate::Result<(GuardOutcome, Vec<ConditionEvaluation>)> {
    let parsed = Reference::parse(ref_text).map_err(|_| {
        crate::procedure::invalid_field("runtime.guard", format!("invalid guard ref {ref_text:?}"))
    })?;
    match parsed.kind() {
        Kind::Signal => {
            let matched = if evidence.repeated_scope.is_empty() {
                visible_emitted_signals(state)
                    .into_iter()
                    .any(|signal| {
                        signal.acceptance == AcceptanceStatus::Accepted
                            && signal.signal_ref.as_str() == ref_text
                    })
            } else {
                signal_matched_in_scope(state, ref_text, evidence.repeated_scope)
            };
            Ok((
                GuardOutcome::from_bool(matched),
                vec![condition_evaluation(
                    ref_text,
                    Some(loop_evidence_ref(loop_context)),
                    loop_context,
                    matched,
                    if matched {
                        "signal emitted in current loop iteration"
                    } else {
                        "signal not emitted in current loop iteration"
                    },
                )],
            ))
        }
        Kind::Condition => {
            let condition_id = parsed.id().to_string();
            if !seen_conditions.insert(condition_id.clone()) {
                return Ok((
                    GuardOutcome::NotMatched,
                    vec![condition_evaluation(
                        ref_text,
                        Some(loop_evidence_ref(loop_context)),
                        loop_context,
                        false,
                        "condition cycle detected during guard evaluation",
                    )],
                ));
            }
            let Some(condition) = trait_ref.conditions.get(parsed.id()) else {
                return Ok((
                    GuardOutcome::NotMatched,
                    vec![condition_evaluation(
                        ref_text,
                        Some(loop_evidence_ref(loop_context)),
                        loop_context,
                        false,
                        "condition ref is not declared",
                    )],
                ));
            };
            let result = evaluate_guard_expr_with_seen(
                trait_ref,
                state,
                &condition.as_guard(),
                loop_context,
                evidence,
                depth + 1,
                seen_conditions,
            );
            seen_conditions.remove(&condition_id);
            result
        }
        _ => Ok((
            GuardOutcome::NotMatched,
            vec![condition_evaluation(
                ref_text,
                Some(loop_evidence_ref(loop_context)),
                loop_context,
                false,
                "guard ref is not a signal or condition",
            )],
        )),
    }
}

/// Latest revision of `slot_ref` was written by THIS loop's body in a
/// previous iteration — the value is stale for this loop's guard purposes.
fn stale_repeated_slot(
    state: &State,
    slot_ref: &str,
    repeated_scope: &[RepeatedActivation],
) -> bool {
    let visible = visible_slot_revisions(state);
    let Some(revision) = visible
        .iter()
        .rev()
        .find(|revision| revision.slot_ref.as_str() == slot_ref)
    else {
        return false;
    };
    let revision_scope = repeated_activation_scope(&revision.position_path);
    // A value written in an enclosing repeated control is still current in
    // nested controls when every shared activation coordinate matches.
    for (revision, current) in revision_scope.iter().zip(repeated_scope) {
        if !same_repeated_control(revision, current) {
            return false;
        }
        if revision.iteration != current.iteration || revision.item_index != current.item_index {
            return true;
        }
    }
    false
}

fn same_repeated_control(left: &RepeatedActivation, right: &RepeatedActivation) -> bool {
    left.kind == right.kind && left.id == right.id && left.owner_path == right.owner_path
}

fn evaluate_guard_predicate(
    trait_ref: &Trait,
    state: &State,
    predicate: &crate::r#trait::condition::GuardPredicate,
    loop_context: &LoopContext,
    evidence: &GuardEvidence<'_>,
    depth: usize,
    seen_conditions: &mut BTreeSet<String>,
) -> crate::Result<(GuardOutcome, Vec<ConditionEvaluation>)> {
    if let Some(signal) = predicate.signal.as_deref() {
        return evaluate_guard_ref(
            trait_ref,
            state,
            signal,
            loop_context,
            evidence,
            depth,
            seen_conditions,
        );
    }
    if let Some(condition) = predicate.condition.as_deref() {
        return evaluate_guard_ref(
            trait_ref,
            state,
            condition,
            loop_context,
            evidence,
            depth,
            seen_conditions,
        );
    }
    if let Some(not) = predicate.not.as_deref() {
        let (child_outcome, mut evaluations) = evaluate_guard_expr_with_seen(
            trait_ref,
            state,
            not,
            loop_context,
            evidence,
            depth + 1,
            seen_conditions,
        )?;
        let depth_exceeded = evaluations
            .iter()
            .any(|evaluation| evaluation.predicate == "guard-depth");
        let outcome = if depth_exceeded {
            GuardOutcome::NotMatched
        } else {
            child_outcome.negate()
        };
        evaluations.push(condition_evaluation_outcome(
            "not[...]",
            Some(loop_evidence_ref(loop_context)),
            loop_context,
            outcome,
            if depth_exceeded {
                "nested guard exceeded maximum depth"
            } else {
                match outcome {
                    GuardOutcome::Matched => "nested guard did not match",
                    GuardOutcome::NotMatched => "nested guard matched",
                    GuardOutcome::Unmeasurable => "nested guard evidence is unmeasurable",
                }
            },
        ));
        return Ok((outcome, evaluations));
    }
    if let Some(iteration) = predicate.iteration {
        let current = serde_json::Number::from(loop_context.iteration_index as u64);
        let expected = serde_json::Number::from(iteration);
        let matched = compare_json_numbers(&current, &expected).is_eq();
        return Ok((
            GuardOutcome::from_bool(matched),
            vec![condition_evaluation(
                &format!("iteration={iteration}"),
                Some(loop_evidence_ref(loop_context)),
                loop_context,
                matched,
                if matched {
                    "iteration matches"
                } else {
                    "iteration does not match"
                },
            )],
        ));
    }
    if let Some(iteration_at_least) = predicate.iteration_at_least {
        let current = serde_json::Number::from(loop_context.iteration_index as u64);
        let expected = serde_json::Number::from(iteration_at_least);
        let matched = compare_json_numbers(&current, &expected).is_ge();
        return Ok((
            GuardOutcome::from_bool(matched),
            vec![condition_evaluation(
                &format!("iteration-at-least={iteration_at_least}"),
                Some(loop_evidence_ref(loop_context)),
                loop_context,
                matched,
                if matched {
                    "iteration lower bound satisfied"
                } else {
                    "iteration lower bound not satisfied"
                },
            )],
        ));
    }
    if let Some(threshold) = predicate.elapsed_seconds_at_least.as_ref() {
        let lhs = ComparisonOperandEvidence::Literal {
            value: JsonValue::from(state.elapsed_seconds),
        };
        let rhs = if let Some(ref_text) = crate::r#trait::condition::numeric_comparison_ref(threshold)
        {
            comparison_ref_operand(state, accepted_value(state, ref_text), ref_text, None, false)
        } else {
            ComparisonOperandEvidence::Literal {
                value: threshold.clone(),
            }
        };
        // The declared schema only proves a ref-backed threshold is numeric,
        // never that its runtime value is non-negative (see
        // `validate_elapsed_predicate`). A negative resolved threshold would
        // make `elapsed-seconds >= -1` match immediately, so `comparison_result`
        // itself fails this closed for the `Elapsed` subject — the same rule
        // ledger replay applies via `evidence.subject`, so evaluation and
        // replay can never disagree on the stored `result`.
        let rhs_negative = operand_selected_value(&rhs)
            .and_then(JsonValue::as_f64)
            .is_some_and(f64::is_sign_negative);
        let result = comparison_result(
            ConditionComparisonOperator::AtLeast,
            &lhs,
            &rhs,
            ConditionComparisonSubject::Elapsed,
        );
        let comparison_evidence = ConditionComparisonEvidence {
            subject: ConditionComparisonSubject::Elapsed,
            lhs,
            operator: ConditionComparisonOperator::AtLeast,
            rhs,
            result,
            stale: false,
        };
        let mut evaluation = condition_evaluation(
            &format!("elapsed-seconds >= {threshold}"),
            None,
            loop_context,
            result,
            if rhs_negative {
                "elapsed-seconds-at-least threshold resolved to a negative runtime value; guard fails closed"
            } else if result {
                "elapsed-seconds evidence matched"
            } else {
                "elapsed-seconds evidence did not match"
            },
        );
        evaluation.comparison_evidence = Some(comparison_evidence);
        return Ok((GuardOutcome::from_bool(result), vec![evaluation]));
    }
    if let Some(slot_ref) = predicate.empty.as_deref() {
        let accepted = accepted_value(state, slot_ref);
        let stale = accepted.is_some()
            && stale_repeated_slot(state, slot_ref, evidence.repeated_scope);
        let matched = !stale
            && accepted.is_some_and(|value| match &value.value {
                // A list is empty at zero length; a text slot at the empty
                // string — both are "nothing accumulated here yet".
                serde_json::Value::Array(items) => items.is_empty(),
                serde_json::Value::String(text) => text.is_empty(),
                _ => false,
            });
        return Ok((
            GuardOutcome::from_bool(matched),
            vec![condition_evaluation(
                &format!("empty({slot_ref})"),
                Some(slot_ref.to_string()),
                loop_context,
                matched,
                slot_list_reason(stale, accepted.is_some(), matched),
            )],
        ));
    }
    if let Some(subject_ref) = predicate.present.as_deref() {
        return Ok(evaluate_present_predicate(
            state,
            predicate,
            subject_ref,
            loop_context,
            evidence,
        ));
    }
    if let Some(slot_ref) = predicate.count.as_deref() {
        let accepted = accepted_value(state, slot_ref);
        let filter = predicate.field.as_deref().zip(predicate.field_equals.as_ref());
        let (count, stale) = measured_count(state, slot_ref, predicate.field.as_deref(), predicate.field_equals.as_ref(), evidence);
        // The label names what was counted, so a filtered count reads as
        // `count(slot:x where status == "fail")` in the run's explanation
        // rather than a bare number no one can reconcile with the ledger.
        let counted = match filter {
            Some((field, expected)) => format!("{slot_ref} where {field} == {expected}"),
            None => slot_ref.to_string(),
        };
        let (label, outcome) = if let Some(expected) = predicate.equals.as_ref() {
            let threshold = count_threshold(state, expected, evidence);
            (
                format!("count({counted}) == {expected}"),
                count.zip(threshold).map_or(GuardOutcome::Unmeasurable, |(actual, threshold)| GuardOutcome::from_bool(actual == threshold)),
            )
        } else if let Some(expected) = predicate.at_least.as_ref() {
            let threshold = count_threshold(state, expected, evidence);
            (
                format!("count({counted}) >= {expected}"),
                count.zip(threshold).map_or(GuardOutcome::Unmeasurable, |(actual, threshold)| GuardOutcome::from_bool(actual >= threshold)),
            )
        } else {
            (format!("count({counted})"), GuardOutcome::NotMatched)
        };
        let outcome = if (stale || count.is_none()) && trait_ref.schema_version.as_str() == "0.3" {
            GuardOutcome::Unmeasurable
        } else if stale || count.is_none() {
            GuardOutcome::NotMatched
        } else { outcome };
        let matched = outcome.routes_true();
        return Ok((
            outcome,
            vec![condition_evaluation_outcome(
                &label,
                Some(slot_ref.to_string()),
                loop_context,
                outcome,
                slot_list_reason(stale, accepted.is_some(), matched),
            )],
        ));
    }
    if let Some(slot_ref) = predicate.slot.as_deref() {
        let accepted = accepted_value(state, slot_ref);
        // A loop-body-produced value from an EARLIER iteration must not satisfy
        // this loop's guards: an approval given before the body ran again
        // describes a superseded state (observed live: a verdict from
        // iteration 1 exited the loop after iteration 2's apply-fixes changed
        // the tree, so the final state escaped its second review). Values
        // produced outside this loop, or in the current iteration, are exempt.
        let stale = accepted.is_some()
            && stale_repeated_slot(state, slot_ref, evidence.repeated_scope);
        let comparison = runtime_comparison_modifier(predicate);
        let comparison_evidence = comparison.map(|(operator, expected)| {
            condition_comparison_evidence(
                state,
                ComparisonLhs {
                    subject: ConditionComparisonSubject::Slot,
                    accepted,
                    ref_text: slot_ref,
                    field: predicate.field.as_deref(),
                    stale,
                },
                operator,
                expected,
            )
        });
        let matched = if comparison.is_some() {
            !stale
                && comparison_evidence
                    .as_ref()
                    .is_some_and(|evidence| evidence.result)
        } else {
            !stale && accepted.is_some()
        };
        let mut evaluation = condition_evaluation(
            &comparison.map_or_else(
                || {
                    slot_predicate_label(
                        slot_ref,
                        predicate.field.as_deref(),
                        predicate.equals.as_ref(),
                    )
                },
                |(operator, value)| {
                    comparison_slot_predicate_label(
                        slot_ref,
                        predicate.field.as_deref(),
                        operator,
                        value,
                    )
                },
            ),
            Some(slot_ref.to_string()),
            loop_context,
            matched,
            if stale {
                "accepted slot evidence is stale (written in an earlier iteration of this loop)"
            } else if matched {
                "accepted slot evidence matched"
            } else {
                "accepted slot evidence did not match"
            },
        );
        evaluation.comparison_evidence = comparison_evidence;
        return Ok((GuardOutcome::from_bool(matched), vec![evaluation]));
    }
    if let Some(output_ref) = predicate.output.as_deref() {
        let accepted = evidence.current_outputs.iter().find(|value| {
            value.ref_text == output_ref && value.acceptance == AcceptanceStatus::Accepted
        });
        let comparison = runtime_comparison_modifier(predicate);
        let comparison_evidence = comparison.map(|(operator, expected)| {
            condition_comparison_evidence(
                state,
                ComparisonLhs {
                    subject: ConditionComparisonSubject::Output,
                    accepted,
                    ref_text: output_ref,
                    field: predicate.field.as_deref(),
                    stale: false,
                },
                operator,
                expected,
            )
        });
        let matched = if comparison.is_some() {
            comparison_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.result)
        } else {
            accepted.is_some()
        };
        let mut evaluation = condition_evaluation(
            &comparison.map_or_else(
                || {
                    output_predicate_label(
                        output_ref,
                        predicate.field.as_deref(),
                        predicate.equals.as_ref(),
                    )
                },
                |(operator, value)| {
                    comparison_output_predicate_label(
                        output_ref,
                        predicate.field.as_deref(),
                        operator,
                        value,
                    )
                },
            ),
            Some(output_ref.to_string()),
            loop_context,
            matched,
            if matched {
                "current output evidence matched"
            } else {
                "current output evidence did not match"
            },
        );
        evaluation.comparison_evidence = comparison_evidence;
        return Ok((GuardOutcome::from_bool(matched), vec![evaluation]));
    }
    if !predicate.all.is_empty() {
        let mut all_evaluations = Vec::new();
        let mut outcome = GuardOutcome::Matched;
        for item in &predicate.all {
            let (item_outcome, evaluations) = evaluate_guard_expr_with_seen(
                trait_ref,
                state,
                item,
                loop_context,
                evidence,
                depth + 1,
                seen_conditions,
            )?;
            outcome = outcome.and(item_outcome);
            all_evaluations.extend(evaluations);
        }
        all_evaluations.push(condition_evaluation_outcome(
            "all[...]",
            Some(loop_evidence_ref(loop_context)),
            loop_context,
            outcome,
            match outcome {
                GuardOutcome::Matched => "all guards matched",
                GuardOutcome::Unmeasurable => "one or more guards are unmeasurable",
                GuardOutcome::NotMatched => "one or more guards did not match",
            },
        ));
        return Ok((outcome, all_evaluations));
    }
    if !predicate.any.is_empty() {
        let mut all_evaluations = Vec::new();
        let mut outcome = GuardOutcome::NotMatched;
        for item in &predicate.any {
            let (item_outcome, evaluations) = evaluate_guard_expr_with_seen(
                trait_ref,
                state,
                item,
                loop_context,
                evidence,
                depth + 1,
                seen_conditions,
            )?;
            outcome = outcome.or(item_outcome);
            all_evaluations.extend(evaluations);
        }
        all_evaluations.push(condition_evaluation_outcome(
            "any[...]",
            Some(loop_evidence_ref(loop_context)),
            loop_context,
            outcome,
            match outcome {
                GuardOutcome::Matched => "at least one guard matched",
                GuardOutcome::Unmeasurable => "no guard matched; at least one is unmeasurable",
                GuardOutcome::NotMatched => "no guard matched",
            },
        ));
        return Ok((outcome, all_evaluations));
    }
    Ok((
        GuardOutcome::NotMatched,
        vec![condition_evaluation(
            "empty-predicate",
            Some(loop_evidence_ref(loop_context)),
            loop_context,
            false,
            "predicate had no evaluable form",
        )],
    ))
}

/// Measure both sides of count comparisons through the same list/filter path.
fn measured_count(
    state: &State,
    slot_ref: &str,
    field: Option<&str>,
    field_equals: Option<&JsonValue>,
    evidence: &GuardEvidence<'_>,
) -> (Option<u64>, bool) {
    let accepted = accepted_value(state, slot_ref);
    let stale = accepted.is_some() && stale_repeated_slot(state, slot_ref, evidence.repeated_scope);
    let count = (!stale).then(|| accepted.and_then(|value| value.value.as_array())).flatten().map(|items| {
        items.iter().filter(|item| match (field, field_equals) {
            (Some(field), Some(expected)) => {
                crate::shared::resolve_field_path(item, field).is_some_and(|actual| actual == expected)
            }
            _ => true,
        }).count()
    }).and_then(|count| u64::try_from(count).ok());
    (count, stale)
}

fn count_threshold(state: &State, threshold: &JsonValue, evidence: &GuardEvidence<'_>) -> Option<u64> {
    if let Some(value) = threshold.as_u64() { return Some(value); }
    let operand = crate::r#trait::condition::parse_count_operand(threshold)?;
    measured_count(
        state,
        &operand.count,
        operand.field.as_ref().and_then(Option::as_deref),
        operand.field_equals.as_ref().and_then(Option::as_ref),
        evidence,
    ).0
}

fn condition_evaluation(
    predicate: &str,
    evidence_ref: Option<String>,
    loop_context: &LoopContext,
    matched: bool,
    reason: &str,
) -> ConditionEvaluation {
    condition_evaluation_outcome(
        predicate,
        evidence_ref,
        loop_context,
        GuardOutcome::from_bool(matched),
        reason,
    )
}

/// Like [`condition_evaluation`], but records the tri-state `outcome` too
/// (serialized only when `Unmeasurable`, per the byte-stability rule).
fn condition_evaluation_outcome(
    predicate: &str,
    evidence_ref: Option<String>,
    loop_context: &LoopContext,
    outcome: GuardOutcome,
    reason: &str,
) -> ConditionEvaluation {
    ConditionEvaluation {
        predicate: predicate.to_string(),
        evidence_ref: (!loop_context.loop_id.is_empty()).then_some(evidence_ref).flatten(),
        scope: (!loop_context.loop_id.is_empty()).then_some(ConditionEvaluationScope {
            loop_id: loop_context.loop_id.clone(),
            sequence_id: loop_context.sequence_id.clone(),
            iteration_index: loop_context.iteration_index,
            // `usize::MAX` is this frame's in-memory "no bound" sentinel
            // (0093) — recorded as absent rather than as a literal huge
            // number in the ledger's evidence.
            max_iterations: (loop_context.max_iterations != usize::MAX)
                .then_some(loop_context.max_iterations),
        }),
        comparison_evidence: None,
        outcome: matches!(outcome, GuardOutcome::Unmeasurable).then_some(outcome),
        matched: outcome.routes_true(),
        reason: reason.to_string(),
    }
}

/// Evaluate a `present` leaf. Evidence table:
/// - subject not supplied, bare form -> `NotMatched` (known-absent port).
/// - subject not supplied, `field` form -> `Unmeasurable` (container
///   unreadable, so field presence cannot be determined).
/// - subject supplied, `field` declared but omitted -> `NotMatched`.
/// - subject supplied, `field` present (including a schema-valid `null`) ->
///   `Matched`; bare form with any supplied value -> `Matched`.
/// - subject stale (written by this loop's body in an earlier iteration) ->
///   `Unmeasurable`, reusing [`stale_repeated_slot`]'s loop-freshness rule.
fn evaluate_present_predicate(
    state: &State,
    predicate: &crate::r#trait::condition::GuardPredicate,
    subject_ref: &str,
    loop_context: &LoopContext,
    evidence: &GuardEvidence<'_>,
) -> (GuardOutcome, Vec<ConditionEvaluation>) {
    let accepted = accepted_value(state, subject_ref);
    let stale = accepted.is_some() && stale_repeated_slot(state, subject_ref, evidence.repeated_scope);
    let field = predicate.field.as_deref();
    let (outcome, reason) = if stale {
        (
            GuardOutcome::Unmeasurable,
            "accepted evidence is stale (written in an earlier iteration of this loop); present is unmeasurable",
        )
    } else {
        match (accepted, field) {
            (None, None) => (GuardOutcome::NotMatched, "subject was not supplied"),
            (None, Some(_)) => (
                GuardOutcome::Unmeasurable,
                "container was not supplied; field presence is unmeasurable",
            ),
            (Some(_), None) => (GuardOutcome::Matched, "subject is present"),
            (Some(value), Some(field_name)) => {
                if crate::shared::resolve_field_path(&value.value, field_name).is_some() {
                    (GuardOutcome::Matched, "declared field is present")
                } else {
                    (GuardOutcome::NotMatched, "declared field is not present")
                }
            }
        }
    };
    let label = match field {
        Some(field_name) => format!("present({subject_ref}).{field_name}"),
        None => format!("present({subject_ref})"),
    };
    let evaluation = condition_evaluation_outcome(
        &label,
        Some(subject_ref.to_string()),
        loop_context,
        outcome,
        reason,
    );
    (outcome, vec![evaluation])
}

fn loop_evidence_ref(loop_context: &LoopContext) -> String {
    format!(
        "loop:{} iteration:{}",
        loop_context.loop_id, loop_context.iteration_index
    )
}

fn slot_predicate_label(slot_ref: &str, field: Option<&str>, equals: Option<&JsonValue>) -> String {
    match (field, equals) {
        (Some(field), Some(value)) => format!("{slot_ref}.{field} == {value}"),
        (None, Some(value)) => format!("{slot_ref} == {value}"),
        _ => format!("present({slot_ref})"),
    }
}

fn output_predicate_label(
    output_ref: &str,
    field: Option<&str>,
    equals: Option<&JsonValue>,
) -> String {
    match (field, equals) {
        (Some(field), Some(value)) => format!("output({output_ref}).{field} == {value}"),
        (None, Some(value)) => format!("output({output_ref}) == {value}"),
        _ => format!("output-present({output_ref})"),
    }
}

fn comparison_slot_predicate_label(
    slot_ref: &str,
    field: Option<&str>,
    operator: ConditionComparisonOperator,
    value: &JsonValue,
) -> String {
    let lhs = field.map_or_else(|| slot_ref.to_string(), |field| format!("{slot_ref}.{field}"));
    format!("{lhs} {} {value}", operator.symbol())
}

fn comparison_output_predicate_label(
    output_ref: &str,
    field: Option<&str>,
    operator: ConditionComparisonOperator,
    value: &JsonValue,
) -> String {
    let lhs = field.map_or_else(
        || format!("output({output_ref})"),
        |field| format!("output({output_ref}).{field}"),
    );
    format!("{lhs} {} {value}", operator.symbol())
}

fn slot_list_reason(stale: bool, present: bool, matched: bool) -> &'static str {
    if stale {
        "accepted slot evidence is stale (written in an earlier iteration of this loop)"
    } else if matched {
        "accepted list slot evidence matched"
    } else if present {
        "accepted list slot evidence did not match"
    } else {
        "accepted list slot evidence is missing"
    }
}

fn runtime_comparison_modifier(
    predicate: &crate::r#trait::condition::GuardPredicate,
) -> Option<(ConditionComparisonOperator, &JsonValue)> {
    predicate
        .equals
        .as_ref()
        .map(|value| (ConditionComparisonOperator::Equals, value))
        .or_else(|| {
            predicate
                .less_than
                .as_ref()
                .map(|value| (ConditionComparisonOperator::LessThan, value))
        })
        .or_else(|| {
            predicate
                .at_most
                .as_ref()
                .map(|value| (ConditionComparisonOperator::AtMost, value))
        })
        .or_else(|| {
            predicate
                .greater_than
                .as_ref()
                .map(|value| (ConditionComparisonOperator::GreaterThan, value))
        })
        .or_else(|| {
            predicate
                .at_least
                .as_ref()
                .map(|value| (ConditionComparisonOperator::AtLeast, value))
        })
}

struct ComparisonLhs<'a> {
    subject: ConditionComparisonSubject,
    accepted: Option<&'a Value>,
    ref_text: &'a str,
    field: Option<&'a str>,
    stale: bool,
}

fn condition_comparison_evidence(
    state: &State,
    lhs_source: ComparisonLhs<'_>,
    operator: ConditionComparisonOperator,
    expected: &JsonValue,
) -> ConditionComparisonEvidence {
    let lhs = comparison_ref_operand(
        state,
        lhs_source.accepted,
        lhs_source.ref_text,
        lhs_source.field,
        lhs_source.subject == ConditionComparisonSubject::Output,
    );
    let rhs = if operator == ConditionComparisonOperator::Equals {
        ComparisonOperandEvidence::Literal {
            value: expected.clone(),
        }
    } else if let Some(expected_ref) = crate::r#trait::condition::numeric_comparison_ref(expected) {
        comparison_ref_operand(
            state,
            accepted_value(state, expected_ref),
            expected_ref,
            None,
            false,
        )
    } else {
        ComparisonOperandEvidence::Literal {
            value: expected.clone(),
        }
    };
    let result = comparison_result(operator, &lhs, &rhs, lhs_source.subject);
    ConditionComparisonEvidence {
        subject: lhs_source.subject,
        lhs,
        operator,
        rhs,
        result,
        stale: lhs_source.stale,
    }
}

fn comparison_ref_operand(
    state: &State,
    accepted: Option<&Value>,
    ref_text: &str,
    field: Option<&str>,
    embed_source_value: bool,
) -> ComparisonOperandEvidence {
    let Some(accepted) = accepted else {
        return ComparisonOperandEvidence::MissingRef {
            ref_text: ref_text.to_string(),
            field: field.map(str::to_string),
        };
    };
    let selected_value = match field {
        Some(field) => crate::shared::resolve_field_path(&accepted.value, field).cloned(),
        None => Some(accepted.value.clone()),
    };
    let slot_revision_acceptance_order = Reference::parse(ref_text)
        .ok()
        .filter(|reference| reference.kind() == Kind::Slot)
        .and_then(|_| {
            visible_slot_revisions(state)
                .into_iter()
                .rev()
                .find(|revision| {
                    revision.slot_ref.as_str() == ref_text
                        && revision.value_digest == accepted.value_digest
                })
                .map(|revision| revision.acceptance_order)
        });
    ComparisonOperandEvidence::Ref {
        ref_text: ref_text.to_string(),
        source_value_digest: accepted.value_digest.clone(),
        source_value: (embed_source_value
            || Reference::parse(ref_text).is_ok_and(|reference| reference.kind() == Kind::Schema))
        .then(|| accepted.value.clone()),
        field: field.map(str::to_string),
        selected_value,
        slot_revision_acceptance_order,
    }
}

fn comparison_result(
    operator: ConditionComparisonOperator,
    lhs: &ComparisonOperandEvidence,
    rhs: &ComparisonOperandEvidence,
    subject: ConditionComparisonSubject,
) -> bool {
    let (Some(lhs), Some(rhs)) = (operand_selected_value(lhs), operand_selected_value(rhs)) else {
        return false;
    };
    if operator == ConditionComparisonOperator::Equals {
        return lhs == rhs;
    }
    // An elapsed-seconds threshold that resolves to a negative runtime value
    // can never be a legitimate soft-budget bound (see the caller in
    // `condition_comparison_evidence` for the elapsed guard, and
    // `validate_comparison_guard_evidence` in ledger_contract.rs, which both
    // route through this one function so evaluation and replay always agree).
    if subject == ConditionComparisonSubject::Elapsed && rhs.as_f64().is_some_and(f64::is_sign_negative) {
        return false;
    }
    lhs.as_number()
        .zip(rhs.as_number())
        .is_some_and(|(lhs, rhs)| operator.matches_ordering(compare_json_numbers(lhs, rhs)))
}

fn operand_selected_value(operand: &ComparisonOperandEvidence) -> Option<&JsonValue> {
    match operand {
        ComparisonOperandEvidence::Ref { selected_value, .. } => selected_value.as_ref(),
        ComparisonOperandEvidence::Literal { value } => Some(value),
        ComparisonOperandEvidence::MissingRef { .. } => None,
    }
}

#[derive(Clone, Copy)]
enum JsonNumber {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}

fn compare_json_numbers(
    left: &serde_json::Number,
    right: &serde_json::Number,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (json_number(left), json_number(right)) {
        (JsonNumber::Signed(left), JsonNumber::Signed(right)) => left.cmp(&right),
        (JsonNumber::Unsigned(left), JsonNumber::Unsigned(right)) => left.cmp(&right),
        (JsonNumber::Signed(left), JsonNumber::Unsigned(right)) => {
            if left < 0 {
                Ordering::Less
            } else {
                (left as u64).cmp(&right)
            }
        }
        (JsonNumber::Unsigned(left), JsonNumber::Signed(right)) => {
            if right < 0 {
                Ordering::Greater
            } else {
                left.cmp(&(right as u64))
            }
        }
        (JsonNumber::Signed(left), JsonNumber::Float(right)) => compare_i64_float(left, right),
        (JsonNumber::Float(left), JsonNumber::Signed(right)) => compare_i64_float(right, left).reverse(),
        (JsonNumber::Unsigned(left), JsonNumber::Float(right)) => compare_u64_float(left, right),
        (JsonNumber::Float(left), JsonNumber::Unsigned(right)) => compare_u64_float(right, left).reverse(),
        (JsonNumber::Float(left), JsonNumber::Float(right)) => left
            .partial_cmp(&right)
            .expect("serde_json numbers are finite"),
    }
}

fn json_number(number: &serde_json::Number) -> JsonNumber {
    if let Some(value) = number.as_i64() {
        JsonNumber::Signed(value)
    } else if let Some(value) = number.as_u64() {
        JsonNumber::Unsigned(value)
    } else {
        JsonNumber::Float(number.as_f64().expect("serde_json numbers are finite"))
    }
}

fn compare_i64_float(integer: i64, float: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    const I64_UPPER_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0;
    if float < i64::MIN as f64 {
        return Ordering::Greater;
    }
    if float >= I64_UPPER_EXCLUSIVE {
        return Ordering::Less;
    }
    compare_integer_and_in_range_float(integer.cmp(&(float.trunc() as i64)), integer as f64, float)
}

fn compare_u64_float(integer: u64, float: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    const U64_UPPER_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;
    if float < 0.0 {
        return Ordering::Greater;
    }
    if float >= U64_UPPER_EXCLUSIVE {
        return Ordering::Less;
    }
    compare_integer_and_in_range_float(integer.cmp(&(float.trunc() as u64)), integer as f64, float)
}

fn compare_integer_and_in_range_float(
    integer_ordering: std::cmp::Ordering,
    integer_as_float: f64,
    float: f64,
) -> std::cmp::Ordering {
    if integer_ordering.is_eq() {
        integer_as_float
            .partial_cmp(&float)
            .expect("serde_json numbers are finite")
    } else {
        integer_ordering
    }
}

fn signal_matched_in_scope(
    state: &State,
    signal_ref: &str,
    repeated_scope: &[RepeatedActivation],
) -> bool {
    visible_emitted_signals(state).into_iter().any(|signal| {
        let signal_scope = repeated_activation_scope(&signal.position_path);
        signal.acceptance == AcceptanceStatus::Accepted
            && signal.signal_ref.as_str() == signal_ref
            && signal_scope
                .iter()
                .zip(repeated_scope)
                .all(|(emitted, current)| {
                    same_repeated_control(emitted, current)
                        && emitted.iteration == current.iteration
                        && emitted.item_index == current.item_index
                })
    })
}

/// Emitted-signal evidence visible from the current control position: the
/// committed ledger plus, for every active `parallel` frame on the stack
/// (outer to inner), that branch's own not-yet-merged buffer — never a
/// sibling or completed branch's buffer, which stays hidden until the
/// barrier.
fn visible_emitted_signals(state: &State) -> Vec<&SignalEmission> {
    let mut all: Vec<&SignalEmission> = state.emitted_signals.iter().collect();
    for frame in &state.control_stack {
        if frame.kind == ControlKind::Parallel {
            all.extend(frame.parallel_buffer.emitted_signals.iter());
        }
    }
    all
}

/// Slot-revision evidence visible from the current control position, mirroring
/// [`visible_emitted_signals`] for slot writes.
fn visible_slot_revisions(state: &State) -> Vec<&SlotRevision> {
    let mut all: Vec<&SlotRevision> = state.slot_revisions.iter().collect();
    for frame in &state.control_stack {
        if frame.kind == ControlKind::Parallel {
            all.extend(frame.parallel_buffer.slot_revisions.iter());
        }
    }
    all
}

fn latest_recorded_slot_revision_order(state: &State) -> usize {
    recorded_slot_revisions(state)
        .into_iter()
        .map(|revision| revision.acceptance_order)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod present_tests {
    use super::*;

    fn accepted_value_evidence(ref_text: &str, value: JsonValue) -> Value {
        Value {
            ref_text: ref_text.to_string(),
            value_digest: crate::digest::canonical_digest(&value).expect("digest"),
            value,
            schema_ref: None,
            source: ValueSource::HostInput,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
            producer_check_verdict: false,
            acceptance: AcceptanceStatus::Accepted,
            schema_validation: Vec::new(),
        }
    }

    fn state_with_port_values(values: Vec<Value>) -> State {
        State {
            run_id: Id::new("run-present-test").expect("id"),
            trait_id: "present-test".to_string(),
            strict_loops: false,
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            sequence_statuses: Vec::new(),
            accepted_port_values: values,
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
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
            guard_evaluations: Vec::new(),
            parallel_panel_records: Vec::new(),
            stop_reason: None,
            elapsed_seconds: 0,
            final_state: FinalState::Running,
        }
    }

    fn empty_loop_context() -> LoopContext {
        LoopContext {
            loop_id: String::new(),
            sequence_id: None,
            iteration_index: 0,
            max_iterations: 1,
        }
    }

    fn bare_predicate(subject_ref: &str) -> crate::r#trait::condition::GuardPredicate {
        crate::r#trait::condition::GuardPredicate {
            present: Some(subject_ref.to_string()),
            ..Default::default()
        }
    }

    fn field_predicate(
        subject_ref: &str,
        field: &str,
    ) -> crate::r#trait::condition::GuardPredicate {
        crate::r#trait::condition::GuardPredicate {
            present: Some(subject_ref.to_string()),
            field: Some(field.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn absent_port_is_not_matched() {
        let state = state_with_port_values(Vec::new());
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let (outcome, _) = evaluate_present_predicate(
            &state,
            &bare_predicate("port:cap"),
            "port:cap",
            &empty_loop_context(),
            &evidence,
        );
        assert_eq!(outcome, GuardOutcome::NotMatched);
        assert!(!outcome.routes_true());
    }

    #[test]
    fn supplied_port_is_matched() {
        let state = state_with_port_values(vec![accepted_value_evidence(
            "port:cap",
            JsonValue::from(5),
        )]);
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let (outcome, _) = evaluate_present_predicate(
            &state,
            &bare_predicate("port:cap"),
            "port:cap",
            &empty_loop_context(),
            &evidence,
        );
        assert_eq!(outcome, GuardOutcome::Matched);
    }

    #[test]
    fn absent_field_is_not_matched() {
        let state = state_with_port_values(vec![accepted_value_evidence(
            "port:cap",
            serde_json::json!({"other": 1}),
        )]);
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let (outcome, _) = evaluate_present_predicate(
            &state,
            &field_predicate("port:cap", "cost-report"),
            "port:cap",
            &empty_loop_context(),
            &evidence,
        );
        assert_eq!(outcome, GuardOutcome::NotMatched);
    }

    #[test]
    fn supplied_but_container_never_accepted_is_unmeasurable() {
        let state = state_with_port_values(Vec::new());
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let (outcome, _) = evaluate_present_predicate(
            &state,
            &field_predicate("port:cap", "cost-report"),
            "port:cap",
            &empty_loop_context(),
            &evidence,
        );
        assert_eq!(outcome, GuardOutcome::Unmeasurable);
        assert!(!outcome.routes_true());
    }

    #[test]
    fn field_present_including_null_is_matched() {
        let state = state_with_port_values(vec![accepted_value_evidence(
            "port:cap",
            serde_json::json!({"cost-report": JsonValue::Null}),
        )]);
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let (outcome, _) = evaluate_present_predicate(
            &state,
            &field_predicate("port:cap", "cost-report"),
            "port:cap",
            &empty_loop_context(),
            &evidence,
        );
        assert_eq!(outcome, GuardOutcome::Matched);
    }

    #[test]
    fn not_of_unmeasurable_stays_unmeasurable_and_routes_false() {
        // The regression this phase exists to close: a bool-only `not` would
        // turn `not(present(unmeasurable))` into `true`.
        let state = state_with_port_values(Vec::new());
        let guard = GuardExpr::Predicate(Box::new(crate::r#trait::condition::GuardPredicate {
            not: Some(Box::new(GuardExpr::Predicate(Box::new(field_predicate(
                "port:cap",
                "cost-report",
            ))))),
            ..Default::default()
        }));
        let evidence = GuardEvidence {
            repeated_scope: &[],
            current_outputs: &[],
        };
        let trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            "id = \"present-test\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Present test\"\nsummary = \"Minimal fixture.\"\n",
        )
        .expect("minimal trait decodes");
        let (outcome, _) = evaluate_guard_expr_with_seen(
            &trait_ref,
            &state,
            &guard,
            &empty_loop_context(),
            &evidence,
            0,
            &mut BTreeSet::new(),
        )
        .expect("guard evaluates");
        assert_eq!(outcome, GuardOutcome::Unmeasurable);
        assert!(!outcome.routes_true());
    }
}

/// Proves the phase's `p434-single-keep-guard-expressible` Done-when
/// clause: an optional-cost-cap keep guard shaped exactly like P434's real
/// `max-cost-microusd` port and `self-improving-traits-result.cost-microusd`
/// field — `.ctx/traits/auto-research` itself is never touched — decodes
/// and validates as a schema-version `"0.3"` document, then composes to the
/// three reachable outcomes of the fail-closed cap pattern
/// describes.
#[cfg(test)]
mod p434_keep_guard_tests {
    use super::*;

    /// `any([absent(cap), all([present(cap), present(report).field,
    /// fieldLte(report.field, cap)])])` against declared ports/schema/slot
    /// mirroring `.ctx/traits/auto-research`'s real `max-cost-microusd`
    /// port and `self-improving-traits-result.cost-microusd` field.
    const KEEP_GUARD_TRAIT_JSON: &str = r#"{
        "id": "p434-keep-guard-fixture",
        "schema-version": "0.3",
        "version": "0.1.0",
        "name": "P434 keep guard fixture",
        "summary": "Proves P434's optional-cost-cap keep guard is expressible without modifying auto-research.",
        "port": [
            {
                "id": "max-cost-microusd",
                "direction": "input",
                "schema": "schema:integer",
                "optional": true,
                "description": "Optional owner-scoped cost cap; keep guard passes automatically when omitted."
            }
        ],
        "schema": [
            {
                "id": "self-improving-traits-result",
                "fields": {
                    "cost-microusd": { "schema": "schema:integer", "required": false }
                }
            }
        ],
        "slot": [
            { "id": "evaluator-result", "schema": "schema:self-improving-traits-result" }
        ],
        "condition": {
            "keep": {
                "any": [
                    { "not": { "present": "port:max-cost-microusd" } },
                    { "all": [
                        { "present": "port:max-cost-microusd" },
                        { "present": "slot:evaluator-result", "field": "cost-microusd" },
                        {
                            "slot": "slot:evaluator-result",
                            "field": "cost-microusd",
                            "at-most": { "ref": "port:max-cost-microusd" }
                        }
                    ] }
                ]
            }
        }
    }"#;

    fn accepted_port(ref_text: &str, value: JsonValue) -> Value {
        Value {
            ref_text: ref_text.to_string(),
            value_digest: crate::digest::canonical_digest(&value).expect("digest"),
            value,
            schema_ref: None,
            source: ValueSource::HostInput,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
            producer_check_verdict: false,
            acceptance: AcceptanceStatus::Accepted,
            schema_validation: Vec::new(),
        }
    }

    fn accepted_slot(ref_text: &str, value: JsonValue) -> Value {
        accepted_port(ref_text, value)
    }

    fn state(ports: Vec<Value>, slots: Vec<Value>) -> State {
        State {
            run_id: Id::new("run-p434-keep-guard-test").expect("id"),
            trait_id: "p434-keep-guard-fixture".to_string(),
            strict_loops: false,
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            sequence_statuses: Vec::new(),
            accepted_port_values: ports,
            accepted_slot_values: slots,
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
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
            guard_evaluations: Vec::new(),
            parallel_panel_records: Vec::new(),
            stop_reason: None,
            elapsed_seconds: 0,
            final_state: FinalState::Running,
        }
    }

    fn evaluate_keep_guard(trait_ref: &crate::r#trait::Trait, run_state: &State) -> GuardOutcome {
        let guard = trait_ref
            .conditions
            .get("keep")
            .expect("keep condition declared")
            .as_guard();
        let evidence = GuardEvidence { repeated_scope: &[], current_outputs: &[] };
        let (outcome, _) = evaluate_guard_expr_with_seen(
            trait_ref,
            run_state,
            &guard,
            &LoopContext { loop_id: String::new(), sequence_id: None, iteration_index: 0, max_iterations: 1 },
            &evidence,
            0,
            &mut BTreeSet::new(),
        )
        .expect("keep guard evaluates");
        outcome
    }

    #[test]
    fn keep_guard_fixture_decodes_and_validates_under_schema_version_0_3() {
        crate::encoding::decode_trait(crate::encoding::Encoding::Json, KEEP_GUARD_TRAIT_JSON)
            .expect("P434-shaped keep guard validates under schema-version 0.3");
    }

    #[test]
    fn keep_guard_routes_true_when_cap_is_omitted() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, KEEP_GUARD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(Vec::new(), Vec::new());
        let outcome = evaluate_keep_guard(&trait_ref, &run_state);
        assert_eq!(outcome, GuardOutcome::Matched);
        assert!(outcome.routes_true());
    }

    #[test]
    fn keep_guard_routes_true_when_cap_is_supplied_and_report_is_within_cap() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, KEEP_GUARD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(
            vec![accepted_port("port:max-cost-microusd", JsonValue::from(5))],
            vec![accepted_slot(
                "slot:evaluator-result",
                serde_json::json!({ "cost-microusd": 3 }),
            )],
        );
        let outcome = evaluate_keep_guard(&trait_ref, &run_state);
        assert_eq!(outcome, GuardOutcome::Matched);
        assert!(outcome.routes_true());
    }

    #[test]
    fn keep_guard_routes_false_and_tags_the_inner_present_leaf_unmeasurable_when_report_is_unmeasurable()
     {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, KEEP_GUARD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(vec![accepted_port("port:max-cost-microusd", JsonValue::from(5))], Vec::new());
        let guard = trait_ref
            .conditions
            .get("keep")
            .expect("keep condition declared")
            .as_guard();
        let evidence = GuardEvidence { repeated_scope: &[], current_outputs: &[] };
        let (outcome, evaluations) = evaluate_guard_expr_with_seen(
            &trait_ref,
            &run_state,
            &guard,
            &LoopContext { loop_id: String::new(), sequence_id: None, iteration_index: 0, max_iterations: 1 },
            &evidence,
            0,
            &mut BTreeSet::new(),
        )
        .expect("keep guard evaluates");
        assert_eq!(outcome, GuardOutcome::NotMatched);
        assert!(!outcome.routes_true());
        let inner_present = evaluations
            .iter()
            .find(|evaluation| evaluation.predicate == "present(slot:evaluator-result).cost-microusd")
            .expect("inner present leaf evidence recorded");
        assert_eq!(inner_present.outcome, Some(GuardOutcome::Unmeasurable));
    }
}

/// Proves task 0085's "condition.equals over a three-level path" and
/// "missing intermediate evaluates false, never errors" Done-when clauses,
/// against a fixture mirroring plannotator's real
/// `hookSpecificOutput.decision.behavior` shape.
#[cfg(test)]
mod nested_field_path_tests {
    use super::*;

    const NESTED_FIELD_TRAIT_JSON: &str = r#"{
        "id": "nested-field-path-fixture",
        "schema-version": "0.3",
        "version": "0.1.0",
        "name": "Nested field path fixture",
        "summary": "Proves a three-level field path typechecks and evaluates.",
        "schema": [
            {
                "id": "decision",
                "fields": {
                    "behavior": { "schema": "schema:text", "required": false }
                }
            },
            {
                "id": "hook-specific-output",
                "fields": {
                    "decision": { "schema": "schema:decision", "required": false }
                }
            }
        ],
        "slot": [
            { "id": "hook-output", "schema": "schema:hook-specific-output" }
        ],
        "condition": {
            "approved": {
                "slot": "slot:hook-output",
                "field": "decision.behavior",
                "equals": "approve"
            }
        }
    }"#;

    fn accepted_slot(ref_text: &str, value: JsonValue) -> Value {
        Value {
            ref_text: ref_text.to_string(),
            value_digest: crate::digest::canonical_digest(&value).expect("digest"),
            value,
            schema_ref: None,
            source: ValueSource::HostInput,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
            producer_check_verdict: false,
            acceptance: AcceptanceStatus::Accepted,
            schema_validation: Vec::new(),
        }
    }

    fn state(slots: Vec<Value>) -> State {
        State {
            run_id: Id::new("run-nested-field-path-test").expect("id"),
            trait_id: "nested-field-path-fixture".to_string(),
            strict_loops: false,
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            sequence_statuses: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: slots,
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
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
            guard_evaluations: Vec::new(),
            parallel_panel_records: Vec::new(),
            stop_reason: None,
            elapsed_seconds: 0,
            final_state: FinalState::Running,
        }
    }

    fn evaluate_approved_guard(trait_ref: &crate::r#trait::Trait, run_state: &State) -> GuardOutcome {
        let guard = trait_ref
            .conditions
            .get("approved")
            .expect("approved condition declared")
            .as_guard();
        let evidence = GuardEvidence { repeated_scope: &[], current_outputs: &[] };
        let (outcome, _) = evaluate_guard_expr_with_seen(
            trait_ref,
            run_state,
            &guard,
            &LoopContext { loop_id: String::new(), sequence_id: None, iteration_index: 0, max_iterations: 1 },
            &evidence,
            0,
            &mut BTreeSet::new(),
        )
        .expect("approved guard evaluates");
        outcome
    }

    #[test]
    fn fixture_decodes_and_validates() {
        crate::encoding::decode_trait(crate::encoding::Encoding::Json, NESTED_FIELD_TRAIT_JSON)
            .expect("nested field path fixture validates");
    }

    #[test]
    fn three_level_path_matches_when_value_is_nested() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, NESTED_FIELD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(vec![accepted_slot(
            "slot:hook-output",
            serde_json::json!({ "decision": { "behavior": "approve" } }),
        )]);
        let outcome = evaluate_approved_guard(&trait_ref, &run_state);
        assert_eq!(outcome, GuardOutcome::Matched);
        assert!(outcome.routes_true());
    }

    #[test]
    fn three_level_path_does_not_match_a_different_value() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, NESTED_FIELD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(vec![accepted_slot(
            "slot:hook-output",
            serde_json::json!({ "decision": { "behavior": "deny" } }),
        )]);
        let outcome = evaluate_approved_guard(&trait_ref, &run_state);
        assert_eq!(outcome, GuardOutcome::NotMatched);
        assert!(!outcome.routes_true());
    }

    #[test]
    fn missing_intermediate_evaluates_false_without_erroring() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Json, NESTED_FIELD_TRAIT_JSON)
                .expect("fixture decodes");
        let run_state = state(vec![accepted_slot(
            "slot:hook-output",
            serde_json::json!({}),
        )]);
        let outcome = evaluate_approved_guard(&trait_ref, &run_state);
        assert_eq!(outcome, GuardOutcome::NotMatched);
        assert!(!outcome.routes_true());
    }
}
