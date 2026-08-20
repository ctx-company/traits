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
    #[error("this repo still carries a retired .ctx/traits layout; migrate it first: {commands}")]
    RetiredLayoutRoots { commands: String },
}

/// Directory names state WRITE AUTHORITY (0179): `authored/` is what YOU
/// write, `vendored/` is what the fetcher writes, `generated/` (below) is
/// what the compiler writes. The prior name `packages/` named the category
/// both authored and vendored trees share instead of the property that
/// separates them.
const TRAIT_PACKAGE_ROOT: &str = ".ctx/traits/authored";
/// Retired 0179 predecessor of [`TRAIT_PACKAGE_ROOT`]. No dual-read — a repo
/// still carrying this directory is refused by [`refuse_retired_roots`]
/// naming the `git mv` to [`TRAIT_PACKAGE_ROOT`].
const RETIRED_TRAIT_PACKAGE_ROOT: &str = ".ctx/traits/packages";
const PROJECT_MANIFEST_ROOT: &str = ".ctx/traits";
/// Stem of the committed project config document (0177). `vendor.toml`
/// (P569's `vendor` stem) and the machine-tier reading of this same path as
/// `RuntimeConfig` both die with no legacy read — dependency declarations
/// now live under `[vendor]` in this document instead of a standalone file.
const PROJECT_MANIFEST_STEM: &str = "config";
const TRAIT_VENDOR_ROOT: &str = ".ctx/traits/vendored";
/// Retired 0179 predecessor of [`TRAIT_VENDOR_ROOT`]. No dual-read — a repo
/// still carrying this directory is refused by [`refuse_retired_roots`]
/// naming the `git mv` to [`TRAIT_VENDOR_ROOT`].
const RETIRED_TRAIT_VENDOR_ROOT: &str = ".ctx/traits/vendor";
/// Traits-level catalog of compiler-written artifacts (0179): 0177's
/// `generated/config.toml` and 0178's `generated/runtime.toml`. Machine-
/// written only, one writer (the config compiler) — never a package
/// namespace, and never hand-edited, the same contract a package's own
/// `generated/` carries.
pub const TRAIT_GENERATED_ROOT: &str = ".ctx/traits/generated";
pub const GENERATED: &str = "generated";
pub const SOURCE_DIR: &str = "source";
/// Canonical trait document under `generated/`.
pub const CANONICAL_MANIFEST: &str = "index.toml";
/// Source-map sidecar under `generated/`.
pub const CANONICAL_SOURCE_MAP: &str = "index.map";
/// Root package manifest (`[package]` + `[dependencies]`), Cargo-like.
///
/// Named `trait.toml` since 0169 (returning to the P569-predecessor name).
/// Shares its filename with [`TRAIT_MANIFEST`], the legacy flat-vendored
/// canonical document; the two are disambiguated structurally, not by name —
/// see `decode_package_manifest`'s `[package]`-table check.
pub const PACKAGE_MANIFEST: &str = "trait.toml";
/// RETIRED per-package budget sidecar (0036, retired 0176). Never read as
/// config anymore: the author's budget lives in `trait.toml` `[budget]` /
/// `[variant.<vid>.budget]` and `[defaults.port]`. The constant survives
/// only so resolution can refuse a package still carrying the file with a
/// tombstone naming the new home, instead of silently ignoring it.
pub const PACKAGE_RUNTIME_CONFIG: &str = "runtime.toml";
/// Legacy canonical document name (flat vendored packages, pre-v2 layouts).
pub const TRAIT_MANIFEST: &str = "trait.toml";
/// Legacy source-map name (pre-v2 layouts).
pub const TRAIT_SOURCE_MAP: &str = "trait.map";
pub const TRAIT_LOCKFILE: &str = "trait.lock";
/// Active repo-local run-worktree root (0052). Everything this product owns
/// repo-locally lives under `.ctx/traits/`; `.ctx/` is a shared namespace and
/// a top-level `worktrees/` there is a claim this product should not make.
///
/// The other four repo-local roots 0052 named — `.ctx/cache/traits`,
/// `.ctx/cache/builtin-traits`, `.ctx/runs`, `.ctx/debug` — were already moved
/// OUT of the repository entirely by P426. This was the one still live.
pub const WORKTREE_ROOT: &str = ".ctx/traits/worktrees";
/// Machine-local runtime configuration path (P311; retiered by 0037). What
/// I decide, on THIS machine: seats, models, credentials, absolute paths,
/// and any budget this machine wants different. Gitignored and never
/// scaffolded — copied from [`RUNTIME_CONFIG_EXAMPLE`] by whoever needs it.
/// Read after [`PROJECT_CONFIG`] at each ancestor so a machine-local field
/// wins the field-wise merge over the committed project decision.
pub const RUNTIME_CONFIG: &str = ".ctx/traits/runtime.toml";

