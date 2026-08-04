// Validates trait procedure declarations.
// Procedure validation.

pub fn validate(t: &Trait) -> crate::Result<()> {
    let input_port_ids: BTreeSet<&str> = t
        .ports
        .iter()
        .filter(|p| matches!(p.direction, crate::r#trait::PortDirection::Input))
        .map(|p| p.id.as_str())
        .collect();
    let output_port_ids: BTreeSet<&str> = t
        .ports
        .iter()
        .filter(|p| matches!(p.direction, crate::r#trait::PortDirection::Output))
        .map(|p| p.id.as_str())
        .collect();
    let slot_ids: BTreeSet<&str> = t.slots.iter().map(|slot| slot.id.as_str()).collect();
    let signal_ids: BTreeSet<&str> = t.signals.iter().map(|signal| signal.id.as_str()).collect();
    let agent_ids: BTreeSet<&str> = t.agents.iter().map(|agent| agent.id.as_str()).collect();
    let resource_ids: BTreeSet<&str> = t.resources.iter().map(|r| r.id.as_str()).collect();
    let sequence_sets = SequenceValidationSets {
        input_port_ids: &input_port_ids,
        output_port_ids: &output_port_ids,
        slot_ids: &slot_ids,
        signal_ids: &signal_ids,
        agent_ids: &agent_ids,
        resource_ids: &resource_ids,
    };

    validate_named_sequences(t, &sequence_sets)?;
    validate_sequence_graph(t)?;

    let Some(ref proc) = t.procedure else {
        reject_unbound_named_failure_routes(t, &BTreeSet::new())?;
        return Ok(());
    };

    if proc.description.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "procedure.description".to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    if proc.sequence.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "procedure.sequence".to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    // Validate boundary input refs: must be local unqualified port:* of input direction.
    for (j, ref_text) in proc.input.iter().enumerate() {
        let parsed =
            Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("procedure.input[{j}]"),
                message: format!("invalid typed ref {ref_text:?}"),
            })?;
        if !PROCEDURE_INPUT_KINDS.contains(&parsed.kind()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.input[{j}]"),
                message: format!(
                    "procedure input ref kind {:?} not allowed; expected port",
                    parsed.kind()
                ),
            }
            .into());
        }
        if parsed.is_qualified() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.input[{j}]"),
                message: "procedure input must be a local unqualified port ref".to_string(),
            }
            .into());
        }
        if !input_port_ids.contains(parsed.id()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.input[{j}]"),
                message: format!(
                    "procedure input port {:?} is not a declared input-direction port",
                    parsed.id()
                ),
            }
            .into());
        }
    }

    // Validate boundary output refs: must be local unqualified port:* of output direction.
    for (j, ref_text) in proc.output.iter().enumerate() {
        let parsed =
            Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("procedure.output[{j}]"),
                message: format!("invalid typed ref {ref_text:?}"),
            })?;
        if !PROCEDURE_OUTPUT_KINDS.contains(&parsed.kind()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.output[{j}]"),
                message: format!(
                    "procedure output ref kind {:?} not allowed; expected port",
                    parsed.kind()
                ),
            }
            .into());
        }
        if parsed.is_qualified() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.output[{j}]"),
                message: "procedure output must be a local unqualified port ref".to_string(),
            }
            .into());
        }
        if !output_port_ids.contains(parsed.id()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.output[{j}]"),
                message: format!(
                    "procedure output port {:?} is not a declared output-direction port",
                    parsed.id()
                ),
            }
            .into());
        }
    }

    // Validate sequence items.
    let mut seen_ids = BTreeSet::new();
    for (i, item) in proc.sequence.iter().enumerate() {
        let base = format!("procedure.sequence[{i}]");
        if let Some(ref id) = item.id {
            crate::shared::validate_slug_shape(id, &format!("{base}.id"))?;
            if !seen_ids.insert(id.clone()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.id"),
                    message: format!("duplicate sequence item id {id:?}"),
                }
                .into());
            }
        }
        validate_sequence_item_declaration(t, item, &base, &sequence_sets)?;
    }

    // Enforce output-port completion: each procedure.output[] port is either
    // backed by a produced slot through port.value or produced directly as a
    // terminal output-port ref.
    let produced_refs = collect_produced_refs(t);

    for (j, ref_text) in proc.output.iter().enumerate() {
        let port_id = Reference::parse(ref_text)
            .ok()
            .filter(|p| p.kind() == Kind::Port && !p.is_qualified())
            .map(|p| p.id().to_string());
        let Some(port_id) = port_id else {
            continue;
        };
        let Some(port) = t.ports.iter().find(|p| p.id == port_id) else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.output[{j}]"),
                message: format!("output port {port_id:?} not found among declared ports"),
            }
            .into());
        };
        if let Some(ref value) = port.value {
            let value_parsed =
                Reference::parse(value).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: format!("port[{}].value", port_id),
                    message: format!("invalid output port value ref {value:?}"),
                })?;
            if value_parsed.is_qualified() || value_parsed.kind() != Kind::Slot {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("port[{}].value", port_id),
                    message: "output port value must be a local unqualified slot ref".to_string(),
                }
                .into());
            }
            validate_slot_backed_port_schema(t, &port_id, value_parsed.as_ref())?;
            if !port.optional && !produced_refs.contains(&value_parsed.to_string()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("procedure.output[{j}]"),
                    message: format!(
                        "required output port {:?} value slot {:?} is not produced by any procedure sequence output",
                        port_id,
                        value_parsed.to_string()
                    ),
                }.into());
            }
        } else {
            let direct_ref = format!("port:{port_id}");
            if !port.optional && !produced_refs.contains(&direct_ref) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("procedure.output[{j}]"),
                    message: format!(
                        "required output port {:?} is neither slot-backed nor directly produced by a terminal sequence output",
                        port_id
                    ),
                }.into());
            }
        }
    }

    // Validate sequence-order if present.
    let ordered = ordered_procedure_items(proc)?;
    validate_failure_routes(t, &ordered, &signal_ids)?;

    validate_produced_before_read(t)?;

    Ok(())
}

struct SequenceValidationSets<'a> {
    input_port_ids: &'a BTreeSet<&'a str>,
    output_port_ids: &'a BTreeSet<&'a str>,
    slot_ids: &'a BTreeSet<&'a str>,
    signal_ids: &'a BTreeSet<&'a str>,
    agent_ids: &'a BTreeSet<&'a str>,
    resource_ids: &'a BTreeSet<&'a str>,
}

fn validate_named_sequences(
    trait_ref: &Trait,
    sets: &SequenceValidationSets<'_>,
) -> crate::Result<()> {
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        let base = format!("sequence.{sequence_id}");
        crate::shared::validate_slug_shape(sequence_id, &base)?;
        if sequence.sequence.is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.sequence"),
                message: "named sequence must contain at least one item".to_string(),
            }
            .into());
        }
        if let Some(description) = sequence.description.as_deref()
            && description.trim().is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.description"),
                    message: "must not be empty".to_string(),
                }
                .into());
            }
        let mut seen_ids = BTreeSet::new();
        for (index, item) in sequence.sequence.iter().enumerate() {
            let item_base = format!("{base}.sequence[{index}]");
            if let Some(id) = item.id.as_deref() {
                crate::shared::validate_slug_shape(id, &format!("{item_base}.id"))?;
                if !seen_ids.insert(id.to_string()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{item_base}.id"),
                        message: format!("duplicate named-sequence item id {id:?}"),
                    }
                    .into());
                }
            }
            validate_sequence_item_declaration(trait_ref, item, &item_base, sets)?;
        }
    }
    Ok(())
}

fn validate_sequence_item_declaration(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    sets: &SequenceValidationSets<'_>,
) -> crate::Result<()> {
    if !item.title.is_empty() && item.title.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.title"),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    let kind = item.effective_kind();
    validate_item_shape(item, kind, base)?;
    validate_item_refs(trait_ref, item, kind, base, sets)?;

    match kind {
        SequenceKind::Prompt => {
            validate_sequence_item_prompt_contract(
                base,
                item.id.as_deref(),
                item,
                &trait_ref.prompts,
            )?;
        }
        SequenceKind::Ask => {
            let when = item.when.as_ref().ok_or_else(|| crate::manifest::Error::InvalidField {
                field_path: format!("{base}.when"),
                message: "ask sequence item must declare a signal guard".to_string(),
            })?;
            let is_signal_guard = matches!(when, GuardExpr::Ref(ref_text) if Reference::parse(ref_text)
                .is_ok_and(|reference| reference.kind() == Kind::Signal && !reference.is_qualified()));
            if !is_signal_guard {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.when"),
                    message: "ask sequence item when must be a declared local signal:<id> guard".to_string(),
                }.into());
            }
            crate::r#trait::condition::validate_guard_expr(
                trait_ref, when, &format!("{base}.when"), sets.slot_ids, sets.signal_ids, false, false,
            )?;
            if item.output.len() != 1 {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output"),
                    message: "ask sequence items must declare exactly one output slot".to_string(),
                }.into());
            }
            let output = item.output.iter().next().expect("count checked");
            if *output.operation() != WriteOperation::Replace {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: "ask output slot must use replace write semantics".to_string(),
                }.into());
            }
            validate_local_slot_ref(output.ref_text(), &format!("{base}.output[0]"), sets.slot_ids)?;
            validate_sequence_item_prompt_contract(base, item.id.as_deref(), item, &trait_ref.prompts)?;
        }
        SequenceKind::Command => {
            let plan = command_plan_for_item(item, base)?;
            if let Some(plan) = plan.as_ref() {
                if plan.argv_from.is_some() {
                    validate_command_argv_from(trait_ref, item, plan.argv_from.as_deref(), base)?;
                } else {
                    validate_sequence_item_command_contract(trait_ref, item, &plan.argv, base)?;
                }
                validate_command_executable_digest_from(
                    trait_ref,
                    item,
                    plan.executable_digest_from.as_deref(),
                    base,
                )?;
            }
            if item.output.len() != 1 {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output"),
                    message: "command-backed sequence items must declare exactly one output slot"
                        .to_string(),
                }
                .into());
            }
            let output = item.output.iter().next().expect("count checked");
            let parsed = Reference::parse(output.ref_text()).map_err(|_| {
                crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: format!("invalid typed ref {:?}", output.ref_text()),
                }
            })?;
            if parsed.kind() != Kind::Slot {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: "command-backed sequence items must output to a slot".to_string(),
                }
                .into());
            }
        }
        SequenceKind::Check => {
            let plan = command_plan_for_item(item, base)?;
            let Some(plan) = plan.as_ref() else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "check sequence item must declare cmd or command".to_string(),
                }
                .into());
            };
            if plan.argv_from.is_some() {
                validate_command_argv_from(trait_ref, item, plan.argv_from.as_deref(), base)?;
            } else {
                validate_sequence_item_command_contract(trait_ref, item, &plan.argv, base)?;
            }
            validate_command_executable_digest_from(
                trait_ref,
                item,
                plan.executable_digest_from.as_deref(),
                base,
            )?;
            if item.output.len() != 1 {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output"),
                    message: "check sequence items must declare exactly one output slot"
                        .to_string(),
                }
                .into());
            }
            let output = item.output.iter().next().expect("count checked");
            if *output.operation() != WriteOperation::Replace {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: "check output slot must use replace write semantics".to_string(),
                }
                .into());
            }
            let output_ref = validate_local_slot_ref(
                output.ref_text(),
                &format!("{base}.output[0]"),
                sets.slot_ids,
            )?;
            let schema_ref = local_slot_schema(trait_ref, output_ref.id());
            if let Some(problem) = check_output_schema_problem(trait_ref, schema_ref.as_deref()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: problem,
                }
                .into());
            }
        }
        SequenceKind::Project => validate_project_item(trait_ref, item, base, sets.slot_ids)?,
        SequenceKind::Sequence => validate_sequence_ref(
            trait_ref,
            item.sequence.as_deref(),
            &format!("{base}.sequence"),
        )?,
        SequenceKind::Branch => {
            validate_sequence_ref(
                trait_ref,
                item.sequence.as_deref(),
                &format!("{base}.sequence"),
            )?;
            if item.otherwise.is_some() {
                validate_sequence_ref(
                    trait_ref,
                    item.otherwise.as_deref(),
                    &format!("{base}.otherwise"),
                )?;
            }
            let when = item
                .when
                .as_ref()
                .ok_or_else(|| crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.when"),
                    message: "branch sequence item must declare when".to_string(),
                })?;
            crate::r#trait::condition::validate_guard_expr(
                trait_ref,
                when,
                &format!("{base}.when"),
                sets.slot_ids,
                sets.signal_ids,
                false,
                false,
            )?;
        }
        SequenceKind::Loop => {
            validate_loop_item(trait_ref, item, base, sets.slot_ids, sets.signal_ids)?
        }
        SequenceKind::ForEach => {
            validate_for_each_item(trait_ref, item, base, sets.slot_ids, sets.signal_ids)?
        }
        SequenceKind::Parallel => validate_parallel_item(trait_ref, item, base, sets)?,
    }
    Ok(())
}

fn validate_parallel_item(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    sets: &SequenceValidationSets<'_>,
) -> crate::Result<()> {
    let max_branches = item
        .max_branches
        .ok_or_else(|| crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-branches"),
            message: "parallel sequence item must declare max-branches".to_string(),
        })?;
    if max_branches == 0 {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-branches"),
            message: "max-branches must be greater than zero".to_string(),
        }
        .into());
    }
    if item.branches.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.branches"),
            message: "parallel sequence item must declare at least one branch".to_string(),
        }
        .into());
    }
    if item.branches.as_slice().len() > max_branches {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.branches"),
            message: format!(
                "parallel declares {} branches but max-branches is {max_branches}",
                item.branches.as_slice().len()
            ),
        }
        .into());
    }
    let mut seen_branches = BTreeSet::new();
    for (j, branch_ref) in item.branches.iter().enumerate() {
        validate_sequence_ref(
            trait_ref,
            Some(branch_ref),
            &format!("{base}.branches[{j}]"),
        )?;
        if !seen_branches.insert(branch_ref.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.branches[{j}]"),
                message: format!("duplicate parallel branch ref {branch_ref:?}"),
            }
            .into());
        }
    }
    validate_parallel_independence(trait_ref, item, base)?;
    validate_parallel_join(trait_ref, item, base, sets.slot_ids, sets.signal_ids)?;
    validate_parallel_branch_failure(item, base)?;
    Ok(())
}

