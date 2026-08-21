// Validates trait procedure declarations.
// Procedure validation.

pub fn validate(t: &Trait) -> crate::Result<()> {
    validate_checklist_item_schema_version(t)?;
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
    let has_success_terminal = trait_has_success_terminal(t);
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
            // Slot-backed ports are produced by ordinary sequence steps, not
            // bound at a terminal exit (a slot-backed port cannot also be
            // bound at an exit per the double-binding refusal above), so
            // this whole-trait producedness check still applies even when
            // the trait declares a success terminal.
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
            if !has_success_terminal
                && !port.optional
                && !produced_refs.contains(&direct_ref)
            {
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

/// Whether any sequence item (including those reached only through named
/// sequences, branches, loops, or parallel branches) is a terminal with
/// `outcome = "success"`.
pub(crate) fn trait_has_success_terminal(trait_ref: &Trait) -> bool {
    let Some(ref procedure) = trait_ref.procedure else {
        return false;
    };
    let mut seen = BTreeSet::new();
    items_have_success_terminal(trait_ref, &procedure.sequence, &mut seen)
}

fn items_have_success_terminal(
    trait_ref: &Trait,
    items: &[SequenceItem],
    seen: &mut BTreeSet<String>,
) -> bool {
    items.iter().any(|item| item_has_success_terminal(trait_ref, item, seen))
}

fn item_has_success_terminal(
    trait_ref: &Trait,
    item: &SequenceItem,
    seen: &mut BTreeSet<String>,
) -> bool {
    if item.effective_kind() == SequenceKind::Terminal
        && item.outcome == Some(TerminalOutcome::Success)
    {
        return true;
    }
    if let Some(sequence_id) = local_sequence_id(item.sequence.as_deref())
        && sequence_contains_success_terminal(trait_ref, &sequence_id, seen)
    {
        return true;
    }
    if let Some(sequence_id) = local_sequence_id(item.otherwise.as_deref())
        && sequence_contains_success_terminal(trait_ref, &sequence_id, seen)
    {
        return true;
    }
    for branch_ref in item.branches.iter() {
        if let Some(sequence_id) = local_sequence_id(Some(branch_ref))
            && sequence_contains_success_terminal(trait_ref, &sequence_id, seen)
        {
            return true;
        }
    }
    false
}

fn sequence_contains_success_terminal(
    trait_ref: &Trait,
    sequence_id: &str,
    seen: &mut BTreeSet<String>,
) -> bool {
    if !seen.insert(sequence_id.to_string()) {
        return false;
    }
    let found = trait_ref
        .sequences
        .get(sequence_id)
        .is_some_and(|sequence| items_have_success_terminal(trait_ref, &sequence.sequence, seen));
    seen.remove(sequence_id);
    found
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
    validate_item_refs(trait_ref, item, base, sets)?;

    match kind {
        SequenceKind::Prompt => {
            validate_sequence_item_prompt_contract(
                trait_ref,
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
            validate_sequence_item_prompt_contract(
                trait_ref,
                base,
                item.id.as_deref(),
                item,
                &trait_ref.prompts,
            )?;
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
                    message: "command-backed sequence items must declare exactly one output"
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
            if !matches!(parsed.kind(), Kind::Slot | Kind::Port) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output[0]"),
                    message: "command-backed sequence items must output to a slot or an output port"
                        .to_string(),
                }
                .into());
            }
            if parsed.kind() == Kind::Port {
                if !crate::r#trait::schema_version_at_least(trait_ref.schema_version.as_str(), "0.5")
                {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.output[0]"),
                        message:
                            "command output to a port requires schema-version \"0.5\" or newer"
                                .to_string(),
                    }
                    .into());
                }
                let port_schema = trait_ref
                    .ports
                    .iter()
                    .find(|port| port.id == parsed.id())
                    .map(|port| port.schema.clone());
                if let Some(port_schema) = port_schema
                    && !schema_refs_compatible(&port_schema, "schema:text")
                {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.output[0]"),
                        message: format!(
                            "output port schema {port_schema:?} is incompatible with a \
                             command's text capture; only schema:text or schema:any are \
                             supported"
                        ),
                    }
                    .into());
                }
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
            if item.output.is_empty() || item.output.len() > 2 {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output"),
                    message: "check sequence items must declare one output (a slot or a port) \
                         or two outputs (one slot and one port)"
                        .to_string(),
                }
                .into());
            }
            let mut slot_output: Option<(usize, &OutputSink)> = None;
            let mut port_output: Option<(usize, &OutputSink)> = None;
            for (index, output) in item.output.iter().enumerate() {
                let field_path = format!("{base}.output[{index}]");
                if *output.operation() != WriteOperation::Replace {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path,
                        message: "check output must use replace write semantics".to_string(),
                    }
                    .into());
                }
                let parsed = Reference::parse(output.ref_text()).map_err(|_| {
                    crate::manifest::Error::InvalidField {
                        field_path: field_path.clone(),
                        message: format!("invalid typed ref {:?}", output.ref_text()),
                    }
                })?;
                match parsed.kind() {
                    Kind::Slot if slot_output.is_none() => slot_output = Some((index, output)),
                    Kind::Port if port_output.is_none() => port_output = Some((index, output)),
                    Kind::Slot | Kind::Port => {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path,
                            message:
                                "check sequence item must declare at most one slot output and \
                                 at most one port output"
                                    .to_string(),
                        }
                        .into());
                    }
                    _ => {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path,
                            message: "check output must be a slot or an output port".to_string(),
                        }
                        .into());
                    }
                }
            }
            if item.output.len() == 2 && (slot_output.is_none() || port_output.is_none()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.output"),
                    message: "check sequence item with two outputs must declare one slot and one port"
                        .to_string(),
                }
                .into());
            }
            if let Some((index, output)) = slot_output {
                let field_path = format!("{base}.output[{index}]");
                let output_ref =
                    validate_local_slot_ref(output.ref_text(), &field_path, sets.slot_ids)?;
                let schema_ref = local_slot_schema(trait_ref, output_ref.id());
                if let Some(problem) = check_output_schema_problem(trait_ref, schema_ref.as_deref())
                {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path,
                        message: problem,
                    }
                    .into());
                }
            }
            if let Some((index, output)) = port_output {
                let field_path = format!("{base}.output[{index}]");
                if !crate::r#trait::schema_version_at_least(trait_ref.schema_version.as_str(), "0.5")
                {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path,
                        message:
                            "command output to a port requires schema-version \"0.5\" or newer"
                                .to_string(),
                    }
                    .into());
                }
                let parsed = Reference::parse(output.ref_text()).expect("validated above");
                let port_schema = trait_ref
                    .ports
                    .iter()
                    .find(|port| port.id == parsed.id())
                    .map(|port| port.schema.clone());
                if let Some(problem) = check_output_schema_problem(trait_ref, port_schema.as_deref())
                {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path,
                        message: problem,
                    }
                    .into());
                }
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
        SequenceKind::Terminal => validate_terminal_item(trait_ref, item, base, sets)?,
    }
    Ok(())
}

