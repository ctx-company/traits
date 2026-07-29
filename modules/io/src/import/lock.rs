use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::import::plan::{
    ArtifactClassification, ArtifactContent, IMPORT_COMMAND_VERSION, TraitLock, TraitLockArtifact,
    TraitLockSnapshot, TraitLockSnapshotMetadata,
};

use super::support::{
    MAX_INLINE_SIZE, encode_base64, filesystem_error, guess_media, is_likely_utf8,
    is_safe_relative_path, relative_import_path,
};

/// Read package-local import evidence from the shared `trait.lock` document.
///
/// Returns `Ok(None)` if no lock exists.
pub fn read_trait_lock(trait_root: &Utf8Path) -> crate::Result<Option<TraitLock>> {
    Ok(crate::lockfile::read_lockfile(trait_root)?.and_then(|document| document.import))
}

/// Update package-local import evidence without replacing other lock layers.
pub fn write_trait_lock(trait_root: &Utf8Path, lock: &TraitLock) -> crate::Result<()> {
    let mut document = crate::lockfile::read_lockfile(trait_root)?.unwrap_or_default();
    document.import = Some(lock.clone());
    crate::lockfile::write_lockfile(trait_root, &mut document)
}

/// Build an artifact snapshot from a source path (file or directory).
///
/// Returns the snapshot and any warnings (e.g., blocked files).
pub fn build_artifact_snapshot(
    source: &Utf8Path,
    source_profile: &str,
) -> crate::Result<(TraitLockSnapshot, Vec<String>)> {
    build_artifact_snapshot_with_locator(source, source_profile, source.as_str())
}

/// Build an artifact snapshot with an explicit source locator.
///
/// The `source_locator` is stored in snapshot metadata so that refresh can
/// re-read the original source without guessing from artifact entries.
pub fn build_artifact_snapshot_with_locator(
    source: &Utf8Path,
    source_profile: &str,
    source_locator: &str,
) -> crate::Result<(TraitLockSnapshot, Vec<String>)> {
    let mut artifacts = Vec::new();
    let mut warnings = Vec::new();

    let metadata =
        fs::symlink_metadata(source).map_err(|e| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: e,
        })?;

    if metadata.file_type().is_symlink() {
        return filesystem_error(source, "import source must not be a symlink");
    }

    if metadata.file_type().is_file() {
        let file_name =
            source
                .file_name()
                .ok_or_else(|| crate::environment::Error::Filesystem {
                    path: source.to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "import source file has no file name component",
                    ),
                })?;
        let (artifact, warning) = build_single_artifact(file_name, source)?;
        artifacts.push(artifact);
        if let Some(w) = warning {
            warnings.push(w);
        }
    } else if metadata.file_type().is_dir() {
        collect_artifacts(source, source, &mut artifacts, &mut warnings)?;
    } else {
        return filesystem_error(source, "import source must be a regular file or directory");
    }

    if artifacts.is_empty() {
        return filesystem_error(
            source,
            "artifact snapshot is empty; source must contain at least one regular file",
        );
    }

    artifacts.sort_by(|a, b| a.normalized_path.cmp(&b.normalized_path));

    let snapshot_digest = ctx_traits_core::import::plan::source_set_digest(&artifacts);
    let snapshot = TraitLockSnapshot {
        snapshot_digest,
        source_profile: source_profile.to_string(),
        import_command_version: IMPORT_COMMAND_VERSION.to_string(),
        canonical_output_digest: None,
        artifacts,
        metadata: TraitLockSnapshotMetadata {
            source_locator: Some(source_locator.to_string()),
            frontmatter_mapping: None,
            graph_digest: None,
            resource_mappings: Vec::new(),
            remote_source: None,
        },
    };
    Ok((snapshot, warnings))
}

