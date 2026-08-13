// Procedure run planning.
// Procedure run planning.

// ---------------------------------------------------------------------------
// Dry planner
// ---------------------------------------------------------------------------

/// Reserved runtime-provided port IDs.
const RUNTIME_PORTS: &[&str] = &["user-prompt", "session"];

/// Allowed kinds for sequence item input refs.
const INPUT_KINDS: &[Kind] = &[Kind::Port, Kind::Slot, Kind::Resource];

/// Allowed kinds for sequence item output refs.
const OUTPUT_KINDS: &[Kind] = &[Kind::Slot, Kind::Port, Kind::Schema];

pub(crate) struct EffectiveSequenceItem<'a> {
    pub(crate) run_index: usize,
    pub(crate) declaration_index: usize,
    pub(crate) item: &'a crate::r#trait::procedure::SequenceItem,
}

pub(crate) fn effective_sequence_items(
    proc: &crate::r#trait::procedure::Model,
) -> crate::Result<Vec<EffectiveSequenceItem<'_>>> {
    let Some(sequence_order) = &proc.sequence_order else {
        return Ok(proc
            .sequence
            .iter()
            .enumerate()
            .map(|(index, item)| EffectiveSequenceItem {
                run_index: index,
                declaration_index: index,
                item,
            })
            .collect());
    };

    let mut ordered = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for (run_index, requested_id) in sequence_order.iter().enumerate() {
        if !seen_ids.insert(requested_id.as_str()) {
            return Err(crate::procedure::invalid_field(
                format!("procedure.sequence-order[{run_index}]"),
                format!("duplicate sequence item id {requested_id:?}"),
            ));
        }
        let Some((declaration_index, item)) = proc
            .sequence
            .iter()
            .enumerate()
            .find(|(_, item)| item.id.as_deref() == Some(requested_id.as_str()))
        else {
            return Err(crate::procedure::invalid_field(
                format!("procedure.sequence-order[{run_index}]"),
                format!("unknown sequence item id {requested_id:?}"),
            ));
        };
        ordered.push(EffectiveSequenceItem {
            run_index,
            declaration_index,
            item,
        });
    }

    if ordered.len() != proc.sequence.len() {
        return Err(crate::procedure::invalid_field(
            "procedure.sequence-order",
            "sequence-order must include every procedure.sequence item exactly once",
        ));
    }

    Ok(ordered)
}

fn require_ref(
    field_path: &str,
    ref_text: &str,
    allowed_kinds: &[Kind],
) -> crate::Result<Reference> {
    let parsed = Reference::parse(ref_text).map_err(|_| {
        crate::procedure::invalid_field(field_path, format!("invalid typed ref {ref_text:?}"))
    })?;
    if !allowed_kinds.contains(&parsed.kind()) {
        let expected: Vec<&str> = allowed_kinds.iter().map(|k| k.as_str()).collect();
        return Err(crate::procedure::invalid_field(
            field_path,
            format!(
                "ref kind {:?} not allowed; expected {}",
                parsed.kind(),
                expected.join(", "),
            ),
        ));
    }
    Ok(parsed)
}

fn push_port_requirement(
    port_requirements: &mut Vec<PortRequirement>,
    port_ref: Reference,
    port_id: &str,
    is_runtime: bool,
    reason: String,
) {
    port_requirements.push(PortRequirement {
        port_ref,
        port_id: port_id.to_string(),
        required: true,
        status: if is_runtime {
            PortRequirementStatus::RuntimeProvided
        } else {
            PortRequirementStatus::BindingRequired
        },
        reason,
    });
}

