// Procedure runtime schema validation.

fn procedure(trait_ref: &Trait) -> crate::Result<&crate::r#trait::procedure::Model> {
    trait_ref.procedure.as_ref().ok_or_else(|| {
        crate::procedure::invalid_field("procedure", "trait has no [procedure] section")
    })
}

fn validate_initial_port_value(trait_ref: &Trait, value: StepSlotOutput) -> crate::Result<Value> {
    let parsed = Reference::parse(&value.ref_text).map_err(|_| {
        crate::procedure::invalid_field(
            "runtime.input",
            format!("invalid input ref {:?}", value.ref_text),
        )
    })?;
    if parsed.kind() != Kind::Port || parsed.is_qualified() {
        let digest = value_digest(&value.value)?;
        return Ok(Value {
            ref_text: value.ref_text,
            value: value.value,
            value_digest: digest,
            schema_ref: None,
            source: value.source.unwrap_or(ValueSource::HostInput),
            producer_evidence: value.producer_evidence,
            command_execution: value.command_execution,
            producer_agent: value.producer_agent,
            producer_harness: value.producer_harness,
            producer_check_verdict: false,
            acceptance: AcceptanceStatus::Rejected,
            position_path: Vec::new(),
            acceptance_order: None,
            schema_validation: vec![SchemaValidation {
                ref_text: "runtime.input".to_string(),
                schema_ref: None,
                status: SchemaStatus::Rejected,
                reason: "initial inputs must be local port:* refs".to_string(),
            }],
        });
    }

    let schema_ref = trait_ref
        .ports
        .iter()
        .find(|port| port.id == parsed.id() && matches!(port.direction, PortDirection::Input))
        .map(|port| port.schema.clone());
    let validation = match schema_ref.as_deref() {
        Some(schema_ref) => {
            validate_value_schema(trait_ref, &value.ref_text, schema_ref, &value.value)?
        }
        None => SchemaValidation {
            ref_text: value.ref_text.clone(),
            schema_ref: None,
            status: SchemaStatus::Rejected,
            reason: "input port is not declared".to_string(),
        },
    };
    let acceptance = if validation.status == SchemaStatus::Accepted {
        AcceptanceStatus::Accepted
    } else {
        AcceptanceStatus::Rejected
    };
    Ok(Value {
        ref_text: value.ref_text,
        value_digest: value_digest(&value.value)?,
        value: value.value,
        schema_ref: schema_ref
            .as_deref()
            .map(runtime_schema_reference)
            .transpose()?
            .flatten(),
        source: value.source.unwrap_or(ValueSource::HostInput),
        producer_evidence: value.producer_evidence,
        command_execution: value.command_execution,
        producer_agent: value.producer_agent,
        producer_harness: value.producer_harness,
        producer_check_verdict: false,
        acceptance,
        position_path: Vec::new(),
        acceptance_order: None,
        schema_validation: vec![validation],
    })
}

/// Reject a checklist verdict list that does not answer every declared item
/// exactly once.
///
/// This is the obligation a schema cannot carry. An `enum` on the item field
/// stops the model inventing a criterion, but nothing in JSON Schema says
/// "and there must be one of each" — so a two-item answer to a three-item
/// checklist is well-formed, accepted, and wrong. The universe of items is
/// recovered from the slot's own schema ref rather than an authored
/// annotation: a slot typed `[schema:<id>-verdict]` is, by construction,
/// answering checklist `<id>`.
///
/// Returns `None` when the sink is not a checklist verdict list, which is the
/// common case and must stay free.
fn checklist_coverage_validation(
    trait_ref: &Trait,
    ref_text: &str,
    schema_ref: &str,
    schema_reference: Option<Reference>,
    value: &JsonValue,
    prior_accepted: Option<&JsonValue>,
) -> Option<SchemaValidation> {
    let inner = schema_ref
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))?
        .trim();

    let reject = |reason: String| {
        Some(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: schema_reference.clone(),
            status: SchemaStatus::Rejected,
            reason,
        })
    };

    if inner == "schema:checklist-item" {
        let Some(entries) = value.as_array() else {
            return reject("checklist coverage: expected an array of items".to_string());
        };
        let mut counts: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for entry in entries {
            if let Some(id) = entry
                .get(crate::r#trait::checklist::ITEM_ID_FIELD)
                .and_then(JsonValue::as_str)
            {
                *counts.entry(id).or_default() += 1;
            }
        }
        let prior_ids: Vec<&str> = prior_accepted
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                entry
                    .get(crate::r#trait::checklist::ITEM_ID_FIELD)
                    .and_then(JsonValue::as_str)
            })
            .collect();
        let outcome = crate::r#trait::checklist::coverage_check(&prior_ids, &counts, true);
        if outcome.missing.is_empty() && outcome.duplicated.is_empty() {
            return None;
        }
        let mut reasons = Vec::new();
        if !outcome.missing.is_empty() {
            reasons.push(format!("missing {:?}", outcome.missing));
        }
        if !outcome.duplicated.is_empty() {
            reasons.push(format!("answered more than once {:?}", outcome.duplicated));
        }
        return reject(format!(
            "checklist coverage: produced checklist carries {} prior item(s); {}",
            prior_ids.iter().collect::<BTreeSet<_>>().len(),
            reasons.join("; ")
        ));
    }

    let schema_id = inner.strip_prefix("schema:")?;
    let checklist =
        crate::r#trait::checklist::checklist_for_verdict_schema(&trait_ref.resources, schema_id)?;

    let Some(entries) = value.as_array() else {
        return reject(format!(
            "checklist coverage: expected an array of verdicts for resource:{}",
            checklist.id
        ));
    };

    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for entry in entries {
        if let Some(item) = entry
            .get(crate::r#trait::checklist::VERDICT_ITEM_FIELD)
            .and_then(JsonValue::as_str)
        {
            *counts.entry(item).or_default() += 1;
        }
    }

    let declared_ids = checklist.checklist_item_ids();
    let outcome = crate::r#trait::checklist::coverage_check(&declared_ids, &counts, false);

    if outcome.missing.is_empty() && outcome.duplicated.is_empty() {
        return None;
    }

    let mut reasons = Vec::new();
    if !outcome.missing.is_empty() {
        reasons.push(format!("missing {:?}", outcome.missing));
    }
    if !outcome.duplicated.is_empty() {
        reasons.push(format!("answered more than once {:?}", outcome.duplicated));
    }
    reject(format!(
        "checklist coverage: resource:{} declares {} item(s); {}",
        checklist.id,
        declared_ids.len(),
        reasons.join("; ")
    ))
}

