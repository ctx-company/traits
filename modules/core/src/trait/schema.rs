//! Schema declarations: named object schema contracts.
//!
//! A `[[schema]]` is backed by exactly one of:
//! - `resource = "resource:<id>"` — an opaque blob read by IO/render phases.
//! - Inline `fields.<id> = { schema, required, description?, hint? }` —
//!   duck-typed field declarations for pure-core port compatibility checks.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::reference::{Kind, Reference};

/// One inline field declaration for duck-typed schema compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct SchemaField {
    /// Field schema ref (e.g. `"schema:text"`, `"schema:number"`).
    pub schema: String,

    /// Whether this field is required.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Advisory hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,

    /// Optional closed set of allowed scalar values for this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<Value>>,
}

/// A `[[schema]]` declaration: a named schema contract.
///
/// Must have exactly one of `resource`, `fields`, or scalar `schema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case", rename = "SchemaDeclaration")]
pub struct Schema {
    /// Schema identifier (e.g. `"scope"`).
    pub id: String,

    /// Resource ref: `resource:<id>` pointing to an opaque blob.
    /// Mutually exclusive with `fields`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,

    /// Inline field declarations for duck-typed compatibility.
    /// Mutually exclusive with `resource`.
    #[serde(default, rename = "fields", skip_serializing_if = "Option::is_none")]
    pub fields: Option<BTreeMap<String, SchemaField>>,

    /// Scalar schema ref for closed-value enum schemas.
    /// Mutually exclusive with `resource` and `fields`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Closed set of allowed scalar values for scalar enum schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<Value>>,

    /// Human-readable description of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Validate a list of schema declarations.
pub fn validate_schemas(schemas: &[Schema], resource_ids: &BTreeSet<&str>) -> crate::Result<()> {
    let schema_ids: BTreeSet<&str> = schemas.iter().map(|s| s.id.as_str()).collect();
    validate_schemas_with_ids(schemas, resource_ids, &schema_ids)
}

/// Validate schemas with a pre-built schema ID set for cross-checking field
/// schemas against declared schemas.
pub fn validate_schemas_with_ids(
    schemas: &[Schema],
    resource_ids: &BTreeSet<&str>,
    schema_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, schema) in schemas.iter().enumerate() {
        let id_path = format!("schema[{i}].id");
        crate::shared::validate_slug_shape(&schema.id, &id_path)?;

        if !seen.insert(schema.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate schema id {:?}", schema.id),
            }
            .into());
        }

        let shape_count = usize::from(schema.resource.is_some())
            + usize::from(schema.fields.is_some())
            + usize::from(schema.schema.is_some());
        if shape_count != 1 {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("schema[{i}]"),
                message: "schema must have exactly one of resource, fields, or schema".to_string(),
            }
            .into());
        }
        if schema.allowed.is_some() && schema.schema.is_none() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("schema[{i}].allowed"),
                message: "allowed values require scalar schema".to_string(),
            }
            .into());
        }

        if let Some(ref resource_ref) = schema.resource {
            let resource_field = format!("schema[{i}].resource");
            let parsed = Reference::parse(resource_ref).map_err(|_| {
                crate::manifest::Error::InvalidField {
                    field_path: resource_field.clone(),
                    message: format!("invalid typed ref {resource_ref:?}"),
                }
            })?;
            if parsed.kind() != Kind::Resource {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: resource_field,
                    message: format!(
                        "schema resource must be kind {:?}, got {:?}",
                        Kind::Resource,
                        parsed.kind()
                    ),
                }
                .into());
            }
            if parsed.is_qualified() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: resource_field,
                    message: "schema resource must be a local unqualified ref".to_string(),
                }
                .into());
            }
            let resource_id = parsed.id();
            if !resource_ids.contains(resource_id) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: resource_field,
                    message: format!("schema resource {resource_ref:?} is not declared"),
                }
                .into());
            }
        }

        if let Some(ref fields) = schema.fields {
            for (field_id, field) in fields {
                let fp = format!("schema[{i}].fields.{field_id}");
                crate::shared::validate_slug_shape(field_id, &fp)?;
                if field.schema.trim().is_empty() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{fp}.schema"),
                        message: "must not be empty".to_string(),
                    }
                    .into());
                }
                // Validate field schema with the shared schema-form validator.
                match crate::schema::form::Schema::try_from_str(&field.schema) {
                    Ok(parsed_schema) => {
                        crate::schema::form::validate(
                            &parsed_schema,
                            &format!("{fp}.schema"),
                            schema_ids,
                        )?;
                    }
                    Err(msg) => {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("{fp}.schema"),
                            message: msg,
                        }
                        .into());
                    }
                }
                if let Some(allowed) = field.allowed.as_ref() {
                    validate_allowed_values(allowed, &field.schema, &format!("{fp}.allowed"))?;
                }
            }
        }

        if let Some(ref schema_ref) = schema.schema {
            let schema_field = format!("schema[{i}].schema");
            match crate::schema::form::Schema::try_from_str(schema_ref) {
                Ok(parsed_schema) => {
                    crate::schema::form::validate(&parsed_schema, &schema_field, schema_ids)?;
                }
                Err(msg) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: schema_field,
                        message: msg,
                    }
                    .into());
                }
            }
            // A scalar declaration with `allowed` is a closed-vocabulary
            // enum; without it, an OPEN scalar alias — a semantically named
            // schema over a builtin scalar (the SDK task type,
            // `[[schema]] id = "task"` over `schema:text`, is the canonical
            // case). Runtime validation checks the base scalar either way
            // and the allowed set only when declared.
            if let Some(allowed) = schema.allowed.as_ref() {
                validate_allowed_values(allowed, schema_ref, &format!("schema[{i}].allowed"))?;
            }
        }
    }

    reject_recursive_schemas(schemas)?;

    Ok(())
}

