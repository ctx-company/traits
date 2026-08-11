//! Encoding abstraction for TOML, JSON, and YAML.
//!
//! All three encodings parse into the same normalized model with canonical
//! kebab-case fields. Raw decode is kept separate from normalization so
//! source digest and canonical digest can diverge correctly: the source
//! digest captures exact authored bytes, while the canonical digest captures
//! the normalized model.
//!
//! Field-level adapters handle scalar-or-array authoring sugar at the decode
//! boundary:
//!
//! - `TargetList` (`OneOrMany<String>`) for project-manifest targets
//!   preserves scalar-vs-array authoring shape in serialization.
//! - `SlugList` for taxonomy fields always serializes as an array
//!   regardless of scalar input; taxonomy slug validation happens in the
//!   post-decode `validate_taxonomy()` pass.
//!
//! Deserialization errors carry line/column path info from the
//! format-specific decoders, wrapped into structured diagnostics without
//! duplicating the manifest model into raw DTOs.

use camino::Utf8Path;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::manifest::ProjectManifest;
use crate::r#trait::Trait;

/// Encoding selection and preflight shape errors.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("unsupported encoding: {encoding}")]
    Unsupported { encoding: String },

    #[error("invalid manifest at {field_path}: {message}")]
    InvalidField { field_path: String, message: String },

    #[error("shape mismatch at {field_path}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field_path: String,
        expected: String,
        actual: String,
    },
}

impl Error {
    pub(crate) fn invalid_field(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field_path: field_path.into(),
            message: message.into(),
        }
    }
}

fn invalid_field(field_path: impl Into<String>, message: impl Into<String>) -> crate::Error {
    Error::invalid_field(field_path, message).into()
}

/// Supported file encodings.
///
/// YAML and YML map to the same `Yaml` variant — both extensions are
/// accepted but produce the same encoding behavior.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Encoding {
    Toml,
    Json,
    Yaml,
}

impl Encoding {
    pub fn extension(self) -> &'static str {
        self.into()
    }

    pub fn from_path(path: &Utf8Path) -> crate::Result<Self> {
        let ext = path.extension().ok_or_else(|| Error::Unsupported {
            encoding: path.as_str().to_string(),
        })?;

        Self::from_extension(ext)
    }

    pub fn from_extension(ext: &str) -> crate::Result<Self> {
        if ext == "yml" {
            return Ok(Self::Yaml);
        }

        ext.parse().map_err(|_| {
            Error::Unsupported {
                encoding: ext.to_string(),
            }
            .into()
        })
    }
}

/// Decode a value from raw text in the given encoding.
///
/// `TargetList` (`OneOrMany<String>`) and `SlugList` field adapters handle
/// scalar-or-array input at the field boundary. Format-specific errors carry
/// line/column info and are wrapped into structured diagnostics. Taxonomy
/// slug validation happens in the post-decode `validate_taxonomy()` pass.
pub fn decode<T>(encoding: Encoding, text: &str) -> crate::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    match encoding {
        Encoding::Toml => {
            toml::from_str(text).map_err(|e| crate::parse::Error::toml_decode("root", e).into())
        }
        Encoding::Json => serde_json::from_str(text)
            .map_err(|e| crate::parse::Error::json_decode("root", e).into()),
        Encoding::Yaml => serde_yaml::from_str(text)
            .map_err(|e| crate::parse::Error::yaml_decode("root", e).into()),
    }
}

/// Decode a project manifest from raw text.
///
/// Decodes directly into `ProjectManifest` — field-level `TargetList` adapters
/// accept scalar-or-array input and normalize to vectors. After decoding,
/// validates dependency declarations (alias syntax, required fields, duplicate
/// aliases) so callers never receive a trusted manifest with invalid
/// dependencies. Format-specific decode errors are preserved separately from
/// semantic dependency validation errors.
pub fn decode_manifest(encoding: Encoding, text: &str) -> crate::Result<ProjectManifest> {
    let manifest: ProjectManifest = decode(encoding, text)?;
    crate::manifest::validate_dependencies(&manifest.dependencies)?;
    Ok(manifest)
}

