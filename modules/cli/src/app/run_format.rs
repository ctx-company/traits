//! CLI-only formatting for run/session surfaces.

use crate::app::presentation::wire_name;

pub(crate) fn print_run_info(summary: &ctx_traits_core::run_info::RunInfoSummary) {
    println!("ctx traits run-info");
    println!("  trait: {}", summary.trait_identity.trait_id);
    println!(
        "  source: {}",
        summary
            .trait_identity
            .source_path
            .as_deref()
            .unwrap_or("unresolved")
    );
    println!("  selection: {}", wire_name(&summary.selection.status));
    println!(
        "  lifecycle: status={} trust={} runnable={}",
        summary.lifecycle.status, summary.lifecycle.trust, summary.lifecycle.runnable
    );
    if summary.lifecycle.gates.is_empty() {
        println!("  gates: pass");
    } else {
        println!("  gates:");
        for gate in &summary.lifecycle.gates {
            println!("    {}: {}", gate.code, gate.message);
        }
    }
    println!("  input-ports:");
    if summary.input_ports.is_empty() {
        println!("    none");
    }
    for port in &summary.input_ports {
        println!(
            "    {} arg={} schema={} required={} submission={}",
            port.port, port.argument, port.schema, port.required, port.submission
        );
    }
    println!("  output-ports:");
    if summary.output_ports.is_empty() {
        println!("    none");
    }
    for port in &summary.output_ports {
        println!(
            "    {} schema={} required={} value-slot={}",
            port.port,
            port.schema,
            port.required,
            port.value_slot.as_deref().unwrap_or("direct")
        );
    }
    println!("  dispatch-reminders:");
    if summary.dispatch_reminders.is_empty() {
        println!("    none");
    }
    for row in &summary.dispatch_reminders {
        let seat = match (row.seat_index, row.list_length) {
            (Some(seat_index), Some(list_length)) => {
                format!(" seat={seat_index}/{list_length}")
            }
            _ => String::new(),
        };
        println!(
            "    agent:{} assigned={} harness={} transport={} session-mode={}{seat}",
            row.role,
            row.assigned,
            row.harness.as_deref().unwrap_or("unassigned"),
            row.transport.as_deref().unwrap_or("unassigned"),
            row.session_mode.as_deref().unwrap_or("unassigned")
        );
    }
    println!("  command-reminders:");
    if summary.command_reminders.is_empty() {
        println!("    none");
    }
    for row in &summary.command_reminders {
        println!("    {} command={}", row.declaration_path, row.command);
    }
    println!("  start-examples:");
    for example in &summary.start_examples {
        println!("    {example}");
    }
}

pub(crate) fn print_run_selection(
    header: &str,
    selection: &ctx_traits_core::run_info::RunInfoSelectionSummary,
) {
    println!("{header}");
    println!("  selection: {}", wire_name(&selection.status));
    if let Some(query) = &selection.query {
        println!("  query: {query}");
    }
    if selection.reasons.is_empty() {
        println!("  reasons: none");
    } else {
        println!("  reasons:");
        for reason in &selection.reasons {
            println!("    {reason}");
        }
    }
    if selection.candidates.is_empty() {
        println!("  candidates: none");
    } else {
        println!("  candidates:");
        for candidate in &selection.candidates {
            println!(
                "    {} score={} rank-tier={} gates={}",
                candidate.trait_id,
                candidate.score,
                candidate.rank_tier,
                if candidate.gates.is_empty() {
                    "pass"
                } else {
                    "blocked"
                }
            );
            for gate in &candidate.gates {
                println!("      {}: {}", gate.code, gate.message);
            }
        }
    }
}

