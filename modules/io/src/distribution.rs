//! Project-scoped npm trait distribution orchestration (P438).
//!
//! Implements `ctx traits install/remove/update/outdated/info`: stages a
//! verified npm package completely before ever mutating project files, then
//! atomically publishes the vendor tree, updates `.ctx/traits.lock`, and
//! surgically edits `.ctx/traits.toml` (only the `[dependencies]` table).
//!
//! Every mutating operation (`install`/`update`/`remove`) fully prepares its
//! manifest text, lock text, and vendor tree *before* touching any live
//! file, then commits through symlink-safe, rename-based publication with
//! rollback: an ordinary failure partway through commit (a later write
//! failing after an earlier one succeeded) restores every artifact already
//! touched, so this project never observes a manifest/lock/vendor state that
//! didn't exist immediately before the operation started.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::distribution::{self as core_distribution, PackageSpec, VersionSelector};
use ctx_traits_core::manifest::{ProjectManifest, ProjectPackageDependency};
use ctx_traits_core::project_lock::{BaseLockEntry, PackageLockEntry, ProjectLock, TraitLockEntry};
use serde::Serialize;

use crate::dependency::LoadedDependency;

/// P439 distribution tier: project (repo-local — manifest and lock are
/// committed, the vendor tree is generated and gitignored, P446) or global
/// (user-machine-wide, `~/.config/ctx`, no repository at all). Every P438
/// mutation (`install`/
/// `remove`/`update`) and the reconciliation transaction now take a scope
/// instead of a bare `repo_root`, so both tiers share exactly one staging,
/// integrity, rollback, lock, and audit pipeline rather than a second copy
/// for the global tier.
#[derive(Debug, Clone)]
pub enum DistributionScope {
    /// Project-scoped: manifest/lock/vendor live under `repo_root/.ctx`.
    Project(Utf8PathBuf),
    /// Global (per-machine): manifest/lock/vendor live directly under the
    /// `ctx` config-home root (`~/.config/ctx/traits.{toml,lock}`,
    /// `~/.config/ctx/traits/<alias>/`).
    Global(Utf8PathBuf),
}

impl DistributionScope {
    pub fn project(repo_root: &Utf8Path) -> Self {
        Self::Project(repo_root.to_path_buf())
    }

    pub fn global() -> crate::Result<Self> {
        Ok(Self::Global(crate::state::global_ctx_root()?))
    }

    /// Root past which ancestor-symlink checks stop climbing: the repo root
    /// for a project scope, the `ctx` config-home root for global. Also the
    /// directory a scoped caller resolves `[registry] base` config from
    /// (`resolve_registry_options`) — the same root a project/global
    /// `.ctx/config.toml` would live under.
    pub fn boundary(&self) -> &Utf8Path {
        match self {
            Self::Project(root) | Self::Global(root) => root,
        }
    }

    pub fn manifest_path(&self, extension: &str) -> Utf8PathBuf {
        match self {
            Self::Project(root) => crate::layout::project_manifest_path(root, extension),
            Self::Global(root) => root.join(format!("traits.{extension}")),
        }
    }

    pub fn lock_path(&self) -> Utf8PathBuf {
        match self {
            Self::Project(root) => crate::project_lock::project_lock_path(root),
            Self::Global(root) => root.join("traits.lock"),
        }
    }

    /// Vendor root for this scope: `.ctx/traits/vendor` under a project
    /// root, or `traits` directly under the global config root (P439's
    /// contracted `~/.config/ctx/traits/<package>/` layout).
    pub fn vendor_root(&self) -> Utf8PathBuf {
        match self {
            Self::Project(root) => crate::layout::trait_vendor_root_path(root),
            Self::Global(root) => root.join("traits"),
        }
    }

    /// The `vendored-path` string recorded in the lock for `alias` at this
    /// scope, and validated by [`crate::project_lock::resolve_package_lock_paths_in`].
    fn vendored_path_string(&self, alias: &str) -> String {
        match self {
            Self::Project(_) => format!("{}/{alias}", crate::layout::trait_vendor_root()),
            Self::Global(_) => format!("traits/{alias}"),
        }
    }

    pub fn vendored_package_root(&self, alias: &str) -> crate::Result<Utf8PathBuf> {
        crate::layout::validate_trait_id(alias).map_err(crate::Error::from)?;
        Ok(self.vendor_root().join(alias))
    }

    /// `"project"` or `"global"`, recorded in the audit journal so a mutation
    /// can be told apart from its project-scoped sibling.
    pub fn audit_scope(&self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Global(_) => "global",
        }
    }

    fn read_manifest(&self) -> crate::Result<ctx_traits_core::manifest::ProjectManifest> {
        let manifest_path = self.manifest_path("toml");
        let text = std::fs::read_to_string(&manifest_path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: manifest_path.to_string(),
                source,
            }
        })?;
        toml::from_str(&text).map_err(|source| {
            crate::parse::Error::TomlDecode {
                context: format!("decode {manifest_path}"),
                source,
            }
            .into()
        })
    }

    fn read_lock(&self) -> crate::Result<Option<ProjectLock>> {
        let lock_path = self.lock_path();
        crate::project_lock::read_lock_at(&lock_path, self.boundary(), |entry| {
            self.resolve_lock_paths(entry)
        })
    }

    /// Validate and resolve one package-lock entry's expected vendor root
    /// and per-trait manifest paths at this scope. `vendored_package_root`
    /// validates the alias (rejects `..`, separators, etc.) before joining
    /// it onto the vendor root, so a forged lock entry can never derive an
    /// expected root outside this scope's vendor tree — matching the
    /// project-scope guard `vendored_dependency_root` already provides.
    fn resolve_lock_paths(
        &self,
        entry: &PackageLockEntry,
    ) -> crate::Result<crate::project_lock::ResolvedPackageLockPaths> {
        let entry_root = self.vendored_package_root(&entry.alias)?;
        crate::project_lock::resolve_package_lock_paths_in(
            &entry_root,
            &self.vendored_path_string(&entry.alias),
            entry,
        )
    }
}

/// One discovered-and-verified trait inside a staged npm package.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct StagedTraitReport {
    pub id: String,
    pub canonical_path: String,
    pub canonical_digest: String,
    pub schema_version: String,
}

/// Result of a successful `install`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallReport {
    pub alias: String,
    pub package: String,
    pub requested: String,
    pub resolved_version: String,
    pub integrity: String,
    pub vendored_path: String,
    pub traits: Vec<StagedTraitReport>,
    /// `true` when this package was merged in from a resolved `extends`
    /// base rather than a local `[dependencies]` declaration (P443).
    pub inherited: bool,
    pub claim: String,
    pub review_hint: String,
}

/// Result of a successful `remove`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RemoveReport {
    pub alias: String,
    pub package: String,
}

/// One `outdated` row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct OutdatedRow {
    pub alias: String,
    pub package: String,
    pub current: String,
    pub wanted: String,
    pub latest: String,
}

/// `info` inspection report. Read-only: never mutates manifest, lock,
/// vendor, or trust state. Downloaded bytes may only enter the registry
/// cache during inspection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InfoReport {
    pub package: String,
    pub resolved_version: String,
    pub claim: String,
    pub traits: Vec<TraitInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitInfo {
    pub id: String,
    pub canonical_path: String,
    pub canonical_digest: String,
    pub schema_version: String,
    pub commands: Vec<core_distribution::CommandSource>,
    pub resource_roots: Vec<String>,
    pub agent_roles: Vec<String>,
}

const CLAIM_ABSENT: &str = "absent (unclaimed; digests below are computed, not publisher-asserted)";
const CLAIM_VERIFIED: &str = "verified (publisher ctx.digests claim matches computed digests)";

/// Registry base URL override plumbing. `None` uses
/// [`crate::registry::DEFAULT_REGISTRY_BASE`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RegistryOptions<'a> {
    pub base_url: Option<&'a str>,
}

impl<'a> RegistryOptions<'a> {
    fn base(&self) -> &str {
        self.base_url
            .unwrap_or(crate::registry::DEFAULT_REGISTRY_BASE)
    }
}

/// Which layer produced [`ResolvedRegistryBase::base`] (P492) — the single
/// piece of provenance both a real registry call (`resolve_registry_options`)
/// and a diagnostic surface (`doctor --config`'s `registry.base` row) need,
/// so neither has to re-derive it by re-reading `CTX_TRAITS_REGISTRY_BASE`
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryBaseSource {
    /// `CTX_TRAITS_REGISTRY_BASE`, non-empty.
    EnvOverride,
    /// The documented `[registry] base` config knob, non-empty.
    Config,
    /// Neither set (or set to an empty string, which is treated identically
    /// to absent at every layer — one emptiness rule, not a rule per layer).
    Default,
}

/// The effective registry base URL for `start_dir`, with the layer that
/// produced it.
#[derive(Debug, Clone)]
pub struct ResolvedRegistryBase {
    pub base: String,
    pub source: RegistryBaseSource,
}

/// Resolve the effective registry base URL and its source layer for
/// `start_dir` (P492) — the one place the `CTX_TRAITS_REGISTRY_BASE` >
/// `[registry] base` > [`crate::registry::DEFAULT_REGISTRY_BASE`] precedence
/// is encoded. `CTX_TRAITS_REGISTRY_BASE` is also how the deterministic
/// local-registry proof in `scripts/byte_compare.rs`, driven by `just test`,
/// points a real `ctx` invocation at an in-process fixture instead of the
/// public npm registry, with no `node`/`npm`/`pnpm` process and no network
/// dependency. Env-on-top matches `ConfigLayer::Environment` sitting above
/// `Repo` everywhere else in this crate.
pub fn resolve_registry_base_with_source(start_dir: &Utf8Path) -> ResolvedRegistryBase {
    if let Ok(base) = std::env::var("CTX_TRAITS_REGISTRY_BASE") {
        if !base.is_empty() {
            return ResolvedRegistryBase {
                base,
                source: RegistryBaseSource::EnvOverride,
            };
        }
    }
    if let Some(base) = crate::harness_config::resolve_registry_base(start_dir) {
        if !base.is_empty() {
            return ResolvedRegistryBase {
                base,
                source: RegistryBaseSource::Config,
            };
        }
    }
    ResolvedRegistryBase {
        base: crate::registry::DEFAULT_REGISTRY_BASE.to_string(),
        source: RegistryBaseSource::Default,
    }
}

/// Resolve the effective registry base URL override for `start_dir` as
/// [`RegistryOptions`], for callers that need only the value (every real
/// registry call site) and not its provenance — see
/// [`resolve_registry_base_with_source`] for the precedence this applies.
pub fn resolve_registry_options(start_dir: &Utf8Path) -> RegistryOptions<'static> {
    let resolved = resolve_registry_base_with_source(start_dir);
    match resolved.source {
        RegistryBaseSource::Default => RegistryOptions::default(),
        RegistryBaseSource::EnvOverride | RegistryBaseSource::Config => RegistryOptions {
            base_url: Some(Box::leak(resolved.base.into_boxed_str())),
        },
    }
}