/// Decode a trait from raw text, with full validation.
///
/// Decodes via an intermediate `serde_json::Value` so narrow preflight checks
/// can run before the trusted `Trait` is constructed. Validation order:
/// 1. Preflight field-shape checks
/// 2. Deserialize into `Trait`
/// 3. Taxonomy
/// 4. Resources
/// 5. Schemas (cross-checked against declared resources)
/// 6. Slots (schema refs cross-checked against declared schemas)
/// 7. Ports (schema refs and value refs cross-checked)
/// 8. Declaration, agents, sessions, signals, prompts, conditions, procedure, composition, relations, extensions
/// 9. Scenarios and evals
pub fn decode_trait(encoding: Encoding, text: &str) -> crate::Result<Trait> {
    decode_trait_with_warnings(encoding, text).map(|(t, _warnings)| t)
}

/// Decode a trait from raw text, also returning transition-period
/// deprecation warnings.
///
/// A canonical document that still carries top-level `status` or `trust`
/// keys (removed per Group 95, 2026-07-19: status now lives in the package
/// manifest's `[package].status`, trust in the machine trust store) decodes
/// successfully — the deprecated keys are stripped before validation — and a
/// deprecation warning is returned for each one found, rather than a hard
/// `deny_unknown_fields` failure. This tolerance is for one transition
/// period only.
pub fn decode_trait_with_warnings(
    encoding: Encoding,
    text: &str,
) -> crate::Result<(Trait, Vec<String>)> {
    use std::collections::BTreeSet;

    let mut value: serde_json::Value = decode(encoding, text)?;
    validate_preflight(&value)?;
    let mut warnings = strip_deprecated_status_trust(&mut value);
    warnings.extend(default_description_from_summary(&mut value));
    let t: Trait =
        serde_json::from_value(value).map_err(|e| crate::parse::Error::json_decode("root", e))?;

    let schema_ids: BTreeSet<&str> = t.schemas.iter().map(|s| s.id.as_str()).collect();

    t.validate_taxonomy()?;

    crate::manifest::validate_dependencies(&t.dependencies)?;

    crate::r#trait::resource::validate_resources(&t.resources)?;

    let resource_ids: BTreeSet<&str> = t.resources.iter().map(|r| r.id.as_str()).collect();
    crate::r#trait::schema::validate_schemas_with_ids(&t.schemas, &resource_ids, &schema_ids)?;

    // Runs after both declarations are individually valid: this pass only
    // checks that a checklist and its companion verdict schema still agree.
    crate::r#trait::checklist::validate_checklist_verdict_schemas(&t.resources, &t.schemas)?;

    crate::r#trait::slot::validate_slots(&t.slots, &schema_ids)?;
    crate::r#trait::port::validate_ports(&t.ports)?;
    crate::r#trait::port::validate_port_cross_refs(
        &t.ports,
        &t.slots.iter().map(|s| s.id.as_str()).collect(),
        &schema_ids,
        &resource_ids,
    )?;
    crate::r#trait::resource::validate_resource_template_refs(
        &t.resources,
        &t.ports.iter().map(|p| p.id.as_str()).collect(),
        &t.slots.iter().map(|s| s.id.as_str()).collect(),
    )?;
    if let Some(ref activation) = t.activation {
        crate::r#trait::activation::validate(activation)?;
    }
    crate::r#trait::agent::validate_agents(&t.agents)?;
    crate::r#trait::session::validate_sessions(&t.sessions)?;
    let session_ids: BTreeSet<&str> = t.sessions.iter().map(|s| s.id.as_str()).collect();
    crate::r#trait::agent::validate_agent_session_bindings(&t.agents, &session_ids)?;
    crate::r#trait::signal::validate_signals(&t.signals)?;
    crate::r#trait::prompt::validate_prompts(&t.prompts)?;
    crate::r#trait::prompt::validate_prompt_resource_sources(&t.prompts, &resource_ids)?;
    crate::r#trait::condition::validate_conditions(&t)?;

    crate::r#trait::procedure::validate(&t)?;

    if let Some(ref composition) = t.composition {
        crate::r#trait::composition::validate(composition)?;
    }

    crate::r#trait::relations::validate_optional(t.relations.as_ref())?;

    crate::r#trait::scenario::validate_scenarios(&t.scenarios)?;
    crate::r#trait::eval::validate_evals(&t.evals)?;

    let slot_ids: BTreeSet<&str> = t.slots.iter().map(|s| s.id.as_str()).collect();
    crate::r#trait::sink::validate_sinks(&t.sinks, &slot_ids)?;

    Ok((t, warnings))
}

