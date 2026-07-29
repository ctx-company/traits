//! Canonical document model emitter: JSON Schema (from
//! `ctx_traits_core::schema::trait_schema()`) rendered into
//! `packages/cdk/src/generated.ts` TypeScript types.
//!
//! Every emitted type is a `type` alias, never an `interface` — object-literal
//! type aliases carry an implicit index signature, which is what keeps
//! `CanonicalTraitDraft` (and every other emitted object type) structurally
//! assignable to `JsonObject`. An `interface` here would break that.
//!
//! An unmapped JSON Schema keyword or a `$ref` to a missing def fails closed
//! (`invalid_model`) rather than silently degrading to `unknown` — a future
//! Rust schema change that this emitter cannot represent must break the
//! build, not quietly narrow the SDK's types.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::invalid_model;

/// Defs whose emitted TS type reuses an existing hand-written vocabulary
/// type from `render()` (SDK vocabulary emitter) rather than being
/// independently mapped, so the canonical document model and the SDK
/// vocabulary never publish two spellings of the same shape.
const VOCAB_ALIASES: &[&str] = &[
    "WriteOperation",
    "ResourceTrigger",
    "ResourceRoot",
    "ResourceRender",
    "SchemaForm",
];

/// JSON Schema authoring marker (added via `#[schemars(extend(...))]` on the
/// Rust side) identifying a def whose Rust newtype accepts scalar-or-array at
/// authoring time but always serializes as an array.
const SCALAR_OR_ARRAY_MARKER: &str = "x-ctx-authoring";

pub fn render(schema: &Value) -> crate::Result<String> {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_model("canonical schema has no $defs"))?;

    let marked_array_items = marked_array_item_types(defs)?;

    let mut out = String::new();
    out.push_str("// --- Canonical document model (generated from the Rust trait schema) ---\n\n");

    for (name, def) in defs {
        if VOCAB_ALIASES.contains(&name.as_str()) {
            out.push_str(&format!(
                "export type Canonical{name} = {name};\n\n",
                name = name
            ));
            continue;
        }
        out.push_str(&render_named_def(name, def, defs, &marked_array_items)?);
    }

    out.push_str(&render_trait_types(schema, defs, &marked_array_items)?);
    Ok(out)
}

/// Pre-scan `$defs` for array-typed defs carrying the scalar-or-array
/// authoring marker, mapping def name to the TS type of one array element.
/// Property positions that `$ref` such a def widen to `T | readonly T[]`
/// instead of the array alias, matching the Rust newtype's authoring shape.
fn marked_array_item_types(defs: &Map<String, Value>) -> crate::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, def) in defs {
        let Some(def) = def.as_object() else {
            continue;
        };
        if def.get(SCALAR_OR_ARRAY_MARKER).and_then(Value::as_str) != Some("scalar-or-array") {
            continue;
        }
        let items = def
            .get("items")
            .ok_or_else(|| invalid_model(format!("{name}: scalar-or-array def has no items")))?;
        out.insert(name.clone(), map_schema(items, defs, &BTreeMap::new())?);
    }
    Ok(out)
}

fn ts_ident(name: &str) -> String {
    format!("Canonical{name}")
}

fn ref_target(schema: &Value) -> Option<&str> {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .map(|r| r.trim_start_matches("#/$defs/"))
}

