//! Runtime materialization of the six embedded first-party built-in
//! meta-trait packages (`generate`, `refine`, `critique`,
//! `explain`, `import`, `spec`) as real files on disk.
//!
//! `ctx_traits_core::builtin_trait_packages` only exposes the packages as
//! bytes compiled into the binary; nothing on disk backs them at runtime
//! outside this crate's own repo checkout. The rest of the IO layer
//! (`run::resolve_trait_path`, declared-resource digesting, the
//! `../spec` sibling dependency join) only ever knows how to read real
//! files under a real package root, so this module's only job is to make one
//! exist — every containment/lifecycle/resource gate downstream stays
//! exactly as it is for a repo-local package, no bypass logic anywhere else.
//!
//! The store lives at `<cli-version>/<id>` under this repository's global
//! per-repository cache root's `builtin-traits` subfamily
//! (`~/.config/ctx/cache/<repo-key>/builtin-traits/`, P426), a sibling of the
//! generated trait cache (`.../traits`), never nested under it:
//! cache-artifact pruning must never touch it, and it has its own lifecycle
//! (one version directory per CLI build, self-healing on demand). A
//! one-release legacy fallback still recognizes a pre-P426
//! `.ctx/cache/builtin-traits/...` store for reads.
//!
//! Every store path this module trusts — the store root, the version
//! directory, a package directory, an intermediate directory, an expected
//! file, or the publish lock — goes through [`crate::path_safety`]'s shared
//! no-follow guards before it is opened, created, or accepted; there is no
//! second, parallel shape-checking implementation in this module.
//!
//! Materialization is guarded by a cross-process `flock` publish lock
//! (`publish.lock` under the store root, opened without following a
//! symlink via [`crate::file_lock::open_lock_file_no_follow`] — the same
//! primitive [`crate::merge_lock`] uses) so two concurrent `ctx` processes
//! never race a partial store into place. A contender first does an
//! optimistic, lock-free validity check (the common case: an already-valid
//! store needs no lock at all); a contender that might publish acquires the
//! lock and re-verifies under it before rotating anything in, so a slower
//! contender can never clobber a fresher publish that raced in first.
//!
//! The store is read-only to every ordinary mutation API (`write.rs`,
//! `lockfile::write_lockfile`, `dependency::sync`) — this module is the only
//! writer, and it never repairs a symlinked or otherwise unsafe store shape;
//! that is a hard error every time. A materialized package tree is also
//! checked to contain *exactly* the embedded inventory: an unexpected plain
//! file or directory is treated as staleness (triggers a full rebuild), but
//! an unexpected symlink or special file is a hard error, never silently
//! rebuilt over.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::builtin_trait_packages::{self, BuiltinTraitPackage};
use ctx_traits_core::digest::Digest;

/// Resolve a bare id to its materialized built-in package manifest path,
/// materializing (or repairing) the on-disk store first if needed.
///
/// Returns `Ok(None)` when `trait_id` is not one of the compiled-in built-in
/// packages, so callers fall through to their existing unresolved-id error
/// unchanged.
pub fn resolve_builtin_manifest_path(
    repo_root: &Utf8Path,
    trait_id: &str,
) -> crate::Result<Option<Utf8PathBuf>> {
    Ok(
        resolve_builtin_package_root(repo_root, trait_id)?.map(|package_root| {
            package_root
                .join(crate::layout::GENERATED)
                .join(crate::layout::CANONICAL_MANIFEST)
        }),
    )
}

/// Resolve a bare id to its materialized built-in *package root*
/// (materializing/repairing the on-disk store first if needed), so callers
/// that need more than the canonical manifest — e.g. reading the package's
/// own authored `trait.toml` for a `[family]` table — can join onto it
/// without duplicating the store's own read-candidate/publish policy.
///
/// Returns `Ok(None)` when `trait_id` is not one of the compiled-in built-in
/// packages, so callers fall through to their existing unresolved-id error
/// unchanged.
pub fn resolve_builtin_package_root(
    repo_root: &Utf8Path,
    trait_id: &str,
) -> crate::Result<Option<Utf8PathBuf>> {
    let Some(package) = builtin_trait_packages::package(trait_id) else {
        return Ok(None);
    };
    // Ordered read-candidate policy: a valid global store always wins; only
    // when it is not (yet) usable does this repair/publish the global store.
    // An unsafe shape is a hard `Err` propagated as-is — it is never
    // downgraded to "absent" — so a symlinked or malformed store cannot be
    // silently shadowed by publishing a fresh global store underneath it.
    let global_version_dir =
        crate::layout::builtin_store_version_dir(repo_root, crate::layout::CLI_VERSION)?;
    if store_is_valid(&global_version_dir)? {
        crate::layout::validate_trait_id(trait_id)?;
        crate::layout::validate_trait_id(package.bucket)?;
        return Ok(Some(global_version_dir.join(package.bucket).join(trait_id)));
    }
    ensure_store_published(repo_root)?;
    let package_root = crate::layout::builtin_store_package_root(
        repo_root,
        crate::layout::CLI_VERSION,
        package.bucket,
        trait_id,
    )?;
    Ok(Some(package_root))
}

