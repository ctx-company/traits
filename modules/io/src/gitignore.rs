//! Nested `.ctx/.gitignore` scaffolding and repository-state doctor
//! diagnostics (P446).
//!
//! `.ctx/` inside a repository checkout mixes committed project source
//! (`.ctx/traits/<id>/`, `.ctx/traits.{toml,lock}`) with machine-local and
//! generated content (`.ctx/config.toml`, `.ctx/harness.toml`,
//! `.ctx/worktrees/`, `.ctx/traits/vendor/`, and the transitional pre-P426
//! `.ctx/{runs,debug,cache}` families). A committed nested `.ctx/.gitignore`
//! makes that boundary self-enforcing without ever touching the repository's
//! root `.gitignore`, which this module never reads or writes.
//!
//! [`ensure_nested_gitignore`] is append-only: called from every setup
//! moment that creates ignorable content (fresh worktree add, write-mode
//! dependency vendoring, project-scoped install/update/reconciliation
//! publication) and from `ctx traits doctor --apply`. [`plan_nested_gitignore`]
//! is the read-only counterpart `doctor`'s default (non-`--apply`) report
//! uses, so the missing-entry diagnostic and every writer share one
//! canonical entry list ([`CANONICAL_ENTRIES`]) and can never diverge.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

/// Canonical nested-ignore entries, in the order written to a fresh
/// `.ctx/.gitignore`. Every diagnostic and writer in this module reads this
/// same list.
pub const CANONICAL_ENTRIES: &[&str] = &[
    // 0052: run worktrees live under `.ctx/traits/` now. The pre-0052
    // top-level entry stays listed on purpose — a checkout that still holds
    // old worktrees must not suddenly show them as untracked, and they are
    // never rewritten in place, only drained as their runs land.
    "traits/worktrees/",
    "worktrees/",
    "config.toml",
    "config.ts",
    "harness.toml",
    "traits/vendor/",
    "runs/",
    "debug/",
    "cache/",
    // Authoring-time npm packages installed by `ctx traits init`, resolved by
    // the CDK build because Node walks up from `.ctx`. Reinstallable from the
    // manifest beside it, and a committed `node_modules` would be a
    // spectacular first impression.
    "node_modules/",
];

/// Repository-relative path of the nested ignore file this module owns.
pub fn nested_gitignore_path(repo_root: &Utf8Path) -> Utf8PathBuf {
    repo_root.join(".ctx").join(".gitignore")
}

/// Read-only plan: whether `.ctx/.gitignore` exists and which canonical
/// entries, if any, are missing from it. Never writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitignorePlan {
    pub path: Utf8PathBuf,
    pub exists: bool,
    pub missing: Vec<String>,
}

/// Result of [`ensure_nested_gitignore`]: whether the file was freshly
/// created and which canonical entries were actually appended this call
/// (empty when the file already carried every entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureReport {
    pub path: Utf8PathBuf,
    pub created: bool,
    pub appended: Vec<String>,
}

