//! Trait package and runtime path model.
//!
//! Canonical trait packages and runtime state live under `.ctx`.
//!
//! Generated host exports (`.agents/skills`, `.pi/skills`, etc.) are render
//! artifacts, never trait source packages.
//!
//! `camino` UTF-8 paths reduce platform ambiguity at domain/IO boundaries.
//!
//! Trait package layout:
//! ```text
//! .ctx/traits/<id>/
//!   trait.toml            # package manifest: [package] + [dependencies]
//!   source/
//!     index.ts | index.mjs
//!   generated/
//!     index.toml          # canonical trait document
//!     index.map
//!   trait.lock
//!   body.md
//!   scenarios.md
//!   import-report.md
//!   resources/
//!   imported/
//! ```
//!
//! Legacy packages (root `trait.ts`, `generated/trait.toml`, flat vendored
//! `trait.toml` without a `[package]` table) remain readable through the
//! candidate lists below.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use thiserror::Error;

/// Trait package layout validation errors.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid trait ID {id:?}: {reason}")]
    InvalidTraitId { id: String, reason: String },
    #[error("conflicting CDK sources for trait {id:?}: both trait.ts and trait.mjs exist")]
    ConflictingCdkSources { id: String },
}

const TRAIT_PACKAGE_ROOT: &str = ".ctx/traits/packages";
/// P569 predecessor of [`TRAIT_PACKAGE_ROOT`]. Packages used to sit directly
/// under `.ctx/traits/`, sharing that namespace with `vendor/` and (since the
/// manifest move) `vendor.toml`/`runtime.toml` — so a package could never be
/// named `vendor` or `runtime` without colliding. `packages/` gives package
/// ids a namespace of their own.
const LEGACY_TRAIT_PACKAGE_ROOT: &str = ".ctx/traits";
const PROJECT_MANIFEST_ROOT: &str = ".ctx/traits";
/// P569 predecessor root of [`PROJECT_MANIFEST_ROOT`]: the manifest used to
/// sit at `.ctx/traits.toml`, one letter from the per-package `trait.toml`.
const LEGACY_PROJECT_MANIFEST_ROOT: &str = ".ctx";
/// Stem of the project dependency manifest. `vendor` names all three faces of
/// one thing — `vendor.toml` declared, `vendor.lock` resolved, `vendor/`
/// fetched — instead of `traits.toml` colliding by a single letter with the
/// per-package manifest.
const PROJECT_MANIFEST_STEM: &str = "vendor";
/// P569 predecessor stem, still read.
const LEGACY_PROJECT_MANIFEST_STEM: &str = "traits";
const TRAIT_VENDOR_ROOT: &str = ".ctx/traits/vendor";
pub const GENERATED: &str = "generated";
pub const SOURCE_DIR: &str = "source";
/// Canonical trait document under `generated/`.
pub const CANONICAL_MANIFEST: &str = "index.toml";
/// Source-map sidecar under `generated/`.
pub const CANONICAL_SOURCE_MAP: &str = "index.map";
/// Root package manifest (`[package]` + `[dependencies]`), Cargo-like.
pub const PACKAGE_MANIFEST: &str = "package.toml";
/// P569 predecessor of [`PACKAGE_MANIFEST`], still read. `trait.toml` sat one
/// letter from the project manifest `traits.toml` while meaning something
/// structurally different; a package holding a family of variants is a
/// package, not a trait.
pub const LEGACY_PACKAGE_MANIFEST: &str = "trait.toml";
/// Optional committed per-package run-config sidecar (P312): budget only,
/// never canonical trait bytes and never read from `generated/`.
pub const PACKAGE_RUN_CONFIG: &str = "config.toml";
/// Legacy canonical document name (flat vendored packages, pre-v2 layouts).
pub const TRAIT_MANIFEST: &str = "trait.toml";
/// Legacy source-map name (pre-v2 layouts).
pub const TRAIT_SOURCE_MAP: &str = "trait.map";
pub const TRAIT_LOCKFILE: &str = "package.lock";
/// P569 predecessor of [`TRAIT_LOCKFILE`], still read.
pub const LEGACY_TRAIT_LOCKFILE: &str = "trait.lock";
/// Legacy repo-local generated-cache root (pre-P426), retained as a
/// one-release dual-read fallback. See [`crate::state::global_cache_root`]
/// for the active default.
pub const CACHE_ROOT: &str = ".ctx/cache/traits";
/// Legacy repo-local debug-trace root (pre-P426), retained as a one-release
/// fallback for `doctor --migrate-state` visibility. See
/// [`crate::state::global_debug_root`] for the active default.
pub const DEBUG_ROOT: &str = ".ctx/debug";
/// Legacy repo-local run-ledger root (pre-P426), retained as a one-release
/// dual-read fallback. See [`crate::state::global_runs_root`] for the active
/// default.
pub const RUN_ROOT: &str = ".ctx/runs";
pub const WORKTREE_ROOT: &str = ".ctx/worktrees";
/// Current repo-local runtime configuration path (P311). Read after the
/// legacy [`LEGACY_RUNTIME_CONFIG`] sibling at each ancestor so `.ctx/`
/// paths always win the merge.
pub const RUNTIME_CONFIG: &str = ".ctx/traits/runtime.toml";
/// P569 predecessor of [`RUNTIME_CONFIG`], still read so an existing checkout
/// keeps working. `config.toml` claimed a scope it never had — the document
/// configures trait EXECUTION, nothing else — and sat beside `traits.toml`
/// with nothing marking one committed and the other machine-local. The new
/// name pairs with `vendor.toml` as *how* to *what*, and matches the
/// `RuntimeConfig` type it deserializes into.
pub const LEGACY_CTX_RUNTIME_CONFIG: &str = ".ctx/config.toml";
/// Legacy repo-local runtime configuration path (pre-P311), retained as a
/// dual-read fallback so existing `ctx.toml` files keep resolving.
pub const LEGACY_RUNTIME_CONFIG: &str = "ctx.toml";
/// Current repo-local harness registry path (P311). Read after the legacy
/// [`LEGACY_HARNESS_REGISTRY`] sibling at each ancestor so `.ctx/` paths
/// always win the merge.
pub const HARNESS_REGISTRY: &str = ".ctx/harness.toml";
/// Legacy repo-local harness registry path (pre-P311), retained as a
/// dual-read fallback so existing `ctx-harness.toml` files keep resolving.
pub const LEGACY_HARNESS_REGISTRY: &str = "ctx-harness.toml";
/// Current global runtime configuration file name, joined onto
/// `${XDG_CONFIG_HOME:-$HOME/.config}/ctx`. Distinct from [`RUNTIME_CONFIG`]
/// so the project's `.ctx/`-relative layout never leaks into the
/// config-home-relative global path.
pub const GLOBAL_RUNTIME_CONFIG: &str = "traits/runtime.toml";
/// P569 predecessor of [`GLOBAL_RUNTIME_CONFIG`], still read. Same rename,
/// machine tier.
pub const LEGACY_CTX_GLOBAL_RUNTIME_CONFIG: &str = "config.toml";
/// Legacy global runtime configuration file name (pre-P311), joined onto the
/// same `ctx` config-home directory as [`GLOBAL_RUNTIME_CONFIG`].
pub const LEGACY_GLOBAL_RUNTIME_CONFIG: &str = "ctx.toml";
/// Optional repo-local TypeScript authoring source for [`RUNTIME_CONFIG`]
/// (P457). When present, the sibling `config.toml` must be a generated
/// artifact (`# ctx:generated` marker + source manifest) — see
/// `crate::config_source`. Absent by default; a repo with no `config.ts`
/// keeps the hand-authored, TOML-first, zero-drift-check, zero-node path.
pub const RUNTIME_CONFIG_SOURCE: &str = ".ctx/config.ts";
/// Optional global TypeScript authoring source for [`GLOBAL_RUNTIME_CONFIG`]
/// (P457), joined onto the same `ctx` config-home directory. Same generated
/// -artifact contract as [`RUNTIME_CONFIG_SOURCE`].
pub const GLOBAL_RUNTIME_CONFIG_SOURCE: &str = "config.ts";
/// Legacy repo-local runtime materialization root for the embedded
/// first-party built-in meta-trait packages (P337; pre-P426), retained as a
/// one-release dual-read fallback. A sibling of [`CACHE_ROOT`], never nested
/// under it: the two have different lifecycles (generated-cache pruning must
/// never touch the built-in store), and different write authority (only
/// `builtin_store::ensure_store_published` ever writes here). See
/// [`legacy_builtin_store_root_path`] and [`builtin_store_root_path`] for the
/// legacy/active roots.
pub const BUILTIN_STORE_ROOT: &str = ".ctx/cache/builtin-traits";