/// Materialize every embedded built-in package under the current CLI
/// version's store directory if it is not already valid. All six packages
/// are always published together (never one at a time) so a sibling
/// `../spec` dependency join always finds a real materialized
/// package, regardless of which built-in id a caller originally resolved.
fn ensure_store_published(repo_root: &Utf8Path) -> crate::Result<()> {
    let store_root = crate::layout::builtin_store_root_path(repo_root)?;
    let version_dir =
        crate::layout::builtin_store_version_dir(repo_root, crate::layout::CLI_VERSION)?;

    // Ancestor safety must be established *before* anything beneath the
    // store root is trusted, including the optimistic validity check below.
    crate::path_safety::ensure_no_symlink_ancestors(&store_root, "built-in trait store root")?;

    // Optimistic fast path: no lock needed when the store is already valid,
    // which is the overwhelmingly common case after the first lookup in a
    // given repo/version.
    if store_is_valid(&version_dir)? {
        return Ok(());
    }

    crate::path_safety::create_dir_all_no_symlinks(&store_root, "built-in trait store root")?;

    let lock_path = store_root.join("publish.lock");
    let lock_file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    // `lock_file`'s exclusive flock is released when it drops at the end of
    // this function (fd close), regardless of which return path is taken.

    // Re-verify under the lock: a racing contender may have already
    // published a fresh store while this process waited to acquire it. Only
    // a destination confirmed stale *while holding the lock* may be rotated
    // out below — a stale pre-lock check must never drive a replace.
    if store_is_valid(&version_dir)? {
        return Ok(());
    }

    publish_store(&store_root, &version_dir)
}

fn unsafe_shape_error(path: &Utf8Path) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "built-in trait store path has an unsafe shape (symlink or wrong file type); refusing to repair automatically",
        ),
    }
    .into()
}

/// Check a terminal leaf's existence and shape through the one shared
/// no-follow guard, dispatching to the expected kind. `Ok(false)` means
/// missing (self-heals by rebuilding); `Ok(true)` means present and the
/// expected kind; `Err` means an unsafe shape (symlink or wrong file type) —
/// a hard error, never auto-repaired.
fn leaf_exists(path: &Utf8Path, expect_directory: bool, label: &str) -> crate::Result<bool> {
    if expect_directory {
        crate::path_safety::ensure_leaf_is_directory_or_absent(path, label)
    } else {
        crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, label)
    }
}

/// Validate one embedded file's package-relative path is a safe path shape
/// (no absolute path, no `..`/empty segments, no backslashes) before it is
/// ever joined onto a real filesystem path.
fn validate_embedded_relative_path(relative_path: &str) -> crate::Result<()> {
    if relative_path.is_empty()
        || relative_path.starts_with('/')
        || relative_path.contains('\\')
        || relative_path.contains("//")
    {
        return Err(unsafe_shape_error(Utf8Path::new(relative_path)));
    }
    for segment in relative_path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(unsafe_shape_error(Utf8Path::new(relative_path)));
        }
    }
    Ok(())
}