pub(crate) fn print_run_session(
    session: &ctx_traits_core::procedure::session::Session,
    out: Option<&str>,
) {
    println!("ctx traits run");
    println!("  status: {}", wire_name(&session.status));
    println!("  session-id: {}", session.session_id.as_str());
    println!("  run-id: {}", session.run_id.as_str());
    println!("  trait: {}", session.trait_id);
    if let Some(trait_source) = &session.provenance.trait_source {
        println!("  origin: {} ({})", trait_source.kind, trait_source.path);
    }
    println!("  state-digest: {}", session.state_digest);
    if let Some(pin) = &session.provenance.trust_approval {
        println!(
            "  trust-pin: digest={} seq={} approved-at={}",
            pin.canonical_digest.as_str(),
            pin.seq,
            pin.approved_at.as_deref().unwrap_or("unknown")
        );
    }
    println!("  ledger: {}", out.unwrap_or("not written"));
    match (
        session.current_run_index,
        session.current_source_index,
        session.current_sequence_title.as_deref(),
    ) {
        (run_index, Some(source_index), Some(title)) => {
            println!("  current-step: run {run_index} / source {source_index}: {title}");
        }
        (run_index, _, Some(title)) => println!("  current-step: run {run_index}: {title}"),
        _ => println!("  current-step: none"),
    }
    if let Some(agent) = &session.current_agent {
        println!(
            "  current-agent: {} ({})",
            agent.ref_text, agent.description
        );
    }
    if let Some(assignments) = &session.provenance.agent_assignments {
        println!("  agent-assignments: {}", assignments.len());
        for assignment in assignments {
            println!(
                "    agent:{} harness={} transport={} evidence={}",
                assignment.role, assignment.harness, assignment.transport, assignment.evidence
            );
        }
    }
    if !session.provenance.harness_probes.is_empty() {
        println!(
            "  harness-probes: {}",
            session.provenance.harness_probes.len()
        );
        for probe in &session.provenance.harness_probes {
            println!(
                "    {} bin={} version={}",
                probe.harness_id, probe.bin, probe.version
            );
        }
    }
    if !session.provenance.out_of_tree_mutations.is_empty() {
        println!(
            "  out-of-tree-mutations: {}",
            session.provenance.out_of_tree_mutations.len()
        );
        for finding in &session.provenance.out_of_tree_mutations {
            println!(
                "    frame={} policy={} paths={}",
                finding.frame,
                finding.policy,
                finding.paths.join(", ")
            );
        }
    }
    if !session.warnings.is_empty() {
        println!("  warnings:");
        for warning in &session.warnings {
            println!("    {warning}");
        }
    }
    if !session.active_path.is_empty() {
        println!(
            "  active-path: {}",
            render_runtime_path(&session.active_path)
        );
    }
    if !session.control_stack.is_empty() {
        println!("  control-stack: {}", session.control_stack.len());
        for frame in &session.control_stack {
            print_control_frame("    ", frame);
        }
    }
    print_branch_decisions("  ", &session.ledger.branch_decisions);
    print_failure_routes("  ", &session.ledger.failure_routes);
    print_parallel_panel_records("  ", &session.ledger.parallel_panel_records);
    print_stop_reason(
        "  ",
        session.stop_reason.as_ref(),
        &session.ledger.guard_evaluations,
    );
    if session
        .stop_reason
        .as_ref()
        .is_some_and(|stop_reason| stop_reason.reason == "stop-if-matched")
    {
        print_escalation_blockers("  ", &session.accepted_slot_values);
    }
    if session.unresolved_inputs.is_empty() {
        println!("  required-inputs: none missing");
    } else {
        println!(
            "  required-inputs: {}",
            session.unresolved_inputs.join(", ")
        );
    }
    println!("  accepted-ports: {}", session.accepted_port_values.len());
    for value in &session.accepted_port_values {
        println!(
            "    {} {}{}",
            value.ref_text,
            value.value_digest,
            producer_suffix(
                value.producer_agent.as_deref(),
                value.producer_harness.as_deref(),
            )
        );
    }
    println!("  accepted-slots: {}", session.accepted_slot_values.len());
    for value in &session.accepted_slot_values {
        println!(
            "    {} {}{}",
            value.ref_text,
            value.value_digest,
            producer_suffix(
                value.producer_agent.as_deref(),
                value.producer_harness.as_deref(),
            )
        );
    }
    println!(
        "  accepted-output-ports: {}",
        session.accepted_output_port_values.len()
    );
    for value in &session.accepted_output_port_values {
        println!(
            "    {} {}{}",
            value.ref_text,
            value.value_digest,
            producer_suffix(
                value.producer_agent.as_deref(),
                value.producer_harness.as_deref(),
            )
        );
    }
    if !session.slot_revisions.is_empty() {
        println!("  slot-revisions: {}", session.slot_revisions.len());
        for revision in &session.slot_revisions {
            println!(
                "    #{} {} {} at {}",
                revision.acceptance_order,
                revision.slot_ref,
                revision.value_digest,
                render_runtime_path(&revision.position_path)
            );
        }
    }
    println!("  emitted-signals: {}", session.emitted_signals.len());
    for signal in &session.emitted_signals {
        println!(
            "    {} {}: {}{}{}",
            signal.signal_ref,
            wire_name(&signal.acceptance),
            signal.reason,
            producer_suffix(
                signal.producer_agent.as_deref(),
                signal.producer_harness.as_deref(),
            ),
            if signal.position_path.is_empty() {
                String::new()
            } else {
                format!(" at {}", render_runtime_path(&signal.position_path))
            }
        );
    }
    println!(
        "  rejected-submissions: {}",
        session.rejected_submissions.len()
    );
    for rejected in &session.rejected_submissions {
        println!(
            "    sequence {} {}: {}{}",
            rejected.sequence_index,
            rejected.ref_text.as_deref().unwrap_or("envelope"),
            rejected.reason,
            if rejected.position_path.is_empty() {
                String::new()
            } else {
                format!(" at {}", render_runtime_path(&rejected.position_path))
            }
        );
    }
    println!(
        "  capabilities: {}",
        session.provider_capability_reports.len()
    );
    for capability in &session.provider_capability_reports {
        println!(
            "    {} supported={} {}",
            capability.capability,
            capability.supported,
            capability.reason.as_deref().unwrap_or("")
        );
    }
    println!("  output-ports: {}", session.output_ports.len());
    for output in &session.output_ports {
        println!(
            "    {} {}: {}",
            output.port_ref,
            format_output_port_status(&output.status),
            output.reason
        );
    }
    if let Some(completion) = &session.completion {
        println!("  completion: {}", completion.event_code);
        for output in &completion.final_outputs {
            println!("    {} {}", output.port_ref, output.value_digest);
        }
    }
    if let Some(frame) = session.next_frame.as_deref() {
        println!("  next-frame:");
        print_sequence_frame("    ", frame);
    } else {
        println!("  next-frame: none");
    }
    println!("  next-action: {}", next_action_hint(session, out));
}