/// The version segment of the built-in trait store
/// (`.ctx/cache/builtin-traits/<CLI_VERSION>/<id>`). One store directory per
/// CLI build: a version bump republishes fresh embedded bytes instead of
/// reusing a possibly-stale prior materialization.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Repo-relative CDK authoring root.
pub fn trait_authoring_root() -> &'static str {
    TRAIT_PACKAGE_ROOT
}

/// CDK authoring root under `repo_root`.
pub fn trait_authoring_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    resolved_package_root(repo_root)
}

/// The package root in effect for `repo_root`: the current `packages/` tree,
/// or the pre-P569 flat tree when a checkout still has packages there and no
/// `packages/` directory yet. Reads AND writes follow the same answer, so a
/// half-migrated store is not something a single command can create.
pub fn resolved_package_root(repo_root: &Utf8Path) -> Utf8PathBuf {
    let current = repo_root.join(TRAIT_PACKAGE_ROOT);
    if current.is_dir() {
        return current;
    }
    let legacy = repo_root.join(LEGACY_TRAIT_PACKAGE_ROOT);
    // The legacy root only counts when it actually holds a package: after the
    // manifest move it always exists (it holds `vendor.toml`), so mere
    // existence would strand every fresh repo on the old layout.
    if legacy.is_dir() && legacy_root_holds_a_package(&legacy) {
        return legacy;
    }
    current
}