/// Validate the optional barrier join policy: local slot shapes, the write
/// operation restricted to `merge`/`set-field`, schema compatibility between
/// `source` and `destination`, the quorum guard, and that no branch writes
/// the join's own `destination`.
fn validate_parallel_join(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    slot_ids: &BTreeSet<&str>,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let Some(join) = item.join.as_ref() else {
        return Ok(());
    };
    let join_base = format!("{base}.join");
    match join {
        JoinPolicy::CollectInOrder => Ok(()),
        JoinPolicy::ReduceMerge {
            destination,
            source,
            operation,
        } => {
            if !matches!(
                operation,
                WriteOperation::Merge | WriteOperation::SetField(_)
            ) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{join_base}.operation"),
                    message: "reduce-merge join operation must be merge or set-field".to_string(),
                }
                .into());
            }
            let destination_parsed = validate_local_slot_ref(
                destination,
                &format!("{join_base}.destination"),
                slot_ids,
            )?;
            let source_parsed =
                validate_local_slot_ref(source, &format!("{join_base}.source"), slot_ids)?;
            let sink = OutputSink::SlotOperation {
                slot: destination.clone(),
                operation: operation.clone(),
            };
            validate_output_sink_operation(
                trait_ref,
                &sink,
                &destination_parsed,
                &format!("{join_base}.destination"),
            )?;
            let destination_schema = local_slot_schema(trait_ref, destination_parsed.id());
            let source_schema = local_slot_schema(trait_ref, source_parsed.id());
            match operation {
                WriteOperation::Merge if destination_schema != source_schema => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{join_base}.source"),
                        message: format!(
                            "merge join requires source schema {destination_schema:?} to exactly match destination schema, got {source_schema:?}"
                        ),
                    }
                    .into());
                }
                WriteOperation::Merge => {}
                WriteOperation::SetField(field) => {
                    if let Some(destination_schema) = destination_schema.as_deref()
                        && let Some(fields) = object_schema_fields(trait_ref, destination_schema)
                            && let Some(field_schema) =
                                fields.get(field).map(|declared| declared.schema.clone())
                                && Some(field_schema) != source_schema {
                                    return Err(crate::manifest::Error::InvalidField {
                                        field_path: format!("{join_base}.source"),
                                        message: format!(
                                            "set-field join requires source schema to match destination field {field:?} schema"
                                        ),
                                    }
                                    .into());
                                }
                }
                _ => {}
            }
            validate_join_destination_not_branch_output(trait_ref, item, destination, &join_base)?;
            Ok(())
        }
        JoinPolicy::QuorumVerdict {
            destination,
            source,
            guard,
        } => {
            let destination_parsed = validate_local_slot_ref(
                destination,
                &format!("{join_base}.destination"),
                slot_ids,
            )?;
            validate_local_slot_ref(source, &format!("{join_base}.source"), slot_ids)?;
            let destination_schema = local_slot_schema(trait_ref, destination_parsed.id());
            let is_list = destination_schema
                .as_deref()
                .and_then(list_element_schema)
                .is_some();
            if !is_list {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{join_base}.destination"),
                    message: format!(
                        "quorum-verdict join requires a list-typed destination slot schema, got {destination_schema:?}"
                    ),
                }
                .into());
            }
            crate::r#trait::condition::validate_guard_expr(
                trait_ref,
                guard,
                &format!("{join_base}.guard"),
                slot_ids,
                signal_ids,
                false,
                false,
            )?;
            validate_join_destination_not_branch_output(trait_ref, item, destination, &join_base)?;
            Ok(())
        }
    }
}

/// P565: a check's output slot must be the two-field verdict record — `ok`
/// (boolean) plus `argv` (list of text) — matching what
/// `procedure::session::check_output_value` writes.
///
/// It used to be a bare `schema:boolean`, and that missing argv is what let
/// three runs exhaust: a consumer handed `false` alone cannot tell WHICH
/// command failed, so it re-validates with whatever command the surrounding
/// prose names. Requiring the argv in the type means the gate always carries
/// its own identity and no document can become a competing source of truth.
///
/// Returns `None` when the schema is acceptable, or the operator-facing
/// reason it is not.
fn check_output_schema_problem(trait_ref: &Trait, schema_ref: Option<&str>) -> Option<String> {
    const REQUIRED: &str = "check output slot must declare an object schema with \
         `ok` (schema:boolean) and `argv` (a list of schema:text)";
    let Some(schema_ref) = schema_ref else {
        return Some(format!("{REQUIRED}, got no schema"));
    };
    let Ok(parsed) = Reference::parse(schema_ref) else {
        return Some(format!("{REQUIRED}, got {schema_ref:?}"));
    };
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return Some(format!("{REQUIRED}, got {schema_ref:?}"));
    }
    let Some(fields) = trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
        .and_then(|schema| schema.fields.as_ref())
    else {
        return Some(format!("{REQUIRED}, got {schema_ref:?}"));
    };
    match fields.get("ok") {
        Some(field) if field.schema == "schema:boolean" => {}
        _ => return Some(format!("{REQUIRED}; `ok` is missing or is not schema:boolean")),
    }
    // The argv is a list, and its element schema is resolved through the same
    // list-schema indirection every other list field uses, so an author may
    // name the element schema whatever reads best as long as it bottoms out
    // at text.
    match fields.get("argv") {
        Some(field) if list_of_text_schema(trait_ref, &field.schema, &mut BTreeSet::new()) => {}
        _ => {
            return Some(format!(
                "{REQUIRED}; `argv` is missing or is not a list of schema:text"
            ));
        }
    }
    None
}

/// Whether `schema_ref` is a list whose elements are text. A list schema is
/// written as its element ref in brackets (`[schema:text]`), the same
/// encoding every other list field uses.
fn list_of_text_schema(trait_ref: &Trait, schema_ref: &str, seen: &mut BTreeSet<String>) -> bool {
    let Some(element) = schema_ref
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    text_schema(trait_ref, element, seen)
}

/// Whether `schema_ref` bottoms out at `schema:text`, following aliases.
fn text_schema(trait_ref: &Trait, schema_ref: &str, seen: &mut BTreeSet<String>) -> bool {
    if schema_ref == "schema:text" {
        return true;
    }
    let Ok(parsed) = Reference::parse(schema_ref) else {
        return false;
    };
    if parsed.kind() != Kind::Schema || parsed.is_qualified() || !seen.insert(parsed.id().to_string())
    {
        return false;
    }
    trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
        .and_then(|schema| schema.schema.as_deref())
        .is_some_and(|base| text_schema(trait_ref, base, seen))
}

fn local_slot_schema(trait_ref: &Trait, slot_id: &str) -> Option<String> {
    trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == slot_id)
        .and_then(|slot| slot.schema.as_ref())
        .map(ToString::to_string)
}

/// No branch may declare the join's own `destination` as an output sink — the
/// barrier-owned aggregate write stays distinct from every branch's own
/// writes.
fn validate_join_destination_not_branch_output(
    trait_ref: &Trait,
    item: &SequenceItem,
    destination: &str,
    join_base: &str,
) -> crate::Result<()> {
    for (j, branch_ref) in item.branches.iter().enumerate() {
        let mut reads = BTreeSet::new();
        let mut writes = BTreeSet::new();
        collect_sequence_ref_effects(
            trait_ref,
            Some(branch_ref),
            &mut reads,
            &mut writes,
            &mut BTreeSet::new(),
        );
        if writes.contains(destination) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{join_base}.destination"),
                message: format!(
                    "branch [{j}] ({branch_ref:?}) must not write join destination {destination:?}"
                ),
            }
            .into());
        }
    }
    Ok(())
}

/// Validate the optional ordered `branch-failure` policy list: each `branch`
/// exactly matches one declared `branches` entry, at most once each.
fn validate_parallel_branch_failure(item: &SequenceItem, base: &str) -> crate::Result<()> {
    if item.branch_failure.is_empty() {
        return Ok(());
    }
    let declared: BTreeSet<&str> = item.branches.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    for (j, entry) in item.branch_failure.iter().enumerate() {
        let field_path = format!("{base}.branch-failure[{j}]");
        if !declared.contains(entry.branch.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.branch"),
                message: format!(
                    "branch-failure branch {:?} does not match a declared parallel branch",
                    entry.branch
                ),
            }
            .into());
        }
        if !seen.insert(entry.branch.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.branch"),
                message: format!(
                    "duplicate branch-failure entry for branch {:?}",
                    entry.branch
                ),
            }
            .into());
        }
    }
    Ok(())
}

/// Reject write/write and either-direction write/read slot conflicts between
/// any pair of parallel branches, in authored order.
fn validate_parallel_independence(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
) -> crate::Result<()> {
    let branch_effects: Vec<(BTreeSet<String>, BTreeSet<String>)> = item
        .branches
        .iter()
        .map(|branch_ref| {
            let mut reads = BTreeSet::new();
            let mut writes = BTreeSet::new();
            collect_sequence_ref_effects(
                trait_ref,
                Some(branch_ref),
                &mut reads,
                &mut writes,
                &mut BTreeSet::new(),
            );
            (reads, writes)
        })
        .collect();
    let refs: Vec<&str> = item.branches.iter().map(String::as_str).collect();
    for left in 0..branch_effects.len() {
        for right in (left + 1)..branch_effects.len() {
            let (left_reads, left_writes) = &branch_effects[left];
            let (right_reads, right_writes) = &branch_effects[right];
            if let Some(slot) = left_writes.intersection(right_writes).next() {
                return Err(parallel_conflict_error(
                    base,
                    left,
                    right,
                    refs[left],
                    refs[right],
                    slot,
                    "both write",
                ));
            }
            if let Some(slot) = left_writes.intersection(right_reads).next() {
                return Err(parallel_conflict_error(
                    base,
                    left,
                    right,
                    refs[left],
                    refs[right],
                    slot,
                    "the first writes what the second reads",
                ));
            }
            if let Some(slot) = right_writes.intersection(left_reads).next() {
                return Err(parallel_conflict_error(
                    base,
                    left,
                    right,
                    refs[left],
                    refs[right],
                    slot,
                    "the second writes what the first reads",
                ));
            }
        }
    }
    Ok(())
}

fn parallel_conflict_error(
    base: &str,
    left: usize,
    right: usize,
    left_ref: &str,
    right_ref: &str,
    slot: &str,
    relation: &str,
) -> crate::Error {
    crate::manifest::Error::InvalidField {
        field_path: format!("{base}.branches"),
        message: format!(
            "parallel branches [{left}] ({left_ref:?}) and [{right}] ({right_ref:?}) are not independent: {relation} slot {slot:?}"
        ),
    }
    .into()
}

/// The ONE reusable recursive sequence-effect walk. Reads come from sequence
/// inputs, `for-each.over`, and every slot-backed typed guard understood by
/// [`collect_guard_slot_refs`]. Writes come from local unqualified `slot:*`
/// output sinks (regardless of write op) plus the implicit `for-each.item`
/// binding; `schema:*` ephemeral outputs and terminal `port:*` outputs are
/// excluded. Traverses nested sequence, branch arms, loop, for-each, and
/// parallel branches, reusing the shared sequence-id recursion guard.
fn collect_item_effects(
    trait_ref: &Trait,
    item: &SequenceItem,
    reads: &mut BTreeSet<String>,
    writes: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
) {
    for input in item.input.ref_texts() {
        if is_local_slot_ref(input) {
            reads.insert(input.to_string());
        }
    }
    if item.effective_kind() == SequenceKind::Project {
        for projection in &item.projection {
            if let Some(source_ref) = projection.source.as_slot_ref() {
                reads.insert(source_ref.to_string());
            }
            writes.insert(projection.destination.clone());
        }
    }
    if item.effective_kind() == SequenceKind::ForEach
        && let Some(over) = item.over.as_deref()
            && is_local_slot_ref(over) {
                reads.insert(over.to_string());
            }
    for guard in [
        item.when.as_ref(),
        item.until.as_ref(),
        item.stop_if.as_ref(),
    ]
    .into_iter()
    .flatten()
    .chain(item.emits.iter().filter_map(|rule| rule.when()))
    .chain(item.input.iter().filter_map(SequenceInput::guard))
    {
        let mut guard_slots = Vec::new();
        collect_guard_slot_refs(trait_ref, guard, &mut guard_slots, &mut BTreeSet::new());
        reads.extend(guard_slots);
    }
    for output in item.output.ref_texts() {
        if is_local_slot_ref(output) {
            writes.insert(output.to_string());
        }
    }
    if item.effective_kind() == SequenceKind::ForEach
        && let Some(item_slot) = item.item.as_deref()
            && is_local_slot_ref(item_slot) {
                writes.insert(item_slot.to_string());
            }
    match item.effective_kind() {
        SequenceKind::Sequence | SequenceKind::Loop | SequenceKind::ForEach => {
            collect_sequence_ref_effects(trait_ref, item.sequence.as_deref(), reads, writes, stack);
        }
        SequenceKind::Branch => {
            collect_sequence_ref_effects(trait_ref, item.sequence.as_deref(), reads, writes, stack);
            collect_sequence_ref_effects(
                trait_ref,
                item.otherwise.as_deref(),
                reads,
                writes,
                stack,
            );
        }
        SequenceKind::Parallel => {
            for branch_ref in item.branches.iter() {
                collect_sequence_ref_effects(trait_ref, Some(branch_ref), reads, writes, stack);
            }
            if let Some(destination) = item.join.as_ref().and_then(JoinPolicy::destination) {
                writes.insert(destination.to_string());
            }
        }
        SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => {}
    }
}

