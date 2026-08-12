//! Filesystem write operations.
//!
//! All writes go through this module so the CLI edge never touches
//! `std::fs::write` directly. This keeps the hexagonal boundary explicit:
//! the app layer orchestrates, the IO layer owns the filesystem.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

/// Rename (move) a directory, mapping a failure to a typed filesystem error
/// naming both the source and destination.
pub fn rename_dir(from: &Utf8Path, to: &Utf8Path) -> crate::Result<()> {
    std::fs::rename(from, to).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: format!("{from} -> {to}"),
            source,
        }
        .into()
    })
}

/// Write text content to a file path, creating or overwriting it.
pub fn write_text(path: &Utf8Path, content: &str) -> crate::Result<()> {
    reject_builtin_store_target(path)?;
    Ok(
        std::fs::write(path, content).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?,
    )
}

/// Write CDK build output after safely creating its parent directories.
pub fn write_build_output(path: &Utf8Path, content: &str) -> crate::Result<()> {
    reject_builtin_store_target(path)?;
    reject_symlink_ancestors(path)?;
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    if parent != Utf8Path::new(".") && !parent.as_str().is_empty() {
        std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
            path: parent.to_string(),
            source: e,
        })?;
    }
    reject_symlink_ancestors(path)?;
    reject_symlink_leaf(path)?;
    // Best-effort TOCTOU guard immediately before writing.
    reject_symlink_ancestors(path)?;
    reject_symlink_leaf(path)?;
    write_text(path, content)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitignoreUpdateResult {
    Added { path: String, entry: String },
    AlreadyPresent { path: String, entry: String },
}

/// Add a generated static export path to the repo `.gitignore` with guards.
pub fn update_gitignore_for_generated_path(
    repo_root: &Utf8Path,
    generated_path: &Utf8Path,
) -> crate::Result<GitignoreUpdateResult> {
    let relative = validate_generated_ignore_path(repo_root, generated_path)?;
    let entry = format!("/{relative}");
    let gitignore = repo_root.join(".gitignore");
    reject_symlink_ancestors(&gitignore)?;
    reject_symlink_leaf(&gitignore)?;
    let mut existing = match std::fs::read_to_string(&gitignore) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(crate::environment::Error::Filesystem {
                path: gitignore.to_string(),
                source: error,
            }
            .into());
        }
    };
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(GitignoreUpdateResult::AlreadyPresent {
            path: gitignore.to_string(),
            entry,
        });
    }
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&entry);
    existing.push('\n');
    std::fs::write(&gitignore, existing).map_err(|e| crate::environment::Error::Filesystem {
        path: gitignore.to_string(),
        source: e,
    })?;
    Ok(GitignoreUpdateResult::Added {
        path: gitignore.to_string(),
        entry,
    })
}

fn validate_generated_ignore_path(
    repo_root: &Utf8Path,
    generated_path: &Utf8Path,
) -> crate::Result<Utf8PathBuf> {
    let relative = normalize_generated_relative_path(repo_root, generated_path)?;
    let raw = relative.as_str();
    if raw.is_empty() || raw.contains('\\') || raw.contains("//") {
        return Err(fs_err(
            generated_path,
            "generated ignore path has an unsafe shape",
        ));
    }
    let protocol_prefix = format!("{}/", crate::layout::trait_protocol_root());
    let authoring_prefix = format!("{}/", crate::layout::trait_authoring_root());
    if raw.starts_with(&protocol_prefix) || raw.starts_with(&authoring_prefix) {
        return Err(fs_err(
            generated_path,
            "refusing to ignore canonical trait package source",
        ));
    }
    for protected in [
        crate::layout::PACKAGE_MANIFEST,
        crate::layout::TRAIT_LOCKFILE,
        "index.toml",
    ] {
        if raw.ends_with(protected) {
            return Err(fs_err(
                generated_path,
                "refusing to ignore canonical trait source or lock files",
            ));
        }
    }
    if raw.contains("/resources/") || raw.contains("/docs/") {
        return Err(fs_err(
            generated_path,
            "refusing to ignore trait resources or docs",
        ));
    }
    let allowed = GENERATED_EXPORT_ROOTS
        .iter()
        .any(|root| raw == *root || raw.starts_with(&format!("{root}/")));
    if !allowed {
        return Err(fs_err(
            generated_path,
            "only generated static export roots may be added to .gitignore",
        ));
    }
    Ok(relative)
}

