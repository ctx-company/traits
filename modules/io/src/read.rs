//! Filesystem read operations.
//!
//! All reads go through this module so the CLI edge never touches
//! `std::fs::read_to_string` directly. This keeps the hexagonal boundary
//! explicit: the app layer orchestrates, the IO layer owns the filesystem.

use camino::Utf8Path;

/// A path inside the built-in trait store (the active global
/// `.../cache/<repo-key>/builtin-traits/...` shape or the legacy
/// `.ctx/cache/builtin-traits/...` shape) gets an extra no-follow guard
/// before any read: every ancestor directory
/// must be plain (never a symlink) and the leaf itself must be a plain
/// regular file (or absent), never a symlink. Ordinary paths are unaffected.
/// Shared by [`read_text`] and [`read_optional_text`] so neither one is a
/// second, un-guarded way to read out of the store.
fn guard_builtin_store_read(path: &Utf8Path) -> crate::Result<()> {
    if !crate::layout::is_within_builtin_store(path) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "built-in trait store directory")?;
    }
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, "built-in trait store file")?;
    Ok(())
}

/// Read UTF-8 text content from a file path.
pub fn read_text(path: &Utf8Path) -> crate::Result<String> {
    guard_builtin_store_read(path)?;
    Ok(
        std::fs::read_to_string(path).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?,
    )
}

/// Read UTF-8 text content when a file exists.
pub fn read_optional_text(path: &Utf8Path) -> crate::Result<Option<String>> {
    guard_builtin_store_read(path)?;
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}