pub(crate) fn print_call_response(
    response: &ctx_traits_core::procedure::session::CallResponse,
    out: Option<&str>,
) {
    println!("ctx traits call");
    println!("  status: {}", wire_name(&response.status));
    println!("  response: {}", wire_name(&response.response_kind));
    println!("  session-id: {}", response.session_id.as_str());
    println!("  run-id: {}", response.run_id.as_str());
    println!("  state-digest: {}", response.updated_session_digest);
    println!("  ledger: {}", out.unwrap_or("not written"));
    println!("  accepted-slots: {}", response.accepted_slot_values.len());
    for value in &response.accepted_slot_values {
        println!(
            "    {} {}{}",
            value.ref_text,
            value.value_digest,
            producer_suffix(
                value.producer_agent.as_deref(),
                value.producer_harness.as_deref(),
            )
        );
    }
    println!("  rejected-slots: {}", response.rejected_slot_values.len());
    for rejected in &response.rejected_slot_values {
        println!(
            "    sequence {} {}: {}",
            rejected.sequence_index,
            rejected.ref_text.as_deref().unwrap_or("envelope"),
            rejected.reason
        );
    }
    println!("  accepted-signals: {}", response.accepted_signals.len());
    println!("  rejected-signals: {}", response.rejected_signals.len());
    if let Some(correction) = &response.correction {
        println!("  correction: {correction}");
    }
    if let Some(completion) = &response.completion {
        println!("  completion: {}", completion.event_code);
        for output in &completion.final_outputs {
            println!("    {} {}", output.port_ref, output.value_digest);
        }
    }
    print_stop_reason(
        "  ",
        response.session.stop_reason.as_ref(),
        &response.session.ledger.guard_evaluations,
    );
    print_branch_decisions("  ", &response.session.ledger.branch_decisions);
    print_failure_routes("  ", &response.session.ledger.failure_routes);
    print_parallel_panel_records("  ", &response.session.ledger.parallel_panel_records);
    if let Some(frame) = response.next_frame.as_deref() {
        println!("  next-frame:");
        print_sequence_frame("    ", frame);
    } else {
        println!("  next-frame: none");
    }
}