/// Whether a pre-P569 `.ctx/traits` directory contains at least one package —
/// a child directory that is not `vendor`/`packages` and carries a manifest
/// under either name.
fn legacy_root_holds_a_package(legacy_root: &Utf8Path) -> bool {
    let Ok(entries) = std::fs::read_dir(legacy_root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(name) = entry.file_name().into_string() else {
            return false;
        };
        if name == "vendor" || name == "packages" || !entry.path().is_dir() {
            return false;
        }
        let root = legacy_root.join(&name);
        root.join(PACKAGE_MANIFEST).is_file()
            || root.join(LEGACY_PACKAGE_MANIFEST).is_file()
            || root.join(GENERATED).is_dir()
    })
}

/// Repo-relative portable protocol/runtime root.
pub fn trait_protocol_root() -> &'static str {
    TRAIT_PACKAGE_ROOT
}

/// Portable protocol/runtime root under `repo_root`.
pub fn trait_protocol_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    resolved_package_root(repo_root)
}

/// Working-directory-relative worktree root.
pub fn worktree_root() -> &'static str {
    WORKTREE_ROOT
}

/// Project manifest path for an encoding extension such as `toml`.
pub fn project_manifest_path(repo_root: &Utf8Path, extension: &str) -> Utf8PathBuf {
    let current = repo_root
        .join(PROJECT_MANIFEST_ROOT)
        .join(format!("{PROJECT_MANIFEST_STEM}.{extension}"));
    if current.exists() {
        return current;
    }
    // P569: a checkout that predates the rename keeps its manifest where it is
    // — reads AND writes follow it, so `dependency add` does not silently
    // start a second manifest beside the one already in use. A fresh repo has
    // neither, so this returns the new path and init scaffolds there.
    let legacy = legacy_project_manifest_path(repo_root, extension);
    if legacy.exists() { legacy } else { current }
}

/// P569 predecessor location of [`project_manifest_path`]. Read when the new
/// path is absent so an existing checkout is not stranded; never written.
pub fn legacy_project_manifest_path(repo_root: &Utf8Path, extension: &str) -> Utf8PathBuf {
    repo_root
        .join(LEGACY_PROJECT_MANIFEST_ROOT)
        .join(format!("{LEGACY_PROJECT_MANIFEST_STEM}.{extension}"))
}

/// Project lock path, paired with [`project_manifest_path`]'s current/legacy
/// layout (P535): a fresh checkout locks at `.ctx/traits/vendor.lock`
/// alongside `vendor.toml`; an existing checkout that still has
/// `.ctx/traits.lock` (P569 predecessor) keeps reading/writing there so it is
/// not stranded beside a manifest it never migrated.
pub fn project_lock_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    let current = repo_root
        .join(PROJECT_MANIFEST_ROOT)
        .join(format!("{PROJECT_MANIFEST_STEM}.lock"));
    if current.exists() {
        return current;
    }
    let legacy = legacy_project_lock_path(repo_root);
    if legacy.exists() { legacy } else { current }
}

/// P569 predecessor location of [`project_lock_path`]. Read when the new path
/// is absent so an existing checkout is not stranded; never written.
pub fn legacy_project_lock_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root
        .join(LEGACY_PROJECT_MANIFEST_ROOT)
        .join(format!("{LEGACY_PROJECT_MANIFEST_STEM}.lock"))
}

/// Project-local host-placement manifest path (`.ctx/host-placements.toml`),
/// the repo-scoped sibling of
/// [`crate::state::global_host_placements_manifest_path`].
pub const HOST_PLACEMENTS_MANIFEST: &str = ".ctx/host-placements.toml";

/// Project-local host-placement manifest path under `repo_root`.
pub fn project_host_placements_manifest_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(HOST_PLACEMENTS_MANIFEST)
}

/// Repo-relative dependency vendor root.
pub fn trait_vendor_root() -> &'static str {
    TRAIT_VENDOR_ROOT
}

/// Dependency vendor root under `repo_root`.
pub fn trait_vendor_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(TRAIT_VENDOR_ROOT)
}

/// Vendored dependency package root for an alias.
pub fn vendored_dependency_root(repo_root: &Utf8Path, alias: &str) -> Result<Utf8PathBuf, Error> {
    validate_trait_id(alias)?;
    Ok(trait_vendor_root_path(repo_root).join(alias))
}

/// Vendored dependency trait manifest path for an alias.
pub fn vendored_dependency_manifest(
    repo_root: &Utf8Path,
    alias: &str,
) -> Result<Utf8PathBuf, Error> {
    Ok(vendored_dependency_root(repo_root, alias)?.join(TRAIT_MANIFEST))
}

/// Canonical protocol manifest path for a trait ID under `repo_root`.
pub fn trait_manifest_path(repo_root: &Utf8Path, trait_id: &str) -> Result<Utf8PathBuf, Error> {
    Ok(TraitPackageRoot::new(repo_root, trait_id)?
        .manifest()
        .to_path_buf())
}

