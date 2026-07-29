//! Typed reference parsing: canonical `<kind>:<path>` references.
//!
//! All canonical references in the Agent Traits protocol use the syntax
//! `<kind>:<path>`. The path uses slash `/` for dependency or module namespace
//! separation. Dot `.` is TOML table structure only — not a canonical
//! reference separator — so dotted symbolic refs are rejected.
//!
//! Local refs have a single path segment (`slot:scope`); dependency-qualified
//! refs have a namespace prefix (`slot:aws/scope`). Both are the same kind with
//! different namespace depth.
//!
//! Fixture refs are an exception: they may contain file-extension dots in path
//! segments, such as `fixture:fixtures/rust-reviewer.agent-skills.md`. All other
//! kinds reject dots to prevent ambiguity with dotted symbolic refs.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error as ThisError;

/// Reference parsing errors.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("invalid typed reference: {ref_text}")]
    MissingSeparator { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    EmptyKind { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    ColonInPath { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    UnknownKind { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    EmptyPath { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    EmptyPathSegment { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    DotPathNotAllowed { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    LocalPathQualified { ref_text: String },

    #[error("invalid typed reference: {ref_text}")]
    QualifiedComponentInvalid { ref_text: String },

    #[error("unresolved reference: {ref_text}")]
    Unresolved { ref_text: String },

    #[error("step '{reader}' reads {ref_text}, never produced by any step")]
    SlotNeverProduced { reader: String, ref_text: String },

    #[error("step '{reader}' reads {ref_text}, first produced by later step '{producer}'")]
    SlotProducedLater {
        reader: String,
        ref_text: String,
        producer: String,
    },
}

impl Error {
    pub fn ref_text(&self) -> &str {
        match self {
            Self::MissingSeparator { ref_text }
            | Self::EmptyKind { ref_text }
            | Self::ColonInPath { ref_text }
            | Self::UnknownKind { ref_text }
            | Self::EmptyPath { ref_text }
            | Self::EmptyPathSegment { ref_text }
            | Self::DotPathNotAllowed { ref_text }
            | Self::LocalPathQualified { ref_text }
            | Self::QualifiedComponentInvalid { ref_text }
            | Self::Unresolved { ref_text }
            | Self::SlotNeverProduced { ref_text, .. }
            | Self::SlotProducedLater { ref_text, .. } => ref_text,
        }
    }
}

/// The known MVP reference kinds.
///
/// Unknown kinds are lint errors in MVP. The kind determines what kind of
/// protocol object a reference points to; it does not validate that the target
/// exists (that is P15+).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumIter,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "kebab-case")]
pub enum Kind {
    Prompt,
    Resource,
    Schema,
    Slot,
    Port,
    Trait,
    Capability,
    Intent,
    Behavior,
    Rule,
    Agent,
    Session,
    Signal,
    Sequence,
    Condition,
    Candidate,
    Fixture,
    Render,
    Report,
}

impl Kind {
    /// Parse a kind string, returning `None` for unknown kinds.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// Return the canonical lowercase string for this kind.
    pub fn as_str(&self) -> &'static str {
        (*self).into()
    }

    /// Whether this kind allows dots in reference paths.
    ///
    /// Only [`Kind::Fixture`] permits dots, for documented file-extension
    /// paths such as `fixture:fixtures/rust-reviewer.agent-skills.md`.
    fn allows_dot_paths(self) -> bool {
        matches!(self, Self::Fixture)
    }

    /// Whether this kind has a local definition section in the trait model.
    ///
    /// Locally declarable kinds (prompt, port, slot, resource, schema, agent,
    /// session, signal, sequence, condition, and rule) are validated against
    /// the local symbol table. Other kinds are built-in, deferred, or
    /// unsupported for local resolution.
    pub fn is_locally_declarable(self) -> bool {
        matches!(
            self,
            Self::Prompt
                | Self::Port
                | Self::Slot
                | Self::Resource
                | Self::Schema
                | Self::Rule
                | Self::Agent
                | Self::Session
                | Self::Signal
                | Self::Sequence
                | Self::Condition
        )
    }

    /// Whether this kind is exported as an SDK reference kind.
    ///
    /// SDK spelling comes from [`Self::as_str`], not serde's lowercase rename;
    /// that distinction matters if a future multi-word variant is added.
    pub fn is_sdk_ref_kind(self) -> bool {
        self.is_locally_declarable() || matches!(self, Self::Trait)
    }

    /// Human-readable reason why a non-declarable kind is not locally
    /// resolvable.
    ///
    /// Returns `None` for locally declarable kinds.
    pub fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Self::Trait => Some("trait refs are identity references, not locally resolvable"),
            Self::Capability => {
                Some("capability refs are deprecated in canonical dataflow; use port refs instead")
            }
            Self::Intent => Some("intent refs are trait-section references, not local definitions"),
            Self::Behavior => {
                Some("behavior refs are trait-section references, not local definitions")
            }
            Self::Candidate => Some("candidate resolution is not implemented in MVP"),
            Self::Fixture => Some("fixture resolution requires IO support"),
            Self::Render => Some("render target resolution is not implemented in MVP"),
            Self::Report => Some("report target resolution is not implemented in MVP"),
            Self::Prompt
            | Self::Resource
            | Self::Schema
            | Self::Slot
            | Self::Port
            | Self::Rule
            | Self::Agent
            | Self::Session
            | Self::Signal
            | Self::Sequence
            | Self::Condition => None,
        }
    }
}

/// Classified reference path: local or dependency-qualified.
///
/// [`Reference::ref_path`] returns this type to make the local-versus-qualified
/// distinction explicit.
///
/// Local refs have a single id segment with no slash:
/// `slot:scope` → `Path::Local { id: "scope" }`.
///
/// Dependency-qualified refs have at least one slash separating a namespace
/// prefix from the id:
/// `slot:aws/scope` → `Path::Qualified { namespace: "aws", id: "scope", path: "aws/scope" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path<'a> {
    /// A local reference with no dependency namespace.
    Local {
        /// The definition id (the entire path).
        id: &'a str,
    },
    /// A dependency-qualified reference with a namespace prefix.
    Qualified {
        /// The first path segment (dependency or module alias).
        namespace: &'a str,
        /// The last path segment (definition id within the namespace).
        id: &'a str,
        /// The full verbatim path (everything after `:`).
        path: &'a str,
    },
}

impl<'a> Path<'a> {
    /// The definition id: the last path segment.
    pub fn id(&self) -> &'a str {
        match self {
            Self::Local { id } => id,
            Self::Qualified { id, .. } => id,
        }
    }

    /// The dependency namespace, if qualified.
    pub fn namespace(&self) -> Option<&'a str> {
        match self {
            Self::Local { .. } => None,
            Self::Qualified { namespace, .. } => Some(namespace),
        }
    }

    /// Whether this ref is dependency-qualified.
    pub fn is_qualified(&self) -> bool {
        matches!(self, Self::Qualified { .. })
    }
}