fn normalize_generated_relative_path(
    repo_root: &Utf8Path,
    generated_path: &Utf8Path,
) -> crate::Result<Utf8PathBuf> {
    let candidate = if generated_path.is_absolute() {
        generated_path
            .strip_prefix(repo_root)
            .map_err(|_| fs_err(generated_path, "generated path is outside the repository"))?
    } else if repo_root != Utf8Path::new(".") && generated_path.starts_with(repo_root) {
        generated_path
            .strip_prefix(repo_root)
            .map_err(|_| fs_err(generated_path, "generated ignore path has an unsafe shape"))?
    } else {
        generated_path
    };
    let mut normalized = Utf8PathBuf::new();
    for component in candidate.components() {
        match component {
            Utf8Component::Normal(part) => normalized.push(part),
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                return Err(fs_err(
                    generated_path,
                    "generated ignore path must not contain parent-directory components",
                ));
            }
            Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                return Err(fs_err(
                    generated_path,
                    "generated ignore path has an unsafe shape",
                ));
            }
        }
    }
    Ok(normalized)
}

/// Write mode for assist candidate writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateWriteMode {
    /// New candidate package (compose, refine --out).
    NewCandidate,
    /// Overwrite existing canonical source (refine --apply).
    RefineApply,
    /// Managed import package (import --llm-assisted).
    ManagedImport,
    /// New package's authoring source, `<root>/source/index.ts` (generate).
    NewCandidateSource,
    /// Overwrite an existing package's authoring source (refine --apply,
    /// task 0065: `refine` edits `source/index.ts`, never `generated/`).
    RefineApplySource,
    /// Managed import package's authoring source (import --llm-assisted,
    /// task 0065).
    ManagedImportSource,
}

impl CandidateWriteMode {
    /// Whether this mode targets `<package>/source/index.ts` instead of
    /// `<package>/generated/index.toml` (task 0065: only `build` writes
    /// canonical; assist writes authoring source).
    fn targets_authoring_source(self) -> bool {
        matches!(
            self,
            CandidateWriteMode::NewCandidateSource
                | CandidateWriteMode::RefineApplySource
                | CandidateWriteMode::ManagedImportSource
        )
    }

    /// The legacy (pre-0065) canonical-targeting mode this source mode
    /// mirrors for leaf-overwrite semantics (new-vs-refine-vs-import).
    fn leaf_semantics(self) -> CandidateWriteMode {
        match self {
            CandidateWriteMode::NewCandidateSource => CandidateWriteMode::NewCandidate,
            CandidateWriteMode::RefineApplySource => CandidateWriteMode::RefineApply,
            CandidateWriteMode::ManagedImportSource => CandidateWriteMode::ManagedImport,
            other => other,
        }
    }
}

/// Request for a safe assist candidate write.
pub struct CandidateWriteRequest<'a> {
    pub target_path: &'a Utf8Path,
    pub trait_id: &'a str,
    pub content: &'a str,
    pub mode: CandidateWriteMode,
}

/// Result of a safe assist candidate write.
#[derive(Debug)]
pub struct CandidateWriteResult {
    pub path: String,
    pub byte_size: u64,
    pub overwritten: bool,
}

const GENERATED_MARKER: &str = "GENERATED FILE - DO NOT EDIT DIRECTLY";
const GENERATED_EXPORT_ROOTS: &[&str] = &[
    ".agents/skills",
    ".opencode/skills",
    ".claude/skills",
    ".github/skills",
    ".pi/skills",
];

/// Write an assist candidate to a safe canonical package path.
///
/// Validates that the target is a canonical trait package
/// (repo-local trait source root plus `<id>/generated/trait.toml`), rejects generated exports,
/// symlinks, unsafe path segments, and existing generated-marker files.
pub fn write_candidate(request: CandidateWriteRequest) -> crate::Result<CandidateWriteResult> {
    let target = request.target_path;

    reject_builtin_store_target(target)?;
    validate_candidate_path(target, request.trait_id, request.mode)?;

    for root in GENERATED_EXPORT_ROOTS {
        if target.as_str().starts_with(root) {
            return Err(crate::environment::Error::Filesystem {
                path: target.to_string(),
                source: std::io::Error::other(format!(
                    "target is inside generated export root {root}"
                )),
            }
            .into());
        }
    }

    reject_symlink_ancestors(target)?;

    let parent = target.parent().unwrap_or_else(|| Utf8Path::new("."));
    if !parent.as_str().is_empty() && parent.as_str() != "." {
        std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
            path: parent.to_string(),
            source: e,
        })?;
    }

    reject_symlink_ancestors(target)?;

    let overwritten = validate_leaf_for_mode(target, request.mode)?;

    let byte_size = request.content.len() as u64;

    // Final best-effort TOCTOU guard immediately before writing.
    reject_symlink_ancestors(target)?;
    let final_overwritten = validate_leaf_for_mode(target, request.mode)?;
    if final_overwritten != overwritten {
        return Err(fs_err(target, "target leaf changed before write"));
    }

    std::fs::write(target, request.content).map_err(|e| crate::environment::Error::Filesystem {
        path: target.to_string(),
        source: e,
    })?;

    Ok(CandidateWriteResult {
        path: target.to_string(),
        byte_size,
        overwritten,
    })
}