/// Install (or re-resolve, for `update`) one npm package into the project.
///
/// Stages the tarball fully (fetch, integrity, extraction, trait discovery,
/// digest computation, claim verification, schema check) before ever
/// touching `.ctx/traits.toml`, `.ctx/traits.lock`, or the vendor tree. A
/// failed staging step leaves every project file untouched.
pub fn install(
    scope: &DistributionScope,
    spec_input: &str,
    alias_override: Option<&str>,
    registry: RegistryOptions<'_>,
) -> crate::Result<InstallReport> {
    install_internal(scope, spec_input, alias_override, false, false, registry)
}

/// Shared install/re-resolve implementation behind the public [`install`]
/// (always a local `[dependencies]` declaration) and P443's inherited-
/// package installs driven by an `extends` base merge, which must publish
/// the same verified lock/vendor evidence without ever writing the local
/// manifest's `[dependencies]` table.
///
/// `allow_ownership_transition` governs only the lock-level alias check: a
/// direct, user-issued `ctx traits install --alias X` (`false`) must still
/// refuse to silently repurpose an alias already locked to a different
/// package. Reconciliation call sites (`reconcile_project_dependencies`,
/// `update_base_and_inherited`) pass `true`, because they have already
/// computed `alias`'s package from the effective base+local manifest merge
/// — the same alias legitimately moving between an inherited package, a
/// local override, and back again (or a base update changing what package
/// an inherited alias points to) is the exact behavior P443 exists to
/// support, not a conflict to refuse.
fn install_internal(
    scope: &DistributionScope,
    spec_input: &str,
    alias_override: Option<&str>,
    inherited: bool,
    allow_ownership_transition: bool,
    registry: RegistryOptions<'_>,
) -> crate::Result<InstallReport> {
    let spec = core_distribution::parse_spec(spec_input).map_err(ctx_traits_core::Error::from)?;
    let alias = alias_override.unwrap_or(&spec.default_alias).to_string();

    // Reject an alias collision before any remote fetch or cache mutation:
    // with the registry offline or hostile, this must still fail with the
    // deterministic local alias-conflict error rather than a staging error.
    // Manifest-level only (never bypassed): the local `[dependencies]` table
    // already agrees with whatever this call is about to install whenever
    // the caller derived `spec_input`/`alias` from that same table, so this
    // never spuriously fires for a legitimate reconciliation call either.
    reject_alias_collision(scope, &alias, &spec.package.full())?;

    let staged = stage_npm_package(&spec, registry)?;
    let requested = match &spec.selector {
        VersionSelector::Latest => core_distribution::caret_range(&staged.resolved_version),
        other => other.as_str().to_string(),
    };

    publish_staged_package(
        scope,
        &alias,
        &spec.package.full(),
        &requested,
        &staged,
        PackageOwnership {
            inherited,
            allow_transition: allow_ownership_transition,
        },
        Some(crate::audit_journal::AuditAction::Install),
    )?;

    let claim_label = match staged.claim_verdict {
        core_distribution::ClaimVerification::Absent => CLAIM_ABSENT,
        core_distribution::ClaimVerification::Verified => CLAIM_VERIFIED,
    };

    let traits = staged
        .traits
        .iter()
        .map(|t| StagedTraitReport {
            id: t.id.clone(),
            canonical_path: t.canonical_path.clone(),
            canonical_digest: t.canonical_digest.clone(),
            schema_version: t.schema_version.clone(),
        })
        .collect::<Vec<_>>();

    Ok(InstallReport {
        alias: alias.clone(),
        package: spec.package.full(),
        requested,
        resolved_version: staged.resolved_version,
        integrity: staged.integrity,
        vendored_path: scope.vendor_root().join(&alias).to_string(),
        traits,
        inherited,
        claim: claim_label.to_string(),
        review_hint: "run `ctx traits trust approve <trait>` for each canonical digest above before running it".to_string(),
    })
}

/// Resolve a user-supplied `remove`/`update` operand against the project
/// manifest by either its alias (vendor-directory key) or its exact npm
/// package name, requiring exactly one unambiguous match. Aliases remain
/// accepted as convenience input, but the contracted operand is the
/// user-facing npm package identity recorded in the manifest/lock, so
/// `ctx traits remove @scope/name` must work after `ctx traits install
/// @scope/name` even when the alias differs.
fn resolve_installed_operand<'a>(
    manifest: &'a ctx_traits_core::manifest::ProjectManifest,
    operand: &str,
) -> crate::Result<&'a str> {
    if manifest.packages.contains_key(operand) {
        return Ok(manifest
            .packages
            .keys()
            .find(|alias| alias.as_str() == operand)
            .expect("just checked contains_key"));
    }
    let by_package: Vec<&str> = manifest
        .packages
        .iter()
        .filter(|(_, dependency)| dependency.npm == operand)
        .map(|(alias, _)| alias.as_str())
        .collect();
    match by_package.as_slice() {
        [alias] => Ok(alias),
        [] => Err(crate::environment::Error::Filesystem {
            path: operand.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no installed dependency matches alias or npm package {operand:?}"),
            ),
        }
        .into()),
        _ => Err(crate::environment::Error::Filesystem {
            path: operand.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{operand:?} matches multiple installed dependencies ({}); use the manifest alias to disambiguate",
                    by_package.join(", ")
                ),
            ),
        }
        .into()),
    }
}

/// Remove one project-installed npm package: manifest entry, project-lock
/// entry, and vendor directory. `operand` may be either the manifest alias
/// or the exact npm package name.
pub fn remove(scope: &DistributionScope, operand: &str) -> crate::Result<RemoveReport> {
    let manifest = scope.read_manifest()?;
    let alias = resolve_installed_operand(&manifest, operand)?.to_string();
    let package = manifest.packages[&alias].npm.clone();

    let manifest_path = scope.manifest_path("toml");
    assert_no_symlink_ancestors(&manifest_path, scope.boundary())?;
    let manifest_snapshot = FileSnapshot::capture(&manifest_path)?;
    let text = manifest_snapshot.previous.clone().ok_or_else(|| {
        crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no project manifest"),
        }
    })?;
    let mut document = text.parse::<toml_edit::DocumentMut>().map_err(|source| {
        crate::parse::Error::TomlEditDecode {
            context: format!("parse {manifest_path} for remove"),
            source: Box::new(source),
        }
    })?;
    if let Some(deps) = document
        .get_mut("dependencies")
        .and_then(|d| d.as_table_like_mut())
    {
        deps.remove(&alias);
    }
    let manifest_text = document.to_string();

    let lock_path = scope.lock_path();
    assert_no_symlink_ancestors(&lock_path, scope.boundary())?;
    let lock_snapshot = FileSnapshot::capture(&lock_path)?;
    let mut lock = scope.read_lock()?.unwrap_or_default();
    let removed_entry = lock.remove_package(&alias);
    let lock_text = encode_project_lock(&mut lock)?;
    let (requested_selector, resolved_version, affected_trait_digests) = removed_entry
        .map(|entry| {
            let digests = entry
                .traits
                .iter()
                .map(|t| t.canonical_digest.clone())
                .collect::<Vec<_>>();
            (entry.requested, entry.resolved_version, digests)
        })
        .unwrap_or_default();

    let vendor_root = scope.vendor_root().join(&alias);
    assert_no_symlink_ancestors(&vendor_root, scope.boundary())?;
    let vendor_backup = backup_path(&vendor_root, "remove");
    let had_vendor = vendor_root.exists();
    if had_vendor {
        std::fs::rename(&vendor_root, &vendor_backup).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: vendor_root.to_string(),
                source,
            }
        })?;
    }

    if let Err(err) = atomic_write(&lock_path, &lock_text) {
        let mut notes = Vec::new();
        if had_vendor {
            notes.extend(try_restore_vendor_backup(&vendor_backup, &vendor_root));
        }
        return Err(with_notes(err, notes));
    }
    if let Err(err) = atomic_write(&manifest_path, &manifest_text) {
        let mut notes = Vec::new();
        notes.extend(lock_snapshot.restore());
        if had_vendor {
            notes.extend(try_restore_vendor_backup(&vendor_backup, &vendor_root));
        }
        return Err(with_notes(err, notes));
    }

    // As with install, the audit record is part of the commit boundary: an
    // ordinary failure appending it restores manifest, lock, and vendor
    // rather than leaving a removal committed with no audit evidence.
    if let Err(err) = append_audit(
        crate::audit_journal::AuditAction::Remove,
        &package,
        &requested_selector,
        &resolved_version,
        &affected_trait_digests,
        scope,
    ) {
        let mut notes = Vec::new();
        notes.extend(manifest_snapshot.restore());
        notes.extend(lock_snapshot.restore());
        if had_vendor {
            notes.extend(try_restore_vendor_backup(&vendor_backup, &vendor_root));
        }
        return Err(with_notes(err, notes));
    }

    if had_vendor {
        let _ = std::fs::remove_dir_all(&vendor_backup);
    }

    Ok(RemoveReport { alias, package })
}

/// Re-resolve one (or, when `operand` is `None`, every) manifest package
/// selector and replace its lock/vendor evidence. `operand` may be either
/// the manifest alias or the exact npm package name.
pub fn update(
    scope: &DistributionScope,
    operand: Option<&str>,
    registry: RegistryOptions<'_>,
) -> crate::Result<Vec<InstallReport>> {
    let manifest = scope.read_manifest()?;
    if operand.is_none() {
        if let Some(extends) = manifest.extends.clone() {
            // `extends` is project-scoped only (P443 SCOPE explicitly
            // excludes global-tier inheritance): refuse before any
            // registry fetch, lock write, or vendor mutation rather than
            // silently resolving and persisting base evidence into the
            // global tier.
            if !matches!(scope, DistributionScope::Project(_)) {
                return Err(crate::environment::Error::Filesystem {
                    path: scope.manifest_path("toml").to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "extends is project-scoped only; a global manifest (ctx traits update -g) may not declare extends",
                    ),
                }
                .into());
            }
            // Explicit update with no operand is the one moment this
            // project may re-resolve its `extends` selector (P443): ordinary
            // sync always replays the exact locked base version, so a newer
            // compatible base only ever moves here.
            return update_base_and_inherited(scope, &manifest, &extends, registry);
        }
    }
    let selected_alias = operand
        .map(|operand| resolve_installed_operand(&manifest, operand))
        .transpose()?
        .map(str::to_string);
    let mut reports = Vec::new();
    for (entry_alias, dependency) in &manifest.packages {
        if let Some(alias) = &selected_alias {
            if alias != entry_alias {
                continue;
            }
        }
        let spec_input = format!("{}@{}", dependency.npm, dependency.version);
        let report = install(scope, &spec_input, Some(entry_alias), registry)?;
        reports.push(report);
    }
    Ok(reports)
}

