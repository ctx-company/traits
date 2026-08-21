//! TOML lockfile IO for package-local `trait.lock` evidence.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
pub use ctx_traits_core::lockfile::{
    LockDependencyDigests, LockDependencyEntry, LockDigests, LockEntry as LockTraitEntry,
    LockExport as LockExportEntry, LockProjection, Lockfile as Document, Metadata,
    ProjectionDriftStatus,
};

/// Result of attempting to update lock export evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockUpdateResult {
    /// The matching trait entry was updated and written.
    Updated { path: String },
    /// No lockfile exists yet.
    SkippedMissingLock { path: String },
    /// The lockfile exists but has no matching trait entry.
    SkippedMissingEntry { path: String, trait_id: String },
}

/// Return the expected lockfile path for a trait package root.
pub fn lockfile_path(trait_root: &Utf8Path) -> Utf8PathBuf {
    crate::layout::package_lock_path(trait_root)
}

/// Read a package-local `trait.lock` if present.
pub fn read_lockfile(trait_root: &Utf8Path) -> crate::Result<Option<Document>> {
    let path = lockfile_path(trait_root);
    if !validate_existing_lockfile_leaf(&path, false)? {
        return Ok(None);
    }

    // Best-effort TOCTOU guard: repeat the no-follow leaf check immediately
    // before reading. std cannot open a file with portable O_NOFOLLOW.
    if !validate_existing_lockfile_leaf(&path, false)? {
        return Ok(None);
    }

    let text =
        std::fs::read_to_string(&path).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?;

    let document = match toml::from_str(&text) {
        Ok(document) => document,
        Err(toml_source) => match serde_json::from_str(&text) {
            Ok(import) => Document {
                import: Some(import),
                ..Document::default()
            },
            Err(_) => {
                return Err(crate::parse::Error::TomlDecode {
                    context: format!("decode trait.lock at {path}"),
                    source: toml_source,
                }
                .into());
            }
        },
    };

    Ok(Some(document))
}

/// Write a package-local `trait.lock` in deterministic TOML order.
pub fn write_lockfile(trait_root: &Utf8Path, document: &mut Document) -> crate::Result<()> {
    if crate::layout::is_builtin_store_package_root(trait_root) {
        return Err(crate::environment::Error::Filesystem {
            path: trait_root.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "refusing to write trait.lock inside the built-in trait store; built-in packages are read-only",
            ),
        }
        .into());
    }
    document.sort_for_output();
    let path = lockfile_path(trait_root);
    let text =
        toml::to_string_pretty(document).map_err(|source| crate::parse::Error::TomlEncode {
            context: format!("encode trait.lock at {path}"),
            source,
        })?;

    ensure_lockfile_parent(&path)?;
    validate_existing_lockfile_leaf(&path, false)?;
    // Best-effort TOCTOU guard immediately before write.
    validate_existing_lockfile_leaf(&path, false)?;

    Ok(
        std::fs::write(&path, text).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?,
    )
}

fn ensure_lockfile_parent(path: &Utf8Path) -> crate::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "lockfile has no parent"),
        })?;
    let root = parent
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lockfile parent has no repository root",
            ),
        })?;
    // A relative package root can have an empty ancestor; validate the current
    // directory instead.
    if root.as_str().is_empty() {
        validate_existing_directory(Utf8Path::new("."))?;
    } else {
        validate_existing_directory(root)?;
    }
    match std::fs::symlink_metadata(parent) {
        Ok(_) => validate_existing_directory(parent),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(parent).map_err(|e| crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source: e,
            })?;
            validate_existing_directory(parent)
        }
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: parent.to_string(),
            source,
        }
        .into()),
    }
}

/// Update one export entry if a lockfile and matching trait entry exist.
pub fn update_export_evidence(
    trait_root: &Utf8Path,
    trait_id: &str,
    variant: Option<&str>,
    export: LockExportEntry,
) -> crate::Result<LockUpdateResult> {
    let path = lockfile_path(trait_root);
    let Some(mut document) = read_lockfile(trait_root)? else {
        return Ok(LockUpdateResult::SkippedMissingLock {
            path: path.to_string(),
        });
    };

    let Some(entry) = document.trait_entry_mut(trait_id, variant) else {
        return Ok(LockUpdateResult::SkippedMissingEntry {
            path: path.to_string(),
            trait_id: trait_id.to_string(),
        });
    };

    match entry
        .exports
        .iter_mut()
        .find(|existing| existing.target == export.target)
    {
        Some(existing) => *existing = export,
        None => entry.exports.push(export),
    }

    write_lockfile(trait_root, &mut document)?;
    Ok(LockUpdateResult::Updated {
        path: path.to_string(),
    })
}

/// Record projection evidence, creating the lockfile and trait entry as
/// needed: projections are first-class evidence a package may carry before
/// any sync/export has produced other lock entries.
pub fn update_projection_evidence(
    trait_root: &Utf8Path,
    trait_id: &str,
    variant: Option<&str>,
    projection: LockProjection,
) -> crate::Result<LockUpdateResult> {
    let path = lockfile_path(trait_root);
    let mut document = read_lockfile(trait_root)?.unwrap_or_else(|| {
        Document::new(Metadata {
            schema_version: Some("0.1".to_string()),
            generated_by: Some("ctx traits internal export".to_string()),
            generated_at: Some("not-recorded".to_string()),
        })
    });
    if document.trait_entry(trait_id, variant).is_none() {
        document.insert(
            trait_id,
            LockTraitEntry {
                variant: variant.map(str::to_string),
                ..LockTraitEntry::default()
            },
        );
    }
    if let Some(entry) = document.trait_entry_mut(trait_id, variant) {
        entry.upsert_projection(projection);
    }
    write_lockfile(trait_root, &mut document)?;
    Ok(LockUpdateResult::Updated {
        path: path.to_string(),
    })
}

