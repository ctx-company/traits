// Trait binding proposal generation.
// Trait relation binding proposals.

pub fn produce_proposals(consumer: &Trait, provider: &Trait) -> Vec<Proposal> {
    produce_binding_proposals_filtered(consumer, provider, None)
}

/// Compare provider output ports against one specific consumer input port
/// and produce deterministic binding proposals for compatible pairs.
///
/// `target_consumer_port_id` restricts proposals to exactly that consumer port.
pub fn produce_proposals_for_port(
    consumer: &Trait,
    provider: &Trait,
    target_consumer_port_id: &str,
) -> Vec<Proposal> {
    produce_binding_proposals_filtered(consumer, provider, Some(target_consumer_port_id))
}

fn produce_binding_proposals_filtered(
    consumer: &Trait,
    provider: &Trait,
    target_port_filter: Option<&str>,
) -> Vec<Proposal> {
    let mut proposals = Vec::new();

    let consumer_schemas: std::collections::BTreeMap<&str, &crate::r#trait::schema::Schema> =
        consumer
            .schemas
            .iter()
            .map(|s| (s.id.as_str(), s))
            .collect();
    let provider_schemas: std::collections::BTreeMap<&str, &crate::r#trait::schema::Schema> =
        provider
            .schemas
            .iter()
            .map(|s| (s.id.as_str(), s))
            .collect();

    for c_port in &consumer.ports {
        if !matches!(c_port.direction, crate::r#trait::PortDirection::Input) {
            continue;
        }
        if let Some(filter) = target_port_filter
            && c_port.id != filter {
                continue;
            }
        for p_port in &provider.ports {
            if !matches!(p_port.direction, crate::r#trait::PortDirection::Output) {
                continue;
            }
            let evidence = compare(
                &ComparisonPort {
                    trait_id: consumer.id.as_str(),
                    port_id: &c_port.id,
                    schema_ref: &c_port.schema,
                    schema: resolve_schema_ref(&c_port.schema, &consumer_schemas),
                },
                &ComparisonPort {
                    trait_id: provider.id.as_str(),
                    port_id: &p_port.id,
                    schema_ref: &p_port.schema,
                    schema: resolve_schema_ref(&p_port.schema, &provider_schemas),
                },
            );
            match evidence.outcome {
                Outcome::Exact
                | Outcome::ProviderSatisfiesConsumerSubset
                | Outcome::AnyWildcardWarning => {
                    let compat = outcome_to_binding(&evidence.outcome);
                    let consumer_ref = Reference::local(Kind::Port, &evidence.consumer.port_id);
                    let provider_ref = Reference::local(Kind::Port, &evidence.provider.port_id);
                    let (consumer_ref, provider_ref) = match (consumer_ref, provider_ref) {
                        (Ok(consumer_ref), Ok(provider_ref)) => (consumer_ref, provider_ref),
                        (consumer_error, provider_error) => {
                            let reason = match (consumer_error.err(), provider_error.err()) {
                                (Some(error), Some(provider_error)) => format!(
                                    "invalid consumer and provider port references: {error}; {provider_error}"
                                ),
                                (Some(error), None) => {
                                    format!("invalid consumer port reference: {error}")
                                }
                                (None, Some(error)) => {
                                    format!("invalid provider port reference: {error}")
                                }
                                (None, None) => unreachable!("at least one reference conversion failed"),
                            };
                            proposals.push(Proposal {
                                consumer: evidence.consumer.clone(),
                                provider: evidence.provider.clone(),
                                compatibility: Compatibility::Incompatible,
                                schema_evidence: Some(reason.clone()),
                                field_mapping: Vec::new(),
                                status: Status::Proposed,
                                accepter: None,
                                reason,
                                stale_reasons: Vec::new(),
                            });
                            continue;
                        }
                    };
                    let mut consumer = evidence.consumer.clone();
                    consumer.port_ref = Some(consumer_ref);
                    let mut provider = evidence.provider.clone();
                    provider.port_ref = Some(provider_ref);
                    proposals.push(Proposal {
                        consumer,
                        provider,
                        compatibility: compat,
                        schema_evidence: Some(evidence.reason.clone()),
                        field_mapping: evidence.field_mapping.clone(),
                        status: Status::Proposed,
                        accepter: None,
                        reason: format!(
                            "compatible port pair: consumer port:{}, provider port:{}",
                            evidence.consumer.port_id, evidence.provider.port_id
                        ),
                        stale_reasons: Vec::new(),
                    });
                }
                _ => {}
            }
        }
    }

    proposals
}