fn runtime_value_for_output_sink(
    trait_ref: &Trait,
    sequence_index: usize,
    sink: &OutputSink,
    output: StepSlotOutput,
    is_check: bool,
    prior_accepted: Option<&JsonValue>,
) -> crate::Result<Value> {
    let parsed = Reference::parse(&output.ref_text).map_err(|_| {
        crate::procedure::invalid_field(
            format!("procedure.sequence[{sequence_index}].output"),
            format!("invalid output ref {:?}", output.ref_text),
        )
    })?;
    if parsed.is_qualified() || !matches!(parsed.kind(), Kind::Slot | Kind::Port | Kind::Schema) {
        return Err(crate::procedure::invalid_field(
            format!("procedure.sequence[{sequence_index}].output"),
            "step output refs must be local slot:*, terminal port:*, or ephemeral schema:* refs",
        ));
    }
    let schema_ref = output_sink_schema_ref(trait_ref, sink)?;
    let validation = match (sink.operation(), schema_ref.as_deref()) {
        (WriteOperation::Merge, Some(schema_ref)) => {
            validate_merge_delta(trait_ref, &output.ref_text, schema_ref, &output.value)?
        }
        (_, Some(schema_ref)) => {
            validate_value_schema(trait_ref, &output.ref_text, schema_ref, &output.value)?
        }
        (_, None) => SchemaValidation {
            ref_text: output.ref_text.clone(),
            schema_ref: None,
            status: SchemaStatus::Accepted,
            reason: "output ref has no schema; treated as schema:any".to_string(),
        },
    };
    // Coverage judges a whole verdict list, so it applies to the write that
    // supplies one. An `append` sink carries a single element and is only
    // complete once the loop that drives it ends — a different obligation,
    // deliberately not claimed here.
    let coverage = if validation.status == SchemaStatus::Accepted
        && matches!(sink.operation(), WriteOperation::Replace)
    {
        schema_ref.as_deref().and_then(|schema_ref| {
            checklist_coverage_validation(
                trait_ref,
                &output.ref_text,
                schema_ref,
                validation.schema_ref.clone(),
                &output.value,
                prior_accepted,
            )
        })
    } else {
        None
    };
    let validation = coverage.unwrap_or(validation);
    // A replace write carries the whole next revision, so it is compared with
    // the previous accepted revision here. The other write modes only yield
    // their resulting value in `apply_runtime_write`, which runs the same
    // check on that result.
    let frozen = if validation.status == SchemaStatus::Accepted
        && matches!(sink.operation(), WriteOperation::Replace)
    {
        schema_ref.as_deref().and_then(|schema_ref| {
            frozen_fields_validation(
                trait_ref,
                &output.ref_text,
                schema_ref,
                &output.value,
                prior_accepted,
            )
        })
    } else {
        None
    };
    let validation = frozen.unwrap_or(validation);
    let acceptance = if matches!(
        sink.operation(),
        WriteOperation::Merge | WriteOperation::SetField(_) | WriteOperation::Increment
    ) {
        if validation.status == SchemaStatus::Accepted {
            AcceptanceStatus::Accepted
        } else {
            AcceptanceStatus::Rejected
        }
    } else if validation.status == SchemaStatus::Rejected {
        AcceptanceStatus::Rejected
    } else {
        AcceptanceStatus::Accepted
    };
    Ok(Value {
        ref_text: output.ref_text,
        value_digest: value_digest(&output.value)?,
        value: output.value,
        schema_ref: schema_ref
            .as_deref()
            .map(runtime_schema_reference)
            .transpose()?
            .flatten(),
        source: output.source.unwrap_or(ValueSource::ManualOutput),
        producer_evidence: output.producer_evidence,
        command_execution: output.command_execution,
        producer_agent: output.producer_agent,
        producer_harness: output.producer_harness,
        producer_check_verdict: is_check,
        acceptance,
        // Stamped by the caller (`apply_step_output`) for an accepted
        // direct output-port value only — see `Value::position_path`.
        position_path: Vec::new(),
        acceptance_order: None,
        schema_validation: vec![validation],
    })
}

