// Procedure runtime readiness checks.
// Procedure runtime readiness checks.

/// Validate a supplied runtime ledger against the current procedure contract.
pub fn validate_run_ledger_contract(
    trait_ref: &Trait,
    ledger: &State,
) -> crate::Result<LedgerValidationReport> {
    let mut diagnostics = Vec::new();
    if ledger.trait_id != trait_ref.id.as_str() {
        diagnostics.push(format!(
            "ledger trait-id {:?} does not match current trait {:?}",
            ledger.trait_id,
            trait_ref.id.as_str()
        ));
    }

    let Some(proc) = trait_ref.procedure.as_ref() else {
        diagnostics.push("run ledger supplied for trait without a procedure".to_string());
        return Ok(LedgerValidationReport {
            status: LedgerContractStatus::Invalid,
            contract_valid: false,
            diagnostics,
        });
    };

    let sequence = effective_sequence_items(proc)?;
    let sequence_contract = SequenceContract::new(&sequence);
    let status_maps = sequence_status_maps(ledger, &mut diagnostics);
    let accepted_evidence = accepted_evidence_maps(ledger, &mut diagnostics);
    validate_sequence_status_contract(trait_ref, &sequence, ledger, &mut diagnostics);
    validate_current_run_index(&sequence, ledger, &mut diagnostics);
    validate_control_state(trait_ref, &sequence, ledger, &mut diagnostics);
    validate_conditional_input_decisions_contract(trait_ref, &sequence, ledger, &mut diagnostics);
    validate_ask_decisions_contract(trait_ref, &sequence, ledger, &mut diagnostics);
    validate_accepted_port_values(trait_ref, ledger, &mut diagnostics)?;
    validate_accepted_slot_values(trait_ref, proc, ledger, &mut diagnostics)?;
    validate_accepted_output_port_values(trait_ref, ledger, &mut diagnostics)?;
    validate_accepted_sequence_outputs(
        &sequence_contract,
        &accepted_evidence,
        ledger,
        &mut diagnostics,
    );
    validate_accepted_slot_producer_statuses(
        &sequence_contract,
        &status_maps,
        &accepted_evidence,
        ledger,
        &mut diagnostics,
    );
    validate_emitted_signals(
        trait_ref,
        &sequence_contract,
        &status_maps,
        &accepted_evidence,
        ledger,
        &mut diagnostics,
    );
    validate_rejected_attempts(trait_ref, &sequence, ledger, &mut diagnostics);
    validate_output_port_contract(trait_ref, ledger, &mut diagnostics)?;
    validate_final_state_contract(
        trait_ref,
        &sequence,
        &sequence_contract,
        &status_maps,
        &accepted_evidence,
        ledger,
        &mut diagnostics,
    )?;

    let status = if diagnostics.is_empty() {
        match ledger.final_state {
            FinalState::Completed => LedgerContractStatus::ValidCompleted,
            FinalState::Running => LedgerContractStatus::ValidRunning,
            FinalState::Blocked => LedgerContractStatus::ValidBlocked,
            FinalState::Rejected => LedgerContractStatus::ValidRejected,
            FinalState::Failed => LedgerContractStatus::ValidFailed,
        }
    } else {
        LedgerContractStatus::Invalid
    };

    Ok(LedgerValidationReport {
        contract_valid: diagnostics.is_empty(),
        status,
        diagnostics,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct ReadyItem<'a> {
    sequence_index: usize,
    run_index: usize,
    item: &'a crate::r#trait::procedure::SequenceItem,
    position_path: Vec<PathSegment>,
    loop_context: Option<LoopContext>,
    for_each_context: Option<ForEachContext>,
    guard_explanations: Vec<ConditionEvaluation>,
}

enum Readiness<'a> {
    Ready(ReadyItem<'a>),
    Blocked {
        missing_inputs: Vec<String>,
        capabilities: Vec<CapabilityReport>,
    },
    Completed,
    Rejected,
    Failed,
}

fn runtime_readiness<'a>(
    trait_ref: &'a Trait,
    state: &State,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
) -> crate::Result<Readiness<'a>> {
    match state.final_state {
        FinalState::Completed => return Ok(Readiness::Completed),
        FinalState::Rejected => return Ok(Readiness::Rejected),
        FinalState::Failed => return Ok(Readiness::Failed),
        FinalState::Blocked if state.stop_reason.is_some() => {
            return Ok(Readiness::Blocked {
                missing_inputs: vec!["run:controlled-stop".to_string()],
                capabilities: vec![CapabilityReport::unsupported(
                    "runtime.controlled-stop",
                    "run stopped with a structured control-flow stop reason",
                )],
            });
        }
        FinalState::Blocked | FinalState::Running => {}
    }

    let missing_ports = missing_required_procedure_ports(trait_ref, state);
    if !missing_ports.is_empty() {
        return Ok(Readiness::Blocked {
            missing_inputs: missing_ports.clone(),
            capabilities: vec![CapabilityReport::unsupported(
                "runtime.input-port-binding",
                format!(
                    "missing required input port value(s): {}",
                    missing_ports.join(", ")
                ),
            )],
        });
    }

    if state.current_run_index >= sequence.len() && state.control_stack.is_empty() {
        let output_ports = finalize_outputs(trait_ref, state)?;
        let missing_outputs: Vec<String> = output_ports
            .iter()
            .filter(|output| output.required && output.status != OutputPortStatus::Accepted)
            .map(|output| output.port_ref.to_string())
            .collect();
        if !missing_outputs.is_empty() {
            return Ok(Readiness::Blocked {
                missing_inputs: missing_outputs.clone(),
                capabilities: vec![CapabilityReport::unsupported(
                    "runtime.output-port-completion",
                    format!(
                        "missing required output port value(s): {}",
                        missing_outputs.join(", ")
                    ),
                )],
            });
        }
        return Ok(Readiness::Completed);
    }

    let Some(ready) = current_ready_item(trait_ref, state, sequence)? else {
        if state.final_state == FinalState::Blocked {
            let missing = missing_current_control_inputs(trait_ref, state)?;
            if !missing.is_empty() {
                return Ok(Readiness::Blocked {
                    missing_inputs: missing.clone(),
                    capabilities: vec![CapabilityReport::unsupported(
                        "runtime.sequence-input",
                        format!("missing current sequence input(s): {}", missing.join(", ")),
                    )],
                });
            }
            return Ok(Readiness::Blocked {
                missing_inputs: vec!["run:blocked-control-state".to_string()],
                capabilities: vec![CapabilityReport::unsupported(
                    "runtime.blocked-control-state",
                    "blocked control state has no current executable item",
                )],
            });
        }
        return Ok(Readiness::Failed);
    };
    let missing = missing_inputs_for_item(trait_ref, &ready, state)?;
    if !missing.is_empty() {
        let mut capabilities = vec![CapabilityReport::unsupported(
            "runtime.sequence-input",
            format!("missing current sequence input(s): {}", missing.join(", ")),
        )];
        capabilities.extend(dependency_capabilities_for_item(ready.item));
        capabilities.sort();
        capabilities.dedup();
        return Ok(Readiness::Blocked {
            missing_inputs: missing,
            capabilities,
        });
    }

    match state.final_state {
        FinalState::Running => Ok(Readiness::Ready(ready)),
        FinalState::Blocked => Ok(Readiness::Blocked {
            missing_inputs: vec!["run:blocked".to_string()],
            capabilities: vec![CapabilityReport::unsupported(
                "runtime.blocked-state",
                "run final state is blocked; refresh or restart the run before applying output",
            )],
        }),
        FinalState::Completed => Ok(Readiness::Completed),
        FinalState::Rejected => Ok(Readiness::Rejected),
        FinalState::Failed => Ok(Readiness::Failed),
    }
}

