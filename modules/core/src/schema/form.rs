//! Schema forms: string-only schema refs for slot and input `schema` fields.
//!
//! P26.5-fix narrows schema forms to string-only values:
//!
//! 1. **Built-in refs**: `schema:text`, `schema:boolean`, `schema:number`,
//!    `schema:integer`, `schema:any`
//! 2. **Local/dependency schema refs**: `schema:<id>` pointing to a declared
//!    `[[schema]]` or a dependency schema
//! 3. **List wrappers**: `"[schema:<id>]"` for list-of schemas
//! 4. **Union wrappers**: `"(schema:<id>|schema:<id>)"` for ordered unions
//!
//! Direct `resource:*` refs, inline JSON Schema objects, arrays, booleans,
//! numbers, and null are **not accepted**. Non-string schema values produce
//! field-specific diagnostics.

use std::borrow::Cow;
use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::reference::{Kind, Reference};
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
    strum::EnumIter,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Builtin {
    Text,
    Boolean,
    Number,
    Integer,
    Any,
    #[serde(rename = "checklist-item")]
    #[strum(serialize = "checklist-item")]
    ChecklistItem,
}

impl Builtin {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn as_ref(self) -> String {
        format!("schema:{}", self.as_str())
    }

    pub fn from_ref(ref_text: &str) -> Option<Self> {
        ref_text.strip_prefix("schema:")?.parse().ok()
    }
}

/// A schema reference for slot/input `schema` fields.
///
/// Always a string value — built-in, local/dependency schema ref, list wrapper,
/// or union wrapper. Non-string schema values are rejected at deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    /// Built-in schema ref (e.g. `schema:text`).
    Builtin(Builtin),
    /// List wrapper: `"[schema:scope]"`. The inner string is the wrapped
    /// schema ref (e.g. `"schema:scope"`).
    List(String),
    /// Ordered union wrapper whose members are plain schema refs.
    Union(Vec<String>),
    /// Local or dependency schema ref (e.g. `"schema:scope"`).
    Ref(String),
}

impl Schema {
    /// The canonical string representation.
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Builtin(b) => Cow::Owned(b.as_ref()),
            Self::List(inner) => Cow::Owned(format!("[{inner}]")),
            Self::Union(members) => Cow::Owned(format!("({})", members.join("|"))),
            Self::Ref(s) => Cow::Borrowed(s),
        }
    }
}

impl std::fmt::Display for Schema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

impl Serialize for Schema {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for Schema {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_schema_string(&s).map_err(serde::de::Error::custom)
    }
}

/// Parse a schema string into a [`Schema`] value.
fn parse_schema_string(s: &str) -> Result<Schema, String> {
    // Check for list wrapper: "[schema:...]"
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() || inner.contains('[') || inner.contains(']') {
            return Err(format!(
                "invalid list schema wrapper {s:?}: empty or nested brackets"
            ));
        }
        if !inner.starts_with("schema:") {
            return Err(format!(
                "invalid list schema wrapper {s:?}: inner value must be a schema ref"
            ));
        }
        return Ok(Schema::List(inner.to_string()));
    }
    if s.starts_with('[') || s.ends_with(']') {
        return Err(format!(
            "invalid list schema wrapper {s:?}: unpaired brackets"
        ));
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            return Err(format!(
                "invalid union schema wrapper {s:?}: union must not be empty"
            ));
        }
        let mut seen = BTreeSet::new();
        let mut members = Vec::new();
        for member in inner.split('|') {
            if member.is_empty() || member.chars().any(char::is_whitespace) {
                return Err(format!(
                    "invalid union schema wrapper {s:?}: members must be schema refs without whitespace"
                ));
            }
            // A member is either a plain schema ref or one list wrapper over
            // a plain schema ref. Nested unions/parens stay rejected.
            let member_ref = if member.starts_with('[') && member.ends_with(']') {
                let inner_ref = &member[1..member.len() - 1];
                if inner_ref.is_empty() || inner_ref.contains(['(', ')', '[', ']', '|']) {
                    return Err(format!(
                        "invalid union schema wrapper {s:?}: list member {member:?} must wrap a plain schema ref"
                    ));
                }
                inner_ref
            } else if member.contains(['(', ')', '[', ']', '|']) {
                return Err(format!(
                    "invalid union schema wrapper {s:?}: members must be plain schema refs or one list wrapper"
                ));
            } else {
                member
            };
            let parsed = Reference::parse(member_ref).map_err(|_| {
                format!("invalid union schema wrapper {s:?}: invalid member {member:?}")
            })?;
            if parsed.kind() != Kind::Schema {
                return Err(format!(
                    "invalid union schema wrapper {s:?}: member {member:?} must be a schema ref"
                ));
            }
            if !seen.insert(member) {
                return Err(format!(
                    "invalid union schema wrapper {s:?}: duplicate member {member:?}"
                ));
            }
            members.push(member.to_string());
        }
        return Ok(Schema::Union(members));
    }
    if s.starts_with('(') || s.ends_with(')') {
        return Err(format!(
            "invalid union schema wrapper {s:?}: unpaired parentheses"
        ));
    }

    // Built-in check.
    if let Some(b) = Builtin::from_ref(s) {
        return Ok(Schema::Builtin(b));
    }

    // Everything else is a schema ref (local or dependency).
    Ok(Schema::Ref(s.to_string()))
}

impl JsonSchema for Schema {
    fn schema_name() -> Cow<'static, str> {
        "SchemaForm".into()
    }

    fn json_schema(r#gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        r#gen.subschema_for::<String>()
    }
}

impl Schema {
    /// Parse a schema string into a [`Schema`] value.
    pub fn try_from_str(s: &str) -> Result<Self, String> {
        parse_schema_string(s)
    }
}

/// Validate a schema value against declared local schemas.
///
/// - Built-in: always valid.
/// - List: inner must be a valid `schema:*` ref (built-in or declared).
/// - Ref: must be `schema:*`. Built-ins are always valid. Local unqualified
///   refs must exist in `declared_schema_ids`. Dependency-qualified refs are
///   accepted as dependency-pending.
pub fn validate(
    schema: &Schema,
    field_path: &str,
    declared_schema_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    match schema {
        Schema::Builtin(_) => Ok(()),
        Schema::List(inner) => validate_schema_ref(inner, field_path, declared_schema_ids),
        Schema::Union(members) => {
            for member in members {
                // Members are plain refs or one list wrapper over a ref;
                // both validate against the inner schema ref.
                let member_ref = member
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                    .unwrap_or(member);
                validate_schema_ref(member_ref, field_path, declared_schema_ids)?;
            }
            Ok(())
        }
        Schema::Ref(ref_text) => validate_schema_ref(ref_text, field_path, declared_schema_ids),
    }
}

fn validate_schema_ref(
    ref_text: &str,
    field_path: &str,
    declared_schema_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid schema ref {ref_text:?}"),
    })?;

    if parsed.kind() != Kind::Schema {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "schema must be kind {:?}, got {:?}",
                Kind::Schema,
                parsed.kind()
            ),
        }
        .into());
    }

    // Built-in schemas are always valid.
    if Builtin::from_ref(ref_text).is_some() {
        return Ok(());
    }

    // Dependency-qualified refs are pending.
    if parsed.is_qualified() {
        return Ok(());
    }

    // Local unqualified refs must be declared.
    if !declared_schema_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "schema ref {ref_text:?} is not a built-in and not a declared local schema"
            ),
        }
        .into());
    }

    Ok(())
}