/// Committed project config document path (0177; retires the 0037
/// `RuntimeConfig` meaning this path used to carry). This is now the
/// hand-authored face of `ConfigDocument`: `[vendor]` (dependency
/// declarations, formerly `vendor.toml`), team-level trait setting
/// overrides, and dispatch defaults — declarative facts only, never
/// executable ones (see `crate::config_document`). When a sibling
/// [`CONFIG_SOURCE`] exists, this path is instead the machine-written
/// artifact under [`crate::layout::TRAIT_GENERATED_ROOT`] built by `ctx
/// traits config build` — see [`config_document_path`].
pub const PROJECT_CONFIG: &str = ".ctx/traits/config.toml";
/// Committed, fully-commented template for [`RUNTIME_CONFIG`] (0037),
/// scaffolded by `ctx traits init`. Every knob present but commented out
/// showing its default (task 0019's rule: a written value freezes today's
/// default forever, a commented one keeps inheriting). Never read by config
/// resolution — it exists to be copied to `runtime.toml`.
pub const RUNTIME_CONFIG_EXAMPLE: &str = ".ctx/traits/runtime.example.toml";
/// Current repo-local harness registry path (P311).
pub const HARNESS_REGISTRY: &str = ".ctx/harness.toml";
/// Current global runtime configuration file name, joined onto
/// `${XDG_CONFIG_HOME:-$HOME/.config}/ctx`. Distinct from [`RUNTIME_CONFIG`]
/// so the project's `.ctx/`-relative layout never leaks into the
/// config-home-relative global path.
pub const GLOBAL_RUNTIME_CONFIG: &str = "traits/runtime.toml";
/// Optional repo-local TypeScript authoring source for [`PROJECT_CONFIG`]
/// (0177; retires P457's `RUNTIME_CONFIG_SOURCE`, which built the sibling
/// `.ctx/config.toml` as a `RuntimeConfig` — that pathway dies with no
/// legacy read; a repo still carrying `.ctx/config.ts` refuses naming
/// 0178's `runtime.ts`). When present, [`config_document_path`] resolves to
/// the compiled `generated/config.toml` instead of the hand-authored
/// [`PROJECT_CONFIG`] path — see `crate::config_document`.
pub const CONFIG_SOURCE: &str = ".ctx/traits/config.ts";
/// The retired P457 TypeScript authoring source (formerly named
/// `RUNTIME_CONFIG_SOURCE`), which built a marker-headed sibling
/// `.ctx/config.toml` as a `RuntimeConfig`. Refusal-only — never read for
/// content. A repo still carrying this path refuses, naming
/// [`RUNTIME_CONFIG_SOURCE`] as the current home for committed executable
/// runtime facts. Not to be confused with [`CONFIG_SOURCE`], the declarative
/// `ConfigDocument` source.
pub const RETIRED_RUNTIME_CONFIG_SOURCE: &str = ".ctx/config.ts";
/// Optional repo-local TypeScript authoring source for [`RUNTIME_CONFIG`]
/// (0178; outright rename of P457's `RUNTIME_CONFIG_SOURCE`, no dual-read).
/// When present, resolution honors the compiled
/// `generated/runtime.toml` artifact under [`TRAIT_GENERATED_ROOT`] instead
/// of the hand-authored [`RUNTIME_CONFIG`] path, mirroring [`CONFIG_SOURCE`]'s
/// pathway — see `crate::runtime_source`. A hand [`RUNTIME_CONFIG`] alongside
/// this source is a refusal (one authority per fact).
pub const RUNTIME_CONFIG_SOURCE: &str = ".ctx/traits/runtime.ts";
/// Global-tier counterpart of [`RUNTIME_CONFIG_SOURCE`], joined onto the same
/// `ctx` config-home directory as [`GLOBAL_RUNTIME_CONFIG`].
pub const GLOBAL_RUNTIME_CONFIG_SOURCE: &str = "traits/runtime.ts";
/// Repo-local committed example for [`RUNTIME_CONFIG_SOURCE`]'s acceptance
/// model (0178): complete, works as-is TypeScript. A repo carrying this file
/// refuses to run until the operator accepts it (`ctx traits config
/// accept`), which materializes [`RUNTIME_CONFIG_SOURCE`] and digest-stamps
/// the acceptance in the trust store — see `crate::runtime_acceptance`.
/// Committed, never gitignored.
pub const RUNTIME_CONFIG_EXAMPLE_TS: &str = ".ctx/traits/runtime.example.ts";

