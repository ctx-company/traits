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
            let Some(allowed) = schema.allowed.as_ref() else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("schema[{i}].allowed"),
                    message: "scalar schema declarations must declare allowed values".to_string(),
                }
                .into());
            };
            validate_allowed_values(allowed, schema_ref, &format!("schema[{i}].allowed"))?;
        }
    }

    Ok(())
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
