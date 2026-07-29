//! Dependency handle, alias registry, and conflict detection.
//!
//! ## Dependency handle
//!
//! A [`DependencyHandle`] wraps a dependency alias and provides convenience
//! methods for creating dependency-qualified typed refs. In CDK and runtime
//! usage, a handle replaces manual string formatting of refs like
//! `prompt:aws/prepare-query`.
//!
//! ## Ref-only behavior
//!
//! This handle creates qualified refs only. It does not emit manifest
//! dependency declarations — manifest authoring emission is deferred to CDK
//! phases. Refs do not imply runtime availability: a qualified ref such as
//! `slot:aws/scope` addresses a definition declared by the dependency, but the
//! dependency must be loaded and the definition must exist before the ref can
//! be resolved at runtime.
//!
//! [`DependencyHandle::try_new`] validates aliases from untrusted input. Ref
//! construction always validates the complete canonical reference.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::Dependency;
use crate::reference::{Kind, Reference};

/// A dependency alias handle for creating qualified typed refs.
///
/// See the [module docs](self) for trusted vs fallible constructor guidance.
pub struct DependencyHandle {
    alias: String,
}

impl DependencyHandle {
    /// Create a handle without validating the alias.
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
        }
    }

    /// Create a handle, validating the alias as a typed-ref namespace.
    ///
    /// Uses the shared kebab-case slug validator so alias grammar is
    /// consistent with manifest dependency validation. Returns an error if
    /// the alias is empty, uppercase, contains underscores, dots, colons,
    /// slashes, whitespace, leading/trailing hyphens, or repeated hyphens.
    pub fn try_new(alias: &str) -> crate::Result<Self> {
        crate::shared::validate_slug_shape(alias, "dependency.alias")?;
        Ok(Self {
            alias: alias.to_string(),
        })
    }

    /// The dependency alias this handle qualifies refs with.
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Create a qualified typed ref.
    pub fn ref_of(&self, kind: Kind, id: &str) -> crate::Result<Reference> {
        Reference::qualified(kind, &self.alias, id)
    }

    /// Create a qualified typed ref, validating via [`Reference::parse`].
    ///
    /// Returns an error if the resulting ref is invalid (empty id, illegal
    /// separators, etc.).
    pub fn try_ref_of(&self, kind: Kind, id: &str) -> crate::Result<Reference> {
        Reference::parse(&format!("{}:{}/{}", kind, self.alias, id))
    }

    // --- Convenience methods ---

    /// Qualified `prompt:` ref. See [`Self::ref_of`].
    pub fn prompt(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Prompt, id)
    }

    /// Qualified `slot:` ref. See [`Self::ref_of`].
    pub fn slot(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Slot, id)
    }

    /// Qualified `port:` ref. See [`Self::ref_of`].
    pub fn port(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Port, id)
    }

    /// Qualified `resource:` ref. See [`Self::ref_of`].
    pub fn resource(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Resource, id)
    }

    /// Qualified `rule:` ref. See [`Self::ref_of`].
    pub fn rule(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Rule, id)
    }

    /// Qualified `signal:` ref. See [`Self::ref_of`].
    pub fn signal(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Signal, id)
    }

    /// Qualified `schema:` ref. See [`Self::ref_of`].
    pub fn schema(&self, id: &str) -> crate::Result<Reference> {
        self.ref_of(Kind::Schema, id)
    }
}

// ---------------------------------------------------------------------------
// Dependency alias registry
// ---------------------------------------------------------------------------

/// Set of declared dependency aliases for reference validation.
///
/// Built from a manifest's `[[dependency]]` sections. Live reference validation
/// uses this registry when checking dependency-qualified refs, so known aliases
/// can remain dependency-pending while undeclared aliases are unresolved.
#[derive(Debug, Clone, Default)]
pub struct DependencyAliases {
    aliases: BTreeSet<String>,
}

impl DependencyAliases {
    /// Build from a slice of manifest dependencies.
    pub fn from_dependencies(deps: &[Dependency]) -> Self {
        Self {
            aliases: deps.iter().map(|d| d.alias.clone()).collect(),
        }
    }