/// Resolve a package root from a generated manifest, flat manifest, or
/// `source/` authoring entry.
pub fn package_root_for_manifest(manifest: &Utf8Path) -> Option<&Utf8Path> {
    let manifest_dir = manifest.parent()?;
    // A native family leaf lives at `generated/<selector>/index.toml`.  Its
    // package-owned resources, lockfile, and sidecars remain at the family
    // root, not in the leaf directory.
    let family_generated_manifest = matches!(manifest.file_name(), Some("index.toml"))
        && manifest_dir
            .parent()
            .is_some_and(|parent| parent.file_name() == Some(GENERATED))
        && manifest_dir
            .parent()
            .and_then(Utf8Path::parent)
            .is_some_and(|root| {
                is_canonical_package_root(root)
                    || is_vendored_package_root(root)
                    || has_package_manifest(root)
                    || is_builtin_trait_package_root(root)
                    || is_builtin_store_package_root(root)
            });
    let generated_manifest = matches!(
        manifest.file_name(),
        Some("index.toml" | "trait.toml" | "trait.json" | "trait.yaml" | "trait.yml")
    ) && manifest_dir.file_name() == Some(GENERATED);
    let source_entry = matches!(manifest.file_name(), Some("index.ts" | "index.mjs"))
        && manifest_dir.file_name() == Some(SOURCE_DIR);
    let package_root = if family_generated_manifest {
        manifest_dir.parent()?.parent()?
    } else if (generated_manifest || source_entry)
        && manifest_dir.parent().is_some_and(|root| {
            is_canonical_package_root(root)
                || is_vendored_package_root(root)
                || has_package_manifest(root)
                || is_builtin_trait_package_root(root)
                || is_builtin_store_package_root(root)
        })
    {
        manifest_dir.parent()?
    } else {
        manifest_dir
    };
    Some(if package_root.as_str().is_empty() {
        Utf8Path::new(".")
    } else {
        package_root
    })
}

/// Canonical-first manifest candidates for a package root.
///
/// Order: v2 canonical (`generated/index.toml`), legacy generated
/// (`generated/trait.toml`), then flat root `trait.toml` (legacy vendored
/// packages; on v2 packages the root `trait.toml` is the package manifest
/// and is distinguished structurally by its `[package]` table).
pub fn package_manifest_read_candidates(package_root: &Utf8Path) -> [Utf8PathBuf; 3] {
    [
        package_root.join(GENERATED).join(CANONICAL_MANIFEST),
        package_root.join(GENERATED).join(TRAIT_MANIFEST),
        package_root.join(TRAIT_MANIFEST),
    ]
}

/// Root package-manifest path for a package root, following an existing
/// pre-P569 `trait.toml` so a package is never split across both names.
pub fn package_manifest_path(package_root: &Utf8Path) -> Utf8PathBuf {
    let current = package_root.join(PACKAGE_MANIFEST);
    if current.is_file() {
        return current;
    }
    let legacy = package_root.join(LEGACY_PACKAGE_MANIFEST);
    if legacy.is_file() { legacy } else { current }
}

/// Lockfile path for a package root.
///
/// An existing lock wins under either name. When none exists yet the lock
/// FOLLOWS ITS MANIFEST: a package still carrying the pre-P569 `trait.toml`
/// gets `trait.lock`, so a legacy package is never left half-renamed with a
/// `package.lock` beside a `trait.toml`. Only a package that has moved to
/// `package.toml` — or a brand new one — gets `package.lock`.
pub fn package_lock_path(package_root: &Utf8Path) -> Utf8PathBuf {
    let current = package_root.join(TRAIT_LOCKFILE);
    if current.is_file() {
        return current;
    }
    let legacy = package_root.join(LEGACY_TRAIT_LOCKFILE);
    if legacy.is_file() {
        return legacy;
    }
    if !package_root.join(PACKAGE_MANIFEST).is_file()
        && package_root.join(LEGACY_PACKAGE_MANIFEST).is_file()
    {
        return legacy;
    }
    current
}