/// Reject a revision that changes what an earlier revision of the same slot
/// froze.
///
/// A `[[schema]]` declaring `frozen = { key, fields }` says its items carry
/// identity across revisions of a slot (`key`) and that some of their fields
/// are written once and never edited afterwards. The check walks the
/// submitted value and the previous accepted value together along the slot's
/// declared schema: through object fields, into every list whose item schema
/// is frozen (matching items by key), and through union members. A matched
/// item whose frozen field changed or vanished is one violation; a field
/// absent before and present now is a first write and passes; items with no
/// key match are new and unchecked; nested frozen lists are matched only
/// inside their matched parent. Lists without a frozen item schema have no
/// item identity and are not compared.
///
/// Returns `None` when there is no previous revision or nothing changed, so
/// the ordinary schema validation stands.
pub(crate) fn frozen_fields_validation(
    trait_ref: &Trait,
    ref_text: &str,
    schema_ref: &str,
    value: &JsonValue,
    prior_accepted: Option<&JsonValue>,
) -> Option<SchemaValidation> {
    let prior = prior_accepted?;
    let mut violations = Vec::new();
    walk_frozen(trait_ref, schema_ref, value, prior, &[], &mut violations);
    if violations.is_empty() {
        return None;
    }
    Some(SchemaValidation {
        ref_text: ref_text.to_string(),
        schema_ref: runtime_schema_reference(schema_ref).ok().flatten(),
        status: SchemaStatus::Rejected,
        reason: violations.join("; "),
    })
}

const FROZEN_KEY_RENDER_LIMIT: usize = 60;

fn walk_frozen(
    trait_ref: &Trait,
    schema_ref: &str,
    value: &JsonValue,
    prior: &JsonValue,
    path: &[String],
    violations: &mut Vec<String>,
) {
    let Ok(form) = crate::schema::form::Schema::try_from_str(schema_ref) else {
        return;
    };
    match form {
        crate::schema::form::Schema::Builtin(_) => {}
        crate::schema::form::Schema::List(inner) => {
            walk_frozen_list(trait_ref, &inner, value, prior, path, violations);
        }
        crate::schema::form::Schema::Union(members) => {
            for member in members {
                walk_frozen(trait_ref, &member, value, prior, path, violations);
            }
        }
        crate::schema::form::Schema::Ref(plain) => {
            let Some(declared) = declared_local_schema(trait_ref, &plain) else {
                return;
            };
            let Some(fields) = declared.fields.as_ref() else {
                return;
            };
            walk_frozen_fields(trait_ref, fields, value, prior, path, violations);
        }
    }
}

fn walk_frozen_fields(
    trait_ref: &Trait,
    fields: &BTreeMap<String, crate::r#trait::schema::SchemaField>,
    value: &JsonValue,
    prior: &JsonValue,
    path: &[String],
    violations: &mut Vec<String>,
) {
    let (Some(object), Some(prior_object)) = (value.as_object(), prior.as_object()) else {
        return;
    };
    for (field_id, field) in fields {
        let (Some(field_value), Some(prior_field_value)) =
            (object.get(field_id), prior_object.get(field_id))
        else {
            continue;
        };
        let mut field_path = path.to_vec();
        field_path.push(field_id.clone());
        walk_frozen(
            trait_ref,
            &field.schema,
            field_value,
            prior_field_value,
            &field_path,
            violations,
        );
    }
}

fn walk_frozen_list(
    trait_ref: &Trait,
    item_schema_ref: &str,
    value: &JsonValue,
    prior: &JsonValue,
    path: &[String],
    violations: &mut Vec<String>,
) {
    let (Some(items), Some(prior_items)) = (value.as_array(), prior.as_array()) else {
        return;
    };
    let Some(declared) = declared_local_schema(trait_ref, item_schema_ref) else {
        return;
    };
    let (Some(frozen), Some(fields)) = (declared.frozen.as_ref(), declared.fields.as_ref()) else {
        return;
    };
    // The list's own field name is the last path segment; the item segment
    // replaces it (`steps` -> `steps[step=...]`). A slot whose value is the
    // list itself has no field name, so its items render as `[step=...]`.
    let (parent_path, list_field) = match path.split_last() {
        Some((last, rest)) => (rest, Some(last.as_str())),
        None => (path, None),
    };
    let noun = list_field.map_or_else(|| "item".to_string(), singular_noun);
    let mut prior_by_key: BTreeMap<String, &JsonValue> = BTreeMap::new();
    for item in prior_items {
        if let Some(key) = item.get(&frozen.key).map(render_frozen_key) {
            prior_by_key.entry(key).or_insert(item);
        }
    }
    for item in items {
        let Some(key) = item.get(&frozen.key).map(render_frozen_key) else {
            continue;
        };
        let Some(prior_item) = prior_by_key.get(&key) else {
            continue;
        };
        let mut item_path = parent_path.to_vec();
        item_path.push(match list_field {
            Some(field) => format!("{field}[{}={key}]", frozen.key),
            None => format!("[{}={key}]", frozen.key),
        });
        let rendered_path = item_path.join(".");
        for field_id in &frozen.fields {
            let outcome = match (prior_item.get(field_id), item.get(field_id)) {
                (Some(before), Some(after)) if before != after => Some("changed"),
                (Some(_), None) => Some("removed"),
                _ => None,
            };
            if let Some(outcome) = outcome {
                violations.push(format!(
                    "{rendered_path}.{field_id} {outcome} — frozen fields never change across revisions; add a new {noun} instead"
                ));
            }
        }
        walk_frozen_fields(trait_ref, fields, item, prior_item, &item_path, violations);
    }
}

fn declared_local_schema<'a>(
    trait_ref: &'a Trait,
    schema_ref: &str,
) -> Option<&'a crate::r#trait::schema::Schema> {
    let parsed = Reference::parse(schema_ref).ok()?;
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return None;
    }
    trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
}

/// The key as the reviewer wrote it, bounded so a long step text names the
/// item without reproducing it.
fn render_frozen_key(value: &JsonValue) -> String {
    let rendered = match value {
        JsonValue::String(text) => text.clone(),
        other => other.to_string(),
    };
    rendered.chars().take(FROZEN_KEY_RENDER_LIMIT).collect()
}