fn current_ready_item<'a>(
    trait_ref: &'a Trait,
    state: &State,
    sequence: &[crate::procedure::run::EffectiveSequenceItem<'a>],
) -> crate::Result<Option<ReadyItem<'a>>> {
    if let Some(frame) = state.control_stack.last() {
        let Some(named) = trait_ref.sequences.get(&frame.sequence_id) else {
            return Ok(None);
        };
        let Some(item) = named.sequence.get(frame.next_index) else {
            return Ok(None);
        };
        if !is_executable_item(item) {
            return Ok(None);
        }
        let loop_context = loop_context_from_stack(&state.control_stack);
        let for_each_context = for_each_context_from_stack(&state.control_stack);
        let path = path_for_nested_item(state, frame.next_index, item);
        let guard_explanations = guard_explanations_for_context(state, loop_context.as_ref());
        return Ok(Some(ReadyItem {
            sequence_index: frame.next_index,
            run_index: state.current_run_index,
            item,
            position_path: path,
            loop_context,
            for_each_context,
            guard_explanations,
        }));
    }

    let Some(outer) = sequence
        .iter()
        .find(|item| item.run_index == state.current_run_index)
    else {
        return Ok(None);
    };
    if !is_executable_item(outer.item) {
        return Ok(None);
    }
    Ok(Some(ReadyItem {
        sequence_index: outer.declaration_index,
        run_index: outer.run_index,
        item: outer.item,
        position_path: Vec::new(),
        loop_context: None,
        for_each_context: None,
        guard_explanations: Vec::new(),
    }))
}