/// Resolve an existing canonical or flat manifest under a package root.
pub fn resolve_package_manifest(package_root: &Utf8Path) -> Option<Utf8PathBuf> {
    package_manifest_read_candidates(package_root)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Path of the optional package-root run-config sidecar ([`PACKAGE_RUN_CONFIG`])
/// for a resolved package root. The file may not exist; callers treat absence
/// as "no sidecar" rather than an error.
pub fn package_run_config_path(package_root: &Utf8Path) -> Utf8PathBuf {
    package_root.join(PACKAGE_RUN_CONFIG)
}

/// Whether a path is exactly a package root under a `.ctx/traits` tree.
///
/// This governs canonical-vs-legacy *write* and *import* semantics
/// (`package_manifest_write_path`, import staging) and stays `.ctx/traits`-
/// scoped: the first-party built-in packages under
/// `modules/core/builtins/traits` are product-owned, not mutable import/write
/// targets, so they must not become "canonical" for those purposes. Use
/// [`is_builtin_trait_package_root`] where only root-resolution (read paths)
/// needs to recognize the built-in tree.
/// Whether `package_root` is an INSTALLED package at `.ctx/traits/vendor/<alias>`.
///
/// Deliberately separate from [`is_canonical_package_root`], which also gates
/// write and import targets: a vendored tree is read-only evidence and must
/// never become a legal place to write. This predicate answers only "does a
/// manifest under here resolve UP to a package root", which vendored packages
/// need for exactly the same reason authored ones do — their `package.toml`,
/// lockfile, and `[family]` table live at the root while a family leaf sits at
/// `generated/<selector>/index.toml`.
///
/// Without it a vendored family leaf resolved its package root into its own
/// `generated/` directory, found no manifest there, and reported
/// `status=draft` for a package whose manifest says `ready` — which is the
/// difference between an installed family being runnable and not.
/// Whether `package_root` holds a package manifest, wherever it lives.
///
/// Path-shape predicates ([`is_canonical_package_root`],
/// [`is_vendored_package_root`], the built-in trees) all answer "is this one of
/// the locations we know about". A package being STAGED for installation is in
/// none of them — it is a temporary directory — so a family leaf inside it
/// resolved its package root into its own `generated/<selector>/` directory,
/// found no `resources/` there, and recorded a resource-manifest digest over an
/// empty file set. Verification then read the same package from its vendored
/// home, found the resources, and disagreed — so an installed package could be
/// vendored but never approved.
///
/// The manifest's presence is the structural fact, independent of location, and
/// it is what makes staging, vendoring, and future locations all resolve alike.
pub fn has_package_manifest(package_root: &Utf8Path) -> bool {
    package_root.join(PACKAGE_MANIFEST).is_file()
        || package_root.join(LEGACY_PACKAGE_MANIFEST).is_file()
}

pub fn is_vendored_package_root(package_root: &Utf8Path) -> bool {
    let Some(parent) = package_root.parent() else {
        return false;
    };
    parent.file_name() == Some("vendor")
        && parent
            .parent()
            .is_some_and(|traits_dir| traits_dir.file_name() == Some("traits"))
        && parent
            .parent()
            .and_then(Utf8Path::parent)
            .is_some_and(|ctx_dir| ctx_dir.file_name() == Some(".ctx"))
}

pub fn is_canonical_package_root(package_root: &Utf8Path) -> bool {
    // Packages live at `.ctx/traits/packages/<id>` since the layout move; the
    // predicate matched `.ctx/traits/<id>` and so stopped recognizing them,
    // which surfaced as `ctx traits build` refusing every native family with
    // "not under a recognized package's source root" even though `init` had
    // just created the package.
    //
    // Both shapes are accepted: a checkout that predates the move keeps its
    // flat `.ctx/traits/<id>` packages working, and dropping that arm made
    // `package_root_for_manifest` stop recognizing `generated/` on those
    // packages — which then resolved a package root INTO its own generated
    // directory and broke lifecycle writes.
    let Some(parent) = package_root.parent() else {
        return false;
    };
    if parent.file_name() == Some("packages") {
        return parent
            .parent()
            .is_some_and(|traits_dir| traits_dir.file_name() == Some("traits"))
            && parent
                .parent()
                .and_then(Utf8Path::parent)
                .is_some_and(|ctx_dir| ctx_dir.file_name() == Some(".ctx"));
    }
    parent.file_name() == Some("traits")
        && parent
            .parent()
            .is_some_and(|ctx_dir| ctx_dir.file_name() == Some(".ctx"))
}

/// The `modules` directory of the first-party built-in
/// `modules/core/builtins/traits/<id>` tree, if `package_root` is exactly a
/// package root under it. Shared by [`is_builtin_trait_package_root`] and
/// [`builtin_trait_package_repo_root`] so the exact-path-shape traversal is
/// encoded in exactly one place.
///
/// Also recognizes the sibling `modules/core/builtins/templates/<id>` tree
/// (P271 teaching templates): templates are authoring inputs, never
/// resolvable runtime traits (see [`is_canonical_package_root`]'s doc for
/// why that distinction matters for write/import targets), but they share
/// the same on-disk package shape as the built-in meta-traits, so `ctx
/// traits build`/`sync`/`check` run against a committed template the same
/// way they run against a committed built-in.
fn builtin_trait_tree_modules_dir(package_root: &Utf8Path) -> Option<&Utf8Path> {
    let traits_dir = package_root.parent()?;
    if !matches!(traits_dir.file_name(), Some("traits" | "templates")) {
        return None;
    }
    let builtins_dir = traits_dir.parent()?;
    if builtins_dir.file_name() != Some("builtins") {
        return None;
    }
    let core_dir = builtins_dir.parent()?;
    if core_dir.file_name() != Some("core") {
        return None;
    }
    let modules_dir = core_dir.parent()?;
    if modules_dir.file_name() == Some("modules") {
        Some(modules_dir)
    } else {
        None
    }
}

/// Whether a path is exactly a package root under the first-party built-in
/// `modules/core/builtins/traits` tree (the six meta-trait packages moved
/// there in P336).
///
/// Narrowly scoped to root-resolution (`package_root_for_manifest`), so that
/// reading a builtin's `generated/index.toml` or `source/index.ts` walks up
/// to the correct package root without also making the built-in tree a
/// canonical write/import target — see [`is_canonical_package_root`].
pub fn is_builtin_trait_package_root(package_root: &Utf8Path) -> bool {
    builtin_trait_tree_modules_dir(package_root).is_some()
}

/// Whether a path is exactly a package root under the first-party
/// `modules/core/builtins/templates` tree (the P271 teaching templates),
/// as distinct from the `modules/core/builtins/traits` meta-trait tree.
///
/// The two trees share an on-disk shape and both resolve through
/// [`builtin_trait_tree_modules_dir`], but only templates use the
/// `generated/index.map` canonical source-map sidecar; the built-in
/// meta-traits still commit an adjacent `source/index.map`. Callers that
/// need to route build/source-map paths differently for the two trees
/// (e.g. `cdk_build::package_build_paths`) use this to narrow to templates
/// only.
pub fn is_builtin_template_package_root(package_root: &Utf8Path) -> bool {
    package_root
        .parent()
        .is_some_and(|parent| parent.file_name() == Some("templates"))
        && builtin_trait_tree_modules_dir(package_root).is_some()
}

/// The repository root for a built-in trait package root (the parent of
/// `modules`), if `package_root` is exactly a package root under
/// `modules/core/builtins/traits`. Used by
/// `export::infer_repo_root_from_trait_file` to resolve `node_modules`/
/// `@ctx-traits/cdk` from the real repo root when rebuilding a built-in
/// package via `ctx traits build`.
pub fn builtin_trait_package_repo_root(package_root: &Utf8Path) -> Option<&Utf8Path> {
    builtin_trait_tree_modules_dir(package_root)?.parent()
}

/// Manifest write path for a canonical package or external flat package.
pub fn package_manifest_write_path(package_root: &Utf8Path) -> Utf8PathBuf {
    if is_canonical_package_root(package_root) {
        package_root.join(GENERATED).join(CANONICAL_MANIFEST)
    } else {
        package_root.join(TRAIT_MANIFEST)
    }
}

/// Paths for one portable protocol package under a repository root.
///
/// All paths are relative to the repo root and use `camino` UTF-8 paths.
/// Construction validates that the trait ID is a single path component to
/// prevent escaping the trait root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitPackageRoot {
    root: Utf8PathBuf,
    generated: Utf8PathBuf,
    source_dir: Utf8PathBuf,
    manifest: Utf8PathBuf,
    package_manifest: Utf8PathBuf,
    source_map: Utf8PathBuf,
    lock: Utf8PathBuf,
    body: Utf8PathBuf,
    scenarios: Utf8PathBuf,
    import_report: Utf8PathBuf,
    resources: Utf8PathBuf,
    imported: Utf8PathBuf,
}