fn print_control_frame(indent: &str, frame: &ctx_traits_core::procedure::runtime::ControlFrame) {
    let control_id = frame.control_item_id.as_deref().unwrap_or("<unnamed>");
    match frame.kind {
        ctx_traits_core::procedure::runtime::ControlKind::Sequence => println!(
            "{indent}sequence control-id={control_id} sequence={} next-index={}",
            frame.sequence_id, frame.next_index
        ),
        ctx_traits_core::procedure::runtime::ControlKind::Branch => println!(
            "{indent}branch control-id={control_id} sequence={} next-index={}",
            frame.sequence_id, frame.next_index
        ),
        ctx_traits_core::procedure::runtime::ControlKind::Loop => println!(
            "{indent}loop control-id={control_id} sequence={} next-index={} iteration={}/{}",
            frame.sequence_id,
            frame.next_index,
            frame
                .iteration_index
                .map(|v| v.saturating_add(1))
                .unwrap_or(0),
            frame.max_iterations.unwrap_or(0)
        ),
        ctx_traits_core::procedure::runtime::ControlKind::ForEach => println!(
            "{indent}for-each control-id={control_id} sequence={} next-index={} item={}/{} max-items={}",
            frame.sequence_id,
            frame.next_index,
            frame.item_index.map(|v| v.saturating_add(1)).unwrap_or(0),
            frame.item_total.unwrap_or(0),
            frame.max_items.unwrap_or(0)
        ),
        ctx_traits_core::procedure::runtime::ControlKind::Parallel => {
            println!(
                "{indent}parallel control-id={control_id} branch={} next-index={} branch-index={}/{} join={}",
                frame.sequence_id,
                frame.next_index,
                frame
                    .iteration_index
                    .map(|v| v.saturating_add(1))
                    .unwrap_or(0),
                frame.max_iterations.unwrap_or(0),
                frame.join.as_ref().map_or(
                    "collect-in-order",
                    ctx_traits_core::r#trait::procedure::JoinPolicy::label
                )
            );
        }
    }
}

fn print_branch_decisions(
    indent: &str,
    decisions: &[ctx_traits_core::procedure::runtime::BranchDecision],
) {
    println!("{indent}branch-decisions: {}", decisions.len());
    for decision in decisions {
        println!(
            "{indent}  {} matched={} selected={} sequence={} at {}",
            decision.branch_id,
            decision.matched,
            decision.selected_arm,
            decision.sequence_id.as_deref().unwrap_or("none"),
            render_runtime_path(&decision.position_path),
        );
    }
}