/// Local schema ids referenced by a `schema` field string, following list
/// (`[schema:x]`) and union (`(schema:a|schema:b)`) wrappers. Built-in refs
/// and dependency-qualified refs are not local edges and are skipped.
fn referenced_local_ids(schema_ref: &str) -> Vec<String> {
    let Ok(parsed) = crate::schema::form::Schema::try_from_str(schema_ref) else {
        return Vec::new();
    };
    match parsed {
        crate::schema::form::Schema::Builtin(_) => Vec::new(),
        crate::schema::form::Schema::List(inner) => {
            local_id_from_plain_ref(&inner).into_iter().collect()
        }
        crate::schema::form::Schema::Union(members) => members
            .iter()
            .filter_map(|member| {
                let plain = member
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                    .unwrap_or(member.as_str());
                local_id_from_plain_ref(plain)
            })
            .collect(),
        crate::schema::form::Schema::Ref(s) => local_id_from_plain_ref(&s).into_iter().collect(),
    }
}

/// Resolve a plain `schema:<id>` string to a local (unqualified,
/// non-built-in) schema id, or `None` if it names a built-in or a
/// dependency-qualified schema.
fn local_id_from_plain_ref(schema_ref: &str) -> Option<String> {
    if crate::schema::form::Builtin::from_ref(schema_ref).is_some() {
        return None;
    }
    let parsed = Reference::parse(schema_ref).ok()?;
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return None;
    }
    Some(parsed.id().to_string())
}

/// Build the id -> referenced-local-ids graph for a set of declarations and
/// reject any cycle with a named, path-bearing error. Flattening at emit
/// walks these same edges (declared `fields.*.schema` and scalar `schema`,
/// through list/union wrappers); a cycle here would not terminate there.
fn reject_recursive_schemas(schemas: &[Schema]) -> crate::Result<()> {
    let mut graph: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for schema in schemas {
        let mut edges = Vec::new();
        if let Some(fields) = &schema.fields {
            for field in fields.values() {
                edges.extend(referenced_local_ids(&field.schema));
            }
        }
        if let Some(schema_ref) = &schema.schema {
            edges.extend(referenced_local_ids(schema_ref));
        }
        graph.insert(schema.id.as_str(), edges);
    }

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let starts: Vec<&str> = graph.keys().copied().collect();
    for start in starts {
        if visited.contains(start) {
            continue;
        }
        let mut path: Vec<&str> = Vec::new();
        let mut on_path: BTreeSet<&str> = BTreeSet::new();
        if let Some(cycle) = walk_for_cycle(start, &graph, &mut path, &mut on_path, &mut visited) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "schema".to_string(),
                message: format!(
                    "schema {:?} is recursive: {}; flattening cannot terminate",
                    cycle[0],
                    cycle.join(" -> ")
                ),
            }
            .into());
        }
    }

    Ok(())
}

/// DFS with an explicit path stack. Returns the cycle path (start id
/// repeated at the end) the first time a node already on the current path
/// is revisited.
fn walk_for_cycle<'a>(
    node: &'a str,
    graph: &BTreeMap<&'a str, Vec<String>>,
    path: &mut Vec<&'a str>,
    on_path: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> Option<Vec<String>> {
    path.push(node);
    on_path.insert(node);

    if let Some(edges) = graph.get(node) {
        for next in edges {
            let Some((&next_key, _)) = graph.get_key_value(next.as_str()) else {
                // Not a local schema in this batch (e.g. dependency-composed
                // set not yet merged in); no local edge to walk.
                continue;
            };
            if on_path.contains(next_key) {
                let start = path.iter().position(|&id| id == next_key).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..].iter().map(|s| s.to_string()).collect();
                cycle.push(next_key.to_string());
                path.pop();
                on_path.remove(node);
                return Some(cycle);
            }
            if !visited.contains(next_key)
                && let Some(cycle) = walk_for_cycle(next_key, graph, path, on_path, visited)
            {
                path.pop();
                on_path.remove(node);
                return Some(cycle);
            }
        }
    }

    path.pop();
    on_path.remove(node);
    visited.insert(node);
    None
}