/// `steps` -> `step`, `blockers` -> `blocker`: the noun a rejection uses for
/// "add a new … instead". A field name that is not a plural stays as is.
fn singular_noun(list_field: &str) -> String {
    match list_field.strip_suffix('s') {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => list_field.to_string(),
    }
}

fn output_sink_schema_ref(trait_ref: &Trait, sink: &OutputSink) -> crate::Result<Option<String>> {
    match sink.operation() {
        WriteOperation::Replace => Ok(output_schema_ref(trait_ref, sink.ref_text())),
        WriteOperation::Append => {
            let Some(slot_schema) = output_schema_ref(trait_ref, sink.ref_text()) else {
                return Ok(None);
            };
            let Some(element_schema) = list_element_schema_ref(&slot_schema) else {
                return Err(crate::procedure::invalid_field(
                    "procedure.sequence.output.operation",
                    format!("append output requires array slot schema, got {slot_schema:?}"),
                ));
            };
            Ok(Some(element_schema))
        }
        WriteOperation::Merge | WriteOperation::Increment => {
            Ok(output_schema_ref(trait_ref, sink.ref_text()))
        }
        WriteOperation::SetField(field) => {
            let Some(slot_schema) = output_schema_ref(trait_ref, sink.ref_text()) else {
                return Ok(None);
            };
            let Some(fields) = runtime_object_schema_fields(trait_ref, &slot_schema) else {
                return Err(crate::procedure::invalid_field(
                    "procedure.sequence.output.operation",
                    format!(
                        "set-field output requires inline object slot schema, got {slot_schema:?}"
                    ),
                ));
            };
            let Some(field_schema) = fields.get(field) else {
                return Err(crate::procedure::invalid_field(
                    "procedure.sequence.output.operation",
                    format!("set-field output names unknown field {field:?}"),
                ));
            };
            Ok(Some(field_schema.schema.clone()))
        }
    }
}

fn runtime_object_schema_fields<'a>(
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

fn validate_merge_delta(
    trait_ref: &Trait,
    ref_text: &str,
    schema_ref: &str,
    value: &JsonValue,
) -> crate::Result<SchemaValidation> {
    let Some(fields) = runtime_object_schema_fields(trait_ref, schema_ref) else {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: runtime_schema_reference(schema_ref)?,
            status: SchemaStatus::Rejected,
            reason: "merge requires an inline object schema".to_string(),
        });
    };
    let Some(object) = value.as_object() else {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: runtime_schema_reference(schema_ref)?,
            status: SchemaStatus::Rejected,
            reason: "merge output must be a JSON object delta".to_string(),
        });
    };
    for (field_id, field_value) in object {
        let Some(field) = fields.get(field_id) else {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: runtime_schema_reference(schema_ref)?,
                status: SchemaStatus::Rejected,
                reason: format!("merge delta contains unknown field {field_id:?}"),
            });
        };
        let validation = if field_value.is_object()
            && runtime_object_schema_fields(trait_ref, &field.schema).is_some()
        {
            validate_merge_delta(trait_ref, ref_text, &field.schema, field_value)?
        } else {
            validate_value_schema(trait_ref, ref_text, &field.schema, field_value)?
        };
        if validation.status != SchemaStatus::Accepted {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: runtime_schema_reference(schema_ref)?,
                status: validation.status,
                reason: format!(
                    "merge field {field_id:?} failed validation: {}",
                    validation.reason
                ),
            });
        }
        if field.allowed.as_ref().is_some_and(|allowed| {
            !allowed
                .iter()
                .any(|allowed_value| allowed_value == field_value)
        }) {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: runtime_schema_reference(schema_ref)?,
                status: SchemaStatus::Rejected,
                reason: format!(
                    "merge field {field_id:?} value {field_value:?} is not one of the allowed schema values: {:?}",
                    field.allowed.as_deref().unwrap_or_default()
                ),
            });
        }
    }
    Ok(SchemaValidation {
        ref_text: ref_text.to_string(),
        schema_ref: runtime_schema_reference(schema_ref)?,
        status: SchemaStatus::Accepted,
        reason: "merge object delta accepted".to_string(),
    })
}

fn list_element_schema_ref(schema_ref: &str) -> Option<String> {
    schema_ref
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::to_string)
}

fn runtime_schema_reference(schema_ref: &str) -> crate::Result<Option<Reference>> {
    if matches!(
        crate::schema::form::Schema::try_from_str(schema_ref),
        Ok(crate::schema::form::Schema::Union(_))
    ) {
        return Ok(None);
    }
    let mut innermost = schema_ref;
    while let Some(inner) = innermost
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        innermost = inner;
    }
    Reference::parse(innermost).map(Some)
}