fn read_optional_string(path: &Utf8Path) -> crate::Result<Option<String>> {
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

/// Plan the missing entries for `repo_root`'s nested `.ctx/.gitignore`
/// without writing anything.
pub fn plan_nested_gitignore(repo_root: &Utf8Path) -> crate::Result<GitignorePlan> {
    let path = nested_gitignore_path(repo_root);
    let existing = read_optional_string(&path)?;
    let existing_lines: BTreeSet<&str> = existing
        .as_deref()
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let missing: Vec<String> = CANONICAL_ENTRIES
        .iter()
        .filter(|entry| !existing_lines.contains(*entry))
        .map(|entry| (*entry).to_string())
        .collect();
    Ok(GitignorePlan {
        path,
        exists: existing.is_some(),
        missing,
    })
}

/// Append-only ensure: create `.ctx/.gitignore` if absent, otherwise append
/// only the canonical entries not already present as an exact line. Existing
/// bytes are never rewritten: an existing file is opened in OS append mode
/// (`O_APPEND`), so this never passes through a truncating replacement write
/// that could lose or corrupt a concurrent edit or fail mid-write — at most
/// one separator newline is added before the appended block. Never touches
/// the repository's root `.gitignore`. Idempotent: a second call with
/// nothing missing performs no write at all.
pub fn ensure_nested_gitignore(repo_root: &Utf8Path) -> crate::Result<EnsureReport> {
    let plan = plan_nested_gitignore(repo_root)?;
    if plan.missing.is_empty() {
        return Ok(EnsureReport {
            path: plan.path,
            created: false,
            appended: Vec::new(),
        });
    }
    if let Some(parent) = plan.path.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let mut block = String::new();
    if plan.exists {
        let existing = read_optional_string(&plan.path)?.unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            block.push('\n');
        }
    }
    for entry in &plan.missing {
        block.push_str(entry);
        block.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(plan.path.as_std_path())
        .map_err(|source| crate::environment::Error::Filesystem {
            path: plan.path.to_string(),
            source,
        })?;
    std::io::Write::write_all(&mut file, block.as_bytes()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: plan.path.to_string(),
            source,
        }
    })?;
    Ok(EnsureReport {
        path: plan.path,
        created: !plan.exists,
        appended: plan.missing,
    })
}

/// One tracked path that should instead be ignored, with a text-only
/// `git rm --cached` remedy. Never executes the remedy itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedRuntimeFinding {
    pub path: String,
    pub remedy: String,
}

/// Tracked `.ctx/config.toml`, `.ctx/config.ts`, and tracked files under
/// `.ctx/{worktrees,runs,debug,cache}`, discovered via `git ls-files` with
/// literal arguments against `repo_root`, sorted. Empty when nothing in
/// those runtime paths is tracked.
pub fn tracked_runtime_paths(repo_root: &Utf8Path) -> crate::Result<Vec<TrackedRuntimeFinding>> {
    let candidates = [
        ".ctx/config.toml",
        ".ctx/config.ts",
        ".ctx/traits/worktrees",
        ".ctx/worktrees",
        ".ctx/runs",
        ".ctx/debug",
        ".ctx/cache",
    ];
    let mut args: Vec<&str> = vec!["ls-files", "--"];
    args.extend(candidates.iter().copied());
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args: &args,
        success_exit_code: &[0],
        timeout_ms: 10_000,
        capture_limit: 1_048_576,
    })?;
    if !output.success {
        return Err(crate::git_process::error("git ls-files", &output));
    }
    output.refuse_if_truncated("git ls-files (tracked runtime paths)")?;
    let mut paths: Vec<String> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|path| TrackedRuntimeFinding {
            remedy: format!("git rm --cached -- {path}"),
            path,
        })
        .collect())
}

/// The global `ctx` config-home store physically resolved inside a Git
/// repository (a dotfiles checkout, most commonly), with the
/// `config/ctx/{runs,debug,cache}/` recommendation this finding carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalStoreFinding {
    pub global_root: Utf8PathBuf,
    pub git_root: Utf8PathBuf,
    pub remedy: String,
}

/// Walk the physically resolved global `ctx` store's ancestors (including
/// itself) for a `.git` directory or file, never inferring this from the
/// invocation repository. `None` when the store sits outside any Git
/// repository, which is the ordinary case.
pub fn global_store_inside_git_repo() -> crate::Result<Option<GlobalStoreFinding>> {
    let global_root = crate::state::global_ctx_root()?;
    let mut current: Option<&Utf8Path> = Some(global_root.as_path());
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            let git_root = dir.to_path_buf();
            return Ok(Some(GlobalStoreFinding {
                global_root: global_root.clone(),
                remedy: format!(
                    "the global ctx store at {global_root} physically resolves inside the Git repository rooted at {git_root} (likely a dotfiles checkout); keep {{runs,debug,cache}} out of that repository's tracked content — for example a separate config/ctx/{{runs,debug,cache}}/ location on the dotfiles side, ignored or symlinked away from version control"
                ),
                git_root,
            }));
        }
        current = dir.parent();
    }
    Ok(None)
}