pub(crate) fn validate_allowed_literal(
    allowed: &[Value],
    literal: &Value,
    field_path: &str,
) -> crate::Result<()> {
    if !allowed.iter().any(|value| value == literal) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("literal {literal:?} is not one of the allowed values"),
        }
        .into());
    }
    Ok(())
}

fn validate_allowed_values(
    allowed: &[Value],
    schema_ref: &str,
    field_path: &str,
) -> crate::Result<()> {
    if allowed.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    let mut seen = BTreeSet::new();
    for (index, value) in allowed.iter().enumerate() {
        validate_allowed_value_shape(value, schema_ref, &format!("{field_path}[{index}]"))?;
        let key = serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"));
        if !seen.insert(key) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{index}]"),
                message: "duplicate allowed value".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_allowed_value_shape(
    value: &Value,
    schema_ref: &str,
    field_path: &str,
) -> crate::Result<()> {
    match schema_ref {
        "schema:text" if value.is_string() => Ok(()),
        "schema:boolean" if value.is_boolean() => Ok(()),
        "schema:number" if value.is_number() => Ok(()),
        "schema:integer" if value.is_i64() || value.is_u64() => Ok(()),
        "schema:text" | "schema:boolean" | "schema:number" | "schema:integer" => {
            Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!("allowed value does not match {schema_ref}"),
            }
            .into())
        }
        "schema:any" => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "allowed values require a scalar schema, not schema:any".to_string(),
        }
        .into()),
        ref_text if ref_text.starts_with('[') => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "allowed values over list schemas are not supported".to_string(),
        }
        .into()),
        ref_text if ref_text.starts_with('(') => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "allowed values over union schemas are not supported".to_string(),
        }
        .into()),
        _ => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message:
                "allowed values require schema:text, schema:boolean, schema:number, or schema:integer"
                    .to_string(),
        }
        .into()),
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(schema_ref: &str) -> SchemaField {
        SchemaField {
            schema: schema_ref.to_string(),
            required: false,
            description: None,
            hint: None,
            allowed: None,
        }
    }

    fn object_schema(id: &str, fields: &[(&str, &str)]) -> Schema {
        Schema {
            id: id.to_string(),
            resource: None,
            fields: Some(
                fields
                    .iter()
                    .map(|(name, schema_ref)| (name.to_string(), field(schema_ref)))
                    .collect(),
            ),
            schema: None,
            allowed: None,
            description: None,
        }
    }

    fn validate_all(schemas: &[Schema]) -> crate::Result<()> {
        let resource_ids: BTreeSet<&str> = BTreeSet::new();
        validate_schemas(schemas, &resource_ids)
    }

    fn expect_recursive_message(err: &crate::Error) -> String {
        match err {
            crate::Error::Manifest(crate::manifest::Error::InvalidField { message, .. }) => {
                message.clone()
            }
            other => panic!("expected InvalidField manifest error, got {other:?}"),
        }
    }

    #[test]
    fn self_cycle_is_rejected() {
        let schemas = vec![object_schema("a", &[("self", "schema:a")])];
        let err = validate_all(&schemas).expect_err("self cycle must be rejected");
        let message = expect_recursive_message(&err);
        assert!(message.contains("recursive"), "message: {message}");
        assert!(message.contains("a -> a"), "message: {message}");
    }

    #[test]
    fn mutual_cycle_is_rejected() {
        let schemas = vec![
            object_schema("a", &[("b-ref", "schema:b")]),
            object_schema("b", &[("a-ref", "schema:a")]),
        ];
        let err = validate_all(&schemas).expect_err("mutual cycle must be rejected");
        let message = expect_recursive_message(&err);
        assert!(message.contains("recursive"), "message: {message}");
        assert!(
            message.contains("a -> b -> a") || message.contains("b -> a -> b"),
            "message: {message}"
        );
    }

    #[test]
    fn cycle_through_list_wrapper_is_rejected() {
        let schemas = vec![
            object_schema("a", &[("items", "[schema:b]")]),
            object_schema("b", &[("a-ref", "schema:a")]),
        ];
        let err = validate_all(&schemas).expect_err("cycle through list wrapper must be rejected");
        let message = expect_recursive_message(&err);
        assert!(message.contains("recursive"), "message: {message}");
    }

    #[test]
    fn deep_acyclic_chain_flattens_without_truncation() {
        // A chain longer than the old MAX_SCHEMA_REF_DEPTH (4) must validate
        // cleanly — depth is no longer a build-time limit.
        let schemas = vec![
            object_schema("a", &[("next", "schema:b")]),
            object_schema("b", &[("next", "schema:c")]),
            object_schema("c", &[("next", "schema:d")]),
            object_schema("d", &[("next", "schema:e")]),
            object_schema("e", &[("next", "schema:f")]),
            object_schema("f", &[("leaf", "schema:text")]),
        ];
        validate_all(&schemas).expect("deep acyclic chain must validate");
    }
}
