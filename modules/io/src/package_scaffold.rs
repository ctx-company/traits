//! Shared package-root claiming and exclusive file creation primitives.
//!
//! Both `ctx traits init` ([`crate::init`]) and `ctx traits new`
//! (template-backed scaffolding, `modules/cli/src/app/new.rs`) need the same
//! safety guarantee before populating a brand-new `.ctx/traits/<id>/`
//! package: claim the root directory atomically, then write every authored
//! file through an exclusive create that never truncates or clobbers an
//! existing file. Centralized here so both callers share exactly one
//! implementation instead of two copies of the same race-free logic.

use camino::Utf8Path;

/// Whether an exclusive file create actually created the file or found one
/// already there. Backed by an exclusive (`O_CREAT|O_EXCL`-equivalent) open,
/// so the check and the write are one atomic filesystem operation — no
/// existence-check-then-write race, and no truncation of a file created
/// concurrently.
pub enum CreateOutcome {
    Created,
    AlreadyExists,
}

/// Atomically claim `root` as a genuinely new directory: `Ok(true)` when
/// this call created the directory (and therefore uniquely owns populating
/// it), `Ok(false)` when the root already existed in any form. The claim and
/// the "did it already exist" check are one `mkdir`, so no other process can
/// observe or race an empty root between checking and creating it, and an
/// existing non-directory (e.g. a planted symlink) is rejected rather than
/// silently treated as absent.
pub fn claim_root(root: &Utf8Path, label: &str) -> crate::Result<bool> {
    match std::fs::create_dir(root.as_std_path()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            crate::path_safety::ensure_leaf_is_directory_or_absent(root, label)?;
            Ok(false)
        }
        Err(e) => Err(fs_write_err(root, e)),
    }
}

/// Ensure `path` exists as a directory, creating it (and parents) through
/// the no-symlink-ancestor-safe path if it is absent. Returns `true` when
/// this call created the directory, `false` when it already existed.
pub fn ensure_dir(path: &Utf8Path, label: &str) -> crate::Result<bool> {
    if crate::path_safety::ensure_leaf_is_directory_or_absent(path, label)? {
        return Ok(false);
    }
    crate::path_safety::create_dir_all_no_symlinks(path, label)?;
    Ok(true)
}

/// Exclusively create `path` with `content`: [`CreateOutcome::AlreadyExists`]
/// when a file is already there (left untouched — never truncated), or
/// [`CreateOutcome::Created`] after a fresh write. The create-and-write is
/// one atomic open, so no other process can observe or race an empty/partial
/// file, and no existing file is ever clobbered. Parent directories are
/// created through the no-symlink-ancestor-safe path first.
pub fn create_new_file(
    path: &Utf8Path,
    label: &str,
    content: &str,
) -> crate::Result<CreateOutcome> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    if !parent.as_str().is_empty() && parent.as_str() != "." {
        crate::path_safety::create_dir_all_no_symlinks(parent, label)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.as_std_path())
    {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(content.as_bytes())
                .map_err(|e| fs_write_err(path, e))?;
            Ok(CreateOutcome::Created)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, label)? {
                Ok(CreateOutcome::AlreadyExists)
            } else {
                Err(fs_err(
                    path,
                    format!("{label} existed during creation but is gone now; retry"),
                ))
            }
        }
        Err(e) => Err(fs_write_err(path, e)),
    }
}

pub(crate) fn fs_err(path: &Utf8Path, message: impl Into<String>) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()),
    }
    .into()
}

pub(crate) fn fs_write_err(path: &Utf8Path, source: std::io::Error) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    }
    .into()
}
