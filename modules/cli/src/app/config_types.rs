//! P457: render `packages/config/src/generated.ts` — the TypeScript type
//! mirror of `ctx_traits_io::harness_config::RuntimeConfig`, derived
//! in-process from `schemars::schema_for!` rather than a second hand-copied
//! vocabulary. Closed in the Rust model (field names, enum values, shape,
//! optionality — `deny_unknown_fields` renders as `additionalProperties:
//! false`) is closed in TS; anything the loader leaves as a free-form
//! `Option<String>` (`harness`, `model`, `reasoning-effort`) stays `string`
//! here too, since narrowing it would invent a constraint the loader
//! doesn't enforce — exactly the second drift source the phase forbids.

use serde_json::Value;

/// The root Rust type's schema title is renamed to `CtxConfig` in the
/// rendered output — the name `defineConfig`/`assignment()` (in
/// `packages/config/src/index.ts`) actually anchor to — while every other
/// `$defs` entry keeps its Rust struct/enum name verbatim (already
/// PascalCase).
const ROOT_TYPE_NAME: &str = "CtxConfig";

/// Load a `schemars`-derived JSON Schema once, as the root object plus its
/// `$defs` map. [`render`] (the TS mirror) uses `RuntimeConfig`; the TS
/// surface (`packages/config`) still mirrors that shape pending 0180's
/// `defineConfig` vocabulary split. [`camel_to_kebab`] (the JS-object ->
/// Rust-model key rewrite every `config build` runs before validation)
/// uses `ConfigDocument` — the type `config build` actually decodes and
/// validates against since 0177.
fn load_schema_for<T: schemars::JsonSchema>() -> crate::Result<(
    serde_json::Map<String, Value>,
    serde_json::Map<String, Value>,
)> {
    let schema: schemars::Schema = schemars::schema_for!(T);
    let schema: Value = serde_json::to_value(&schema)
        .map_err(|source| crate::Error::json("serialize config JSON schema", source))?;
    let object = schema
        .as_object()
        .cloned()
        .ok_or_else(|| crate::Error::Command {
            message: "config schema root is not a JSON object".to_string(),
        })?;
    let defs = object
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok((object, defs))
}

/// Rewrite a `config build`-emitted JS object's keys from `config.ts`'s
/// camelCase field names back to the kebab-case names `ConfigDocument`
/// (and the TOML it loads) actually use — walking `ConfigDocument`'s own
/// schema, the type `config build` validates against since 0177 (distinct
/// from [`render`]'s `RuntimeConfig` schema, which mirrors the TS surface
/// pending 0180). Only keys that are themselves schema *field names* are
/// renamed: a `Record<string, T>` map's keys (vendor aliases, trait IDs,
/// ...) are author-chosen identifiers and pass through byte-for-byte. A key
/// with no matching field in its object's schema (a typo, or a field
/// outside `ConfigDocument` entirely) is left exactly as authored, so
/// `ConfigDocument`'s `deny_unknown_fields` decode error names the real,
/// un-renamed mistake rather than a silently kebab-cased guess.
pub(crate) fn camel_to_kebab(value: &Value) -> crate::Result<Value> {
    let (root, defs) = load_schema_for::<ctx_traits_io::config_document::ConfigDocument>()?;
    convert(value, &root, &defs)
}