/// Remove deprecated top-level `status`/`trust` keys from a decoded document
/// value, returning one deprecation warning per key found. Transition-period
/// tolerance only: the canonical `Trait` schema no longer has these fields
/// (Group 95, 2026-07-19), and `deny_unknown_fields` would otherwise hard-fail
/// on documents still carrying them.
fn strip_deprecated_status_trust(value: &mut serde_json::Value) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(object) = value.as_object_mut() else {
        return warnings;
    };
    for field in ["status", "trust"] {
        if object.remove(field).is_some() {
            warnings.push(format!(
                "deprecated field {field:?} on canonical trait document is ignored; {} \
                 (removed per Group 95, 2026-07-19)",
                if field == "status" {
                    "status now lives in the package manifest's [package].status"
                } else {
                    "trust is now machine-local in the trust store, keyed by canonical digest"
                }
            ));
        }
    }
    warnings
}

/// 0165 transition tolerance: a canonical document from schema-version 0.2
/// or 0.3 declares only `summary`, but `Trait::description` is now the
/// required field. Rather than have every pre-0.4 document refuse to decode
/// at all (which would also break `migrate::plan_migration`'s own
/// decode-before-rewrite step), copy `summary` into `description` when
/// `description` is absent — the same value the trait author already
/// approved for model-visible prose under the old schema. `summary` itself
/// is left in place so it still resolves as the (now-redundant) user-only
/// override until the document is migrated to 0.4 and rewritten.
fn default_description_from_summary(value: &mut serde_json::Value) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(object) = value.as_object_mut() else {
        return warnings;
    };
    if object.contains_key("description") {
        return warnings;
    }
    let is_pre_0_4 = matches!(
        object
            .get("schema-version")
            .and_then(serde_json::Value::as_str),
        Some("0.2") | Some("0.3")
    );
    if !is_pre_0_4 {
        return warnings;
    }
    if let Some(summary) = object.get("summary").cloned() {
        object.insert("description".to_string(), summary);
        warnings.push(
            "canonical trait document has no `description`; defaulting it from `summary` \
             (pre-0.4 schema; migrate to 0.4 to author `description` directly)"
                .to_string(),
        );
    }
    warnings
}

/// Encode a value to text in the given encoding.
///
/// `SlugList` taxonomy fields always serialize as arrays. Rich guidance lists
/// such as `intent.require` accept compact string authoring but serialize as
/// normalized guidance items so generated TOML can carry optional local text.
/// `TargetList` (`OneOrMany<String>`) preserves scalar-vs-array authoring shape.
pub fn encode<T>(encoding: Encoding, value: &T) -> crate::Result<String>
where
    T: serde::Serialize,
{
    match encoding {
        Encoding::Toml => toml::to_string_pretty(value)
            .map_err(|e| crate::parse::Error::toml_encode("root", e).into()),
        Encoding::Json => serde_json::to_string_pretty(value)
            .map_err(|e| crate::parse::Error::json_encode("root", e).into()),
        Encoding::Yaml => serde_yaml::to_string(value)
            .map_err(|e| crate::parse::Error::yaml_encode("root", e).into()),
    }
}

// ---------------------------------------------------------------------------
// Pre-deserialization field validation
// ---------------------------------------------------------------------------

