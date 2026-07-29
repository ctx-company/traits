//! Shared types: `OneOrMany<T>`, `Slug`, `SlugList` for taxonomy values.
//!
//! - `OneOrMany<T>` accepts scalar-or-array input via `#[serde(untagged)]`.
//!   Serialization preserves the variant; used for non-taxonomy fields.
//! - `Slug` deserializes permissively (raw string), validates on explicit
//!   construction or through `check()`.
//! - `SlugList` is a `Vec<Slug>`-backed newtype that accepts scalar-or-array
//!   input and always serializes as an array. Slug validation happens in the
//!   post-decode taxonomy validation pass, not during deserialization, so
//!   diagnostics carry canonical field paths.
//! - [`deserialize_string_list`] is the shared scalar-or-array `Vec<String>`
//!   deserializer used by `ContractRefList` and
//!   `RefList`. Each wrapper type keeps its own identity so prompt
//!   and procedure contracts cannot drift together, but the decode
//!   logic lives in one place.

use schemars::JsonSchema;
use serde::de::{self, SeqAccess};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use thiserror::Error as ThisError;

/// Shared taxonomy and shape validation errors.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("shape mismatch at {field_path}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field_path: String,
        expected: String,
        actual: String,
    },
}

impl Error {
    pub(crate) fn shape_mismatch(
        field_path: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::ShapeMismatch {
            field_path: field_path.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

/// Deserialize a scalar-or-array of strings into `Vec<String>`.
///
/// Shared decoder used by
/// [`ContractRefList`](crate::trait::prompt::ContractRefList) and
/// [`RefList`](crate::trait::procedure::RefList). Keeping the
/// decode logic here avoids duplicating the same visitor; each wrapper type
/// maintains its own type identity for API clarity.
pub fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringListVisitor;

    impl<'de> de::Visitor<'de> for StringListVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Vec<String>, E> {
            Ok(vec![v])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Vec<String>, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                items.push(item);
            }
            Ok(items)
        }
    }

    deserializer.deserialize_any(StringListVisitor)
}

macro_rules! string_list_wrapper {
    ($(#[$meta:meta])* $vis:vis struct $name:ident) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
        #[serde(transparent)]
        $(#[$meta])*
        $vis struct $name(Vec<String>);

        impl $name {
            pub fn new(items: Vec<String>) -> Self {
                Self(items)
            }

            pub fn as_slice(&self) -> &[String] {
                &self.0
            }

            pub fn iter(&self) -> std::slice::Iter<'_, String> {
                self.0.iter()
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let items = crate::shared::deserialize_string_list(deserializer)?;
                Ok(Self(items))
            }
        }
    };
}

macro_rules! reference_list_wrapper {
    ($(#[$meta:meta])* $vis:vis struct $name:ident) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
        #[serde(transparent)]
        $(#[$meta])*
        $vis struct $name(Vec<crate::reference::Reference>);

        impl $name {
            pub fn new(items: Vec<crate::reference::Reference>) -> Self {
                Self(items)
            }

            pub fn as_slice(&self) -> &[crate::reference::Reference] {
                &self.0
            }

            pub fn iter(&self) -> std::slice::Iter<'_, crate::reference::Reference> {
                self.0.iter()
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let items = crate::shared::deserialize_string_list(deserializer)?
                    .into_iter()
                    .map(|item| {
                        crate::reference::Reference::parse(&item)
                            .map_err(serde::de::Error::custom)
                    })
                    .collect::<Result<_, _>>()?;
                Ok(Self(items))
            }
        }
    };
}

pub(crate) use reference_list_wrapper;
pub(crate) use string_list_wrapper;

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    crate::shared::reference_list_wrapper! {
        struct TestReferenceList
    }

    #[test]
    fn reference_list_deserialization_rejects_malformed_items() {
        let invalid: Result<TestReferenceList, _> =
            serde_json::from_str("[\"slot:ready\", \"port:\"]");
        assert!(invalid.is_err());

        let valid: TestReferenceList =
            serde_json::from_str("\"slot:ready\"").expect("valid scalar reference list");
        assert_eq!(valid.as_slice()[0].as_str(), "slot:ready");
        assert_eq!(valid.iter().count(), 1);
        assert!(!valid.is_empty());

        let empty = TestReferenceList::new(Vec::new());
        assert!(empty.is_empty());
    }
}

/// A value that may be authored as a single scalar or an array.
///
/// Serde deserializes this as whichever variant matches the input shape.
/// Serialization preserves the variant — used for non-taxonomy fields
/// (`TargetList`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::One(_) => false,
            Self::Many(v) => v.is_empty(),
        }
    }

    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::One(v) => std::slice::from_ref(v),
            Self::Many(v) => v.as_slice(),
        }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::One(v) => vec![v].into_iter(),
            Self::Many(v) => v.into_iter(),
        }
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(items: Vec<T>) -> Self {
        Self::Many(items)
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(item: T) -> Self {
        Self::One(item)
    }
}

