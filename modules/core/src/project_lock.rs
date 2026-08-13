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

/// Which transport a locked package was installed through (P535).
/// `#[default]` is `Npm`, so a pre-P535 lock entry with no `transport` field
/// at all decodes exactly as it always has, with no unrelated lock churn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageTransport {
    #[default]
    Npm,
    /// A project-scoped local `path:` source (P535): no registry, no
    /// version, no tarball integrity — only an authored relative path and
    /// the vendored tree's digest evidence.
    Path,
    /// A git-transport source (0191): no registry, no npm SRI integrity —
    /// only a resolved repository URL, requested ref, and resolved commit
    /// sha, plus the vendored tree's digest evidence.
    Git,
}

/// One locked package: exact registry evidence for an npm-transport entry
/// (P438), or the authored relative path and digest evidence for a
/// path-transport entry (P535) — plus every trait discovered inside the
/// (possibly multi-trait) package either way.
///
/// The npm-only fields (`package`, `requested`, `resolved_version`,
/// `integrity`) stay flat top-level `String`s, defaulted to empty and
/// omitted from output when empty, rather than moving under a nested
/// `[package.source]` table: this is what lets a pre-P535 npm lock entry
/// keep decoding byte-identically with `transport` simply absent. A path
/// entry never populates them — it would otherwise be claiming npm SRI or
/// registry-resolution evidence it does not have — and instead populates
/// `path`, left empty (and omitted) for an npm entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageLockEntry {
    /// Project manifest alias / vendor directory name.
    pub alias: String,
    /// Transport this package was installed through. Omitted from output
    /// (and defaulted on decode) for the common npm case, so an ordinary
    /// npm-only project's lock is byte-identical to a pre-P535 lock.
    #[serde(default, skip_serializing_if = "PackageTransport::is_npm")]
    pub transport: PackageTransport,
    /// Full npm package identifier, e.g. `@scope/name` or `name`. Empty for
    /// a path-transport entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    /// The selector authored in the project manifest at lock time (a caret
    /// range, explicit range, or dist-tag), recorded so a fresh clone can
    /// tell whether the manifest and lock still agree. Empty for a
    /// path-transport entry, which has no version selector.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested: String,
    /// The exact npm version this lock entry resolved to. Empty for a
    /// path-transport entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_version: String,
    /// npm SRI integrity string for the tarball, e.g. `sha512-<base64>`.
    /// Empty for a path-transport entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integrity: String,
    /// The authored relative path (P535), normalized at parse time. Empty
    /// for an npm-transport entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Resolved git repository URL (0191). Empty for a non-git entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The ref (branch, tag, or explicit sha) authored/requested at lock
    /// time (0191). Empty for a non-git entry, and for an unpinned git entry
    /// that resolved from the default branch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_ref: String,
    /// The exact commit sha the requested ref resolved to (0191). Empty for
    /// a non-git entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_commit: String,
    /// Vendor directory path relative to the repo root
    /// (`.ctx/traits/vendored/<alias>`).
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

impl PackageTransport {
    fn is_npm(&self) -> bool {
        matches!(self, Self::Npm)
    }
}

impl PackageLockEntry {
    /// The unambiguous source identity for this locked entry: the npm
    /// package identifier, `"path:<path>"` for a path-transport entry, or
    /// `"git+<url>#path=<p>"` for a git-transport entry — the same shape
    /// [`crate::manifest::ProjectPackageDependency::identity`] produces from
    /// the manifest side, so the two are directly comparable.
    pub fn identity(&self) -> String {
        match self.transport {
            PackageTransport::Npm => self.package.clone(),
            PackageTransport::Path => format!("path:{}", self.path),
            PackageTransport::Git => {
                if self.path.is_empty() {
                    format!("git+{}", self.url)
                } else {
                    format!("git+{}#path={}", self.url, self.path)
                }
            }
        }
    }
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
    /// package root (e.g. `.ctx/traits/config.toml`). Descriptive only: base
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
///
/// A native family package (P530/P531; folded and shared via `path:` since
/// P535, e.g. `.ctx/traits/authored/implement/`) contributes one entry per
/// declared variant, and every variant shares the same canonical `id` —
/// only `variant`/`is_default_variant`/`aliases` and each entry's own
/// `canonical_path` tell them apart. Resolution must therefore never key
/// purely on `id`: see `ResolvedTraitPath` in `ctx-traits-io`'s
/// `project_lock` for the lookup that respects this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitLockEntry {
    /// Trait ID from its canonical document.
    pub id: String,
    /// The native family variant name this entry was published from (e.g.
    /// `"quick"`, `"default"`), or `None` for an ordinary (non-family)
    /// trait. Omitted from output for the common non-family case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// `true` when `variant` is the family's declared default variant — the
    /// entry a bare (variant-less) trait ID reference resolves to.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_default_variant: bool,
    /// Legacy hyphenated package aliases this variant publishes (e.g.
    /// `"implement-quick"`), from the family's `[family.variant.<name>]`
    /// table. Empty for a non-family trait.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
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

#[cfg(test)]
mod package_lock_entry_tests {
    use super::*;