/// Map an arbitrary JSON Schema fragment (a def body, an `items` schema, an
/// `additionalProperties` schema, a `oneOf`/`anyOf` member) to a TS type.
/// Never widens scalar-or-array refs — that only happens at property
/// positions, via [`map_property`].
fn map_schema(
    schema: &Value,
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
) -> crate::Result<String> {
    if let Value::Bool(allowed) = schema {
        return Ok(if *allowed { "JsonValue" } else { "never" }.to_string());
    }
    if let Some(target) = ref_target(schema) {
        if !defs.contains_key(target) {
            return Err(invalid_model(format!("$ref to missing def {target}")));
        }
        return Ok(if VOCAB_ALIASES.contains(&target) {
            format!("Canonical{target}")
        } else {
            ts_ident(target)
        });
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| invalid_model("expected a JSON Schema object"))?;

    if let Some(constant) = obj.get("const") {
        return literal(constant);
    }
    if let Some(values) = obj.get("enum").and_then(Value::as_array) {
        return string_union(values);
    }
    if let Some(members) = obj.get("oneOf").and_then(Value::as_array) {
        return map_variants(members, defs, marked);
    }
    if let Some(members) = obj.get("anyOf").and_then(Value::as_array) {
        let non_null: Vec<&Value> = members
            .iter()
            .filter(|m| m.get("type").and_then(Value::as_str) != Some("null"))
            .collect();
        if non_null.is_empty() {
            return Err(invalid_model("anyOf has no non-null members"));
        }
        let mapped = non_null
            .iter()
            .map(|m| map_schema(m, defs, marked))
            .collect::<crate::Result<Vec<_>>>()?;
        return Ok(dedupe_join(mapped));
    }
    if let Some(additional) = obj.get("additionalProperties")
        && let Some(value_schema) = additional.as_object()
    {
        let value_ty = map_schema(&Value::Object(value_schema.clone()), defs, marked)?;
        return Ok(format!("Readonly<Record<string, {value_ty}>>"));
    }
    if obj.contains_key("properties") || obj.get("type").and_then(Value::as_str) == Some("object") {
        return render_object_body(obj, defs, marked, &[]);
    }
    if let Some(items) = obj.get("items") {
        let item_ty = map_schema(items, defs, marked)?;
        return Ok(format!("readonly {item_ty}[]"));
    }
    let base_type = base_scalar_type(obj)?;
    if let Some(ty) = base_type {
        return Ok(ty);
    }
    // Empty/permissive schema (serde_json::Value passthrough fields).
    Ok("JsonValue".to_string())
}

fn base_scalar_type(obj: &Map<String, Value>) -> crate::Result<Option<String>> {
    let Some(type_value) = obj.get("type") else {
        return Ok(None);
    };
    let names: Vec<&str> = match type_value {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => return Err(invalid_model("unsupported \"type\" shape")),
    };
    let mapped: Vec<&str> = names
        .into_iter()
        .map(|name| match name {
            "string" => Ok("string"),
            "boolean" => Ok("boolean"),
            "integer" | "number" => Ok("number"),
            "object" => Ok("JsonObject"),
            other => Err(invalid_model(format!("unmapped JSON Schema type {other}"))),
        })
        .collect::<crate::Result<Vec<_>>>()?;
    if mapped.is_empty() {
        return Ok(None);
    }
    Ok(Some(dedupe_join(
        mapped.into_iter().map(str::to_string).collect(),
    )))
}

/// `oneOf` members: either a pure string enum (every member a `const` string
/// or a plain string `enum` list) or a union of object shapes.
fn map_variants(
    members: &[Value],
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
) -> crate::Result<String> {
    let mut parts = Vec::new();
    for member in members {
        let mapped = if let Some(constant) = member.get("const") {
            literal(constant)?
        } else if let Some(values) = member.get("enum").and_then(Value::as_array) {
            string_union(values)?
        } else {
            map_schema(member, defs, marked)?
        };
        parts.push(mapped);
    }
    Ok(dedupe_join(parts))
}

fn literal(value: &Value) -> crate::Result<String> {
    match value {
        Value::String(s) => Ok(format!("{s:?}")),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(invalid_model(format!("unsupported const literal {other}"))),
    }
}

fn string_union(values: &[Value]) -> crate::Result<String> {
    let parts = values
        .iter()
        .map(literal)
        .collect::<crate::Result<Vec<_>>>()?;
    Ok(dedupe_join(parts))
}

fn dedupe_join(mut parts: Vec<String>) -> String {
    parts.dedup();
    parts.join(" | ")
}