fn assigned_agent_role(
    trait_ref: &Trait,
    agent_ref: Option<&str>,
    position_path: &[PathSegment],
    sequence_index: usize,
) -> crate::Result<Option<AgentRole>> {
    let Some(agent_ref) = agent_ref else {
        return Ok(None);
    };
    let parsed = Reference::parse(agent_ref).map_err(|_| {
        crate::procedure::invalid_field(
            "procedure.sequence.agent",
            format!("invalid agent ref {agent_ref:?}"),
        )
    })?;
    if parsed.kind() != Kind::Agent || parsed.is_qualified() {
        return Err(crate::procedure::invalid_field(
            "procedure.sequence.agent",
            "assigned agent must be a local agent:<id> ref",
        ));
    }
    let Some(agent) = trait_ref
        .agents
        .iter()
        .find(|agent| agent.id == parsed.id())
    else {
        return Err(crate::procedure::invalid_field(
            "procedure.sequence.agent",
            format!("unresolved assigned agent ref {agent_ref:?}"),
        ));
    };
    let structural_seat = AgentDeclarationSites::build(trait_ref).ordinal_for(
        &agent.id,
        position_path,
        sequence_index,
    );
    Ok(Some(AgentRole {
        role: agent.id.clone(),
        ref_text: agent_ref.to_string(),
        description: agent.description.clone(),
        summary: agent.summary.clone(),
        system: agent.system.clone(),
        structural_seat,
    }))
}

/// The zero-based role-local structural seat ([`AgentRole::structural_seat`])
/// for a top-level `agent_ref` site declared at `sequence_index` in
/// `procedure.sequence` — exposed for a static (not-yet-active) planned-item
/// projection outside the live/preview runtime path (e.g. a TUI/plain
/// planned-step row), which has no live or preview `position_path` to hand
/// `assigned_agent_role`. `None` for a nested site (not resolvable from a
/// declaration index alone) or an unparseable/non-local agent ref.
pub fn structural_seat_for_declaration_index(
    trait_ref: &Trait,
    agent_ref: &str,
    sequence_index: usize,
) -> Option<u32> {
    let parsed = Reference::parse(agent_ref).ok()?;
    if parsed.kind() != Kind::Agent || parsed.is_qualified() {
        return None;
    }
    AgentDeclarationSites::build(trait_ref).ordinal_for(parsed.id(), &[], sequence_index)
}