/// Pre-deserialization validation for narrow nested diagnostics that would
/// otherwise collapse to a generic `root` serde error.
fn validate_preflight(value: &serde_json::Value) -> crate::Result<()> {
    // Port schema fields must be strings.
    if let Some(ports) = value.get("port").and_then(|v| v.as_array()) {
        for (i, port) in ports.iter().enumerate() {
            if let Some(schema) = port.get("schema")
                && !schema.is_string()
            {
                return Err(invalid_field(
                    format!("port[{i}].schema"),
                    format!("schema must be a string, got {}", json_type_name(schema)),
                ));
            }
        }
    }

    // Slot schema fields must be strings.
    if let Some(slots) = value.get("slot").and_then(|v| v.as_array()) {
        for (i, slot) in slots.iter().enumerate() {
            if let Some(schema) = slot.get("schema")
                && !schema.is_string()
            {
                return Err(invalid_field(
                    format!("slot[{i}].schema"),
                    format!("schema must be a string, got {}", json_type_name(schema)),
                ));
            }
        }
    }

    // Schema field schemas must be strings.
    if let Some(schemas) = value.get("schema").and_then(|v| v.as_array()) {
        for (i, schema) in schemas.iter().enumerate() {
            if let Some(fields) = schema.get("fields").and_then(|f| f.as_object()) {
                for (field_id, field_val) in fields {
                    if let Some(field_schema) = field_val.get("schema")
                        && !field_schema.is_string()
                    {
                        return Err(invalid_field(
                            format!("schema[{i}].fields.{field_id}.schema"),
                            format!(
                                "schema must be a string, got {}",
                                json_type_name(field_schema)
                            ),
                        ));
                    }
                }
            }
        }
    }

    // Resource fields must have correct types for narrow diagnostics.
    if let Some(resources) = value.get("resource").and_then(|v| v.as_array()) {
        for (i, resource) in resources.iter().enumerate() {
            if let Some(path) = resource.get("path")
                && !path.is_string()
            {
                return Err(invalid_field(
                    format!("resource[{i}].path"),
                    format!("path must be a string, got {}", json_type_name(path)),
                ));
            }
            if let Some(hint) = resource.get("hint")
                && !hint.is_string()
            {
                return Err(invalid_field(
                    format!("resource[{i}].hint"),
                    format!("hint must be a string, got {}", json_type_name(hint)),
                ));
            }
            if let Some(trigger) = resource.get("trigger")
                && !trigger.is_string()
            {
                return Err(invalid_field(
                    format!("resource[{i}].trigger"),
                    format!("trigger must be a string, got {}", json_type_name(trigger)),
                ));
            }
        }
    }

    if let Some(behavior) = value.get("behavior").and_then(|v| v.as_object()) {
        for field in [
            "verbosity",
            "directness",
            "scope-control",
            "initiative",
            "uncertainty",
        ] {
            if behavior.get(field).is_some_and(|value| value.is_array()) {
                return Err(invalid_field(
                    format!("behavior.{field}"),
                    "must be a string or object scalar, not an array",
                ));
            }
        }
    }

    if let Some(activation_value) = value.get("activation") {
        let Some(activation) = activation_value.as_object() else {
            return Err(invalid_field(
                "activation",
                format!(
                    "activation must be an object, got {}",
                    json_type_name(activation_value)
                ),
            ));
        };

        validate_i32_preflight(
            activation.get("priority"),
            "activation.priority",
            "priority",
        )?;
        validate_i32_preflight(
            activation.get("min-score"),
            "activation.min-score",
            "min-score",
        )?;

        // Declaration rule IDs are required for duplicate detection and future
        // explanations, so report shape errors before serde can collapse nested
        // array-table errors to `root`.
        if let Some(rules_value) = activation.get("rule") {
            let Some(rules) = rules_value.as_array() else {
                return Err(invalid_field(
                    "activation.rule",
                    format!(
                        "activation rule must be an array, got {}",
                        json_type_name(rules_value)
                    ),
                ));
            };

            for (i, rule) in rules.iter().enumerate() {
                let base = format!("activation.rule[{i}]");
                let Some(rule_obj) = rule.as_object() else {
                    return Err(invalid_field(
                        base,
                        format!(
                            "activation rule must be an object, got {}",
                            json_type_name(rule)
                        ),
                    ));
                };

                match rule_obj.get("id") {
                    None => {
                        return Err(invalid_field(format!("{base}.id"), "id is required"));
                    }
                    Some(id) if !id.is_string() => {
                        return Err(invalid_field(
                            format!("{base}.id"),
                            format!("id must be a string, got {}", json_type_name(id)),
                        ));
                    }
                    _ => {}
                }

                if let Some(reason) = rule_obj.get("reason")
                    && !reason.is_string()
                {
                    return Err(invalid_field(
                        format!("{base}.reason"),
                        format!("reason must be a string, got {}", json_type_name(reason)),
                    ));
                }

                for field in [
                    "mode",
                    "language",
                    "file-glob",
                    "task-keyword",
                    "signal",
                    "explicit-phrase",
                    "exclude-file-glob",
                    "exclude-keyword",
                ] {
                    validate_string_list_preflight(
                        rule_obj.get(field),
                        &format!("{base}.{field}"),
                        field,
                    )?;
                }

                validate_i32_preflight(
                    rule_obj.get("weight"),
                    &format!("{base}.weight"),
                    "weight",
                )?;

                if let Some(load_level) = rule_obj.get("load-level")
                    && !load_level.is_string()
                {
                    return Err(invalid_field(
                        format!("{base}.load-level"),
                        format!(
                            "load-level must be a string, got {}",
                            json_type_name(load_level)
                        ),
                    ));
                }

                if rule_obj.contains_key("exclusions") {
                    return Err(invalid_field(
                        format!("{base}.exclusions"),
                        "use flat exclude-file-glob and exclude-keyword fields",
                    ));
                }
            }
        }
    }

    validate_sequence_guard_preflight(value)?;

    // Scenario preflight: required fields, variant validation, plural rejection.
    if let Some(scenarios_value) = value.get("scenario") {
        let Some(scenarios) = scenarios_value.as_array() else {
            return Err(invalid_field(
                "scenario",
                format!(
                    "scenario must be an array, got {}",
                    json_type_name(scenarios_value)
                ),
            ));
        };

        for (i, scenario) in scenarios.iter().enumerate() {
            let base = format!("scenario[{i}]");
            let Some(obj) = scenario.as_object() else {
                return Err(invalid_field(
                    base.clone(),
                    format!(
                        "scenario must be an object, got {}",
                        json_type_name(scenario)
                    ),
                ));
            };

            validate_required_string(obj, &base, "id")?;
            validate_required_string(obj, &base, "variant")?;

            if let Some(v) = obj.get("variant").and_then(|v| v.as_str())
                && crate::r#trait::ScenarioVariant::parse(v).is_none()
            {
                return Err(invalid_field(
                    format!("{base}.variant"),
                    format!(
                        "unknown scenario variant {v:?} (expected positive, negative, or edge)"
                    ),
                ));
            }

            validate_optional_string(obj, &base, "input")?;
            validate_optional_string(obj, &base, "output")?;
            validate_string_list_preflight(obj.get("tag"), &format!("{base}.tag"), "tag")?;

            if obj.contains_key("tags") {
                return Err(invalid_field(
                    format!("{base}.tags"),
                    "use singular tag for repeatable scenario tags",
                ));
            }
        }
    }

    // Eval preflight: required fields, variant validation, plural rejection.
    if let Some(evals_value) = value.get("eval") {
        let Some(evals) = evals_value.as_array() else {
            return Err(invalid_field(
                "eval",
                format!("eval must be an array, got {}", json_type_name(evals_value)),
            ));
        };

        for (i, eval) in evals.iter().enumerate() {
            let base = format!("eval[{i}]");
            let Some(obj) = eval.as_object() else {
                return Err(invalid_field(
                    base.clone(),
                    format!("eval must be an object, got {}", json_type_name(eval)),
                ));
            };

            validate_required_string(obj, &base, "id")?;
            validate_required_string(obj, &base, "variant")?;

            if let Some(v) = obj.get("variant").and_then(|v| v.as_str())
                && crate::r#trait::EvalVariant::parse(v).is_none()
            {
                return Err(invalid_field(
                    format!("{base}.variant"),
                    format!(
                        "unknown eval variant {v:?} (expected documentation, lint, golden-render, behavioral, or runtime)"
                    ),
                ));
            }

            validate_optional_string(obj, &base, "input")?;
            validate_optional_string(obj, &base, "output")?;
            validate_string_list_preflight(
                obj.get("scenario"),
                &format!("{base}.scenario"),
                "scenario",
            )?;

            if obj.contains_key("scenarios") {
                return Err(invalid_field(
                    format!("{base}.scenarios"),
                    "use singular scenario for repeatable eval scenario refs",
                ));
            }
        }
    }

    Ok(())
}