fn validate_candidate_path(
    path: &Utf8Path,
    trait_id: &str,
    mode: CandidateWriteMode,
) -> crate::Result<()> {
    let s = path.as_str();

    if s.is_empty() {
        return Err(fs_err(path, "path is empty"));
    }

    if s.contains('\\') {
        return Err(fs_err(path, "path contains backslashes"));
    }

    if s.contains("//") {
        return Err(fs_err(path, "path contains empty segments"));
    }

    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        return Err(fs_err(path, "path looks like a Windows drive prefix"));
    }

    for component in path.components() {
        match component {
            Utf8Component::ParentDir => return Err(fs_err(path, "path contains parent traversal")),
            Utf8Component::Normal("") => return Err(fs_err(path, "path contains empty segments")),
            _ => {}
        }
    }

    let package_id = if mode.targets_authoring_source() {
        let Some(id) = canonical_source_package_id(path) else {
            return Err(fs_err(
                path,
                format!(
                    "target must be a canonical {}/<package>/source/index.ts path",
                    crate::layout::trait_protocol_root()
                ),
            ));
        };
        id
    } else {
        let Some(id) = canonical_generated_package_id(path) else {
            return Err(fs_err(
                path,
                format!(
                    "target must be a canonical {}/<package>/generated/index.toml path",
                    crate::layout::trait_protocol_root()
                ),
            ));
        };
        id
    };

    let package_matches_trait = package_id == trait_id;
    let package_is_candidate = matches!(
        mode,
        CandidateWriteMode::NewCandidate | CandidateWriteMode::NewCandidateSource
    ) && package_id
        .strip_prefix(trait_id)
        .is_some_and(|suffix| suffix.starts_with('-'));
    if !package_matches_trait && !package_is_candidate {
        return Err(fs_err(
            path,
            "target package must match trait ID or use a <trait-id>-candidate suffix",
        ));
    }

    Ok(())
}

/// Collects a path's normal (non-root, non-parent) components, rejecting
/// any root/parent-traversal component outright. Shared by both canonical
/// package-id matchers below.
fn normal_components(path: &Utf8Path) -> Option<Vec<&str>> {
    if path
        .components()
        .any(|component| !matches!(component, Utf8Component::RootDir | Utf8Component::Normal(_)))
    {
        return None;
    }
    Some(
        path.components()
            .filter_map(|component| match component {
                Utf8Component::Normal(value) => Some(value),
                _ => None,
            })
            .collect(),
    )
}

/// Extracts the trait ID from a package-relative tail (e.g. `["generated",
/// "index.toml"]`) under either the current `.ctx/traits/authored/<id>/...`
/// root or the pre-P569 flat `.ctx/traits/<id>/...` root — the same two
/// shapes [`crate::layout::is_canonical_package_root`] accepts for reads, so
/// the safe writer accepts whichever shape a checkout's packages actually
/// live under instead of only the shape that predates the P569 move.
fn trait_id_for_tail<'a>(normals: &[&'a str], tail: &[&str]) -> Option<&'a str> {
    let tail_len = tail.len();
    if normals.len() < tail_len + 1 || &normals[normals.len() - tail_len..] != tail {
        return None;
    }
    let head = &normals[..normals.len() - tail_len];
    match head {
        [".ctx", "traits", id] if !id.is_empty() => Some(*id),
        [".ctx", "traits", "authored", id] if !id.is_empty() => Some(*id),
        _ => None,
    }
}