fn collect_artifacts(
    base: &Utf8Path,
    current: &Utf8Path,
    artifacts: &mut Vec<TraitLockArtifact>,
    warnings: &mut Vec<String>,
) -> crate::Result<()> {
    let metadata = match fs::symlink_metadata(current) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(format!("missing source path: {current}"));
            return Ok(());
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: current.to_string(),
                source: e,
            }
            .into());
        }
    };

    if metadata.file_type().is_symlink() {
        let rel = relative_import_path(base, current).unwrap_or_else(|_| current.to_string());
        warnings.push(format!("symlink skipped: {rel}"));
        artifacts.push(TraitLockArtifact {
            normalized_path: rel,
            original_source_uri: Some(current.to_string()),
            byte_digest: Digest::source("symlink"),
            byte_size: 0,
            file_classification: ArtifactClassification::Special,
            media_guess: "unknown".to_string(),
            warnings: vec!["path is a symlink".to_string()],
            participated_in_conversion: false,
            content: ArtifactContent::Blocked {
                reason: "symlink paths cannot be embedded reproducibly".to_string(),
            },
        });
        return Ok(());
    }

    if metadata.file_type().is_dir() {
        let reader = match fs::read_dir(current) {
            Ok(r) => r,
            Err(_) => {
                warnings.push(format!("unreadable directory: {current}"));
                return Ok(());
            }
        };
        for entry in reader {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = match Utf8PathBuf::from_path_buf(entry.path()) {
                Ok(p) => p,
                Err(_) => continue,
            };
            collect_artifacts(base, &path, artifacts, warnings)?;
        }
        return Ok(());
    }

    if metadata.file_type().is_file() {
        let rel = relative_import_path(base, current)?;
        if rel.is_empty() {
            return filesystem_error(
                current,
                "artifact normalized path is empty; internal caller bug",
            );
        }
        if !is_safe_relative_path(&rel) {
            warnings.push(format!("unsafe relative path: {rel}"));
            return Ok(());
        }
        let (artifact, warning) = build_single_artifact(&rel, current)?;
        artifacts.push(artifact);
        if let Some(w) = warning {
            warnings.push(w);
        }
        return Ok(());
    }

    let rel = relative_import_path(base, current).unwrap_or_else(|_| current.to_string());
    warnings.push(format!("special file skipped: {rel}"));
    artifacts.push(TraitLockArtifact {
        normalized_path: rel,
        original_source_uri: Some(current.to_string()),
        byte_digest: Digest::source("special"),
        byte_size: 0,
        file_classification: ArtifactClassification::Special,
        media_guess: "unknown".to_string(),
        warnings: vec!["path is a special file".to_string()],
        participated_in_conversion: false,
        content: ArtifactContent::Blocked {
            reason: "special files cannot be embedded reproducibly".to_string(),
        },
    });
    Ok(())
}

fn build_single_artifact(
    normalized_path: &str,
    full_path: &Utf8Path,
) -> crate::Result<(TraitLockArtifact, Option<String>)> {
    let bytes = fs::read(full_path).map_err(|e| crate::environment::Error::Filesystem {
        path: full_path.to_string(),
        source: e,
    })?;
    let byte_size = bytes.len() as u64;
    let byte_digest = Digest::from_bytes(&bytes);

    let is_text = is_likely_utf8(&bytes);
    let media_guess = guess_media(normalized_path, is_text);

    let (classification, content, warning) = if !is_text {
        if byte_size > MAX_INLINE_SIZE {
            (
                ArtifactClassification::Binary,
                ArtifactContent::Blocked {
                    reason: format!("binary file exceeds inline size limit ({byte_size} bytes)"),
                },
                Some(format!("blocked binary file too large: {normalized_path}")),
            )
        } else {
            (
                ArtifactClassification::Binary,
                ArtifactContent::Base64 {
                    data: encode_base64(&bytes),
                },
                None,
            )
        }
    } else if byte_size > MAX_INLINE_SIZE {
        (
            ArtifactClassification::Text,
            ArtifactContent::Blocked {
                reason: format!("text file exceeds inline size limit ({byte_size} bytes)"),
            },
            Some(format!("blocked text file too large: {normalized_path}")),
        )
    } else {
        let text = String::from_utf8_lossy(&bytes).to_string();
        (
            ArtifactClassification::Text,
            ArtifactContent::Text { text },
            None,
        )
    };

    let artifact = TraitLockArtifact {
        normalized_path: normalized_path.to_string(),
        original_source_uri: Some(full_path.to_string()),
        byte_digest,
        byte_size,
        file_classification: classification,
        media_guess,
        warnings: Vec::new(),
        participated_in_conversion: normalized_path == "SKILL.md",
        content,
    };
    Ok((artifact, warning))
}