/// Check every intermediate directory component of `relative_path` under
/// `package_root` (e.g. `"generated"` in `"generated/index.toml"`) exists as
/// a plain directory. `Ok(false)` on the shallowest missing component;
/// `Err` on the shallowest unsafe one.
fn intermediate_dirs_exist(package_root: &Utf8Path, relative_path: &str) -> crate::Result<bool> {
    let mut current = package_root.to_owned();
    let segments: Vec<&str> = relative_path.split('/').collect();
    let Some((_, dirs)) = segments.split_last() else {
        return Ok(true);
    };
    for segment in dirs {
        current = current.join(segment);
        if !leaf_exists(&current, true, "built-in trait package directory")? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The complete set of package-relative entries (files and their intermediate
/// directories) a package's embedded manifest declares. This is the "what
/// should be here" half of the exhaustiveness check in
/// [`expected_version_entries`].
fn expected_entries(package: &BuiltinTraitPackage) -> BTreeSet<Utf8PathBuf> {
    let mut entries = BTreeSet::new();
    for file in package.files {
        let path = Utf8PathBuf::from(file.relative_path);
        let mut current = path.clone();
        entries.insert(path);
        while let Some(parent) = current.parent().filter(|p| !p.as_str().is_empty()) {
            entries.insert(parent.to_path_buf());
            current = parent.to_path_buf();
        }
    }
    entries
}

/// The complete set of version-directory-relative entries the embedded
/// manifest declares across *all* built-in packages: every package's own
/// directory plus every entry [`expected_entries`] declares inside it. This
/// is the single exhaustive inventory the version directory's actual
/// contents are compared against in [`store_is_valid`] — the one place a
/// sibling entry alongside the six known package directories can be seen.
fn expected_version_entries() -> BTreeSet<Utf8PathBuf> {
    let mut entries = BTreeSet::new();
    for package in builtin_trait_packages::packages() {
        let bucket_dir = Utf8PathBuf::from(package.bucket);
        let package_dir = bucket_dir.join(package.id);
        for relative in expected_entries(package) {
            entries.insert(package_dir.join(relative));
        }
        entries.insert(package_dir);
        // The bucket directory is an expected entry in its own right: the
        // exhaustiveness walk sees every level, so omitting it would read as
        // an undeclared sibling and rebuild the store on every check.
        entries.insert(bucket_dir);
    }
    entries
}

/// Walk `package_root`'s actual on-disk contents without ever following a
/// symlink, returning the complete set of package-relative entries found.
/// A symlink or other special file anywhere in the tree is a hard error —
/// this is the "what is actually here" half of the exhaustiveness check, and
/// hidden content must never be silently accepted or silently deleted as
/// ordinary staleness.
fn collect_actual_entries(package_root: &Utf8Path) -> crate::Result<BTreeSet<Utf8PathBuf>> {
    let mut entries = BTreeSet::new();
    walk_dir_no_follow(package_root, Utf8Path::new(""), &mut entries)?;
    Ok(entries)
}

fn walk_dir_no_follow(
    dir: &Utf8Path,
    relative: &Utf8Path,
    out: &mut BTreeSet<Utf8PathBuf>,
) -> crate::Result<()> {
    let read_dir = std::fs::read_dir(dir.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        }
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
        let os_name = entry.file_name();
        let name = os_name.to_str().ok_or_else(|| unsafe_shape_error(dir))?;
        let entry_relative = relative.join(name);
        let entry_path = dir.join(name);
        // `DirEntry::file_type` is an `lstat`-equivalent call: it never
        // follows a symlink, so a symlinked entry is always caught here
        // rather than silently traversed into or read through.
        let file_type =
            entry
                .file_type()
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: entry_path.to_string(),
                    source,
                })?;
        if file_type.is_dir() {
            out.insert(entry_relative.clone());
            walk_dir_no_follow(&entry_path, &entry_relative, out)?;
        } else if file_type.is_file() {
            out.insert(entry_relative);
        } else {
            return Err(unsafe_shape_error(&entry_path));
        }
    }
    Ok(())
}

/// Whether the store's version directory contains all six built-in packages
/// with byte-for-byte matching content and, across the *entire* version
/// directory tree (not just each package's own subtree), no undeclared
/// entry. `Ok(false)` means "missing or stale, rebuild it"; an unsafe path
/// shape or undeclared hidden content anywhere in the tree is a hard `Err`,
/// never treated as "just rebuild".
fn store_is_valid(version_dir: &Utf8Path) -> crate::Result<bool> {
    if !leaf_exists(version_dir, true, "built-in trait store version directory")? {
        return Ok(false);
    }
    for package in builtin_trait_packages::packages() {
        if !package_is_valid(version_dir, package)? {
            return Ok(false);
        }
    }
    // Exhaustive inventory over the whole version directory, not each
    // package in isolation: this is the only check that can see an
    // undeclared sibling entry (extra package-like directory, stray file, or
    // symlink) sitting directly under the version directory alongside the
    // six known package directories. A symlink or special file anywhere in
    // the tree is caught here as a hard `Err` via `collect_actual_entries`;
    // an extra ordinary file or directory is ordinary staleness and repaired
    // by the full rebuild `Ok(false)` triggers.
    if collect_actual_entries(version_dir)? != expected_version_entries() {
        return Ok(false);
    }
    Ok(true)
}