/// A parsed canonical typed reference: `<kind>:<path>`.
///
/// The path is preserved verbatim from the source text. [`Self::ref_path`]
/// decomposes it into namespace and id for dependency-qualified refs.
/// `Display` round-trips to the canonical accepted text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct Reference(String);

struct ParsedReference<'a> {
    kind: Kind,
    path: &'a str,
}

impl Reference {
    /// Parse a canonical `<kind>:<path>` reference.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if:
    /// - there is no `:` separator
    /// - there are multiple `:` separators (colon inside the path)
    /// - the kind is empty
    /// - the kind is not one of the known MVP kinds
    /// - the path is empty
    /// - the path contains a dot `.` for non-fixture kinds (dotted symbolic
    ///   refs are not canonical)
    /// - the path contains empty slash segments (`//`, leading/trailing `/`)
    pub fn parse(text: &str) -> crate::Result<Self> {
        parse_components(text)?;
        Ok(Self(text.to_string()))
    }

    /// Construct a reference from canonical text.
    pub fn raw(text: impl Into<String>) -> crate::Result<Self> {
        Self::parse(&text.into())
    }

    /// Construct a local typed ref (no dependency namespace).
    ///
    pub fn local(kind: Kind, id: impl AsRef<str>) -> crate::Result<Self> {
        let ref_text = format!("{}:{}", kind, id.as_ref());
        if id.as_ref().contains('/') {
            return Err(Error::LocalPathQualified { ref_text }.into());
        }
        Self::parse(&ref_text)
    }

    /// Construct a dependency-qualified typed ref.
    ///
    pub fn qualified(
        kind: Kind,
        namespace: impl AsRef<str>,
        id: impl AsRef<str>,
    ) -> crate::Result<Self> {
        let ref_text = format!("{}:{}/{}", kind, namespace.as_ref(), id.as_ref());
        if namespace.as_ref().contains('/') || id.as_ref().contains('/') {
            return Err(Error::QualifiedComponentInvalid { ref_text }.into());
        }
        Self::parse(&ref_text)
    }

    /// The reference kind.
    ///
    /// Values decoded from wire data are validated during deserialization.
    pub fn kind(&self) -> Kind {
        self.components()
            .expect("reference must be validated before typed access")
            .kind
    }

    /// Fallible reference-kind access.
    pub fn try_kind(&self) -> crate::Result<Kind> {
        Ok(self.components()?.kind)
    }