/// The version segment of the built-in trait store
/// (`.ctx/cache/builtin-traits/<CLI_VERSION>/<id>`). One store directory per
/// CLI build: a version bump republishes fresh embedded bytes instead of
/// reusing a possibly-stale prior materialization.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Traits-level generated-catalog root under `repo_root` (0179). See
/// [`TRAIT_GENERATED_ROOT`] for the write-authority contract.
pub fn trait_generated_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(TRAIT_GENERATED_ROOT)
}

/// Refuses a repo still carrying a retired `.ctx/traits/packages` or
/// `.ctx/traits/vendor` directory (0179 hard rename, no dual-read), naming
/// both `git mv` commands. Called from the entry points that already return
/// `Result` — package discovery and the vendor-root resolver — so build,
/// run, list, sync, and dependency all refuse before touching a
/// half-renamed tree instead of silently misreading it.
pub fn refuse_retired_roots(repo_root: &Utf8Path) -> Result<(), Error> {
    let retired_package = repo_root.join(RETIRED_TRAIT_PACKAGE_ROOT);
    let retired_vendor = repo_root.join(RETIRED_TRAIT_VENDOR_ROOT);
    let mut commands = Vec::new();
    if retired_package.is_dir() {
        commands.push("git mv .ctx/traits/packages .ctx/traits/authored");
    }
    if retired_vendor.is_dir() {
        commands.push("git mv .ctx/traits/vendor .ctx/traits/vendored");
    }
    if commands.is_empty() {
        return Ok(());
    }
    Err(Error::RetiredLayoutRoots {
        commands: commands.join("; "),
    })
}

/// Repo-relative CDK authoring root.
pub fn trait_authoring_root() -> &'static str {
    TRAIT_PACKAGE_ROOT
}

/// CDK authoring root under `repo_root`.
pub fn trait_authoring_root_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    resolved_package_root(repo_root)
}

/// The package root in effect for `repo_root`: the `authored/` tree.
pub fn resolved_package_root(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(TRAIT_PACKAGE_ROOT)
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

/// Hand-authored config document path under `repo_root` (0177), ignoring any
/// `[`CONFIG_SOURCE`]` generated pathway. `extension` is accepted for
/// call-site compatibility with the pre-0177 multi-encoding manifest, but
/// the config document is TOML-only, ever — a non-`"toml"` extension never
/// resolves to an existing path.
pub fn project_manifest_path(repo_root: &Utf8Path, extension: &str) -> Utf8PathBuf {
    if extension == "toml" {
        return repo_root.join(PROJECT_CONFIG);
    }
    repo_root
        .join(PROJECT_MANIFEST_ROOT)
        .join(format!("{PROJECT_MANIFEST_STEM}.{extension}"))
}

/// Committed config document path under `repo_root` (0177), resolving the
/// `[`CONFIG_SOURCE`]`-generated pathway before the hand-authored
/// [`PROJECT_CONFIG`] path: when `.ctx/traits/config.ts` exists, the
/// document lives at `generated/config.toml` under
/// [`TRAIT_GENERATED_ROOT`], compiled by `ctx traits config build`.
/// Drift/staleness/both-present refusals are `crate::config_document`'s
/// concern, not this path resolver's.
pub fn config_document_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    if repo_root.join(CONFIG_SOURCE).exists() {
        return trait_generated_root_path(repo_root).join("config.toml");
    }
    repo_root.join(PROJECT_CONFIG)
}

