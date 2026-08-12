//! Generated cache artifact IO.
//!
//! Reads and writes deterministic cache metadata under a generated cache root
//! (default: the invocation repository's global per-repository cache root's
//! `traits` subfamily, `~/.config/ctx/cache/<repo-key>/traits/`, P426).
//! Cache artifacts are never stored under canonical trait source packages.
//! Safe path guards reject traversal, symlink leaves, directories, special
//! files, and exact trait source root paths.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

pub use ctx_traits_core::cache::StoredCacheArtifact;

/// Metadata file name for stored cache artifact records.
const METADATA_FILENAME: &str = "cache-metadata.json";

/// Validated, generated advisory narration for one canonical trait digest.
/// This is deliberately separate from build-cache metadata: it is neither
/// source nor authority and can only be selected by the canonical digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitExplanation {
    pub version: u8,
    pub trait_id: String,
    pub canonical_digest: String,
    pub explanation: String,
}

/// Read one digest-addressed advisory explanation. Missing records are a
/// cache miss; malformed or unsafe records are rejected rather than shown.
pub fn read_trait_explanation(
    repo_root: &Utf8Path,
    trait_id: &str,
    canonical_digest: &str,
) -> crate::Result<Option<TraitExplanation>> {
    let path = trait_explanation_path(repo_root, canonical_digest)?;
    let parent = path.parent().expect("explanation cache path has parent");
    crate::path_safety::ensure_no_symlink_ancestors(parent, "trait explanation cache ancestor")?;
    if !crate::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "trait explanation")? {
        return Ok(None);
    }
    let text = crate::read::read_text(&path)?;
    let record: TraitExplanation =
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse trait explanation at {path}"),
            source,
        })?;
    if record.version != 1
        || record.trait_id != trait_id
        || record.canonical_digest != canonical_digest
        || record.explanation.trim().is_empty()
    {
        return Err(crate::Error::Usage {
            message: format!("invalid trait explanation cache record at {path}"),
        });
    }
    Ok(Some(record))
}

/// Publish a validated advisory explanation under its canonical digest.
pub fn write_trait_explanation(
    repo_root: &Utf8Path,
    record: &TraitExplanation,
) -> crate::Result<()> {
    if record.version != 1 || record.explanation.trim().is_empty() {
        return Err(crate::Error::Usage {
            message: "trait explanation cache record must be version 1 with non-empty text"
                .to_string(),
        });
    }
    let path = trait_explanation_path(repo_root, &record.canonical_digest)?;
    let parent = path.parent().expect("explanation cache path has parent");
    crate::path_safety::create_dir_all_no_symlinks(parent, "trait explanation cache ancestor")?;
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "trait explanation")?;
    let text = serde_json::to_string_pretty(record).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize trait explanation at {path}"),
            source,
        }
    })?;
    crate::write::write_bytes_atomically(&path, format!("{text}\n").as_bytes())
}

/// Serialize a cache miss per canonical digest and re-check after acquiring
/// the lock so concurrent callers never generate the same narration twice.
pub fn get_or_generate_trait_explanation(
    repo_root: &Utf8Path,
    trait_id: &str,
    canonical_digest: &str,
    generate: impl FnOnce() -> Result<TraitExplanation, String>,
) -> crate::Result<(TraitExplanation, bool)> {
    if let Some(record) = read_trait_explanation(repo_root, trait_id, canonical_digest)? {
        return Ok((record, true));
    }
    let path = trait_explanation_path(repo_root, canonical_digest)?;
    let parent = path.parent().expect("explanation cache path has parent");
    crate::path_safety::create_dir_all_no_symlinks(parent, "trait explanation cache ancestor")?;
    let lock_path = parent.join(format!(
        ".{}.lock",
        path.file_stem().unwrap_or("explanation")
    ));
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(
        &lock_path,
        "trait explanation lock",
    )?;
    let lock = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    if let Some(record) = read_trait_explanation(repo_root, trait_id, canonical_digest)? {
        return Ok((record, true));
    }
    let record = generate().map_err(|message| crate::Error::Usage { message })?;
    if record.trait_id != trait_id || record.canonical_digest != canonical_digest {
        return Err(crate::Error::Usage {
            message: "generated trait explanation identity does not match cache key".to_string(),
        });
    }
    write_trait_explanation(repo_root, &record)?;
    Ok((record, false))
}

