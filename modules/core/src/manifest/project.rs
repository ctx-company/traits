//! Project manifest structs: schema version, project metadata, default
//! targets, and trait dependency entries.
//!
//! Domain structs are the primary API. Scalar-or-array authoring sugar is
//! handled by `OneOrMany<String>` (`TargetList`) on `default_target` and
//! `target` fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::dependency::Dependency;
use super::source::TraitSource;
use crate::shared::TargetList;

/// Repo-level project manifest (`.ctx/traits.toml` / `.json` / `.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectManifest {
    pub schema_version: String,

    /// Optional depth-one project-scoped manifest inheritance (P443): an npm
    /// spec (e.g. `@org/team-base`, `@org/team-base@^1.0.0`) resolving to a
    /// published package whose payload is itself a project manifest at
    /// `.ctx/traits.*`. That base manifest's `[dependencies]` (npm project
    /// package installs) merge under this manifest's own `[dependencies]`
    /// before local entries, with local entries winning on alias collision.
    /// A base manifest that itself sets `extends` is refused: inheritance is
    /// depth-one only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectMetadata>,

    #[serde(default, rename = "trait", skip_serializing_if = "Vec::is_empty")]
    pub trait_entries: Vec<TraitEntry>,

    #[serde(default, rename = "dependency", skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,

    /// `[dependencies]` table: npm-transport project installs (P438), keyed
    /// by vendor alias. Distinct from `[[dependency]]` above (`dependencies`
    /// field, "dependency" TOML key): that table declares what a package
    /// needs from another trait; this table declares what the *project*
    /// installs from the npm registry. `ctx traits install` and `remove` are
    /// the only writers of this table.
    #[serde(
        default,
        rename = "dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub packages: BTreeMap<String, ProjectPackageDependency>,
}

/// One `[dependencies]` entry: an npm package the project installs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectPackageDependency {
    /// Full npm package identifier, e.g. `@scope/name` or `name`.
    pub npm: String,
    /// Semver range or dist-tag selector authored for this dependency.
    pub version: String,
}

/// Project-level metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Render/export targets. Accepts scalar-or-array at decode boundary via
    /// `TargetList` (`OneOrMany<String>`).
    #[serde(default, skip_serializing_if = "OneOrMany::is_empty")]
    pub default_target: TargetList,
}

/// A trait dependency entry in the project manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitEntry {
    pub id: String,
    pub version: String,
    pub source: TraitSource,

    /// Render/export targets. Accepts scalar-or-array at decode boundary via
    /// `TargetList` (`OneOrMany<String>`).
    #[serde(default, skip_serializing_if = "OneOrMany::is_empty")]
    pub target: TargetList,
}

use crate::shared::OneOrMany;