/// Re-resolve the `extends` base selector fresh, merge its (possibly
/// changed) `[dependencies]` under this project's local declarations, and
/// publish the complete resulting effective package set: new/changed
/// inherited packages install or re-resolve, unchanged ones are left byte-
/// identical (their alias resolves to the same requested/resolved-version
/// pair `install_internal`'s own alias-collision/lock-compatibility checks
/// already treat as a no-op republish), and inherited packages the new base
/// no longer declares — and that this project's own manifest does not
/// separately declare — are pruned. Local declarations are never touched.
fn update_base_and_inherited(
    scope: &DistributionScope,
    manifest: &ProjectManifest,
    extends: &str,
    registry: RegistryOptions<'_>,
) -> crate::Result<Vec<InstallReport>> {
    let resolved_base = resolve_base(extends, registry)?;
    let (effective, inherited_aliases) =
        merge_effective_packages(Some(&resolved_base.packages), &manifest.packages);

    let mut reports = Vec::new();
    for (alias, dependency) in &effective {
        let spec_input = format!("{}@{}", dependency.npm, dependency.version);
        let report = install_internal(
            scope,
            &spec_input,
            Some(alias),
            inherited_aliases.contains(alias),
            true,
            registry,
        )?;
        reports.push(report);
    }

    // Keyed off `effective` (see the matching comment in
    // `reconcile_project_dependencies`), not `inherited_aliases`: read after
    // the install loop above, so every alias still in `effective` already
    // carries its freshly republished `inherited` flag here regardless.
    let previously_inherited: Vec<String> = scope
        .read_lock()?
        .unwrap_or_default()
        .packages
        .iter()
        .filter(|entry| entry.inherited && !effective.contains_key(&entry.alias))
        .map(|entry| entry.alias.clone())
        .collect();
    for alias in previously_inherited {
        remove_stale_inherited_package(scope, &alias)?;
    }

    write_base_lock(
        scope,
        Some(BaseLockEntry {
            extends: extends.to_string(),
            package: resolved_base.package,
            resolved_version: resolved_base.resolved_version,
            integrity: resolved_base.integrity,
            manifest_path: resolved_base.manifest_relative_path,
            manifest_digest: resolved_base.manifest_digest,
        }),
    )?;

    Ok(reports)
}

/// Report locked, wanted (highest satisfying the manifest selector), and
/// registry-latest versions for every project package.
pub fn outdated(
    repo_root: &Utf8Path,
    registry: RegistryOptions<'_>,
) -> crate::Result<Vec<OutdatedRow>> {
    let manifest = read_project_manifest(repo_root)?;
    let lock = crate::project_lock::read_project_lock(repo_root)?;
    let mut rows = Vec::new();
    for (alias, dependency) in &manifest.packages {
        let current = lock
            .as_ref()
            .and_then(|lock| lock.package_entry(alias))
            .map(|entry| entry.resolved_version.clone())
            .unwrap_or_else(|| "unlocked".to_string());
        let metadata = crate::registry::fetch_metadata(registry.base(), &dependency.npm)?;
        let versions = metadata.version_list();
        let selector = parse_selector(&dependency.version)?;
        let wanted = core_distribution::resolve_version(
            &dependency.npm,
            &versions,
            &metadata.dist_tags,
            &selector,
        )
        .map_err(ctx_traits_core::Error::from)?;
        let latest = metadata
            .dist_tags
            .get("latest")
            .cloned()
            .unwrap_or_else(|| wanted.clone());
        rows.push(OutdatedRow {
            alias: alias.clone(),
            package: dependency.npm.clone(),
            current,
            wanted,
            latest,
        });
    }
    Ok(rows)
}

/// Inspect a package's metadata, claims, canonical digests, and capability
/// surface without modifying any project state. Downloaded bytes only ever
/// land in the registry cache.
pub fn info(
    repo_root: &Utf8Path,
    spec_input: &str,
    registry: RegistryOptions<'_>,
) -> crate::Result<InfoReport> {
    let _ = repo_root;
    let spec = core_distribution::parse_spec(spec_input).map_err(ctx_traits_core::Error::from)?;
    let staged = stage_npm_package(&spec, registry)?;
    let claim_label = match staged.claim_verdict {
        core_distribution::ClaimVerification::Absent => CLAIM_ABSENT,
        core_distribution::ClaimVerification::Verified => CLAIM_VERIFIED,
    };
    let traits = staged
        .traits
        .iter()
        .map(|t| TraitInfo {
            id: t.id.clone(),
            canonical_path: t.canonical_path.clone(),
            canonical_digest: t.canonical_digest.clone(),
            schema_version: t.schema_version.clone(),
            commands: t.capabilities.commands.clone(),
            resource_roots: t.capabilities.resource_roots.clone(),
            agent_roles: t.capabilities.agent_roles.clone(),
        })
        .collect();
    Ok(InfoReport {
        package: spec.package.full(),
        resolved_version: staged.resolved_version,
        claim: claim_label.to_string(),
        traits,
    })
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

pub(crate) struct StagedTrait {
    pub(crate) id: String,
    pub(crate) canonical_path: String,
    /// The discovered trait package's own root, relative to the staging
    /// root (e.g. `packages/foo`, or empty for a package rooted at the
    /// staging root). Used to select an exact trait by package identity
    /// rather than by prefix-matching `canonical_path`.
    pub(crate) package_root: String,
    schema_version: String,
    source_digest: String,
    canonical_digest: String,
    model_visible_digest: String,
    resource_manifest_digest: String,
    capabilities: core_distribution::CapabilitySurface,
}

pub(crate) struct StagedPackage {
    pub(crate) resolved_version: String,
    integrity: String,
    pub(crate) staging_root: Utf8PathBuf,
    pub(crate) traits: Vec<StagedTrait>,
    claim_verdict: core_distribution::ClaimVerification,
}

/// A local package and its canonical trait, shared by install-adjacent
/// inspection and publication. The root `trait.toml` is decoded only as the
/// package manifest; the canonical trait is resolved independently.
#[derive(Debug)]
pub struct LocalTraitPackage {
    pub root: Utf8PathBuf,
    pub manifest_path: Utf8PathBuf,
    pub package_manifest: Option<ctx_traits_core::manifest::PackageManifest>,
    pub loaded: LoadedDependency,
}

#[derive(Debug)]
pub struct LocalPackageInspection {
    pub packages: Vec<LocalTraitPackage>,
    pub required_paths: Vec<String>,
    pub canonical_digests: std::collections::BTreeMap<String, String>,
    /// Directories the default pack exclude set dropped from the walk,
    /// carried as evidence for the publish report — see
    /// [`crate::publish::PACK_DEFAULT_EXCLUDES`].
    pub excluded: Vec<core_distribution::SkippedPath>,
}

/// Decode only the package-root manifest. Callers that need lifecycle status
/// can use this before loading the canonical trait document.
pub fn read_package_manifest(
    root: &Utf8Path,
) -> crate::Result<Option<ctx_traits_core::manifest::PackageManifest>> {
    let path = crate::layout::package_manifest_path(root);
    if !path.is_file() {
        return Ok(None);
    }
    let text = crate::read::read_text(&path)?;
    Ok(ctx_traits_core::manifest::decode_package_manifest(
        &text,
        path.as_str(),
    )?)
}

/// Discover and load one or more authored trait packages under a local npm
/// root. This is deliberately the same loader used by registry installation.
pub fn inspect_local_package(root: &Utf8Path) -> crate::Result<LocalPackageInspection> {
    let discovered = crate::registry::discover_trait_packages(root)?;
    let mut packages = Vec::new();
    for discovered_package in discovered {
        let package_manifest = read_package_manifest(&discovered_package.absolute_root)?;
        let Some(manifest_path) = canonical_manifest(
            &discovered_package.absolute_root,
            package_manifest.is_some(),
        ) else {
            continue;
        };
        let loaded = crate::dependency::load_dependency_package(
            discovered_package.relative_root.as_str().trim_matches('/'),
            package_manifest
                .as_ref()
                .map(|manifest| manifest.package.id.as_str()),
            package_manifest
                .as_ref()
                .map(|manifest| manifest.package.version.as_str()),
            &manifest_path,
        )?;
        packages.push(LocalTraitPackage {
            root: discovered_package.absolute_root,
            manifest_path,
            package_manifest,
            loaded,
        });
    }
    if packages.is_empty() {
        if let Some(package_manifest) = read_package_manifest(root)? {
            if let Some(manifest_path) = canonical_manifest(root, true) {
                let loaded = crate::dependency::load_dependency_package(
                    "self",
                    Some(package_manifest.package.id.as_str()),
                    Some(package_manifest.package.version.as_str()),
                    &manifest_path,
                )?;
                packages.push(LocalTraitPackage {
                    root: root.to_path_buf(),
                    manifest_path,
                    package_manifest: Some(package_manifest),
                    loaded,
                });
            }
        }
    }
    packages.sort_by(|left, right| left.root.cmp(&right.root));
    let excludes = crate::harness_config::resolve_pack_excludes(root);
    let mut required_paths = Vec::new();
    let mut excluded = Vec::new();
    for package in &packages {
        collect_package_files(
            root,
            &package.root,
            &excludes,
            &mut required_paths,
            &mut excluded,
        )?;
    }
    required_paths.sort();
    required_paths.dedup();
    excluded.sort_by(|a, b| a.path.cmp(&b.path));
    excluded.dedup_by(|a, b| a.path == b.path);
    // Refuse loudly rather than silently ship an incomplete tarball: only a
    // path a loaded package's canonical document actually declares (a
    // resource `path`) triggers this — an ordinary build `target/` with no
    // declared resource inside it is unaffected and still publishes cleanly.
    for package in &packages {
        for resource in &package.loaded.trait_ref.resources {
            let Some(path) = resource.path.as_deref() else {
                continue;
            };
            if !matches!(
                resource.root,
                ctx_traits_core::r#trait::resource::ResourceRoot::Package
            ) {
                continue;
            }
            let absolute = package.root.join(path);
            let relative = absolute.strip_prefix(root).unwrap_or(&absolute).as_str();
            if let Some(rule) = excluded
                .iter()
                .find(|skip| {
                    relative == skip.path || relative.starts_with(&format!("{}/", skip.path))
                })
                .map(|skip| skip.rule.clone())
            {
                return Err(crate::publish::Error::Unsafe {
                    path: relative.to_string(),
                    message: format!(
                        "declared resource {resource_id:?} is under the default pack exclude \
                         rule {rule:?}; it would be silently omitted from the tarball — declare \
                         [publish] exclude to override the default excludes if this path must \
                         ship",
                        resource_id = resource.id,
                    ),
                }
                .into());
            }
        }
    }
    let canonical_digests = packages
        .iter()
        .map(|package| {
            (
                package
                    .manifest_path
                    .strip_prefix(root)
                    .unwrap_or(&package.manifest_path)
                    .to_string(),
                package.loaded.canonical_digest.to_string(),
            )
        })
        .collect();
    Ok(LocalPackageInspection {
        packages,
        required_paths,
        canonical_digests,
        excluded,
    })
}

fn canonical_manifest(root: &Utf8Path, has_package_manifest: bool) -> Option<Utf8PathBuf> {
    crate::layout::package_manifest_read_candidates(root)
        .into_iter()
        .find(|candidate| {
            candidate.is_file()
                && !(has_package_manifest
                    && candidate == &crate::layout::package_manifest_path(root))
        })
}

fn collect_package_files(
    npm_root: &Utf8Path,
    package_root: &Utf8Path,
    excludes: &[String],
    paths: &mut Vec<String>,
    excluded: &mut Vec<core_distribution::SkippedPath>,
) -> crate::Result<()> {
    let entries = std::fs::read_dir(package_root).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: package_root.to_string(),
            source,
        }
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: package_root.to_string(),
            source,
        })?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| {
            crate::environment::Error::Filesystem {
                path: package_root.to_string(),
                source: std::io::Error::other("non-UTF-8 publish path"),
            }
        })?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(crate::publish::Error::Unsafe {
                path: path.to_string(),
                message: "symlinks are not publishable".to_string(),
            }
            .into());
        }
        if metadata.is_dir() {
            let name = path.file_name().unwrap_or_default();
            if crate::publish::is_pack_excluded(name, excludes) {
                excluded.push(core_distribution::SkippedPath {
                    path: path.strip_prefix(npm_root).unwrap_or(&path).to_string(),
                    rule: name.to_string(),
                });
                continue;
            }
            collect_package_files(npm_root, &path, excludes, paths, excluded)?;
        } else if metadata.is_file() {
            paths.push(path.strip_prefix(npm_root).unwrap_or(&path).to_string());
        } else {
            return Err(crate::publish::Error::Unsafe {
                path: path.to_string(),
                message: "special files are not publishable".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

/// Stage one npm package: resolve, download, verify, and extract its exact
/// version (via the single shared, integrity-checked registry cache in
/// [`crate::registry::fetch_and_extract_version`]), discover every trait
/// package inside its (possibly multi-trait) dual-use tarball, load and
/// digest each, and verify the optional `ctx.digests` publisher claim
/// against the full discovered set. The only staging pipeline in this
/// crate: both project installs (`ctx traits install`) and package-local
/// `source.package` npm dependencies ([`crate::dependency`]) call this, so
/// neither can vendor a package whose publisher claim, schema version, or
/// integrity the other would have refused.
pub(crate) fn stage_npm_package(
    spec: &PackageSpec,
    registry: RegistryOptions<'_>,
) -> crate::Result<StagedPackage> {
    let package = spec.package.full();
    let cache_root = registry_cache_root()?;
    let fetched = crate::registry::fetch_and_extract_version(
        registry.base(),
        &package,
        &spec.selector,
        &cache_root,
    )?;
    let resolved_version = fetched.resolved_version;
    let integrity = fetched.integrity;
    let staging_root = fetched.root;

    let inspection = inspect_local_package(&staging_root)?;
    if inspection.packages.is_empty() {
        return Err(crate::registry::Error::NoTraitPackageFound {
            package: package.clone(),
            version: resolved_version.clone(),
        }
        .into());
    }

    let claim = crate::registry::load_publisher_claim(&staging_root)?;

    let mut traits = Vec::new();
    let mut computed_digests = std::collections::BTreeMap::new();
    for inspected_package in &inspection.packages {
        let manifest_path = &inspected_package.manifest_path;
        let loaded = &inspected_package.loaded;
        if !ctx_traits_core::r#trait::is_schema_version_supported(
            loaded.trait_ref.schema_version.as_str(),
        ) {
            return Err(crate::environment::Error::Filesystem {
                path: manifest_path.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "trait {} declares schema-version {}, which this binary does not support; upgrade ctx",
                        loaded.id,
                        loaded.trait_ref.schema_version.as_str()
                    ),
                ),
            }
            .into());
        }
        let canonical_path = manifest_path
            .strip_prefix(&staging_root)
            .unwrap_or(manifest_path)
            .to_string();
        computed_digests.insert(
            canonical_path.clone(),
            loaded.canonical_digest.as_str().to_string(),
        );
        let capabilities = core_distribution::project_capability_surface(&loaded.trait_ref)?;
        traits.push(StagedTrait {
            id: loaded.id.clone(),
            canonical_path,
            package_root: inspected_package
                .root
                .strip_prefix(&staging_root)
                .unwrap_or(&inspected_package.root)
                .as_str()
                .trim_matches('/')
                .to_string(),
            schema_version: loaded.trait_ref.schema_version.as_str().to_string(),
            source_digest: loaded.source_digest.as_str().to_string(),
            canonical_digest: loaded.canonical_digest.as_str().to_string(),
            model_visible_digest: loaded.model_visible_digest.as_str().to_string(),
            resource_manifest_digest: loaded.resource_manifest_digest.as_str().to_string(),
            capabilities,
        });
    }

    let claim_verdict =
        core_distribution::verify_publisher_claim(claim.as_ref(), &computed_digests)
            .map_err(ctx_traits_core::Error::from)?;

    Ok(StagedPackage {
        resolved_version,
        integrity,
        staging_root,
        traits,
        claim_verdict,
    })
}