fn trait_explanation_path(
    repo_root: &Utf8Path,
    canonical_digest: &str,
) -> crate::Result<Utf8PathBuf> {
    let digest = ctx_traits_core::digest::Digest::parse(canonical_digest).map_err(|error| {
        crate::Error::Usage {
            message: error.to_string(),
        }
    })?;
    let hex = digest
        .as_str()
        .strip_prefix("sha256:")
        .expect("validated digest prefix");
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_cache_root(&key)?
        .join("trait-explanations")
        .join(format!("{hex}.json")))
}

/// Resolve the cache root from an optional override or the invocation
/// repository's global per-repository cache root (P426). An explicit
/// override always wins and is never redirected through global state.
pub fn cache_root(
    repo_root: &Utf8Path,
    cache_root_override: Option<&str>,
) -> crate::Result<Utf8PathBuf> {
    match cache_root_override {
        Some(path) => Ok(Utf8PathBuf::from(path)),
        None => {
            let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
            Ok(crate::state::global_cache_root(&key)?.join("traits"))
        }
    }
}

/// Resolve the cache root to *read* generated cache metadata from: identical
/// to [`cache_root`], the single write/read root.
pub fn cache_read_root(
    repo_root: &Utf8Path,
    cache_root_override: Option<&str>,
) -> crate::Result<Utf8PathBuf> {
    cache_root(repo_root, cache_root_override)
}

/// Stored cache metadata as persisted on disk.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct StoredCacheMetadata {
    #[serde(default, rename = "artifact", skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<StoredCacheArtifact>,
}

/// Read stored cache metadata from the cache root.
///
/// Returns empty metadata if the cache root or metadata file does not exist.
pub fn read_cache_metadata(cache_root: &Utf8Path) -> crate::Result<StoredCacheMetadata> {
    let metadata_path = safe_metadata_path(cache_root)?;

    if !ensure_metadata_leaf_safe(&metadata_path)? {
        return Ok(StoredCacheMetadata::default());
    }

    let text = crate::read::read_text(&metadata_path)?;
    let mut metadata: StoredCacheMetadata =
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse cache metadata at {metadata_path}"),
            source,
        })?;

    metadata.artifacts.sort_by(compare_stored_artifacts);
    Ok(metadata)
}

/// Write cache metadata to the cache root, creating directories as needed.
pub fn write_cache_metadata(
    cache_root: &Utf8Path,
    metadata: &StoredCacheMetadata,
) -> crate::Result<()> {
    let metadata_path = safe_metadata_path(cache_root)?;

    std::fs::create_dir_all(cache_root).map_err(|e| crate::environment::Error::Filesystem {
        path: cache_root.to_string(),
        source: e,
    })?;

    ensure_existing_ancestors_safe(cache_root)?;
    ensure_metadata_leaf_safe(&metadata_path)?;

    let mut sorted = metadata.artifacts.clone();
    sorted.sort_by(compare_stored_artifacts);

    let json = serde_json::to_string_pretty(&StoredCacheMetadata { artifacts: sorted }).map_err(
        |source| crate::parse::Error::JsonSerialize {
            context: format!("serialize cache metadata at {metadata_path}"),
            source,
        },
    )?;

    ensure_metadata_leaf_safe(&metadata_path)?;
    crate::write::write_text(&metadata_path, &format!("{json}\n"))
}

/// Remove cache metadata file entirely (used by prune when all artifacts are pruned).
pub fn remove_cache_metadata(cache_root: &Utf8Path) -> crate::Result<()> {
    let metadata_path = safe_metadata_path(cache_root)?;

    if !ensure_metadata_leaf_safe(&metadata_path)? {
        return Ok(());
    }

    match std::fs::remove_file(&metadata_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: metadata_path.to_string(),
            source: e,
        }
        .into()),
    }
}

// ---------------------------------------------------------------------------
// Named build cache prune (opt-in, P342/P428)
// ---------------------------------------------------------------------------