/// Extracts the trait ID from an absolute path by locating a `.ctx`
/// component and matching the remainder against [`trait_id_for_tail`]. Tries
/// every `.ctx` occurrence from rightmost (closest to the target file) to
/// leftmost — an absolute checkout path can itself sit under an ancestor
/// `.ctx` directory (e.g. a worktree managed inside another `.ctx`-rooted
/// tree), and the innermost one is always the target's own protocol root.
fn trait_id_for_tail_absolute<'a>(normals: &[&'a str], tail: &[&str]) -> Option<&'a str> {
    normals
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, component)| (*component == ".ctx").then_some(index))
        .find_map(|ctx_position| trait_id_for_tail(&normals[ctx_position..], tail))
}

fn canonical_generated_package_id(path: &Utf8Path) -> Option<&str> {
    let normals = normal_components(path)?;
    const TAIL: [&str; 1] = ["generated"];
    // `generated/<manifest>` accepts either manifest filename, so the tail
    // match happens per-candidate rather than as one fixed slice.
    for manifest in [crate::layout::PACKAGE_MANIFEST, "index.toml"] {
        let tail = [TAIL[0], manifest];
        let found = if path.is_absolute() {
            trait_id_for_tail_absolute(&normals, &tail)
        } else {
            trait_id_for_tail(&normals, &tail)
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Mirror of [`canonical_generated_package_id`] for the authoring-source
/// shape `<package>/source/index.ts` (task 0065).
fn canonical_source_package_id(path: &Utf8Path) -> Option<&str> {
    let normals = normal_components(path)?;
    const TAIL: [&str; 2] = ["source", "index.ts"];
    if path.is_absolute() {
        trait_id_for_tail_absolute(&normals, &TAIL)
    } else {
        trait_id_for_tail(&normals, &TAIL)
    }
}

fn reject_symlink_ancestors(path: &Utf8Path) -> crate::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let mut skipped_absolute_alias = false;
    for ancestor in parent.ancestors() {
        if ancestor.as_str().is_empty() || ancestor.as_str() == "." {
            continue;
        }
        if path.is_absolute()
            && !skipped_absolute_alias
            && ancestor.parent().is_some_and(|p| p.as_str() == "/")
        {
            skipped_absolute_alias = true;
            continue;
        }
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(fs_err(path, "symlink in ancestor path"));
                }
                if !meta.is_dir() {
                    return Err(fs_err(path, "non-directory in ancestor path"));
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

fn reject_symlink_leaf(path: &Utf8Path) -> crate::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(fs_err(path, "target leaf is a symlink"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeafState {
    Missing,
    RegularFile,
    Symlink,
    Special,
}

fn inspect_leaf(path: &Utf8Path) -> crate::Result<LeafState> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                Ok(LeafState::Symlink)
            } else if meta.is_file() {
                Ok(LeafState::RegularFile)
            } else {
                Ok(LeafState::Special)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LeafState::Missing),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}

fn validate_leaf_for_mode(path: &Utf8Path, mode: CandidateWriteMode) -> crate::Result<bool> {
    match inspect_leaf(path)? {
        LeafState::Missing => match mode.leaf_semantics() {
            CandidateWriteMode::RefineApply => {
                Err(fs_err(path, "refine apply target must already exist"))
            }
            CandidateWriteMode::NewCandidate | CandidateWriteMode::ManagedImport => Ok(false),
            _ => unreachable!("leaf_semantics() only returns legacy modes"),
        },
        LeafState::Symlink => Err(fs_err(path, "target is a symlink")),
        LeafState::Special => Err(fs_err(path, "target is not a regular file")),
        LeafState::RegularFile => validate_regular_leaf_for_mode(path, mode),
    }
}

fn validate_regular_leaf_for_mode(
    path: &Utf8Path,
    mode: CandidateWriteMode,
) -> crate::Result<bool> {
    let existing =
        std::fs::read_to_string(path).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?;

    if existing.contains(GENERATED_MARKER) {
        return Err(fs_err(
            path,
            "existing file contains a generated marker; refusing to overwrite generated content",
        ));
    }

    match mode.leaf_semantics() {
        CandidateWriteMode::NewCandidate | CandidateWriteMode::ManagedImport => Err(fs_err(
            path,
            "target already exists; use refine or choose a new --out package",
        )),
        CandidateWriteMode::RefineApply => Ok(true),
        _ => unreachable!("leaf_semantics() only returns legacy modes"),
    }
}

/// Write a resource file under a canonical trait package.
///
/// Rejects symlink ancestors and symlink/special leaves. Creates parent
/// directories safely with re-checks after creation.
pub fn write_resource_file(path: &Utf8Path, content: &str) -> crate::Result<()> {
    reject_builtin_store_target(path)?;
    reject_symlink_ancestors(path)?;

    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    if !parent.as_str().is_empty() && parent.as_str() != "." {
        let parent_exists = std::fs::symlink_metadata(parent)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !parent_exists {
            std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source: e,
            })?;
            reject_symlink_ancestors(path)?;
        }
    }

    match inspect_leaf(path)? {
        LeafState::Missing => {}
        LeafState::Symlink => return Err(fs_err(path, "resource target is a symlink")),
        LeafState::Special => return Err(fs_err(path, "resource target is not a regular file")),
        LeafState::RegularFile => {}
    }

    reject_symlink_ancestors(path)?;
    std::fs::write(path, content).map_err(|e| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: e,
    })?;

    Ok(())
}

/// Reject any ordinary mutation API target that lies inside the built-in
/// trait store (the active global `.../cache/<repo-key>/builtin-traits/...`
/// shape or the legacy `.ctx/cache/builtin-traits/...` shape). Only
/// `builtin_store`'s internal materializer may write there; every
/// general-purpose write entry point in this module checks this first.
fn reject_builtin_store_target(path: &Utf8Path) -> crate::Result<()> {
    if crate::layout::is_within_builtin_store(path) {
        return Err(fs_err(
            path,
            "refusing to write inside the built-in trait store; built-in packages are read-only",
        ));
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: a sibling `.<file-name>.tmp` file is
/// created (refusing to follow a symlink at that temp leaf), fully written
/// and `fsync`'d, then renamed over `path` after a last symlink re-check.
/// A failure at any point removes the temp file so no stray `.tmp` sibling
/// is left behind. Shared by [`crate::run_session`]'s text ledger writer and
/// by [`crate::host_install`]'s placement-manifest and archive writes so
/// there is exactly one temp-file/rename discipline in the crate.
pub fn write_bytes_atomically(path: &Utf8Path, bytes: &[u8]) -> crate::Result<()> {
    write_bytes_atomically_raw(path, bytes).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()
    })
}

/// The same temp-file/`fsync`/rename discipline as [`write_bytes_atomically`],
/// but returning a bare [`std::io::Result`] instead of wrapping into
/// [`crate::Error`] — shared with [`crate::export::fs::Service`], whose
/// [`crate::export::host::Interface::write`] boundary is `std::io::Result`
/// (a `host::Interface` implementor has no crate-error boundary to cross),
/// so the real filesystem-backed export/host-placement artifact write is
/// atomic through the same one temp-file/rename primitive as every other
/// write in this module.
pub(crate) fn write_bytes_atomically_raw(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let Some(file_name) = path.file_name() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic write path must include a file name",
        ));
    };
    let temp_path = parent.join(format!(".{file_name}.tmp"));
    if is_symlink(&temp_path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing to write through symlinked temp path {temp_path}"),
        ));
    }
    match std::fs::remove_file(&temp_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path.as_std_path())?;
        file.write_all(bytes).and_then(|_| file.sync_all())?;
        if is_symlink(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("refusing to write through symlinked target path {path}"),
            ));
        }
        std::fs::rename(temp_path.as_std_path(), path.as_std_path())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(temp_path.as_std_path());
    }
    result
}