/// Deterministic, repo-keyed root for the single shared registry cache
/// ([`crate::registry::REGISTRY_CACHE_SUBDIR`]), consumed by both project
/// installs and package-local npm dependency resolution
/// ([`crate::dependency::resolve_source`]) so neither maintains a second
/// cache family.
pub(crate) fn registry_cache_root() -> crate::Result<Utf8PathBuf> {
    let key = crate::state::current_repo_key().unwrap_or_else(|_| "no-repo".to_string());
    Ok(crate::state::global_cache_root(&key)?.join(crate::registry::REGISTRY_CACHE_SUBDIR))
}

// ---------------------------------------------------------------------------
// `extends` base resolution (P443)
// ---------------------------------------------------------------------------

/// A fully staged, decoded, and verified `extends` base: its exact registry
/// evidence plus the project-package (`[dependencies]`) table its published
/// manifest declares, ready to merge under this project's local
/// declarations.
struct ResolvedBase {
    package: String,
    resolved_version: String,
    integrity: String,
    manifest_relative_path: String,
    manifest_digest: String,
    packages: BTreeMap<String, ProjectPackageDependency>,
}

/// Locate the one project-manifest payload an `extends` base package
/// publishes, at the same repo-relative `.ctx/traits.*` location its own
/// `ctx traits init` would have created it — the same encoding-priority scan
/// as [`crate::discovery::manifest`], rooted at a staged base package
/// instead of a repository.
fn discover_base_manifest(staging_root: &Utf8Path) -> crate::Result<(Utf8PathBuf, String)> {
    let mut found = Vec::new();
    for extension in ["toml", "json", "yaml", "yml"] {
        let path = crate::layout::project_manifest_path(staging_root, extension);
        if path.is_file() {
            found.push(path);
        }
    }
    match found.len() {
        0 => Err(crate::environment::Error::Filesystem {
            path: staging_root.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "extends base package does not publish a .ctx/traits.{toml,json,yaml,yml} project manifest",
            ),
        }
        .into()),
        1 => {
            let path = found.remove(0);
            let relative = path.strip_prefix(staging_root).unwrap_or(&path).to_string();
            Ok((path, relative))
        }
        _ => Err(crate::environment::Error::Filesystem {
            path: staging_root.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "extends base package publishes multiple project manifest encodings",
            ),
        }
        .into()),
    }
}

/// Stage one `extends` base package version through the same verified
/// registry cache every other resolution in this module uses, decode its
/// published project manifest, and refuse a base that itself declares
/// `extends`: P443 is depth-one inheritance only, and this refusal is
/// checked before any of *this* project's own lock/manifest/vendor state is
/// touched by any caller.
fn resolve_base(spec_input: &str, registry: RegistryOptions<'_>) -> crate::Result<ResolvedBase> {
    let spec = core_distribution::parse_spec(spec_input).map_err(ctx_traits_core::Error::from)?;
    let cache_root = registry_cache_root()?;
    let fetched = crate::registry::fetch_and_extract_version(
        registry.base(),
        &spec.package.full(),
        &spec.selector,
        &cache_root,
    )?;
    let (manifest_path, manifest_relative_path) = discover_base_manifest(&fetched.root)?;
    let text = crate::read::read_text(&manifest_path)?;
    let encoding = ctx_traits_core::encoding::Encoding::from_path(&manifest_path)?;
    let manifest = ctx_traits_core::encoding::decode_manifest(encoding, &text)?;
    if manifest.extends.is_some() {
        return Err(crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "extends base {} itself declares extends; nested extends is refused (P443 is depth-one only)",
                    spec.package.full()
                ),
            ),
        }
        .into());
    }
    let manifest_digest = ctx_traits_core::digest::canonical_digest(&manifest)?;
    Ok(ResolvedBase {
        package: spec.package.full(),
        resolved_version: fetched.resolved_version,
        integrity: fetched.integrity,
        manifest_relative_path,
        manifest_digest: manifest_digest.as_str().to_string(),
        packages: manifest.packages,
    })
}

/// Republish the exact locked `extends` base version — never a fresh range
/// resolution — and assert the freshly staged evidence still matches the
/// lock exactly: resolved version, tarball integrity, and decoded manifest
/// digest. Mirrors [`replay_locked_package`]'s exact-version replay
/// contract, so an ordinary sync's base resolution is exactly as
/// lock-authoritative as its package resolution.
fn replay_locked_base(
    base: &BaseLockEntry,
    registry: RegistryOptions<'_>,
) -> crate::Result<ResolvedBase> {
    let spec_input = core_distribution::exact_version_spec(&base.package, &base.resolved_version);
    let resolved = resolve_base(&spec_input, registry)?;
    if resolved.resolved_version != base.resolved_version {
        return Err(crate::environment::Error::Filesystem {
            path: base.package.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "locked extends base version {} for {} did not resolve exactly (registry returned {}); it may have been unpublished",
                    base.resolved_version, base.package, resolved.resolved_version
                ),
            ),
        }
        .into());
    }
    if resolved.integrity != base.integrity {
        return Err(crate::environment::Error::Filesystem {
            path: base.package.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "locked integrity for extends base {}@{} does not match the registry's current tarball; refusing to use it",
                    base.package, base.resolved_version
                ),
            ),
        }
        .into());
    }
    if resolved.manifest_digest != base.manifest_digest {
        return Err(crate::environment::Error::Filesystem {
            path: base.package.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "locked manifest digest for extends base {}@{} does not match its currently published content; refusing to use it (run `ctx traits update` to accept the change)",
                    base.package, base.resolved_version
                ),
            ),
        }
        .into());
    }
    Ok(resolved)
}

/// Merge one resolved base's `[dependencies]` declarations under this
/// project's local `[dependencies]` table (P443): base entries first, local
/// entries overlaid by alias so a local declaration always wins on
/// collision. Returns the merged effective map together with the subset of
/// aliases that came from the base and were not shadowed locally.
fn merge_effective_packages(
    base_packages: Option<&BTreeMap<String, ProjectPackageDependency>>,
    local_packages: &BTreeMap<String, ProjectPackageDependency>,
) -> (BTreeMap<String, ProjectPackageDependency>, BTreeSet<String>) {
    let mut effective = base_packages.cloned().unwrap_or_default();
    let inherited: BTreeSet<String> = effective
        .keys()
        .filter(|alias| !local_packages.contains_key(alias.as_str()))
        .cloned()
        .collect();
    for (alias, dependency) in local_packages {
        effective.insert(alias.clone(), dependency.clone());
    }
    (effective, inherited)
}