/// Render/export target list: scalar-or-array of profile name strings.
///
/// Uses `OneOrMany<String>` which preserves scalar-vs-array authoring shape
/// in serialization. Not a taxonomy field.
pub type TargetList = OneOrMany<String>;

/// A kebab-case slug: lowercase letters/digits separated by single hyphens.
///
/// Deserializes permissively (raw string). Validation happens through
/// explicit construction (`Slug::new`, `Slug::new_at`) or the post-decode
/// taxonomy validation pass (`Slug::check`). Custom user slugs are allowed
/// when syntactically valid; no built-in vocabulary table is enforced.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema, derive_more::Display)]
pub struct Slug(String);

impl Slug {
    pub fn raw(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn new(value: impl Into<String>) -> crate::Result<Self> {
        let value = value.into();
        validate_slug_shape(&value, "slug")?;
        Ok(Self(value))
    }

    pub fn new_at(value: impl Into<String>, field_path: &str) -> crate::Result<Self> {
        let value = value.into();
        validate_slug_shape(&value, field_path)?;
        Ok(Self(value))
    }

    /// Validate this slug's shape with a canonical field path.
    pub fn check(&self, field_path: &str) -> crate::Result<()> {
        validate_slug_shape(&self.0, field_path)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Permissive deserialization: stores raw string without validation.
/// Taxonomy validation happens in the post-decode pass with canonical paths.
impl<'de> Deserialize<'de> for Slug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Slug(s))
    }
}

/// Kebab-case slug list: accepts scalar-or-array input and normalizes to
/// `Vec<Slug>`.
///
/// Always serializes as an array (canonical output). Slug shape validation
/// happens in the post-decode taxonomy pass, not during deserialization, so
/// diagnostics carry canonical field paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
pub struct SlugList(Vec<Slug>);

impl SlugList {
    pub fn new(items: Vec<Slug>) -> Self {
        Self(items)
    }

    pub fn as_slice(&self) -> &[Slug] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Slug> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for SlugList {
    type Item = Slug;
    type IntoIter = std::vec::IntoIter<Slug>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Deserializes scalar-or-array input without slug validation.
/// Taxonomy validation happens in the post-decode pass.
impl<'de> Deserialize<'de> for SlugList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let items = deserialize_string_list(deserializer)?
            .into_iter()
            .map(Slug::raw)
            .collect();
        Ok(Self(items))
    }
}

/// Validate that a string is a non-empty kebab-case slug.
///
/// Allows lowercase letters and digits separated by single hyphens. Rejects
/// empty, whitespace, uppercase, underscores, leading/trailing hyphens, and
/// repeated separators. The `field_path` identifies the canonical field for
/// diagnostics.
///
/// Trait IDs intentionally keep a separate legacy validator for now; see
/// `Trait::validate_identifier` for the documented transition decision.
pub fn validate_slug_shape(value: &str, field_path: &str) -> crate::Result<()> {
    if value.is_empty() {
        return Err(
            Error::shape_mismatch(field_path, "non-empty kebab-case slug", "empty string").into(),
        );
    }

    if value.starts_with('-') || value.ends_with('-') {
        return Err(Error::shape_mismatch(
            field_path,
            "kebab-case slug without leading/trailing hyphens",
            value,
        )
        .into());
    }

    if value.contains("--") {
        return Err(Error::shape_mismatch(
            field_path,
            "kebab-case slug with single hyphens only",
            value,
        )
        .into());
    }

    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(Error::shape_mismatch(
            field_path,
            "kebab-case slug (lowercase letters, digits, hyphens)",
            value,
        )
        .into());
    }

    Ok(())
}

/// Desugar a `family:variant` reference into its canonical slug.
///
/// No colon returns `Ok(None)` (the caller should treat `value` as already
/// canonical). Exactly one colon validates both halves as slugs via
/// [`validate_slug_shape`] against `{field_path}.family` and
/// `{field_path}.variant`; `family:default` desugars to the bare family,
/// anything else to `family-variant`. More than one colon is a shape
/// mismatch at `field_path`. String-only and deterministic: no filesystem or
/// host dependencies.
pub fn desugar_variant_ref(value: &str, field_path: &str) -> crate::Result<Option<String>> {
    let mut parts = value.splitn(3, ':');
    let family = parts.next().unwrap_or_default();
    let Some(variant) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        return Err(
            Error::shape_mismatch(field_path, "family:variant with a single colon", value).into(),
        );
    }

    validate_slug_shape(family, &format!("{field_path}.family"))?;
    validate_slug_shape(variant, &format!("{field_path}.variant"))?;

    if variant == "default" {
        Ok(Some(family.to_string()))
    } else {
        Ok(Some(format!("{family}-{variant}")))
    }
}