impl TraitPackageRoot {
    /// Construct paths for a trait ID under a repo root.
    ///
    /// The trait ID must be a single path component (no slashes, no `..`).
    pub fn new(repo_root: &Utf8Path, trait_id: &str) -> Result<Self, Error> {
        validate_trait_id(trait_id)?;

        let root = resolved_package_root(repo_root).join(trait_id);
        let generated = root.join(GENERATED);

        Ok(Self {
            manifest: generated.join(CANONICAL_MANIFEST),
            source_map: generated.join(CANONICAL_SOURCE_MAP),
            source_dir: root.join(SOURCE_DIR),
            package_manifest: package_manifest_path(&root),
            lock: package_lock_path(&root),
            body: root.join("body.md"),
            scenarios: root.join("scenarios.md"),
            import_report: root.join("import-report.md"),
            resources: root.join("resources"),
            imported: root.join("imported"),
            generated,
            root,
        })
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    pub fn generated(&self) -> &Utf8Path {
        &self.generated
    }

    pub fn source_dir(&self) -> &Utf8Path {
        &self.source_dir
    }

    pub fn manifest(&self) -> &Utf8Path {
        &self.manifest
    }

    pub fn package_manifest(&self) -> &Utf8Path {
        &self.package_manifest
    }

    pub fn source_map(&self) -> &Utf8Path {
        &self.source_map
    }

    pub fn lock(&self) -> &Utf8Path {
        &self.lock
    }

    pub fn body(&self) -> &Utf8Path {
        &self.body
    }

    pub fn scenarios(&self) -> &Utf8Path {
        &self.scenarios
    }

    pub fn import_report(&self) -> &Utf8Path {
        &self.import_report
    }

    pub fn resources(&self) -> &Utf8Path {
        &self.resources
    }

    pub fn imported(&self) -> &Utf8Path {
        &self.imported
    }
}

/// CDK source path for a package root, if exactly one supported source
/// exists. v2 packages author at `<root>/source/index.ts` (or `.mjs`);
/// legacy packages author at `<root>/trait.ts` (or `.mjs`). More than one
/// existing source is a conflict.
///
/// Resolves purely from `package_root` — callers that already have a
/// package root (built-in/template packages under
/// `modules/core/builtins/{traits,templates}`, external flat packages, or a
/// `.ctx/traits/<id>` root) never need to reconstruct one from a repo root
/// and trait id.
pub fn package_cdk_source_path(package_root: &Utf8Path) -> Result<Option<Utf8PathBuf>, Error> {
    let candidates = [
        package_root.join(SOURCE_DIR).join("index.ts"),
        package_root.join(SOURCE_DIR).join("index.mjs"),
        package_root.join("trait.ts"),
        package_root.join("trait.mjs"),
    ];
    let mut existing = candidates.into_iter().filter(|path| path.is_file());
    let first = existing.next();
    if existing.next().is_some() {
        return Err(Error::ConflictingCdkSources {
            id: package_root.file_name().unwrap_or_default().to_string(),
        });
    }
    Ok(first)
}

/// CDK source path for a trait ID under a `.ctx/traits` authoring root, if
/// exactly one supported source exists. Thin wrapper over
/// [`package_cdk_source_path`] that also validates `trait_id`.
pub fn trait_cdk_source_path(
    repo_root: &Utf8Path,
    trait_id: &str,
) -> Result<Option<Utf8PathBuf>, Error> {
    validate_trait_id(trait_id)?;
    let root = trait_authoring_root_path(repo_root).join(trait_id);
    package_cdk_source_path(&root)
}

/// Canonical source-map sidecar path for a trait's CDK authoring source.
pub fn trait_source_map_path(repo_root: &Utf8Path, trait_id: &str) -> Result<Utf8PathBuf, Error> {
    Ok(TraitPackageRoot::new(repo_root, trait_id)?
        .source_map()
        .to_path_buf())
}

/// Legacy repo-local built-in trait store root under `repo_root`
/// (`.ctx/cache/builtin-traits`), retained as a one-release dual-read
/// fallback. See [`crate::state::global_cache_root`] for the active default.
pub fn legacy_builtin_store_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(BUILTIN_STORE_ROOT)
}