/// P402 concurrent `for-each` independence proof, consolidated onto the SAME
/// recursive effect walk [`collect_item_effects`] uses for `parallel`-branch
/// independence validation above — never a second, CLI-side hand-rolled
/// direct-field walker. Recursively covers nested sequences, branches,
/// loops, projections, guards (including slot-backed guards), `for-each.over`
/// itself, and local-slot semantics, exactly like the `parallel` check does.
///
/// Readiness to dispatch a later `for-each` item is not proof it is
/// independent of an earlier in-wave item's writes (a later item can read a
/// pre-existing accumulator/slot value an earlier item is about to update):
/// this is a static check of the body's own declared steps — if any step's
/// declared input ref is also written by any step's declared output sink
/// elsewhere in the same body (excluding the loop's own `item_slot`, which
/// every item rebinds fresh and is never a cross-item hazard), the body is
/// not provably independent. Returns `true` (fail closed: treat as a hazard)
/// when `body_sequence_id` does not name a known sequence.
pub fn for_each_body_has_cross_item_hazard(
    trait_ref: &Trait,
    body_sequence_id: &str,
    item_slot: Option<&str>,
) -> bool {
    let Some(sequence) = trait_ref.sequences.get(body_sequence_id) else {
        return true;
    };
    let mut reads = BTreeSet::new();
    let mut writes = BTreeSet::new();
    let mut stack = BTreeSet::new();
    stack.insert(body_sequence_id.to_string());
    for nested in &sequence.sequence {
        collect_item_effects(trait_ref, nested, &mut reads, &mut writes, &mut stack);
    }
    if let Some(item_slot) = item_slot {
        writes.remove(item_slot);
    }
    reads.iter().any(|read_ref| writes.contains(read_ref))
}

fn collect_sequence_ref_effects(
    trait_ref: &Trait,
    sequence_ref: Option<&str>,
    reads: &mut BTreeSet<String>,
    writes: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
) {
    let Some(sequence_id) = local_sequence_id(sequence_ref) else {
        return;
    };
    if !stack.insert(sequence_id.clone()) {
        return;
    }
    if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
        for nested in &sequence.sequence {
            collect_item_effects(trait_ref, nested, reads, writes, stack);
        }
    }
    stack.remove(&sequence_id);
}

fn is_local_slot_ref(ref_text: &str) -> bool {
    Reference::parse(ref_text)
        .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
}

fn validate_item_shape(item: &SequenceItem, kind: SequenceKind, base: &str) -> crate::Result<()> {
    let has_prompt = !item.prompt.trim().is_empty();
    let has_command = item.cmd.is_some() || item.command.is_some();
    let has_projection = !item.projection.is_empty();
    let has_command_options = item.timeout_ms.is_some()
        || item.idle_timeout_ms.is_some()
        || !item.success_exit_code.is_empty();
    let has_sequence_control = item.sequence.is_some()
        || item.when.is_some()
        || item.otherwise.is_some()
        || item.until.is_some()
        || item.stop_if.is_some()
        || item.max_iterations.is_some()
        || item.max_iterations_from.is_some()
        || item.on_exhausted.is_some()
        || item.on_stop.is_some()
        || item.over.is_some()
        || item.item.is_some()
        || item.max_items.is_some()
        || item.on_complete.is_some();

    if kind != SequenceKind::Branch && kind != SequenceKind::Ask && (item.when.is_some() || item.otherwise.is_some()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: if item.when.is_some() {
                format!("{base}.when")
            } else {
                format!("{base}.otherwise")
            },
            message: "when and otherwise are valid only on branch sequence items".to_string(),
        }
        .into());
    }

    if kind != SequenceKind::ForEach && item.concurrent {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.concurrent"),
            message: "concurrent is valid only on for-each sequence items".to_string(),
        }
        .into());
    }

    if kind != SequenceKind::Parallel && (!item.branches.is_empty() || item.max_branches.is_some())
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: if !item.branches.is_empty() {
                format!("{base}.branches")
            } else {
                format!("{base}.max-branches")
            },
            message: "branches and max-branches are valid only on parallel sequence items"
                .to_string(),
        }
        .into());
    }

    if kind != SequenceKind::Parallel && (item.join.is_some() || !item.branch_failure.is_empty()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: if item.join.is_some() {
                format!("{base}.join")
            } else {
                format!("{base}.branch-failure")
            },
            message: "join and branch-failure are valid only on parallel sequence items"
                .to_string(),
        }
        .into());
    }

    match kind {
        SequenceKind::Prompt => {
            if !has_prompt
                || has_command
                || has_projection
                || has_command_options
                || has_sequence_control
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message:
                        "prompt sequence item must declare prompt and no command/control fields"
                            .to_string(),
                }
                .into());
            }
        }
        SequenceKind::Ask => {
            if !has_prompt
                || has_command
                || has_projection
                || has_command_options
                || item.agent.is_some()
                || item.format.is_some()
                || !item.emits.is_empty()
                || item.on_failure.is_some()
                || item.otherwise.is_some()
                || item.sequence.is_some()
                || item.until.is_some()
                || item.stop_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_stop.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || item.on_complete.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "ask sequence item must declare prompt, a signal when guard, ordinary inputs, and one local replace output; it cannot declare agent, command, emits, format, failure-route, or control fields".to_string(),
                }.into());
            }
        }
        SequenceKind::Command => {
            if has_prompt || !has_command || has_projection || has_sequence_control {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "command sequence item must declare cmd or command and no prompt/control fields"
                        .to_string(),
                }.into());
            }
        }
        SequenceKind::Check => {
            if has_prompt
                || !has_command
                || has_projection
                || has_sequence_control
                || item.format.is_some()
                || item.on_failure.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "check sequence item must declare cmd or command and no prompt/projection/format/control/on-failure fields — a check's false verdict is a normal accepted value, never a rejected-output route"
                        .to_string(),
                }.into());
            }
        }
        SequenceKind::Project => {
            if has_prompt
                || has_command
                || !has_projection
                || has_sequence_control
                || has_command_options
                || item.agent.is_some()
                || item.format.is_some()
                || !item.emits.is_empty()
                || item.on_failure.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "project sequence item must declare projection and no prompt/command/control/agent/format/emits/on-failure fields".to_string(),
                }
                .into());
            }
        }
        SequenceKind::Sequence => {
            if has_prompt
                || has_command
                || has_projection
                || item.sequence.is_none()
                || item.until.is_some()
                || item.stop_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_stop.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || item.on_complete.is_some()
                || item.on_failure.is_some()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message:
                        "sequence item kind=sequence must declare only sequence plus ordinary input/output/emits fields"
                            .to_string(),
                }.into());
            }
        }
        SequenceKind::Branch => {
            if has_prompt
                || has_command
                || has_projection
                || !item.input.is_empty()
                || !item.output.is_empty()
                || !item.emits.is_empty()
                || item.id.is_none()
                || item.sequence.is_none()
                || item.when.is_none()
                || item.until.is_some()
                || item.stop_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_stop.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || item.on_complete.is_some()
                || item.on_failure.is_some()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "branch sequence item must declare id, when, sequence, optional otherwise, and no prompt/command/other control fields".to_string(),
                }.into());
            }
        }
        SequenceKind::Loop => {
            if has_prompt
                || has_command
                || has_projection
                || item.sequence.is_none()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || item.on_complete.is_some()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
                // A loop has no failure to route. Spending the budget without
                // matching `until` is exhaustion, which `on-exhausted` governs
                // and the ledger's stop reason records; the body's own items
                // route their own failures. `on-failure` on a loop only ever
                // meant "emit this on exhaustion", which conflated the two.
                || item.on_failure.is_some()
                // Control items never become `ReadyItem`, so `emits` on a
                // loop is a silent no-op; reject it rather than let it read
                // as meaningful authoring.
                || !item.emits.is_empty()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "loop sequence item must declare sequence, optional max-iterations/until/stop-if/on-exhausted/on-stop, and no prompt/command/for-each/on-failure/emits fields"
                        .to_string(),
                }.into());
            }
        }
        SequenceKind::ForEach => {
            if has_prompt
                || has_command
                || has_projection
                || item.sequence.is_none()
                || item.over.is_none()
                || item.item.is_none()
                || item.until.is_some()
                || item.stop_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_stop.is_some()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "for-each sequence item must declare sequence, over, item, optional max-items/on-complete/on-failure, and no prompt/command/loop fields"
                        .to_string(),
                }.into());
            }
        }
        SequenceKind::Parallel => {
            if has_prompt
                || has_command
                || has_projection
                || has_command_options
                || has_sequence_control
                || item.id.is_none()
                || item.branches.is_empty()
                || item.max_branches.is_none()
                || !item.input.is_empty()
                || !item.output.is_empty()
                || !item.emits.is_empty()
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "parallel sequence item must declare id, branches, max-branches, optional join/branch-failure/on-failure, and no prompt/command/sequence/branch/loop/for-each/input/output/emits/agent/format fields".to_string(),
                }.into());
            }
        }
    }
    Ok(())
}

fn validate_item_refs(
    trait_ref: &Trait,
    item: &SequenceItem,
    kind: SequenceKind,
    base: &str,
    sets: &SequenceValidationSets<'_>,
) -> crate::Result<()> {
    validate_optional_agent(
        item.agent.as_deref(),
        &format!("{base}.agent"),
        sets.agent_ids,
    )?;

    // Runtime readiness records at most one conditional-input decision per
    // (sequence-index, ref, position) (P290's `record_conditional_input_decisions`).
    // Two declarations of the same ref where either is guarded would let
    // readiness evaluate both guards but collapse them into one recorded
    // decision, so frame inclusion and persisted evidence could diverge.
    // Reject the ambiguity statically instead of widening the decision key.
    let mut seen_input_refs: BTreeSet<&str> = BTreeSet::new();
    let mut decorated_input_refs: BTreeSet<&str> = BTreeSet::new();
    for (j, input) in item.input.iter().enumerate() {
        let ref_text = input.ref_text();
        let is_guarded = input.guard().is_some();
        let is_optional = input.is_optional();
        if !seen_input_refs.insert(ref_text) && (is_guarded || is_optional || decorated_input_refs.contains(ref_text))
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!(
                    "duplicate sequence item input {ref_text:?} is ambiguous because it is guarded or optional"
                ),
            }
            .into());
        }
        if is_guarded || is_optional {
            decorated_input_refs.insert(ref_text);
        }
        let parsed =
            Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!("invalid typed ref {ref_text:?}"),
            })?;
        if !SEQUENCE_INPUT_KINDS.contains(&parsed.kind()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!(
                    "sequence input ref kind {:?} not allowed; expected port, slot, or resource",
                    parsed.kind()
                ),
            }
            .into());
        }
        if !parsed.is_qualified()
            && parsed.kind() == Kind::Port
            && !sets.input_port_ids.contains(parsed.id())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!(
                    "sequence item input port {:?} is not a declared input-direction port",
                    parsed.id()
                ),
            }
            .into());
        }
        if !parsed.is_qualified()
            && parsed.kind() == Kind::Slot
            && !sets.slot_ids.contains(parsed.id())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!("sequence item input slot {:?} is not declared", parsed.id()),
            }
            .into());
        }
        if !parsed.is_qualified()
            && parsed.kind() == Kind::Resource
            && !sets.resource_ids.contains(parsed.id())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}]"),
                message: format!(
                    "sequence item input resource {:?} is not a declared local resource",
                    parsed.id()
                ),
            }
            .into());
        }
        if let Some(guard) = input.guard() {
            if parsed.kind() != Kind::Resource {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.input[{j}].when"),
                    message: format!(
                        "input guard is valid only on a resource input; got kind {:?}",
                        parsed.kind()
                    ),
                }
                .into());
            }
            crate::r#trait::condition::validate_guard_expr(
                trait_ref,
                guard,
                &format!("{base}.input[{j}].when"),
                sets.slot_ids,
                sets.signal_ids,
                false,
                false,
            )?;
        }
        if is_optional && parsed.kind() != Kind::Slot {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input[{j}].optional"),
                message: format!(
                    "optional sequence input is valid only on a slot input; got kind {:?}",
                    parsed.kind()
                ),
            }
            .into());
        }
    }

    let mut seen_output_refs = BTreeSet::new();
    for (j, sink) in item.output.iter().enumerate() {
        let ref_text = sink.ref_text();
        if !seen_output_refs.insert(ref_text) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.output[{j}]"),
                message: format!("duplicate sequence output sink {ref_text:?}"),
            }
            .into());
        }
        let parsed =
            Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("{base}.output[{j}]"),
                message: format!("invalid typed ref {ref_text:?}"),
            })?;
        if !SEQUENCE_OUTPUT_KINDS.contains(&parsed.kind()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.output[{j}]"),
                message: format!(
                    "sequence output ref kind {:?} not allowed; expected slot, terminal output port, or schema",
                    parsed.kind()
                ),
            }.into());
        }
        if parsed.is_qualified() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.output[{j}]"),
                message: "sequence output must be local and unqualified".to_string(),
            }
            .into());
        }
        match parsed.kind() {
            Kind::Slot if !sets.slot_ids.contains(parsed.id()) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[{j}]"),
                    message: format!("sequence output slot {:?} is not declared", parsed.id()),
                }
                .into());
            }
            Kind::Port if !sets.output_port_ids.contains(parsed.id()) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[{j}]"),
                    message: format!(
                        "direct sequence output port {:?} is not a declared output-direction port",
                        parsed.id()
                    ),
                }
                .into());
            }
            Kind::Port if matches!(kind, SequenceKind::Command | SequenceKind::Check) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[{j}]"),
                    message: "command-backed sequence items must output to slots".to_string(),
                }
                .into());
            }
            Kind::Port => validate_direct_output_port_not_consumed(trait_ref, ref_text, base)?,
            Kind::Schema => {
                validate_schema_output_ref(trait_ref, &parsed, &format!("{base}.output[{j}]"))?
            }
            _ => {}
        }
        validate_output_sink_operation(trait_ref, sink, &parsed, &format!("{base}.output[{j}]"))?;
    }

    for (j, emit) in item.emits.iter().enumerate() {
        validate_signal_ref(
            emit.signal_ref(),
            &format!("{base}.emits[{j}]"),
            sets.signal_ids,
        )?;
        if let Some(when) = emit.when() {
            crate::r#trait::condition::validate_guard_expr(
                trait_ref,
                when,
                &format!("{base}.emits[{j}].when"),
                sets.slot_ids,
                sets.signal_ids,
                false,
                true,
            )?;
            validate_output_predicates_read_declared_outputs(
                when,
                &item.output,
                &format!("{base}.emits[{j}].when"),
            )?;
        }
    }
    validate_optional_signal(
        item.on_complete.as_deref(),
        &format!("{base}.on-complete"),
        sets.signal_ids,
    )?;
    if let Some(on_failure) = item.on_failure.as_ref() {
        let signal_path = match on_failure {
            FailureTarget::Signal(_) => format!("{base}.on-failure"),
            FailureTarget::Route(_) => format!("{base}.on-failure.signal"),
        };
        validate_optional_signal(on_failure.signal_ref(), &signal_path, sets.signal_ids)?;
    }
    Ok(())
}

