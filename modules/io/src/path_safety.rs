//! Shared no-follow filesystem safety primitives.
//!
//! Both the generated cache root (`cache.rs`) and the built-in trait runtime
//! store (`builtin_store.rs`) need the same guarantee before trusting a
//! directory chain as a root: every existing ancestor component must be
//! inspected with `symlink_metadata` (never followed) and must be a plain
//! directory, and a terminal leaf must be exactly the expected kind (regular
//! file or directory), never a symlink or other special file.
//!
//! Historically every caller of this module operated on paths that stayed
//! relative to the working directory (`.ctx/cache/traits/...`,
//! `.ctx/cache/builtin-traits/...`), so walking `path.ancestors()` never
//! climbed above the working directory and never touched platform path
//! aliases (e.g. macOS `/tmp` -> `/private/tmp`) the way an absolute-path
//! walk could. Since P426 moved the active default cache/store roots to an
//! absolute path under the config home (`~/.config/ctx/cache/<repo-key>/...`),
//! that ancestor walk now legitimately climbs through the config home's real
//! absolute ancestors (still safe: every one of them is expected to be a
//! plain directory, never a symlink). Callers that canonicalize a root first
//! (`resource.rs`) run this check *before* canonicalizing, against the
//! original relative/lexical path, for the same anti-alias reason.

use camino::Utf8Path;

/// What to do with an ancestor component that does not exist yet: leave it
/// missing (a plain existence check) or create it (safe `mkdir -p`).
enum OnMissingAncestor {
    Leave,
    Create,
}

/// Shared ancestor walker for [`ensure_no_symlink_ancestors`] and
/// [`create_dir_all_no_symlinks`]: walks every component of `path` from the
/// root down, inspecting each with `symlink_metadata` (never following a
/// symlink) and rejecting any existing component that is a symlink or a
/// non-directory. This is the one place that decides "is this ancestor
/// chain safe to trust", so every caller — a read-only existence check or a
/// directory-creating one — shares exactly the same verdict.
fn walk_ancestors_no_follow(
    path: &Utf8Path,
    label: &str,
    on_missing: OnMissingAncestor,
) -> crate::Result<()> {
    let mut ancestors: Vec<&Utf8Path> = path.ancestors().collect();
    ancestors.reverse();
    // Tolerate the OS root alias (macOS `/tmp -> /private/tmp`,
    // `/var -> /private/var`, which puts `$TMPDIR`-derived config homes used
    // by isolated-HOME test/CI runs under a symlinked ancestor): skip exactly
    // the immediate child of `/` for an absolute path. Creating a symlink
    // directly under `/` requires root; every deeper, user-writable segment
    // is still checked below. Matches the same lenient policy `run_session`,
    // `write.rs`, and `lockfile.rs` each already implement separately for
    // user-supplied absolute paths.
    let mut skipped_absolute_alias = false;
    for ancestor in ancestors {
        if ancestor.as_str().is_empty() {
            continue;
        }
        if path.is_absolute()
            && !skipped_absolute_alias
            && ancestor.parent().is_some_and(|p| p.as_str() == "/")
        {
            skipped_absolute_alias = true;
            continue;
        }
        match std::fs::symlink_metadata(ancestor.as_std_path()) {
            Ok(meta) => {
                let file_type = meta.file_type();
                if file_type.is_symlink() {
                    return Err(unsafe_path_error(
                        ancestor,
                        &format!("{label} must not be a symlink"),
                    ));
                }
                if !file_type.is_dir() {
                    return Err(unsafe_path_error(
                        ancestor,
                        &format!("{label} must be a directory"),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if matches!(on_missing, OnMissingAncestor::Create) {
                    match std::fs::create_dir(ancestor.as_std_path()) {
                        Ok(()) => {}
                        Err(e2) if e2.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e2) => {
                            return Err(crate::environment::Error::Filesystem {
                                path: ancestor.to_string(),
                                source: e2,
                            }
                            .into());
                        }
                    }
                }
            }
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

/// Ensure every existing ancestor of `path` (including `path` itself) is a
/// plain directory, never a symlink. A component that does not exist yet is
/// fine (not yet created, e.g. a cache root that will be created shortly);
/// an existing non-directory or symlink component is a hard error labeled
/// with `label` for diagnostics.
pub fn ensure_no_symlink_ancestors(path: &Utf8Path, label: &str) -> crate::Result<()> {
    walk_ancestors_no_follow(path, label, OnMissingAncestor::Leave)
}

/// Safely create `path` and every missing ancestor directory, component by
/// component: each existing component must already be a plain directory
/// (never a symlink, checked with `symlink_metadata` before it is trusted),
/// and each missing component is created only after that check passes. This
/// is the safe replacement for calling `std::fs::create_dir_all` and
/// validating ancestors afterward — that ordering lets an unsafe existing
/// component get walked through (or a concurrently-planted symlink win a
/// race) before it is ever inspected.
pub fn create_dir_all_no_symlinks(path: &Utf8Path, label: &str) -> crate::Result<()> {
    walk_ancestors_no_follow(path, label, OnMissingAncestor::Create)
}

/// Check a terminal leaf expected to be a regular file: `Ok(false)` if it
/// does not exist, `Ok(true)` if it exists and is a plain regular file,
/// `Err` if it is a symlink or any other special file type.
pub fn ensure_leaf_is_regular_file_or_absent(path: &Utf8Path, label: &str) -> crate::Result<bool> {
    ensure_leaf_matches_or_absent(path, label, false)
}

/// Check a terminal leaf expected to be a directory: `Ok(false)` if it does
/// not exist, `Ok(true)` if it exists and is a plain directory, `Err` if it
/// is a symlink or any other special file type.
pub fn ensure_leaf_is_directory_or_absent(path: &Utf8Path, label: &str) -> crate::Result<bool> {
    ensure_leaf_matches_or_absent(path, label, true)
}

fn ensure_leaf_matches_or_absent(
    path: &Utf8Path,
    label: &str,
    expect_directory: bool,
) -> crate::Result<bool> {
    match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(meta) => {
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                return Err(unsafe_path_error(
                    path,
                    &format!("{label} must not be a symlink"),
                ));
            }
            let matches = if expect_directory {
                file_type.is_dir()
            } else {
                file_type.is_file()
            };
            if !matches {
                let expected = if expect_directory {
                    "a directory"
                } else {
                    "a regular file"
                };
                return Err(unsafe_path_error(
                    path,
                    &format!("{label} must be {expected}"),
                ));
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}

/// Validate a bare, untrusted string before it is ever used as a single
/// path component (a file-name segment, never a full path): non-empty,
/// ASCII alphanumeric/`-`/`_` only, no `/`, `\`, or `..`. Shared by
/// [`crate::run_session`]'s run-session id validator and
/// [`crate::context_ledger`]'s host-key validator (both halves — harness id
/// and host-reported session id — are untrusted input from a harness), so
/// there is exactly one bare-path-component charset rule in the crate.
/// `label` names the value in the error message (e.g. `"run-session ID"`,
/// `"harness id"`).
pub fn validate_bare_path_component(value: &str, label: &str) -> crate::Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(unsafe_path_error(
            Utf8Path::new(value),
            &format!("{label} must contain only letters, digits, '-' or '_'"),
        ));
    }
    Ok(())
}

fn unsafe_path_error(path: &Utf8Path, message: &str) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_string()),
    }
    .into()
}