/// Pure value/schema acceptance check. `pub(crate)` so both the runtime
/// (accepted output values) and the manifest-time procedure validator
/// (static literal projection sources, P431) share the ONE schema checker
/// rather than each maintaining its own literal type matrix.
pub(crate) fn validate_value_schema(
    trait_ref: &Trait,
    ref_text: &str,
    schema_ref: &str,
    value: &JsonValue,
) -> crate::Result<SchemaValidation> {
    let parsed_form = crate::schema::form::Schema::try_from_str(schema_ref).map_err(|message| {
        crate::procedure::invalid_field("runtime.schema", message)
    })?;
    if let crate::schema::form::Schema::Union(members) = parsed_form {
        let mut pending = false;
        let mut unsupported = false;
        for member in members {
            let validation = validate_value_schema(trait_ref, ref_text, &member, value)?;
            match validation.status {
                SchemaStatus::Accepted => {
                    return Ok(SchemaValidation {
                        ref_text: ref_text.to_string(),
                        schema_ref: None,
                        status: SchemaStatus::Accepted,
                        reason: format!("union accepted by first matching member {member:?}"),
                    });
                }
                SchemaStatus::IoPending => pending = true,
                SchemaStatus::Unsupported => unsupported = true,
                SchemaStatus::Rejected => {}
            }
        }
        let status = if unsupported {
            SchemaStatus::Unsupported
        } else if pending {
            SchemaStatus::IoPending
        } else {
            SchemaStatus::Rejected
        };
        let reason = match status {
            SchemaStatus::Unsupported => {
                "union has no accepted member and includes unsupported validation".to_string()
            }
            SchemaStatus::IoPending => {
                "union has no accepted member and includes validation needing external evidence"
                    .to_string()
            }
            SchemaStatus::Rejected => "value matched no union member".to_string(),
            SchemaStatus::Accepted => unreachable!("accepted unions return above"),
        };
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: None,
            status,
            reason,
        });
    }

    if schema_ref.starts_with('[') && schema_ref.ends_with(']') {
        let inner = &schema_ref[1..schema_ref.len() - 1];
        if !value.is_array() {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: runtime_schema_reference(schema_ref)?,
                status: SchemaStatus::Rejected,
                reason: "expected JSON array for list schema".to_string(),
            });
        }
        let Some(items) = value.as_array() else {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: runtime_schema_reference(schema_ref)?,
                status: SchemaStatus::Rejected,
                reason: "expected JSON array for list schema".to_string(),
            });
        };
        let mut pending = false;
        for item in items {
            let validation = validate_value_schema(trait_ref, ref_text, inner, item)?;
            match validation.status {
                SchemaStatus::Rejected => {
                    return Ok(SchemaValidation {
                        ref_text: ref_text.to_string(),
                        schema_ref: runtime_schema_reference(schema_ref)?,
                        status: SchemaStatus::Rejected,
                        reason: format!(
                            "list item failed schema validation: {}",
                            validation.reason
                        ),
                    });
                }
                SchemaStatus::IoPending | SchemaStatus::Unsupported => pending = true,
                SchemaStatus::Accepted => {}
            }
        }
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: runtime_schema_reference(schema_ref)?,
            status: if pending {
                SchemaStatus::IoPending
            } else {
                SchemaStatus::Accepted
            },
            reason: if pending {
                "list items include schema validation that needs external evidence".to_string()
            } else {
                "list schema accepted".to_string()
            },
        });
    }

    match schema_ref {
        "schema:text" => {
            return primitive_schema(ref_text, schema_ref, value.is_string(), "JSON string");
        }
        "schema:boolean" => {
            return primitive_schema(ref_text, schema_ref, value.is_boolean(), "JSON boolean");
        }
        "schema:number" => {
            return primitive_schema(ref_text, schema_ref, value.is_number(), "JSON number");
        }
        "schema:integer" => {
            return primitive_schema(
                ref_text,
                schema_ref,
                value.is_i64() || value.is_u64(),
                "JSON integer",
            );
        }
        "schema:any" => {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: Some(Reference::parse(schema_ref)?),
                status: SchemaStatus::Accepted,
                reason: "schema:any accepts any JSON value".to_string(),
            });
        }
        "schema:checklist-item" => {
            return Ok(
                match crate::r#trait::checklist::validate_checklist_item_value(value) {
                    Ok(()) => SchemaValidation {
                        ref_text: ref_text.to_string(),
                        schema_ref: Some(Reference::parse(schema_ref)?),
                        status: SchemaStatus::Accepted,
                        reason: "schema:checklist-item accepted".to_string(),
                    },
                    Err(reason) => SchemaValidation {
                        ref_text: ref_text.to_string(),
                        schema_ref: Some(Reference::parse(schema_ref)?),
                        status: SchemaStatus::Rejected,
                        reason,
                    },
                },
            );
        }
        _ => {}
    }

    let parsed = Reference::parse(schema_ref).map_err(|_| {
        crate::procedure::invalid_field(
            "runtime.schema",
            format!("invalid schema ref {schema_ref:?}"),
        )
    })?;
    if parsed.kind() != Kind::Schema {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Rejected,
            reason: "schema ref must use schema:* kind".to_string(),
        });
    }
    if parsed.is_qualified() {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Unsupported,
            reason: "dependency-qualified schema validation requires loaded dependency evidence"
                .to_string(),
        });
    }
    let Some(schema) = trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
    else {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Rejected,
            reason: "local schema ref is not declared".to_string(),
        });
    };
    if schema.resource.is_some() {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::IoPending,
            reason: "resource-backed schema validation requires IO-backed schema evidence"
                .to_string(),
        });
    }
    if let Some(base_schema) = schema.schema.as_deref() {
        let validation = validate_value_schema(trait_ref, ref_text, base_schema, value)?;
        if validation.status != SchemaStatus::Accepted {
            return Ok(SchemaValidation {
                ref_text: ref_text.to_string(),
                schema_ref: Some(Reference::parse(schema_ref)?),
                status: validation.status,
                reason: format!(
                    "scalar enum base schema failed validation: {}",
                    validation.reason
                ),
            });
        }
        if let Some(allowed) = schema.allowed.as_ref()
            && !allowed.iter().any(|allowed_value| allowed_value == value) {
                return Ok(SchemaValidation {
                    ref_text: ref_text.to_string(),
                    schema_ref: Some(Reference::parse(schema_ref)?),
                    status: SchemaStatus::Rejected,
                    reason: format!(
                        "value {value:?} is not one of the allowed schema values: {allowed:?}"
                    ),
                });
            }
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Accepted,
            reason: "scalar enum schema accepted".to_string(),
        });
    }
    let Some(fields) = schema.fields.as_ref() else {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Unsupported,
            reason: "opaque schema has no inline fields".to_string(),
        });
    };
    let Some(object) = value.as_object() else {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: SchemaStatus::Rejected,
            reason: "inline-field schema expects a JSON object".to_string(),
        });
    };
    let mut pending = false;
    let mut unsupported = false;
    for (field_id, field) in fields {
        match object.get(field_id) {
            Some(field_value) => {
                let validation =
                    validate_value_schema(trait_ref, ref_text, &field.schema, field_value)?;
                match validation.status {
                    SchemaStatus::Rejected => {
                        return Ok(SchemaValidation {
                            ref_text: ref_text.to_string(),
                            schema_ref: Some(Reference::parse(schema_ref)?),
                            status: SchemaStatus::Rejected,
                            reason: format!(
                                "field {field_id:?} failed validation: {}",
                                validation.reason
                            ),
                        });
                    }
                    SchemaStatus::IoPending => pending = true,
                    SchemaStatus::Unsupported => unsupported = true,
                    SchemaStatus::Accepted => {}
                }
                if let Some(allowed) = field.allowed.as_ref()
                    && !allowed
                        .iter()
                        .any(|allowed_value| allowed_value == field_value)
                    {
                        return Ok(SchemaValidation {
                            ref_text: ref_text.to_string(),
                            schema_ref: Some(Reference::parse(schema_ref)?),
                            status: SchemaStatus::Rejected,
                            reason: format!(
                                "field {field_id:?} value {field_value:?} is not one of the allowed schema values: {allowed:?}"
                            ),
                        });
                    }
            }
            None if field.required => {
                return Ok(SchemaValidation {
                    ref_text: ref_text.to_string(),
                    schema_ref: Some(Reference::parse(schema_ref)?),
                    status: SchemaStatus::Rejected,
                    reason: format!("required field {field_id:?} is missing"),
                });
            }
            None => {}
        }
    }
    if unsupported || pending {
        return Ok(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: Some(Reference::parse(schema_ref)?),
            status: if unsupported {
                SchemaStatus::Unsupported
            } else {
                SchemaStatus::IoPending
            },
            reason: if unsupported {
                "inline-field schema contains unsupported field validation".to_string()
            } else {
                "inline-field schema contains field validation that needs external evidence"
                    .to_string()
            },
        });
    }
    Ok(SchemaValidation {
        ref_text: ref_text.to_string(),
        schema_ref: Some(Reference::parse(schema_ref)?),
        status: SchemaStatus::Accepted,
        reason: "inline-field schema accepted".to_string(),
    })
}