    /// Whether the given alias is declared.
    pub fn contains(&self, alias: &str) -> bool {
        self.aliases.contains(alias)
    }

    /// Whether no aliases are declared.
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Dependency conflict detection
// ---------------------------------------------------------------------------

/// Kind of dependency conflict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictKind {
    /// Same alias declared for different trait IDs.
    DuplicateAlias,
    /// Same trait ID requested with incompatible versions.
    IncompatibleVersion,
}

/// A single participant in a dependency conflict.
///
/// `source_index` is the position of this declaration in the input slice
/// passed to [`detect_dependency_conflicts`]. It lets consumers point back to
/// the original declaration. Participants are sorted by `source_index`
/// (declaration order) within each conflict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConflictParticipant {
    /// Position in the input dependency slice (declaration order).
    pub source_index: usize,
    pub alias: String,
    pub trait_id: String,
    pub version: String,
}

impl ConflictParticipant {
    fn from_dep(dep: &Dependency, source_index: usize) -> Self {
        Self {
            source_index,
            alias: dep.alias.clone(),
            trait_id: dep.id.clone(),
            version: dep.version.clone(),
        }
    }
}

/// A structured dependency conflict record for composition and check phases.
///
/// Produced by [`detect_dependency_conflicts`]. Does not resolve conflicts
/// automatically — phases that consume these records decide how to handle them.
///
/// `participants` is the authority: it carries all conflicting declarations
/// with their `source_index`, `alias`, `trait_id`, and `version`. There is no
/// winner or selected declaration. Participants are sorted by `source_index`
/// (declaration order in the input slice).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DependencyConflict {
    pub kind: ConflictKind,
    pub participants: Vec<ConflictParticipant>,
}

/// Detect dependency conflicts in a flat list of dependencies.
///
/// Works on any flat slice — pre-validation, cross-manifest, composition, or
/// check inputs where conflicting declarations may be aggregated. Trusted
/// manifest decoding rejects duplicates within one manifest, but this function
/// reports conflicts without rejecting.
///
/// Checks for:
/// - **Duplicate alias**: same alias used by different trait IDs
/// - **Incompatible version**: same trait ID requested with different versions
///
/// Each conflict's `participants` are sorted by `source_index` (declaration
/// order in the input slice). The returned conflicts are sorted by kind then
/// participants for deterministic output. Does not modify or resolve conflicts
/// — callers (composition/check phases) decide how to handle them.
pub fn detect_dependency_conflicts(deps: &[Dependency]) -> Vec<DependencyConflict> {
    let mut conflicts = Vec::new();

    let mut by_alias: BTreeMap<String, Vec<(usize, &Dependency)>> = BTreeMap::new();
    let mut by_trait: BTreeMap<String, Vec<(usize, &Dependency)>> = BTreeMap::new();

    for (idx, dep) in deps.iter().enumerate() {
        by_alias
            .entry(dep.alias.clone())
            .or_default()
            .push((idx, dep));
        by_trait.entry(dep.id.clone()).or_default().push((idx, dep));
    }

    for group in by_alias.values() {
        let trait_ids: BTreeSet<&str> = group.iter().map(|(_, d)| d.id.as_str()).collect();
        if trait_ids.len() > 1 {
            let mut participants: Vec<_> = group
                .iter()
                .map(|(idx, d)| ConflictParticipant::from_dep(d, *idx))
                .collect();
            participants.sort();
            conflicts.push(DependencyConflict {
                kind: ConflictKind::DuplicateAlias,
                participants,
            });
        }
    }

    for group in by_trait.values() {
        let versions: BTreeSet<&str> = group.iter().map(|(_, d)| d.version.as_str()).collect();
        if versions.len() > 1 {
            let mut participants: Vec<_> = group
                .iter()
                .map(|(idx, d)| ConflictParticipant::from_dep(d, *idx))
                .collect();
            participants.sort();
            conflicts.push(DependencyConflict {
                kind: ConflictKind::IncompatibleVersion,
                participants,
            });
        }
    }

    conflicts.sort();
    conflicts
}
