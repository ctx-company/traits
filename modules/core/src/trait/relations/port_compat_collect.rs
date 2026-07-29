// Trait port compatibility evidence collection.
// Trait relation port compatibility collection.

pub fn collect_compatibility(consumer: &Trait, provider: &Trait) -> Vec<Evidence> {
    let mut results = Vec::new();

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
        for p_port in &provider.ports {
            if !matches!(p_port.direction, crate::r#trait::PortDirection::Output) {
                continue;
            }
            results.push(compare(
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
            ));
        }
    }

    results
}

// --- Helpers ---

fn unwrap_list(ref_text: &str) -> &str {
    ref_text
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(ref_text)
}

fn is_any(inner: &str) -> bool {
    inner == "schema:any"
}

fn is_dependency_qualified(inner: &str) -> bool {
    inner.contains('/')
}

fn resolve_schema_ref<'a>(
    ref_text: &str,
    schemas: &'a std::collections::BTreeMap<&str, &crate::r#trait::schema::Schema>,
) -> Option<&'a crate::r#trait::schema::Schema> {
    let inner = unwrap_list(ref_text);
    if let Ok(parsed) = Reference::parse(inner) {
        if !parsed.is_qualified() {
            return schemas.get(parsed.id()).copied();
        }
    }
    None
}