/// Overwrite just the lock's `[base]` evidence, preserving every package
/// entry untouched. Shared by explicit base update and plain sync's
/// extends-removed repair.
fn write_base_lock(scope: &DistributionScope, base: Option<BaseLockEntry>) -> crate::Result<()> {
    let lock_path = scope.lock_path();
    assert_no_symlink_ancestors(&lock_path, scope.boundary())?;
    let mut lock = scope.read_lock()?.unwrap_or_default();
    lock.base = base;
    let text = encode_project_lock(&mut lock)?;
    atomic_write(&lock_path, &text)
}

/// Remove one package's lock entry and vendor directory without touching
/// the manifest: used only for a package that is no longer inherited from
/// the (re-resolved) `extends` base and was never declared in the local
/// `[dependencies]` table either, so there is nothing in the manifest to
/// edit. Mirrors [`remove`]'s lock/vendor commit steps minus the manifest
/// edit.
fn remove_stale_inherited_package(scope: &DistributionScope, alias: &str) -> crate::Result<()> {
    let lock_path = scope.lock_path();
    assert_no_symlink_ancestors(&lock_path, scope.boundary())?;
    let lock_snapshot = FileSnapshot::capture(&lock_path)?;
    let mut lock = scope.read_lock()?.unwrap_or_default();
    lock.remove_package(alias);
    let lock_text = encode_project_lock(&mut lock)?;

    let vendor_root = scope.vendored_package_root(alias)?;
    assert_no_symlink_ancestors(&vendor_root, scope.boundary())?;
    let vendor_backup = backup_path(&vendor_root, "prune");
    let had_vendor = vendor_root.exists();
    if had_vendor {
        if let Err(source) = std::fs::rename(&vendor_root, &vendor_backup) {
            return Err(crate::environment::Error::Filesystem {
                path: vendor_root.to_string(),
                source,
            }
            .into());
        }
    }
    if let Err(err) = atomic_write(&lock_path, &lock_text) {
        let mut notes = Vec::new();
        if had_vendor {
            notes.extend(try_restore_vendor_backup(&vendor_backup, &vendor_root));
        }
        notes.extend(lock_snapshot.restore());
        return Err(with_notes(err, notes));
    }
    if had_vendor {
        let _ = std::fs::remove_dir_all(&vendor_backup);
    }
    Ok(())
}

/// Summary of the currently locked `extends` base, for CLI reporting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BaseSummary {
    pub extends: String,
    pub package: String,
    pub resolved_version: String,
    pub integrity: String,
}

/// Read the currently locked `extends` base evidence, if any.
pub fn current_base(scope: &DistributionScope) -> crate::Result<Option<BaseSummary>> {
    let lock = scope.read_lock()?;
    Ok(lock.and_then(|lock| lock.base).map(|base| BaseSummary {
        extends: base.extends,
        package: base.package,
        resolved_version: base.resolved_version,
        integrity: base.integrity,
    }))
}

// ---------------------------------------------------------------------------
// Transactional publication
// ---------------------------------------------------------------------------

/// A captured "undo" snapshot of a file's prior content (or absence),
/// captured before any write in a transaction so a later failure can restore
/// exactly what was there before this operation started.
struct FileSnapshot {
    path: Utf8PathBuf,
    previous: Option<String>,
}

impl FileSnapshot {
    fn capture(path: &Utf8Path) -> crate::Result<Self> {
        let previous = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source,
                }
                .into());
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            previous,
        })
    }

    /// Restore this file to its captured prior content (or absence).
    /// Returns a note naming what could not be undone when the restore
    /// itself fails, rather than swallowing that failure — the caller
    /// appends it to the primary error via `with_rollback_notes`.
    fn restore(&self) -> Option<String> {
        let result = match &self.previous {
            Some(text) => atomic_write(&self.path, text),
            None => std::fs::remove_file(&self.path).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: self.path.to_string(),
                    source,
                }
                .into()
            }),
        };
        result
            .err()
            .map(|err| format!("could not restore {}: {err}", self.path))
    }
}

// Symlink-ancestor rejection, atomic file writes, and lock encoding reuse
// the same primitives `crate::project_lock` already implements for
// `.ctx/traits.lock` itself (`assert_no_symlink_ancestors`,
// `atomic_write_string`, `encode_project_lock`) rather than a second copy.
use crate::project_lock::{
    assert_no_symlink_ancestors, atomic_write_string as atomic_write, encode_project_lock,
};

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn backup_path(path: &Utf8Path, label: &str) -> Utf8PathBuf {
    path.with_file_name(format!(
        "{}.{label}-backup-{}-{}",
        path.file_name().unwrap_or("dir"),
        std::process::id(),
        epoch_nanos()
    ))
}

/// P443 ownership evidence for one `publish_staged_package` call, bundled
/// into a single argument to keep the function's arity in check.
#[derive(Debug, Clone, Copy)]
struct PackageOwnership {
    /// `true` when this package was merged in from a resolved `extends`
    /// base rather than declared in the local `[dependencies]` table.
    inherited: bool,
    /// `true` when the caller (a reconciliation pass, never a direct
    /// user-issued install) has already computed `package` from the
    /// effective base+local manifest merge, so an alias legitimately
    /// changing which package it points to is expected here, not a
    /// conflict — see the lock-level check below.
    allow_transition: bool,
}

/// Publish a fully staged package: prepares the edited manifest text, the
/// updated lock text, and a complete vendor-directory copy before touching
/// any live file, then commits via symlink-safe atomic writes and a
/// rename-based vendor swap. A failure at any commit step restores every
/// artifact already touched by this call.
fn publish_staged_package(
    scope: &DistributionScope,
    alias: &str,
    package: &str,
    requested: &str,
    staged: &StagedPackage,
    ownership: PackageOwnership,
    audit_action: Option<crate::audit_journal::AuditAction>,
) -> crate::Result<()> {
    reject_alias_collision(scope, alias, package)?;

    // An inherited (P443 `extends`-merged) package is never declared in the
    // local `[dependencies]` table: only its lock/vendor evidence is
    // published, so the manifest a project author reads back always shows
    // exactly what they themselves declared.
    let manifest_path = scope.manifest_path("toml");
    let manifest_write = if ownership.inherited {
        None
    } else {
        assert_no_symlink_ancestors(&manifest_path, scope.boundary())?;
        let manifest_snapshot = FileSnapshot::capture(&manifest_path)?;
        let manifest_text = prepare_manifest_dependency_text(
            manifest_snapshot.previous.as_deref(),
            alias,
            package,
            requested,
            &manifest_path,
        )?;
        Some((manifest_snapshot, manifest_text))
    };

    let lock_path = scope.lock_path();
    assert_no_symlink_ancestors(&lock_path, scope.boundary())?;
    let lock_snapshot = FileSnapshot::capture(&lock_path)?;
    let mut lock = scope.read_lock()?.unwrap_or_default();
    if let Some(existing) = lock.package_entry(alias) {
        if existing.package != package && !ownership.allow_transition {
            return Err(alias_collision_error(alias, &existing.package, package));
        }
    }
    let vendored_path = scope.vendored_path_string(alias);
    let tree_digest = crate::registry::compute_tree_digest(&staged.staging_root)?;
    lock.upsert_package(PackageLockEntry {
        alias: alias.to_string(),
        package: package.to_string(),
        requested: requested.to_string(),
        resolved_version: staged.resolved_version.clone(),
        integrity: staged.integrity.clone(),
        vendored_path,
        tree_digest,
        inherited: ownership.inherited,
        traits: staged
            .traits
            .iter()
            .map(|t| TraitLockEntry {
                id: t.id.clone(),
                canonical_path: t.canonical_path.clone(),
                schema_version: t.schema_version.clone(),
                source_digest: t.source_digest.clone(),
                canonical_digest: t.canonical_digest.clone(),
                model_visible_digest: t.model_visible_digest.clone(),
                resource_manifest_digest: t.resource_manifest_digest.clone(),
            })
            .collect(),
    });
    let lock_text = encode_project_lock(&mut lock)?;

    let vendor_root = scope.vendored_package_root(alias)?;
    assert_no_symlink_ancestors(&vendor_root, scope.boundary())?;
    if let Some(parent) = vendor_root.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }

    // Project-scope only (P446): a global publication has no repository to
    // gitignore. Snapshotted here, before vendor staging is created below —
    // once this snapshot exists, every failure from this point on (staging
    // itself failing is the sole exception, since nothing has touched a
    // live file yet) restores it exactly like manifest/lock/vendor. This
    // file is part of the same transaction, not a fire-and-forget
    // post-commit action.
    let gitignore_snapshot = match scope {
        DistributionScope::Project(repo_root) => Some(FileSnapshot::capture(
            &crate::gitignore::nested_gitignore_path(repo_root),
        )?),
        DistributionScope::Global(_) => None,
    };

    let vendor_staging = backup_path(&vendor_root, "publish");
    if vendor_staging.exists() {
        std::fs::remove_dir_all(&vendor_staging).ok();
    }
    copy_dir_recursive(&staged.staging_root, &vendor_staging)?;

    // Ensure the nested ignore file once staging has fully succeeded but
    // before the live vendor swap below (P446): from here on ensure may
    // have mutated a live file, so its own failure — like every commit step
    // after it — must restore the snapshot and remove the now-orphaned
    // staging tree rather than leaving either behind.
    if let DistributionScope::Project(repo_root) = scope {
        if let Err(err) = crate::gitignore::ensure_nested_gitignore(repo_root) {
            let mut notes = Vec::new();
            if let Some(snapshot) = &gitignore_snapshot {
                notes.extend(snapshot.restore());
            }
            let _ = std::fs::remove_dir_all(&vendor_staging);
            return Err(with_notes(err, notes));
        }
    }

    // Everything is prepared and validated. Commit: vendor swap, then lock,
    // then manifest — each step rolls back everything already applied if a
    // later step fails.
    let vendor_backup = backup_path(&vendor_root, "prior");
    let had_existing_vendor = vendor_root.exists();
    if had_existing_vendor {
        if let Err(source) = std::fs::rename(&vendor_root, &vendor_backup) {
            let mut notes = Vec::new();
            if let Some(snapshot) = &gitignore_snapshot {
                notes.extend(snapshot.restore());
            }
            let _ = std::fs::remove_dir_all(&vendor_staging);
            return Err(with_notes(
                crate::environment::Error::Filesystem {
                    path: vendor_root.to_string(),
                    source,
                }
                .into(),
                notes,
            ));
        }
    }
    if let Err(source) = std::fs::rename(&vendor_staging, &vendor_root) {
        let mut notes = Vec::new();
        if had_existing_vendor {
            notes.extend(try_restore_vendor_backup(&vendor_backup, &vendor_root));
        }
        if let Some(snapshot) = &gitignore_snapshot {
            notes.extend(snapshot.restore());
        }
        let _ = std::fs::remove_dir_all(&vendor_staging);
        return Err(with_notes(
            crate::environment::Error::Filesystem {
                path: vendor_root.to_string(),
                source,
            }
            .into(),
            notes,
        ));
    }

    if let Err(err) = atomic_write(&lock_path, &lock_text) {
        let mut notes = Vec::new();
        if let Some(snapshot) = &gitignore_snapshot {
            notes.extend(snapshot.restore());
        }
        notes.extend(rollback_vendor_swap(
            &vendor_root,
            &vendor_backup,
            had_existing_vendor,
        ));
        return Err(with_notes(err, notes));
    }
    if let Some((_, manifest_text)) = &manifest_write {
        if let Err(err) = atomic_write(&manifest_path, manifest_text) {
            let mut notes = Vec::new();
            notes.extend(lock_snapshot.restore());
            if let Some(snapshot) = &gitignore_snapshot {
                notes.extend(snapshot.restore());
            }
            notes.extend(rollback_vendor_swap(
                &vendor_root,
                &vendor_backup,
                had_existing_vendor,
            ));
            return Err(with_notes(err, notes));
        }
    }

    // For a real mutation (install/update, not a sync repair that merely
    // replays already-locked content) the audit record is a required
    // mutation artifact, not a best-effort side note: an ordinary failure
    // appending it rolls back manifest, lock, and vendor exactly like a
    // failure at any earlier commit step, so this project never observes a
    // committed package mutation with no audit evidence.
    if let Some(action) = audit_action {
        let trait_digests: Vec<String> = staged
            .traits
            .iter()
            .map(|t| t.canonical_digest.clone())
            .collect();
        if let Err(err) = append_audit(
            action,
            package,
            requested,
            &staged.resolved_version,
            &trait_digests,
            scope,
        ) {
            let mut notes = Vec::new();
            if let Some((manifest_snapshot, _)) = &manifest_write {
                notes.extend(manifest_snapshot.restore());
            }
            notes.extend(lock_snapshot.restore());
            if let Some(snapshot) = &gitignore_snapshot {
                notes.extend(snapshot.restore());
            }
            notes.extend(rollback_vendor_swap(
                &vendor_root,
                &vendor_backup,
                had_existing_vendor,
            ));
            return Err(with_notes(err, notes));
        }
    }

    if had_existing_vendor {
        let _ = std::fs::remove_dir_all(&vendor_backup);
    }

    Ok(())
}