/// Map a property's schema, applying scalar-or-array widening when the
/// property is (possibly through an `anyOf [..., null]` Option wrapper) a
/// bare `$ref` to a marked array def.
fn map_property(
    schema: &Value,
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
) -> crate::Result<String> {
    let bare_ref = ref_target(schema).or_else(|| {
        schema
            .get("anyOf")
            .and_then(Value::as_array)
            .and_then(|members| {
                let non_null: Vec<&Value> = members
                    .iter()
                    .filter(|m| m.get("type").and_then(Value::as_str) != Some("null"))
                    .collect();
                match non_null.as_slice() {
                    [only] => ref_target(only),
                    _ => None,
                }
            })
    });
    if let Some(target) = bare_ref
        && let Some(item_ty) = marked.get(target)
    {
        return Ok(format!("{item_ty} | readonly {item_ty}[]"));
    }
    map_schema(schema, defs, marked)
}

fn jsdoc(indent: &str, description: Option<&str>) -> String {
    let Some(description) = description else {
        return String::new();
    };
    let safe = description.replace("*/", "* /");
    let mut out = format!("{indent}/**\n");
    for line in safe.lines() {
        out.push_str(&format!("{indent} * {line}\n"));
    }
    out.push_str(&format!("{indent} */\n"));
    out
}

fn render_object_body(
    obj: &Map<String, Value>,
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
    force_optional: &[String],
) -> crate::Result<String> {
    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<String> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if properties.is_empty() {
        return Ok("Readonly<Record<string, never>>".to_string());
    }

    let mut out = String::from("{\n");
    for (key, value_schema) in &properties {
        let ty = map_property(value_schema, defs, marked)?;
        let optional = !required.contains(key) || force_optional.contains(key);
        let description = value_schema.get("description").and_then(Value::as_str);
        out.push_str(&jsdoc("  ", description));
        // `| undefined` on every optional field: the workspace tsconfig sets
        // `exactOptionalPropertyTypes`, under which `foo?: T` (without the
        // explicit `| undefined` arm) rejects an incrementally-built
        // producer assigning `undefined` to trim an absent field — the
        // pattern `sequence.ts`'s canonical assembly (and others) rely on.
        out.push_str(&format!(
            "  readonly {key:?}{opt}: {ty}{undef};\n",
            opt = if optional { "?" } else { "" },
            undef = if optional { " | undefined" } else { "" }
        ));
    }
    out.push('}');
    Ok(out)
}

fn render_named_def(
    name: &str,
    def: &Value,
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
) -> crate::Result<String> {
    let ident = ts_ident(name);
    let obj = def
        .as_object()
        .ok_or_else(|| invalid_model(format!("{name}: def is not an object")))?;
    let description = obj.get("description").and_then(Value::as_str);
    let doc = jsdoc("", description);
    let body = map_schema(def, defs, marked)?;
    Ok(format!("{doc}export type {ident} = {body};\n\n"))
}

/// `Trait`'s own required fields (identity: `id`, `schema-version`,
/// `version`, `name`, `summary`) are optional on `CanonicalTraitDraft` — the
/// CDK draft is a pre-synth document.
fn render_trait_types(
    schema: &Value,
    defs: &Map<String, Value>,
    marked: &BTreeMap<String, String>,
) -> crate::Result<String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| invalid_model("root schema is not an object"))?;
    let description = obj.get("description").and_then(Value::as_str);
    let required: Vec<String> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str(&jsdoc("", description));
    out.push_str(&format!(
        "export type CanonicalTrait = {};\n\n",
        render_object_body(obj, defs, marked, &[])?
    ));

    out.push_str(
        "/**\n * The CDK's pre-synth draft document: `Trait`'s identity fields\n * (`id`, `schema-version`, `version`, `name`, `summary`) are optional.\n */\n",
    );
    let draft_body = render_object_body(obj, defs, marked, &required)?;
    out.push_str(&format!(
        "export type CanonicalTraitDraft = {draft_body};\n\n"
    ));

    Ok(out)
}
