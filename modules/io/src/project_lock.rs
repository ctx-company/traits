//! Deterministic, symlink-safe reads/writes for the project-level
//! `.ctx/traits/config.lock` (P438/P535; renamed from `vendor.lock` by
//! 0177 — constants/docs/proofs only, no data migration since `vendor.lock`
//! never materialized). See [`ctx_traits_core::project_lock`] for the
//! evidence model, and [`crate::layout::project_lock_path`] for the path
//! this module writes through.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::project_lock::{PackageLockEntry, ProjectLock};

/// Project lock path under a repo root — `.ctx/traits/config.lock`.
pub fn project_lock_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    crate::layout::project_lock_path(repo_root)
}

/// Read the project lock, if present. Returns `None` when the file does not
/// exist so callers can distinguish "no lock yet" from a decode failure.
/// Validates every package entry's `vendored-path` and every trait's
/// `canonical-path` via [`resolve_package_lock_paths`] before returning: a
/// forged lock (wrong-alias vendor path, absolute/traversing canonical
/// path) is rejected here, once, for every consumer, rather than at each
/// call site.
pub fn read_project_lock(repo_root: &Utf8Path) -> crate::Result<Option<ProjectLock>> {
    read_lock_at(&project_lock_path(repo_root), repo_root, |entry| {
        resolve_package_lock_paths(repo_root, entry)
    })
}

/// Scope-generic sibling of [`read_project_lock`], used by
/// [`crate::distribution::DistributionScope::Global`] to read
/// `~/.config/ctx/traits.lock` through the same symlink-guard and
/// per-entry path validation as the project lock, without duplicating
/// either. `symlink_boundary` is the root past which ancestor-symlink
/// checks stop climbing (the repo root, or the global `ctx` config root).
pub fn read_lock_at(
    lock_path: &Utf8Path,
    symlink_boundary: &Utf8Path,
    resolve_entry: impl Fn(&PackageLockEntry) -> crate::Result<ResolvedPackageLockPaths>,
) -> crate::Result<Option<ProjectLock>> {
    assert_no_symlink_ancestors(lock_path, symlink_boundary)?;
    match std::fs::read_to_string(lock_path) {
        Ok(text) => {
            let lock: ProjectLock =
                toml::from_str(&text).map_err(|source| crate::parse::Error::TomlDecode {
                    context: format!("decode lock at {lock_path}"),
                    source,
                })?;
            for entry in &lock.packages {
                resolve_entry(entry)?;
            }
            Ok(Some(lock))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
        .into()),
    }
}

/// Validated, IO-safe paths derived from one locked npm package entry.
///
/// `vendor_root` is always the layout-derived path for `entry.alias`
/// ([`crate::layout::vendored_dependency_root`]) — never the lock's bare
/// `vendored-path` string, which is checked for exact agreement but is not
/// itself used to build a filesystem path. `traits` carries each locked
/// trait's canonical manifest path (validated relative and traversal-free
/// before being joined under `vendor_root`) alongside its family
/// variant/alias identity, in the same order as `entry.traits` — a native
/// family package's leaves share one `id`, so callers must resolve through
/// [`ResolvedTraitPath`]'s `for_id`/`for_variant`/`for_alias` rather than a
/// plain id-keyed map, which would silently collapse them.
pub struct ResolvedPackageLockPaths {
    pub vendor_root: Utf8PathBuf,
    pub traits: Vec<ResolvedTraitPath>,
}

/// One locked trait's validated canonical manifest path plus the family
/// identity needed to disambiguate it from sibling leaves that share the
/// same `id`.
#[derive(Debug, Clone)]
pub struct ResolvedTraitPath {
    pub id: String,
    pub variant: Option<String>,
    pub is_default_variant: bool,
    pub aliases: Vec<String>,
    pub path: Utf8PathBuf,
}

impl ResolvedPackageLockPaths {
    /// Resolve a bare (variant-less) trait ID reference: the single entry
    /// with this `id` when there is exactly one, or the family's declared
    /// default variant when several variants share it.
    pub fn path_for_id(&self, id: &str) -> Option<&Utf8PathBuf> {
        let mut matches = self.traits.iter().filter(|t| t.id == id);
        let first = matches.next()?;
        match matches.next() {
            None => Some(&first.path),
            Some(second) => std::iter::once(first)
                .chain(std::iter::once(second))
                .chain(matches)
                .find(|t| t.is_default_variant)
                .map(|t| &t.path),
        }
    }

    /// Resolve a `family:variant` reference (`variant == "default"` defers
    /// to [`Self::path_for_id`], matching a folded family's own `default`
    /// selector, which need not literally be named `"default"`).
    pub fn path_for_variant(&self, id: &str, variant: &str) -> Option<&Utf8PathBuf> {
        if variant == "default" {
            return self.path_for_id(id);
        }
        self.traits
            .iter()
            .find(|t| t.id == id && t.variant.as_deref() == Some(variant))
            .map(|t| &t.path)
    }

    /// Resolve a legacy hyphenated package alias published by a family variant.
    pub fn path_for_alias(&self, alias: &str) -> Option<&Utf8PathBuf> {
        self.traits
            .iter()
            .find(|t| t.aliases.iter().any(|candidate| candidate == alias))
            .map(|t| &t.path)
    }
}

