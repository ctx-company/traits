//! Export filesystem control.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

pub use ctx_traits_core::export::{Format, Identity, Marker, OwnershipKey};

pub mod control;
pub mod fs;
mod host;
mod policy;

#[derive(Debug, Error)]
pub enum Error {
    #[error("filesystem error at {path}: {reason}")]
    UnsafePath {
        path: Utf8PathBuf,
        reason: UnsafePathReason,
    },
    #[error("filesystem error at {path}: export target escapes output root")]
    EscapesOutputRoot { path: Utf8PathBuf },
    #[error("filesystem error at {path}: refusing to write export inside trait source directory")]
    InsideTraitSource { path: Utf8PathBuf },
    #[error("filesystem error at {path}: export target has no parent directory")]
    TargetWithoutParent { path: Utf8PathBuf },
    #[error(
        "filesystem error at {path}: export parent path contains a symlink; use a non-symlinked path (on macOS, the resolved /private/... path)"
    )]
    SymlinkAncestor { path: Utf8PathBuf },
    #[error("filesystem error at {path}: export parent path is not a directory")]
    ParentNotDirectory { path: Utf8PathBuf },
    #[error("filesystem error at {path}: export target leaf is a symlink")]
    LeafSymlink { path: Utf8PathBuf },
    #[error("filesystem error at {path}: export target exists and is not a regular file")]
    LeafNotRegularFile { path: Utf8PathBuf },
    #[error(
        "filesystem error at {path}: existing export target is unmanaged and will not be overwritten"
    )]
    UnmanagedTarget { path: Utf8PathBuf },
    #[error(
        "filesystem error at {path}: existing generated export belongs to a different trait or profile"
    )]
    OwnershipMismatch { path: Utf8PathBuf, existing: Marker },
    #[error("filesystem error at {path}: managed target does not exist")]
    Missing { path: Utf8PathBuf },
    #[error(
        "filesystem error at {path}: managed target content has been locally modified since it was placed"
    )]
    LocallyModified { path: Utf8PathBuf },
    #[error("filesystem error at {path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafePathReason {
    UnsafeShape,
    WindowsDrivePrefix,
    ParentTraversal,
    EmptySegment,
}

impl std::fmt::Display for UnsafePathReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsafeShape => "export path contains an unsafe path shape",
            Self::WindowsDrivePrefix => "export path must not use a Windows drive prefix",
            Self::ParentTraversal => "export path must not contain '..' traversal",
            Self::EmptySegment => "export path must not contain empty path segments",
        })
    }
}

impl Error {
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::UnsafePath { path, .. }
            | Self::EscapesOutputRoot { path }
            | Self::InsideTraitSource { path }
            | Self::TargetWithoutParent { path }
            | Self::SymlinkAncestor { path }
            | Self::ParentNotDirectory { path }
            | Self::LeafSymlink { path }
            | Self::LeafNotRegularFile { path }
            | Self::UnmanagedTarget { path }
            | Self::OwnershipMismatch { path, .. }
            | Self::Missing { path }
            | Self::LocallyModified { path }
            | Self::Io { path, .. } => path,
        }
    }
    pub fn kind(&self) -> std::io::ErrorKind {
        match self {
            Self::UnsafePath { .. }
            | Self::TargetWithoutParent { .. }
            | Self::ParentNotDirectory { .. }
            | Self::LeafNotRegularFile { .. } => std::io::ErrorKind::InvalidInput,
            Self::EscapesOutputRoot { .. }
            | Self::InsideTraitSource { .. }
            | Self::SymlinkAncestor { .. }
            | Self::LeafSymlink { .. } => std::io::ErrorKind::PermissionDenied,
            Self::UnmanagedTarget { .. }
            | Self::OwnershipMismatch { .. }
            | Self::LocallyModified { .. } => std::io::ErrorKind::AlreadyExists,
            Self::Missing { .. } => std::io::ErrorKind::NotFound,
            Self::Io { source, .. } => source.kind(),
        }
    }
}

