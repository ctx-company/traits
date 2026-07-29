//! Protocol schema emission: generates JSON Schema artifacts from Rust model
//! types that implement the Agent Traits protocol.
//!
//! Generated schemas describe the **canonical normalized JSON serialization
//! shape** of the trait aggregate — not the complete TOML/YAML authoring
//! schema. Taxonomy fields like `SlugList` accept scalar-or-array at authoring
//! time but always serialize as arrays; the emitted schema reflects that
//! canonical array shape. Non-taxonomy fields using `OneOrMany<T>` preserve
//! their scalar-or-array shape in both authoring and serialization.
//!
//! Generated schemas target and support the official protocol documentation in
//! `.protocol/agent-traits/`. They are non-authoritative support artifacts —
//! they help consumers validate and understand canonical JSON output — not the
//! authoritative protocol source and not runtime permission enforcement.
//!
//! Only protocol-facing trait-root types are emitted. Project-local manifest
//! types, ctx-specific runtime types (`CommandOutput`, `Envelope`, `Lockfile`,
//! etc.) are excluded to keep the portable protocol boundary explicit.

pub mod form;

use schemars::schema_for;
use serde_json::Value;
use strum::IntoEnumIterator;
use thiserror::Error as ThisError;

use crate::manifest::PackageStatus;
use crate::r#trait::procedure::WriteOperation;
use crate::r#trait::resource::{ResourceRender, ResourceRoot, ResourceTrigger};
use crate::r#trait::{AGENT_TEMPLATES, Trait, TrustVerdict};
use crate::{builtins, reference::Kind, schema::form::Builtin};

/// Protocol schema generation errors.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("invalid schema: {message}")]
    Invalid { message: String },
}

/// The `$schema` URI identifying JSON Schema draft 2020-12.
const JSON_SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Protocol identifiers selectable via `--protocol`.
#[derive(Debug, Clone, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Protocol {
    /// The portable Agent Traits trait-root schema.
    AgentTraits,
    /// SDK-facing generated type export list.
    SdkTypes,
}

impl Protocol {
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

/// Output format identifiers selectable via `--format`.
#[derive(Debug, Clone, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Format {
    /// JSON Schema (draft 2020-12).
    Json,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

/// Emit a JSON Schema for the Agent Traits trait-root model.
///
/// The schema describes the **canonical normalized JSON serialization shape**
/// of the `Trait` aggregate: taxonomy fields like `SlugList` appear as arrays
/// (their canonical output form), while `OneOrMany<T>` fields retain their
/// scalar-or-array representation. This is not the complete TOML/YAML
/// authoring schema — authoring permits scalar shorthand for taxonomy fields
/// that normalize to arrays.
///
/// Generated fields are kebab-case, matching the serialization convention.
/// The schema includes a `$schema` key identifying JSON Schema draft 2020-12.
pub fn trait_schema() -> crate::Result<Value> {
    let schema = schema_for!(Trait);
    let mut value = serde_json::to_value(&schema).map_err(|e| Error::Invalid {
        message: format!("failed to serialize generated schema: {e}"),
    })?;
    if let Value::Object(ref mut map) = value {
        map.insert(
            "$schema".to_string(),
            Value::String(JSON_SCHEMA_DRAFT.to_string()),
        );
    }
    Ok(value)
}

/// Extract the string enum values from a type's generated JSON schema.
///
/// Used to publish structural-enum vocabularies (e.g. `ResourceTrigger`,
/// `Status`, `Trust`) through `x-ctx-sdk` sourced from the Rust enum's
/// `schemars` output, rather than a copied string list that could drift.
/// A schemars enum schema is either a top-level `enum` array (no
/// doc-commented variants) or a `oneOf` array whose entries are each either a
/// single `const` (a doc-commented variant) or a shared `enum` array (the
/// undocumented variants grouped together). Both shapes are read here.
fn string_enum_values<T: schemars::JsonSchema>(name: &str) -> crate::Result<Vec<String>> {
    let schema = serde_json::to_value(schema_for!(T)).map_err(|error| Error::Invalid {
        message: format!("failed to serialize {name} schema: {error}"),
    })?;
    let mut values = Vec::new();
    if let Some(entries) = schema.get("oneOf").and_then(Value::as_array) {
        for variant in entries {
            values.extend(string_enum_variant_values(variant, name)?);
        }
    } else {
        values.extend(string_enum_variant_values(&schema, name)?);
    }
    Ok(values)
}

fn string_enum_variant_values(variant: &Value, name: &str) -> crate::Result<Vec<String>> {
    if let Some(value) = variant.get("const").and_then(Value::as_str) {
        return Ok(vec![value.to_string()]);
    }
    variant
        .get("enum")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Invalid {
            message: format!("{name} schema variant has no const or enum value"),
        })?
        .iter()
        .map(|entry| {
            entry.as_str().map(str::to_string).ok_or_else(|| {
                Error::Invalid {
                    message: format!("{name} schema enum contains a non-string value"),
                }
                .into()
            })
        })
        .collect()
}