/// Reject publication before any staging work when `alias` is already
/// declared in the project manifest for a *different* npm package: a
/// reinstall/update of the same package under its existing alias, or a
/// first install of a fresh alias, both proceed. A missing manifest means no
/// alias is yet claimed, but a manifest that exists and fails to parse is
/// propagated rather than silently treated as collision-free. Called both
/// up front in `install` (before any remote fetch) and again inside
/// `publish_staged_package` against the lock as a second, independent
/// race/drift guard.
fn reject_alias_collision(
    scope: &DistributionScope,
    alias: &str,
    package: &str,
) -> crate::Result<()> {
    let manifest_path = scope.manifest_path("toml");
    if !manifest_path.exists() {
        return Ok(());
    }
    let manifest = scope.read_manifest()?;
    if let Some(existing) = manifest.packages.get(alias) {
        if existing.npm != package {
            return Err(alias_collision_error(alias, &existing.npm, package));
        }
    }
    Ok(())
}

fn alias_collision_error(
    alias: &str,
    existing_package: &str,
    requested_package: &str,
) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: alias.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "alias {alias:?} is already installed as {existing_package:?}; install {requested_package:?} under a distinct --alias, or run `ctx traits remove {existing_package}` first"
            ),
        ),
    }
    .into()
}

/// Undo a committed vendor swap: remove the newly-placed tree, then restore
/// the prior tree from its backup. Returns one note per step that itself
/// failed, so the caller can append them to the primary error rather than
/// leaving the package sitting in a now-unreferenced backup directory.
fn rollback_vendor_swap(
    vendor_root: &Utf8Path,
    vendor_backup: &Utf8Path,
    had_existing: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Err(source) = std::fs::remove_dir_all(vendor_root) {
        if source.kind() != std::io::ErrorKind::NotFound {
            notes.push(format!("could not remove {vendor_root}: {source}"));
        }
    }
    if had_existing {
        if let Err(source) = std::fs::rename(vendor_backup, vendor_root) {
            notes.push(format!(
                "could not restore vendor backup {vendor_backup} to {vendor_root}: {source}"
            ));
        }
    }
    notes
}

/// Attempt to move a vendor backup back into place; returns a note naming
/// the still-present backup when the restore itself fails.
fn try_restore_vendor_backup(vendor_backup: &Utf8Path, vendor_root: &Utf8Path) -> Option<String> {
    std::fs::rename(vendor_backup, vendor_root)
        .err()
        .map(|source| {
            format!("could not restore vendor backup {vendor_backup} to {vendor_root}: {source}")
        })
}

/// Fold zero-or-more rollback notes into `err`, wrapping only when at least
/// one restore step itself failed.
fn with_notes(err: crate::Error, notes: Vec<String>) -> crate::Error {
    if notes.is_empty() {
        err
    } else {
        crate::error::with_rollback_notes(err, notes)
    }
}

/// Build the edited `.ctx/traits.toml` text with `alias`'s `[dependencies]`
/// entry set to `{ npm = package, version = requested }`, without writing
/// anything. Pure preparation so a parse failure never mutates live state.
fn prepare_manifest_dependency_text(
    existing_text: Option<&str>,
    alias: &str,
    package: &str,
    requested: &str,
    manifest_path: &Utf8Path,
) -> crate::Result<String> {
    let text = existing_text
        .map(str::to_string)
        .unwrap_or_else(|| "schema-version = \"0.2\"\n".to_string());
    let mut document = text.parse::<toml_edit::DocumentMut>().map_err(|source| {
        crate::parse::Error::TomlEditDecode {
            context: format!("parse {manifest_path} for install"),
            source: Box::new(source),
        }
    })?;
    if document.get("dependencies").is_none() {
        document["dependencies"] = toml_edit::table();
    }
    let deps = document["dependencies"]
        .as_table_like_mut()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "[dependencies] must be a table",
            ),
        })?;
    let mut entry = toml_edit::InlineTable::new();
    entry.insert("npm", toml_edit::Value::from(package));
    entry.insert("version", toml_edit::Value::from(requested));
    deps.insert(
        alias,
        toml_edit::Item::Value(toml_edit::Value::InlineTable(entry)),
    );
    Ok(document.to_string())
}

fn read_project_manifest(
    repo_root: &Utf8Path,
) -> crate::Result<ctx_traits_core::manifest::ProjectManifest> {
    let manifest_path = crate::layout::project_manifest_path(repo_root, "toml");
    let text = std::fs::read_to_string(&manifest_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source,
        }
    })?;
    toml::from_str(&text).map_err(|source| {
        crate::parse::Error::TomlDecode {
            context: format!("decode {manifest_path}"),
            source,
        }
        .into()
    })
}

fn parse_selector(version: &str) -> crate::Result<VersionSelector> {
    // Reuse the same spec grammar for a bare selector string recorded in the
    // manifest by parsing a synthetic `pkg@<selector>` spec.
    let spec = core_distribution::parse_spec(&format!("pkg@{version}"))
        .map_err(ctx_traits_core::Error::from)?;
    Ok(spec.selector)
}

fn copy_dir_recursive(source: &Utf8Path, dest: &Utf8Path) -> crate::Result<()> {
    std::fs::create_dir_all(dest).map_err(|source_err| crate::environment::Error::Filesystem {
        path: dest.to_string(),
        source: source_err,
    })?;
    for entry in
        std::fs::read_dir(source).map_err(|source_err| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_err,
        })?
    {
        let entry = entry.map_err(|source_err| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_err,
        })?;
        let file_type =
            entry
                .file_type()
                .map_err(|source_err| crate::environment::Error::Filesystem {
                    path: source.to_string(),
                    source: source_err,
                })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let child_source = source.join(name.as_ref());
        let child_dest = dest.join(name.as_ref());
        if file_type.is_dir() {
            copy_dir_recursive(&child_source, &child_dest)?;
        } else if file_type.is_file() {
            std::fs::copy(&child_source, &child_dest).map_err(|source_err| {
                crate::environment::Error::Filesystem {
                    path: child_dest.to_string(),
                    source: source_err,
                }
            })?;
        }
    }
    Ok(())
}

fn append_audit(
    action: crate::audit_journal::AuditAction,
    package: &str,
    requested: &str,
    resolved_version: &str,
    trait_digests: &[String],
    scope: &DistributionScope,
) -> crate::Result<()> {
    let (month, timestamp) = crate::audit_journal::epoch_month_and_timestamp();
    crate::audit_journal::append_record(
        &month,
        &crate::audit_journal::AuditRecord {
            action,
            package: package.to_string(),
            requested_selector: requested.to_string(),
            resolved_version: resolved_version.to_string(),
            trait_digests: trait_digests.to_vec(),
            project_path: scope.boundary().to_string(),
            scope: scope.audit_scope().to_string(),
            timestamp,
        },
    )
}

// ---------------------------------------------------------------------------
// Reconciliation (`ctx traits sync`)
// ---------------------------------------------------------------------------