fn validate_loop_item(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    slot_ids: &BTreeSet<&str>,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_sequence_ref(
        trait_ref,
        item.sequence.as_deref(),
        &format!("{base}.sequence"),
    )?;
    if item.max_iterations.is_some() && item.max_iterations_from.is_some() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-iterations-from"),
            message: "loop must declare either max-iterations or max-iterations-from, not both"
                .to_string(),
        }
        .into());
    }
    let has_bound = item.max_iterations.is_some() || item.max_iterations_from.is_some();
    if !has_bound && item.until.is_none() && item.stop_if.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-iterations"),
            message:
                "unbounded loop must declare until or stop-if — a loop with neither a bound nor an exit guard can never end"
                    .to_string(),
        }
        .into());
    }
    if !has_bound && item.on_exhausted.is_some() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.on-exhausted"),
            message: "on-exhausted requires max-iterations or max-iterations-from — an unbounded loop cannot exhaust".to_string(),
        }
        .into());
    }
    if item.max_iterations == Some(0) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-iterations"),
            message: "max-iterations must be greater than zero".to_string(),
        }
        .into());
    }
    if let Some(max_iterations_from) = item.max_iterations_from.as_deref() {
        validate_integer_input_port_ref(
            trait_ref,
            max_iterations_from,
            &format!("{base}.max-iterations-from"),
        )?;
        if !item.input.ref_texts().any(|input| input == max_iterations_from) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input"),
                message: "dynamic loop bound port must be declared as a loop input".to_string(),
            }
            .into());
        }
        if item.input.is_optional_for(max_iterations_from) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.input"),
                message: "dynamic loop bound port must not be declared as an optional input"
                    .to_string(),
            }
            .into());
        }
    }
    if let Some(until) = item.until.as_ref() {
        crate::r#trait::condition::validate_guard_expr(
            trait_ref,
            until,
            &format!("{base}.until"),
            slot_ids,
            signal_ids,
            true,
            true,
        )?;
        validate_loop_guard_output_predicates(trait_ref, item, until, &format!("{base}.until"))?;
    }
    if let Some(stop_if) = item.stop_if.as_ref() {
        crate::r#trait::condition::validate_guard_expr(
            trait_ref,
            stop_if,
            &format!("{base}.stop-if"),
            slot_ids,
            signal_ids,
            true,
            true,
        )?;
        validate_loop_guard_output_predicates(
            trait_ref,
            item,
            stop_if,
            &format!("{base}.stop-if"),
        )?;
    }
    if let Some(on_exhausted) = item.on_exhausted.as_ref() {
        validate_exhaustion_target(on_exhausted, &format!("{base}.on-exhausted"), signal_ids)?;
    }
    if let Some(on_stop) = item.on_stop.as_ref() {
        validate_on_stop_requires_stop_if(item, base)?;
        validate_stop_signal_target(on_stop, &format!("{base}.on-stop"), signal_ids)?;
    }
    Ok(())
}

/// `on-stop` names which signal(s) a `stop-if` match emits, so it is
/// meaningless on a loop that never declares `stop-if`.
fn validate_on_stop_requires_stop_if(item: &SequenceItem, base: &str) -> crate::Result<()> {
    if item.stop_if.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.on-stop"),
            message: "loop on-stop requires stop-if to be declared".to_string(),
        }
        .into());
    }
    Ok(())
}

/// A loop's `on-stop` declaration: unlike `on-exhausted`, a `stop-if` match
/// always halts the loop, so the `"continue"`/`"block"` policy keywords are
/// meaningless here and rejected — only signal refs are accepted.
fn validate_stop_signal_target(
    target: &ExhaustionTarget,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_signal_target(target, field_path, "on-stop", false, signal_ids)
}

fn validate_exhaustion_target(
    target: &ExhaustionTarget,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_signal_target(target, field_path, "on-exhausted", true, signal_ids)
}

/// Shared walk over an `ExhaustionTarget`'s One/Many entries, resolving
/// each `signal:<id>` ref and rejecting duplicates — the one implementation
/// `on-exhausted` and `on-stop` both drive, differing only in whether the
/// `"continue"`/`"block"` policy keywords are legal entries (`on-exhausted`
/// only; a `stop-if` match always halts the loop, so they are meaningless
/// for `on-stop`) and in the field name their error messages cite.
fn validate_signal_target(
    target: &ExhaustionTarget,
    field_path: &str,
    field_label: &str,
    allow_policy_keywords: bool,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let invalid = |value: &str| -> crate::Error {
        let message = if allow_policy_keywords {
            format!("loop {field_label} accepts \"continue\", \"block\", or one or more signal:<id> refs (got {value:?})")
        } else {
            format!("loop {field_label} accepts one or more signal:<id> refs, not a policy keyword (got {value:?})")
        };
        crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message,
        }
        .into()
    };
    let validate_entry = |value: &str| -> crate::Result<()> {
        if matches!(value, "continue" | "block") {
            return if allow_policy_keywords { Ok(()) } else { Err(invalid(value)) };
        }
        if value.starts_with("signal:") {
            return validate_signal_ref(value, field_path, signal_ids);
        }
        Err(invalid(value))
    };
    match target {
        ExhaustionTarget::One(value) => validate_entry(value)?,
        ExhaustionTarget::Many(values) => {
            if values.is_empty() {
                return Err(invalid("[]"));
            }
            let mut seen = BTreeSet::new();
            for value in values {
                // A multi-entry list is never a bare policy keyword: unlike
                // the single-value form (where `on-exhausted = "continue"`
                // is the whole declaration), mixing a keyword into a
                // signal list is nonsensical regardless of which field
                // otherwise allows the keyword alone.
                if allow_policy_keywords && matches!(value.as_str(), "continue" | "block") {
                    return Err(invalid(value));
                }
                validate_entry(value)?;
                if !seen.insert(value.as_str()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: field_path.to_string(),
                        message: format!("loop {field_label} declares duplicate signal {value:?}"),
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn validate_command_argv_from(
    trait_ref: &Trait,
    item: &SequenceItem,
    argv_from: Option<&str>,
    base: &str,
) -> crate::Result<()> {
    let Some(argv_from) = argv_from else {
        return Ok(());
    };
    let port =
        validate_local_input_port_ref(trait_ref, argv_from, &format!("{base}.command.argv-from"))?;
    if port.schema != "[schema:text]" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.command.argv-from"),
            message: format!(
                "command argv-from requires a [schema:text] input port, got {:?}",
                port.schema
            ),
        }
        .into());
    }
    if !item.input.ref_texts().any(|input| input == argv_from) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "command argv-from port must be declared as a command input".to_string(),
        }
        .into());
    }
    if item.input.is_optional_for(argv_from) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "command argv-from port must not be declared as an optional input".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_command_executable_digest_from(
    trait_ref: &Trait,
    item: &SequenceItem,
    digest_from: Option<&str>,
    base: &str,
) -> crate::Result<()> {
    let Some(ref_text) = digest_from else {
        return Ok(());
    };
    let field_path = format!("{base}.command.executable-digest-from");
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.clone(),
        message: format!("invalid executable digest ref {ref_text:?}"),
    })?;
    if parsed.is_qualified() || !matches!(parsed.kind(), Kind::Port | Kind::Slot) {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: "executable digest must use a local text port or slot ref".to_string(),
        }
        .into());
    }
    let schema_ref = match parsed.kind() {
        Kind::Port => trait_ref
            .ports
            .iter()
            .find(|port| {
                port.id == parsed.id()
                    && matches!(port.direction, crate::r#trait::PortDirection::Input)
            })
            .map(|port| port.schema.clone()),
        Kind::Slot => local_slot_schema(trait_ref, parsed.id()),
        _ => None,
    };
    if schema_ref.as_deref() != Some("schema:text") {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: format!("executable digest ref requires schema:text, got {schema_ref:?}"),
        }
        .into());
    }
    if !item.input.ref_texts().any(|input| input == ref_text) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "executable digest ref must be declared as a command input".to_string(),
        }
        .into());
    }
    if item.input.is_optional_for(ref_text) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "executable digest ref must not be declared as an optional input".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_integer_input_port_ref(
    trait_ref: &Trait,
    ref_text: &str,
    field_path: &str,
) -> crate::Result<()> {
    let port = validate_local_input_port_ref(trait_ref, ref_text, field_path)?;
    if port.schema != "schema:integer" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "dynamic loop bound requires a schema:integer input port, got {:?}",
                port.schema
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_local_input_port_ref<'a>(
    trait_ref: &'a Trait,
    ref_text: &str,
    field_path: &str,
) -> crate::Result<&'a crate::r#trait::Port> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid input port ref {ref_text:?}"),
    })?;
    if parsed.kind() != Kind::Port || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "expected a local input port:* ref".to_string(),
        }
        .into());
    }
    trait_ref
        .ports
        .iter()
        .find(|port| {
            port.id == parsed.id() && matches!(port.direction, crate::r#trait::PortDirection::Input)
        })
        .ok_or_else(|| {
            crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!("unresolved local input port ref {ref_text:?}"),
            }
            .into()
        })
}

fn validate_project_item(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let mut destinations = BTreeSet::new();
    for (index, projection) in item.projection.iter().enumerate() {
        let projection_path = format!("{base}.projection[{index}]");
        let destination = validate_local_slot_ref(
            &projection.destination,
            &format!("{projection_path}.destination"),
            slot_ids,
        )?;
        if !destinations.insert(projection.destination.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{projection_path}.destination"),
                message: format!("duplicate project destination {:?}", projection.destination),
            }
            .into());
        }
        if !matches!(
            projection.operation,
            WriteOperation::Replace | WriteOperation::Append | WriteOperation::Increment
        ) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{projection_path}.operation"),
                message: "project operation must be replace, append, or increment".to_string(),
            }
            .into());
        }
        let destination_schema =
            local_slot_schema(trait_ref, destination.id()).ok_or_else(|| {
                crate::manifest::Error::InvalidField {
                    field_path: format!("{projection_path}.destination"),
                    message: "project destination slot must declare a schema".to_string(),
                }
            })?;
        let expected_source_schema = match &projection.operation {
            WriteOperation::Replace => destination_schema.clone(),
            WriteOperation::Append => list_element_schema(&destination_schema).ok_or_else(|| {
                crate::manifest::Error::InvalidField {
                    field_path: format!("{projection_path}.destination"),
                    message: format!(
                        "project append destination must use a list schema, got {destination_schema:?}"
                    ),
                }
            })?,
            WriteOperation::Increment => destination_schema.clone(),
            WriteOperation::Merge | WriteOperation::SetField(_) => {
                unreachable!("operation checked above")
            }
        };

        match &projection.source {
            ProjectionSource::Slot(source_ref) => {
                let source = validate_local_slot_ref(
                    source_ref,
                    &format!("{projection_path}.source"),
                    slot_ids,
                )?;
                let source_schema = local_slot_schema(trait_ref, source.id()).ok_or_else(|| {
                    crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.source"),
                        message: "project source slot must declare a schema".to_string(),
                    }
                })?;
                let selected_schema = if let Some(field) = projection.field.as_deref() {
                    crate::r#trait::condition::resolve_object_field_path_schema(
                        trait_ref,
                        &source_schema,
                        field,
                        &projection_path,
                    )
                    .map(|field_schema| field_schema.schema.clone())
                    .map_err(|_| crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.field"),
                        message: format!(
                            "project source field {field:?} is not declared by an inline object schema"
                        ),
                    })?
                } else {
                    source_schema
                };
                if selected_schema != expected_source_schema {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.destination"),
                        message: format!(
                            "project source schema {selected_schema:?} does not exactly match destination write schema {expected_source_schema:?}"
                        ),
                    }
                    .into());
                }
            }
            ProjectionSource::Literal { literal } => {
                if projection.field.is_some() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.field"),
                        message: "project field is valid only for a slot-backed source; a literal source has no field to select".to_string(),
                    }
                    .into());
                }
                let validation = crate::procedure::runtime::validate_value_schema(
                    trait_ref,
                    &projection_path,
                    &expected_source_schema,
                    literal,
                )?;
                if validation.status != crate::procedure::runtime::SchemaStatus::Accepted {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.source"),
                        message: format!(
                            "project literal source does not statically satisfy destination write schema {expected_source_schema:?}: {}",
                            validation.reason
                        ),
                    }
                    .into());
                }
            }
        }
    }

    for (index, projection) in item.projection.iter().enumerate() {
        if let Some(source_ref) = projection.source.as_slot_ref()
            && destinations.contains(source_ref) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.projection[{index}].source"),
                    message: "project source must not be written by the same project step"
                        .to_string(),
                }
                .into());
            }
    }

    if item.input.iter().any(SequenceInput::is_optional) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "project input must not be optional; a projection source must always have a value"
                .to_string(),
        }
        .into());
    }

    let expected_inputs: Vec<&str> = item
        .projection
        .iter()
        .filter_map(|projection| projection.source.as_slot_ref())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let declared_inputs: BTreeSet<&str> = item.input.ref_texts().collect();
    if item.input.len() != expected_inputs.len()
        || expected_inputs.as_slice() != declared_inputs.iter().copied().collect::<Vec<_>>()
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.input"),
            message: "project input must declare exactly its projection source slots".to_string(),
        }
        .into());
    }
    if item.output.len() != item.projection.len()
        || item
            .output
            .iter()
            .zip(&item.projection)
            .any(|(sink, projection)| {
                sink.ref_text() != projection.destination
                    || sink.operation() != &projection.operation
            })
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.output"),
            message: "project output must declare each projection destination and operation in projection order".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_for_each_item(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    slot_ids: &BTreeSet<&str>,
    _signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_sequence_ref(
        trait_ref,
        item.sequence.as_deref(),
        &format!("{base}.sequence"),
    )?;
    if item.max_items == Some(0) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-items"),
            message: "max-items must be greater than zero".to_string(),
        }
        .into());
    }
    let over = item.over.as_deref().expect("shape checked");
    let item_slot = item.item.as_deref().expect("shape checked");
    let over_parsed = validate_local_slot_ref(over, &format!("{base}.over"), slot_ids)?;
    let item_parsed = validate_local_slot_ref(item_slot, &format!("{base}.item"), slot_ids)?;
    let over_slot = trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == over_parsed.id())
        .ok_or_else(|| crate::manifest::Error::InvalidField {
            field_path: format!("{base}.over"),
            message: format!("unresolved slot {over:?}"),
        })?;
    let item_slot_decl = trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == item_parsed.id())
        .ok_or_else(|| crate::manifest::Error::InvalidField {
            field_path: format!("{base}.item"),
            message: format!("unresolved slot {item_slot:?}"),
        })?;
    let Some(over_schema_ref) = over_slot.schema.as_ref().map(ToString::to_string) else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.over"),
            message: "for-each over slot must declare a list schema".to_string(),
        }
        .into());
    };
    let Some(item_schema_ref) = item_slot_decl.schema.as_ref().map(ToString::to_string) else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.item"),
            message: "for-each item slot must declare a schema".to_string(),
        }
        .into());
    };
    let Some(element_schema) = list_element_schema(&over_schema_ref) else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.over"),
            message: "for-each over slot must use a list schema such as [schema:item]".to_string(),
        }
        .into());
    };
    if element_schema == "schema:any" || item_schema_ref == "schema:any" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: base.to_string(),
            message: "for-each over schema:any is not allowed".to_string(),
        }
        .into());
    }
    if element_schema != item_schema_ref {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.item"),
            message: format!(
                "for-each item slot schema {:?} does not match list element schema {:?}",
                item_schema_ref, element_schema
            ),
        }
        .into());
    }
    validate_for_each_no_scalar_checklist_verdict(trait_ref, item, base)?;
    Ok(())
}