fn primitive_schema(
    ref_text: &str,
    schema_ref: &str,
    accepted: bool,
    expected: &str,
) -> crate::Result<SchemaValidation> {
    Ok(SchemaValidation {
        ref_text: ref_text.to_string(),
        schema_ref: Some(Reference::parse(schema_ref)?),
        status: if accepted {
            SchemaStatus::Accepted
        } else {
            SchemaStatus::Rejected
        },
        reason: if accepted {
            format!("value matches {expected}")
        } else if expected == "JSON string" {
            format!("expected {expected}")
        } else {
            format!("expected {expected}; submit a JSON value for non-text schemas")
        },
    })
}

#[cfg(test)]
mod frozen_fields_tests {
    use super::*;
    use serde_json::json;

    /// A scratch verdict shape: blockers identified by `id` (with a frozen
    /// `what`), each carrying steps identified by `step` (with a frozen
    /// `done-when` and a frozen object-valued `meta`).
    fn fixture_trait() -> Trait {
        let toml_src = r#"
id = "frozen-fixture"
schema-version = "0.3"
version = "0.1.0"
name = "Frozen fixture"
description = "Test fixture."

[[schema]]
id = "verdict"

[schema.fields.status]
schema = "schema:text"
required = true

[schema.fields.blockers]
schema = "[schema:blocker]"
required = true

[schema.fields.notes]
schema = "[schema:note]"

[[schema]]
id = "blocker"
frozen = { key = "id", fields = ["what"] }

[schema.fields.id]
schema = "schema:text"
required = true

[schema.fields.what]
schema = "schema:text"

[schema.fields.steps]
schema = "[schema:blocker-step]"
required = true

[[schema]]
id = "blocker-step"
frozen = { key = "step", fields = ["done-when", "meta"] }

[schema.fields.step]
schema = "schema:text"
required = true

[schema.fields.done-when]
schema = "schema:text"

[schema.fields.meta]
schema = "schema:any"

[schema.fields.status]
schema = "schema:text"
required = true

[[schema]]
id = "note"

[schema.fields.text]
schema = "schema:text"
required = true

[[slot]]
id = "review"
schema = "schema:verdict"

[[slot]]
id = "steps"
schema = "[schema:blocker-step]"
"#;
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, toml_src)
            .expect("fixture trait must decode: it is not itself the shape under test")
    }

    fn step(text: &str, done_when: Option<&str>, status: &str) -> JsonValue {
        let mut item = json!({"step": text, "status": status});
        if let Some(done_when) = done_when {
            item["done-when"] = json!(done_when);
        }
        item
    }

    fn verdict(blockers: Vec<JsonValue>) -> JsonValue {
        json!({"status": "revise", "blockers": blockers})
    }

    fn blocker(id: &str, what: &str, steps: Vec<JsonValue>) -> JsonValue {
        json!({"id": id, "what": what, "steps": steps})
    }

    fn check(value: &JsonValue, prior: &JsonValue) -> Option<SchemaValidation> {
        frozen_fields_validation(&fixture_trait(), "slot:review", "schema:verdict", value, Some(prior))
    }

    #[test]
    fn no_prior_revision_is_never_a_violation() {
        let value = verdict(vec![blocker("b1", "x", vec![step("s", Some("green"), "open")])]);
        let outcome = frozen_fields_validation(&fixture_trait(), "slot:review", "schema:verdict", &value, None);
        assert!(outcome.is_none());
    }

    #[test]
    fn a_changed_done_when_is_rejected_with_the_item_path() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("Rewrite the thing", Some("greps pass"), "open")])]);
        let value = verdict(vec![blocker("b1", "x", vec![step("Rewrite the thing", Some("greps and tests pass"), "open")])]);
        let outcome = check(&value, &prior).expect("changed frozen field must be rejected");
        assert_eq!(outcome.status, SchemaStatus::Rejected);
        assert_eq!(
            outcome.reason,
            "blockers[id=b1].steps[step=Rewrite the thing].done-when changed — frozen fields never change across revisions; add a new step instead"
        );
    }

    #[test]
    fn a_removed_done_when_is_rejected() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("greps pass"), "open")])]);
        let value = verdict(vec![blocker("b1", "x", vec![step("s1", None, "open")])]);
        let outcome = check(&value, &prior).expect("removed frozen field must be rejected");
        assert_eq!(
            outcome.reason,
            "blockers[id=b1].steps[step=s1].done-when removed — frozen fields never change across revisions; add a new step instead"
        );
    }

    #[test]
    fn a_first_write_of_a_frozen_field_is_allowed() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", None, "open")])]);
        let value = verdict(vec![blocker("b1", "x", vec![step("s1", Some("greps pass"), "open")])]);
        assert!(check(&value, &prior).is_none(), "set-once must pass");
    }

    #[test]
    fn unfrozen_fields_and_new_items_change_freely() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("greps pass"), "open")])]);
        let value = verdict(vec![blocker(
            "b1",
            "x",
            vec![step("s1", Some("greps pass"), "done"), step("s2", Some("tests pass"), "open")],
        )]);
        assert!(check(&value, &prior).is_none(), "status flips and appended steps pass");
    }

    #[test]
    fn a_renamed_parent_leaves_its_children_unchecked() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("greps pass"), "open")])]);
        let value = verdict(vec![blocker("b2", "y", vec![step("s1", Some("something else"), "open")])]);
        assert!(check(&value, &prior).is_none(), "no parent match means no child match");
    }

    #[test]
    fn a_dropped_item_is_not_a_frozen_violation() {
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("greps pass"), "open")])]);
        let value = verdict(vec![]);
        assert!(check(&value, &prior).is_none(), "item presence is not what frozen governs");
    }

    #[test]
    fn frozen_object_values_compare_deeply() {
        let mut before = step("s1", Some("greps pass"), "open");
        before["meta"] = json!({"owner": "reviewer", "tags": ["a", "b"]});
        let mut same = before.clone();
        same["status"] = json!("done");
        let mut changed = before.clone();
        changed["meta"] = json!({"owner": "reviewer", "tags": ["a", "c"]});

        let prior = verdict(vec![blocker("b1", "x", vec![before])]);
        assert!(check(&verdict(vec![blocker("b1", "x", vec![same])]), &prior).is_none());
        let outcome = check(&verdict(vec![blocker("b1", "x", vec![changed])]), &prior)
            .expect("a nested change inside a frozen object is a violation");
        assert!(outcome.reason.starts_with("blockers[id=b1].steps[step=s1].meta changed"), "{}", outcome.reason);
    }

    #[test]
    fn every_violation_is_named_in_one_reason() {
        let prior = verdict(vec![blocker("b1", "old what", vec![step("s1", Some("a"), "open"), step("s2", Some("b"), "open")])]);
        let value = verdict(vec![blocker("b1", "new what", vec![step("s1", Some("a2"), "open"), step("s2", Some("b2"), "open")])]);
        let outcome = check(&value, &prior).expect("three violations");
        let expected = [
            "blockers[id=b1].what changed — frozen fields never change across revisions; add a new blocker instead",
            "blockers[id=b1].steps[step=s1].done-when changed — frozen fields never change across revisions; add a new step instead",
            "blockers[id=b1].steps[step=s2].done-when changed — frozen fields never change across revisions; add a new step instead",
        ];
        assert_eq!(outcome.reason, expected.join("; "));
    }

    #[test]
    fn a_top_level_frozen_list_renders_bare_item_paths() {
        let prior = json!([step("s1", Some("a"), "open")]);
        let value = json!([step("s1", Some("b"), "open")]);
        let outcome = frozen_fields_validation(&fixture_trait(), "slot:steps", "[schema:blocker-step]", &value, Some(&prior))
            .expect("top-level frozen list is checked");
        assert_eq!(
            outcome.reason,
            "[step=s1].done-when changed — frozen fields never change across revisions; add a new item instead"
        );
    }

    #[test]
    fn a_long_key_is_bounded_in_the_path() {
        let long = "x".repeat(200);
        let prior = json!([step(&long, Some("a"), "open")]);
        let value = json!([step(&long, Some("b"), "open")]);
        let outcome = frozen_fields_validation(&fixture_trait(), "slot:steps", "[schema:blocker-step]", &value, Some(&prior))
            .expect("violation");
        let expected_key = "x".repeat(FROZEN_KEY_RENDER_LIMIT);
        assert!(outcome.reason.starts_with(&format!("[step={expected_key}].done-when changed")), "{}", outcome.reason);
    }

    #[test]
    fn lists_without_a_frozen_item_schema_are_not_compared() {
        let mut prior = verdict(vec![]);
        prior["notes"] = json!([{"text": "a"}]);
        let mut value = verdict(vec![]);
        value["notes"] = json!([{"text": "b"}]);
        assert!(check(&value, &prior).is_none(), "no identity, no comparison");
    }

    #[test]
    fn a_replace_write_with_a_changed_frozen_field_is_rejected_at_the_sink() {
        let trait_ref = fixture_trait();
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("a"), "open")])]);
        let output = StepSlotOutput {
            ref_text: "slot:review".to_string(),
            value: verdict(vec![blocker("b1", "x", vec![step("s1", Some("b"), "open")])]),
            source: None,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        };
        let sink = OutputSink::Ref("slot:review".to_string());
        let value = runtime_value_for_output_sink(&trait_ref, 0, &sink, output, false, Some(&prior))
            .expect("sink resolution must not error");
        assert_eq!(value.acceptance, AcceptanceStatus::Rejected);
        let reason = &value.schema_validation.last().expect("validation evidence").reason;
        assert!(reason.contains("steps[step=s1].done-when changed"), "{reason}");
    }

    #[test]
    fn a_replace_write_that_keeps_frozen_fields_is_accepted_at_the_sink() {
        let trait_ref = fixture_trait();
        let prior = verdict(vec![blocker("b1", "x", vec![step("s1", Some("a"), "open")])]);
        let output = StepSlotOutput {
            ref_text: "slot:review".to_string(),
            value: verdict(vec![blocker("b1", "x", vec![step("s1", Some("a"), "done"), step("s2", None, "open")])]),
            source: None,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        };
        let sink = OutputSink::Ref("slot:review".to_string());
        let value = runtime_value_for_output_sink(&trait_ref, 0, &sink, output, false, Some(&prior))
            .expect("sink resolution must not error");
        assert_eq!(value.acceptance, AcceptanceStatus::Accepted);
    }
}