fn validate_sequence_guard_preflight(value: &serde_json::Value) -> crate::Result<()> {
    if let Some(procedure) = value.get("procedure").and_then(|v| v.as_object()) {
        validate_sequence_items_guard_preflight(procedure.get("sequence"), "procedure.sequence")?;
    }
    if let Some(sequences) = value.get("sequence").and_then(|v| v.as_object()) {
        for (sequence_id, sequence) in sequences {
            if let Some(sequence_obj) = sequence.as_object() {
                validate_sequence_items_guard_preflight(
                    sequence_obj.get("sequence"),
                    &format!("sequence.{sequence_id}.sequence"),
                )?;
            }
        }
    }
    Ok(())
}

fn validate_sequence_items_guard_preflight(
    value: Option<&serde_json::Value>,
    field_path: &str,
) -> crate::Result<()> {
    let Some(items) = value.and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        for field in ["until", "abort-if"] {
            validate_guard_shape_preflight(
                item_obj.get(field),
                &format!("{field_path}[{index}].{field}"),
            )?;
        }
    }
    Ok(())
}

fn validate_guard_shape_preflight(
    value: Option<&serde_json::Value>,
    field_path: &str,
) -> crate::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_string() || value.is_object() {
        return Ok(());
    }
    if let Some(items) = value.as_array() {
        for (index, item) in items.iter().enumerate() {
            validate_guard_shape_preflight(Some(item), &format!("{field_path}[{index}]"))?;
        }
        return Ok(());
    }
    Err(invalid_field(
        field_path,
        format!(
            "guard must be a string ref, predicate object, or array of guards, got {}",
            json_type_name(value)
        ),
    ))
}