/// Validate a relative host-placement path template result: reuses the
/// export write policy's path-shape rules (no absolute paths, no `..`
/// traversal, no backslashes, no empty segments) so host placement and
/// export share exactly one path-shape validator.
pub fn validate_relative_path(path: &Utf8Path) -> Result<(), Error> {
    policy::validate_path(path)
}

/// The read-only outcome of probing a managed (or companion) path's current
/// state, shared by [`verify_removable`] and `host status`. A leaf artifact
/// carries the `> GENERATED FILE ...` marker and is checked by trait/
/// ownership identity (`identity = Some(..)`); a companion resource file
/// cannot carry a marker without changing its bytes (breaking its digest),
/// so its ownership is digest-keyed only (`identity = None`) — "absent, or
/// bytes still match what was recorded".
#[derive(Debug, Clone)]
pub(crate) enum ManagedState {
    Missing,
    LeafSymlink,
    LeafNotRegularFile,
    UnmanagedTarget,
    OwnershipMismatch(Marker),
    LocallyModified,
    Ok(Vec<u8>),
}

/// Probe `output_root`/`relative_target`'s current on-disk state without
/// mutating anything: whether it is missing, a symlink, a non-regular file,
/// unmanaged, owned by a different trait/ownership, locally modified since
/// it was placed, or matches exactly. Returns the resolved absolute target
/// path alongside the state so a caller can report or act on it.
pub(crate) fn inspect_managed(
    output_root: &Utf8Path,
    relative_target: &Utf8Path,
    identity: Option<&Identity>,
    expected_digest: &ctx_traits_core::digest::Digest,
) -> Result<(Utf8PathBuf, ManagedState), Error> {
    policy::validate_path(output_root)?;
    policy::validate_path(relative_target)?;
    let target = output_root.join(relative_target);
    policy::validate_path(&target)?;
    policy::validate_target(output_root, &target)?;

    match std::fs::symlink_metadata(target.as_std_path()) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Ok((target, ManagedState::LeafSymlink));
        }
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok((target, ManagedState::LeafNotRegularFile)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((target, ManagedState::Missing));
        }
        Err(source) => {
            return Err(Error::Io {
                path: target,
                source,
            });
        }
    }

    match identity {
        Some(identity) => {
            let text =
                std::fs::read_to_string(target.as_std_path()).map_err(|source| Error::Io {
                    path: target.clone(),
                    source,
                })?;
            let Some(marker) = Marker::parse(&text) else {
                return Ok((target, ManagedState::UnmanagedTarget));
            };
            if !marker.is_owned_by(identity) {
                return Ok((target, ManagedState::OwnershipMismatch(marker)));
            }
            if &ctx_traits_core::digest::Digest::from_bytes(text.as_bytes()) != expected_digest {
                return Ok((target, ManagedState::LocallyModified));
            }
            Ok((target, ManagedState::Ok(text.into_bytes())))
        }
        None => {
            let bytes = std::fs::read(target.as_std_path()).map_err(|source| Error::Io {
                path: target.clone(),
                source,
            })?;
            if &ctx_traits_core::digest::Digest::from_bytes(&bytes) != expected_digest {
                return Ok((target, ManagedState::LocallyModified));
            }
            Ok((target, ManagedState::Ok(bytes)))
        }
    }
}