/// Project lock path (0177: `config.lock`, renamed from P535's
/// `vendor.lock`; no legacy read — `vendor.lock` never materialized in any
/// known checkout).
pub fn project_lock_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root
        .join(PROJECT_MANIFEST_ROOT)
        .join(format!("{PROJECT_MANIFEST_STEM}.lock"))
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
    // A native family variant lives at `generated/<selector>/index.toml`.  Its
    // package-owned resources, lockfile, and sidecars remain at the family
    // root, not in the variant directory.
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
    // Slots 2 and 3 share PACKAGE_MANIFEST's filename with TRAIT_MANIFEST
    // (both "trait.toml"); the caller disambiguates structurally via the
    // `[package]`-table check, not by filename.
    [
        package_root.join(GENERATED).join(CANONICAL_MANIFEST),
        package_root.join(GENERATED).join(TRAIT_MANIFEST),
        package_root.join(TRAIT_MANIFEST),
    ]
}

/// Root package-manifest path for a package root.
pub fn package_manifest_path(package_root: &Utf8Path) -> Utf8PathBuf {
    package_root.join(PACKAGE_MANIFEST)
}

/// Lockfile path for a package root.
pub fn package_lock_path(package_root: &Utf8Path) -> Utf8PathBuf {
    package_root.join(TRAIT_LOCKFILE)
}

/// Resolve an existing canonical or flat manifest under a package root.
pub fn resolve_package_manifest(package_root: &Utf8Path) -> Option<Utf8PathBuf> {
    package_manifest_read_candidates(package_root)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Path of the committed per-package `runtime.toml` ([`PACKAGE_RUNTIME_CONFIG`])
/// for a resolved package root. The file may not exist; callers treat
/// absence as "not declared".
pub fn package_runtime_config_path(package_root: &Utf8Path) -> Utf8PathBuf {
    package_root.join(PACKAGE_RUNTIME_CONFIG)
}

/// Whether a path is exactly a package root under a `.ctx/traits` tree.
///
/// This governs canonical-vs-legacy *write* and *import* semantics
/// (`package_manifest_write_path`, import staging) and stays `.ctx/traits`-
/// scoped: the first-party built-in packages under
/// `modules/builtin/traits` are product-owned, not mutable import/write
/// targets, so they must not become "canonical" for those purposes. Use
/// [`is_builtin_trait_package_root`] where only root-resolution (read paths)
/// needs to recognize the built-in tree.
/// Whether `package_root` is an INSTALLED package at `.ctx/traits/vendor/<alias>`.
///
/// Deliberately separate from [`is_canonical_package_root`], which also gates
/// write and import targets: a vendored tree is read-only evidence and must
/// never become a legal place to write. This predicate answers only "does a
/// manifest under here resolve UP to a package root", which vendored packages
/// need for exactly the same reason authored ones do — their `trait.toml`,
/// lockfile, and `[family]` table live at the root while a family variant sits
/// at `generated/<selector>/index.toml`.
///
/// Without it a vendored family variant resolved its package root into its own
/// `generated/` directory, found no manifest there, and reported
/// `status=draft` for a package whose manifest says `ready` — which is the
/// difference between an installed family being runnable and not.
/// Whether `package_root` holds a package manifest, wherever it lives.
///
/// Path-shape predicates ([`is_canonical_package_root`],
/// [`is_vendored_package_root`], the built-in trees) all answer "is this one of
/// the locations we know about". A package being STAGED for installation is in
/// none of them — it is a temporary directory — so a family variant inside it
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
}