/// A `for-each` body must not write a scalar checklist verdict, directly or
/// through any nested sequence/loop/branch/for-each/parallel arm it reaches:
/// runtime coverage only checks a whole verdict list supplied by one
/// `replace` write, so one verdict per iteration has no coverage proof.
fn validate_for_each_no_scalar_checklist_verdict(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
) -> crate::Result<()> {
    let Some(sequence_id) = local_sequence_id(item.sequence.as_deref()) else {
        return Ok(());
    };
    let mut stack = BTreeSet::new();
    let Some((output_path, checklist_id)) =
        find_checklist_verdict_output_in_sequence(trait_ref, &sequence_id, &mut stack)
    else {
        return Ok(());
    };
    Err(crate::manifest::Error::InvalidField {
        field_path: output_path.clone(),
        message: format!(
            "for-each {base} body writes a scalar checklist verdict for resource:{checklist_id} at {output_path}; one-verdict-per-iteration accumulation across for-each iterations has no coverage proof. Emit every verdict in one whole-list replace write to a [schema:{checklist_id}-verdict] slot instead."
        ),
    }
    .into())
}

fn find_checklist_verdict_output_in_sequence(
    trait_ref: &Trait,
    sequence_id: &str,
    stack: &mut BTreeSet<String>,
) -> Option<(String, String)> {
    if !stack.insert(sequence_id.to_string()) {
        return None;
    }
    let found = trait_ref.sequences.get(sequence_id).and_then(|sequence| {
        find_checklist_verdict_output_in_items(trait_ref, &sequence.sequence, sequence_id, stack)
    });
    stack.remove(sequence_id);
    found
}

fn find_checklist_verdict_output_in_items(
    trait_ref: &Trait,
    items: &[SequenceItem],
    sequence_id: &str,
    stack: &mut BTreeSet<String>,
) -> Option<(String, String)> {
    for (index, item) in items.iter().enumerate() {
        for (output_index, output) in item.output.iter().enumerate() {
            if let Some(checklist_id) = checklist_verdict_slot_schema(trait_ref, output.ref_text())
            {
                return Some((
                    format!("sequence.{sequence_id}.sequence[{index}].output[{output_index}]"),
                    checklist_id,
                ));
            }
        }
        if let Some(nested) = local_sequence_id(item.sequence.as_deref())
            && let Some(found) =
                find_checklist_verdict_output_in_sequence(trait_ref, &nested, stack)
            {
                return Some(found);
            }
        if let Some(nested) = local_sequence_id(item.otherwise.as_deref())
            && let Some(found) =
                find_checklist_verdict_output_in_sequence(trait_ref, &nested, stack)
            {
                return Some(found);
            }
        for branch_ref in item.branches.iter() {
            if let Some(nested) = local_sequence_id(Some(branch_ref))
                && let Some(found) =
                    find_checklist_verdict_output_in_sequence(trait_ref, &nested, stack)
                {
                    return Some(found);
                }
        }
    }
    None
}

/// The checklist resource id when `ref_text` is a local slot declaring a
/// scalar `schema:<id>-verdict` schema. `None` for whole-list `[schema:...]`
/// slots (the supported shape) and every non-checklist schema.
fn checklist_verdict_slot_schema(trait_ref: &Trait, ref_text: &str) -> Option<String> {
    let parsed = Reference::parse(ref_text).ok()?;
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return None;
    }
    let schema_ref = local_slot_schema(trait_ref, parsed.id())?;
    if list_element_schema(&schema_ref).is_some() {
        return None;
    }
    let schema_id = schema_ref.trim().strip_prefix("schema:")?;
    crate::r#trait::checklist::checklist_for_verdict_schema(&trait_ref.resources, schema_id)
        .map(|checklist| checklist.id.clone())
}

fn validate_sequence_ref(
    trait_ref: &Trait,
    value: Option<&str>,
    field_path: &str,
) -> crate::Result<()> {
    let Some(value) = value else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "sequence ref is required".to_string(),
        }
        .into());
    };
    let parsed = Reference::parse(value).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid sequence ref {value:?}"),
    })?;
    if parsed.kind() != Kind::Sequence || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "sequence ref must be a local sequence:<id> ref".to_string(),
        }
        .into());
    }
    if trait_ref.sequences.get(parsed.id()).is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("unresolved local sequence ref {value:?}"),
        }
        .into());
    }
    Ok(())
}

fn validate_schema_output_ref(
    trait_ref: &Trait,
    parsed: &Reference,
    field_path: &str,
) -> crate::Result<()> {
    let ref_text = parsed.as_str();
    if matches!(
        ref_text,
        "schema:text" | "schema:boolean" | "schema:number" | "schema:integer" | "schema:any"
    ) {
        return Ok(());
    }
    if !trait_ref
        .schemas
        .iter()
        .any(|schema| schema.id == parsed.id())
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("schema output ref {ref_text:?} is not declared"),
        }
        .into());
    }
    Ok(())
}

fn validate_output_sink_operation(
    trait_ref: &Trait,
    sink: &OutputSink,
    parsed: &Reference,
    field_path: &str,
) -> crate::Result<()> {
    match sink.operation() {
        WriteOperation::Replace => Ok(()),
        WriteOperation::Append => {
            let schema_ref =
                local_operation_slot_schema(trait_ref, parsed, field_path, "append", "an array")?;
            let Some(element) = list_element_schema(&schema_ref) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "append output operation requires an array slot schema, got {schema_ref:?}"
                    ),
                }
                .into());
            };
            // Checklist coverage judges a whole verdict list, and an appended
            // element is one verdict — legitimately incomplete until whatever
            // drives the appends finishes. Rather than let a checklist slot
            // accept appends with coverage silently switched off, refuse the
            // wiring: a checklist that cannot be checked for completeness is
            // the exact failure the typed form exists to prevent.
            let element_id = element
                .trim()
                .strip_prefix("schema:")
                .unwrap_or(element.trim());
            if let Some(checklist) = crate::r#trait::checklist::checklist_for_verdict_schema(
                &trait_ref.resources,
                element_id,
            ) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "append is not supported for checklist verdicts: coverage of resource:{} is checked against a whole verdict list, which only a replace write supplies. Emit every verdict in one write instead.",
                        checklist.id
                    ),
                }
                .into());
            }
            Ok(())
        }
        WriteOperation::Merge => {
            let schema_ref = local_operation_slot_schema(
                trait_ref,
                parsed,
                field_path,
                "merge",
                "an inline object",
            )?;
            object_schema_fields(trait_ref, &schema_ref).map(|_| ()).ok_or_else(|| {
                crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "merge output operation requires an inline object slot schema, got {schema_ref:?}"
                    ),
                }
                .into()
            })
        }
        WriteOperation::SetField(field) => {
            let schema_ref = local_operation_slot_schema(
                trait_ref,
                parsed,
                field_path,
                "set-field",
                "an inline object",
            )?;
            let Some(fields) = object_schema_fields(trait_ref, &schema_ref) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "set-field output operation requires an inline object slot schema, got {schema_ref:?}"
                    ),
                }
                .into());
            };
            if !fields.contains_key(field) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!("set-field output operation names unknown field {field:?}"),
                }
                .into());
            }
            Ok(())
        }
        WriteOperation::Increment => {
            let schema_ref = local_operation_slot_schema(
                trait_ref,
                parsed,
                field_path,
                "increment",
                "a numeric",
            )?;
            if !numeric_schema(trait_ref, &schema_ref, &mut BTreeSet::new()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "increment output operation requires a numeric slot schema, got {schema_ref:?}"
                    ),
                }
                .into());
            }
            Ok(())
        }
    }
}

fn local_operation_slot_schema(
    trait_ref: &Trait,
    parsed: &Reference,
    field_path: &str,
    operation: &str,
    expected: &str,
) -> crate::Result<String> {
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("{operation} output operation requires a local slot sink"),
        }
        .into());
    }
    let Some(slot) = trait_ref.slots.iter().find(|slot| slot.id == parsed.id()) else {
        return Ok(String::new());
    };
    slot.schema
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| {
            crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!("{operation} output operation requires {expected} slot schema"),
            }
            .into()
        })
}

fn object_schema_fields<'a>(
    trait_ref: &'a Trait,
    schema_ref: &str,
) -> Option<&'a BTreeMap<String, crate::r#trait::schema::SchemaField>> {
    let parsed = Reference::parse(schema_ref).ok()?;
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return None;
    }
    trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())?
        .fields
        .as_ref()
}

fn numeric_schema(trait_ref: &Trait, schema_ref: &str, seen: &mut BTreeSet<String>) -> bool {
    if matches!(schema_ref, "schema:number" | "schema:integer") {
        return true;
    }
    let Ok(parsed) = Reference::parse(schema_ref) else {
        return false;
    };
    if parsed.kind() != Kind::Schema
        || parsed.is_qualified()
        || !seen.insert(parsed.id().to_string())
    {
        return false;
    }
    trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
        .and_then(|schema| schema.schema.as_deref())
        .is_some_and(|base| numeric_schema(trait_ref, base, seen))
}

fn validate_output_predicates_read_declared_outputs(
    guard: &GuardExpr,
    outputs: &OutputSinkList,
    field_path: &str,
) -> crate::Result<()> {
    let declared: BTreeSet<&str> = outputs.ref_texts().collect();
    validate_output_predicates_read_refs(guard, &declared, field_path)
}

fn validate_loop_guard_output_predicates(
    trait_ref: &Trait,
    item: &SequenceItem,
    guard: &GuardExpr,
    field_path: &str,
) -> crate::Result<()> {
    let Some(sequence_id) = local_sequence_id(item.sequence.as_deref()) else {
        return Ok(());
    };
    let mut refs = BTreeSet::new();
    collect_sequence_output_refs(trait_ref, &sequence_id, &mut refs, &mut BTreeSet::new());
    validate_output_predicates_read_refs(
        guard,
        &refs.iter().map(String::as_str).collect(),
        field_path,
    )
}

fn validate_output_predicates_read_refs(
    guard: &GuardExpr,
    declared: &BTreeSet<&str>,
    field_path: &str,
) -> crate::Result<()> {
    match guard {
        GuardExpr::Ref(_) => Ok(()),
        GuardExpr::Any(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_output_predicates_read_refs(
                    item,
                    declared,
                    &format!("{field_path}[{index}]"),
                )?;
            }
            Ok(())
        }
        GuardExpr::Predicate(predicate) => {
            if let Some(output) = predicate.output.as_deref()
                && !declared.contains(output) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.output"),
                        message: format!(
                            "output predicate {output:?} must read an output declared by the same step or loop body"
                        ),
                    }
                    .into());
                }
            if let Some(not) = predicate.not.as_deref() {
                validate_output_predicates_read_refs(not, declared, &format!("{field_path}.not"))?;
            }
            for (index, item) in predicate.all.iter().enumerate() {
                validate_output_predicates_read_refs(
                    item,
                    declared,
                    &format!("{field_path}.all[{index}]"),
                )?;
            }
            for (index, item) in predicate.any.iter().enumerate() {
                validate_output_predicates_read_refs(
                    item,
                    declared,
                    &format!("{field_path}.any[{index}]"),
                )?;
            }
            Ok(())
        }
    }
}

fn collect_sequence_output_refs(
    trait_ref: &Trait,
    sequence_id: &str,
    refs: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) {
    refs.extend(collect_guaranteed_sequence_outputs(
        trait_ref,
        sequence_id,
        seen,
    ));
}

fn validate_sequence_graph(trait_ref: &Trait) -> crate::Result<()> {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        let mut edges = Vec::new();
        for item in &sequence.sequence {
            if matches!(
                item.effective_kind(),
                SequenceKind::Sequence
                    | SequenceKind::Branch
                    | SequenceKind::Loop
                    | SequenceKind::ForEach
            ) {
                if let Some(target) = item.sequence.as_deref() {
                    let parsed = Reference::parse(target).map_err(|_| {
                        crate::manifest::Error::InvalidField {
                            field_path: format!("sequence.{sequence_id}.sequence"),
                            message: format!("invalid sequence ref {target:?}"),
                        }
                    })?;
                    edges.push(parsed.id().to_string());
                }
                if let Some(target) = item.otherwise.as_deref() {
                    let parsed = Reference::parse(target).map_err(|_| {
                        crate::manifest::Error::InvalidField {
                            field_path: format!("sequence.{sequence_id}.otherwise"),
                            message: format!("invalid sequence ref {target:?}"),
                        }
                    })?;
                    edges.push(parsed.id().to_string());
                }
            }
            if item.effective_kind() == SequenceKind::Parallel {
                for (j, target) in item.branches.iter().enumerate() {
                    let parsed = Reference::parse(target).map_err(|_| {
                        crate::manifest::Error::InvalidField {
                            field_path: format!("sequence.{sequence_id}.branches[{j}]"),
                            message: format!("invalid sequence ref {target:?}"),
                        }
                    })?;
                    edges.push(parsed.id().to_string());
                }
            }
        }
        graph.insert(sequence_id.clone(), edges);
    }
    let mut memo = BTreeMap::new();
    for id in trait_ref.sequences.keys() {
        let mut stack = Vec::new();
        validate_sequence_depth(id, &graph, &mut stack, &mut memo)?;
    }
    Ok(())
}