/// Outcome of an opt-in named build-cache prune. Operational reporting only;
/// never a canonical/ledger artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCachePruneOutcome {
    /// The declared cache name considered.
    pub name: String,
    /// The exact directory considered: the invocation repository's global
    /// per-repository cache root's `build/<name>` subfamily.
    pub path: Utf8PathBuf,
    /// Whether that directory existed before this call.
    pub existed: bool,
    /// Bytes measured before any deletion. Missing targets measure as zero.
    pub byte_size: u64,
    /// Whether it was actually removed (always `false` under `dry_run`).
    pub removed: bool,
    /// Whether this was a dry-run (no filesystem mutation).
    pub dry_run: bool,
}

/// Prune one named build-artifact cache directory (the invocation
/// repository's global per-repository cache root's `build/<name>`
/// subfamily). This is a constrained, opt-in destructive path, kept entirely
/// separate from generated-cache metadata pruning: it refuses any target
/// that is not *exactly* the repo-owned `build/<name>` directory, and
/// rejects a symlinked ancestor chain or a symlinked/special leaf before
/// removing anything, reusing the same [`crate::path_safety`] guards the
/// seed/harvest copy paths use. Under `dry_run` it inspects and reports
/// without deleting. A missing directory is a no-op success, so the command
/// is safe to run repeatedly. `declared_names` is the effective
/// `[run.build-cache.<name>]` declaration set resolved once for the selected
/// repository (by the caller); `name` must be a member of it, or this
/// returns an actionable unknown-cache error before any path is touched on
/// disk. [`prune_declared_build_caches`] passes its own `declared_names`
/// straight through, so prune-one and prune-all always agree on the same
/// declaration set.
pub fn prune_named_build_cache(
    repo_root: &Utf8Path,
    name: &str,
    declared_names: &BTreeSet<String>,
    dry_run: bool,
) -> crate::Result<BuildCachePruneOutcome> {
    if !declared_names.contains(name) {
        return Err(crate::environment::Error::Filesystem {
            path: name.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "build cache {name:?} is not declared in [run.build-cache.<name>] for this repository"
                ),
            ),
        }
        .into());
    }
    let path = crate::layout::build_cache_root_path(repo_root, name)?;
    // Refuse anything that is not precisely the repo-owned build-cache dir,
    // so this destructive path can never be redirected at an arbitrary target.
    if !crate::layout::is_build_cache_root(&path, name) {
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to prune a path that is not exactly a repo-owned build-cache directory",
            ),
        }
        .into());
    }
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "build cache ancestor")?;
    }
    let existed =
        crate::path_safety::ensure_leaf_is_directory_or_absent(&path, "build cache directory")?;
    let byte_size = if existed { directory_size(&path)? } else { 0 };
    let removed = if existed && !dry_run {
        std::fs::remove_dir_all(path.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
        })?;
        true
    } else {
        false
    };
    Ok(BuildCachePruneOutcome {
        name: name.to_string(),
        path,
        existed,
        byte_size,
        removed,
        dry_run,
    })
}

/// Prune the historical singleton `build-target` cache. Unlike named caches,
/// it was never declared in `[run.build-cache.*]`; accepting it through the
/// declared-cache path would preserve the no-op this compatibility command is
/// meant to repair.
pub fn prune_build_target_cache(
    repo_root: &Utf8Path,
    dry_run: bool,
) -> crate::Result<BuildCachePruneOutcome> {
    let path = crate::layout::build_target_cache_root_path(repo_root)?;
    if !crate::layout::is_build_target_cache_root(&path) {
        return Err(crate::Error::Usage {
            message: format!(
                "refusing to prune a path that is not exactly the repo-owned build-target cache directory: {path}"
            ),
        });
    }
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "build target cache ancestor")?;
    }
    let existed = crate::path_safety::ensure_leaf_is_directory_or_absent(
        &path,
        "build target cache directory",
    )?;
    let byte_size = if existed { directory_size(&path)? } else { 0 };
    let removed = if existed && !dry_run {
        std::fs::remove_dir_all(path.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
        })?;
        true
    } else {
        false
    };
    Ok(BuildCachePruneOutcome {
        name: "build-target".to_string(),
        path,
        existed,
        byte_size,
        removed,
        dry_run,
    })
}