/// The zero-based role-local structural seat ([`AgentRole::structural_seat`])
/// for an `agent_ref` site declared at `item_index` inside named sequence
/// `sequence_id`'s own body — the nested counterpart of
/// [`structural_seat_for_declaration_index`], exposed for a static
/// (not-yet-active) planned-item projection (e.g. a TUI/plain planned-step
/// row for a loop/branch/parallel/for-each body) that has a dry-plan
/// container position (the named sequence it is walking plus the item's
/// index within it) but no live/preview `position_path`. Path-independent:
/// the same `(sequence_id, item_index)` resolves to the same seat
/// regardless of how many different control sites reference `sequence_id`
/// (P456). `None` for an unparseable/non-local agent ref or a site the
/// declaration walk never reached (e.g. a named sequence unreachable from
/// `procedure.sequence`).
pub fn structural_seat_for_nested_declaration(
    trait_ref: &Trait,
    agent_ref: &str,
    sequence_id: &str,
    item_index: usize,
) -> Option<u32> {
    let parsed = Reference::parse(agent_ref).ok()?;
    if parsed.kind() != Kind::Agent || parsed.is_qualified() {
        return None;
    }
    let container_path = [PathSegment {
        kind: "sequence".to_string(),
        id: Some(sequence_id.to_string()),
        index: item_index,
        iteration: None,
        item_index: None,
    }];
    AgentDeclarationSites::build(trait_ref).ordinal_for(parsed.id(), &container_path, 0)
}

struct AgentSite {
    role: String,
    ordinal: u32,
}

/// Pure declaration-order table of every reachable `agent:<role>` reference
/// site in `trait_ref`, independent of any runtime state, so the same
/// authored site always resolves to the same zero-based per-role ordinal
/// (P456). A named sequence reached from more than one control site (or
/// twice from the same `parallel`/`branch`) is walked only once: whichever
/// control site reaches it first in declaration order owns its sites, and
/// every later caller of that same shared sequence resolves to the same
/// sites — nested identity is keyed purely by `(sequence_id, item_index)`,
/// the named sequence's OWN id and the item's position within it, never by
/// the path used to reach it. That makes identity independent of which
/// activation path (live control-stack, preview projection, or dry-plan
/// nested-child walk) led to the site, and immune to how many different
/// callers a shared named sequence has.
struct AgentDeclarationSites {
    top_level: BTreeMap<usize, AgentSite>,
    /// Keyed by `(sequence_id, item_index)` — the named sequence body
    /// containing the site and the site's index within that body.
    nested: BTreeMap<(String, usize), AgentSite>,
}

impl AgentDeclarationSites {
    fn build(trait_ref: &Trait) -> Self {
        let mut walk = DeclarationWalk {
            trait_ref,
            visited: BTreeSet::new(),
            counters: BTreeMap::new(),
            sites: AgentDeclarationSites {
                top_level: BTreeMap::new(),
                nested: BTreeMap::new(),
            },
        };
        if let Some(proc) = trait_ref.procedure.as_ref() {
            walk.walk(&proc.sequence, None);
        }
        walk.sites
    }

    /// `position_path` may come from a live control-stack activation
    /// ([`path_for_nested_item`], which always appends a trailing `"item"`
    /// segment for the leaf itself on top of one segment per control-stack
    /// frame) or from the preview projector ([`expand_nested_preview`],
    /// which only ever reaches container depth — one segment per nesting
    /// level, no trailing leaf segment). Both alias the SAME declared site,
    /// so only the LAST container segment is used to key the lookup: its
    /// `id` is the named sequence body the site lives in and its `index` is
    /// the site's position within that body — exactly `nested`'s key,
    /// regardless of how many ancestor segments led there or through which
    /// caller. The trailing `"item"` segment (uniquely identifiable — no
    /// ancestor control-stack frame is ever `"item"` kind) is stripped
    /// first (P456).
    fn ordinal_for(
        &self,
        role: &str,
        position_path: &[PathSegment],
        sequence_index: usize,
    ) -> Option<u32> {
        if position_path.is_empty() {
            return self
                .top_level
                .get(&sequence_index)
                .filter(|site| site.role == role)
                .map(|site| site.ordinal);
        }
        let container_path = match position_path.last() {
            Some(last) if last.kind == "item" => &position_path[..position_path.len() - 1],
            _ => position_path,
        };
        let last = container_path.last()?;
        let sequence_id = last.id.as_ref()?;
        self.nested
            .get(&(sequence_id.clone(), last.index))
            .filter(|site| site.role == role)
            .map(|site| site.ordinal)
    }
}