/// Plan a procedure run without executing anything.
///
/// Every typed ref in `procedure.input`, `procedure.sequence`,
/// `procedure.output`, and each sequence item's `input`/`output` is
/// defensively parsed and kind-validated at its field path.
///
/// **Slot tracking** uses monotonic planned output state: once any reachable
/// item can produce a slot, later global completion checks treat it as planned,
/// while each sequence item's status still records first-pass input order.
///
/// **Port requirements** are derived from `procedure.input` boundary ports.
/// Required local input ports become `runtime-provided` for reserved ports
/// (`port:user-prompt`, `port:session`), otherwise `binding-required`.
///
/// **Output ports** are resolved through `port.value = "slot:<id>"`. Required
/// output ports whose slots are not planned-produced are missing; optional
/// output ports whose slots are not planned-produced are optional-missing.
pub fn plan_procedure_run(trait_ref: &Trait, run_id: Id) -> crate::Result<Plan> {
    let proc = trait_ref.procedure.as_ref().ok_or_else(|| {
        crate::procedure::invalid_field("procedure", "trait has no [procedure] section")
    })?;

    let mut sequence_items = Vec::new();
    let mut nested_state = NestedPlanState::new(trait_ref);

    let mut port_requirements: Vec<PortRequirement> = Vec::new();

    // Build port lookup maps.
    let input_ports: BTreeMap<&str, &crate::r#trait::Port> = trait_ref
        .ports
        .iter()
        .filter(|p| matches!(p.direction, PortDirection::Input))
        .map(|p| (p.id.as_str(), p))
        .collect();
    let output_ports: BTreeMap<&str, &crate::r#trait::Port> = trait_ref
        .ports
        .iter()
        .filter(|p| matches!(p.direction, PortDirection::Output))
        .map(|p| (p.id.as_str(), p))
        .collect();

    let resource_ids: BTreeSet<&str> = trait_ref.resources.iter().map(|r| r.id.as_str()).collect();

    // --- Model input: derive port requirements ---

    for (j, ref_text) in proc.input.iter().enumerate() {
        let field = format!("procedure.input[{j}]");
        let parsed = require_ref(&field, ref_text, &[Kind::Port])?;
        if parsed.is_qualified() {
            return Err(crate::procedure::invalid_field(
                field,
                "procedure input must be a local unqualified port ref",
            ));
        }
        let port_id = parsed.id();
        let Some(port) = input_ports.get(port_id) else {
            return Err(crate::procedure::invalid_field(
                field,
                format!("procedure input port {port_id:?} is not a declared input port"),
            ));
        };
        let required = !port.optional;
        let status = if RUNTIME_PORTS.contains(&port_id) {
            PortRequirementStatus::RuntimeProvided
        } else {
            PortRequirementStatus::BindingRequired
        };
        port_requirements.push(PortRequirement {
            port_ref: parsed.clone(),
            port_id: port_id.to_string(),
            required,
            status,
            reason: format!("declared in procedure.input[{j}]"),
        });
    }

    // --- Walk sequence items ---

    for ordered_item in effective_sequence_items(proc)? {
        let seq_idx = ordered_item.declaration_index;
        let run_idx = ordered_item.run_index;
        let item = ordered_item.item;
        let field = format!("procedure.sequence[{seq_idx}]");

        // Validate sequence item inputs.
        let mut parsed_inputs: Vec<Reference> = Vec::new();
        for (j, input) in item.input.iter().enumerate() {
            let input_ref = input.ref_text();
            let inp_field = format!("{field}.input[{j}]");
            let p = require_ref(&inp_field, input_ref, INPUT_KINDS)?;
            if input.guard().is_some() && p.kind() != Kind::Resource {
                return Err(crate::procedure::invalid_field(
                    format!("{inp_field}.when"),
                    "input guard is valid only on a resource input",
                ));
            }
            if !p.is_qualified() {
                match p.kind() {
                    Kind::Port => {
                        // Local port inputs are reflected in port requirements.
                        let port_id = p.id();
                        let is_runtime = RUNTIME_PORTS.contains(&port_id);
                        if !input_ports.contains_key(port_id) {
                            return Err(crate::procedure::invalid_field(
                                inp_field,
                                format!(
                                    "sequence item input port {port_id:?} is not a declared input port"
                                ),
                            ));
                        }
                        push_port_requirement(
                            &mut port_requirements,
                            p.clone(),
                            port_id,
                            is_runtime,
                            format!("required by {field}.input[{j}]"),
                        );
                    }
                    Kind::Slot
                        if !input.is_optional()
                            && !nested_state.available_slots.contains::<str>(input_ref) =>
                    {
                        upsert_slot(&mut nested_state.slot_states, input_ref, SlotState::Missing);
                    }
                    Kind::Resource => {
                        let res_id = p.id();
                        if !resource_ids.contains(res_id) {
                            return Err(crate::procedure::invalid_field(
                                inp_field,
                                format!(
                                    "sequence item input resource {res_id:?} is not a declared local resource"
                                ),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            parsed_inputs.push(p);
        }

        // Validate outputs — must be local unqualified slot refs or direct output ports.
        let parsed_outputs: Vec<Reference> = item
            .output
            .iter()
            .enumerate()
            .map(|(j, sink)| {
                let out_field = format!("{field}.output[{j}]");
                let p = require_ref(&out_field, sink.ref_text(), OUTPUT_KINDS)?;
                if p.is_qualified() {
                    return Err(crate::procedure::invalid_field(
                        out_field,
                        "sequence output must be a local unqualified ref",
                    ));
                }
                Ok(p)
            })
            .collect::<crate::Result<_>>()?;

        let input_refs: Vec<Reference> = parsed_inputs.clone();
        let output_refs: Vec<Reference> = parsed_outputs.clone();

        let kind = item.effective_kind();
        require_executable_loop_bound(item, &field, kind)?;
        let command_plan = if matches!(kind, SequenceKind::Command | SequenceKind::Check) {
            command_plan_for_item(item, &field)?
        } else {
            None
        };

        let (planned_prompt, has_dependency_prompt) = if kind != SequenceKind::Prompt {
            (None, false)
        } else {
            let prompt_classification = classify_prompt(&item.prompt)
                .map_err(|msg| crate::procedure::invalid_field(format!("{field}.prompt"), msg))?;

            let has_dependency_prompt = matches!(
                prompt_classification,
                PromptClassification::DependencyPromptRef(_)
            );

            let planned_prompt = match prompt_classification {
                PromptClassification::Inline => PlannedPromptSource::Inline,
                PromptClassification::DependencyPromptRef(_) => {
                    PlannedPromptSource::DependencyPendingPromptRef
                }
                PromptClassification::LocalPromptRef(ref parsed) => {
                    if trait_ref.prompts.get(parsed.id()).is_none() {
                        return Err(crate::procedure::invalid_field(
                            format!("{field}.prompt"),
                            format!("unresolved local prompt ref {:?}", parsed.to_string()),
                        ));
                    }
                    PlannedPromptSource::LocalPromptRef
                }
            };
            (Some(planned_prompt), has_dependency_prompt)
        };

        // Compute status BEFORE adding outputs.
        let status = compute_item_status(
            &parsed_inputs,
            &nested_state.available_slots,
            has_dependency_prompt,
            &item.input,
        );
        let position = PlanPosition {
            parent_sequence_index: seq_idx,
            parent_run_index: run_idx,
            depth: 0,
        };
        let before_children = nested_state.clone();
        let children = plan_nested_children(
            &mut nested_state,
            &mut port_requirements,
            item.sequence.as_deref(),
            position,
        )?;
        let otherwise_children = if kind == SequenceKind::Branch {
            let then_state = nested_state.clone();
            nested_state = before_children.clone();
            let otherwise = plan_nested_children(
                &mut nested_state,
                &mut port_requirements,
                item.otherwise.as_deref(),
                position,
            )?;
            merge_branch_states(&mut nested_state, &before_children, &then_state);
            otherwise
        } else {
            Vec::new()
        };
        let parallel_branches = if kind == SequenceKind::Parallel {
            plan_parallel_branches(&mut nested_state, &mut port_requirements, item, position)?
        } else {
            Vec::new()
        };

        sequence_items.push(PlannedSequenceItem {
            sequence_index: seq_idx,
            run_index: run_idx,
            item_id: item.id.clone(),
            title: item.title.clone(),
            input_refs,
            output_refs,
            kind: match kind {
                SequenceKind::Prompt => PlannedSequenceKind::Prompt,
                SequenceKind::Ask => PlannedSequenceKind::Ask,
                SequenceKind::Command => PlannedSequenceKind::Command,
                SequenceKind::Check => PlannedSequenceKind::Check,
                SequenceKind::Project => PlannedSequenceKind::Project,
                SequenceKind::Sequence => PlannedSequenceKind::Sequence,
                SequenceKind::Branch => PlannedSequenceKind::Branch,
                SequenceKind::Loop => PlannedSequenceKind::Loop,
                SequenceKind::ForEach => PlannedSequenceKind::ForEach,
                SequenceKind::Parallel => PlannedSequenceKind::Parallel,
                SequenceKind::Terminal => PlannedSequenceKind::Terminal,
            },
            agent_ref: item.agent.as_deref().map(Reference::parse).transpose()?,
            // A top-level site: `seq_idx` IS the declaration index the
            // shared authored-site walk keys `top_level` by (P456).
            structural_seat: item.agent.as_deref().and_then(|agent_ref| {
                crate::procedure::runtime::structural_seat_for_declaration_index(
                    trait_ref, agent_ref, seq_idx,
                )
            }),
            sequence_ref: item.sequence.as_deref().map(Reference::parse).transpose()?,
            otherwise_sequence_ref: item
                .otherwise
                .as_deref()
                .map(Reference::parse)
                .transpose()?,
            prompt_source: planned_prompt,
            command_plan,
            children,
            otherwise_children,
            parallel_branches,
            max_branches: item.max_branches,
            join: item.join.clone(),
            branch_failure: item.branch_failure.clone(),
            concurrent: item.concurrent,
            status,
        });

        // After status: producer edges + output state for local outputs.
        for out_ref in &parsed_outputs {
            apply_planned_output(
                out_ref.as_str(),
                &mut nested_state,
                PlanPosition {
                    parent_sequence_index: seq_idx,
                    parent_run_index: run_idx,
                    depth: 0,
                },
            );
        }
    }

    // --- Output ports: resolve through port.value or direct port output ---

    let mut output_port_records: Vec<PlannedOutputPort> = Vec::new();

    for (j, ref_text) in proc.output.iter().enumerate() {
        let field = format!("procedure.output[{j}]");
        let parsed = require_ref(&field, ref_text, &[Kind::Port])?;
        if parsed.is_qualified() {
            return Err(crate::procedure::invalid_field(
                field,
                "procedure output must be a local unqualified port ref",
            ));
        }
        let port_id = parsed.id();
        let Some(port) = output_ports.get(port_id) else {
            return Err(crate::procedure::invalid_field(
                field,
                format!("procedure output port {port_id:?} is not a declared output port"),
            ));
        };

        let value_ref = if let Some(ref value) = port.value {
            Reference::parse(value).map_err(|_| {
                crate::procedure::invalid_field(
                    format!("port[{}].value", port_id),
                    format!("invalid output port value ref {value:?}"),
                )
            })?
        } else {
            parsed.clone()
        };
        let required = !port.optional;

        let planned_produced = nested_state
            .direct_output_ports
            .contains(value_ref.as_str())
            || nested_state
                .slot_states
                .get(value_ref.as_str())
                .is_some_and(|state| *state == SlotState::PlannedProduced);
        let status = if planned_produced {
            OutputPortStatus::PlannedProduced
        } else if required {
            OutputPortStatus::Missing
        } else {
            OutputPortStatus::OptionalMissing
        };

        output_port_records.push(PlannedOutputPort {
            port_ref: parsed.clone(),
            value_slot_ref: value_ref,
            required,
            status,
            reason: format!("resolved from procedure.output[{j}]"),
        });
    }

    // --- Convert ---

    let slots: Vec<PlannedSlot> = nested_state
        .slot_states
        .into_iter()
        .map(|(slot_ref, state)| {
            Ok(PlannedSlot {
                slot_ref: Reference::parse(&slot_ref)?,
                state,
            })
        })
        .collect::<crate::Result<_>>()?;

    Ok(Plan {
        run_id,
        trait_id: trait_ref.id.as_str().to_string(),
        worktree_required: proc.worktree_required,
        sequence_items,
        slots,
        producer_edges: nested_state.producer_edges,
        port_requirements,
        output_ports: output_port_records,
        session_title_sink: trait_ref.sinks.session_title.clone(),
        acceptance: AcceptanceState::Pending,
    })
}

fn compute_item_status(
    input_refs: &[Reference],
    available_slots: &BTreeSet<String>,
    has_dependency_prompt: bool,
    inputs: &crate::r#trait::procedure::SequenceInputList,
) -> SequenceItemStatus {
    for r in input_refs {
        if r.is_qualified() {
            continue;
        }
        if r.kind() == Kind::Slot {
            let ref_text = r.to_string();
            if !available_slots.contains(&ref_text) && !inputs.is_optional_for(&ref_text) {
                return SequenceItemStatus::Blocked;
            }
        }
    }

    let has_dep_input = input_refs.iter().any(|r| {
        r.is_qualified()
            && matches!(r.kind(), Kind::Port | Kind::Slot | Kind::Resource)
            && !(r.kind() == Kind::Slot && inputs.is_optional_for(r.as_ref()))
    });

    if has_dep_input || has_dependency_prompt {
        return SequenceItemStatus::DependencyPending;
    }

    SequenceItemStatus::Planned
}

#[derive(Clone)]
struct NestedPlanState<'a> {
    trait_ref: &'a Trait,
    available_slots: BTreeSet<String>,
    direct_output_ports: BTreeSet<String>,
    slot_states: BTreeMap<String, SlotState>,
    producer_edges: Vec<ProducerEdge>,
    // Named-sequence planning is graph-shaped, not tree-shaped. The first
    // expansion owns the child subtree and global slot/output effects; repeated
    // references remain as their parent `sequence_ref` markers to avoid
    // exponential diamond clones. No separate effect cache is needed because
    // dry-plan slot/output state is monotonic and global for the procedure.
    expanded_sequences: BTreeSet<String>,
    stack: Vec<String>,
}

fn merge_branch_states(
    state: &mut NestedPlanState<'_>,
    before: &NestedPlanState<'_>,
    then_state: &NestedPlanState<'_>,
) {
    state.available_slots = state
        .available_slots
        .intersection(&then_state.available_slots)
        .cloned()
        .collect();
    state.direct_output_ports = state
        .direct_output_ports
        .intersection(&then_state.direct_output_ports)
        .cloned()
        .collect();
    state
        .slot_states
        .retain(|slot, _| state.available_slots.contains(slot));
    let mut arm_edges: Vec<_> = then_state
        .producer_edges
        .iter()
        .chain(state.producer_edges.iter())
        .filter(|edge| !before.producer_edges.contains(edge))
        .cloned()
        .collect();
    arm_edges.sort_by(|left, right| {
        (left.run_index, left.sequence_index, left.slot_ref.as_str()).cmp(&(
            right.run_index,
            right.sequence_index,
            right.slot_ref.as_str(),
        ))
    });
    arm_edges.dedup();
    state.producer_edges = before.producer_edges.clone();
    state.producer_edges.extend(arm_edges);
    state.expanded_sequences = state
        .expanded_sequences
        .intersection(&then_state.expanded_sequences)
        .cloned()
        .collect();
}

/// Dry-plan each parallel branch from a clone of the SAME pre-panel state so no
/// branch observes another's output, then merge the union of branch effects
/// once all branches are planned. Authored order and refs are preserved.
fn plan_parallel_branches(
    state: &mut NestedPlanState<'_>,
    port_requirements: &mut Vec<PortRequirement>,
    item: &crate::r#trait::procedure::SequenceItem,
    position: PlanPosition,
) -> crate::Result<Vec<PlannedParallelBranch>> {
    let before = state.clone();
    let mut merged = before.clone();
    let mut branches = Vec::new();
    for branch_ref in item.branches.iter() {
        let mut branch_state = before.clone();
        let children = plan_nested_children(
            &mut branch_state,
            port_requirements,
            Some(branch_ref),
            position,
        )?;
        union_parallel_branch(&mut merged, &before, &branch_state);
        branches.push(PlannedParallelBranch {
            sequence_ref: Reference::parse(branch_ref)?,
            children,
        });
    }
    *state = merged;
    Ok(branches)
}

/// Merge one planned parallel branch's effects into the shared post-panel state.
/// Every branch runs, so slot/output/producer effects are unioned (not
/// intersected as with branch arms).
fn union_parallel_branch(
    merged: &mut NestedPlanState<'_>,
    before: &NestedPlanState<'_>,
    branch: &NestedPlanState<'_>,
) {
    merged
        .available_slots
        .extend(branch.available_slots.iter().cloned());
    merged
        .direct_output_ports
        .extend(branch.direct_output_ports.iter().cloned());
    for (slot, slot_state) in &branch.slot_states {
        upsert_slot(&mut merged.slot_states, slot, slot_state.clone());
    }
    for edge in &branch.producer_edges {
        if !before.producer_edges.contains(edge) && !merged.producer_edges.contains(edge) {
            merged.producer_edges.push(edge.clone());
        }
    }
    merged
        .expanded_sequences
        .extend(branch.expanded_sequences.iter().cloned());
}

impl<'a> NestedPlanState<'a> {
    fn new(trait_ref: &'a Trait) -> Self {
        Self {
            trait_ref,
            available_slots: BTreeSet::new(),
            direct_output_ports: BTreeSet::new(),
            slot_states: BTreeMap::new(),
            producer_edges: Vec::new(),
            expanded_sequences: BTreeSet::new(),
            stack: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct PlanPosition {
    parent_sequence_index: usize,
    parent_run_index: usize,
    depth: usize,
}

fn plan_nested_children(
    state: &mut NestedPlanState<'_>,
    port_requirements: &mut Vec<PortRequirement>,
    sequence_ref: Option<&str>,
    position: PlanPosition,
) -> crate::Result<Vec<PlannedSequenceItem>> {
    let Some(sequence_ref) = sequence_ref else {
        return Ok(Vec::new());
    };
    let parsed = Reference::parse(sequence_ref).map_err(|_| {
        crate::procedure::invalid_field(
            "procedure.sequence.sequence",
            format!("invalid sequence ref {sequence_ref:?}"),
        )
    })?;
    if parsed.kind() != Kind::Sequence || parsed.is_qualified() {
        return Ok(Vec::new());
    }
    let sequence_id = parsed.id().to_string();
    if position.depth > crate::r#trait::procedure::MAX_SEQUENCE_NESTING_DEPTH {
        return Err(crate::procedure::invalid_field(
            format!("sequence.{sequence_id}.sequence"),
            format!(
                "dry-plan nested sequence depth exceeds maximum {}",
                crate::r#trait::procedure::MAX_SEQUENCE_NESTING_DEPTH
            ),
        ));
    }
    if state.expanded_sequences.contains(&sequence_id) {
        return Ok(Vec::new());
    }
    if state.stack.iter().any(|id| id == &sequence_id) {
        return Err(crate::procedure::invalid_field(
            format!("sequence.{sequence_id}"),
            "recursive/cyclic sequence refs are not allowed in dry-plan",
        ));
    }
    let Some(named) = state.trait_ref.sequences.get(sequence_id.as_str()) else {
        return Ok(Vec::new());
    };
    let named_items = named.sequence.clone();

    state.stack.push(sequence_id.clone());
    let mut children = Vec::new();
    for (index, item) in named_items.iter().enumerate() {
        let field = format!("sequence.{sequence_id}.sequence[{index}]");
        let mut parsed_inputs = Vec::new();
        for seq_input in item.input.iter() {
            let input_ref = seq_input.ref_text();
            if let Ok(input) = Reference::parse(input_ref) {
                if input.kind() == Kind::Port && !input.is_qualified() {
                    let port_id = input.id();
                    let is_runtime = RUNTIME_PORTS.contains(&port_id);
                    push_port_requirement(
                        port_requirements,
                        input.clone(),
                        port_id,
                        is_runtime,
                        format!("required by {field}.input"),
                    );
                }
                if input.kind() == Kind::Slot
                    && !input.is_qualified()
                    && !seq_input.is_optional()
                    && !state.available_slots.contains(input_ref)
                {
                    upsert_slot(&mut state.slot_states, input_ref, SlotState::Missing);
                }
                parsed_inputs.push(input);
            }
        }
        let kind = item.effective_kind();
        require_executable_loop_bound(item, &field, kind)?;
        let command_plan = if matches!(kind, SequenceKind::Command | SequenceKind::Check) {
            command_plan_for_item(item, &field)?
        } else {
            None
        };
        let (prompt_source, has_dependency_prompt) = if kind == SequenceKind::Prompt {
            match classify_prompt(&item.prompt).map_err(|message| {
                crate::procedure::invalid_field(format!("{field}.prompt"), message)
            })? {
                PromptClassification::Inline => (Some(PlannedPromptSource::Inline), false),
                PromptClassification::DependencyPromptRef(_) => {
                    (Some(PlannedPromptSource::DependencyPendingPromptRef), true)
                }
                PromptClassification::LocalPromptRef(_) => {
                    (Some(PlannedPromptSource::LocalPromptRef), false)
                }
            }
        } else {
            (None, false)
        };
        let status = compute_item_status(
            &parsed_inputs,
            &state.available_slots,
            has_dependency_prompt,
            &item.input,
        );
        let child_position = PlanPosition {
            depth: position.depth + 1,
            ..position
        };
        let before_children = state.clone();
        let nested_children = plan_nested_children(
            state,
            port_requirements,
            item.sequence.as_deref(),
            child_position,
        )?;
        let otherwise_children = if kind == SequenceKind::Branch {
            let then_state = state.clone();
            *state = before_children.clone();
            let otherwise = plan_nested_children(
                state,
                port_requirements,
                item.otherwise.as_deref(),
                child_position,
            )?;
            merge_branch_states(state, &before_children, &then_state);
            otherwise
        } else {
            Vec::new()
        };
        let parallel_branches = if kind == SequenceKind::Parallel {
            plan_parallel_branches(state, port_requirements, item, child_position)?
        } else {
            Vec::new()
        };
        let output_refs: Vec<Reference> = item
            .output
            .iter()
            .filter_map(|sink| Reference::parse(sink.ref_text()).ok())
            .collect();
        for output_ref in item.output.ref_texts() {
            apply_planned_output(output_ref, state, position);
        }
        children.push(PlannedSequenceItem {
            sequence_index: index,
            run_index: position.parent_run_index,
            item_id: item.id.clone(),
            title: item.title.clone(),
            input_refs: parsed_inputs,
            output_refs,
            kind: match kind {
                SequenceKind::Prompt => PlannedSequenceKind::Prompt,
                SequenceKind::Ask => PlannedSequenceKind::Ask,
                SequenceKind::Command => PlannedSequenceKind::Command,
                SequenceKind::Check => PlannedSequenceKind::Check,
                SequenceKind::Project => PlannedSequenceKind::Project,
                SequenceKind::Sequence => PlannedSequenceKind::Sequence,
                SequenceKind::Branch => PlannedSequenceKind::Branch,
                SequenceKind::Loop => PlannedSequenceKind::Loop,
                SequenceKind::ForEach => PlannedSequenceKind::ForEach,
                SequenceKind::Parallel => PlannedSequenceKind::Parallel,
                SequenceKind::Terminal => PlannedSequenceKind::Terminal,
            },
            agent_ref: item.agent.as_deref().map(Reference::parse).transpose()?,
            // `sequence_id` (this named body's own id) plus `index` (the
            // site's position within it) is the same path-independent
            // identity the runtime declaration walk keys nested sites by
            // (P456).
            structural_seat: item.agent.as_deref().and_then(|agent_ref| {
                crate::procedure::runtime::structural_seat_for_nested_declaration(
                    state.trait_ref,
                    agent_ref,
                    &sequence_id,
                    index,
                )
            }),
            sequence_ref: item.sequence.as_deref().map(Reference::parse).transpose()?,
            otherwise_sequence_ref: item
                .otherwise
                .as_deref()
                .map(Reference::parse)
                .transpose()?,
            prompt_source,
            command_plan,
            children: nested_children,
            otherwise_children,
            parallel_branches,
            max_branches: item.max_branches,
            join: item.join.clone(),
            branch_failure: item.branch_failure.clone(),
            concurrent: item.concurrent,
            status,
        });
    }
    state.stack.pop();
    state.expanded_sequences.insert(sequence_id);
    Ok(children)
}

fn require_executable_loop_bound(
    item: &crate::r#trait::procedure::SequenceItem,
    field: &str,
    kind: SequenceKind,
) -> crate::Result<()> {
    if kind == SequenceKind::Loop
        && item.max_iterations.is_none()
        && item.max_iterations_from.is_none()
        && item.until.is_none()
        && item.abort_if.is_none()
    {
        let step = item.id.as_deref().unwrap_or("unnamed");
        return Err(crate::procedure::invalid_field(
            format!("{field}.max-iterations"),
            format!("loop step {step:?} is unbounded and will not run"),
        ));
    }
    Ok(())
}

fn apply_planned_output(output_ref: &str, state: &mut NestedPlanState<'_>, position: PlanPosition) {
    let Some(output) = Reference::parse(output_ref).ok() else {
        return;
    };
    if output.is_qualified() {
        return;
    }
    match output.kind() {
        Kind::Slot => {
            state.producer_edges.push(ProducerEdge {
                sequence_index: position.parent_sequence_index,
                run_index: position.parent_run_index,
                slot_ref: output.clone(),
            });
            state.available_slots.insert(output_ref.to_string());
            upsert_slot(
                &mut state.slot_states,
                output_ref,
                SlotState::PlannedProduced,
            );
        }
        Kind::Port => {
            state.direct_output_ports.insert(output_ref.to_string());
        }
        Kind::Schema => {}
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Guidance-only outcome
// ---------------------------------------------------------------------------

/// The outcome of runtime planning for an arbitrary valid trait.
///
/// Guidance-only traits (no `[procedure]`) produce a `GuidanceOnly` outcome
/// with no sequence items, slots, or port requirements. Model-backed
/// traits produce a `Planned` outcome containing the full dry plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum TraitPlan {
    /// The trait has no procedure. It may still provide guidance, prompts,
    /// behavior, or metadata.
    GuidanceOnly {
        /// The trait ID.
        trait_id: String,
        /// Why no procedure plan was produced.
        reason: String,
    },
    /// The trait has a procedure and a dry plan was produced.
    Planned(Plan),
}

/// Plan a trait's runtime without executing anything.
///
/// For guidance-only traits (no `[procedure]`), returns
/// [`TraitPlan::GuidanceOnly`] — not an error. For procedure-backed
/// traits, delegates to [`plan_procedure_run`] and returns
/// [`TraitPlan::Planned`].
pub fn plan_trait_runtime(trait_ref: &Trait, run_id: Id) -> crate::Result<TraitPlan> {
    if trait_ref.procedure.is_none() {
        return Ok(TraitPlan::GuidanceOnly {
            trait_id: trait_ref.id.as_str().to_string(),
            reason: "no procedure declared".to_string(),
        });
    }
    let plan = plan_procedure_run(trait_ref, run_id)?;
    Ok(TraitPlan::Planned(plan))
}

#[cfg(test)]
mod session_title_sink_plan_tests {
    use super::plan_procedure_run;
    use crate::encoding::{Encoding, decode_trait};
    use crate::procedure::run::Id;

    fn decode(text: &str) -> crate::r#trait::Trait {
        decode_trait(Encoding::Toml, text).expect("decode")
    }

    fn run_id() -> Id {
        Id::new("plan-sink-run".to_string()).expect("run id")
    }

    /// 0110: dry-run performs no effects, but a declared sink must still be
    /// visible in the plan — `plan_procedure_run` is a pure function (no
    /// ledger write, no dispatch reachable from it at all), so surfacing the
    /// declaration here is itself the "no effect" guarantee, not merely a
    /// display nicety.
    #[test]
    fn a_declared_sink_appears_in_the_plan_with_no_effect() {
        const FIXTURE: &str = r#"
id = "plan-sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Plan Sink Fixture"
description = "Scratch fixture for the 0110 dry-plan sink surface."

[[slot]]
id = "done"
schema = "schema:text"

[procedure]
description = "One step."

[[procedure.sequence]]
id = "noop"
title = "Noop"
kind = "command"
cmd = "true"
output = ["slot:done"]

[sink.session-title]
mode = "verbatim"
input = "Fixed session title"
"#;
        let trait_ref = decode(FIXTURE);
        let plan = plan_procedure_run(&trait_ref, run_id()).expect("plan");
        let sink = plan.session_title_sink.expect("sink surfaced in the plan");
        assert_eq!(sink.mode, crate::r#trait::SinkMode::Verbatim);
    }

    #[test]
    fn no_declaration_means_no_sink_in_the_plan() {
        const FIXTURE: &str = r#"
id = "plan-no-sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Plan No Sink Fixture"
description = "Scratch fixture for the 0110 dry-plan sink surface."

[[slot]]
id = "done"
schema = "schema:text"

[procedure]
description = "One step."

[[procedure.sequence]]
id = "noop"
title = "Noop"
kind = "command"
cmd = "true"
output = ["slot:done"]
"#;
        let trait_ref = decode(FIXTURE);
        let plan = plan_procedure_run(&trait_ref, run_id()).expect("plan");
        assert!(plan.session_title_sink.is_none());
    }
}