/// Reconcile project (`[dependencies]`) npm packages before package-local
/// dependency sync: honors an existing compatible project lock exactly (a
/// fresh clone reproduces the same bytes), verifying every locked trait's
/// canonical/resource digest against the vendored files actually on disk
/// rather than trusting the vendor directory's mere presence. Only an
/// explicit `update`, or manifest/lock evidence that is missing or
/// incompatible, performs fresh range selection.
pub fn reconcile_project_dependencies(
    repo_root: &Utf8Path,
    locked: bool,
    registry: RegistryOptions<'_>,
) -> crate::Result<Vec<String>> {
    let manifest_path = crate::layout::project_manifest_path(repo_root, "toml");
    if !manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let manifest = read_project_manifest(repo_root)?;
    if manifest.packages.is_empty() && manifest.extends.is_none() {
        return Ok(Vec::new());
    }
    let lock = crate::project_lock::read_project_lock(repo_root)?;
    let mut warnings = Vec::new();
    let existing_base = lock.as_ref().and_then(|lock| lock.base.clone());
    let base_compatible = manifest
        .extends
        .as_ref()
        .zip(existing_base.as_ref())
        .is_some_and(|(extends, base)| extends == &base.extends);

    // The effective package set this call reconciles, and which of its
    // aliases are inherited from the base rather than locally declared.
    // A compatible locked base never needs live re-resolution here: every
    // inherited package it already produced is recorded, alias-for-alias,
    // on the currently locked entries (`inherited = true`), so ordinary
    // sync only replays/verifies those exact entries — it never re-derives
    // the merge from a freshly resolved base. Only a first sync (no
    // compatible lock yet) resolves the base to discover what it declares.
    let mut effective: BTreeMap<String, ProjectPackageDependency> = manifest.packages.clone();
    let mut inherited_aliases: BTreeSet<String> = BTreeSet::new();
    let mut next_base = existing_base.clone().filter(|_| base_compatible);

    if let Some(extends) = &manifest.extends {
        if base_compatible && locked {
            // `--locked` never performs network IO: trust the alias set the
            // currently locked inherited entries already record, exactly
            // like every other `--locked` drift check in this function.
            for entry in lock.as_ref().map(|l| l.packages.as_slice()).unwrap_or(&[]) {
                if entry.inherited && !effective.contains_key(&entry.alias) {
                    inherited_aliases.insert(entry.alias.clone());
                    effective.insert(
                        entry.alias.clone(),
                        ProjectPackageDependency {
                            npm: entry.package.clone(),
                            version: entry.requested.clone(),
                        },
                    );
                }
            }
        } else if base_compatible {
            // Plain sync re-verifies the pinned base exactly like
            // `replay_locked_package` re-verifies a pinned package: never a
            // fresh range resolution, but an active integrity/digest check
            // against the registry rather than blind trust in local lock
            // bytes.
            let base = existing_base
                .as_ref()
                .expect("base_compatible implies Some");
            let resolved_base = replay_locked_base(base, registry)?;
            let (merged, inherited) =
                merge_effective_packages(Some(&resolved_base.packages), &manifest.packages);
            effective = merged;
            inherited_aliases = inherited;
        } else if locked {
            warnings.push(format!(
                "extends drift: project lock has no matching base evidence for {extends:?} (run `ctx traits sync` without --locked, or `ctx traits update` to resolve the base)"
            ));
        } else {
            let resolved_base = resolve_base(extends, registry)?;
            let (merged, inherited) =
                merge_effective_packages(Some(&resolved_base.packages), &manifest.packages);
            effective = merged;
            inherited_aliases = inherited;
            next_base = Some(BaseLockEntry {
                extends: extends.clone(),
                package: resolved_base.package,
                resolved_version: resolved_base.resolved_version,
                integrity: resolved_base.integrity,
                manifest_path: resolved_base.manifest_relative_path,
                manifest_digest: resolved_base.manifest_digest,
            });
        }
    } else if existing_base.is_some() && locked {
        warnings.push(
            "extends drift: traits.lock still records a base but the project manifest no longer declares extends (run `ctx traits sync` without --locked to repair)".to_string(),
        );
        for entry in lock.as_ref().map(|l| l.packages.as_slice()).unwrap_or(&[]) {
            if entry.inherited {
                inherited_aliases.insert(entry.alias.clone());
                effective.insert(
                    entry.alias.clone(),
                    ProjectPackageDependency {
                        npm: entry.package.clone(),
                        version: entry.requested.clone(),
                    },
                );
            }
        }
        // Plain sync with `extends` removed drops inherited packages and
        // base evidence below, exactly like any other stale-entry prune.
    }

    for (alias, dependency) in &effective {
        let locked_entry = lock.as_ref().and_then(|lock| lock.package_entry(alias));
        // Ownership (`inherited`) must also match the freshly computed
        // effective map, not just package/version: an alias whose content
        // happens to be byte-identical to what's already locked but whose
        // owner just transitioned (e.g. a local override added that
        // restates the base's own package) still needs republishing so the
        // lock's `inherited` flag — and the manifest write it gates — stay
        // truthful, and so a later stale-entry prune keys off correct
        // evidence.
        let compatible = locked_entry.is_some_and(|entry| {
            entry.package == dependency.npm
                && entry.requested == dependency.version
                && entry.inherited == inherited_aliases.contains(alias)
        });
        if compatible {
            let entry = locked_entry.expect("compatible implies a locked entry");
            if vendor_matches_lock(repo_root, entry) {
                continue;
            }
            if locked {
                warnings.push(format!(
                    "dependencies.{alias} drift: vendored content for {} does not match locked evidence (run `ctx traits sync` without --locked to repair)",
                    dependency.npm
                ));
                continue;
            }
            replay_locked_package(repo_root, alias, entry, registry)?;
            continue;
        }
        if locked {
            warnings.push(format!(
                "dependencies.{alias} drift: project lock missing or stale for {} (run `ctx traits sync` without --locked, or `ctx traits update {alias}`)",
                dependency.npm
            ));
            continue;
        }
        let spec_input = format!("{}@{}", dependency.npm, dependency.version);
        install_internal(
            &DistributionScope::project(repo_root),
            &spec_input,
            Some(alias),
            inherited_aliases.contains(alias),
            true,
            registry,
        )?;
    }

    if !locked {
        let scope = DistributionScope::project(repo_root);
        // Keyed off `effective` (the just-computed base+local merge), not
        // `inherited_aliases`: an alias that only *transitioned* ownership
        // (inherited -> local override or back) is still present in
        // `effective` and was just republished by the loop above under its
        // new ownership, so it must never be treated as stale here even
        // though its pre-sync lock entry was `inherited = true` and it is
        // no longer in the freshly computed inherited set.
        let stale: Vec<String> = lock
            .as_ref()
            .map(|lock| lock.packages.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|entry| entry.inherited && !effective.contains_key(&entry.alias))
            .map(|entry| entry.alias.clone())
            .collect();
        for alias in stale {
            remove_stale_inherited_package(&scope, &alias)?;
        }
        let base_unchanged = lock.as_ref().and_then(|l| l.base.as_ref()) == next_base.as_ref();
        if !base_unchanged {
            write_base_lock(&scope, next_base)?;
        }
    }

    Ok(warnings)
}

/// Recompute every locked trait's canonical and resource-manifest digest
/// from the files actually vendored on disk and compare against the lock's
/// recorded evidence. `false` on any missing vendor directory, unreadable
/// trait, or digest mismatch — the caller decides whether that is drift to
/// report (`--locked`) or content to repair (plain sync).
fn vendor_matches_lock(repo_root: &Utf8Path, entry: &PackageLockEntry) -> bool {
    let Ok(resolved) = crate::project_lock::resolve_package_lock_paths(repo_root, entry) else {
        return false;
    };
    vendor_matches_lock_resolved(&resolved, entry)
}

/// Scope-generic core of [`vendor_matches_lock`], shared by project-scope
/// `sync` drift detection and [`approve_package`]'s pre-write verification
/// for both distribution scopes, so trust approval never trusts locked
/// digests without re-checking them against the vendored bytes actually on
/// disk right now.
fn vendor_matches_lock_resolved(
    resolved: &crate::project_lock::ResolvedPackageLockPaths,
    entry: &PackageLockEntry,
) -> bool {
    if !resolved.vendor_root.is_dir() {
        return false;
    }
    // The tree digest is authoritative for the *complete* vendored package:
    // it alone catches a changed, added, or removed file that no per-trait
    // canonical/resource digest names (package `trait.toml`, `config.toml`,
    // package metadata, or any other vendored byte). The per-trait digest
    // checks below additionally pin those specific mismatches to a trait ID
    // for a clearer drift/repair message when they are the cause.
    match crate::registry::compute_tree_digest(&resolved.vendor_root) {
        Ok(digest) if digest == entry.tree_digest => {}
        _ => return false,
    }
    for trait_entry in &entry.traits {
        let Some(manifest_path) = resolved.trait_manifest_paths.get(&trait_entry.id) else {
            return false;
        };
        let Ok(loaded) =
            crate::dependency::load_dependency_package(&entry.alias, None, None, manifest_path)
        else {
            return false;
        };
        if loaded.canonical_digest.as_str() != trait_entry.canonical_digest
            || loaded.resource_manifest_digest.as_str() != trait_entry.resource_manifest_digest
        {
            return false;
        }
    }
    true
}

/// Republish the exact locked version: fetches and stages `entry.package` at
/// `entry.resolved_version` (never a fresh range resolution), asserts the
/// freshly staged integrity still matches the locked evidence, and publishes
/// it with the lock's originally recorded `requested` selector text intact
/// — so a repaired vendor tree never silently rewrites what the manifest
/// author asked for.
fn replay_locked_package(
    repo_root: &Utf8Path,
    alias: &str,
    entry: &PackageLockEntry,
    registry: RegistryOptions<'_>,
) -> crate::Result<()> {
    let spec_input = core_distribution::exact_version_spec(&entry.package, &entry.resolved_version);
    let spec = core_distribution::parse_spec(&spec_input).map_err(ctx_traits_core::Error::from)?;
    let staged = stage_npm_package(&spec, registry)?;
    if staged.resolved_version != entry.resolved_version {
        return Err(crate::environment::Error::Filesystem {
            path: alias.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "locked version {} for {} did not resolve exactly (registry returned {}); it may have been unpublished",
                    entry.resolved_version, entry.package, staged.resolved_version
                ),
            ),
        }
        .into());
    }
    if staged.integrity != entry.integrity {
        return Err(crate::environment::Error::Filesystem {
            path: alias.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "locked integrity for {}@{} does not match the registry's current tarball; refusing to vendor",
                    entry.package, entry.resolved_version
                ),
            ),
        }
        .into());
    }
    publish_staged_package(
        &DistributionScope::project(repo_root),
        alias,
        &entry.package,
        &entry.requested,
        &staged,
        PackageOwnership {
            inherited: entry.inherited,
            allow_transition: false,
        },
        None,
    )
}

// ---------------------------------------------------------------------------
// Package-granular trust resolution (`ctx traits trust approve <package>`, P439)
// ---------------------------------------------------------------------------

/// An installed package located during `trust approve` resolution, together
/// with the tier it was found at.
pub struct ResolvedInstalledPackage {
    pub scope: DistributionScope,
    pub entry: PackageLockEntry,
}

/// Resolve `operand` (manifest/lock alias, or exact npm package name)
/// against installed packages in binding order: project scope first (when
/// `repo_root` is `Some`, i.e. the invocation is inside a repository), then
/// the global scope. Nearest-tier wins; an ambiguous match *within* one tier
/// (two installed packages both matching by alias/name — not expected given
/// alias uniqueness, but a lock could in principle be hand-edited) is
/// rejected rather than guessed.
pub fn resolve_installed_package(
    repo_root: Option<&Utf8Path>,
    operand: &str,
) -> crate::Result<ResolvedInstalledPackage> {
    if let Some(repo_root) = repo_root {
        let scope = DistributionScope::project(repo_root);
        if let Some(entry) = find_installed_package(&scope, operand)? {
            return Ok(ResolvedInstalledPackage { scope, entry });
        }
    }
    let scope = DistributionScope::global()?;
    if let Some(entry) = find_installed_package(&scope, operand)? {
        return Ok(ResolvedInstalledPackage { scope, entry });
    }
    Err(crate::environment::Error::Filesystem {
        path: operand.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "no installed package matches alias or npm package {operand:?} at project or global scope"
            ),
        ),
    }
    .into())
}

fn find_installed_package(
    scope: &DistributionScope,
    operand: &str,
) -> crate::Result<Option<PackageLockEntry>> {
    let Some(lock) = scope.read_lock()? else {
        return Ok(None);
    };
    let matches: Vec<&PackageLockEntry> = lock
        .packages
        .iter()
        .filter(|entry| entry.alias == operand || entry.package == operand)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).clone())),
        many => Err(crate::environment::Error::Filesystem {
            path: operand.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{operand:?} matches multiple installed packages at {} scope ({}); use the exact manifest alias to disambiguate",
                    scope.audit_scope(),
                    many.iter().map(|e| e.alias.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ),
        }
        .into()),
    }
}

