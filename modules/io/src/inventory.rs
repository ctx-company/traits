//! Shared cross-tier trait inventory (P439).
//!
//! Every consumer that must answer "which trait does id X resolve to, and
//! what did it shadow" — explicit-id run resolution, query-based run
//! selection, `ctx traits list`, and run-start origin/version reporting —
//! scans through exactly the functions in this module, in exactly this
//! binding order: repo-authored, repo-vendored, user-global, built-in.
//! Consolidating the scan here is what keeps those surfaces from being able
//! to disagree with each other about which tier wins or what it shadowed.
//!
//! Project tiers (repo-authored, repo-vendored) are only scanned when the
//! invocation is inside a genuine Git repository ([`crate::state::InvocationRoot::Repo`]):
//! an ad-hoc (non-repository) invocation omits them entirely and resolves
//! purely against the global and built-in tiers, so a stray `.ctx/traits`
//! directory sitting in whatever cwd an ad-hoc invocation happens to run
//! from can never shadow an installed global trait (P439). Project-scoped
//! *mutation* surfaces (`install`, `sync`, ... without `-g`) are unaffected
//! by this and still write project state relative to the literal cwd in a
//! non-repository directory, exactly as before — only *resolving* a trait id
//! for run/list/query purposes requires a real repository to consult that
//! state. Project paths are built from the repository root expressed
//! relative to the current working directory, not the literal cwd, so
//! resolution from a *repository* subdirectory consults the actual project
//! tiers instead of silently missing them.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use crate::state::InvocationRoot;

/// The tier a candidate was found at, in binding order (lower value wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    RepoAuthored,
    RepoVendored,
    UserGlobal,
    BuiltIn,
}

/// One tier's candidate manifest for a trait id, with a stable display
/// origin label.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub tier: Tier,
    pub path: Utf8PathBuf,
    /// Stable origin label: `"trait-id"` (repo-authored), `"npm:pkg@version"`
    /// (repo-vendored), `"npm (global):pkg@version"` (user-global), or
    /// `"built-in"`.
    pub origin: String,
}

/// The winning candidate for one trait id, plus every candidate at a
/// further tier it shadowed.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub winner: Candidate,
    pub shadowed: Vec<Candidate>,
}

/// One invocation's discovered identity, resolved once and shared by every
/// tier query it makes — so a multi-id scan (query selection, `list`) never
/// re-runs Git discovery per candidate id.
pub struct InventoryContext {
    invocation: InvocationRoot,
    repo_root_for_paths: Utf8PathBuf,
}

impl InventoryContext {
    /// Discover the current invocation's identity once.
    pub fn discover() -> crate::Result<Self> {
        let invocation = crate::state::discover_invocation_root()?;
        let repo_root_for_paths = crate::state::repo_root_for_relative_paths_from(&invocation)?;
        Ok(Self {
            invocation,
            repo_root_for_paths,
        })
    }

    pub fn invocation(&self) -> &InvocationRoot {
        &self.invocation
    }

    /// The project root usable for building repo-relative project-tier
    /// paths, expressed relative to the current working directory: the
    /// discovered Git repository root when one exists, otherwise the
    /// literal cwd (project-tier resolution has never required a Git
    /// repository).
    pub fn repo_root_for_paths(&self) -> &Utf8Path {
        &self.repo_root_for_paths
    }

    /// Resolve `id` across every tier visible from this invocation, in
    /// binding order. Returns `Ok(None)` when no tier offers `id` at all.
    pub fn resolve_tiers(&self, id: &str) -> crate::Result<Option<Resolution>> {
        let mut candidates = Vec::new();

        // Project tiers (repo-authored, repo-vendored) are only consulted
        // when this invocation is inside a genuine Git repository. An
        // ad-hoc (non-repository) invocation must resolve purely against
        // global/built-in tiers — a stray `.ctx/traits` sitting in whatever
        // directory the invocation happens to run from is not a project and
        // must never shadow the installed global trait (P439).
        if matches!(self.invocation, crate::state::InvocationRoot::Repo(_)) {
            let repo_root = &self.repo_root_for_paths;
            if let Some(path) = repo_authored_candidate(repo_root, id)? {
                candidates.push(Candidate {
                    tier: Tier::RepoAuthored,
                    path,
                    origin: "trait-id".to_string(),
                });
            }
            if let Some((path, origin)) = crate::distribution::resolve_vendored_trait_id(
                &crate::distribution::DistributionScope::project(repo_root),
                id,
            )? {
                candidates.push(Candidate {
                    tier: Tier::RepoVendored,
                    path,
                    origin,
                });
            }
        }

        if let Some((path, origin)) = crate::distribution::resolve_vendored_trait_id(
            &crate::distribution::DistributionScope::global()?,
            id,
        )? {
            candidates.push(Candidate {
                tier: Tier::UserGlobal,
                path,
                origin,
            });
        }

        if ctx_traits_core::builtin_trait_packages::package(id).is_some()
            && let Some(path) =
                crate::builtin_store::resolve_builtin_manifest_path(self.invocation.path(), id)?
        {
            candidates.push(Candidate {
                tier: Tier::BuiltIn,
                path,
                origin: "built-in".to_string(),
            });
        }

        if candidates.is_empty() {
            return Ok(None);
        }
        candidates.sort_by_key(|candidate| candidate.tier);
        let mut iter = candidates.into_iter();
        let winner = iter.next().expect("checked non-empty above");
        Ok(Some(Resolution {
            winner,
            shadowed: iter.collect(),
        }))
    }

    /// Union of every trait id visible from this invocation across all
    /// tiers — the id set [`Self::resolve_tiers`] can be queried against by
    /// `list` and query selection. Does not itself resolve winners/shadows;
    /// call [`Self::resolve_tiers`] per id for that.
    pub fn candidate_ids(&self) -> crate::Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();

        // Mirrors `resolve_tiers`: project tiers are only scanned inside a
        // genuine Git repository, so an ad-hoc invocation's candidate id set
        // (used by `list` and query selection) never includes a stray local
        // `.ctx/traits` package (P439).
        if matches!(self.invocation, crate::state::InvocationRoot::Repo(_)) {
            let repo_root = &self.repo_root_for_paths;
            for package in crate::discovery::trait_packages(repo_root)? {
                ids.insert(package.trait_id);
            }
            for trait_ref in crate::distribution::vendored_trait_ids(
                &crate::distribution::DistributionScope::project(repo_root),
            )? {
                ids.insert(trait_ref.id);
            }
        }

        for trait_ref in crate::distribution::vendored_trait_ids(
            &crate::distribution::DistributionScope::global()?,
        )? {
            ids.insert(trait_ref.id);
        }

        for package in ctx_traits_core::builtin_trait_packages::packages() {
            ids.insert(package.id.to_string());
        }

        Ok(ids)
    }
}

/// Repo-authored candidate for `id` under `repo_root`'s protocol root
/// (`.ctx/traits/<id>`), if a manifest exists there.
fn repo_authored_candidate(repo_root: &Utf8Path, id: &str) -> crate::Result<Option<Utf8PathBuf>> {
    let path = crate::layout::trait_manifest_path(repo_root, id)?;
    if path.is_file() {
        return Ok(Some(path));
    }
    // A native family has no canonical at the package root; its default
    // variant is what the bare id resolves to. Discovery makes the same substitution
    // (`discovery::trait_packages`) — both have to, or the id appears as a
    // candidate here and then resolves to nothing, which is precisely how
    // `implement`, `plan`, and `refactor` came to be listed as `source-only`.
    crate::discovery::family_default_manifest(repo_root, id)
}