/// Validate a `kind = "terminal"` item: `flow.error`/`flow.success` authored
/// exit points. Requires `schema-version` "0.4" or newer, an `outcome`, and
/// payload writes that either target the reserved error-record port
/// (`error`) or name declared output ports (`success`).
fn validate_terminal_item(
    trait_ref: &Trait,
    item: &SequenceItem,
    base: &str,
    sets: &SequenceValidationSets<'_>,
) -> crate::Result<()> {
    if !crate::r#trait::schema_version_at_least(trait_ref.schema_version.as_str(), "0.4") {
        return Err(crate::manifest::Error::InvalidField {
            field_path: base.to_string(),
            message: "kind = \"terminal\" requires a trait declaring schema-version \"0.4\" or newer"
                .to_string(),
        }
        .into());
    }
    let outcome = item.outcome.ok_or_else(|| crate::manifest::Error::InvalidField {
        field_path: format!("{base}.outcome"),
        message: "terminal sequence item must declare outcome".to_string(),
    })?;

    let mut destinations = BTreeSet::new();
    for (index, projection) in item.payload.iter().enumerate() {
        let projection_path = format!("{base}.payload[{index}]");
        if !destinations.insert(projection.destination.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{projection_path}.destination"),
                message: format!("duplicate terminal payload destination {:?}", projection.destination),
            }
            .into());
        }
        match outcome {
            TerminalOutcome::Error => {
                if projection.destination != crate::r#trait::port::TERMINAL_ERROR_PORT_ID {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.destination"),
                        message: format!(
                            "error terminal payload destination must be the reserved {:?} port",
                            crate::r#trait::port::TERMINAL_ERROR_PORT_ID
                        ),
                    }
                    .into());
                }
            }
            TerminalOutcome::Success => {
                if !sets.output_port_ids.contains(projection.destination.as_str()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.destination"),
                        message: format!(
                            "success terminal payload destination {:?} must name a declared output port",
                            projection.destination
                        ),
                    }
                    .into());
                }
                let matching_port = trait_ref.ports.iter().find(|port| port.id == projection.destination);
                if matching_port.is_some_and(|port| port.value.is_some()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{projection_path}.destination"),
                        message: format!(
                            "output port {:?} already declares value and cannot also be bound at an exit",
                            projection.destination
                        ),
                    }
                    .into());
                }
            }
        }
        if let ProjectionSource::Slot(source_ref) = &projection.source {
            validate_local_slot_ref(source_ref, &format!("{projection_path}.source"), sets.slot_ids)?;
        }
    }

    if outcome == TerminalOutcome::Success {
        // Value-backed (slot-backed) ports are enforced by the whole-trait
        // producedness check in `validate` (they cannot also be bound at an
        // exit, per the double-binding refusal above); only "direct" ports
        // (no declared `value`) must be bound at each success exit.
        for port in trait_ref.ports.iter().filter(|p| {
            matches!(p.direction, crate::r#trait::PortDirection::Output)
                && !p.optional
                && p.value.is_none()
        }) {
            if !destinations.contains(port.id.as_str()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: format!(
                        "required output port {:?} is not bound at this success exit",
                        port.id
                    ),
                }
                .into());
            }
        }
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
                optional: false,
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
    if item.effective_kind() == SequenceKind::Terminal {
        for projection in &item.payload {
            if let Some(source_ref) = projection.source.as_slot_ref() {
                reads.insert(source_ref.to_string());
            }
        }
    }
    for guard in [
        item.when.as_ref(),
        item.until.as_ref(),
        item.abort_if.as_ref(),
    ]
    .into_iter()
    .flatten()
    .chain(item.on_complete.iter().filter_map(|rule| rule.when()))
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
        | SequenceKind::Project
        | SequenceKind::Terminal => {}
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
        || item.abort_if.is_some()
        || item.max_iterations.is_some()
        || item.max_iterations_from.is_some()
        || item.on_exhausted.is_some()
        || item.on_abort.is_some()
        || item.over.is_some()
        || item.item.is_some()
        || item.max_items.is_some();

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
                || !item.on_complete.is_empty()
                || item.on_failure.is_some()
                || item.otherwise.is_some()
                || item.sequence.is_some()
                || item.until.is_some()
                || item.abort_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_abort.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || !item.on_complete.is_empty()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "ask sequence item must declare prompt, a signal when guard, ordinary inputs, and one local replace output; it cannot declare agent, command, on-complete, format, failure-route, or control fields".to_string(),
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
                || !item.on_complete.is_empty()
                || item.on_failure.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "project sequence item must declare projection and no prompt/command/control/agent/format/on-complete/on-failure fields".to_string(),
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
                || item.abort_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_abort.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || !item.on_complete.is_empty()
                || item.on_failure.is_some()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message:
                        "sequence item kind=sequence must declare only sequence plus ordinary input/output/on-complete fields"
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
                || !item.on_complete.is_empty()
                || item.id.is_none()
                || item.sequence.is_none()
                || item.when.is_none()
                || item.until.is_some()
                || item.abort_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_abort.is_some()
                || item.over.is_some()
                || item.item.is_some()
                || item.max_items.is_some()
                || !item.on_complete.is_empty()
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
                || !item.on_complete.is_empty()
                || has_command_options
                || item.format.is_some()
                || item.agent.is_some()
                // A loop has no failure to route. Spending the budget without
                // matching `until` is exhaustion, which `on-exhausted` governs
                // and the ledger's stop reason records; the body's own items
                // route their own failures. `on-failure` on a loop only ever
                // meant "emit this on exhaustion", which conflated the two.
                || item.on_failure.is_some()
                // Control items never become `ReadyItem`, so `on-complete` on a
                // loop is a silent no-op; reject it rather than let it read
                // as meaningful authoring.
                || !item.on_complete.is_empty()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "loop sequence item must declare sequence, optional max-iterations/until/abort-if/on-exhausted/on-abort, and no prompt/command/for-each/on-failure/on-complete fields"
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
                || item.abort_if.is_some()
                || item.max_iterations.is_some()
                || item.max_iterations_from.is_some()
                || item.on_exhausted.is_some()
                || item.on_abort.is_some()
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
                || !item.on_complete.is_empty()
                || item.format.is_some()
                || item.agent.is_some()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "parallel sequence item must declare id, branches, max-branches, optional join/branch-failure/on-failure, and no prompt/command/sequence/branch/loop/for-each/input/output/on-complete/agent/format fields".to_string(),
                }.into());
            }
        }
        SequenceKind::Terminal => {
            if has_prompt
                || has_command
                || has_projection
                || has_command_options
                || has_sequence_control
                || !item.output.is_empty()
                || !item.on_complete.is_empty()
                || item.on_failure.is_some()
                || item.format.is_some()
                || item.agent.is_some()
                || !item.branches.is_empty()
                || item.max_branches.is_some()
                || item.join.is_some()
                || !item.branch_failure.is_empty()
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: base.to_string(),
                    message: "terminal sequence item must declare only outcome/message/payload plus ordinary inputs, and no prompt/command/sequence/branch/loop/for-each/output/on-complete/on-failure/agent/format/parallel fields".to_string(),
                }.into());
            }
        }
    }
    Ok(())
}

fn validate_item_refs(
    trait_ref: &Trait,
    item: &SequenceItem,
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
            Kind::Port => validate_direct_output_port_not_consumed(trait_ref, ref_text, base)?,
            Kind::Schema => {
                validate_schema_output_ref(trait_ref, &parsed, &format!("{base}.output[{j}]"))?
            }
            _ => {}
        }
        validate_output_sink_operation(trait_ref, sink, &parsed, &format!("{base}.output[{j}]"))?;
    }

    for (j, emit) in item.on_complete.iter().enumerate() {
        validate_signal_ref(
            emit.signal_ref(),
            &format!("{base}.on-complete[{j}]"),
            sets.signal_ids,
        )?;
        if let Some(when) = emit.when() {
            crate::r#trait::condition::validate_guard_expr(
                trait_ref,
                when,
                &format!("{base}.on-complete[{j}].when"),
                sets.slot_ids,
                sets.signal_ids,
                false,
                true,
            )?;
            validate_output_predicates_read_declared_outputs(
                when,
                &item.output,
                &format!("{base}.on-complete[{j}].when"),
            )?;
        }
    }
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
    if !has_bound && item.until.is_none() && item.abort_if.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.max-iterations"),
            message:
                "unbounded loop must declare until or abort-if — a loop with neither a bound nor an exit guard can never end"
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
        let parsed_bound =
            Reference::parse(max_iterations_from).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("{base}.max-iterations-from"),
                message: format!("invalid loop bound ref {max_iterations_from:?}"),
            })?;
        if parsed_bound.kind() == Kind::Setting {
            validate_integer_setting_ref(
                trait_ref,
                max_iterations_from,
                &format!("{base}.max-iterations-from"),
            )?;
        } else {
            validate_integer_input_port_ref(
                trait_ref,
                max_iterations_from,
                &format!("{base}.max-iterations-from"),
            )?;
            if !item.input.ref_texts().any(|input| input == max_iterations_from) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.input"),
                    message: "dynamic loop bound port must be declared as a loop input"
                        .to_string(),
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
    if let Some(abort_if) = item.abort_if.as_ref() {
        crate::r#trait::condition::validate_guard_expr(
            trait_ref,
            abort_if,
            &format!("{base}.abort-if"),
            slot_ids,
            signal_ids,
            true,
            true,
        )?;
        validate_loop_guard_output_predicates(
            trait_ref,
            item,
            abort_if,
            &format!("{base}.abort-if"),
        )?;
    }
    if let Some(on_exhausted) = item.on_exhausted.as_ref() {
        validate_exhaustion_target(on_exhausted, &format!("{base}.on-exhausted"), signal_ids)?;
    }
    if let Some(on_abort) = item.on_abort.as_ref() {
        validate_on_abort_requires_abort_if(item, base)?;
        validate_abort_signal_target(on_abort, &format!("{base}.on-abort"), signal_ids)?;
    }
    Ok(())
}

/// `on-abort` names which signal(s) a `abort-if` match emits, so it is
/// meaningless on a loop that never declares `abort-if`.
fn validate_on_abort_requires_abort_if(item: &SequenceItem, base: &str) -> crate::Result<()> {
    if item.abort_if.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.on-abort"),
            message: "loop on-abort requires abort-if to be declared".to_string(),
        }
        .into());
    }
    Ok(())
}

/// A loop's `on-abort` declaration: unlike `on-exhausted`, a `abort-if` match
/// always halts the loop, so the `"continue"`/`"abort"` policy keywords are
/// meaningless here and rejected — only signal refs are accepted.
fn validate_abort_signal_target(
    target: &ExhaustionTarget,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_signal_target(target, field_path, "on-abort", false, signal_ids)
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
/// `on-exhausted` and `on-abort` both drive, differing only in whether the
/// `"continue"`/`"abort"` policy keywords are legal entries (`on-exhausted`
/// only; a `abort-if` match always halts the loop, so they are meaningless
/// for `on-abort`) and in the field name their error messages cite.
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
        if matches!(value, "continue" | "abort") {
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
                if allow_policy_keywords && matches!(value.as_str(), "continue" | "abort") {
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

/// Resolve a `setting:<id>` ref to its declaration, failing the build with
/// the resolved id when no such setting is declared (task Watch item: every
/// `setting:` reference site shares this lookup so an unknown id can never
/// build in one position and fail in another).
pub(crate) fn resolve_setting_ref<'a>(
    trait_ref: &'a Trait,
    ref_text: &str,
    field_path: &str,
) -> crate::Result<&'a crate::r#trait::Setting> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid setting ref {ref_text:?}"),
    })?;
    if parsed.kind() != Kind::Setting || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "expected a local setting:* ref".to_string(),
        }
        .into());
    }
    trait_ref
        .settings
        .iter()
        .find(|setting| setting.id == parsed.id())
        .ok_or_else(|| {
            crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!("unresolved setting ref {ref_text:?} names no declared setting"),
            }
            .into()
        })
}