/// Active runtime built-in trait store root: the global per-repository cache
/// root's `builtin-traits` subfamily (P426). `repo_root` is used only to
/// derive the repository's state key, never as a path prefix.
pub fn builtin_store_root_path(repo_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_cache_root(&key)?.join("builtin-traits"))
}

/// Active named build-artifact cache root for one declared
/// `[run.build-cache.<name>]` cache (P428, generalizing P342/P426's
/// singleton `build-target` root): the global per-repository cache root's
/// `build/<name>` subfamily. `repo_root` is used only to derive the
/// repository's state key, never as a path prefix. `name` must already have
/// passed `ctx_traits_io::harness_config`'s bare-id validation (a safe
/// single path component) — this function does not re-validate it.
pub fn build_cache_root_path(repo_root: &Utf8Path, name: &str) -> crate::Result<Utf8PathBuf> {
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_cache_root(&key)?
        .join("build")
        .join(name))
}

/// The historical singleton build cache. It predates declared named caches,
/// but remains a repository-owned derived directory eligible for explicit
/// reclamation.
pub fn build_target_cache_root_path(repo_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_cache_root(&key)?.join("build-target"))
}

/// Whether `path` is exactly the named build-artifact cache root
/// (`.../ctx/cache/<repo-key>/build/<name>`) for `name`, after resolving
/// lexical `.`/`..` aliases. The opt-in build-cache prune uses this to
/// refuse any target that is not precisely that repo-owned directory, so the
/// destructive removal can never be redirected at an arbitrary path. The
/// literal `ctx` component (the config-home root this crate always
/// resolves, see `crate::state::global_ctx_root`) keeps this from also
/// matching an unrelated caller path that merely happens to contain
/// `cache/<anything>/build/<name>`.
pub fn is_build_cache_root(path: &Utf8Path, name: &str) -> bool {
    let components = normalized_normal_components(path);
    matches!(components.as_slice(), [.., "ctx", "cache", _, "build", leaf] if *leaf == name)
}

/// Whether `path` is exactly the legacy singleton build-target cache root.
pub fn is_build_target_cache_root(path: &Utf8Path) -> bool {
    matches!(
        normalized_normal_components(path).as_slice(),
        [.., "ctx", "cache", _, "build-target"]
    )
}

/// Runtime built-in trait store version directory
/// (`<builtin-store-root>/<version>`).
pub fn builtin_store_version_dir(
    repo_root: &Utf8Path,
    version: &str,
) -> crate::Result<Utf8PathBuf> {
    Ok(builtin_store_root_path(repo_root)?.join(version))
}

/// Runtime built-in trait store package root for one built-in id
/// (`<builtin-store-root>/<version>/<id>`). Validates `trait_id` is a single
/// safe path component before ever joining it onto a real path.
pub fn builtin_store_package_root(
    repo_root: &Utf8Path,
    version: &str,
    trait_id: &str,
) -> crate::Result<Utf8PathBuf> {
    validate_trait_id(trait_id)?;
    Ok(builtin_store_version_dir(repo_root, version)?.join(trait_id))
}

/// Lexically normalize `path` into its sequence of `Normal` component
/// strings, resolving `.` and `..` aliases *without touching the
/// filesystem* (never a `canonicalize`, which would follow symlinks — see
/// `path_safety`'s module doc for why that must not happen here). A `..`
/// pops the preceding `Normal` component the same way the shell or a real
/// filesystem walk would collapse `a/../b` to `b`; a leading `..` with
/// nothing to pop is simply dropped, since every caller of this function
/// only cares about repo-relative containment, never about escaping above
/// the repo root.
///
/// This is the shared classifier both [`is_within_builtin_store`] and
/// [`is_builtin_store_package_root`] build on, so a lexical alias like
/// `.ctx/cache/scratch/../builtin-traits/<version>/<id>` is recognized as
/// what it effectively is — inside the store — instead of bypassing a raw,
/// un-normalized component scan.
fn normalized_normal_components(path: &Utf8Path) -> Vec<&str> {
    let mut stack: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            Utf8Component::Normal(value) => stack.push(value),
            Utf8Component::ParentDir => {
                stack.pop();
            }
            Utf8Component::CurDir | Utf8Component::RootDir | Utf8Component::Prefix(_) => {}
        }
    }
    stack
}