/// Update one eval-result entry if a lockfile and matching trait entry exist.
pub fn update_eval_result_evidence(
    trait_root: &Utf8Path,
    trait_id: &str,
    variant: Option<&str>,
    result: ctx_traits_core::r#trait::EvalResult,
) -> crate::Result<LockUpdateResult> {
    let path = lockfile_path(trait_root);
    let Some(mut document) = read_lockfile(trait_root)? else {
        return Ok(LockUpdateResult::SkippedMissingLock {
            path: path.to_string(),
        });
    };

    let Some(entry) = document.trait_entry_mut(trait_id, variant) else {
        return Ok(LockUpdateResult::SkippedMissingEntry {
            path: path.to_string(),
            trait_id: trait_id.to_string(),
        });
    };

    match entry
        .eval_results
        .iter_mut()
        .find(|existing| existing.eval_id == result.eval_id && existing.profile == result.profile)
    {
        Some(existing) => *existing = result,
        None => entry.eval_results.push(result),
    }

    write_lockfile(trait_root, &mut document)?;
    Ok(LockUpdateResult::Updated {
        path: path.to_string(),
    })
}

/// Digest a locked export file if it exists and is a regular non-symlink file.
pub fn digest_locked_export(
    repo_root: &Utf8Path,
    export: &LockExportEntry,
) -> crate::Result<Option<Digest>> {
    let path = locked_export_path(repo_root, &export.path);
    if !validate_existing_regular_leaf(&path, false)? {
        return Ok(None);
    }

    // Best-effort TOCTOU guard immediately before reading the locked export.
    if !validate_existing_regular_leaf(&path, false)? {
        return Ok(None);
    }

    let bytes = std::fs::read(&path).map_err(|e| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: e,
    })?;

    Ok(Some(Digest::from_bytes(&bytes)))
}

fn validate_existing_lockfile_leaf(path: &Utf8Path, required: bool) -> crate::Result<bool> {
    validate_safe_path_shape(path)?;
    validate_existing_parent_dirs(path)?;
    validate_existing_regular_leaf(path, required)
}

fn validate_existing_regular_leaf(path: &Utf8Path, required: bool) -> crate::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return filesystem_error(
                    path,
                    std::io::ErrorKind::PermissionDenied,
                    "lock evidence path is a symlink",
                );
            }
            if !metadata.file_type().is_file() {
                return filesystem_error(
                    path,
                    std::io::ErrorKind::InvalidInput,
                    "lock evidence path is not a regular file",
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => filesystem_error(
            path,
            std::io::ErrorKind::NotFound,
            "lockfile disappeared before write",
        ),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}

fn validate_existing_parent_dirs(path: &Utf8Path) -> crate::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock evidence path has no parent directory",
            ),
        })?;

    let mut ancestors = Vec::new();
    let mut current = Some(parent);
    while let Some(path) = current {
        ancestors.push(path);
        current = path.parent();
    }

    let mut skipped_absolute_alias = false;
    for ancestor in ancestors.into_iter().rev() {
        if ancestor.as_str().is_empty() || ancestor == Utf8Path::new(".") {
            continue;
        }
        // Tolerate the OS root alias (macOS `/tmp -> /private/tmp`, `/var -> /private/var`);
        // matches `io::write`. Every deeper, user-writable segment is still checked.
        if path.is_absolute()
            && !skipped_absolute_alias
            && ancestor.parent().is_some_and(|p| p.as_str() == "/")
        {
            skipped_absolute_alias = true;
            continue;
        }
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return filesystem_error(
                        ancestor,
                        std::io::ErrorKind::PermissionDenied,
                        "lock evidence parent path contains a symlink; use a non-symlinked path (on macOS, the resolved /private/... path)",
                    );
                }
                if !metadata.file_type().is_dir() {
                    return filesystem_error(
                        ancestor,
                        std::io::ErrorKind::InvalidInput,
                        "lock evidence parent path is not a directory",
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: ancestor.to_string(),
                    source: e,
                }
                .into());
            }
        }
    }
    Ok(())
}

fn validate_existing_directory(path: &Utf8Path) -> crate::Result<()> {
    validate_existing_parent_dirs(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => filesystem_error(
            path,
            std::io::ErrorKind::PermissionDenied,
            "lock evidence directory is a symlink",
        ),
        Ok(metadata) if !metadata.file_type().is_dir() => filesystem_error(
            path,
            std::io::ErrorKind::InvalidInput,
            "lock evidence directory is not a directory",
        ),
        Ok(_) => Ok(()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

fn validate_safe_path_shape(path: &Utf8Path) -> crate::Result<()> {
    let raw = path.as_str();
    if raw.is_empty() || raw.contains('\\') || raw.contains("//") {
        return filesystem_error(
            path,
            std::io::ErrorKind::InvalidInput,
            "lock evidence path contains an unsafe path shape",
        );
    }
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return filesystem_error(
            path,
            std::io::ErrorKind::InvalidInput,
            "lock evidence path must not use a Windows drive prefix",
        );
    }
    for component in path.components() {
        if matches!(component, camino::Utf8Component::ParentDir) {
            return filesystem_error(
                path,
                std::io::ErrorKind::InvalidInput,
                "lock evidence path must not contain '..' traversal",
            );
        }
    }
    Ok(())
}

fn filesystem_error<T>(
    path: &Utf8Path,
    kind: std::io::ErrorKind,
    message: &str,
) -> crate::Result<T> {
    Err(crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(kind, message),
    }
    .into())
}

fn locked_export_path(repo_root: &Utf8Path, export_path: &str) -> Utf8PathBuf {
    let path = Utf8Path::new(export_path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        repo_root.join(path)
    }
}