/// Verify (without deleting) that `output_root`/`relative_target` is a
/// safe, still-owned, digest-matching managed artifact: refuses a missing,
/// symlinked, non-regular, unmanaged, foreign (different trait/ownership),
/// or locally modified (content digest no longer matches `expected_digest`)
/// target — the same checks [`export::policy`] uses to decide whether a
/// write may overwrite an existing managed artifact, applied in reverse for
/// deletion. `identity` is `Some` for a marker-owned leaf artifact, `None`
/// for a digest-owned companion resource file (which cannot carry the
/// marker without changing its bytes — see [`write_companion`]). Returns the
/// resolved absolute target path and its current bytes, so a caller
/// performing a multi-path transactional removal can preflight every path
/// before deleting any of them, and can restore exactly these bytes if a
/// later step in that transaction fails.
pub(crate) fn verify_removable(
    output_root: &Utf8Path,
    relative_target: &Utf8Path,
    identity: Option<&Identity>,
    expected_digest: &ctx_traits_core::digest::Digest,
) -> Result<(Utf8PathBuf, Vec<u8>), Error> {
    let (target, state) = inspect_managed(output_root, relative_target, identity, expected_digest)?;
    match state {
        ManagedState::Ok(bytes) => Ok((target, bytes)),
        ManagedState::Missing => Err(Error::Missing { path: target }),
        ManagedState::LeafSymlink => Err(Error::LeafSymlink { path: target }),
        ManagedState::LeafNotRegularFile => Err(Error::LeafNotRegularFile { path: target }),
        ManagedState::UnmanagedTarget => Err(Error::UnmanagedTarget { path: target }),
        ManagedState::OwnershipMismatch(marker) => Err(Error::OwnershipMismatch {
            path: target,
            existing: marker,
        }),
        ManagedState::LocallyModified => Err(Error::LocallyModified { path: target }),
    }
}

/// Remove a managed host-placement artifact at `output_root`/`relative_target`.
///
/// Refuses to remove a missing, symlinked, non-regular, unmanaged, foreign
/// (different trait/ownership), or locally modified (content digest no
/// longer matches `expected_digest`) target — see [`verify_removable`].
pub fn remove_managed(
    output_root: &Utf8Path,
    relative_target: &Utf8Path,
    identity: &Identity,
    expected_digest: &ctx_traits_core::digest::Digest,
) -> Result<(), Error> {
    let (target, _bytes) = verify_removable(
        output_root,
        relative_target,
        Some(identity),
        expected_digest,
    )?;
    std::fs::remove_file(target.as_std_path()).map_err(|source| Error::Io {
        path: target.clone(),
        source,
    })?;
    prune_empty_ancestors(output_root, target.parent().unwrap_or(output_root));
    Ok(())
}

/// Remove now-empty directories created for a placement, walking upward from
/// `start` and stopping at (never removing) `root` itself or the first
/// non-empty directory. Best-effort: an inability to read or remove a
/// directory silently stops the walk rather than failing the removal that
/// already succeeded.
pub(crate) fn prune_empty_ancestors(root: &Utf8Path, start: &Utf8Path) {
    let mut current = start;
    while current != root && current.starts_with(root) {
        match std::fs::read_dir(current.as_std_path()) {
            Ok(entries) => {
                if entries.count() != 0 {
                    break;
                }
                if std::fs::remove_dir(current.as_std_path()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
}

/// Infer the repository root from either a protocol or CDK authoring trait path.
pub fn infer_repo_root_from_trait_file(trait_file: &Utf8Path) -> &Utf8Path {
    let Some(trait_root) = crate::layout::package_root_for_manifest(trait_file) else {
        return trait_file;
    };
    // `modules/core/builtins/traits/<id>`: the first-party meta-trait
    // packages moved here still need `@ctx-traits/cdk` resolved from the
    // real repo-root `node_modules` when rebuilt via `ctx traits build`.
    if let Some(repo_root) = crate::layout::builtin_trait_package_repo_root(trait_root) {
        return repo_root;
    }
    let Some(parent) = trait_root.parent() else {
        return trait_root;
    };
    // 0179 (formerly P569's `packages/`): packages live at
    // `<repo>/.ctx/traits/authored/<id>`; a checkout that predates the move
    // still has `<repo>/.ctx/traits/<id>`. Walking a fixed number of levels
    // silently returned the PACKAGE root as the repo root on the new layout,
    // which made a default export target resolve inside the trait's own
    // source directory and get refused.
    let traits_dir = if parent.file_name() == Some("authored") {
        match parent.parent() {
            Some(dir) => dir,
            None => return trait_root,
        }
    } else {
        parent
    };
    if traits_dir.file_name() != Some("traits") {
        return trait_root;
    }
    let Some(tree_dir) = traits_dir.parent() else {
        return trait_root;
    };
    match tree_dir.file_name() {
        Some(".ctx") => tree_dir.parent().map_or(trait_root, |parent| parent),
        _ => trait_root,
    }
}