fn validate_sequence_depth(
    current: &str,
    graph: &BTreeMap<String, Vec<String>>,
    stack: &mut Vec<String>,
    memo: &mut BTreeMap<String, usize>,
) -> crate::Result<usize> {
    if let Some(depth) = memo.get(current) {
        if stack.len().saturating_add(*depth) > MAX_SEQUENCE_NESTING_DEPTH {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("sequence.{current}"),
                message: format!(
                    "sequence nesting exceeds maximum depth {MAX_SEQUENCE_NESTING_DEPTH}"
                ),
            }
            .into());
        }
        return Ok(*depth);
    }
    if stack.iter().any(|item| item == current) {
        let start = stack.iter().position(|item| item == current).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(current.to_string());
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("sequence.{current}"),
            message: format!(
                "recursive/cyclic sequence refs are not allowed: {}",
                cycle.join(" -> ")
            ),
        }
        .into());
    }
    if stack.len() >= MAX_SEQUENCE_NESTING_DEPTH {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("sequence.{current}"),
            message: format!("sequence nesting exceeds maximum depth {MAX_SEQUENCE_NESTING_DEPTH}"),
        }
        .into());
    }
    stack.push(current.to_string());
    let mut max_child_depth = 0;
    if let Some(edges) = graph.get(current) {
        for edge in edges {
            max_child_depth =
                max_child_depth.max(validate_sequence_depth(edge, graph, stack, memo)?);
        }
    }
    stack.pop();
    let depth = max_child_depth + 1;
    memo.insert(current.to_string(), depth);
    Ok(depth)
}

fn validate_produced_before_read(trait_ref: &Trait) -> crate::Result<()> {
    let Some(procedure) = trait_ref.procedure.as_ref() else {
        return Ok(());
    };
    let mut produced = BTreeSet::new();
    let mut stack = BTreeSet::new();
    let mut memo = BTreeMap::new();
    let first_producers = first_slot_producers(trait_ref, procedure)?;
    validate_produced_before_read_in_items(
        trait_ref,
        ordered_procedure_items(procedure)?.into_iter(),
        &mut produced,
        &mut stack,
        &mut memo,
        &first_producers,
    )?;
    Ok(())
}

fn ordered_procedure_items(procedure: &Model) -> crate::Result<Vec<(usize, &SequenceItem)>> {
    let Some(sequence_order) = procedure.sequence_order.as_ref() else {
        return Ok(procedure.sequence.iter().enumerate().collect());
    };
    let mut by_id = BTreeMap::new();
    for (declaration_index, item) in procedure.sequence.iter().enumerate() {
        let Some(id) = item.id.as_deref() else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.sequence[{declaration_index}].id"),
                message: "sequence-order requires every procedure.sequence item to declare an id"
                    .to_string(),
            }
            .into());
        };
        if by_id.insert(id, (declaration_index, item)).is_some() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.sequence[{declaration_index}].id"),
                message: format!("duplicate sequence item id {id:?}"),
            }
            .into());
        }
    }
    if sequence_order.len() != procedure.sequence.len() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "procedure.sequence-order".to_string(),
            message: "sequence-order must include every procedure.sequence item exactly once"
                .to_string(),
        }
        .into());
    }
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for (run_index, requested_id) in sequence_order.iter().enumerate() {
        if !seen.insert(requested_id.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.sequence-order[{run_index}]"),
                message: format!("duplicate sequence-order id {requested_id:?}"),
            }
            .into());
        }
        let Some((declaration_index, item)) = by_id.get(requested_id.as_str()) else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("procedure.sequence-order[{run_index}]"),
                message: format!("unknown sequence item id {requested_id:?}"),
            }
            .into());
        };
        ordered.push((*declaration_index, *item));
    }
    Ok(ordered)
}

fn validate_failure_routes(
    trait_ref: &Trait,
    ordered: &[(usize, &SequenceItem)],
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let positions: BTreeMap<&str, usize> = ordered
        .iter()
        .enumerate()
        .filter_map(|(run_index, (_, item))| item.id.as_deref().map(|id| (id, run_index)))
        .collect();
    let route_context = FailureRouteValidationContext {
        trait_ref,
        ordered,
        positions: &positions,
        signal_ids,
    };
    let mut bound_sequences = BTreeSet::new();
    let mut available = BTreeSet::new();
    for (run_index, (declaration_index, item)) in ordered.iter().enumerate() {
        let base = format!("procedure.sequence[{declaration_index}].on-failure");
        validate_failure_route_occurrence(&route_context, item, run_index, &base, &available)?;
        validate_nested_failure_routes(
            &route_context,
            item,
            run_index,
            &format!("procedure.sequence[{declaration_index}]"),
            &mut BTreeSet::new(),
            &mut bound_sequences,
            &available,
        )?;
        collect_item_slot_outputs(trait_ref, item, &mut available, &mut BTreeSet::new());
    }
    reject_unbound_named_failure_routes(trait_ref, &bound_sequences)?;
    Ok(())
}

fn validate_failure_route_occurrence(
    context: &FailureRouteValidationContext<'_, '_>,
    item: &SequenceItem,
    run_index: usize,
    base: &str,
    available_before_source: &BTreeSet<String>,
) -> crate::Result<()> {
    let Some(route) = item.on_failure.as_ref().and_then(FailureTarget::route) else {
        return Ok(());
    };
    if item.id.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: base.to_string(),
            message: "a recovery route source must declare an id".to_string(),
        }
        .into());
    }
    let Some(target_run_index) = context.positions.get(route.step.as_str()) else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.step"),
            message: format!(
                "recovery target {:?} is not a top-level procedure step",
                route.step
            ),
        }
        .into());
    };
    if *target_run_index <= run_index {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.step"),
            message: "recovery target must occur later in effective procedure order".to_string(),
        }
        .into());
    }
    validate_optional_signal(
        route.signal.as_deref(),
        &format!("{base}.signal"),
        context.signal_ids,
    )?;
    let mut available = available_before_source.clone();
    for (recovery_run_index, (_, recovery)) in
        context.ordered.iter().enumerate().skip(*target_run_index)
    {
        validate_recovery_reads(
            context.trait_ref,
            recovery,
            &mut available,
            &mut BTreeSet::new(),
            &format!("{base}.step"),
            recovery_run_index,
        )?;
        collect_item_slot_outputs(
            context.trait_ref,
            recovery,
            &mut available,
            &mut BTreeSet::new(),
        );
    }
    Ok(())
}

struct FailureRouteValidationContext<'a, 'b> {
    trait_ref: &'a Trait,
    ordered: &'b [(usize, &'a SequenceItem)],
    positions: &'b BTreeMap<&'a str, usize>,
    signal_ids: &'b BTreeSet<&'a str>,
}

fn validate_nested_failure_routes(
    context: &FailureRouteValidationContext<'_, '_>,
    owner: &SequenceItem,
    run_index: usize,
    base: &str,
    stack: &mut BTreeSet<String>,
    bound_sequences: &mut BTreeSet<String>,
    available_before_owner: &BTreeSet<String>,
) -> crate::Result<()> {
    let mut arms: Vec<&str> = [owner.sequence.as_deref(), owner.otherwise.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if owner.effective_kind() == SequenceKind::Parallel {
        arms = owner.branches.iter().map(String::as_str).collect();
    }
    for reference in arms {
        let Some(sequence_id) = local_sequence_id(Some(reference)) else {
            continue;
        };
        if !stack.insert(sequence_id.clone()) {
            continue;
        }
        bound_sequences.insert(sequence_id.clone());
        if let Some(sequence) = context.trait_ref.sequences.get(&sequence_id) {
            let mut available = available_before_owner.clone();
            for (index, item) in sequence.sequence.iter().enumerate() {
                let item_base = format!("sequence.{sequence_id}.sequence[{index}]");
                validate_failure_route_occurrence(
                    context,
                    item,
                    run_index,
                    &format!("{item_base}.on-failure"),
                    &available,
                )?;
                validate_nested_failure_routes(
                    context,
                    item,
                    run_index,
                    &format!("{base}/{item_base}"),
                    stack,
                    bound_sequences,
                    &available,
                )?;
                collect_item_slot_outputs(
                    context.trait_ref,
                    item,
                    &mut available,
                    &mut BTreeSet::new(),
                );
            }
        }
        stack.remove(&sequence_id);
    }
    Ok(())
}

fn validate_recovery_reads(
    trait_ref: &Trait,
    item: &SequenceItem,
    available: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    field_path: &str,
    recovery_run_index: usize,
) -> crate::Result<()> {
    let mut reads: Vec<String> = item
        .input
        .iter()
        .filter(|input| !input.is_optional())
        .map(|input| input.ref_text().to_string())
        .collect();
    if item.effective_kind() == SequenceKind::ForEach {
        reads.extend(item.over.clone());
    }
    for when in [item.when.as_ref()]
        .into_iter()
        .flatten()
        .chain(item.input.iter().filter_map(SequenceInput::guard))
    {
        let mut guard_slots = Vec::new();
        collect_guard_slot_refs(trait_ref, when, &mut guard_slots, &mut BTreeSet::new());
        reads.extend(guard_slots);
    }
    for input in &reads {
        if Reference::parse(input)
            .is_ok_and(|reference| reference.kind() == Kind::Slot && !reference.is_qualified())
            && !available.contains(input)
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!(
                    "recovery route bypasses production of input {input:?} required by step {:?} at effective run {recovery_run_index}",
                    item.id
                ),
            }
            .into());
        }
    }

    let mut validate_sequence =
        |reference: Option<&str>, available: &mut BTreeSet<String>| -> crate::Result<()> {
            let Some(sequence_id) = local_sequence_id(reference) else {
                return Ok(());
            };
            if !stack.insert(sequence_id.clone()) {
                return Ok(());
            }
            if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
                for nested in &sequence.sequence {
                    validate_recovery_reads(
                        trait_ref,
                        nested,
                        available,
                        stack,
                        field_path,
                        recovery_run_index,
                    )?;
                    collect_local_slot_outputs(nested, available);
                }
            }
            stack.remove(&sequence_id);
            Ok(())
        };
    if item.effective_kind() == SequenceKind::Parallel {
        // Every branch runs from the same pre-panel state; after the panel the
        // union of branch outputs is available.
        let before = available.clone();
        let mut union = before.clone();
        for branch_ref in item.branches.iter() {
            let mut branch_available = before.clone();
            validate_sequence(Some(branch_ref), &mut branch_available)?;
            union.extend(branch_available);
        }
        *available = union;
    } else if item.effective_kind() == SequenceKind::Branch {
        let mut then_available = available.clone();
        let mut otherwise_available = available.clone();
        validate_sequence(item.sequence.as_deref(), &mut then_available)?;
        validate_sequence(item.otherwise.as_deref(), &mut otherwise_available)?;
        *available = then_available
            .intersection(&otherwise_available)
            .cloned()
            .collect();
    } else if item.effective_kind() == SequenceKind::ForEach {
        let mut body_available = available.clone();
        if let Some(item_slot) = item.item.as_deref() {
            body_available.insert(item_slot.to_string());
        }
        validate_sequence(item.sequence.as_deref(), &mut body_available)?;
    } else {
        validate_sequence(item.sequence.as_deref(), available)?;
    }
    Ok(())
}