/// Recursively measure a declared cache without following symlinks. The root
/// has already passed the no-follow guard; nested symlinks contribute no bytes
/// and are never traversed by the deletion path either.
fn directory_size(path: &Utf8Path) -> crate::Result<u64> {
    let mut bytes = 0;
    for entry in std::fs::read_dir(path.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
    })? {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        })?;
        let child = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| crate::Error::Usage {
            message: format!("build cache path is not UTF-8: {}", path),
        })?;
        let metadata = std::fs::symlink_metadata(child.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: child.to_string(),
                source,
            }
        })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            bytes += directory_size(&child)?;
        } else if metadata.is_file() {
            bytes += metadata.len();
        }
    }
    Ok(bytes)
}

/// Prune every declared build cache, deriving its targets from
/// `declared_names` (`[run.build-cache.<name>]` keys) — never scanning or
/// deleting an undeclared sibling directory. Returns one outcome per name, in
/// `declared_names`' (deterministic `BTreeSet`) order.
pub fn prune_declared_build_caches(
    repo_root: &Utf8Path,
    declared_names: &BTreeSet<String>,
    dry_run: bool,
) -> crate::Result<Vec<BuildCachePruneOutcome>> {
    declared_names
        .iter()
        .map(|name| prune_named_build_cache(repo_root, name, declared_names, dry_run))
        .collect()
}

// ---------------------------------------------------------------------------
// Safety guards
// ---------------------------------------------------------------------------

/// Ensure the cache root is safe: no traversal, not under the trait source root,
/// not a symlink, and not a special file.
fn ensure_safe_cache_root(cache_root: &Utf8Path) -> crate::Result<()> {
    // Reject parent-dir traversal.
    for component in cache_root.components() {
        if matches!(component, Utf8Component::ParentDir) {
            return Err(crate::environment::Error::Filesystem {
                path: cache_root.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cache root must not contain '..' traversal",
                ),
            }
            .into());
        }
    }

    // Component-aware check: reject the source root or any path beneath it.
    let components: Vec<&str> = cache_root
        .components()
        .filter_map(|c| match c {
            Utf8Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    for window in components.windows(2) {
        if matches!(window, [".agents", "traits"] | [".ctx", "traits"]) {
            return Err(crate::environment::Error::Filesystem {
                path: cache_root.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cache root must not be under the trait source root",
                ),
            }
            .into());
        }
    }

    // Also reject if the last two components are exactly a protected trait root.
    if components.len() >= 2 {
        let n = components.len();
        if matches!(
            (components[n - 2], components[n - 1]),
            (".agents", "traits") | (".ctx", "traits")
        ) {
            return Err(crate::environment::Error::Filesystem {
                path: cache_root.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cache root must not be exactly the trait source root",
                ),
            }
            .into());
        }
    }

    crate::path_safety::ensure_no_symlink_ancestors(cache_root, "cache root")?;

    Ok(())
}

fn safe_metadata_path(cache_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    ensure_safe_cache_root(cache_root)?;
    ensure_existing_ancestors_safe(cache_root)?;

    let metadata_path = cache_root.join(METADATA_FILENAME);
    let relative = metadata_path.strip_prefix(cache_root).map_err(|_| {
        crate::environment::Error::Filesystem {
            path: metadata_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cache metadata path escaped cache root",
            ),
        }
    })?;

    if relative != Utf8Path::new(METADATA_FILENAME) {
        return Err(crate::environment::Error::Filesystem {
            path: metadata_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cache metadata path must be the expected metadata file under cache root",
            ),
        }
        .into());
    }

    Ok(metadata_path)
}

fn ensure_existing_ancestors_safe(cache_root: &Utf8Path) -> crate::Result<()> {
    crate::path_safety::ensure_no_symlink_ancestors(cache_root, "cache root ancestor")
}

/// Check if the metadata leaf exists and is a regular file. Returns false for nonexistent.
fn ensure_metadata_leaf_safe(path: &Utf8Path) -> crate::Result<bool> {
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, "cache metadata file")
}

fn compare_stored_artifacts(a: &StoredCacheArtifact, b: &StoredCacheArtifact) -> Ordering {
    a.artifact_id
        .cmp(&b.artifact_id)
        .then_with(|| a.key.trait_id.cmp(&b.key.trait_id))
}