fn is_symlink(path: &Utf8Path) -> bool {
    std::fs::symlink_metadata(path.as_std_path())
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn fs_err(path: &Utf8Path, msg: impl Into<String>) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::other(msg.into()),
    }
    .into()
}

/// Write an edit to an existing package manifest (`trait.toml`).
///
/// The sole write path for lifecycle transitions (`ctx traits
/// activate`/`deactivate`, Group 95, 2026-07-19), which mutate only
/// `[package].status`. Safety guards:
/// - Rejects paths under generated export roots (`.agents/skills`, etc.)
/// - Rejects paths that are not a package-root `trait.toml`
/// - Rejects symlink ancestors and symlink/special leaf, with TOCTOU
///   re-checks after path validation and immediately before write
/// - Commits atomically: content is written to a sibling temp file and
///   renamed over the target, so a crash or interruption never leaves a
///   truncated manifest
pub fn write_package_manifest(path: &Utf8Path, content: &str) -> crate::Result<()> {
    reject_builtin_store_target(path)?;
    for root in GENERATED_EXPORT_ROOTS {
        if path.as_str().starts_with(root) {
            return Err(fs_err(
                path,
                format!("target is inside generated export root {root}"),
            ));
        }
    }

    validate_package_manifest_shape(path)?;
    reject_symlink_ancestors(path)?;

    let existing = match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(fs_err(path, "target is a symlink"));
            }
            if !meta.file_type().is_file() {
                return Err(fs_err(path, "target is not a regular file"));
            }
            let text = std::fs::read_to_string(path).map_err(|e| {
                crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source: e,
                }
            })?;
            Some(text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(fs_err(path, "package manifest must already exist"));
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: e,
            }
            .into());
        }
    };

    reject_symlink_ancestors(path)?;

    if let Some(ref prev) = existing
        && prev == content
    {
        return Ok(());
    }

    reject_symlink_ancestors(path)?;
    match inspect_leaf(path)? {
        LeafState::Symlink => return Err(fs_err(path, "target became a symlink before write")),
        LeafState::Special => {
            return Err(fs_err(path, "target became a special file before write"));
        }
        LeafState::Missing => return Err(fs_err(path, "target disappeared before write")),
        LeafState::RegularFile => {}
    }

    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let tmp_path = parent.join(format!(".{}.tmp", path.file_name().unwrap_or("trait.toml")));
    reject_symlink_leaf(&tmp_path)?;
    std::fs::write(&tmp_path, content).map_err(|e| crate::environment::Error::Filesystem {
        path: tmp_path.to_string(),
        source: e,
    })?;
    std::fs::rename(&tmp_path, path).map_err(|e| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: e,
    })?;

    Ok(())
}