/// Validate and resolve `entry`'s vendor root and every trait's canonical
/// manifest path, jailed under that vendor root.
///
/// This is the only sanctioned way to turn project-lock evidence into
/// filesystem paths (P438 recurrence: a forged lock must never be able to
/// point list/run/sync outside `.ctx/traits/vendored/<matching-alias>`).
/// Rejects: a `vendored-path` that does not exactly equal the layout-derived
/// path for `entry.alias` (catches both traversal/absolute forgeries and a
/// vendor path stolen from a different alias/package), and any
/// `canonical-path` that is empty, absolute, or contains `..` traversal.
pub fn resolve_package_lock_paths(
    repo_root: &Utf8Path,
    entry: &PackageLockEntry,
) -> crate::Result<ResolvedPackageLockPaths> {
    let expected_vendor_root = crate::layout::vendored_dependency_root(repo_root, &entry.alias)?;
    let expected_vendored_path = format!("{}/{}", crate::layout::trait_vendor_root(), entry.alias);
    resolve_package_lock_paths_in(&expected_vendor_root, &expected_vendored_path, entry)
}

/// Scope-generic sibling of [`resolve_package_lock_paths`]: validates
/// `entry` against a caller-supplied expected vendor root and expected
/// `vendored-path` string instead of assuming the project's
/// `.ctx/traits/vendored/<alias>` layout, so [`crate::distribution`]'s global
/// tier reuses the exact same forged-lock rejection rules.
pub fn resolve_package_lock_paths_in(
    expected_vendor_root: &Utf8Path,
    expected_vendored_path: &str,
    entry: &PackageLockEntry,
) -> crate::Result<ResolvedPackageLockPaths> {
    if entry.vendored_path != expected_vendored_path {
        return Err(forged_lock_error(format!(
            "package {:?} vendored-path {:?} does not match the layout-derived path {expected_vendored_path:?} for alias {:?}",
            entry.identity(),
            entry.vendored_path,
            entry.alias
        )));
    }
    let mut traits = Vec::new();
    for trait_entry in &entry.traits {
        let relative = Utf8Path::new(&trait_entry.canonical_path);
        let unsafe_path = trait_entry.canonical_path.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, camino::Utf8Component::ParentDir));
        if unsafe_path {
            return Err(forged_lock_error(format!(
                "trait {:?} canonical-path {:?} in package {:?} must be a non-empty relative path without .. traversal",
                trait_entry.id,
                trait_entry.canonical_path,
                entry.identity()
            )));
        }
        traits.push(ResolvedTraitPath {
            id: trait_entry.id.clone(),
            variant: trait_entry.variant.clone(),
            is_default_variant: trait_entry.is_default_variant,
            aliases: trait_entry.aliases.clone(),
            path: expected_vendor_root.join(relative),
        });
    }
    Ok(ResolvedPackageLockPaths {
        vendor_root: expected_vendor_root.to_path_buf(),
        traits,
    })
}

fn forged_lock_error(message: String) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: "traits.lock".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, message),
    }
    .into()
}

/// Serialize the project lock to deterministic pretty TOML (sorting its
/// contents first), without writing anything. Shared preparation step for
/// [`write_project_lock`] and for callers that must stage lock bytes before
/// a larger multi-file transaction commits (P438 install/remove
/// publication).
pub fn encode_project_lock(lock: &mut ProjectLock) -> crate::Result<String> {
    lock.sort_for_output();
    toml::to_string_pretty(lock).map_err(|source| {
        crate::parse::Error::TomlEncode {
            context: "encode project lock".to_string(),
            source,
        }
        .into()
    })
}

/// Atomically write the project lock (temp file + rename), after sorting its
/// contents for deterministic byte output.
pub fn write_project_lock(repo_root: &Utf8Path, lock: &mut ProjectLock) -> crate::Result<()> {
    let path = project_lock_path(repo_root);
    assert_no_symlink_ancestors(&path, repo_root)?;
    let text = encode_project_lock(lock)?;
    atomic_write_string(&path, &text)
}

/// Write `contents` to `path` via a sibling temp file plus rename, so a
/// reader can never observe a partially written file. Shared by the project
/// lock writer and by the P438 distribution transaction, which needs the
/// same primitive for the manifest and vendor-swap commit steps.
pub(crate) fn atomic_write_string(path: &Utf8Path, contents: &str) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let tmp_path = path.with_file_name(format!(
        "{}.tmp.{}.{}",
        path.file_name().unwrap_or("file"),
        std::process::id(),
        epoch_nanos()
    ));
    std::fs::write(&tmp_path, contents).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: tmp_path.to_string(),
            source,
        }
    })?;
    if let Err(source) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into());
    }
    Ok(())
}

/// Reject a write target whose leaf or any ancestor up to (and excluding)
/// `repo_root` is a symlink, without following or modifying the symlink's
/// target. Shared by the project lock reader/writer and by the P438
/// distribution transaction's manifest/lock/vendor commit steps.
pub(crate) fn assert_no_symlink_ancestors(
    path: &Utf8Path,
    repo_root: &Utf8Path,
) -> crate::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(symlink_ancestor_error(path));
    }
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == repo_root || dir.as_str().is_empty() {
            break;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(dir)
            && metadata.file_type().is_symlink()
        {
            return Err(symlink_ancestor_error(dir));
        }
        current = dir.parent();
    }
    Ok(())
}

fn symlink_ancestor_error(path: &Utf8Path) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to read/write through a symlinked ancestor",
        ),
    }
    .into()
}

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}
