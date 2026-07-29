//! Project-level npm registry lock evidence model.
//!
//! `.ctx/traits.lock` records exact registry evidence for npm-transport
//! project dependencies (P438): resolved package version, tarball integrity,
//! vendor path, and per-trait canonical/resource digest evidence for every
//! trait discovered inside the (possibly multi-trait) npm package.
//!
//! This is deliberately a separate document from the package-local
//! `trait.lock` ([`crate::lockfile`]): a project installs npm packages, a
//! package declares what it needs. Overloading one lock model with both
//! relations would conflate "what does this repo consume" with "what does
//! this trait require".

use serde::{Deserialize, Serialize};

/// The project-level `.ctx/traits.lock` document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectLock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// Resolved `extends` base evidence (P443), present only when the
    /// project manifest declares `extends`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BaseLockEntry>,
    #[serde(rename = "package", default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageLockEntry>,
}

impl ProjectLock {
    pub fn new(metadata: Metadata) -> Self {
        Self {
            metadata: Some(metadata),
            base: None,
            packages: Vec::new(),
        }
    }

    pub fn package_entry(&self, alias: &str) -> Option<&PackageLockEntry> {
        self.packages.iter().find(|entry| entry.alias == alias)
    }

    pub fn upsert_package(&mut self, entry: PackageLockEntry) {
        if let Some(existing) = self
            .packages
            .iter_mut()
            .find(|existing| existing.alias == entry.alias)
        {
            *existing = entry;
        } else {
            self.packages.push(entry);
        }
    }

    pub fn remove_package(&mut self, alias: &str) -> Option<PackageLockEntry> {
        let index = self
            .packages
            .iter()
            .position(|entry| entry.alias == alias)?;
        Some(self.packages.remove(index))
    }

    pub fn sort_for_output(&mut self) {
        self.packages
            .sort_by(|left, right| left.alias.cmp(&right.alias));
        for entry in &mut self.packages {
            entry.traits.sort_by(|left, right| left.id.cmp(&right.id));
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// Generated project-lock metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// One locked npm package: exact registry evidence plus every trait
/// discovered inside its (possibly multi-trait) dual-use tarball.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageLockEntry {
    /// Project manifest alias / vendor directory name.
    pub alias: String,
    /// Full npm package identifier, e.g. `@scope/name` or `name`.
    pub package: String,
    /// The selector authored in the project manifest at lock time (a caret
    /// range, explicit range, or dist-tag), recorded so a fresh clone can
    /// tell whether the manifest and lock still agree.
    pub requested: String,
    /// The exact npm version this lock entry resolved to.
    pub resolved_version: String,
    /// npm SRI integrity string for the tarball, e.g. `sha512-<base64>`.
    pub integrity: String,
    /// Vendor directory path relative to the repo root
    /// (`.ctx/traits/vendor/<alias>`).
    pub vendored_path: String,
    /// Aggregate digest over every regular file's relative path and content
    /// under the vendored package (see
    /// `ctx_traits_io::registry::compute_tree_digest`). Authoritative
    /// evidence for the *complete* vendored tree: unlike the per-trait
    /// canonical/resource digests below, it also covers files no other
    /// evidence names (`trait.toml`, `config.toml`, package metadata, and
    /// any file added or removed since lock time).
    #[serde(default)]
    pub tree_digest: String,
    /// `true` when this package entry was merged in from the resolved
    /// `extends` base's `[dependencies]` table rather than declared in this
    /// project's own local manifest (P443). Local declarations always win
    /// on alias collision, so an inherited entry's alias is never also a
    /// local declaration; this flag is what lets a later base update tell
    /// "no longer inherited, safe to prune" apart from "a local override
    /// the project author still wants".
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherited: bool,
    #[serde(rename = "trait", default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<TraitLockEntry>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Locked evidence for a resolved `extends` base (P443): exactly which
/// published package/version its manifest payload was read from, and the
/// digest of the decoded manifest content that produced the inherited
/// `[dependencies]` entries above — the reviewable trail for "why did this
/// package appear/change/disappear" in a `git diff` of this lock.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BaseLockEntry {
    /// The `extends` spec authored in the project manifest at lock time,
    /// recorded so a later sync can tell whether the manifest and lock
    /// still agree (same contract as `PackageLockEntry::requested`).
    pub extends: String,
    /// Full npm package identifier the base resolved to.
    pub package: String,
    /// The exact npm version the base resolved to.
    pub resolved_version: String,
    /// npm SRI integrity string for the base package's tarball.
    pub integrity: String,
    /// Path to the base's published project manifest, relative to the base
    /// package root (e.g. `.ctx/traits.toml`). Descriptive only: base
    /// resolution never trusts this string to build a filesystem path, it
    /// re-discovers the manifest inside a freshly staged, integrity-verified
    /// base package every time.
    pub manifest_path: String,
    /// Canonical digest of the base's decoded project manifest, so a base
    /// content change is reviewable in the lock diff even when its version
    /// number and integrity happen to be reused.
    pub manifest_digest: String,
}

/// Per-trait lock evidence for one trait discovered inside a locked npm
/// package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitLockEntry {
    /// Trait ID from its canonical document.
    pub id: String,
    /// Path to the trait's canonical manifest, relative to the vendored
    /// package root. Drives resolution: npm package names need not match
    /// trait IDs, and one package may contain multiple traits.
    pub canonical_path: String,
    /// The trait's own `schema-version`, checked against the running
    /// binary's supported schema versions before full decode.
    pub schema_version: String,
    pub source_digest: String,
    pub canonical_digest: String,
    pub model_visible_digest: String,
    pub resource_manifest_digest: String,
}