fn reject_unbound_named_failure_routes(
    trait_ref: &Trait,
    bound_sequences: &BTreeSet<String>,
) -> crate::Result<()> {
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        if bound_sequences.contains(sequence_id) {
            continue;
        }
        for (index, item) in sequence.sequence.iter().enumerate() {
            if item
                .on_failure
                .as_ref()
                .and_then(FailureTarget::route)
                .is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("sequence.{sequence_id}.sequence[{index}].on-failure"),
                    message: "recovery route has no enclosing procedure step".to_string(),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn collect_item_slot_outputs(
    trait_ref: &Trait,
    item: &SequenceItem,
    outputs: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
) {
    collect_local_slot_outputs(item, outputs);
    let mut child_outputs = |reference: Option<&str>| {
        let Some(sequence_id) = local_sequence_id(reference) else {
            return BTreeSet::new();
        };
        if !stack.insert(sequence_id.clone()) {
            return BTreeSet::new();
        }
        let mut child = BTreeSet::new();
        if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
            for nested in &sequence.sequence {
                collect_item_slot_outputs(trait_ref, nested, &mut child, stack);
            }
        }
        stack.remove(&sequence_id);
        child
    };
    if item.effective_kind() == SequenceKind::Parallel {
        // Every branch runs, so every branch output escapes the panel.
        for branch_ref in item.branches.iter() {
            outputs.extend(child_outputs(Some(branch_ref)));
        }
        // The barrier join's own aggregate write is a post-barrier output of
        // the panel itself, available to whatever reads it afterward.
        if let Some(destination) = item.join.as_ref().and_then(JoinPolicy::destination) {
            outputs.insert(destination.to_string());
        }
        return;
    }
    let then_outputs = child_outputs(item.sequence.as_deref());
    if item.effective_kind() == SequenceKind::Branch {
        let otherwise_outputs = child_outputs(item.otherwise.as_deref());
        outputs.extend(then_outputs.intersection(&otherwise_outputs).cloned());
    } else if item.effective_kind() != SequenceKind::ForEach {
        outputs.extend(then_outputs);
    }
}

fn validate_produced_before_read_in_sequence(
    trait_ref: &Trait,
    sequence_id: &str,
    produced: &BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    memo: &mut BTreeMap<(String, Vec<String>), BTreeSet<String>>,
    first_producers: &BTreeMap<String, String>,
) -> crate::Result<BTreeSet<String>> {
    let key = (
        sequence_id.to_string(),
        produced.iter().cloned().collect::<Vec<_>>(),
    );
    if let Some(cached) = memo.get(&key) {
        return Ok(cached.clone());
    }
    if !stack.insert(sequence_id.to_string()) {
        return Ok(produced.clone());
    }
    let Some(sequence) = trait_ref.sequences.get(sequence_id) else {
        stack.remove(sequence_id);
        return Ok(produced.clone());
    };
    let mut current = produced.clone();
    validate_produced_before_read_in_items(
        trait_ref,
        sequence.sequence.iter().enumerate(),
        &mut current,
        stack,
        memo,
        first_producers,
    )?;
    stack.remove(sequence_id);
    memo.insert(key, current.clone());
    Ok(current)
}

fn validate_produced_before_read_in_items<'a>(
    trait_ref: &Trait,
    items: impl Iterator<Item = (usize, &'a SequenceItem)>,
    produced: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    memo: &mut BTreeMap<(String, Vec<String>), BTreeSet<String>>,
    first_producers: &BTreeMap<String, String>,
) -> crate::Result<()> {
    for (index, item) in items {
        let base = sequence_item_base(trait_ref, item, index);
        let reader = item.id.clone().unwrap_or_else(|| base.clone());
        let mut guard_slot_reads = Vec::new();
        for guard in item.input.iter().filter_map(SequenceInput::guard) {
            collect_guard_slot_refs(trait_ref, guard, &mut guard_slot_reads, &mut BTreeSet::new());
        }
        let required_inputs: Vec<&str> = item
            .input
            .iter()
            .filter(|input| !input.is_optional())
            .map(SequenceInput::ref_text)
            .collect();
        for input in required_inputs.into_iter().chain(guard_slot_reads.iter().map(String::as_str)) {
            let Ok(reference) = Reference::parse(input) else {
                continue;
            };
            if reference.kind() == Kind::Slot
                && !reference.is_qualified()
                && !produced.contains(input)
            {
                let error = match first_producers.get(input) {
                    Some(producer) => crate::reference::Error::SlotProducedLater {
                        reader: reader.clone(),
                        ref_text: input.to_string(),
                        producer: producer.clone(),
                    },
                    None => crate::reference::Error::SlotNeverProduced {
                        reader: reader.clone(),
                        ref_text: input.to_string(),
                    },
                };
                return Err(error.into());
            }
        }
        if item.effective_kind() == SequenceKind::ForEach
            && let Some(over) = item.over.as_deref()
                && !produced.contains(over) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.over"),
                        message: format!(
                            "for-each over slot {over:?} must be produced before the for-each item first runs"
                        ),
                    }.into());
                }
        if item.effective_kind() == SequenceKind::Branch
            && let Some(when) = item.when.as_ref() {
                validate_guard_slots_produced(trait_ref, when, produced, &reader, first_producers)?;
            }
        if matches!(
            item.effective_kind(),
            SequenceKind::Sequence | SequenceKind::Loop | SequenceKind::ForEach
        )
            && let Some(sequence_id) = local_sequence_id(item.sequence.as_deref()) {
                // The for-each item is a runtime-bound local value for its body,
                // not an output that escapes to subsequent procedure items.
                let mut body_produced = produced.clone();
                let item_slot = item.item.clone();
                if let Some(item_slot) = item_slot.as_deref() {
                    body_produced.insert(item_slot.to_string());
                }
                let mut body_outputs = validate_produced_before_read_in_sequence(
                    trait_ref,
                    &sequence_id,
                    &body_produced,
                    stack,
                    memo,
                    first_producers,
                )?;
                if let Some(item_slot) = item_slot.filter(|item_slot| {
                    !produced.contains(item_slot)
                        && !sequence_explicitly_produces_slot(trait_ref, &sequence_id, item_slot)
                }) {
                    body_outputs.remove(&item_slot);
                }
                if item.effective_kind() != SequenceKind::ForEach {
                    *produced = body_outputs;
                }
            }
        if item.effective_kind() == SequenceKind::Branch {
            let then_outputs = match local_sequence_id(item.sequence.as_deref()) {
                Some(sequence_id) => validate_produced_before_read_in_sequence(
                    trait_ref,
                    &sequence_id,
                    produced,
                    stack,
                    memo,
                    first_producers,
                )?,
                None => produced.clone(),
            };
            let otherwise_outputs = match local_sequence_id(item.otherwise.as_deref()) {
                Some(sequence_id) => validate_produced_before_read_in_sequence(
                    trait_ref,
                    &sequence_id,
                    produced,
                    stack,
                    memo,
                    first_producers,
                )?,
                None => produced.clone(),
            };
            *produced = then_outputs
                .intersection(&otherwise_outputs)
                .cloned()
                .collect();
        }
        if item.effective_kind() == SequenceKind::Parallel {
            // Validate every branch against the same pre-panel produced set; the
            // union of non-skippable branch outputs is available after the panel.
            let mut union = produced.clone();
            for branch_ref in item.branches.iter() {
                let Some(sequence_id) = local_sequence_id(Some(branch_ref)) else {
                    continue;
                };
                let branch_outputs = validate_produced_before_read_in_sequence(
                    trait_ref,
                    &sequence_id,
                    produced,
                    stack,
                    memo,
                    first_producers,
                )?;
                let can_skip = item.branch_failure.iter().any(|entry| {
                    entry.branch == *branch_ref
                        && entry.on_failure == BranchFailurePolicy::Skip
                });
                if !can_skip {
                    union.extend(branch_outputs);
                }
            }
            *produced = union;
        }
        if item.effective_kind() != SequenceKind::Branch {
            collect_local_slot_outputs(item, produced);
        }
    }
    Ok(())
}

fn validate_guard_slots_produced(
    trait_ref: &Trait,
    guard: &GuardExpr,
    produced: &BTreeSet<String>,
    reader: &str,
    first_producers: &BTreeMap<String, String>,
) -> crate::Result<()> {
    let mut slots = Vec::new();
    collect_guard_slot_refs(trait_ref, guard, &mut slots, &mut BTreeSet::new());
    for slot in slots {
        if produced.contains(&slot) {
            continue;
        }
        let error = match first_producers.get(&slot) {
            Some(producer) => crate::reference::Error::SlotProducedLater {
                reader: reader.to_string(),
                ref_text: slot,
                producer: producer.clone(),
            },
            None => crate::reference::Error::SlotNeverProduced {
                reader: reader.to_string(),
                ref_text: slot,
            },
        };
        return Err(error.into());
    }
    Ok(())
}

fn collect_guard_slot_refs(
    trait_ref: &Trait,
    guard: &GuardExpr,
    slots: &mut Vec<String>,
    seen_conditions: &mut BTreeSet<String>,
) {
    match guard {
        GuardExpr::Ref(reference) => {
            if let Ok(reference) = Reference::parse(reference)
                && reference.kind() == Kind::Condition
                && seen_conditions.insert(reference.id().to_string())
            {
                if let Some(condition) = trait_ref.conditions.get(reference.id()) {
                    collect_guard_slot_refs(
                        trait_ref,
                        &condition.as_guard(),
                        slots,
                        seen_conditions,
                    );
                }
                seen_conditions.remove(reference.id());
            }
        }
        GuardExpr::Any(items) => {
            for item in items {
                collect_guard_slot_refs(trait_ref, item, slots, seen_conditions);
            }
        }
        GuardExpr::Predicate(predicate) => {
            for slot in [
                predicate.slot.as_deref(),
                predicate.empty.as_deref(),
                predicate.count.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if Reference::parse(slot).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                }) {
                    slots.push(slot.to_string());
                }
            }
            if let Some((_, threshold)) = crate::r#trait::condition::ordered_modifier(predicate)
                && let Some(rhs_ref) = crate::r#trait::condition::numeric_comparison_ref(threshold)
                && Reference::parse(rhs_ref).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                })
            {
                slots.push(rhs_ref.to_string());
            }
            for threshold in [predicate.equals.as_ref(), predicate.at_least.as_ref()].into_iter().flatten() {
                if let Some(operand) = crate::r#trait::condition::parse_count_operand(threshold)
                    && Reference::parse(&operand.count).is_ok_and(|reference| {
                        reference.kind() == Kind::Slot && !reference.is_qualified()
                    })
                {
                    slots.push(operand.count);
                }
            }
            if let Some(condition) = predicate.condition.as_deref() {
                collect_guard_slot_refs(
                    trait_ref,
                    &GuardExpr::Ref(condition.to_string()),
                    slots,
                    seen_conditions,
                );
            }
            if let Some(not) = predicate.not.as_deref() {
                collect_guard_slot_refs(trait_ref, not, slots, seen_conditions);
            }
            for item in predicate.all.iter().chain(&predicate.any) {
                collect_guard_slot_refs(trait_ref, item, slots, seen_conditions);
            }
        }
    }
}

fn first_slot_producers(
    trait_ref: &Trait,
    procedure: &Model,
) -> crate::Result<BTreeMap<String, String>> {
    let mut producers = BTreeMap::new();
    let mut stack = BTreeSet::new();
    collect_first_slot_producers(
        trait_ref,
        ordered_procedure_items(procedure)?.into_iter(),
        &mut producers,
        &mut stack,
    );
    Ok(producers)
}

fn collect_first_slot_producers<'a>(
    trait_ref: &Trait,
    items: impl Iterator<Item = (usize, &'a SequenceItem)>,
    producers: &mut BTreeMap<String, String>,
    stack: &mut BTreeSet<String>,
) {
    for (index, item) in items {
        let mut child_refs: Vec<&str> = item.sequence.as_deref().into_iter().collect();
        if item.effective_kind() == SequenceKind::Parallel {
            child_refs = item.branches.iter().map(String::as_str).collect();
        }
        for child_ref in child_refs {
            if let Some(sequence_id) = local_sequence_id(Some(child_ref))
                && stack.insert(sequence_id.clone())
            {
                if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
                    collect_first_slot_producers(
                        trait_ref,
                        sequence.sequence.iter().enumerate(),
                        producers,
                        stack,
                    );
                }
                stack.remove(&sequence_id);
            }
        }
        let label = item
            .id
            .clone()
            .unwrap_or_else(|| sequence_item_base(trait_ref, item, index));
        for output in item.output.ref_texts() {
            if Reference::parse(output)
                .is_ok_and(|reference| reference.kind() == Kind::Slot && !reference.is_qualified())
            {
                producers
                    .entry(output.to_string())
                    .or_insert_with(|| label.clone());
            }
        }
    }
}

fn sequence_explicitly_produces_slot(trait_ref: &Trait, sequence_id: &str, slot: &str) -> bool {
    fn contains_output(
        trait_ref: &Trait,
        sequence_id: &str,
        slot: &str,
        stack: &mut BTreeSet<String>,
    ) -> bool {
        if !stack.insert(sequence_id.to_string()) {
            return false;
        }
        let found = trait_ref
            .sequences
            .get(sequence_id)
            .is_some_and(|sequence| {
                sequence.sequence.iter().any(|item| {
                    item.output.ref_texts().any(|output| output == slot)
                        || local_sequence_id(item.sequence.as_deref())
                            .is_some_and(|nested| contains_output(trait_ref, &nested, slot, stack))
                })
            });
        stack.remove(sequence_id);
        found
    }

    contains_output(trait_ref, sequence_id, slot, &mut BTreeSet::new())
}

fn sequence_item_base(trait_ref: &Trait, item: &SequenceItem, index: usize) -> String {
    if trait_ref.procedure.as_ref().is_some_and(|procedure| {
        procedure
            .sequence
            .iter()
            .enumerate()
            .any(|(procedure_index, procedure_item)| {
                procedure_index == index && std::ptr::eq(procedure_item, item)
            })
    }) {
        return format!("procedure.sequence[{index}]");
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        for (sequence_index, sequence_item) in sequence.sequence.iter().enumerate() {
            if std::ptr::eq(sequence_item, item) {
                return format!("sequence.{sequence_id}.sequence[{sequence_index}]");
            }
        }
    }
    format!("procedure.sequence[{index}]")
}

fn collect_local_slot_outputs(item: &SequenceItem, produced: &mut BTreeSet<String>) {
    for output in item.output.ref_texts() {
        if Reference::parse(output)
            .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
        {
            produced.insert(output.to_string());
        }
    }
}

fn validate_signal_ref(
    ref_text: &str,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid typed ref {ref_text:?}"),
    })?;
    if parsed.kind() != Kind::Signal {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "signal ref kind {:?} not allowed; expected signal",
                parsed.kind()
            ),
        }
        .into());
    }
    if !parsed.is_qualified() && !signal_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("unresolved local signal ref {ref_text:?}"),
        }
        .into());
    }
    Ok(())
}

fn validate_agent_ref(
    ref_text: &str,
    field_path: &str,
    agent_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid typed ref {ref_text:?}"),
    })?;
    if parsed.kind() != Kind::Agent {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "agent ref kind {:?} not allowed; expected agent",
                parsed.kind()
            ),
        }
        .into());
    }
    if parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "agent ref must be local and unqualified".to_string(),
        }
        .into());
    }
    if agent_ids.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "sequence item agent requires at least one declared [[agent]] role"
                .to_string(),
        }
        .into());
    }
    if !agent_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("unresolved local agent ref {ref_text:?}"),
        }
        .into());
    }
    Ok(())
}

fn validate_optional_agent(
    value: Option<&str>,
    field_path: &str,
    agent_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    if let Some(value) = value {
        validate_agent_ref(value, field_path, agent_ids)?;
    }
    Ok(())
}

fn validate_optional_signal(
    value: Option<&str>,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    if let Some(value) = value {
        validate_signal_ref(value, field_path, signal_ids)?;
    }
    Ok(())
}

fn validate_local_slot_ref(
    ref_text: &str,
    field_path: &str,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<Reference> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid slot ref {ref_text:?}"),
    })?;
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "expected a local slot:* ref".to_string(),
        }
        .into());
    }
    if !slot_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("unresolved local slot ref {ref_text:?}"),
        }
        .into());
    }
    Ok(parsed)
}

fn list_element_schema(schema_ref: &str) -> Option<String> {
    schema_ref
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::to_string)
}