pub fn sdk_types_schema() -> crate::Result<Value> {
    let mut ref_kinds: Vec<&str> = Kind::iter()
        .filter(|kind| kind.is_sdk_ref_kind())
        .map(|kind| kind.as_str())
        .collect();
    ref_kinds.sort_unstable();
    let schema_builtins: Vec<String> = Builtin::iter().map(Builtin::as_ref).collect();
    let resource_triggers = string_enum_values::<ResourceTrigger>("ResourceTrigger")?;
    let resource_roots = string_enum_values::<ResourceRoot>("ResourceRoot")?;
    let resource_renders = string_enum_values::<ResourceRender>("ResourceRender")?;
    let statuses = string_enum_values::<PackageStatus>("PackageStatus")?;
    let trust_levels = string_enum_values::<TrustVerdict>("TrustVerdict")?;
    let write_operations = serde_json::to_value(schema_for!(WriteOperation))
        .map_err(|error| Error::Invalid {
            message: format!("failed to serialize write-operation schema: {error}"),
        })?
        .get("oneOf")
        .cloned()
        .ok_or_else(|| Error::Invalid {
            message: "write-operation schema has no oneOf variants".to_string(),
        })?;
    let mut value = serde_json::json!({
        "$schema": JSON_SCHEMA_DRAFT,
        "title": "ctx.traits SDK generated type export list",
        "type": "object",
        "x-ctx-sdk": {
            "builtins": {
                "intent": {
                    "require": builtins::INTENT_REQUIRE,
                    "focus": builtins::INTENT_FOCUS,
                    "avoid": builtins::INTENT_AVOID,
                    "block": builtins::INTENT_BLOCK,
                },
                "tone": builtins::BEHAVIOR_TONE,
                "method": builtins::BEHAVIOR_METHOD,
                "verbosity": builtins::BEHAVIOR_VERBOSITY,
                "format": builtins::BEHAVIOR_FORMAT,
                "directness": builtins::BEHAVIOR_DIRECTNESS,
                "scope-control": builtins::BEHAVIOR_SCOPE_CONTROL,
                "initiative": builtins::BEHAVIOR_INITIATIVE,
                "uncertainty": builtins::BEHAVIOR_UNCERTAINTY,
            },
            "agent-templates": AGENT_TEMPLATES,
            "ref-kinds": ref_kinds,
            "resource-triggers": resource_triggers,
            "resource-roots": resource_roots,
            "resource-renders": resource_renders,
            "statuses": statuses,
            "trust-levels": trust_levels,
            "schema-forms": {
                "builtins": schema_builtins,
                "wrappers": [
                    {
                        "name": "list",
                        "prefix": "[",
                        "separator": "",
                        "suffix": "]",
                        "type-template": "[${string}]"
                    },
                    {
                        "name": "union",
                        "prefix": "(",
                        "separator": "|",
                        "suffix": ")",
                        "type-template": "(${string})"
                    }
                ]
            },
            "write-operations": write_operations,
            "synth-formats": ["toml", "json", "yaml"],
            "runtime-surfaces": ["wasm-core", "wasm-plugin", "cli-json"],
            "runtime-operations": [
                "validate",
                "normalize",
                "synth",
                "audit",
                "import-plan",
                "render",
                "compose",
                "resolve",
                "explain",
                "pack",
                "cache-plan",
                "ledger-update"
            ]
        }
    });
    if let Value::Object(ref mut map) = value {
        map.insert(
            "description".to_string(),
            Value::String(
                "Generated SDK mirror source; helpers stay hand-written, type mirrors are generated."
                    .to_string(),
            ),
        );
    }
    Ok(value)
}