    /// A pre-P535 npm lock entry (no `transport` field at all) must keep
    /// decoding as an npm entry, byte-identically to before.
    #[test]
    fn legacy_npm_entry_without_transport_decodes_as_npm() {
        let toml = r#"alias = "demo"
package = "@scope/demo"
requested = "^1.0.0"
resolved-version = "1.2.0"
integrity = "sha512-abc"
vendored-path = ".ctx/traits/vendored/demo"
tree-digest = "sha256:deadbeef"
"#;
        let entry: PackageLockEntry = toml::from_str(toml).unwrap();
        assert_eq!(entry.transport, PackageTransport::Npm);
        assert_eq!(entry.identity(), "@scope/demo");
        assert_eq!(entry.path, "");
    }

    #[test]
    fn path_entry_round_trips_without_npm_fields() {
        let entry = PackageLockEntry {
            alias: "implement".to_string(),
            transport: PackageTransport::Path,
            path: ".ctx/traits/authored/implement".to_string(),
            vendored_path: ".ctx/traits/vendored/implement".to_string(),
            tree_digest: "sha256:cafebabe".to_string(),
            ..Default::default()
        };
        let text = toml::to_string_pretty(&entry).unwrap();
        assert!(
            !text.contains("integrity") && !text.contains("resolved-version"),
            "path entry must not fabricate npm SRI/registry evidence: {text}"
        );
        let decoded: PackageLockEntry = toml::from_str(&text).unwrap();
        assert_eq!(decoded.transport, PackageTransport::Path);
        assert_eq!(decoded.identity(), "path:.ctx/traits/authored/implement");
        assert_eq!(decoded.path, entry.path);
    }

    #[test]
    fn git_entry_round_trips_without_npm_fields() {
        let entry = PackageLockEntry {
            alias: "refactor".to_string(),
            transport: PackageTransport::Git,
            url: "https://github.com/o/r.git".to_string(),
            requested_ref: "v1.0.0".to_string(),
            resolved_commit: "abc123".to_string(),
            path: "traits/refactor".to_string(),
            vendored_path: ".ctx/traits/vendored/refactor".to_string(),
            tree_digest: "sha256:cafebabe".to_string(),
            ..Default::default()
        };
        let text = toml::to_string_pretty(&entry).unwrap();
        assert!(
            !text.contains("integrity") && !text.contains("resolved-version"),
            "git entry must not fabricate npm SRI/registry evidence: {text}"
        );
        let decoded: PackageLockEntry = toml::from_str(&text).unwrap();
        assert_eq!(decoded.transport, PackageTransport::Git);
        assert_eq!(
            decoded.identity(),
            "git+https://github.com/o/r.git#path=traits/refactor"
        );
        assert_eq!(decoded.url, entry.url);
        assert_eq!(decoded.resolved_commit, entry.resolved_commit);
    }

    /// Guard against lock byte-compat regression: a pre-0191 npm+path lock
    /// document must decode/encode byte-identically now that the git fields
    /// exist, since they are all skip-if-empty.
    #[test]
    fn pre_git_lock_document_stays_byte_identical() {
        let npm = PackageLockEntry {
            alias: "demo".to_string(),
            package: "@scope/demo".to_string(),
            requested: "^1.0.0".to_string(),
            resolved_version: "1.2.0".to_string(),
            integrity: "sha512-abc".to_string(),
            vendored_path: ".ctx/traits/vendored/demo".to_string(),
            tree_digest: "sha256:deadbeef".to_string(),
            ..Default::default()
        };
        let path = PackageLockEntry {
            alias: "implement".to_string(),
            transport: PackageTransport::Path,
            path: ".ctx/traits/authored/implement".to_string(),
            vendored_path: ".ctx/traits/vendored/implement".to_string(),
            tree_digest: "sha256:cafebabe".to_string(),
            ..Default::default()
        };
        let mut lock = ProjectLock::new(Metadata::default());
        lock.upsert_package(npm);
        lock.upsert_package(path);
        lock.sort_for_output();
        let first = lock.to_json().unwrap();
        let decoded: ProjectLock = serde_json::from_str(&first).unwrap();
        let second = decoded.to_json().unwrap();
        assert_eq!(first, second);
        assert!(!first.contains("\"url\""));
        assert!(!first.contains("\"requested-ref\""));
        assert!(!first.contains("\"resolved-commit\""));
    }
}