fn validate_slot_backed_port_schema(
    trait_ref: &Trait,
    port_id: &str,
    slot_ref: &str,
) -> crate::Result<()> {
    let Some(port) = trait_ref.ports.iter().find(|port| port.id == port_id) else {
        return Ok(());
    };
    let Ok(parsed) = Reference::parse(slot_ref) else {
        return Ok(());
    };
    let Some(slot) = trait_ref.slots.iter().find(|slot| slot.id == parsed.id()) else {
        return Ok(());
    };
    let Some(slot_schema) = slot.schema.as_ref().map(ToString::to_string) else {
        return Ok(());
    };
    if port.schema != slot_schema && port.schema != "schema:any" && slot_schema != "schema:any" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("port[{port_id}].value"),
            message: format!(
                "output port schema {:?} is incompatible with value slot schema {:?}",
                port.schema, slot_schema
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_direct_output_port_not_consumed(
    trait_ref: &Trait,
    port_ref: &str,
    field_path: &str,
) -> crate::Result<()> {
    for (index, item) in trait_ref
        .procedure
        .as_ref()
        .into_iter()
        .flat_map(|procedure| procedure.sequence.iter().enumerate())
    {
        if item.input.ref_texts().any(|input| input == port_ref) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!(
                    "direct output port {port_ref:?} must not be consumed as internal state; found procedure.sequence[{index}].input"
                ),
            }.into());
        }
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        for (index, item) in sequence.sequence.iter().enumerate() {
            if item.input.ref_texts().any(|input| input == port_ref) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!(
                        "direct output port {port_ref:?} must not be consumed as internal state; found sequence.{sequence_id}.sequence[{index}].input"
                    ),
                }.into());
            }
        }
    }
    Ok(())
}

fn collect_produced_refs(trait_ref: &Trait) -> BTreeSet<String> {
    trait_ref
        .procedure
        .as_ref()
        .map_or_else(BTreeSet::new, |procedure| {
            collect_guaranteed_item_outputs(trait_ref, &procedure.sequence, &mut BTreeSet::new())
        })
}

/// Outputs from a branch are available after it only when every selected arm
/// produces them. A branch without an otherwise arm can be a no-op.
fn collect_guaranteed_item_outputs(
    trait_ref: &Trait,
    items: &[SequenceItem],
    stack: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let mut produced = BTreeSet::new();
    for item in items {
        produced.extend(item.output.ref_texts().map(str::to_string));
        if item.effective_kind() == SequenceKind::Parallel {
            // Every branch runs, so every branch's guaranteed outputs are
            // guaranteed after the panel.
            for branch_ref in item.branches.iter() {
                if let Some(sequence_id) = local_sequence_id(Some(branch_ref)) {
                    produced.extend(collect_guaranteed_sequence_outputs(
                        trait_ref,
                        &sequence_id,
                        stack,
                    ));
                }
            }
            if let Some(destination) = item.join.as_ref().and_then(JoinPolicy::destination) {
                produced.insert(destination.to_string());
            }
            continue;
        }
        let then_outputs = local_sequence_id(item.sequence.as_deref())
            .map(|sequence_id| collect_guaranteed_sequence_outputs(trait_ref, &sequence_id, stack))
            .unwrap_or_default();
        if item.effective_kind() == SequenceKind::Branch {
            let otherwise_outputs = local_sequence_id(item.otherwise.as_deref())
                .map(|sequence_id| {
                    collect_guaranteed_sequence_outputs(trait_ref, &sequence_id, stack)
                })
                .unwrap_or_default();
            produced.extend(then_outputs.intersection(&otherwise_outputs).cloned());
        } else if item.effective_kind() != SequenceKind::ForEach {
            produced.extend(then_outputs);
        }
    }
    produced
}

fn collect_guaranteed_sequence_outputs(
    trait_ref: &Trait,
    sequence_id: &str,
    stack: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    if !stack.insert(sequence_id.to_string()) {
        return BTreeSet::new();
    }
    let produced = trait_ref
        .sequences
        .get(sequence_id)
        .map_or_else(BTreeSet::new, |sequence| {
            collect_guaranteed_item_outputs(trait_ref, &sequence.sequence, stack)
        });
    stack.remove(sequence_id);
    produced
}

fn local_sequence_id(ref_text: Option<&str>) -> Option<String> {
    let parsed = Reference::parse(ref_text?).ok()?;
    (parsed.kind() == Kind::Sequence && !parsed.is_qualified()).then(|| parsed.id().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_from_toml(toml_src: &str) -> SequenceItem {
        toml::from_str(toml_src).expect("test fixture must be a valid sequence item")
    }

    fn loop_item(on_exhausted: Option<&str>) -> SequenceItem {
        let policy = on_exhausted
            .map(|value| format!("on-exhausted = \"{value}\"\n"))
            .unwrap_or_default();
        item_from_toml(&format!(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\n{policy}"
        ))
    }

    #[test]
    fn loop_shape_allows_on_exhausted_field() {
        validate_item_shape(
            &loop_item(Some("continue")),
            SequenceKind::Loop,
            "procedure.sequence[0]",
        )
        .expect("on-exhausted is a legal loop field at the shape level");
    }

    #[test]
    fn loop_shape_allows_omitted_on_exhausted() {
        validate_item_shape(
            &loop_item(None),
            SequenceKind::Loop,
            "procedure.sequence[0]",
        )
        .expect("omitting on-exhausted stays valid; continue is the runtime default");
    }

    #[test]
    fn non_loop_item_rejects_exhaustion_policy() {
        let item = item_from_toml(
            "id = \"stage\"\ntitle = \"Stage\"\nkind = \"sequence\"\nsequence = \"sequence:stage\"\non-exhausted = \"continue\"\n",
        );
        validate_item_shape(&item, SequenceKind::Sequence, "procedure.sequence[0]")
            .expect_err("on-exhausted is loop-only");
    }

    #[test]
    fn prompt_item_rejects_idle_timeout_ms_like_its_sibling_timeout_ms() {
        let item = item_from_toml(
            "id = \"review\"\ntitle = \"Review\"\nkind = \"prompt\"\nprompt = \"prompt:review\"\nidle-timeout-ms = 5000\n",
        );
        validate_item_shape(&item, SequenceKind::Prompt, "procedure.sequence[0]")
            .expect_err("idle-timeout-ms is command-only, same as timeout-ms");
    }

    #[test]
    fn idle_timeout_ms_round_trips_through_command_declaration_and_shorthand() {
        let declaration = item_from_toml(
            "id = \"gate\"\ntitle = \"Gate\"\nkind = \"command\"\n[command]\nargv = [\"true\"]\nidle-timeout-ms = 5000\n",
        );
        assert_eq!(
            declaration.command.as_ref().and_then(|command| command.idle_timeout_ms),
            Some(5000)
        );
        let toml_out = toml::to_string(&declaration).expect("declaration serializes");
        assert!(toml_out.contains("idle-timeout-ms = 5000"));

        let shorthand = item_from_toml(
            "id = \"gate\"\ntitle = \"Gate\"\nkind = \"command\"\ncmd = \"true\"\nidle-timeout-ms = 5000\n",
        );
        assert_eq!(shorthand.idle_timeout_ms, Some(5000));
        let toml_out = toml::to_string(&shorthand).expect("shorthand serializes");
        assert!(toml_out.contains("idle-timeout-ms = 5000"));
    }

    #[test]
    fn loop_rejects_emits() {
        let item = item_from_toml(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\nemits = [\"signal:done\"]\n",
        );
        let error = validate_item_shape(&item, SequenceKind::Loop, "procedure.sequence[0]")
            .expect_err("loop items never become ReadyItem, so emits is a silent no-op");
        assert!(
            format!("{error}").contains("emits"),
            "error must name the offending field: {error}"
        );
    }

    #[test]
    fn loop_rejects_on_failure_naming_on_exhausted() {
        let item = item_from_toml(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\non-failure = \"signal:done\"\n",
        );
        let error = validate_item_shape(&item, SequenceKind::Loop, "procedure.sequence[0]")
            .expect_err("a loop has no failure of its own to route");
        assert!(
            format!("{error}").contains("on-exhausted"),
            "error must steer authors toward on-exhausted: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_accepts_keywords() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_exhaustion_target(&ExhaustionTarget::One("continue".to_string()), "x.on-exhausted", &signal_ids)
            .expect("\"continue\" is a legal keyword");
        validate_exhaustion_target(&ExhaustionTarget::One("block".to_string()), "x.on-exhausted", &signal_ids)
            .expect("\"block\" is a legal keyword");
    }

    #[test]
    fn validate_exhaustion_target_accepts_resolved_signal() {
        let signal_ids: BTreeSet<&str> = ["refinement-exhausted"].into_iter().collect();
        validate_exhaustion_target(
            &ExhaustionTarget::One("signal:refinement-exhausted".to_string()),
            "x.on-exhausted",
            &signal_ids,
        )
        .expect("a declared local signal ref resolves");
    }

    #[test]
    fn validate_exhaustion_target_rejects_unresolved_signal() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_exhaustion_target(
            &ExhaustionTarget::One("signal:missing".to_string()),
            "x.on-exhausted",
            &signal_ids,
        )
        .expect_err("an unresolved local signal ref must be rejected");
        assert!(
            format!("{error}").contains("signal:missing"),
            "error must name the offending ref: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_rejects_unknown_keyword() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_exhaustion_target(
            &ExhaustionTarget::One("ignore".to_string()),
            "x.on-exhausted",
            &signal_ids,
        )
        .expect_err("only \"continue\", \"block\", or a signal ref are legal");
        assert!(
            format!("{error}").contains("on-exhausted"),
            "error must name the offending field: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_rejects_empty_list() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_exhaustion_target(&ExhaustionTarget::Many(Vec::new()), "x.on-exhausted", &signal_ids)
            .expect_err("an empty list is not a legal declaration");
        assert!(
            format!("{error}").contains("on-exhausted"),
            "error must name the offending field: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_rejects_duplicate_signals() {
        let signal_ids: BTreeSet<&str> = ["a", "b"].into_iter().collect();
        let error = validate_exhaustion_target(
            &ExhaustionTarget::Many(vec!["signal:a".to_string(), "signal:a".to_string()]),
            "x.on-exhausted",
            &signal_ids,
        )
        .expect_err("duplicate signal entries must be rejected");
        assert!(
            format!("{error}").contains("duplicate"),
            "error must call out the duplication: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_accepts_multi_signal_list() {
        let signal_ids: BTreeSet<&str> = ["a", "b"].into_iter().collect();
        validate_exhaustion_target(
            &ExhaustionTarget::Many(vec!["signal:a".to_string(), "signal:b".to_string()]),
            "x.on-exhausted",
            &signal_ids,
        )
        .expect("distinct resolved signals are legal");
    }

    fn loop_item_with_stop(stop_if: bool, on_stop: Option<&str>) -> SequenceItem {
        let stop_if_line = if stop_if { "stop-if = { slot = \"slot:verdict\", field = \"status\", equals = \"revise\" }\n" } else { "" };
        let on_stop_line = on_stop
            .map(|value| format!("on-stop = \"{value}\"\n"))
            .unwrap_or_default();
        item_from_toml(&format!(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\n{stop_if_line}{on_stop_line}"
        ))
    }

    #[test]
    fn loop_shape_allows_on_stop_field() {
        validate_item_shape(
            &loop_item_with_stop(true, Some("signal:recurring-blocker-unresolved")),
            SequenceKind::Loop,
            "procedure.sequence[0]",
        )
        .expect("on-stop is a legal loop field at the shape level");
    }

    #[test]
    fn on_stop_requires_stop_if() {
        let item = loop_item_with_stop(false, Some("signal:recurring-blocker-unresolved"));
        let error = validate_on_stop_requires_stop_if(&item, "procedure.sequence[0]")
            .expect_err("on-stop without stop-if must be rejected");
        assert!(
            format!("{error}").contains("stop-if"),
            "error must steer authors toward stop-if: {error}"
        );
    }

    #[test]
    fn on_stop_allowed_alongside_stop_if() {
        let item = loop_item_with_stop(true, Some("signal:recurring-blocker-unresolved"));
        validate_on_stop_requires_stop_if(&item, "procedure.sequence[0]")
            .expect("on-stop declared alongside stop-if is legal");
    }

    #[test]
    fn non_loop_item_rejects_on_stop() {
        let item = item_from_toml(
            "id = \"stage\"\ntitle = \"Stage\"\nkind = \"sequence\"\nsequence = \"sequence:stage\"\non-stop = \"signal:done\"\n",
        );
        validate_item_shape(&item, SequenceKind::Sequence, "procedure.sequence[0]")
            .expect_err("on-stop is loop-only");
    }

    #[test]
    fn validate_stop_signal_target_accepts_resolved_signal() {
        let signal_ids: BTreeSet<&str> = ["recurring-blocker-unresolved"].into_iter().collect();
        validate_stop_signal_target(
            &ExhaustionTarget::One("signal:recurring-blocker-unresolved".to_string()),
            "x.on-stop",
            &signal_ids,
        )
        .expect("a declared local signal ref resolves");
    }

    #[test]
    fn validate_stop_signal_target_rejects_unresolved_signal() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_stop_signal_target(
            &ExhaustionTarget::One("signal:missing".to_string()),
            "x.on-stop",
            &signal_ids,
        )
        .expect_err("an unresolved local signal ref must be rejected");
        assert!(
            format!("{error}").contains("signal:missing"),
            "error must name the offending ref: {error}"
        );
    }

    #[test]
    fn validate_stop_signal_target_rejects_continue_keyword() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_stop_signal_target(
            &ExhaustionTarget::One("continue".to_string()),
            "x.on-stop",
            &signal_ids,
        )
        .expect_err("a stop-if match always halts the loop, so \"continue\" is meaningless here");
        assert!(
            format!("{error}").contains("policy keyword"),
            "error must explain why the keyword is rejected: {error}"
        );
    }

    #[test]
    fn validate_stop_signal_target_rejects_block_keyword() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_stop_signal_target(&ExhaustionTarget::One("block".to_string()), "x.on-stop", &signal_ids)
            .expect_err("\"block\" is equally meaningless for on-stop");
    }

    #[test]
    fn validate_stop_signal_target_rejects_empty_list() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_stop_signal_target(&ExhaustionTarget::Many(Vec::new()), "x.on-stop", &signal_ids)
            .expect_err("an empty list is not a legal declaration");
    }

    #[test]
    fn validate_stop_signal_target_rejects_duplicate_signals() {
        let signal_ids: BTreeSet<&str> = ["a"].into_iter().collect();
        let error = validate_stop_signal_target(
            &ExhaustionTarget::Many(vec!["signal:a".to_string(), "signal:a".to_string()]),
            "x.on-stop",
            &signal_ids,
        )
        .expect_err("duplicate signal entries must be rejected");
        assert!(
            format!("{error}").contains("duplicate"),
            "error must call out the duplication: {error}"
        );
    }
}