/// A `setting:` loop bound: must name a declared `schema = "number"` setting
/// whose default (and bounds, when present) are whole numbers — the
/// "integerness is reference-site validation" ruling. Unlike a port loop
/// bound, a setting is not required to be declared as a loop input: settings
/// are resolved at activation, not accepted at runtime.
fn validate_integer_setting_ref(
    trait_ref: &Trait,
    ref_text: &str,
    field_path: &str,
) -> crate::Result<()> {
    let setting = resolve_setting_ref(trait_ref, ref_text, field_path)?;
    if setting.schema != crate::r#trait::SettingSchema::Number {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "dynamic loop bound requires a schema = \"number\" setting, got {:?}",
                setting.schema
            ),
        }
        .into());
    }
    let is_whole = |value: &serde_json::Number| value.as_f64().is_some_and(|n| n.fract() == 0.0);
    if !setting.default.as_f64().is_some_and(|n| n.fract() == 0.0) {
        // Not routed through `is_whole` above: `default` is a bare
        // `serde_json::Value`, not a `serde_json::Number`.
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "dynamic loop bound setting default {:?} must be a whole number",
                setting.default
            ),
        }
        .into());
    }
    if setting.min.as_ref().is_some_and(|min| !is_whole(min))
        || setting.max.as_ref().is_some_and(|max| !is_whole(max))
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "dynamic loop bound setting min/max must be whole numbers".to_string(),
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
    if item.on_complete.len() > 1 {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.on-complete"),
            message: "for-each on-complete accepts at most one signal".to_string(),
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
    let Some((output_path, kind)) =
        find_checklist_verdict_output_in_sequence(trait_ref, &sequence_id, &mut stack)
    else {
        return Ok(());
    };
    let message = match kind {
        ChecklistScalarKind::Declared(checklist_id) => format!(
            "for-each {base} body writes a scalar checklist verdict for resource:{checklist_id} at {output_path}; one-verdict-per-iteration accumulation across for-each iterations has no coverage proof. Emit every verdict in one whole-list replace write to a [schema:{checklist_id}-verdict] slot instead."
        ),
        ChecklistScalarKind::Produced => format!(
            "for-each {base} body writes a scalar produced-checklist item at {output_path}; one-item-per-iteration accumulation across for-each iterations has no coverage proof. Emit every item in one whole-list replace write to a [schema:checklist-item] slot instead."
        ),
    };
    Err(crate::manifest::Error::InvalidField {
        field_path: output_path.clone(),
        message,
    }
    .into())
}

/// The two shapes a scalar checklist write can take.
enum ChecklistScalarKind {
    /// A scalar `schema:<resource_id>-verdict` write for a declared checklist.
    Declared(String),
    /// A scalar `schema:checklist-item` write for a produced checklist.
    Produced,
}