    /// The original path verbatim (everything after `:`).
    ///
    pub fn path(&self) -> &str {
        self.components()
            .expect("reference must be validated before typed access")
            .path
    }

    /// The original reference text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate this reference and return typed components.
    fn components(&self) -> crate::Result<ParsedReference<'_>> {
        parse_components(&self.0)
    }

    /// Validate this reference with canonical typed-ref rules.
    pub fn check(&self) -> crate::Result<()> {
        self.components().map(|_| ())
    }

    /// Classify this ref's path as local or dependency-qualified.
    ///
    pub fn ref_path(&self) -> Path<'_> {
        let path = self.path();
        if let Some(first_idx) = path.find('/') {
            let last_idx = path.rfind('/').unwrap_or(first_idx);
            Path::Qualified {
                namespace: &path[..first_idx],
                id: &path[last_idx + 1..],
                path,
            }
        } else {
            Path::Local { id: path }
        }
    }

    /// Whether this ref is dependency-qualified (path contains at least one
    /// `/`).
    ///
    pub fn is_qualified(&self) -> bool {
        self.path().contains('/')
    }

    /// The dependency namespace: the first path segment before the first `/`.
    ///
    /// Returns `None` for local refs (no `/`).
    ///
    pub fn namespace(&self) -> Option<&str> {
        self.ref_path().namespace()
    }

    /// The definition id: the last path segment after the last `/`.
    ///
    /// For local refs, this is the entire path.
    ///
    pub fn id(&self) -> &str {
        self.ref_path().id()
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Reference {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Reference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl std::ops::Deref for Reference {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<Reference> for String {
    fn from(value: Reference) -> Self {
        value.0
    }
}

impl AsRef<str> for Reference {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

fn parse_components(text: &str) -> crate::Result<ParsedReference<'_>> {
    let Some((kind_str, path)) = text.split_once(':') else {
        return Err(Error::MissingSeparator {
            ref_text: text.to_string(),
        }
        .into());
    };

    if kind_str.is_empty() {
        return Err(Error::EmptyKind {
            ref_text: text.to_string(),
        }
        .into());
    }

    if path.contains(':') {
        return Err(Error::ColonInPath {
            ref_text: text.to_string(),
        }
        .into());
    }

    let kind = Kind::parse(kind_str).ok_or_else(|| Error::UnknownKind {
        ref_text: text.to_string(),
    })?;

    validate_path(kind, path, text)?;

    Ok(ParsedReference { kind, path })
}

/// Validate a reference path.
///
/// Rejects empty paths, empty slash segments, and dots for non-fixture kinds.
fn validate_path(kind: Kind, path: &str, original: &str) -> crate::Result<()> {
    if path.is_empty() {
        return Err(Error::EmptyPath {
            ref_text: original.to_string(),
        }
        .into());
    }

    if path.split('/').any(|seg| seg.is_empty()) {
        return Err(Error::EmptyPathSegment {
            ref_text: original.to_string(),
        }
        .into());
    }

    if !kind.allows_dot_paths() && path.contains('.') {
        return Err(Error::DotPathNotAllowed {
            ref_text: original.to_string(),
        }
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Kind, Path, Reference};

    #[test]
    fn local_and_qualified_references_have_total_typed_accessors() {
        let local = Reference::parse("slot:scope").expect("valid local reference");
        assert_eq!(local.kind(), Kind::Slot);
        assert_eq!(local.path(), "scope");
        assert_eq!(local.ref_path(), Path::Local { id: "scope" });

        let qualified = Reference::parse("slot:aws/scope").expect("valid qualified reference");
        assert_eq!(qualified.namespace(), Some("aws"));
        assert_eq!(qualified.id(), "scope");
        assert!(qualified.is_qualified());
    }

    #[test]
    fn deserialization_rejects_malformed_wire_references() {
        let invalid: Result<Reference, _> = serde_json::from_str("\"slot:\"");
        assert!(invalid.is_err());

        let valid: Reference =
            serde_json::from_str("\"schema:aws/user\"").expect("valid wire reference");
        assert_eq!(valid.kind(), Kind::Schema);
        assert_eq!(valid.path(), "aws/user");
    }

    #[test]
    fn constructors_reject_malformed_components() {
        assert!(Reference::raw("slot:").is_err());
        assert!(Reference::local(Kind::Slot, "bad/name").is_err());
        assert!(Reference::local(Kind::Slot, "").is_err());
        assert!(Reference::qualified(Kind::Slot, "aws", "scope").is_ok());
        assert!(Reference::qualified(Kind::Slot, "", "scope").is_err());
        assert!(Reference::qualified(Kind::Slot, "aws/team", "scope").is_err());
    }
}
