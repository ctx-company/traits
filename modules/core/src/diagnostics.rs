//! Reference diagnostics and explanation data.
//!
//! Structured records produced by reference validation. These make reference
//! errors and explanations reusable by audit, check, render, and activation
//! without burying them in free-form CLI text. All records use stable ordering
//! so check/diff output remains deterministic.

use serde::{Deserialize, Serialize};

use crate::reference::Kind;

/// Stable diagnostic code for reference validation issues.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefDiagCode {
    /// A local ref points to a definition that does not exist in the trait.
    UnresolvedLocal,
    /// Two definitions share the same `(kind, id)` within one trait.
    DuplicateDefinition,
    /// A step output references a slot that was not explicitly declared.
    /// Rendering can tolerate this, so it is a warning, not an error.
    ImplicitStepSlot,
    /// A dependency-qualified ref uses an alias that is not declared.
    UnresolvedDependencyAlias,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// The outcome of attempting to resolve a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionOutcome {
    /// Resolved to a local definition within the trait.
    ResolvedLocal,
    /// Resolved (or would resolve) via a dependency namespace.
    ResolvedDependency,
    /// No matching local definition found.
    Unresolved,
    /// Dependency alias is not declared in the manifest.
    UnresolvedDependency,
}

/// A reference diagnostic record.
///
/// Produced when a reference cannot be resolved or has issues. Fields are
/// stable and deterministically ordered so check/diff/audit output stays
/// consistent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RefDiagnostic {
    /// Stable diagnostic code.
    pub code: RefDiagCode,
    /// Error, warning, or info.
    pub severity: Severity,
    /// The trait ID whose definitions were checked.
    pub trait_id: String,
    /// Canonical field path where the ref appears (e.g. `step.gather.input`).
    pub field_path: String,
    /// The original ref text as authored.
    pub ref_text: String,
    /// The kind parsed from the ref text.
    pub expected_kind: Kind,
    /// What happened during resolution.
    pub outcome: ResolutionOutcome,
}

impl PartialOrd for RefDiagnostic {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RefDiagnostic {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort key excludes derived diagnostic fields; field order is wire order.
        (
            &self.code,
            &self.severity,
            &self.trait_id,
            &self.field_path,
            &self.ref_text,
        )
            .cmp(&(
                &other.code,
                &other.severity,
                &other.trait_id,
                &other.field_path,
                &other.ref_text,
            ))
    }
}

/// The source of a resolved reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefSource {
    /// Resolved to a local definition within the trait.
    Local {
        /// Field path of the definition (e.g. `prompt.prepare-query`).
        field_path: String,
    },
    /// Dependency-qualified ref with a declared alias. The namespace exists
    /// in the manifest but the dependency's symbols are not loaded —
    /// resolution is pending until a loaded-symbol phase.
    DependencyQualified {
        /// The dependency alias / namespace prefix.
        namespace: String,
    },
    /// Built-in ref recognized without a definition (e.g. `schema:text`).
    Builtin,
    /// Ref kind or shape is not supported in the current MVP scope.
    Unsupported {
        /// Why this ref is not supported.
        reason: String,
    },
}

/// Explanation for a reference resolution.
///
/// Captures whether a ref is local, dependency-qualified, built-in, or
/// unsupported. Serializable for future JSON ABI and plugin consumers.
/// Unresolved refs produce [`RefDiagnostic`] records, not explanations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RefExplanation {
    /// The original ref text as authored.
    pub ref_text: String,
    /// The kind parsed from the ref text.
    pub kind: Kind,
    /// Where the ref resolved (or why it didn't).
    pub source: RefSource,
}

/// A diagnostic for a duplicate definition within one trait.
///
/// Produced when two definitions share the same `(kind, id)` pair.
/// Deterministically ordered for stable check/diff output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DuplicateDiagnostic {
    /// Stable diagnostic code (always [`RefDiagCode::DuplicateDefinition`]).
    pub code: RefDiagCode,
    /// Always [`Severity::Error`].
    pub severity: Severity,
    /// The trait ID whose definitions were checked.
    pub trait_id: String,
    /// The kind of the duplicate definition.
    pub kind: Kind,
    /// The duplicate definition ID.
    pub id: String,
    /// Field path of the first definition.
    pub first_field_path: String,
    /// Field path of the duplicate definition.
    pub duplicate_field_path: String,
}

impl PartialOrd for DuplicateDiagnostic {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DuplicateDiagnostic {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort key is the duplicate identity; metadata fields remain wire-first.
        (
            &self.kind,
            &self.id,
            &self.first_field_path,
            &self.duplicate_field_path,
        )
            .cmp(&(
                &other.kind,
                &other.id,
                &other.first_field_path,
                &other.duplicate_field_path,
            ))
    }
}
