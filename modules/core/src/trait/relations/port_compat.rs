// Trait relation port compatibility.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Outcome {
    /// Exact schema match (identical built-in or local schema ID, or all
    /// required fields match with no extras).
    Exact,
    /// Provider has all consumer-required fields plus extra fields.
    ProviderSatisfiesConsumerSubset,
    /// Either side is `schema:any` — compatible with a warning.
    AnyWildcardWarning,
    /// Resource-backed or dependency-qualified schemas that cannot be compared
    /// by pure core. Needs runtime/IO decision.
    IoPending,
    /// Missing required consumer fields or incompatible field schemas.
    Incompatible,
}

/// Structured port compatibility evidence used by binding proposals and
/// composition planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Evidence {
    pub consumer: PortEndpoint,
    pub provider: PortEndpoint,
    pub outcome: Outcome,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_required_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incompatible_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_mapping: Vec<FieldMapping>,
}

fn endpoint(trait_id: &str, port_id: &str, schema_ref: &str) -> PortEndpoint {
    let mut endpoint = PortEndpoint::new(trait_id, port_id);
    // `compare` returns malformed refs as explicit incompatible evidence. The
    // optional wire field remains absent only on that diagnostic path.
    if let Ok(schema_ref) = Reference::parse(schema_ref) {
        endpoint.schema_ref = Some(schema_ref);
    }
    endpoint
}

/// A port and its resolved schema for compatibility comparison.
pub struct ComparisonPort<'a> {
    pub trait_id: &'a str,
    pub port_id: &'a str,
    pub schema_ref: &'a str,
    pub schema: Option<&'a crate::r#trait::schema::Schema>,
}

/// Map a compatibility outcome to the binding-proposal compatibility enum.
fn outcome_to_binding(outcome: &Outcome) -> Compatibility {
    match outcome {
        Outcome::Exact => Compatibility::Exact,
        Outcome::ProviderSatisfiesConsumerSubset => Compatibility::ProviderSuperset,
        Outcome::AnyWildcardWarning => Compatibility::AnyWildcard,
        Outcome::IoPending => Compatibility::IoPending,
        Outcome::Incompatible => Compatibility::Incompatible,
    }
}

/// Construct an `Incompatible` evidence record for malformed/wrong-kind schema refs.
fn incompatible_evidence(
    consumer: &ComparisonPort<'_>,
    provider: &ComparisonPort<'_>,
    reason: String,
) -> Evidence {
    Evidence {
        consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
        provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
        outcome: Outcome::Incompatible,
        reason,
        missing_required_fields: Vec::new(),
        incompatible_fields: Vec::new(),
        field_mapping: Vec::new(),
    }
}

/// Compare a single consumer input port against a single provider output port
/// and produce structured compatibility evidence following the 7-step
/// algorithm:
/// 1. If either side is `schema:any` → `AnyWildcardWarning`.
/// 2. If either side is dependency-qualified or resource-backed → `IoPending`.
/// 3. If both are comparable built-ins and equal → `Exact`; differ → `Incompatible`.
/// 4. If both are inline field schemas, check required fields:
///    - Missing required → `Incompatible` with `missing_required_fields`.
///    - Mismatched field schema → `Incompatible` with `incompatible_fields`.
///    - All required present + extras → `ProviderSatisfiesConsumerSubset`.
///    - All required present, no extras → `Exact`.
pub fn compare(consumer: &ComparisonPort<'_>, provider: &ComparisonPort<'_>) -> Evidence {
    let consumer_inner = unwrap_list(consumer.schema_ref);
    let provider_inner = unwrap_list(provider.schema_ref);

    let consumer_union = matches!(
        crate::schema::form::Schema::try_from_str(consumer.schema_ref),
        Ok(crate::schema::form::Schema::Union(_))
    );
    let provider_union = matches!(
        crate::schema::form::Schema::try_from_str(provider.schema_ref),
        Ok(crate::schema::form::Schema::Union(_))
    );
    if consumer_union || provider_union {
        let exact = consumer.schema_ref == provider.schema_ref;
        return Evidence {
            consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
            provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
            outcome: if exact {
                Outcome::Exact
            } else {
                Outcome::Incompatible
            },
            reason: if exact {
                "identical ordered union schemas".to_string()
            } else {
                "union schemas require identical ordered members for static compatibility"
                    .to_string()
            },
            missing_required_fields: Vec::new(),
            incompatible_fields: Vec::new(),
            field_mapping: Vec::new(),
        };
    }

    // Defensive step: parse both unwrapped refs. If either fails to parse
    // or is not kind Schema, return Incompatible with a clear reason.
    // Trusted decode should prevent these inputs, but helper misuse must
    // not silently fall through to IoPending.
    match (
        Reference::parse(consumer_inner),
        Reference::parse(provider_inner),
    ) {
        (Err(_), _) => {
            return incompatible_evidence(
                consumer,
                provider,
                format!("malformed consumer schema ref {:?}", consumer.schema_ref),
            );
        }
        (_, Err(_)) => {
            return incompatible_evidence(
                consumer,
                provider,
                format!("malformed provider schema ref {:?}", provider.schema_ref),
            );
        }
        (Ok(c_parsed), Ok(p_parsed))
            if c_parsed.kind() != Kind::Schema || p_parsed.kind() != Kind::Schema =>
        {
            let side = if c_parsed.kind() != Kind::Schema {
                format!(
                    "consumer schema ref {:?} has kind {:?}, expected schema",
                    consumer.schema_ref,
                    c_parsed.kind()
                )
            } else {
                format!(
                    "provider schema ref {:?} has kind {:?}, expected schema",
                    provider.schema_ref,
                    p_parsed.kind()
                )
            };
            return incompatible_evidence(consumer, provider, side);
        }
        _ => {}
    }

    // Step 1: schema:any wildcard.
    if is_any(consumer_inner) || is_any(provider_inner) {
        return Evidence {
            consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
            provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
            outcome: Outcome::AnyWildcardWarning,
            reason: "schema:any wildcard on consumer or provider side".to_string(),
            missing_required_fields: Vec::new(),
            incompatible_fields: Vec::new(),
            field_mapping: Vec::new(),
        };
    }

    // Step 2: dependency-qualified or resource-backed → IO-pending.
    let consumer_dep = is_dependency_qualified(consumer_inner);
    let provider_dep = is_dependency_qualified(provider_inner);
    let consumer_res = consumer
        .schema
        .map(|s| s.resource.is_some())
        .unwrap_or(false);
    let provider_res = provider
        .schema
        .map(|s| s.resource.is_some())
        .unwrap_or(false);
    if consumer_dep || provider_dep || consumer_res || provider_res {
        return Evidence {
            consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
            provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
            outcome: Outcome::IoPending,
            reason:
                "resource-backed or dependency-qualified schema cannot be compared by pure core"
                    .to_string(),
            missing_required_fields: Vec::new(),
            incompatible_fields: Vec::new(),
            field_mapping: Vec::new(),
        };
    }

    // Step 3: comparable built-ins.
    let consumer_builtin = crate::schema::form::Builtin::from_ref(consumer_inner);
    let provider_builtin = crate::schema::form::Builtin::from_ref(provider_inner);
    if consumer_builtin.is_some() || provider_builtin.is_some() {
        let outcome = if consumer_inner == provider_inner {
            Outcome::Exact
        } else {
            Outcome::Incompatible
        };
        return Evidence {
            consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
            provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
            outcome,
            reason: format!("built-in schema comparison: {consumer_inner} vs {provider_inner}"),
            missing_required_fields: Vec::new(),
            incompatible_fields: Vec::new(),
            field_mapping: Vec::new(),
        };
    }

    // Steps 4-7: inline field schema comparison.
    match (consumer.schema, provider.schema) {
        (Some(cs), Some(ps)) => match (&cs.fields, &ps.fields) {
            (Some(cs_fields), Some(ps_fields)) => {
                compare_port_fields(consumer, cs_fields, provider, ps_fields)
            }
            _ => Evidence {
                consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
                provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
                outcome: Outcome::IoPending,
                reason:
                    "schema comparison requires unresolved or opaque schema details in pure core"
                        .to_string(),
                missing_required_fields: Vec::new(),
                incompatible_fields: Vec::new(),
                field_mapping: Vec::new(),
            },
        },
        _ => {
            // Local schema refs that are unresolved or not field-comparable
            // in pure core. Needs IO/runtime decision.
            Evidence {
                consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
                provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
                outcome: Outcome::IoPending,
                reason:
                    "schema comparison requires unresolved or opaque schema details in pure core"
                        .to_string(),
                missing_required_fields: Vec::new(),
                incompatible_fields: Vec::new(),
                field_mapping: Vec::new(),
            }
        }
    }
}