#[cfg(test)]
mod produced_checklist_coverage_tests {
    use super::*;
    use serde_json::json;

    fn fixture_trait() -> Trait {
        let toml_src = r#"
id = "produced-checklist-fixture"
schema-version = "0.3"
version = "0.1.0"
name = "Produced checklist fixture"
description = "Test fixture."

[[slot]]
id = "plan"
schema = "[schema:checklist-item]"
"#;
        crate::encoding::decode_trait(crate::encoding::Encoding::Toml, toml_src)
            .expect("fixture trait must decode: it is not itself the shape under test")
    }

    fn item(id: &str, status: &str) -> JsonValue {
        json!({"id": id, "text": "do the thing", "status": status})
    }

    #[test]
    fn first_write_mints_the_universe_with_no_prior() {
        let trait_ref = fixture_trait();
        let value = json!([item("a", "todo"), item("b", "todo")]);
        let outcome = checklist_coverage_validation(
            &trait_ref,
            "slot:plan",
            "[schema:checklist-item]",
            None,
            &value,
            None,
        );
        assert!(outcome.is_none(), "first write has no prior universe to violate");
    }

    #[test]
    fn replace_dropping_a_prior_id_is_rejected() {
        let trait_ref = fixture_trait();
        let prior = json!([item("a", "todo"), item("b", "todo")]);
        let value = json!([item("a", "done")]);
        let outcome = checklist_coverage_validation(
            &trait_ref,
            "slot:plan",
            "[schema:checklist-item]",
            None,
            &value,
            Some(&prior),
        )
        .expect("dropped id must be rejected");
        assert_eq!(outcome.status, SchemaStatus::Rejected);
        assert!(outcome.reason.contains("missing"));
    }