fn validate_package_manifest_shape(path: &Utf8Path) -> crate::Result<()> {
    let s = path.as_str();
    if s.is_empty() || s.contains('\\') || s.contains("//") {
        return Err(fs_err(path, "path contains unsafe segments"));
    }
    for component in path.components() {
        if component == Utf8Component::ParentDir {
            return Err(fs_err(path, "path contains parent traversal"));
        }
    }
    if path.file_name() == Some("package.toml") {
        return Err(fs_err(
            path,
            "package.toml found — renamed to trait.toml in 0169",
        ));
    }
    if path.file_name() != Some(crate::layout::PACKAGE_MANIFEST) {
        return Err(fs_err(
            path,
            "lifecycle edits target a package-root trait.toml only",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod authoring_source_write_tests {
    use super::*;

    fn scratch_dir(label: &str) -> Utf8PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ctx-write-rs-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    #[test]
    fn new_candidate_source_writes_source_index_ts_under_traits_root() {
        let root = scratch_dir("new-source");
        let target = root.join(".ctx/traits/my-trait/source/index.ts");
        let result = write_candidate(CandidateWriteRequest {
            target_path: &target,
            trait_id: "my-trait",
            content: "export default trait(\"my-trait\", {});",
            mode: CandidateWriteMode::NewCandidateSource,
        })
        .unwrap();
        assert!(!result.overwritten);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "export default trait(\"my-trait\", {});"
        );
    }

    #[test]
    fn new_candidate_source_rejects_generated_shaped_target() {
        let root = scratch_dir("reject-generated");
        let target = root.join(".ctx/traits/my-trait/generated/index.toml");
        let err = write_candidate(CandidateWriteRequest {
            target_path: &target,
            trait_id: "my-trait",
            content: "id = \"my-trait\"",
            mode: CandidateWriteMode::NewCandidateSource,
        })
        .unwrap_err();
        assert!(format!("{err}").contains("source/index.ts"));
    }

    #[test]
    fn refine_apply_source_requires_existing_file_and_overwrites_it() {
        let root = scratch_dir("refine-apply-source");
        let target = root.join(".ctx/traits/my-trait/source/index.ts");
        let missing = write_candidate(CandidateWriteRequest {
            target_path: &target,
            trait_id: "my-trait",
            content: "new",
            mode: CandidateWriteMode::RefineApplySource,
        })
        .unwrap_err();
        assert!(format!("{missing}").contains("must already exist"));

        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old").unwrap();
        let result = write_candidate(CandidateWriteRequest {
            target_path: &target,
            trait_id: "my-trait",
            content: "new",
            mode: CandidateWriteMode::RefineApplySource,
        })
        .unwrap();
        assert!(result.overwritten);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }
}