/// Compare inline field schemas for port compatibility.
fn compare_port_fields(
    consumer: &ComparisonPort<'_>,
    consumer_fields: &std::collections::BTreeMap<String, crate::r#trait::schema::SchemaField>,
    provider: &ComparisonPort<'_>,
    provider_fields: &std::collections::BTreeMap<String, crate::r#trait::schema::SchemaField>,
) -> Evidence {
    let mut missing = Vec::new();
    let mut incompatible = Vec::new();
    let mut field_mapping = Vec::new();

    for (field_id, c_field) in consumer_fields {
        if c_field.required {
            match provider_fields.get(field_id) {
                None => missing.push(field_id.clone()),
                Some(p_field) => {
                    if c_field.schema != p_field.schema {
                        incompatible.push(format!(
                            "{field_id}: consumer={c}, provider={p}",
                            c = c_field.schema,
                            p = p_field.schema
                        ));
                    } else {
                        field_mapping.push(FieldMapping {
                            consumer_field: field_id.clone(),
                            provider_field: field_id.clone(),
                        });
                    }
                }
            }
        } else if let Some(p_field) = provider_fields.get(field_id) {
            if c_field.schema == p_field.schema {
                field_mapping.push(FieldMapping {
                    consumer_field: field_id.clone(),
                    provider_field: field_id.clone(),
                });
            }
        }
    }

    if !missing.is_empty() || !incompatible.is_empty() {
        return Evidence {
            consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
            provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
            outcome: Outcome::Incompatible,
            reason: if !missing.is_empty() {
                format!("missing required fields: {}", missing.join(", "))
            } else {
                format!("incompatible field schemas: {}", incompatible.join("; "))
            },
            missing_required_fields: missing,
            incompatible_fields: incompatible,
            field_mapping: Vec::new(),
        };
    }

    let has_extras = provider_fields
        .keys()
        .any(|k| !consumer_fields.contains_key(k));
    let (outcome, reason) = if has_extras {
        let extras: Vec<&String> = provider_fields
            .keys()
            .filter(|k| !consumer_fields.contains_key(*k))
            .collect();
        (
            Outcome::ProviderSatisfiesConsumerSubset,
            format!(
                "provider has all required fields plus extras: {}",
                extras
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else {
        (
            Outcome::Exact,
            "all consumer required fields present, no extra provider fields".to_string(),
        )
    };

    Evidence {
        consumer: endpoint(consumer.trait_id, consumer.port_id, consumer.schema_ref),
        provider: endpoint(provider.trait_id, provider.port_id, provider.schema_ref),
        outcome,
        reason,
        missing_required_fields: Vec::new(),
        incompatible_fields: Vec::new(),
        field_mapping,
    }
}