/// Result of a successful `trust approve <package>`: every canonical digest
/// across the package's current trait entries, now recorded verified in one
/// atomic, cross-process-locked write.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageApproveReport {
    pub package: String,
    pub alias: String,
    pub scope: String,
    pub digests: Vec<String>,
    /// Per-member write evidence from the shared locked append, including
    /// guard (b)'s supersession statement for any member whose lineage
    /// already had a different verified digest (P534 review blocker 1).
    pub updates: Vec<crate::trust::TrustUpdate>,
}

/// Resolve `operand` to one installed package (project scope first, then
/// global) and atomically mark every one of its *current* trait canonical
/// digests as locally verified trust, under
/// [`crate::trust::update_digests_locked`]'s single cross-process-locked
/// read-modify-write — never a loop of single-digest writes, which could
/// expose a partially approved package or drop a concurrent trust change.
/// Approval always targets exact digests: a later canonical edit changes the
/// digest and is unreviewed again, exactly like single-trait `trust verify`.
pub fn approve_package(
    repo_root: Option<&Utf8Path>,
    operand: &str,
    reason: Option<String>,
) -> crate::Result<PackageApproveReport> {
    let resolved = resolve_installed_package(repo_root, operand)?;
    // Re-verify the vendored tree and every trait's canonical/resource
    // digest against the lock's recorded evidence immediately before
    // writing trust: `traits.lock` alone is not proof the bytes on disk
    // still match it (tampering, partial sync, or a stale lock all diverge
    // silently otherwise), and approval must never mint verified trust for
    // evidence that is not current.
    let resolved_paths = resolved.scope.resolve_lock_paths(&resolved.entry)?;
    if !vendor_matches_lock_resolved(&resolved_paths, &resolved.entry) {
        return Err(crate::environment::Error::Filesystem {
            path: resolved_paths.vendor_root.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "vendored content for package {:?} at {} scope does not match locked evidence; run `ctx traits update {}{}` to repair before approving",
                    resolved.entry.package,
                    resolved.scope.audit_scope(),
                    resolved.entry.alias,
                    if matches!(resolved.scope, DistributionScope::Global(_)) {
                        " -g"
                    } else {
                        ""
                    }
                ),
            ),
        }
        .into());
    }
    let digests: Vec<String> = resolved
        .entry
        .traits
        .iter()
        .map(|t| t.canonical_digest.clone())
        .collect();
    let updates: Vec<crate::trust::DigestTrustUpdate> = resolved
        .entry
        .traits
        .iter()
        .map(|t| {
            crate::trust::DigestTrustUpdate::named(
                t.id.clone(),
                t.canonical_digest.clone(),
                crate::trust::TrustState::Verified,
                reason.clone(),
            )
        })
        .collect();
    let written = crate::trust::update_digests_locked(&updates)?;
    Ok(PackageApproveReport {
        package: resolved.entry.package,
        alias: resolved.entry.alias,
        scope: resolved.scope.audit_scope().to_string(),
        digests,
        updates: written,
    })
}

// ---------------------------------------------------------------------------
// Vendored trait-id enumeration (shared by project and global tiers, P439)
// ---------------------------------------------------------------------------

/// One trait entry recorded in a scope's `traits.lock`, without resolving
/// its manifest path — used by [`crate::inventory`] to enumerate every
/// candidate id a tier offers before resolving the one that actually wins.
#[derive(Debug, Clone)]
pub struct VendoredTraitRef {
    pub id: String,
    pub package: String,
    pub version: String,
}

/// Every trait id `scope`'s `traits.lock` currently vendors, in lock order.
/// Returns an empty vector when the scope has no lock at all (nothing
/// installed there yet) rather than an error.
pub fn vendored_trait_ids(scope: &DistributionScope) -> crate::Result<Vec<VendoredTraitRef>> {
    let Some(lock) = scope.read_lock()? else {
        return Ok(Vec::new());
    };
    let mut refs = Vec::new();
    for package in &lock.packages {
        for trait_entry in &package.traits {
            refs.push(VendoredTraitRef {
                id: trait_entry.id.clone(),
                package: package.package.clone(),
                version: package.resolved_version.clone(),
            });
        }
    }
    Ok(refs)
}

// ---------------------------------------------------------------------------
// Vendored trait-id resolution (shared by project and global tiers, P439)
// ---------------------------------------------------------------------------

/// Resolve trait id `id` to a vendored npm-installed trait's canonical
/// manifest path at `scope`, via that scope's `traits.lock` evidence —
/// exactly [`crate::run`]'s pre-P439 `resolve_project_vendored_trait_id`,
/// generalized so the global tier reuses the identical lock-evidence lookup
/// and schema-version gate instead of a second copy. Returns the manifest
/// path and a stable origin label carrying the resolved package/version
/// (`"npm:pkg@1.0.0"` for the project tier, `"npm (global):pkg@1.0.0"` for
/// the global tier) so every consumer of this label — explicit-id run
/// start, query selection, `list`, and run formatting — reports which
/// package/version won, not merely which tier.
pub fn resolve_vendored_trait_id(
    scope: &DistributionScope,
    id: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    let Some(lock) = scope.read_lock()? else {
        return Ok(None);
    };
    for package in &lock.packages {
        for trait_entry in &package.traits {
            if trait_entry.id != id {
                continue;
            }
            if !ctx_traits_core::r#trait::is_schema_version_supported(&trait_entry.schema_version) {
                return Err(crate::environment::Error::Filesystem {
                    path: scope.lock_path().to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "trait {id:?} (npm package {}, vendored at {}) declares schema-version {} which this binary does not support; upgrade ctx",
                            package.package, package.vendored_path, trait_entry.schema_version
                        ),
                    ),
                }
                .into());
            }
            let resolved = scope.resolve_lock_paths(package)?;
            let path = resolved
                .trait_manifest_paths
                .get(id)
                .cloned()
                .expect("trait_entry.id == id was just matched above");
            let origin = match scope {
                DistributionScope::Project(_) => {
                    format!("npm:{}@{}", package.package, package.resolved_version)
                }
                DistributionScope::Global(_) => {
                    format!(
                        "npm (global):{}@{}",
                        package.package, package.resolved_version
                    )
                }
            };
            return Ok(Some((path, origin)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod publish_exclude_tests {
    use camino::Utf8PathBuf;

    fn scratch_root(tag: &str) -> Utf8PathBuf {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let package = root.join(format!(
            "ctx-publish-exclude-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if package.exists() {
            std::fs::remove_dir_all(package.as_std_path()).expect("clear stale scratch package");
        }
        std::fs::create_dir_all(package.as_std_path()).expect("create scratch package dir");
        package
    }

    const TRAIT_TOML: &str =
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"ready\"\n";

    fn write_generated(root: &camino::Utf8Path, extra_resource: &str) {
        std::fs::create_dir_all(root.join(".ctx/traits/demo/generated").as_std_path()).unwrap();
        std::fs::write(root.join("trait.toml").as_std_path(), TRAIT_TOML).unwrap();
        std::fs::write(
            root.join(".ctx/traits/demo/trait.toml").as_std_path(),
            TRAIT_TOML,
        )
        .unwrap();
        std::fs::write(
            root.join(".ctx/traits/demo/generated/index.toml")
                .as_std_path(),
            format!(
                r#"id = "demo"
schema-version = "0.2"
version = "0.1.0"
name = "Demo"
summary = "A publish-exclude fixture."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "true"
output = ["slot:notified"]
{extra_resource}
"#
            ),
        )
        .unwrap();
    }

    /// A declared resource whose path falls under a default pack-exclude
    /// directory (`target/`) must refuse the inspection loudly, naming the
    /// path and the matched exclude rule — never silently ship an
    /// incomplete tarball nor silently verify clean against its own
    /// already-filtered walk.
    #[test]
    fn declared_resource_under_excluded_dir_refuses_loudly() {
        let root = scratch_root("declared-in-target");
        write_generated(
            &root,
            r#"
[[resource]]
id = "generated-doc"
path = "target/generated.txt"
hint = "generated artifact accidentally declared as a resource"
"#,
        );
        std::fs::create_dir_all(root.join(".ctx/traits/demo/target").as_std_path()).unwrap();
        std::fs::write(
            root.join(".ctx/traits/demo/target/generated.txt")
                .as_std_path(),
            b"hello",
        )
        .unwrap();

        let error = super::inspect_local_package(&root)
            .expect_err("a declared resource under a default pack exclude must refuse");
        let message = error.to_string();
        assert!(
            message.contains("target/generated.txt"),
            "expected the refusal to name the declared path: {message}"
        );
        assert!(
            message.contains("target") && message.contains("[publish] exclude"),
            "expected the refusal to name the matched rule and the override remedy: {message}"
        );
    }

    /// An ordinary build `target/` directory with no resource declared
    /// inside it must still inspect cleanly, and the pre-skip manifest must
    /// record it as an excluded (skipped) path rather than silently
    /// vanishing with no evidence at all.
    #[test]
    fn incidental_excluded_dir_with_no_declared_resource_inspects_cleanly() {
        let root = scratch_root("incidental-target");
        write_generated(&root, "");
        std::fs::create_dir_all(root.join(".ctx/traits/demo/target").as_std_path()).unwrap();
        std::fs::write(
            root.join(".ctx/traits/demo/target/build-artifact.txt")
                .as_std_path(),
            b"hi",
        )
        .unwrap();

        let inspection = super::inspect_local_package(&root)
            .expect("an incidental excluded dir with no declared resource must not refuse");
        assert!(
            inspection
                .required_paths
                .iter()
                .all(|path| !path.starts_with("target/")),
            "excluded directory contents must not appear in required_paths: {:?}",
            inspection.required_paths
        );
        assert!(
            inspection
                .excluded
                .iter()
                .any(|skipped| skipped.path.ends_with("target") && skipped.rule == "target"),
            "expected the skipped-path evidence to record the excluded target/ dir: {:?}",
            inspection.excluded
        );
    }

    /// [`crate::publish::is_pack_excluded`] is the one exclude authority both
    /// [`crate::publish::copy_safe`] and [`collect_package_files`] call —
    /// this pins its default set directly, so a future edit that silently
    /// narrows or widens the set fails here first.
    #[test]
    fn is_pack_excluded_matches_the_documented_default_set() {
        let defaults: Vec<String> = crate::publish::PACK_DEFAULT_EXCLUDES
            .iter()
            .map(|name| name.to_string())
            .collect();
        for name in [".git", "node_modules", "target", ".turbo"] {
            assert!(
                crate::publish::is_pack_excluded(name, &defaults),
                "{name} must be excluded by default"
            );
        }
        for name in ["src", "resources", "generated", "package.json"] {
            assert!(
                !crate::publish::is_pack_excluded(name, &defaults),
                "{name} must not be excluded by default"
            );
        }
    }

    /// A declared `[publish] exclude` override replaces the default set
    /// wholesale: a name outside the override is no longer excluded, proving
    /// the knob (not merely the refusal) works.
    #[test]
    fn is_pack_excluded_honors_a_declared_override() {
        let overridden = vec!["dist".to_string()];
        assert!(crate::publish::is_pack_excluded("dist", &overridden));
        assert!(!crate::publish::is_pack_excluded("target", &overridden));
    }
}
