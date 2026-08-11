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
//!
//! [`ensure_runtime_exclude`] is a separate, narrower tier (0164): it keeps
//! the dispatched harness's own runtime writes into a run worktree (today
//! just `.agents/runs/`, [`RUNTIME_EXCLUDE_ENTRIES`]) invisible to `git
//! status` / `git add -A` from inside that worktree, by appending to
//! `info/exclude` in the repository's COMMON Git directory (resolved via
//! `git rev-parse --git-path info/exclude`, so the entry covers every linked
//! worktree, not just the one being created). This is deliberately shared
//! with the target repository's own primary checkout — `info/exclude` is
//! local and uncommitted there, never touching root `.gitignore` or any
//! tracked file, and the entry list is kept to `.agents/runs/` specifically
//! (never the broader `.agents/`) so it can never hide committed content
//! such as projected skills under `.agents/skills/`. Excluding a path here
//! never hides it if it is already tracked in the target repository's
//! history — that edge is guarded by the traits' own `git add -A` + `git
//! reset -q -- .agents/runs` commit-tail recipe, unchanged by this module.

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
    // 0037: the machine-local runtime tier. The committed siblings —
    // `traits/config.toml` (project decisions) and
    // `traits/runtime.example.toml` (the template this file is copied from) —
    // are deliberately NOT listed here.
    "traits/runtime.toml",
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

/// Runtime-tier exclude entries appended to the common Git directory's
/// `info/exclude` by [`ensure_runtime_exclude`] — the harness-writer paths
/// discovered for 0164. Deliberately narrow: `.agents/runs/` only, never the
/// broader `.agents/` (see the module doc).
pub const RUNTIME_EXCLUDE_ENTRIES: &[&str] = &[".agents/runs/"];

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
    append_missing_lines(plan.path, plan.exists, plan.missing)
}

/// Shared append-only body for [`ensure_nested_gitignore`] and
/// [`ensure_runtime_exclude`]: given a resolved ignore-file `path`, whether
/// it already `exists`, and the exact-line `missing` entries a prior
/// read-only plan computed, create the file if absent and append only the
/// missing lines via `O_APPEND` — never a truncating rewrite that could lose
/// or corrupt a concurrent edit or fail mid-write. Idempotent: an empty
/// `missing` list performs no write at all.
fn append_missing_lines(
    path: Utf8PathBuf,
    exists: bool,
    missing: Vec<String>,
) -> crate::Result<EnsureReport> {
    if missing.is_empty() {
        return Ok(EnsureReport {
            path,
            created: false,
            appended: Vec::new(),
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let mut block = String::new();
    if exists {
        let existing = read_optional_string(&path)?.unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            block.push('\n');
        }
    }
    for entry in &missing {
        block.push_str(entry);
        block.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path())
        .map_err(|source| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        })?;
    std::io::Write::write_all(&mut file, block.as_bytes()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
    })?;
    Ok(EnsureReport {
        path,
        created: !exists,
        appended: missing,
    })
}

/// Idempotent ensure that `.agents/runs/` (see [`RUNTIME_EXCLUDE_ENTRIES`])
/// is excluded from `git status` / `git add -A` everywhere inside
/// `repo_root`'s repository, including every linked worktree: resolves
/// `info/exclude` in the COMMON Git directory via
/// `git rev-parse --git-path info/exclude` run at `repo_root` (mirroring
/// `seed_baseline_root`'s `--git-path` technique in `worktree.rs`), then
/// appends only the missing canonical lines through the same exact-line,
/// `O_APPEND` machinery [`ensure_nested_gitignore`] uses. Never touches root
/// `.gitignore` or any tracked file — a path already tracked in the target
/// repository's history stays visible; the traits' `git add -A` + `git
/// reset -q -- .agents/runs` commit-tail recipe remains the guard for that
/// case. `info/exclude` is local and uncommitted, so this is deliberately
/// shared with the repository's own primary checkout (see the module doc).
pub fn ensure_runtime_exclude(repo_root: &Utf8Path) -> crate::Result<EnsureReport> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args: &["rev-parse", "--git-path", "info/exclude"],
        success_exit_code: &[0],
        timeout_ms: crate::git_process::PLUMBING_TIMEOUT_MS,
        capture_limit: 65_536,
    })?;
    if !output.success {
        return Err(crate::git_process::error(
            "git rev-parse --git-path info/exclude",
            &output,
        ));
    }
    let raw = output.stdout.trim();
    let path = if Utf8Path::new(raw).is_absolute() {
        Utf8PathBuf::from(raw)
    } else {
        repo_root.join(raw)
    };
    let existing = read_optional_string(&path)?;
    let existing_lines: BTreeSet<&str> = existing
        .as_deref()
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let missing: Vec<String> = RUNTIME_EXCLUDE_ENTRIES
        .iter()
        .filter(|entry| !existing_lines.contains(*entry))
        .map(|entry| (*entry).to_string())
        .collect();
    let exists = existing.is_some();
    append_missing_lines(path, exists, missing)
}

/// One tracked path that should instead be ignored, with a text-only
/// `git rm --cached` remedy. Never executes the remedy itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedRuntimeFinding {
    pub path: String,
    pub remedy: String,
}