fn find_checklist_verdict_output_in_sequence(
    trait_ref: &Trait,
    sequence_id: &str,
    stack: &mut BTreeSet<String>,
) -> Option<(String, ChecklistScalarKind)> {
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
) -> Option<(String, ChecklistScalarKind)> {
    for (index, item) in items.iter().enumerate() {
        for (output_index, output) in item.output.iter().enumerate() {
            if let Some(kind) = checklist_verdict_slot_schema(trait_ref, output.ref_text()) {
                return Some((
                    format!("sequence.{sequence_id}.sequence[{index}].output[{output_index}]"),
                    kind,
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

/// The checklist resource id when `ref_text` is a local slot or output port
/// declaring a scalar `schema:<id>-verdict` schema. `None` for whole-list
/// `[schema:...]` sinks (the supported shape) and every non-checklist schema.
fn checklist_verdict_slot_schema(
    trait_ref: &Trait,
    ref_text: &str,
) -> Option<ChecklistScalarKind> {
    let parsed = Reference::parse(ref_text).ok()?;
    if parsed.is_qualified() {
        return None;
    }
    let schema_ref = match parsed.kind() {
        Kind::Slot => local_slot_schema(trait_ref, parsed.id())?,
        Kind::Port => trait_ref
            .ports
            .iter()
            .find(|port| {
                port.id == parsed.id()
                    && matches!(port.direction, crate::r#trait::PortDirection::Output)
            })
            .map(|port| port.schema.clone())?,
        _ => return None,
    };
    if list_element_schema(&schema_ref).is_some() {
        return None;
    }
    let schema_id = schema_ref.trim().strip_prefix("schema:")?;
    if schema_id == "checklist-item" {
        return Some(ChecklistScalarKind::Produced);
    }
    crate::r#trait::checklist::checklist_for_verdict_schema(&trait_ref.resources, schema_id)
        .map(|checklist| ChecklistScalarKind::Declared(checklist.id.clone()))
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

/// The produced-checklist builtin is a schema-version-gated form: it must
/// not silently change how a schema-version "0.2" trait resolves, so a slot
/// or port declaring it requires a trait declaring "0.3" or "0.4" — the
/// same precedent as count-to-count comparisons (`condition.rs`).
fn validate_checklist_item_schema_version(t: &Trait) -> crate::Result<()> {
    let slot_schemas = t.slots.iter().filter_map(|slot| {
        slot.schema
            .as_ref()
            .map(|schema| ("slot", slot.id.as_str(), schema.as_str().into_owned()))
    });
    let port_schemas = t
        .ports
        .iter()
        .map(|port| ("port", port.id.as_str(), port.schema.clone()));
    let uses_checklist_item = slot_schemas
        .chain(port_schemas)
        .find(|(_, _, schema)| schema.trim_matches(['[', ']']) == "schema:checklist-item");
    let Some((kind, id, _)) = uses_checklist_item else {
        return Ok(());
    };
    if crate::r#trait::schema_version_at_least(t.schema_version.as_str(), "0.3") {
        return Ok(());
    }
    Err(crate::manifest::Error::InvalidField {
        field_path: format!("{kind}.{id}.schema"),
        message: format!(
            "{kind}:{id} declares schema:checklist-item, which requires a trait declaring schema-version \"0.3\" or newer, got {:?}",
            t.schema_version.as_str()
        ),
    }
    .into())
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
            if element_id == "checklist-item" {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: "append is not supported for produced checklists: coverage is checked against a whole item list, which only a replace write supplies. Emit every item in one write instead.".to_string(),
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

/// Shared mutable state of one produced-before-read walk: the static
/// producer facts computed up front, the recursion guard and memo, and
/// `co_produced` — for each slot, a snapshot of the guaranteed-produced set
/// at its FIRST producing step's completion. The snapshot is what makes
/// guard-implied production transitive: a branch guard that must have
/// measured slot X to route true proves X's producer ran, and therefore
/// everything guaranteed at that producer's completion — provided X has
/// exactly one producing step ([`Self::producer_counts`]), else no producer
/// is identifiable and nothing is implied.
struct ProducedBeforeReadWalk {
    first_producers: BTreeMap<String, String>,
    producer_counts: BTreeMap<String, usize>,
    stack: BTreeSet<String>,
    memo: ProducedBeforeReadMemo,
    co_produced: BTreeMap<String, BTreeSet<String>>,
}

fn validate_produced_before_read(trait_ref: &Trait) -> crate::Result<()> {
    let Some(procedure) = trait_ref.procedure.as_ref() else {
        return Ok(());
    };
    // A slot declared `optional`, or carrying a `default`, already holds
    // something a step may read — so it starts the walk produced. The two
    // reasons differ and both are true here: an optional slot says reading it
    // never blocks, and a defaulted slot has a value before any step runs.
    //
    // Seeded once at the declaration rather than asserted at each read. The
    // per-site `slot.optional()` still exists for a read that tolerates an
    // empty slot the DECLARATION does not call optional.
    let mut produced: BTreeSet<String> = trait_ref
        .slots
        .iter()
        .filter(|slot| slot.optional.unwrap_or(false) || slot.default.is_some())
        .map(|slot| format!("slot:{}", slot.id))
        .collect();
    let mut possible = BTreeSet::new();
    let mut walk = ProducedBeforeReadWalk {
        first_producers: first_slot_producers(trait_ref, procedure)?,
        producer_counts: slot_producer_counts(trait_ref, procedure)?,
        stack: BTreeSet::new(),
        memo: BTreeMap::new(),
        co_produced: BTreeMap::new(),
    };
    validate_produced_before_read_in_items(
        trait_ref,
        ordered_procedure_items(procedure)?.into_iter(),
        &mut produced,
        &mut possible,
        &mut walk,
    )?;
    Ok(())
}

fn ordered_procedure_items(procedure: &Model) -> crate::Result<Vec<(usize, &SequenceItem)>> {
    Ok(procedure.sequence.iter().enumerate().collect())
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
    possible: &BTreeSet<String>,
    walk: &mut ProducedBeforeReadWalk,
) -> crate::Result<(BTreeSet<String>, BTreeSet<String>)> {
    let key = (
        sequence_id.to_string(),
        produced.iter().cloned().collect::<Vec<_>>(),
        possible.iter().cloned().collect::<Vec<_>>(),
    );
    if let Some((outputs, possible_outputs, co_delta)) = walk.memo.get(&key).cloned() {
        // Replay the co-produced snapshots this walk recorded, so a cache
        // hit leaves the same guard-implication facts a fresh walk would.
        for (slot, snapshot) in co_delta {
            walk.co_produced.entry(slot).or_insert(snapshot);
        }
        return Ok((outputs, possible_outputs));
    }
    if !walk.stack.insert(sequence_id.to_string()) {
        return Ok((produced.clone(), possible.clone()));
    }
    let Some(sequence) = trait_ref.sequences.get(sequence_id) else {
        walk.stack.remove(sequence_id);
        return Ok((produced.clone(), possible.clone()));
    };
    let co_keys_before: BTreeSet<String> = walk.co_produced.keys().cloned().collect();
    let mut current = produced.clone();
    let mut current_possible = possible.clone();
    validate_produced_before_read_in_items(
        trait_ref,
        sequence.sequence.iter().enumerate(),
        &mut current,
        &mut current_possible,
        walk,
    )?;
    walk.stack.remove(sequence_id);
    let co_delta: BTreeMap<String, BTreeSet<String>> = walk
        .co_produced
        .iter()
        .filter(|(slot, _)| !co_keys_before.contains(*slot))
        .map(|(slot, snapshot)| (slot.clone(), snapshot.clone()))
        .collect();
    walk.memo.insert(
        key,
        (current.clone(), current_possible.clone(), co_delta),
    );
    Ok((current, current_possible))
}

/// Memo for [`validate_produced_before_read_in_sequence`]: keyed by the
/// sequence plus BOTH entry sets (results depend on each), holding the
/// guaranteed and possible output sets plus the co-produced snapshots first
/// recorded inside that walk.
type ProducedBeforeReadMemo = BTreeMap<
    (String, Vec<String>, Vec<String>),
    (
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeMap<String, BTreeSet<String>>,
    ),
>;

/// The produced-before-read walk, tracking two sets: `produced` — slots
/// guaranteed written on EVERY path reaching the current item — and
/// `possible` — slots written on AT LEAST ONE such path. Step inputs (and
/// `for-each over`) must be in `produced`: a step that runs cannot read
/// evidence its own path never wrote. Branch `when` guards only need
/// `possible`: at runtime a guard over a never-accepted slot is
/// `Unmeasurable` and routes false, so a rung whose predecessor never
/// produced its evidence simply does not fire — the linearized maybe-ladder
/// idiom (sibling `flow.when` chains). Reads of slots on NO path reaching
/// the reader stay refused everywhere.
fn validate_produced_before_read_in_items<'a>(
    trait_ref: &Trait,
    items: impl Iterator<Item = (usize, &'a SequenceItem)>,
    produced: &mut BTreeSet<String>,
    possible: &mut BTreeSet<String>,
    walk: &mut ProducedBeforeReadWalk,
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
                let error = if possible.contains(input) {
                    crate::reference::Error::SlotConditionallyProduced {
                        reader: reader.clone(),
                        ref_text: input.to_string(),
                        producer: walk.first_producers.get(input).cloned().unwrap_or_default(),
                    }
                } else {
                    match walk.first_producers.get(input) {
                        Some(producer) => crate::reference::Error::SlotProducedLater {
                            reader: reader.clone(),
                            ref_text: input.to_string(),
                            producer: producer.clone(),
                        },
                        None => crate::reference::Error::SlotNeverProduced {
                            reader: reader.clone(),
                            ref_text: input.to_string(),
                        },
                    }
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
                validate_guard_slots_produced(
                    trait_ref,
                    when,
                    possible,
                    &reader,
                    &walk.first_producers,
                )?;
            }
        if matches!(
            item.effective_kind(),
            SequenceKind::Sequence | SequenceKind::Loop | SequenceKind::ForEach
        )
            && let Some(sequence_id) = local_sequence_id(item.sequence.as_deref()) {
                // The for-each item is a runtime-bound local value for its body,
                // not an output that escapes to subsequent procedure items.
                let mut body_produced = produced.clone();
                let mut body_possible_base = possible.clone();
                let item_slot = item.item.clone();
                if let Some(item_slot) = item_slot.as_deref() {
                    body_produced.insert(item_slot.to_string());
                    body_possible_base.insert(item_slot.to_string());
                }
                let (mut body_outputs, mut body_possible) =
                    validate_produced_before_read_in_sequence(
                        trait_ref,
                        &sequence_id,
                        &body_produced,
                        &body_possible_base,
                        walk,
                    )?;
                if let Some(item_slot) = item_slot.filter(|item_slot| {
                    !produced.contains(item_slot)
                        && !sequence_explicitly_produces_slot(trait_ref, &sequence_id, item_slot)
                }) {
                    body_outputs.remove(&item_slot);
                    body_possible.remove(&item_slot);
                }
                if item.effective_kind() != SequenceKind::ForEach {
                    *produced = body_outputs;
                    *possible = body_possible;
                } else {
                    // A for-each may run zero times: nothing new is
                    // guaranteed, but everything its body can write is
                    // possible afterward.
                    possible.extend(body_possible);
                }
            }
        if item.effective_kind() == SequenceKind::Branch {
            // Guard-implied production: the then arm only runs when the
            // guard routed TRUE, which requires every slot in the guard's
            // measured-when-true set to hold accepted evidence — proving
            // that slot's one producing step ran, and with it everything
            // guaranteed at that step's completion (the co-produced
            // snapshot). "The verdict is approved" thereby implies the
            // whole arm that produced the verdict, transitively down a
            // sibling ladder. Slots with several producing steps imply
            // nothing (no single producer to pin), and the otherwise arm
            // gets no enrichment: it also runs on Unmeasurable, which
            // guarantees no measurement at all.
            let mut then_base = produced.clone();
            if let Some(when) = item.when.as_ref() {
                let mut measured = Vec::new();
                collect_guard_slots_measured_when(
                    trait_ref,
                    when,
                    true,
                    &mut measured,
                    &mut BTreeSet::new(),
                );
                for slot in measured {
                    if walk.producer_counts.get(&slot).copied() == Some(1)
                        && let Some(snapshot) = walk.co_produced.get(&slot)
                    {
                        then_base.extend(snapshot.iter().cloned());
                    }
                }
            }
            let mut then_possible_base = possible.clone();
            then_possible_base.extend(then_base.iter().cloned());
            let (then_outputs, then_possible) = match local_sequence_id(item.sequence.as_deref()) {
                Some(sequence_id) => validate_produced_before_read_in_sequence(
                    trait_ref,
                    &sequence_id,
                    &then_base,
                    &then_possible_base,
                    walk,
                )?,
                None => (then_base.clone(), then_possible_base.clone()),
            };
            let (otherwise_outputs, otherwise_possible) =
                match local_sequence_id(item.otherwise.as_deref()) {
                    Some(sequence_id) => validate_produced_before_read_in_sequence(
                        trait_ref,
                        &sequence_id,
                        produced,
                        possible,
                        walk,
                    )?,
                    None => (produced.clone(), possible.clone()),
                };
            // Divergence-aware join: an arm that ends in an authored terminal
            // (flow.error / flow.success) never reaches the code after the
            // branch, so its produced set must not shrink the post-branch
            // intersection. If only one arm diverges, the surviving arm's
            // set passes through unintersected; if both diverge, the join
            // point is unreachable and the pre-branch set is kept as a
            // harmless placeholder.
            let then_diverges = local_sequence_id(item.sequence.as_deref())
                .is_some_and(|id| sequence_ends_in_terminal(trait_ref, &id, &mut BTreeSet::new()));
            let otherwise_diverges = local_sequence_id(item.otherwise.as_deref())
                .is_some_and(|id| sequence_ends_in_terminal(trait_ref, &id, &mut BTreeSet::new()));
            *produced = match (then_diverges, otherwise_diverges) {
                (true, true) => produced.clone(),
                (true, false) => otherwise_outputs.clone(),
                (false, true) => then_outputs.clone(),
                (false, false) => then_outputs
                    .intersection(&otherwise_outputs)
                    .cloned()
                    .collect(),
            };
            // Either arm's writes are possible downstream, though only one
            // arm runs. Each arm was seeded with the PRE-branch possible
            // set, so a then-arm slot is never possible inside its own
            // otherwise arm — mutually exclusive arms stay strict.
            possible.extend(then_possible);
            possible.extend(otherwise_possible);
        }
        if item.effective_kind() == SequenceKind::Parallel {
            // Validate every branch against the same pre-panel produced set; the
            // union of non-skippable branch outputs is available after the panel.
            let mut union = produced.clone();
            let mut possible_union = possible.clone();
            for branch_ref in item.branches.iter() {
                let Some(sequence_id) = local_sequence_id(Some(branch_ref)) else {
                    continue;
                };
                if sequence_contains_terminal(trait_ref, &sequence_id, &mut BTreeSet::new()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: sequence_item_base(trait_ref, item, index),
                        message: format!(
                            "parallel branch {sequence_id:?} contains an authored terminal (flow.error/flow.success); terminals are not permitted inside parallel branches"
                        ),
                    }
                    .into());
                }
                let (branch_outputs, branch_possible) =
                    validate_produced_before_read_in_sequence(
                        trait_ref,
                        &sequence_id,
                        produced,
                        possible,
                        walk,
                    )?;
                let can_skip = item.branch_failure.iter().any(|entry| {
                    entry.branch == *branch_ref
                        && entry.on_failure == BranchFailurePolicy::Skip
                });
                if !can_skip {
                    union.extend(branch_outputs);
                }
                possible_union.extend(branch_possible);
            }
            *produced = union;
            *possible = possible_union;
        }
        if item.effective_kind() != SequenceKind::Branch {
            collect_local_slot_outputs(item, produced);
            collect_local_slot_outputs(item, possible);
            // Record each slot's first-producer snapshot: the guaranteed set
            // at this step's completion, the fact guard-implied production
            // replays inside then arms.
            for output in item.output.ref_texts() {
                if Reference::parse(output).is_ok_and(|parsed| {
                    parsed.kind() == Kind::Slot && !parsed.is_qualified()
                }) {
                    walk.co_produced
                        .entry(output.to_string())
                        .or_insert_with(|| produced.clone());
                }
            }
        }
    }
    Ok(())
}

/// Validate a branch `when` guard's slot reads against `available` — the
/// POSSIBLE set, not the guaranteed one: a guard over conditionally-produced
/// evidence is legal (absent at runtime means `Unmeasurable`, routing the
/// branch false), so only slots produced on NO path reaching the guard are
/// refused here.
fn validate_guard_slots_produced(
    trait_ref: &Trait,
    guard: &GuardExpr,
    available: &BTreeSet<String>,
    reader: &str,
    first_producers: &BTreeMap<String, String>,
) -> crate::Result<()> {
    let mut slots = Vec::new();
    collect_guard_slot_refs(trait_ref, guard, &mut slots, &mut BTreeSet::new());
    for slot in slots {
        if available.contains(&slot) {
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

/// How many distinct sequence items produce each slot, across the whole
/// procedure. Guard-implied production only fires for count == 1: with one
/// producing step, accepted evidence pins exactly which step ran; with
/// several, no snapshot is identifiable. Shared sub-sequences are visited
/// once (a sequence reachable from two branch items still holds the same
/// producing steps).
fn slot_producer_counts(
    trait_ref: &Trait,
    procedure: &Model,
) -> crate::Result<BTreeMap<String, usize>> {
    fn collect<'a>(
        trait_ref: &Trait,
        items: impl Iterator<Item = (usize, &'a SequenceItem)>,
        counts: &mut BTreeMap<String, usize>,
        visited: &mut BTreeSet<String>,
    ) {
        for (_, item) in items {
            let mut child_refs: Vec<&str> = item
                .sequence
                .as_deref()
                .into_iter()
                .chain(item.otherwise.as_deref())
                .collect();
            if item.effective_kind() == SequenceKind::Parallel {
                child_refs = item.branches.iter().map(String::as_str).collect();
            }
            for child_ref in child_refs {
                if let Some(sequence_id) = local_sequence_id(Some(child_ref))
                    && visited.insert(sequence_id.clone())
                    && let Some(sequence) = trait_ref.sequences.get(&sequence_id)
                {
                    collect(trait_ref, sequence.sequence.iter().enumerate(), counts, visited);
                }
            }
            for output in item.output.ref_texts() {
                if Reference::parse(output).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                }) {
                    *counts.entry(output.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut counts = BTreeMap::new();
    let mut visited = BTreeSet::new();
    collect(
        trait_ref,
        ordered_procedure_items(procedure)?.into_iter(),
        &mut counts,
        &mut visited,
    );
    Ok(counts)
}

/// The slots a guard must hold ACCEPTED evidence for whenever it routes the
/// way `routes_true` says — an under-approximation (missing a slot is
/// always sound; the caller just implies less). Polarity matters because
/// absent evidence is `Unmeasurable`, which never routes true and which
/// `not` preserves: a leaf routing true or measured-false proves its slot
/// accepted, `not` swaps the polarities, strong-Kleene `all` needs every
/// child true (union) but only SOME child false (intersection), and `any`
/// mirrors it. Leaves with version-dependent or tri-state absent semantics
/// (`count`, `present`-when-false, comparison right-hand sides) contribute
/// nothing rather than risk overclaiming.
fn collect_guard_slots_measured_when(
    trait_ref: &Trait,
    guard: &GuardExpr,
    routes_true: bool,
    slots: &mut Vec<String>,
    seen_conditions: &mut BTreeSet<String>,
) {
    fn child_sets(
        trait_ref: &Trait,
        items: &[GuardExpr],
        child_polarity: bool,
        seen_conditions: &mut BTreeSet<String>,
    ) -> Vec<BTreeSet<String>> {
        items
            .iter()
            .map(|item| {
                let mut child = Vec::new();
                collect_guard_slots_measured_when(
                    trait_ref,
                    item,
                    child_polarity,
                    &mut child,
                    seen_conditions,
                );
                child.into_iter().collect()
            })
            .collect()
    }
    fn extend_union(slots: &mut Vec<String>, sets: Vec<BTreeSet<String>>) {
        for set in sets {
            slots.extend(set);
        }
    }
    fn extend_intersection(slots: &mut Vec<String>, sets: Vec<BTreeSet<String>>) {
        let Some((first, rest)) = sets.split_first() else {
            return;
        };
        for slot in first {
            if rest.iter().all(|set| set.contains(slot)) {
                slots.push(slot.clone());
            }
        }
    }
    match guard {
        GuardExpr::Ref(reference) => {
            if let Ok(reference) = Reference::parse(reference)
                && reference.kind() == Kind::Condition
                && seen_conditions.insert(reference.id().to_string())
            {
                if let Some(condition) = trait_ref.conditions.get(reference.id()) {
                    collect_guard_slots_measured_when(
                        trait_ref,
                        &condition.as_guard(),
                        routes_true,
                        slots,
                        seen_conditions,
                    );
                }
                seen_conditions.remove(reference.id());
            }
        }
        // A bare array is `any` semantics: true needs only one child true
        // (intersection of guarantees), false needs every child false
        // (union).
        GuardExpr::Any(items) => {
            let sets = child_sets(trait_ref, items, routes_true, seen_conditions);
            if routes_true {
                extend_intersection(slots, sets);
            } else {
                extend_union(slots, sets);
            }
        }
        GuardExpr::Predicate(predicate) => {
            if let Some(not) = predicate.not.as_deref() {
                collect_guard_slots_measured_when(
                    trait_ref,
                    not,
                    !routes_true,
                    slots,
                    seen_conditions,
                );
                return;
            }
            if !predicate.all.is_empty() {
                let sets = child_sets(trait_ref, &predicate.all, routes_true, seen_conditions);
                if routes_true {
                    extend_union(slots, sets);
                } else {
                    extend_intersection(slots, sets);
                }
                return;
            }
            if !predicate.any.is_empty() {
                let sets = child_sets(trait_ref, &predicate.any, routes_true, seen_conditions);
                if routes_true {
                    extend_intersection(slots, sets);
                } else {
                    extend_union(slots, sets);
                }
                return;
            }
            if let Some(condition) = predicate.condition.as_deref() {
                collect_guard_slots_measured_when(
                    trait_ref,
                    &GuardExpr::Ref(condition.to_string()),
                    routes_true,
                    slots,
                    seen_conditions,
                );
                return;
            }
            // Slot-value and emptiness leaves measure their slot in BOTH
            // routed outcomes: Matched and NotMatched each require accepted
            // evidence (stale evidence is accepted too); only Unmeasurable
            // — which routes neither way through `not` — means unmeasured.
            for slot in [predicate.slot.as_deref(), predicate.empty.as_deref()]
                .into_iter()
                .flatten()
            {
                if Reference::parse(slot).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                }) {
                    slots.push(slot.to_string());
                }
            }
            // `present` proves its slot subject accepted only when it routes
            // true; `count` absent-behavior is schema-version-dependent —
            // both contribute only on the safe side.
            if routes_true
                && let Some(subject) = predicate.present.as_deref()
                && Reference::parse(subject).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                })
            {
                slots.push(subject.to_string());
            }
            if routes_true
                && let Some(count) = predicate.count.as_deref()
                && Reference::parse(count).is_ok_and(|reference| {
                    reference.kind() == Kind::Slot && !reference.is_qualified()
                })
            {
                slots.push(count.to_string());
            }
        }
    }
}

/// Whether every path through `sequence_id` ends in an authored terminal
/// (flow.error / flow.success): the last item is a terminal, or a branch
/// whose then AND otherwise arms both end in one. Cycle-guarded so a
/// self-referential local-sequence id (shouldn't occur, but validation must
/// not loop) resolves to "does not diverge" rather than hanging.
fn sequence_ends_in_terminal(
    trait_ref: &Trait,
    sequence_id: &str,
    seen: &mut BTreeSet<String>,
) -> bool {
    if !seen.insert(sequence_id.to_string()) {
        return false;
    }
    let result = trait_ref
        .sequences
        .get(sequence_id)
        .and_then(|sequence| sequence.sequence.last())
        .is_some_and(|item| item_ends_in_terminal(trait_ref, item, seen));
    seen.remove(sequence_id);
    result
}

fn item_ends_in_terminal(
    trait_ref: &Trait,
    item: &SequenceItem,
    seen: &mut BTreeSet<String>,
) -> bool {
    match item.effective_kind() {
        SequenceKind::Terminal => true,
        SequenceKind::Branch => {
            let then_diverges = local_sequence_id(item.sequence.as_deref())
                .is_some_and(|id| sequence_ends_in_terminal(trait_ref, &id, seen));
            let otherwise_diverges = local_sequence_id(item.otherwise.as_deref())
                .is_some_and(|id| sequence_ends_in_terminal(trait_ref, &id, seen));
            then_diverges && otherwise_diverges
        }
        _ => false,
    }
}

/// Whether `sequence_id`'s tree contains an authored terminal ANYWHERE —
/// mid-sequence, on a single arm of a nested branch, or nested inside a
/// branch/parallel item — regardless of whether that terminal is reached on
/// every path. Terminals are not position-constrained, so a
/// reachable-but-not-guaranteed terminal is still unsafe inside a parallel
/// branch: runtime dispatch would end the run mid-panel with undefined
/// barrier interaction. Cycle-guarded like `sequence_ends_in_terminal`.
fn sequence_contains_terminal(trait_ref: &Trait, sequence_id: &str, seen: &mut BTreeSet<String>) -> bool {
    if !seen.insert(sequence_id.to_string()) {
        return false;
    }
    let result = trait_ref
        .sequences
        .get(sequence_id)
        .is_some_and(|sequence| {
            sequence
                .sequence
                .iter()
                .any(|item| item_contains_terminal(trait_ref, item, seen))
        });
    seen.remove(sequence_id);
    result
}

fn item_contains_terminal(trait_ref: &Trait, item: &SequenceItem, seen: &mut BTreeSet<String>) -> bool {
    if item.effective_kind() == SequenceKind::Terminal {
        return true;
    }
    let branch_hit = local_sequence_id(item.sequence.as_deref())
        .is_some_and(|id| sequence_contains_terminal(trait_ref, &id, seen))
        || local_sequence_id(item.otherwise.as_deref())
            .is_some_and(|id| sequence_contains_terminal(trait_ref, &id, seen));
    if branch_hit {
        return true;
    }
    item.branches.iter().any(|branch_ref| {
        local_sequence_id(Some(branch_ref))
            .is_some_and(|id| sequence_contains_terminal(trait_ref, &id, seen))
    })
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

/// Whether two schema refs are compatible for a direct write: identical, or
/// either side is the `schema:any` wildcard.
fn schema_refs_compatible(a: &str, b: &str) -> bool {
    a == b || a == "schema:any" || b == "schema:any"
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
    if !schema_refs_compatible(&port.schema, &slot_schema) {
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

    fn minimal_trait_with_setting(setting: crate::r#trait::Setting) -> Trait {
        let mut trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            "id = \"setting-loop-bound-test\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Setting loop bound test\"\nsummary = \"Minimal fixture.\"\n",
        )
        .expect("minimal trait decodes");
        trait_ref.settings.push(setting);
        trait_ref
    }

    fn number_setting(id: &str, default: serde_json::Value) -> crate::r#trait::Setting {
        crate::r#trait::Setting {
            id: id.to_string(),
            schema: crate::r#trait::SettingSchema::Number,
            description: "A test setting.".to_string(),
            default,
            min: None,
            max: None,
        }
    }

    #[test]
    fn integer_setting_ref_accepts_a_declared_whole_number_setting() {
        let trait_ref =
            minimal_trait_with_setting(number_setting("review-rounds", serde_json::json!(3)));
        validate_integer_setting_ref(&trait_ref, "setting:review-rounds", "field")
            .expect("whole-number setting is a valid loop bound");
    }

    #[test]
    fn integer_setting_ref_rejects_unknown_setting_id() {
        let trait_ref =
            minimal_trait_with_setting(number_setting("review-rounds", serde_json::json!(3)));
        let error = validate_integer_setting_ref(&trait_ref, "setting:not-declared", "field")
            .expect_err("unknown setting id must fail the build");
        assert!(
            format!("{error}").contains("setting:not-declared"),
            "error must name the resolved id: {error}"
        );
    }

    #[test]
    fn integer_setting_ref_rejects_non_number_schema() {
        let trait_ref = minimal_trait_with_setting(crate::r#trait::Setting {
            id: "review-mode".to_string(),
            schema: crate::r#trait::SettingSchema::Text,
            description: "A test setting.".to_string(),
            default: serde_json::json!("strict"),
            min: None,
            max: None,
        });
        validate_integer_setting_ref(&trait_ref, "setting:review-mode", "field")
            .expect_err("a text setting cannot bind a loop bound");
    }

    #[test]
    fn integer_setting_ref_rejects_fractional_default() {
        let trait_ref =
            minimal_trait_with_setting(number_setting("review-rounds", serde_json::json!(2.5)));
        validate_integer_setting_ref(&trait_ref, "setting:review-rounds", "field")
            .expect_err("a fractional default fails the integerness-at-reference-site rule");
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
    fn loop_rejects_on_complete() {
        let item = item_from_toml(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\non-complete = [\"signal:done\"]\n",
        );
        let error = validate_item_shape(&item, SequenceKind::Loop, "procedure.sequence[0]")
            .expect_err("loop items never become ReadyItem, so on-complete is a silent no-op");
        assert!(
            format!("{error}").contains("on-complete"),
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

    /// (0093) A loop with neither a bound nor an exit guard can never end;
    /// refusal must name `max-iterations`.
    #[test]
    fn loop_with_no_bound_and_no_guard_is_refused_by_name() {
        const FIXTURE: &str = r#"
id = "unbounded-no-guard-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Unbounded No Guard Fixture"
description = "Regression fixture: a loop with neither a bound nor until/abort-if is refused."

[[agent]]
id = "reviewer"
description = "Produces the loop body output."
summary = "Reviewer role."

[prompt.review]
text = "Do the loop body work."

[[sequence.loop-body.sequence]]
id = "produce-output"
title = "Produce output"
agent = "agent:reviewer"
prompt = "prompt:review"

[procedure]
description = "One loop with neither a bound nor an exit guard."

[[procedure.sequence]]
id = "unbounded-loop"
title = "Unbounded loop"
kind = "loop"
sequence = "sequence:loop-body"
"#;
        let trait_ref: crate::r#trait::Trait =
            toml::from_str(FIXTURE).expect("fixture trait parses");
        let error = validate(&trait_ref)
            .expect_err("a loop with no bound and no exit guard can never end");
        let message = format!("{error}");
        assert!(
            message.contains("max-iterations") && message.contains("can never end"),
            "error must name max-iterations and explain why: {error}"
        );
    }

    /// (0093) `on-exhausted` without a bound is meaningless — an unbounded
    /// loop cannot exhaust; refusal must name `on-exhausted`.
    #[test]
    fn loop_with_on_exhausted_and_no_bound_is_refused_by_name() {
        const FIXTURE: &str = r#"
id = "unbounded-on-exhausted-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Unbounded On Exhausted Fixture"
description = "Regression fixture: on-exhausted without a bound is refused."

[[agent]]
id = "reviewer"
description = "Produces the typed verdict for the loop."
summary = "Reviewer role."

[[slot]]
id = "verdict"
schema = "schema:verdict"
description = "Typed verdict carrying a status field."

[[schema]]
id = "verdict"
description = "Verdict object with a status enum."

[schema.fields.status]
schema = "schema:text"
required = true
description = "approved or revise."
allowed = [
    "approved",
    "revise",
]

[prompt.review]
text = "Produce the typed verdict object."

[[sequence.loop-body.sequence]]
id = "produce-verdict"
title = "Produce verdict"
agent = "agent:reviewer"
prompt = "prompt:review"
output = ["slot:verdict"]

[procedure]
description = "One unbounded loop that declares on-exhausted despite having no bound."

[[procedure.sequence]]
id = "verdict-loop"
title = "Verdict loop"
kind = "loop"
sequence = "sequence:loop-body"
on-exhausted = "continue"

[procedure.sequence.until]
slot = "slot:verdict"
field = "status"
equals = "approved"
"#;
        let trait_ref: crate::r#trait::Trait =
            toml::from_str(FIXTURE).expect("fixture trait parses");
        let error = validate(&trait_ref)
            .expect_err("on-exhausted without a bound must be refused");
        let message = format!("{error}");
        assert!(
            message.contains("on-exhausted") && message.contains("cannot exhaust"),
            "error must name on-exhausted and explain why: {error}"
        );
    }

    #[test]
    fn validate_exhaustion_target_accepts_keywords() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_exhaustion_target(&ExhaustionTarget::One("continue".to_string()), "x.on-exhausted", &signal_ids)
            .expect("\"continue\" is a legal keyword");
        validate_exhaustion_target(&ExhaustionTarget::One("abort".to_string()), "x.on-exhausted", &signal_ids)
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

    fn loop_item_with_stop(abort_if: bool, on_abort: Option<&str>) -> SequenceItem {
        let abort_if_line = if abort_if { "abort-if = { slot = \"slot:verdict\", field = \"status\", equals = \"revise\" }\n" } else { "" };
        let on_abort_line = on_abort
            .map(|value| format!("on-abort = \"{value}\"\n"))
            .unwrap_or_default();
        item_from_toml(&format!(
            "id = \"refinement-loop\"\ntitle = \"Refine\"\nkind = \"loop\"\nsequence = \"sequence:refine-work\"\nmax-iterations = 3\n{abort_if_line}{on_abort_line}"
        ))
    }

    #[test]
    fn loop_shape_allows_on_abort_field() {
        validate_item_shape(
            &loop_item_with_stop(true, Some("signal:recurring-blocker-unresolved")),
            SequenceKind::Loop,
            "procedure.sequence[0]",
        )
        .expect("on-abort is a legal loop field at the shape level");
    }

    #[test]
    fn on_abort_requires_abort_if() {
        let item = loop_item_with_stop(false, Some("signal:recurring-blocker-unresolved"));
        let error = validate_on_abort_requires_abort_if(&item, "procedure.sequence[0]")
            .expect_err("on-abort without abort-if must be rejected");
        assert!(
            format!("{error}").contains("abort-if"),
            "error must steer authors toward abort-if: {error}"
        );
    }

    #[test]
    fn on_abort_allowed_alongside_abort_if() {
        let item = loop_item_with_stop(true, Some("signal:recurring-blocker-unresolved"));
        validate_on_abort_requires_abort_if(&item, "procedure.sequence[0]")
            .expect("on-abort declared alongside abort-if is legal");
    }

    #[test]
    fn non_loop_item_rejects_on_abort() {
        let item = item_from_toml(
            "id = \"stage\"\ntitle = \"Stage\"\nkind = \"sequence\"\nsequence = \"sequence:stage\"\non-abort = \"signal:done\"\n",
        );
        validate_item_shape(&item, SequenceKind::Sequence, "procedure.sequence[0]")
            .expect_err("on-abort is loop-only");
    }

    #[test]
    fn validate_abort_signal_target_accepts_resolved_signal() {
        let signal_ids: BTreeSet<&str> = ["recurring-blocker-unresolved"].into_iter().collect();
        validate_abort_signal_target(
            &ExhaustionTarget::One("signal:recurring-blocker-unresolved".to_string()),
            "x.on-abort",
            &signal_ids,
        )
        .expect("a declared local signal ref resolves");
    }

    #[test]
    fn validate_abort_signal_target_rejects_unresolved_signal() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_abort_signal_target(
            &ExhaustionTarget::One("signal:missing".to_string()),
            "x.on-abort",
            &signal_ids,
        )
        .expect_err("an unresolved local signal ref must be rejected");
        assert!(
            format!("{error}").contains("signal:missing"),
            "error must name the offending ref: {error}"
        );
    }

    #[test]
    fn validate_abort_signal_target_rejects_continue_keyword() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        let error = validate_abort_signal_target(
            &ExhaustionTarget::One("continue".to_string()),
            "x.on-abort",
            &signal_ids,
        )
        .expect_err("an abort-if match always halts the loop, so \"continue\" is meaningless here");
        assert!(
            format!("{error}").contains("policy keyword"),
            "error must explain why the keyword is rejected: {error}"
        );
    }

    #[test]
    fn validate_abort_signal_target_rejects_abort_keyword() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_abort_signal_target(&ExhaustionTarget::One("abort".to_string()), "x.on-abort", &signal_ids)
            .expect_err("\"block\" is equally meaningless for on-abort");
    }

    #[test]
    fn validate_abort_signal_target_rejects_empty_list() {
        let signal_ids: BTreeSet<&str> = BTreeSet::new();
        validate_abort_signal_target(&ExhaustionTarget::Many(Vec::new()), "x.on-abort", &signal_ids)
            .expect_err("an empty list is not a legal declaration");
    }

    #[test]
    fn validate_abort_signal_target_rejects_duplicate_signals() {
        let signal_ids: BTreeSet<&str> = ["a"].into_iter().collect();
        let error = validate_abort_signal_target(
            &ExhaustionTarget::Many(vec!["signal:a".to_string(), "signal:a".to_string()]),
            "x.on-abort",
            &signal_ids,
        )
        .expect_err("duplicate signal entries must be rejected");
        assert!(
            format!("{error}").contains("duplicate"),
            "error must call out the duplication: {error}"
        );
    }

    /// A base trait carrying a `review` checklist, its companion verdict
    /// schema, a scalar `verdict` slot, a whole-list `verdicts` slot, and a
    /// scalar-verdict-typed output port — one named sequence (`sequence:body`)
    /// whose single item nests `sequence:branch`, whose own prompt step
    /// writes `output` (patched in by each test) to one of those sinks. The
    /// `for-each` in these tests points its `.sequence` ref at `body`, so the
    /// write is reachable one level of nesting deep.
    fn checklist_for_each_fixture_trait(body_output: &str) -> Trait {
        let toml_src = format!(
            r#"
id = "checklist-for-each-fixture"
schema-version = "0.3"
version = "0.1.0"
name = "Checklist for-each fixture"
description = "Test fixture."

[[resource]]
id = "review"
variant = "checklist"

[[resource.item]]
id = "thoroughness"
text = "Is it thorough?"

[[schema]]
id = "review-verdict"

[schema.fields.item]
schema = "schema:text"
required = true
allowed = ["thoroughness"]

[schema.fields.status]
schema = "schema:text"
required = true
allowed = ["pass", "fail", "waived"]

[[slot]]
id = "verdict"
schema = "schema:review-verdict"

[[slot]]
id = "verdicts"
schema = "[schema:review-verdict]"

[[port]]
id = "verdict-port"
direction = "output"
schema = "schema:review-verdict"
description = "Terminal verdict port."

[sequence.branch]
sequence = [
    {{ id = "nested-answer", title = "Nested answer", kind = "prompt", prompt = "Answer the checklist.", output = ["{body_output}"] }},
]

[sequence.body]
sequence = [
    {{ id = "answer", title = "Answer", kind = "sequence", sequence = "sequence:branch" }},
]
"#
        );
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect("fixture trait must decode: it is not itself the shape under test")
    }

    fn for_each_over_body_item() -> SequenceItem {
        item_from_toml(
            "id = \"per-item\"\ntitle = \"Per item\"\nkind = \"for-each\"\nsequence = \"sequence:body\"\n",
        )
    }

    #[test]
    fn for_each_body_writing_scalar_checklist_verdict_via_nested_sequence_is_refused() {
        let trait_ref = checklist_for_each_fixture_trait("slot:verdict");
        let error = validate_for_each_no_scalar_checklist_verdict(
            &trait_ref,
            &for_each_over_body_item(),
            "procedure.sequence[0]",
        )
        .expect_err(
            "a scalar verdict write reachable through the nested sequence one level under the \
             for-each body has no coverage proof and must be refused",
        );
        let message = format!("{error}");
        assert!(
            message.contains("whole-list replace write"),
            "error must point at the replace shape: {message}"
        );
        assert!(
            message.contains("resource:review"),
            "error must name the checklist resource: {message}"
        );
    }

    #[test]
    fn for_each_body_replace_writing_whole_verdict_list_is_allowed() {
        let trait_ref = checklist_for_each_fixture_trait("slot:verdicts");
        validate_for_each_no_scalar_checklist_verdict(
            &trait_ref,
            &for_each_over_body_item(),
            "procedure.sequence[0]",
        )
        .expect("a whole-list replace write is the coverage-checked shape and must pass");
    }

    #[test]
    fn for_each_body_writing_scalar_checklist_verdict_via_output_port_is_refused() {
        let trait_ref = checklist_for_each_fixture_trait("port:verdict-port");
        let error = validate_for_each_no_scalar_checklist_verdict(
            &trait_ref,
            &for_each_over_body_item(),
            "procedure.sequence[0]",
        )
        .expect_err(
            "an output port typed with a scalar checklist-verdict schema is the same \
             one-verdict-per-iteration hole as a scalar slot and must be refused too",
        );
        assert!(
            format!("{error}").contains("whole-list replace write"),
            "error must point at the replace shape: {error}"
        );
    }

    fn produced_checklist_fixture_trait(schema_version: &str) -> Trait {
        let toml_src = format!(
            r#"
id = "produced-checklist-validate-fixture"
schema-version = "{schema_version}"
version = "0.1.0"
name = "Produced checklist validate fixture"
description = "Test fixture."

[[slot]]
id = "plan"
schema = "[schema:checklist-item]"

[[slot]]
id = "plan-item"
schema = "schema:checklist-item"
"#
        );
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect("fixture trait must decode: it is not itself the shape under test")
    }

    #[test]
    fn produced_checklist_requires_schema_version_0_3_or_0_4() {
        let trait_ref = produced_checklist_fixture_trait("0.3");
        validate(&trait_ref)
            .expect("schema-version 0.3 must be accepted for schema:checklist-item");
    }

    #[test]
    fn produced_checklist_on_schema_version_0_2_is_rejected() {
        let toml_src = r#"
id = "produced-checklist-old-version-fixture"
schema-version = "0.2"
version = "0.1.0"
name = "Produced checklist old version fixture"
description = "Test fixture."

[[slot]]
id = "plan"
schema = "[schema:checklist-item]"
"#;
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, toml_src)
            .expect_err("schema:checklist-item requires schema-version 0.3 or 0.4");
        assert!(
            format!("{error}").contains("schema:checklist-item"),
            "error must name the gated builtin: {error}"
        );
    }

    /// The sibling `flow.when` ladder fixture: `rung-one` produces
    /// `first-out` only inside its arm, and `rung-two` — a SIBLING, not a
    /// nested block — reads it. `extra` splices per-test variations in.
    fn sibling_ladder_toml(rung_two_when: &str, extra: &str) -> String {
        format!(
            r#"
id = "sibling-when-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Sibling when fixture"
description = "Test fixture."

[[slot]]
id = "zero-out"
schema = "schema:text"

[[slot]]
id = "first-out"
schema = "schema:text"

[[slot]]
id = "second-out"
schema = "schema:text"

[[slot]]
id = "ghost"
schema = "schema:text"

[[slot]]
id = "late-out"
schema = "schema:text"

[[sequence.arm-one.sequence]]
id = "produce-first"
title = "Produce first"
output = ["slot:first-out"]
command = {{ argv = ["printf", "first"] }}

[[sequence.arm-two.sequence]]
id = "produce-second"
title = "Produce second"
output = ["slot:second-out"]
command = {{ argv = ["printf", "second"] }}
{extra}
[procedure]
description = "Sibling when ladder."

[[procedure.sequence]]
id = "step-zero"
title = "Step zero"
output = ["slot:zero-out"]
command = {{ argv = ["git", "status", "--porcelain"] }}

[[procedure.sequence]]
id = "rung-one"
title = "Rung one"
kind = "branch"
sequence = "sequence:arm-one"
when = {{ empty = "slot:zero-out" }}

[[procedure.sequence]]
id = "rung-two"
title = "Rung two"
kind = "branch"
sequence = "sequence:arm-two"
when = {rung_two_when}

[[procedure.sequence]]
id = "produce-late"
title = "Produce late"
output = ["slot:late-out"]
command = {{ argv = ["printf", "late"] }}
"#
        )
    }

    #[test]
    fn sibling_branch_guard_may_read_conditionally_produced_slot() {
        let toml_src =
            sibling_ladder_toml(r#"{ slot = "slot:first-out", equals = "first" }"#, "");
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src).expect(
            "a sibling branch guard over a conditionally-produced slot is the linearized \
             maybe-ladder idiom and must validate",
        );
    }

    #[test]
    fn step_input_reading_conditionally_produced_slot_is_refused() {
        // The guard reads only zero-out (produced unconditionally), so it
        // implies nothing about first-out — the arm step's read of it stays
        // a conditional-production refusal. (A guard reading first-out
        // itself WOULD legalize the read — that is guard-implied
        // production, proven separately below.)
        let toml_src = sibling_ladder_toml(
            r#"{ slot = "slot:zero-out", equals = "clean" }"#,
            "input = [\"slot:first-out\"]\n",
        );
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("a step INPUT over an unimplied conditionally-produced slot stays refused");
        assert!(
            format!("{error}").contains("only produced inside a conditional branch"),
            "error must name the conditional production, not claim a later producer: {error}"
        );
    }

    #[test]
    fn guard_reading_slot_produced_only_later_stays_refused() {
        let toml_src = sibling_ladder_toml(r#"{ slot = "slot:late-out", equals = "late" }"#, "");
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("a guard over a slot produced only later stays refused");
        assert!(
            format!("{error}").contains("first produced by later step 'produce-late'"),
            "error must name the later producer: {error}"
        );
    }

    #[test]
    fn guard_reading_never_produced_slot_stays_refused() {
        let toml_src = sibling_ladder_toml(r#"{ slot = "slot:ghost", equals = "boo" }"#, "");
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("a guard over a never-produced slot stays refused");
        assert!(
            format!("{error}").contains("never produced by any step"),
            "error must say the slot has no producer at all: {error}"
        );
    }

    #[test]
    fn guard_in_otherwise_arm_cannot_read_then_arm_slot() {
        // The otherwise arm holds a nested branch whose guard reads the THEN
        // arm's output — mutually exclusive arms, so the slot is possible on
        // no path reaching that guard; the relaxation must not legalize it.
        let toml_src = sibling_ladder_toml(
            r#"{ slot = "slot:zero-out", equals = "x" }"#,
            r#"
[[sequence.arm-reader.sequence]]
id = "nested-rung"
title = "Nested rung"
kind = "branch"
sequence = "sequence:arm-two"
when = { slot = "slot:first-out", equals = "first" }
"#,
        )
        .replace(
            "when = { empty = \"slot:zero-out\" }",
            "when = { empty = \"slot:zero-out\" }\notherwise = \"sequence:arm-reader\"",
        );
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("a guard in the otherwise arm reading the then arm's slot stays refused");
        assert!(
            format!("{error}").contains("first produced by later step 'produce-first'"),
            "the arms are exclusive, so the read must stay refused: {error}"
        );
    }

    /// Three-rung ladder for guard-implied production: the version arm
    /// produces `new-version`, then the `verdict`, then `post-out`; the
    /// changelog rung guards on the verdict and its arm step reads
    /// `reader_input`.
    fn verdict_ladder_toml(reader_input: &str, extra: &str) -> String {
        format!(
            r#"
id = "verdict-ladder-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Verdict ladder fixture"
description = "Test fixture."

[[slot]]
id = "zero-out"
schema = "schema:text"

[[slot]]
id = "new-version"
schema = "schema:text"

[[slot]]
id = "verdict"
schema = "schema:text"

[[slot]]
id = "post-out"
schema = "schema:text"

[[slot]]
id = "changelog-out"
schema = "schema:text"

[[sequence.arm-version.sequence]]
id = "bump-the-declared-version-files"
title = "Bump the declared version files"
output = ["slot:new-version"]
command = {{ argv = ["printf", "1.2.3"] }}

[[sequence.arm-version.sequence]]
id = "review-the-version-bump"
title = "Review the version bump"
input = ["slot:new-version"]
output = ["slot:verdict"]
command = {{ argv = ["printf", "approved"] }}

[[sequence.arm-version.sequence]]
id = "after-the-verdict"
title = "After the verdict"
output = ["slot:post-out"]
command = {{ argv = ["printf", "late"] }}

[[sequence.arm-changelog.sequence]]
id = "draft-the-changelog-entry"
title = "Draft the changelog entry"
input = ["{reader_input}"]
output = ["slot:changelog-out"]
command = {{ argv = ["printf", "entry"] }}
{extra}
[procedure]
description = "Verdict ladder."

[[procedure.sequence]]
id = "step-zero"
title = "Step zero"
output = ["slot:zero-out"]
command = {{ argv = ["git", "status", "--porcelain"] }}

[[procedure.sequence]]
id = "rung-version"
title = "Rung version"
kind = "branch"
sequence = "sequence:arm-version"
when = {{ empty = "slot:zero-out" }}

[[procedure.sequence]]
id = "rung-changelog"
title = "Rung changelog"
kind = "branch"
sequence = "sequence:arm-changelog"
when = {{ slot = "slot:verdict", equals = "approved" }}
"#
        )
    }

    #[test]
    fn sibling_rung_step_input_may_read_slots_implied_by_the_guarded_verdict() {
        // "The verdict is approved" proves the version arm ran through the
        // verdict's producer — so a LATER sibling rung's step may read
        // new-version, produced in that arm before the verdict.
        let toml_src = verdict_ladder_toml("slot:new-version", "");
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect("guard-implied production must admit reads of the verdict's co-produced slots");
    }

    #[test]
    fn guard_implication_stops_at_the_guarded_slots_producer() {
        // post-out is produced AFTER the verdict in the same arm: verdict
        // evidence proves nothing about it, so the read stays refused.
        let toml_src = verdict_ladder_toml("slot:post-out", "");
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("slots produced after the guarded slot are not implied");
        assert!(
            format!("{error}").contains("only produced inside a conditional branch"),
            "the refusal must stay the conditional-production one: {error}"
        );
    }

    #[test]
    fn guard_implication_requires_a_single_producer() {
        // A second producer of the verdict means accepted evidence no
        // longer pins WHICH producer ran — no snapshot is implied.
        let toml_src = verdict_ladder_toml(
            "slot:new-version",
            r#"
[[sequence.arm-changelog.sequence]]
id = "another-verdict-writer"
title = "Another verdict writer"
output = ["slot:verdict"]
command = { argv = ["printf", "approved"] }
"#,
        );
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &toml_src)
            .expect_err("a multi-producer guarded slot implies nothing");
        assert!(
            format!("{error}").contains("only produced inside a conditional branch"),
            "the read must stay refused when the producer is ambiguous: {error}"
        );
    }

    #[test]
    fn produced_checklist_append_is_statically_refused() {
        let trait_ref = produced_checklist_fixture_trait("0.3");
        let parsed = Reference::parse("slot:plan").expect("valid ref");
        let sink = OutputSink::SlotOperation {
            slot: "slot:plan".to_string(),
            operation: WriteOperation::Append,
            optional: false,
        };
        let error = validate_output_sink_operation(&trait_ref, &sink, &parsed, "field")
            .expect_err("append to a produced checklist has no coverage proof and must be refused");
        assert!(
            format!("{error}").contains("one write instead"),
            "error must point at the replace shape: {error}"
        );
    }

    #[test]
    fn produced_checklist_for_each_scalar_write_is_statically_refused() {
        let toml_src = r#"
id = "produced-checklist-for-each-fixture"
schema-version = "0.3"
version = "0.1.0"
name = "Produced checklist for-each fixture"
description = "Test fixture."

[[slot]]
id = "item"
schema = "schema:checklist-item"

[sequence.body]
sequence = [
    { id = "answer", title = "Answer", kind = "prompt", prompt = "Answer.", output = ["slot:item"] },
]
"#;
        let trait_ref = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, toml_src)
            .expect("fixture trait must decode: it is not itself the shape under test");
        let error = validate_for_each_no_scalar_checklist_verdict(
            &trait_ref,
            &for_each_over_body_item(),
            "procedure.sequence[0]",
        )
        .expect_err(
            "a scalar produced-checklist-item write under for-each has no coverage proof and must be refused",
        );
        assert!(
            format!("{error}").contains("whole-list replace write"),
            "error must point at the replace shape: {error}"
        );
    }

    // -----------------------------------------------------------------
    // 0206: command/check steps output directly to output ports
    // -----------------------------------------------------------------

    /// A schema-version to splice into the 0206 command/check port fixture,
    /// and whether the command item's output is a port (`slot:*` otherwise).
    fn command_port_fixture(schema_version: &str) -> String {
        format!(
            r#"
id = "command-port-fixture"
schema-version = "{schema_version}"
version = "0.1.0"
name = "Command port fixture"
description = "0206 fixture: a command step writing directly to an output port."

[[port]]
id = "commit-report"
direction = "output"
schema = "schema:text"
description = "Direct command output port."

[procedure]
description = "One command step writing to a port."

[[procedure.sequence]]
id = "commit"
title = "Commit"
kind = "command"
cmd = "git commit -m msg"
output = ["port:commit-report"]
"#
        )
    }

    #[test]
    fn command_output_port_accepted_at_schema_version_0_5() {
        let trait_ref =
            crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &command_port_fixture("0.5"))
                .expect("0206 fixture must decode");
        validate(&trait_ref).expect("command output to a declared port is accepted at 0.5");
    }

    #[test]
    fn command_output_port_refused_below_schema_version_0_5() {
        let error = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            &command_port_fixture("0.4"),
        )
        .expect_err("command output to a port below the 0.5 floor must be refused");
        assert!(
            format!("{error}").contains("schema-version \"0.5\" or newer"),
            "error must name the version floor, not decode as an unknown-field error: {error}"
        );
    }

    #[test]
    fn command_output_port_not_consumed_is_rejected() {
        let mut src = command_port_fixture("0.5");
        src.push_str(
            r#"
[[procedure.sequence]]
id = "echo-back"
title = "Echo back"
kind = "prompt"
prompt = "prompt:echo"
input = ["port:commit-report"]
output = ["port:commit-report"]

[prompt.echo]
text = "Echo the report."
"#,
        );
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &src)
            .expect_err("a directly-written output port must not be consumed as input");
        assert!(
            format!("{error}").contains("must not be consumed as internal state"),
            "error must name the not-consumed rule: {error}"
        );
    }

    #[test]
    fn command_output_port_schema_text_is_accepted() {
        let trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            &command_port_fixture("0.5"),
        )
        .expect("0206 fixture must decode");
        validate(&trait_ref)
            .expect("a schema:text command output port is compatible with the text capture");
    }

    #[test]
    fn command_output_port_incompatible_schema_is_rejected() {
        let src = command_port_fixture("0.5").replace(
            "schema = \"schema:text\"\ndescription = \"Direct command output port.\"",
            "schema = \"schema:command-typed\"\ndescription = \"Direct command output port.\"\n\n[[schema]]\nid = \"command-typed\"\n\n[schema.fields.ok]\nschema = \"schema:boolean\"\nrequired = true",
        );
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &src)
            .expect_err("a typed command output port is incompatible with the text capture and must be refused");
        assert!(
            format!("{error}").contains("incompatible with a command's text capture"),
            "error must name the text-capture incompatibility: {error}"
        );
    }

    fn check_port_fixture(outputs: &str) -> String {
        format!(
            r#"
id = "check-port-fixture"
schema-version = "0.5"
version = "0.1.0"
name = "Check port fixture"
description = "0206 fixture: a check step writing directly to an output port."

[[port]]
id = "verdict-report"
direction = "output"
schema = "schema:check-verdict"
description = "Direct check output port."

[[slot]]
id = "verdict"
schema = "schema:check-verdict"
description = "Check verdict slot."

[[schema]]
id = "check-verdict"
description = "P565 verdict shape."

[schema.fields.ok]
schema = "schema:boolean"
required = true

[schema.fields.argv]
schema = "[schema:text]"
required = true

[procedure]
description = "One check step writing to a port."

[[procedure.sequence]]
id = "gate"
title = "Gate"
kind = "check"
cmd = "true"
{outputs}
"#
        )
    }

    #[test]
    fn check_output_port_accepted_at_schema_version_0_5() {
        let trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            &check_port_fixture(r#"output = ["port:verdict-report"]"#),
        )
        .expect("0206 check-port fixture must decode");
        validate(&trait_ref).expect("check output to a declared port is accepted at 0.5");
    }

    #[test]
    fn check_output_slot_and_port_accepted_together() {
        let trait_ref = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            &check_port_fixture(r#"output = ["slot:verdict", "port:verdict-report"]"#),
        )
        .expect("0206 dual-sink check fixture must decode");
        validate(&trait_ref)
            .expect("a check may write its slot and its port together, per the scope ruling");
    }

    #[test]
    fn check_output_rejects_two_slots() {
        let error = crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            &check_port_fixture(r#"output = ["slot:verdict", "slot:verdict"]"#),
        )
        .expect_err("two outputs of the same kind is never the blessed shape");
        assert!(
            format!("{error}").contains("duplicate sequence output sink"),
            "error must name the offending shape: {error}"
        );
    }

    #[test]
    fn check_output_port_schema_must_match_p565_verdict_shape() {
        let mut src = check_port_fixture(r#"output = ["port:verdict-report"]"#);
        src = src.replace("schema = \"schema:check-verdict\"\ndescription = \"Direct check output port.\"", "schema = \"schema:text\"\ndescription = \"Direct check output port.\"");
        let error = crate::encoding::decode_trait(crate::encoding::Encoding::Toml, &src)
            .expect_err("a check's port output must satisfy the same P565 shape as its slot output");
        assert!(
            format!("{error}").contains("ok") && format!("{error}").contains("argv"),
            "error must name the required verdict shape: {error}"
        );
    }
}