/// `Ok(true)` only when the package directory contains every declared file,
/// present, expected kind, matching digest. `Ok(false)` signals "missing or
/// stale" (absent or digest-mismatched, repaired by a full rebuild); an
/// unsafe shape anywhere is a hard `Err`. Undeclared-entry exhaustiveness for
/// this package's own subtree is covered by the single version-directory-wide
/// walk in [`store_is_valid`], not duplicated here.
fn package_is_valid(version_dir: &Utf8Path, package: &BuiltinTraitPackage) -> crate::Result<bool> {
    crate::layout::validate_trait_id(package.id)?;
    let package_root = version_dir.join(package.bucket).join(package.id);
    if !leaf_exists(&package_root, true, "built-in trait package directory")? {
        return Ok(false);
    }
    for file in package.files {
        validate_embedded_relative_path(file.relative_path)?;
        if !intermediate_dirs_exist(&package_root, file.relative_path)? {
            return Ok(false);
        }
        let file_path = package_root.join(file.relative_path);
        if !leaf_exists(&file_path, false, "built-in trait package file")? {
            return Ok(false);
        }
        let bytes = std::fs::read(file_path.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: file_path.to_string(),
                source,
            }
        })?;
        if Digest::from_bytes(&bytes).as_str() != file.digest {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Stage every embedded package into a freshly claimed staging directory,
/// re-read and re-digest every staged file, then atomically publish by
/// renaming the completed staging directory into place. Called only while
/// holding the publish lock, and only once the destination has been
/// reverified stale (missing or digest-mismatched) under that same lock.
fn publish_store(store_root: &Utf8Path, version_dir: &Utf8Path) -> crate::Result<()> {
    let staging_dir = claim_staging_dir(store_root)?;
    if let Err(error) = write_staged_packages(&staging_dir) {
        let _ = std::fs::remove_dir_all(staging_dir.as_std_path());
        return Err(error);
    }
    if let Err(error) = verify_staged_packages(&staging_dir) {
        let _ = std::fs::remove_dir_all(staging_dir.as_std_path());
        return Err(error);
    }

    if leaf_exists(version_dir, true, "built-in trait store version directory")? {
        std::fs::remove_dir_all(version_dir.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: version_dir.to_string(),
                source,
            }
        })?;
    }

    std::fs::rename(staging_dir.as_std_path(), version_dir.as_std_path()).map_err(|source| {
        crate::Error::from(crate::environment::Error::Filesystem {
            path: version_dir.to_string(),
            source,
        })
    })
}

fn claim_staging_dir(store_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let seed = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let staging_dir = store_root.join(format!(".staging-{seed}"));
    std::fs::create_dir(staging_dir.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: staging_dir.to_string(),
            source,
        }
    })?;
    Ok(staging_dir)
}

fn write_staged_packages(staging_dir: &Utf8Path) -> crate::Result<()> {
    for package in builtin_trait_packages::packages() {
        crate::layout::validate_trait_id(package.id)?;
        let package_root = staging_dir.join(package.bucket).join(package.id);
        for file in package.files {
            validate_embedded_relative_path(file.relative_path)?;
            let file_path = package_root.join(file.relative_path);
            if let Some(parent) = file_path.parent() {
                crate::path_safety::create_dir_all_no_symlinks(
                    parent,
                    "built-in trait store staging directory",
                )?;
            }
            std::fs::write(file_path.as_std_path(), file.bytes).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: file_path.to_string(),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

/// Re-read and re-digest every file just staged, rejecting the whole publish
/// if anything does not match its embedded digest exactly.
fn verify_staged_packages(staging_dir: &Utf8Path) -> crate::Result<()> {
    for package in builtin_trait_packages::packages() {
        let package_root = staging_dir.join(package.bucket).join(package.id);
        for file in package.files {
            let file_path = package_root.join(file.relative_path);
            if !leaf_exists(&file_path, false, "staged built-in trait package file")? {
                return Err(unsafe_shape_error(&file_path));
            }
            let bytes = std::fs::read(file_path.as_std_path()).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: file_path.to_string(),
                    source,
                }
            })?;
            if Digest::from_bytes(&bytes).as_str() != file.digest {
                return Err(crate::environment::Error::Filesystem {
                    path: file_path.to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "staged built-in trait file digest mismatch after write",
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}