/// Mutable state threaded through one [`AgentDeclarationSites::build`] walk,
/// bundled so the recursive traversal method takes a small, fixed argument
/// list regardless of how much state the walk itself accumulates.
struct DeclarationWalk<'a> {
    trait_ref: &'a Trait,
    /// Named-sequence ids already walked, so a sequence reached from more
    /// than one control site (or twice from the same `parallel`/`branch`)
    /// contributes its sites only once — whichever site reaches it first in
    /// declaration order.
    visited: BTreeSet<String>,
    /// Per-role next ordinal, assigned in declaration order as each
    /// `agent:<role>` site is reached.
    counters: BTreeMap<String, u32>,
    sites: AgentDeclarationSites,
}

impl DeclarationWalk<'_> {
    /// Walk one body in declaration order. `owning_sequence_id` is `None`
    /// for the top-level `procedure.sequence` body (whose sites key by
    /// declaration index alone) and `Some(id)` for a named sequence's own
    /// body reached via a `sequence`/`loop`/`for-each`/`branch`/`parallel`
    /// control item (whose sites key by `(id, item_index)`, independent of
    /// the path that reached this body).
    fn walk(
        &mut self,
        body: &[crate::r#trait::procedure::SequenceItem],
        owning_sequence_id: Option<&str>,
    ) {
        for (index, item) in body.iter().enumerate() {
            if is_executable_item(item)
                && let Some(role) = item
                    .agent
                    .as_deref()
                    .and_then(|agent_ref| Reference::parse(agent_ref).ok())
                    .filter(|parsed| parsed.kind() == Kind::Agent && !parsed.is_qualified())
                    .map(|parsed| parsed.id().to_string())
                {
                    let ordinal_slot = self.counters.entry(role.clone()).or_insert(0);
                    let ordinal = *ordinal_slot;
                    *ordinal_slot += 1;
                    let site = AgentSite { role, ordinal };
                    match owning_sequence_id {
                        None => {
                            self.sites.top_level.insert(index, site);
                        }
                        Some(sequence_id) => {
                            self.sites
                                .nested
                                .insert((sequence_id.to_string(), index), site);
                        }
                    }
                }
            let seq_refs: Vec<&str> = match item.effective_kind() {
                SequenceKind::Sequence | SequenceKind::Loop | SequenceKind::ForEach => {
                    item.sequence.as_deref().into_iter().collect()
                }
                SequenceKind::Branch => [item.sequence.as_deref(), item.otherwise.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect(),
                SequenceKind::Parallel => item.branches.iter().map(String::as_str).collect(),
                _ => Vec::new(),
            };
            for seq_ref in seq_refs {
                let Ok(seq_id) = sequence_id_from_ref(seq_ref) else {
                    continue;
                };
                if !self.visited.insert(seq_id.clone()) {
                    continue;
                }
                let Some(named) = self.trait_ref.sequences.get(&seq_id) else {
                    continue;
                };
                self.walk(&named.sequence, Some(seq_id.as_str()));
            }
        }
    }
}

fn is_executable_item(item: &crate::r#trait::procedure::SequenceItem) -> bool {
    matches!(
        item.effective_kind(),
        SequenceKind::Prompt | SequenceKind::Ask | SequenceKind::Command | SequenceKind::Check
    )
}

fn producer_path_for_ready(ready: &ReadyItem<'_>) -> Vec<PathSegment> {
    if !ready.position_path.is_empty() {
        return ready.position_path.clone();
    }
    vec![PathSegment {
        kind: "procedure".to_string(),
        id: ready.item.id.clone(),
        index: ready.run_index,
        iteration: None,
        item_index: None,
    }]
}

fn duplicate_slot_write_in_activation(
    state: &State,
    slot_ref: &str,
    position_path: &[PathSegment],
) -> bool {
    visible_slot_revisions(state).into_iter().any(|revision| {
        revision.slot_ref.as_str() == slot_ref && revision.position_path.as_slice() == position_path
    })
}

fn path_for_nested_item(
    state: &State,
    item_index: usize,
    item: &crate::r#trait::procedure::SequenceItem,
) -> Vec<PathSegment> {
    let mut path = Vec::new();
    if let Some(first) = state.control_stack.first() {
        path.push(PathSegment {
            kind: "procedure".to_string(),
            id: first.control_item_id.clone(),
            index: first.parent_run_index,
            iteration: None,
            item_index: None,
        });
    }
    for frame in &state.control_stack {
        path.push(PathSegment {
            kind: control_kind_name(&frame.kind).to_string(),
            id: Some(frame.sequence_id.clone()),
            index: frame.next_index,
            iteration: frame.iteration_index,
            item_index: frame.item_index,
        });
    }
    path.push(PathSegment {
        kind: "item".to_string(),
        id: item.id.clone(),
        index: item_index,
        iteration: state
            .control_stack
            .iter()
            .rev()
            .find(|frame| matches!(frame.kind, ControlKind::Loop | ControlKind::Parallel))
            .and_then(|frame| frame.iteration_index),
        item_index: state
            .control_stack
            .iter()
            .rev()
            .find(|frame| frame.kind == ControlKind::ForEach)
            .and_then(|frame| frame.item_index),
    });
    path
}

fn control_kind_name(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::Sequence => "sequence",
        ControlKind::Branch => "branch",
        ControlKind::Loop => "loop",
        ControlKind::ForEach => "for-each",
        ControlKind::Parallel => "parallel",
    }
}