pub fn is_vendored_package_root(package_root: &Utf8Path) -> bool {
    let Some(parent) = package_root.parent() else {
        return false;
    };
    parent.file_name() == Some("vendored")
        && parent
            .parent()
            .is_some_and(|traits_dir| traits_dir.file_name() == Some("traits"))
        && parent
            .parent()
            .and_then(Utf8Path::parent)
            .is_some_and(|ctx_dir| ctx_dir.file_name() == Some(".ctx"))
}

pub fn is_canonical_package_root(package_root: &Utf8Path) -> bool {
    // Packages live at `.ctx/traits/authored/<id>` (0179, formerly
    // `packages/`).
    let Some(parent) = package_root.parent() else {
        return false;
    };
    parent.file_name() == Some("authored")
        && parent
            .parent()
            .is_some_and(|traits_dir| traits_dir.file_name() == Some("traits"))
        && parent
            .parent()
            .and_then(Utf8Path::parent)
            .is_some_and(|ctx_dir| ctx_dir.file_name() == Some(".ctx"))
}

/// The `modules` directory of the first-party built-in
/// `modules/builtin/traits/<id>` tree, if `package_root` is exactly a
/// package root under it. Shared by [`is_builtin_trait_package_root`] and
/// [`builtin_trait_package_repo_root`] so the exact-path-shape traversal is
/// encoded in exactly one place.
///
/// Also recognizes the two sibling trees, which share the same on-disk
/// package shape so `ctx traits build`/`sync`/`check` run against any of
/// them the same way: `modules/builtin/templates/<id>` (teaching templates
/// -- authoring inputs, never resolvable runtime traits; see
/// [`is_canonical_package_root`]'s doc for why that matters for
/// write/import targets) and `modules/builtin/shared/<id>` (packages a
/// built-in depends on but which declare no procedure of their own).
fn builtin_trait_tree_modules_dir(package_root: &Utf8Path) -> Option<&Utf8Path> {
    let traits_dir = package_root.parent()?;
    if !matches!(
        traits_dir.file_name(),
        Some("traits" | "templates" | "shared")
    ) {
        return None;
    }
    let builtin_dir = traits_dir.parent()?;
    if builtin_dir.file_name() != Some("builtin") {
        return None;
    }
    let modules_dir = builtin_dir.parent()?;
    if modules_dir.file_name() == Some("modules") {
        Some(modules_dir)
    } else {
        None
    }
}

/// Whether a path is exactly a package root under the first-party built-in
/// `modules/builtin/traits` tree (the six meta-trait packages moved
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
/// `modules/builtin/templates` tree (the P271 teaching templates),
/// as distinct from the `modules/builtin/traits` meta-trait tree.
///
/// All three buckets share an on-disk shape and resolve through
/// [`builtin_trait_tree_modules_dir`]; this narrows to templates for the
/// callers that genuinely treat a scaffold differently from a runnable
/// package. It is NOT the build/source-map router any more: every bucket
/// writes its canonical and its map under `generated/` (task 0230). It used
/// to be, and the adjacent `source/index.map` files that produced were
/// build debris, deleted with the fix.
pub fn is_builtin_template_package_root(package_root: &Utf8Path) -> bool {
    package_root
        .parent()
        .is_some_and(|parent| parent.file_name() == Some("templates"))
        && builtin_trait_tree_modules_dir(package_root).is_some()
}

/// The repository root for a built-in trait package root (the parent of
/// `modules`), if `package_root` is exactly a package root under
/// `modules/builtin/traits`. Used by
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