fn convert(
    value: &Value,
    schema: &serde_json::Map<String, Value>,
    defs: &serde_json::Map<String, Value>,
) -> crate::Result<Value> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = ref_name(reference);
        let resolved = defs
            .get(&name)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| crate::Error::Command {
                message: format!("RuntimeConfig schema $ref {name} not found in $defs"),
            })?;
        return convert(value, &resolved, defs);
    }
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        return convert_union(value, variants, defs);
    }
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        return convert_union(value, variants, defs);
    }
    match value {
        Value::Object(map) => {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                let mut out = serde_json::Map::new();
                for (key, entry) in map {
                    let kebab = camel_to_kebab_str(key);
                    let converted = match properties.get(&kebab).and_then(Value::as_object) {
                        Some(property_schema) => convert(entry, property_schema, defs)?,
                        None => entry.clone(),
                    };
                    let out_key = if properties.contains_key(&kebab) {
                        kebab
                    } else {
                        key.clone()
                    };
                    out.insert(out_key, converted);
                }
                Ok(Value::Object(out))
            } else if let Some(additional) = schema.get("additionalProperties") {
                match additional.as_object() {
                    Some(inner) => {
                        let mut out = serde_json::Map::new();
                        for (key, entry) in map {
                            out.insert(key.clone(), convert(entry, inner, defs)?);
                        }
                        Ok(Value::Object(out))
                    }
                    None => Ok(value.clone()),
                }
            } else {
                Ok(value.clone())
            }
        }
        Value::Array(items) => match schema.get("items").and_then(Value::as_object) {
            Some(item_schema) => Ok(Value::Array(
                items
                    .iter()
                    .map(|item| convert(item, item_schema, defs))
                    .collect::<crate::Result<_>>()?,
            )),
            None => Ok(value.clone()),
        },
        _ => Ok(value.clone()),
    }
}

/// Pick the `anyOf`/`oneOf` variant matching `value`'s JSON shape (a
/// nullable-field wrapper, or `RoleAssignmentValue`'s untagged
/// `ProfileAssignment | ProfileAssignment[]`) and recurse into it. Every
/// other value kind (string, number, bool, null) needs no key rewriting
/// regardless of which variant is "correct", so any resolvable variant is
/// sufficient.
fn convert_union(
    value: &Value,
    variants: &[Value],
    defs: &serde_json::Map<String, Value>,
) -> crate::Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    for variant in variants {
        let Some(variant_schema) = variant.as_object() else {
            continue;
        };
        if variant_schema.get("type").and_then(Value::as_str) == Some("null") {
            continue;
        }
        let resolved = resolve_variant(variant_schema, defs)?;
        let matches = match value {
            Value::Array(_) => resolved.get("type").and_then(Value::as_str) == Some("array"),
            Value::Object(_) => {
                resolved.contains_key("properties") || resolved.contains_key("additionalProperties")
            }
            _ => true,
        };
        if matches {
            return convert(value, variant_schema, defs);
        }
    }
    Ok(value.clone())
}

fn resolve_variant(
    schema: &serde_json::Map<String, Value>,
    defs: &serde_json::Map<String, Value>,
) -> crate::Result<serde_json::Map<String, Value>> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = ref_name(reference);
        return defs
            .get(&name)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| crate::Error::Command {
                message: format!("RuntimeConfig schema $ref {name} not found in $defs"),
            });
    }
    Ok(schema.clone())
}

