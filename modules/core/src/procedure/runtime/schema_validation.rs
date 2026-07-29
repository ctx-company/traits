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
) -> Option<SchemaValidation> {
    let inner = schema_ref
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))?
        .trim();
    let schema_id = inner.strip_prefix("schema:")?;
    let checklist =
        crate::r#trait::checklist::checklist_for_verdict_schema(&trait_ref.resources, schema_id)?;

    let reject = |reason: String| {
        Some(SchemaValidation {
            ref_text: ref_text.to_string(),
            schema_ref: schema_reference.clone(),
            status: SchemaStatus::Rejected,
            reason,
        })
    };

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

    let declared = checklist.checklist_item_ids();
    let missing: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|id| !counts.contains_key(id))
        .collect();
    let duplicated: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|id| counts.get(id).is_some_and(|count| *count > 1))
        .collect();

    if missing.is_empty() && duplicated.is_empty() {
        return None;
    }

    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push(format!("missing {missing:?}"));
    }
    if !duplicated.is_empty() {
        reasons.push(format!("answered more than once {duplicated:?}"));
    }
    reject(format!(
        "checklist coverage: resource:{} declares {} item(s); {}",
        checklist.id,
        declared.len(),
        reasons.join("; ")
    ))
}

fn runtime_value_for_output_sink(
    trait_ref: &Trait,
    sequence_index: usize,
    sink: &OutputSink,
    output: StepSlotOutput,
    is_check: bool,
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
            )
        })
    } else {
        None
    };
    let validation = coverage.unwrap_or(validation);
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
        schema_validation: vec![validation],
    })
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