/// Tracked `.ctx/config.toml`, `.ctx/config.ts`, `.ctx/traits/runtime.toml`
/// (machine-local per 0037 — the committed tier is `.ctx/traits/config.toml`),
/// and tracked files under
/// `.ctx/{worktrees,runs,debug,cache}`, discovered via `git ls-files` with
/// literal arguments against `repo_root`, sorted. Empty when nothing in
/// those runtime paths is tracked.
pub fn tracked_runtime_paths(repo_root: &Utf8Path) -> crate::Result<Vec<TrackedRuntimeFinding>> {
    let candidates = [
        ".ctx/config.toml",
        ".ctx/config.ts",
        ".ctx/traits/runtime.toml",
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Utf8Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.as_std_path())
            .status()
            .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"));
        assert!(status.success(), "git {args:?} exited non-zero");
    }

    fn fresh_repo(tag: &str) -> Utf8PathBuf {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let repo = root.join(format!(
            "ctx-gitignore-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if repo.exists() {
            std::fs::remove_dir_all(repo.as_std_path()).expect("clear stale scratch repo");
        }
        std::fs::create_dir_all(repo.as_std_path()).expect("create scratch repo dir");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.name", "ctx-gitignore-test"]);
        git(
            &repo,
            &["config", "user.email", "gitignore-test@example.invalid"],
        );
        repo
    }

    fn porcelain_status(dir: &Utf8Path) -> String {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir.as_std_path())
            .output()
            .expect("git status runs");
        assert!(output.status.success(), "git status --porcelain failed");
        String::from_utf8(output.stdout).expect("utf8 status")
    }

    fn write_runtime_state(dir: &Utf8Path) {
        std::fs::create_dir_all(dir.join(".agents/runs").as_std_path())
            .expect("create .agents/runs");
        std::fs::write(dir.join(".agents/runs/ledger.json").as_std_path(), b"{}\n")
            .expect("write ledger.json");
    }

    /// A fresh repository with no `.agents` ignore rule anywhere: after
    /// [`ensure_runtime_exclude`], `.agents/runs/ledger.json` must stay
    /// invisible to both `git status --porcelain` and `git add -A`, and
    /// root `.gitignore` must remain untouched.
    #[test]
    fn ensure_runtime_exclude_hides_agents_runs_from_status_and_add() {
        let repo = fresh_repo("hides-runs");

        ensure_runtime_exclude(&repo).expect("ensure_runtime_exclude");
        write_runtime_state(&repo);

        let status = porcelain_status(&repo);
        assert!(
            !status.contains(".agents"),
            "runtime state must not appear in git status: {status}"
        );

        git(&repo, &["add", "-A"]);
        let output = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(repo.as_std_path())
            .output()
            .expect("git diff --cached runs");
        let staged = String::from_utf8(output.stdout).expect("utf8 diff");
        assert!(
            !staged.contains(".agents"),
            "git add -A must not stage runtime state: {staged}"
        );

        assert!(
            !repo.join(".gitignore").exists(),
            "root .gitignore must stay absent, never written by this module"
        );

        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }

    /// A second call with the entry already present must append nothing —
    /// the idempotence [`append_missing_lines`] guarantees.
    #[test]
    fn ensure_runtime_exclude_second_call_appends_nothing() {
        let repo = fresh_repo("idempotent");

        let first = ensure_runtime_exclude(&repo).expect("first ensure_runtime_exclude");
        assert_eq!(first.appended, vec![".agents/runs/".to_string()]);

        let second = ensure_runtime_exclude(&repo).expect("second ensure_runtime_exclude");
        assert!(
            second.appended.is_empty(),
            "second call must append nothing: {:?}",
            second.appended
        );
        assert!(!second.created);

        let contents =
            std::fs::read_to_string(second.path.as_std_path()).expect("read info/exclude");
        assert_eq!(
            contents.matches(".agents/runs/").count(),
            1,
            "the exact-line entry must not be duplicated: {contents:?}"
        );

        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }

    /// Proof 2: from INSIDE a linked worktree, the common-dir resolution
    /// still hides `.agents/runs/`, and the primary checkout's `info/exclude`
    /// carries no exclude beyond the one deliberate `.agents/runs/` line —
    /// the shared-scope side effect this tier deliberately accepts, kept
    /// narrow.
    #[test]
    fn ensure_runtime_exclude_covers_a_linked_worktree_via_the_common_dir() {
        let repo = fresh_repo("linked-worktree");
        std::fs::write(repo.join("root.txt").as_std_path(), b"root\n").expect("write root file");
        git(&repo, &["add", "root.txt"]);
        git(&repo, &["commit", "--quiet", "-m", "root"]);

        ensure_runtime_exclude(&repo).expect("ensure_runtime_exclude on primary checkout");

        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let worktree = root.join(format!(
            "ctx-gitignore-test-linked-worktree-wt-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if worktree.exists() {
            std::fs::remove_dir_all(worktree.as_std_path()).ok();
        }
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "ctx-gitignore-test-branch",
                worktree.as_str(),
            ],
        );

        write_runtime_state(&worktree);
        let status = porcelain_status(&worktree);
        assert!(
            !status.contains(".agents"),
            "runtime state must be hidden from inside the linked worktree: {status}"
        );

        let exclude_path = repo.join(".git/info/exclude");
        let contents =
            std::fs::read_to_string(exclude_path.as_std_path()).expect("read info/exclude");
        for line in contents.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            assert_eq!(
                line, ".agents/runs/",
                "info/exclude must carry only the deliberate runtime-exclude line, found: {line:?}"
            );
        }

        git(&repo, &["worktree", "remove", "--force", worktree.as_str()]);
        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }
}