/// Authoring-source write path for a canonical package: `<root>/source/index.ts`.
///
/// The assist-facing sibling of [`package_manifest_write_path`] — `generate`,
/// `refine --apply` and `import --llm-assisted` write here so that `build`
/// stays the only producer of `<root>/generated/index.toml` (task 0065).
pub fn package_source_write_path(package_root: &Utf8Path) -> Utf8PathBuf {
    package_root.join(SOURCE_DIR).join("index.ts")
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
/// `modules/builtin/{traits,templates}`, external flat packages, or a
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
/// literal `ctx`/`traits` components (the config-home root this crate always
/// resolves, see `crate::state::global_ctx_root`, and the trait root 0235
/// moved state under) keep this from also matching an unrelated caller path
/// that merely happens to contain `cache/<anything>/build/<name>`.
pub fn is_build_cache_root(path: &Utf8Path, name: &str) -> bool {
    let components = normalized_normal_components(path);
    matches!(
        components.as_slice(),
        [.., "ctx", "traits", "cache", _, "build", leaf] if *leaf == name
    )
}

/// Whether `path` is exactly the legacy singleton build-target cache root.
pub fn is_build_target_cache_root(path: &Utf8Path) -> bool {
    matches!(
        normalized_normal_components(path).as_slice(),
        [.., "ctx", "traits", "cache", _, "build-target"]
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
mod retired_layout_root_tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-retired-layout-roots-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    #[test]
    fn resolved_package_root_prefers_authored_over_the_retired_packages_dir() {
        let repo_root = scratch_dir("resolved-package-root");
        std::fs::create_dir_all(repo_root.join(".ctx/traits/authored")).unwrap();
        assert_eq!(
            resolved_package_root(&repo_root),
            repo_root.join(".ctx/traits/authored")
        );
    }

    #[test]
    fn refuse_retired_roots_is_ok_on_a_migrated_repo() {
        let repo_root = scratch_dir("migrated-ok");
        std::fs::create_dir_all(repo_root.join(".ctx/traits/authored")).unwrap();
        std::fs::create_dir_all(repo_root.join(".ctx/traits/vendored")).unwrap();
        assert!(refuse_retired_roots(&repo_root).is_ok());
    }

    #[test]
    fn refuse_retired_roots_names_both_git_mv_commands() {
        let repo_root = scratch_dir("both-retired");
        std::fs::create_dir_all(repo_root.join(".ctx/traits/packages")).unwrap();
        std::fs::create_dir_all(repo_root.join(".ctx/traits/vendor")).unwrap();
        let err = refuse_retired_roots(&repo_root).expect_err("retired roots must refuse");
        let message = err.to_string();
        assert!(
            message.contains("git mv .ctx/traits/packages .ctx/traits/authored"),
            "names the authored migration: {message}"
        );
        assert!(
            message.contains("git mv .ctx/traits/vendor .ctx/traits/vendored"),
            "names the vendored migration: {message}"
        );
    }

    #[test]
    fn refuse_retired_roots_reports_only_the_retired_dir_actually_present() {
        let repo_root = scratch_dir("packages-only-retired");
        std::fs::create_dir_all(repo_root.join(".ctx/traits/packages")).unwrap();
        let err = refuse_retired_roots(&repo_root).expect_err("retired packages must refuse");
        let message = err.to_string();
        assert!(message.contains("authored"));
        assert!(!message.contains("vendored"));
    }

    #[test]
    fn is_canonical_package_root_matches_authored_not_the_retired_packages_name() {
        assert!(is_canonical_package_root(Utf8Path::new(
            "/repo/.ctx/traits/authored/implement"
        )));
        assert!(!is_canonical_package_root(Utf8Path::new(
            "/repo/.ctx/traits/packages/implement"
        )));
    }

    #[test]
    fn is_vendored_package_root_matches_vendored_not_the_retired_vendor_name() {
        assert!(is_vendored_package_root(Utf8Path::new(
            "/repo/.ctx/traits/vendored/demo"
        )));
        assert!(!is_vendored_package_root(Utf8Path::new(
            "/repo/.ctx/traits/vendor/demo"
        )));
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

/// The authoring-time npm manifest: `.ctx/traits/package.json`.
///
/// Beside the sources that import through it, not a level above. Node walks
/// up from `.ctx/traits/authored/<id>/source/index.ts` and reaches
/// `.ctx/traits/node_modules` first; `.ctx/traits/config.ts` resolves from
/// the same directory. A repository with its own root `node_modules` still
/// resolves everything else through the next step up.
pub fn authoring_manifest_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    authoring_install_root(repo_root).join("package.json")
}

/// Where an authoring install puts `node_modules`, and the directory a
/// package manager runs in.
pub fn authoring_install_root(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(PROJECT_MANIFEST_ROOT)
}