/// Whether `path` lies anywhere within the runtime built-in trait store
/// (`.ctx/cache/builtin-traits/...`), after resolving lexical `.`/`..`
/// aliases. Does not require the path to exist. Used to reject ordinary
/// mutation APIs (`write.rs`, `lockfile::write_lockfile`, `dependency::sync`)
/// that must never write into the store; only `builtin_store`'s internal
/// materializer may.
pub fn is_within_builtin_store(path: &Utf8Path) -> bool {
    let components = normalized_normal_components(path);
    components
        .windows(3)
        .any(|window| window == [".ctx", "cache", "builtin-traits"])
        // Active global shape: `.../ctx/cache/<repo-key>/builtin-traits/...`,
        // where `<repo-key>` is one arbitrary path component. Anchoring on
        // the literal `ctx` config-home component keeps this from matching
        // an unrelated caller path that merely happens to contain
        // `cache/<anything>/builtin-traits`.
        || components
            .windows(4)
            .any(|window| window[0] == "ctx" && window[1] == "cache" && window[3] == "builtin-traits")
}

/// Whether `package_root` is exactly a runtime built-in trait store package
/// root: `.../.ctx/cache/builtin-traits/<version>/<id>`, after resolving
/// lexical `.`/`..` aliases.
///
/// Read-side resolution only (`package_root_for_manifest`, dependency/lock
/// write rejection checks) — the store is never a canonical write/import
/// target and must never be returned by a "discover repo packages" scan; see
/// [`is_canonical_package_root`] for that boundary.
pub fn is_builtin_store_package_root(package_root: &Utf8Path) -> bool {
    let components = normalized_normal_components(package_root);
    if let Some(len) = components.len().checked_sub(5) {
        let prefix = &components[len..len + 3];
        // Legacy shape: `.ctx/cache/builtin-traits/<version>/<id>`.
        if matches!(prefix, [".ctx", "cache", "builtin-traits"]) {
            return true;
        }
    }
    // Active global shape: `.../ctx/cache/<repo-key>/builtin-traits/<version>/<id>`,
    // where `<repo-key>` is one arbitrary path component. Anchoring on the
    // literal `ctx` config-home component keeps this from matching an
    // unrelated caller path that merely happens to contain
    // `cache/<anything>/builtin-traits/<version>/<id>`.
    if let Some(len) = components.len().checked_sub(6) {
        let prefix = &components[len..len + 4];
        if prefix[0] == "ctx" && prefix[1] == "cache" && prefix[3] == "builtin-traits" {
            return true;
        }
    }
    false
}

/// Validate that a trait ID is a single safe path component.
pub(crate) fn validate_trait_id(trait_id: &str) -> Result<(), Error> {
    if trait_id.is_empty() {
        return Err(Error::InvalidTraitId {
            id: trait_id.to_string(),
            reason: "empty trait ID".to_string(),
        });
    }

    if trait_id.contains('/') || trait_id.contains('\\') {
        return Err(Error::InvalidTraitId {
            id: trait_id.to_string(),
            reason: "trait ID must not contain path separators".to_string(),
        });
    }

    if trait_id == "." || trait_id == ".." {
        return Err(Error::InvalidTraitId {
            id: trait_id.to_string(),
            reason: "trait ID must not be a path traversal component".to_string(),
        });
    }

    let path = Utf8Path::new(trait_id);
    if path.components().count() != 1 {
        return Err(Error::InvalidTraitId {
            id: trait_id.to_string(),
            reason: "trait ID must be a single path component".to_string(),
        });
    }

    match path.components().next() {
        Some(Utf8Component::Normal(_)) => Ok(()),
        _ => Err(Error::InvalidTraitId {
            id: trait_id.to_string(),
            reason: "trait ID must be a normal path component".to_string(),
        }),
    }
}

#[cfg(test)]
mod build_cache_tests {
    use super::*;

    #[test]
    fn matches_only_the_named_global_build_cache_shape() {
        let path = Utf8Path::new("/home/u/.config/ctx/cache/repo-abcd1234/build/cargo");
        assert!(is_build_cache_root(path, "cargo"));
        assert!(!is_build_cache_root(path, "pnpm-store"));
    }

    #[test]
    fn rejects_the_legacy_singleton_shape_and_unrelated_paths() {
        assert!(!is_build_cache_root(
            Utf8Path::new("/home/u/repo/.ctx/cache/build-target"),
            "build-target"
        ));
        assert!(!is_build_cache_root(
            Utf8Path::new("/home/u/.config/ctx/cache/repo-abcd1234/traits"),
            "cargo"
        ));
    }

    #[test]
    fn resolves_lexical_parent_dir_aliases_before_matching() {
        let path = Utf8Path::new("/home/u/.config/ctx/cache/repo-abcd1234/build/other/../cargo");
        assert!(is_build_cache_root(path, "cargo"));
    }
}