fn loop_context_from_stack(stack: &[ControlFrame]) -> Option<LoopContext> {
    stack.iter().rev().find_map(|frame| {
        if frame.kind == ControlKind::Loop {
            let max_iterations = frame.max_iterations?;
            Some(LoopContext {
                loop_id: frame
                    .control_item_id
                    .clone()
                    .unwrap_or_else(|| frame.sequence_id.clone()),
                sequence_id: Some(frame.sequence_id.clone()),
                iteration_index: frame.iteration_index.unwrap_or(0),
                max_iterations,
            })
        } else {
            None
        }
    })
}

fn guard_context_from_stack(stack: &[ControlFrame]) -> Option<LoopContext> {
    stack.iter().rev().find_map(|frame| match frame.kind {
        ControlKind::Loop => loop_context_from_stack(std::slice::from_ref(frame)),
        ControlKind::ForEach => for_each_guard_context_from_stack(std::slice::from_ref(frame)),
        ControlKind::Sequence | ControlKind::Branch | ControlKind::Parallel => None,
    })
}

#[derive(Clone, PartialEq, Eq)]
struct ActivationOwnerSegment {
    kind: String,
    id: Option<String>,
    index: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct RepeatedActivation {
    owner_path: Vec<ActivationOwnerSegment>,
    kind: String,
    id: Option<String>,
    iteration: Option<usize>,
    item_index: Option<usize>,
}

fn repeated_activation_scope(path: &[PathSegment]) -> Vec<RepeatedActivation> {
    path.iter()
        .enumerate()
        .filter(|(_, segment)| matches!(segment.kind.as_str(), "loop" | "for-each"))
        .map(|(position, segment)| RepeatedActivation {
            owner_path: path[..position]
                .iter()
                .map(|owner| ActivationOwnerSegment {
                    kind: owner.kind.clone(),
                    id: owner.id.clone(),
                    index: owner.index,
                })
                .collect(),
            kind: segment.kind.clone(),
            id: segment.id.clone(),
            iteration: segment.iteration,
            item_index: segment.item_index,
        })
        .collect()
}

/// Reuse the guard scope shape for a for-each activation so signal guards
/// cannot observe emissions from a different item.
fn for_each_guard_context_from_stack(stack: &[ControlFrame]) -> Option<LoopContext> {
    stack.iter().rev().find_map(|frame| {
        (frame.kind == ControlKind::ForEach).then(|| LoopContext {
            loop_id: frame
                .control_item_id
                .clone()
                .unwrap_or_else(|| frame.sequence_id.clone()),
            sequence_id: Some(frame.sequence_id.clone()),
            iteration_index: frame.item_index.unwrap_or(0),
            max_iterations: frame.item_total.unwrap_or(1),
        })
    })
}

fn for_each_context_from_stack(stack: &[ControlFrame]) -> Option<ForEachContext> {
    stack.iter().rev().find_map(|frame| {
        if frame.kind == ControlKind::ForEach {
            Some(ForEachContext {
                for_each_id: frame
                    .control_item_id
                    .clone()
                    .unwrap_or_else(|| frame.sequence_id.clone()),
                item_index: frame.item_index.unwrap_or(0),
                item_total: frame.item_total.unwrap_or(0),
                max_items: frame
                    .max_items
                    .unwrap_or_else(|| frame.item_total.unwrap_or(0)),
            })
        } else {
            None
        }
    })
}

fn guard_explanations_for_context(
    state: &State,
    loop_context: Option<&LoopContext>,
) -> Vec<ConditionEvaluation> {
    let Some(loop_context) = loop_context else {
        return Vec::new();
    };
    let latest_iteration = state
        .guard_evaluations
        .iter()
        .filter_map(|evaluation| matching_guard_scope(evaluation, loop_context))
        .filter(|iteration| *iteration <= loop_context.iteration_index)
        .max();
    let Some(latest_iteration) = latest_iteration else {
        return Vec::new();
    };
    state
        .guard_evaluations
        .iter()
        .rev()
        .filter(|evaluation| {
            matching_guard_scope(evaluation, loop_context) == Some(latest_iteration)
        })
        .take(MAX_GUARD_EXPLANATIONS)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn matching_guard_scope(
    evaluation: &ConditionEvaluation,
    loop_context: &LoopContext,
) -> Option<usize> {
    let scope = evaluation.scope.as_ref()?;
    if scope.loop_id != loop_context.loop_id {
        return None;
    }
    if scope.sequence_id.as_deref() != loop_context.sequence_id.as_deref() {
        return None;
    }
    Some(scope.iteration_index)
}

fn advance_after_current_leaf(state: &mut State) {
    if let Some(frame) = state.control_stack.last_mut() {
        if let Some(next) = frame.next_index.checked_add(1) {
            frame.next_index = next;
        } else {
            stop_with_reason(
                state,
                FinalState::Failed,
                STOP_CONTROL_INDEX_OVERFLOW,
                state.active_path.clone(),
                None,
            );
        }
    } else if let Some(next) = state.current_run_index.checked_add(1) {
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
}

fn sequence_id_from_ref(ref_text: &str) -> crate::Result<String> {
    let parsed = Reference::parse(ref_text).map_err(|_| {
        crate::procedure::invalid_field(
            "procedure.sequence.sequence",
            format!("invalid sequence ref {ref_text:?}"),
        )
    })?;
    if parsed.kind() != Kind::Sequence || parsed.is_qualified() {
        return Err(crate::procedure::invalid_field(
            "procedure.sequence.sequence",
            "sequence control refs must be local sequence:<id> refs",
        ));
    }
    Ok(parsed.id().to_string())
}