    #[test]
    fn replace_duplicating_a_prior_id_is_rejected() {
        let trait_ref = fixture_trait();
        let prior = json!([item("a", "todo")]);
        let value = json!([item("a", "done"), item("a", "done")]);
        let outcome = checklist_coverage_validation(
            &trait_ref,
            "slot:plan",
            "[schema:checklist-item]",
            None,
            &value,
            Some(&prior),
        )
        .expect("duplicated id must be rejected");
        assert!(outcome.reason.contains("more than once"));
    }

    #[test]
    fn replace_carrying_every_prior_id_with_updated_status_is_accepted() {
        let trait_ref = fixture_trait();
        let prior = json!([item("a", "todo"), item("b", "todo")]);
        let value = json!([item("a", "done"), item("b", "done")]);
        let outcome = checklist_coverage_validation(
            &trait_ref,
            "slot:plan",
            "[schema:checklist-item]",
            None,
            &value,
            Some(&prior),
        );
        assert!(outcome.is_none(), "complete round must be accepted");
    }

    #[test]
    fn replace_adding_a_new_id_joins_the_universe() {
        let trait_ref = fixture_trait();
        let prior = json!([item("a", "todo")]);
        let value = json!([item("a", "done"), item("b", "todo")]);
        let outcome = checklist_coverage_validation(
            &trait_ref,
            "slot:plan",
            "[schema:checklist-item]",
            None,
            &value,
            Some(&prior),
        );
        assert!(outcome.is_none(), "a new id may join the universe on a replace write");
    }

    #[test]
    fn malformed_item_is_rejected_by_value_schema_before_coverage_runs() {
        let trait_ref = fixture_trait();
        let value = json!([{"id": "a", "status": "todo"}]);
        let validation =
            validate_value_schema(&trait_ref, "slot:plan", "[schema:checklist-item]", &value)
                .expect("validation must not error");
        assert_eq!(validation.status, SchemaStatus::Rejected);
    }

    #[test]
    fn well_formed_item_passes_value_schema() {
        let trait_ref = fixture_trait();
        let value = json!([item("a", "todo")]);
        let validation =
            validate_value_schema(&trait_ref, "slot:plan", "[schema:checklist-item]", &value)
                .expect("validation must not error");
        assert_eq!(validation.status, SchemaStatus::Accepted);
    }
}