/// Lower-camel-case to kebab-case (`sessionMode` -> `session-mode`), the
/// inverse of [`to_camel_case`].
fn camel_to_kebab_str(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_uppercase() {
            out.push('-');
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

pub(crate) fn render() -> crate::Result<String> {
    let (object, defs) = load_schema_for::<ctx_traits_io::harness_config::RuntimeConfig>()?;
    let object = &object;

    let mut out = String::new();
    out.push_str("// GENERATED FILE — do not edit by hand.\n");
    out.push_str("// Regenerate with `ctx traits internal sdk-generate`.\n\n");
    out.push_str(
        "export type JsonValue = null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue | undefined };\n\n",
    );

    out.push_str(&render_named(ROOT_TYPE_NAME, object)?);
    out.push('\n');

    let mut names: Vec<&String> = defs.keys().collect();
    names.sort();
    for name in names {
        // 0171 cleanup: `model-tier` is retired (`has_tier_declaration` in
        // `ctx_traits_io::harness_config` keeps warning it, not rejecting
        // it) — pruned from the *rendered TS surface* only, so authoring
        // completion stops offering it, while `camel_to_kebab` (sharing
        // this same `load_schema`) still renames a legacy author's
        // `modelTier` for the loader's retired-warning path to catch. Do
        // not remove `model-tier` handling from `camel_to_kebab` to
        // "unify" with this skip-list — that would turn a legacy
        // `modelTier` key into an unknown-field decode error instead of
        // the warn-and-accept path.
        if name == "AgentModelTier" {
            continue;
        }
        let def =
            defs.get(name)
                .and_then(Value::as_object)
                .ok_or_else(|| crate::Error::Command {
                    message: format!("RuntimeConfig schema $defs.{name} is not a JSON object"),
                })?;
        out.push_str(&render_named(name, def)?);
        out.push('\n');
    }

    Ok(out)
}

fn render_named(name: &str, schema: &serde_json::Map<String, Value>) -> crate::Result<String> {
    if schema.contains_key("properties") || is_object_type(schema) {
        return render_interface(name, schema);
    }
    let ty = ts_type(&Value::Object(schema.clone()))?;
    Ok(format!("export type {name} = {ty};\n"))
}

fn is_object_type(schema: &serde_json::Map<String, Value>) -> bool {
    schema.contains_key("properties")
        || (matches!(schema.get("type").and_then(Value::as_str), Some("object"))
            && schema.get("additionalProperties").and_then(Value::as_bool) != Some(false))
}

fn render_interface(name: &str, schema: &serde_json::Map<String, Value>) -> crate::Result<String> {
    // A record-shaped def (`additionalProperties: { ... }`, no fixed
    // `properties`) renders as a type alias to `Record<string, T>` instead
    // of an interface.
    if !schema.contains_key("properties")
        && let Some(additional) = schema.get("additionalProperties")
        && additional.is_object()
    {
        let value_ty = ts_type(additional)?;
        return Ok(format!(
            "export type {name} = Record<string, {value_ty}>;\n"
        ));
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<String> = schema
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

    let mut names: Vec<&String> = properties.keys().collect();
    names.sort();
    let mut body = String::new();
    for property_name in names {
        // 0171 cleanup: `model-tier` is retired — see the matching skip in
        // `render` for why this asymmetric (render-only) prune is
        // deliberate.
        if property_name == "model-tier" && (name == "AgentDefaults" || name == "ProfileAssignment")
        {
            continue;
        }
        let property_schema = &properties[property_name];
        let ts_ty = ts_type(property_schema)?;
        let optional = !required.contains(property_name);
        let field = to_camel_case(property_name);
        body.push_str(&format!(
            "  {field}{}: {ts_ty};\n",
            if optional { "?" } else { "" }
        ));
    }
    Ok(format!("export interface {name} {{\n{body}}}\n"))
}

/// Render a JSON Schema fragment as a TypeScript type expression. Handles
/// exactly the shapes `schemars::schema_for!` produces for this crate's
/// config model: `$ref`, `anyOf`/`oneOf` (nullable wrappers, untagged
/// unions, and const-per-variant enums), string enums, primitives, arrays,
/// and `additionalProperties` maps. Anything else is a controlled
/// generation error naming the unhandled shape, rather than a silently
/// wrong `any`.
fn ts_type(schema: &Value) -> crate::Result<String> {
    let Some(object) = schema.as_object() else {
        return Ok("unknown".to_string());
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return Ok(ref_name(reference));
    }
    if let Some(variants) = object.get("anyOf").and_then(Value::as_array) {
        return render_union(variants);
    }
    if let Some(variants) = object.get("oneOf").and_then(Value::as_array) {
        return render_union(variants);
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return Ok(string_literal_union(values));
    }
    if let Some(additional) = object.get("additionalProperties") {
        if additional.is_object() {
            let value_ty = ts_type(additional)?;
            return Ok(format!("Record<string, {value_ty}>"));
        }
        if additional == &Value::Bool(true) {
            // An unconstrained value schema (schemars' rendering of
            // `serde_json::Value`, e.g. `setting: BTreeMap<String, Value>`)
            // — mirror the JsonValue alias the CDK generated file already
            // exports, rather than falling through to a bare `object`.
            return Ok("Record<string, JsonValue>".to_string());
        }
    }
    if object.contains_key("properties") {
        return render_inline_object(object);
    }
    let kind = match object.get("type") {
        Some(Value::Array(types)) => {
            let mut kinds: Vec<&str> = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|kind| *kind != "null")
                .collect();
            kinds.dedup();
            match kinds.as_slice() {
                [kind] => Some((*kind).to_string()),
                _ => None,
            }
        }
        Some(Value::String(kind)) => Some(kind.clone()),
        _ => None,
    };
    match kind.as_deref() {
        Some("array") => {
            let items = object.get("items").cloned().unwrap_or(Value::Bool(true));
            let item_ty = ts_type(&items)?;
            Ok(format!("{item_ty}[]"))
        }
        Some(kind) => Ok(primitive_ts_type(kind)),
        None => Ok("unknown".to_string()),
    }
}

fn render_inline_object(object: &serde_json::Map<String, Value>) -> crate::Result<String> {
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<String> = object
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
    let mut names: Vec<&String> = properties.keys().collect();
    names.sort();
    let mut parts = Vec::new();
    for name in names {
        let ty = ts_type(&properties[name])?;
        let optional = !required.contains(name);
        parts.push(format!(
            "{}{}: {ty}",
            to_camel_case(name),
            if optional { "?" } else { "" }
        ));
    }
    Ok(format!("{{ {} }}", parts.join("; ")))
}

fn render_union(variants: &[Value]) -> crate::Result<String> {
    let mut parts = Vec::new();
    for variant in variants {
        if variant.get("type").and_then(Value::as_str) == Some("null") {
            continue;
        }
        // A const-per-variant enum (schemars' rendering of a documented
        // fieldless enum, e.g. `AgentModelTier`): collect the literal
        // instead of recursing, so the union stays flat.
        if let Some(constant) = variant.get("const").and_then(Value::as_str) {
            parts.push(format!("\"{constant}\""));
            continue;
        }
        parts.push(ts_type(variant)?);
    }
    match parts.as_slice() {
        [] => Ok("null".to_string()),
        _ => {
            parts.dedup();
            Ok(parts.join(" | "))
        }
    }
}

fn string_literal_union(values: &[Value]) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn primitive_ts_type(kind: &str) -> String {
    match kind {
        "string" => "string".to_string(),
        "boolean" => "boolean".to_string(),
        "integer" | "number" => "number".to_string(),
        "null" => "null".to_string(),
        other => other.to_string(),
    }
}

fn ref_name(reference: &str) -> String {
    reference
        .rsplit('/')
        .next()
        .unwrap_or(reference)
        .to_string()
}

/// Deterministic kebab-case JSON property name to lower-camel-case
/// TypeScript field name (`session-mode` -> `sessionMode`).
fn to_camel_case(name: &str) -> String {
    let mut result = String::new();
    for part in name.split('-') {
        if part.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(part);
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            result.extend(first.to_uppercase());
            result.push_str(chars.as_str());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `camel_to_kebab` walks `ConfigDocument`'s schema (0177) — `agent.
    /// role.*.modelTier` was a `RuntimeConfig`-authoring concern from the
    /// retired P457 `config.ts` pathway and has no equivalent here; an
    /// unmatched key passes through unrenamed rather than being force-fit
    /// into a schema it doesn't belong to.
    #[test]
    fn camel_to_kebab_passes_through_keys_outside_the_document_schema() {
        let input = serde_json::json!({
            "agent": {
                "role": {
                    "worker": { "modelTier": "top" }
                }
            }
        });
        let converted = camel_to_kebab(&input).expect("camel_to_kebab");
        assert_eq!(
            converted["agent"]["role"]["worker"]["modelTier"],
            serde_json::json!("top")
        );
    }

    /// The rendered TS surface omits both the retired `model-tier` field
    /// (on `AgentDefaults`/`ProfileAssignment`) and the now-orphaned
    /// `AgentModelTier` type.
    #[test]
    fn render_omits_retired_model_tier_surface() {
        let rendered = render().expect("render");
        assert!(
            !rendered.contains("AgentModelTier"),
            "rendered TS surface still contains AgentModelTier:\n{rendered}"
        );
        assert!(
            !rendered.contains("modelTier"),
            "rendered TS surface still contains modelTier:\n{rendered}"
        );
    }
}