fn print_parallel_panel_records(
    indent: &str,
    records: &[ctx_traits_core::procedure::runtime::ParallelPanelRecord],
) {
    if records.is_empty() {
        return;
    }
    println!("{indent}parallel-panel-records: {}", records.len());
    for record in records {
        let branches: Vec<String> = record
            .branches
            .iter()
            .map(|branch| {
                format!(
                    "{}={}",
                    branch.branch_ref,
                    branch
                        .outcome
                        .map_or("pending", parallel_branch_outcome_label)
                )
            })
            .collect();
        println!(
            "{indent}  {} join={} disposition={} branches=[{}] at {}",
            record.control_item_id.as_deref().unwrap_or("<unnamed>"),
            record.join_policy,
            parallel_panel_disposition_label(record.disposition),
            branches.join(", "),
            render_runtime_path(&record.position_path),
        );
        if let Some(digest) = record.result_digest.as_ref() {
            println!("{indent}    result-digest={digest}");
        }
        if let Some(index) = record.guard_evaluation_index {
            println!("{indent}    guard-evaluation-index={index}");
        }
    }
}

fn parallel_branch_outcome_label(
    outcome: ctx_traits_core::procedure::runtime::ParallelBranchOutcome,
) -> &'static str {
    match outcome {
        ctx_traits_core::procedure::runtime::ParallelBranchOutcome::Committed => "committed",
        ctx_traits_core::procedure::runtime::ParallelBranchOutcome::Skipped => "skipped",
        ctx_traits_core::procedure::runtime::ParallelBranchOutcome::Parked => "parked",
    }
}

fn parallel_panel_disposition_label(
    disposition: ctx_traits_core::procedure::runtime::ParallelPanelDisposition,
) -> &'static str {
    match disposition {
        ctx_traits_core::procedure::runtime::ParallelPanelDisposition::Completed => "completed",
        ctx_traits_core::procedure::runtime::ParallelPanelDisposition::Routed => "routed",
        ctx_traits_core::procedure::runtime::ParallelPanelDisposition::Stopped => "stopped",
        ctx_traits_core::procedure::runtime::ParallelPanelDisposition::Parked => "parked",
    }
}

fn print_failure_routes(
    indent: &str,
    routes: &[ctx_traits_core::procedure::runtime::FailureRouteRecord],
) {
    println!("{indent}failure-routes: {}", routes.len());
    for route in routes {
        println!(
            "{indent}  {} -> {} signal={} at {}",
            route.source_step_id,
            route.target_step_id,
            route.signal.as_deref().unwrap_or("none"),
            render_runtime_path(&route.position_path),
        );
    }
}