/// Validate that a required string field exists and is a string.
fn validate_required_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    base: &str,
    field: &str,
) -> crate::Result<()> {
    match obj.get(field) {
        None => Err(invalid_field(
            format!("{base}.{field}"),
            format!("{field} is required"),
        )),
        Some(v) if !v.is_string() => Err(invalid_field(
            format!("{base}.{field}"),
            format!("{field} must be a string, got {}", json_type_name(v)),
        )),
        _ => Ok(()),
    }
}

/// Validate that an optional field, when present, is a string.
fn validate_optional_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    base: &str,
    field: &str,
) -> crate::Result<()> {
    match obj.get(field) {
        None | Some(serde_json::Value::String(_)) => Ok(()),
        Some(v) => Err(invalid_field(
            format!("{base}.{field}"),
            format!("{field} must be a string, got {}", json_type_name(v)),
        )),
    }
}

fn validate_string_list_preflight(
    value: Option<&serde_json::Value>,
    field_path: &str,
    field: &str,
) -> crate::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };

    if value.is_string() {
        return Ok(());
    }

    if let Some(items) = value.as_array() {
        for (i, item) in items.iter().enumerate() {
            if !item.is_string() {
                return Err(invalid_field(
                    format!("{field_path}[{i}]"),
                    format!(
                        "{field} item must be a string, got {}",
                        json_type_name(item)
                    ),
                ));
            }
        }
        return Ok(());
    }

    Err(invalid_field(
        field_path,
        format!(
            "{field} must be a string or array of strings, got {}",
            json_type_name(value)
        ),
    ))
}

fn validate_i32_preflight(
    value: Option<&serde_json::Value>,
    field_path: &str,
    field: &str,
) -> crate::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };

    let valid_i32 = value.as_i64().is_some_and(|n| i32::try_from(n).is_ok())
        || value.as_u64().is_some_and(|n| i32::try_from(n).is_ok());
    if valid_i32 {
        return Ok(());
    }

    Err(invalid_field(
        field_path,
        format!(
            "{field} must be an integer number, got {}",
            json_type_name(value)
        ),
    ))
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