fn format_guard_check_suffix(
    last_check: Option<usize>,
    evaluations: &[ctx_traits_core::r#trait::ConditionEvaluation],
) -> String {
    let Some(index) = last_check else {
        return String::new();
    };
    let Some(evaluation) = evaluations.get(index) else {
        return format!(" last-check={index}");
    };
    format!(
        " last-check={} matched={} reason={}",
        evaluation.predicate, evaluation.matched, evaluation.reason
    )
}

fn print_stop_reason(
    indent: &str,
    stop_reason: Option<&ctx_traits_core::procedure::runtime::StopReason>,
    evaluations: &[ctx_traits_core::r#trait::ConditionEvaluation],
) {
    let Some(stop_reason) = stop_reason else {
        return;
    };
    println!(
        "{indent}stop-reason: {} at {}{}",
        stop_reason.reason,
        render_runtime_path(&stop_reason.at),
        format_guard_check_suffix(stop_reason.last_check, evaluations)
    );
}

/// Prints stable blocker ids from accepted slot values escalated
/// `needs-owner`, sorted and deduplicated for byte-stable output.
fn print_escalation_blockers(
    indent: &str,
    accepted_slot_values: &[ctx_traits_core::procedure::runtime::Value],
) {
    let mut blockers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for value in accepted_slot_values {
        let gloss = ctx_traits_core::procedure::story::value_gloss(&value.value);
        if gloss.escalation.as_deref() != Some("needs-owner") {
            continue;
        }
        blockers.extend(gloss.blockers);
    }
    if !blockers.is_empty() {
        println!(
            "{indent}escalation-blockers: {}",
            blockers.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
}

pub(crate) fn print_sequence_frame(
    indent: &str,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) {
    println!("{indent}title: {}", frame.title);
    if !frame.position_path.is_empty() {
        println!(
            "{indent}position-path: {}",
            render_runtime_path(&frame.position_path)
        );
    }
    if let Some(loop_context) = &frame.loop_context {
        println!(
            "{indent}loop: {} iteration {}/{}",
            loop_context.loop_id,
            loop_context.iteration_index.saturating_add(1),
            loop_context.max_iterations
        );
    }
    if let Some(for_each_context) = &frame.for_each_context {
        println!(
            "{indent}for-each: {} item {}/{} max {}",
            for_each_context.for_each_id,
            for_each_context.item_index.saturating_add(1),
            for_each_context.item_total,
            for_each_context.max_items
        );
    }
    if !frame.guard_explanations.is_empty() {
        println!("{indent}guard-explanations:");
        for guard in &frame.guard_explanations {
            println!(
                "{indent}  {} matched={} {}",
                guard.predicate, guard.matched, guard.reason
            );
        }
    }
    if let Some(run_index) = frame.run_index {
        println!("{indent}run-index: {run_index}");
    }
    if let Some(sequence_index) = frame.sequence_index {
        println!("{indent}source-index: {sequence_index}");
    }
    if let Some(item_id) = &frame.item_id {
        println!("{indent}item-id: {item_id}");
    }
    if let Some(agent) = &frame.assigned_agent {
        println!(
            "{indent}assigned-agent: {} ({})",
            agent.ref_text, agent.description
        );
    }
    if let Some(command) = &frame.command {
        println!("{indent}command-permission: {}", command.permission_code);
        println!(
            "{indent}command-argv: {}",
            crate::app::presentation::argv_display(&command.argv)
        );
        println!("{indent}command-output-slot: {}", command.output_slot);
        println!("{indent}command-execution: trusted local runtime executes when current");
    }
    if frame.available_inputs.is_empty() {
        println!("{indent}available-inputs: none");
    } else {
        println!("{indent}available-inputs:");
        for value in &frame.available_inputs {
            println!("{indent}  {} {}", value.ref_text, value.value_digest);
        }
    }
    if frame.resource_evidence.is_empty() {
        println!("{indent}resources: none");
    } else {
        println!("{indent}resources:");
        for resource in &frame.resource_evidence {
            println!(
                "{indent}  {} available={} {}",
                resource.resource_ref, resource.available, resource.reason
            );
        }
    }
    if frame.requested_outputs.is_empty() {
        println!("{indent}requested-outputs: none");
    } else {
        println!("{indent}requested-outputs:");
        for output in &frame.requested_outputs {
            let operation = serde_json::to_string(&output.operation)
                .unwrap_or_else(|_| "\"replace\"".to_string());
            println!(
                "{indent}  {} operation={} schema={}",
                output.slot_ref,
                operation,
                output.schema_ref.as_deref().unwrap_or("schema:any")
            );
        }
    }
    if frame.allowed_signals.is_empty() {
        println!("{indent}allowed-signals: none");
    } else {
        println!(
            "{indent}allowed-signals: {}",
            frame.allowed_signals.join(", ")
        );
    }
    if let Some(template) = &frame.call_template {
        println!("{indent}call-template:");
        println!("{indent}  session-id: {}", template.session_id);
        println!("{indent}  run-id: {}", template.run_id);
        println!("{indent}  state-digest: {}", template.state_digest);
        println!(
            "{indent}  expected-run-index: {}",
            template.expected_run_index
        );
        if let Some(source_index) = template.expected_source_index {
            println!("{indent}  expected-source-index: {source_index}");
        }
        if let Some(item_id) = &template.expected_sequence_item_id {
            println!("{indent}  expected-sequence-item-id: {item_id}");
        }
        if !template.expected_position_path.is_empty() {
            println!(
                "{indent}  expected-position-path: {}",
                render_runtime_path(&template.expected_position_path)
            );
        }
        if let Some(agent) = &template.required_agent {
            println!(
                "{indent}  required-agent: {} ({})",
                agent.ref_text, agent.description
            );
        }
        if let Some(agent) = &template.caller.agent {
            println!("{indent}  caller-agent: {agent}");
        }
        if template.produced_slots.is_empty() {
            println!("{indent}  produced-slots: none");
        } else {
            println!("{indent}  produced-slots:");
            for (slot, expectation) in &template.produced_slots {
                println!("{indent}    {slot}: {expectation}");
            }
        }
    }
    println!("{indent}instruction: do not skip steps; submit through ctx traits session frame set");
}

pub(crate) fn render_runtime_path(
    path: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> String {
    if path.is_empty() {
        return "procedure".to_string();
    }
    path.iter()
        .map(|segment| {
            let id = segment
                .id
                .as_deref()
                .map(|id| format!(":{id}"))
                .unwrap_or_default();
            let iteration = segment
                .iteration
                .map(|iteration| format!("#iteration:{iteration}"))
                .unwrap_or_default();
            let item_index = segment
                .item_index
                .map(|item_index| format!("#item:{item_index}"))
                .unwrap_or_default();
            format!(
                "{}{}[{}]{}{}",
                segment.kind,
                segment
                    .id
                    .as_deref()
                    .map(|_| id.clone())
                    .unwrap_or_default(),
                segment.index,
                iteration,
                item_index
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn next_action_hint(
    session: &ctx_traits_core::procedure::session::Session,
    out: Option<&str>,
) -> String {
    let agent_flag = session
        .current_agent
        .as_ref()
        .map(|agent| format!(" --agent {}", agent.role))
        .unwrap_or_default();
    match session.status {
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput
        | ctx_traits_core::procedure::session::Status::WaitingOnHuman
        | ctx_traits_core::procedure::session::Status::Rejected => format!(
            "ctx traits session frame set --session {} --key <target> --value <value>{agent_flag}",
            out.unwrap_or("<run-session>")
        ),
        ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned => {
            match session.current_agent.as_ref() {
                Some(agent) => format!(
                    "current frame is assigned to agent role {} but this run has no assignment for it; {}",
                    agent.role,
                    ctx_traits_io::harness_config::unassigned_role_remediation(&agent.role)
                ),
                None => format!(
                    "current frame has an assigned agent role but this run has no assignment for it; {}",
                    ctx_traits_io::harness_config::unassigned_role_remediation("<role>")
                ),
            }
        }
        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired => {
            "resume through the trusted local runtime to execute the current command frame"
                .to_string()
        }
        ctx_traits_core::procedure::session::Status::Blocked => {
            "provide missing inputs/resources and start or resume with explicit state".to_string()
        }
        ctx_traits_core::procedure::session::Status::Completed => "run.completed".to_string(),
        ctx_traits_core::procedure::session::Status::Failed => "inspect diagnostics".to_string(),
        ctx_traits_core::procedure::session::Status::AwaitingInput => {
            format!(
                "ctx traits session frame set --session {} --key <input-port> --value <value>",
                out.unwrap_or("<run-session>")
            )
        }
    }
}

fn producer_suffix(agent: Option<&str>, harness: Option<&str>) -> String {
    match (agent, harness) {
        (None, None) => String::new(),
        (agent, harness) => format!(
            " producer-agent={} producer-harness={}",
            agent.unwrap_or("-"),
            harness.unwrap_or("-")
        ),
    }
}

fn format_output_port_status(
    status: &ctx_traits_core::procedure::runtime::OutputPortStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::runtime::OutputPortStatus::Accepted => "accepted",
        ctx_traits_core::procedure::runtime::OutputPortStatus::Missing => "missing",
        ctx_traits_core::procedure::runtime::OutputPortStatus::OptionalMissing => {
            "optional-missing"
        }
    }
}
