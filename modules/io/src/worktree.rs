//! `--worktree` execution capability: dedicated git worktree creation/resume
//! and gitignored context seeding at the IO boundary.
//!
//! A `--worktree` run executes trusted local commands and harness subprocesses
//! inside a dedicated `git worktree` under `.ctx/worktrees/<id>` on a branch
//! `ctx/run/<id>`, so a driven run never mutates the invocation checkout. Core
//! never sees the live worktree path: it stays an operational execution
//! capability threaded through IO command/harness requests, not run-session
//! data. Core does see the minimal `{id, branch}` provenance pair (via
//! `Provenance.worktree`) so a later `ctx traits merge` can re-resolve the
//! path from the ledger.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::procedure::session::{SeedFileDigest, SeedSnapshot};

/// Prepared (created or resumed) worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWorktree {
    pub path: Utf8PathBuf,
    pub branch: String,
    pub resumed: bool,
    /// Seed-time byte digests captured while creating a new worktree. Empty
    /// when resuming an already-registered worktree: no seeding happens on
    /// resume, so there is no new baseline to record.
    pub seed_snapshots: Vec<SeedSnapshot>,
    /// Stable `git-lock-retry` evidence strings emitted while preparing this
    /// worktree (see [`RetryWarnings`]). Empty when no Git call hit
    /// classified transient lock contention.
    pub retry_warnings: Vec<String>,
}

/// Attempts before a Git failure [`classify_transient_lock`] recognizes is
/// surfaced as a typed error instead of retried again.
const MAX_LOCK_RETRY_ATTEMPTS: u32 = 5;
/// Jitter-free exponential backoff (ms), indexed by the attempt about to be
/// retried (attempt 2, 3, 4, 5 — i.e. after the 1st, 2nd, 3rd, 4th failure).
const LOCK_RETRY_BACKOFF_MS: [u64; 4] = [100, 200, 400, 800];

/// Subdirectory of the private per-worktree seed-baseline root
/// ([`seed_baseline_root`]) that mirrors seed-time ancestor bytes. Kept under
/// its own literal subdirectory name, distinct from
/// [`SEED_BASELINE_SCRATCH_SUBDIR`], so a seeded repository-relative path —
/// however it is spelled — can never alias an internal merge-scratch control
/// file: the two namespaces never share a parent directory with each other's
/// contents.
const SEED_BASELINE_ANCESTOR_SUBDIR: &str = "ancestors";
/// Subdirectory of the private per-worktree seed-baseline root holding
/// transient `git merge-file` scratch input/output files. See
/// [`SEED_BASELINE_ANCESTOR_SUBDIR`].
const SEED_BASELINE_SCRATCH_SUBDIR: &str = "scratch";

/// Stable reason code for a Git failure classified as short-lived repository
/// lock contention. See [`classify_transient_lock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransientLockReason {
    IndexLock,
    ShallowLock,
    AnotherProcess,
}

impl TransientLockReason {
    fn code(self) -> &'static str {
        match self {
            Self::IndexLock => "index-lock",
            Self::ShallowLock => "shallow-lock",
            Self::AnotherProcess => "another-process",
        }
    }
}

/// Classify a failed, non-timeout Git process result as short-lived
/// repository lock contention. Matches only the three approved transient
/// signatures (`index.lock`, `shallow.lock`, `Another git process seems to
/// be running`) against lowercased stderr — a bare `.lock` mention that is
/// not one of these three is not retried, and neither is a timed-out result
/// (a timeout is its own distinct failure mode, not lock contention).
fn classify_transient_lock(output: &crate::command::RunOutput) -> Option<TransientLockReason> {
    if output.success || output.timed_out {
        return None;
    }
    let stderr = output.stderr.to_ascii_lowercase();
    if stderr.contains("index.lock") {
        Some(TransientLockReason::IndexLock)
    } else if stderr.contains("shallow.lock") {
        Some(TransientLockReason::ShallowLock)
    } else if stderr.contains("another git process seems to be running") {
        Some(TransientLockReason::AnotherProcess)
    } else {
        None
    }
}

/// Stable, path-free `git-lock-retry` evidence accumulated across the Git
/// calls of one logical worktree/merge operation lifecycle. Never carries
/// raw stderr or machine-local paths — only the operation label, attempt
/// count, backoff, and classified reason code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetryWarnings(Vec<String>);

impl RetryWarnings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<String> {
        self.0
    }

    fn push(&mut self, warning: String) {
        self.0.push(warning);
    }
}

/// Run one `git` invocation under `operation`'s bounded retry policy: up to
/// [`MAX_LOCK_RETRY_ATTEMPTS`] total attempts with jitter-free exponential
/// backoff, but only for failures [`classify_transient_lock`] recognizes as
/// short-lived lock contention. A success, or any unclassified failure,
/// returns on the first attempt exactly like a bare `run_git` call — the
/// caller inspects `output.success` itself. The fifth matching failure
/// returns the typed Git error `git_error` would produce for that attempt,
/// augmented with the bounded-attempt context.
fn run_git_retrying(
    dir: &Utf8Path,
    args: &[&str],
    operation: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<crate::command::RunOutput> {
    run_git_retrying_with_timeout(
        dir,
        args,
        operation,
        warnings,
        crate::git_process::PLUMBING_TIMEOUT_MS,
    )
}

/// Same retry policy as [`run_git_retrying`], but with an explicit timeout
/// rather than the plumbing default — for the long-running operations
/// (`rebase`, `rebase --continue`, `worktree add`) whose duration depends on
/// working-tree size or replay length rather than being a fixed local
/// plumbing cost.
fn run_git_retrying_with_timeout(
    dir: &Utf8Path,
    args: &[&str],
    operation: &str,
    warnings: &mut RetryWarnings,
    timeout_ms: u64,
) -> crate::Result<crate::command::RunOutput> {
    let mut attempt: u32 = 1;
    loop {
        let output = run_git_with_timeout(dir, args, timeout_ms)?;
        let Some(reason) = classify_transient_lock(&output) else {
            return Ok(output);
        };
        if attempt >= MAX_LOCK_RETRY_ATTEMPTS {
            let command = format!("git {}", args.join(" "));
            return Err(git_error_with_retry_context(
                &command, &output, attempt, reason,
            ));
        }
        let backoff_ms = LOCK_RETRY_BACKOFF_MS[(attempt - 1) as usize];
        warnings.push(format!(
            "git-lock-retry operation={operation} attempt={}/{MAX_LOCK_RETRY_ATTEMPTS} backoff-ms={backoff_ms} reason={}",
            attempt + 1,
            reason.code()
        ));
        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        attempt += 1;
    }
}

/// Augment the typed Git error `git_error` would produce with the exhausted
/// retry budget's attempt count and classified reason, so the CLI-facing
/// error names why a fifth classified lock failure was not retried again.
fn git_error_with_retry_context(
    command: &str,
    output: &crate::command::RunOutput,
    attempts: u32,
    reason: TransientLockReason,
) -> crate::Error {
    match git_error(command, output) {
        crate::Error::Environment(crate::environment::Error::Git {
            command,
            path,
            exit_status,
            timed_out,
            message,
        }) => crate::environment::Error::Git {
            command,
            path,
            exit_status,
            timed_out,
            message: format!(
                "{message} (git-lock-retry exhausted: attempts={attempts}/{MAX_LOCK_RETRY_ATTEMPTS} reason={})",
                reason.code()
            ),
        }
        .into(),
        other => other,
    }
}

/// Derive a short, readable worktree ID from an already-unique session ID
/// (`session-<digest>`) instead of minting new entropy: the session ID is
/// already collision-free for this process, so its digest tail is reused.
pub fn derive_worktree_id(session_id: &str) -> String {
    let digest = session_id.strip_prefix("session-").unwrap_or(session_id);
    let hex: String = digest
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    let short = if hex.is_empty() {
        "0".to_string()
    } else {
        hex.chars().take(12).collect::<String>()
    };
    format!("wt-{short}")
}

/// Create the dedicated worktree for `id`, seeding the declared gitignored
/// context roots. Fails if `id` is already a registered worktree: a fresh
/// `run`/`session start` must never execute inside a checkout another
/// invocation (concurrent or prior) is already using — that reopens the
/// collision this capability exists to close. Only a standalone `drive`
/// resuming its own prior run should reuse an existing worktree; use
/// [`resume_or_prepare_worktree`] for that caller.
pub fn prepare_worktree(
    id: &str,
    contents: WorktreeContents<'_>,
    setup: &[Vec<String>],
    setup_env: &BTreeMap<String, String>,
    setup_timeout_ms: Option<u64>,
    setup_capture_bytes: Option<u64>,
    worktree_add_timeout_ms: Option<u64>,
) -> crate::Result<PreparedWorktree> {
    let (repo_root, path, branch) = resolve_worktree_location(id)?;
    let mut warnings = RetryWarnings::new();
    if let Some(existing_branch) = existing_worktree_branch(&repo_root, &path, &mut warnings)? {
        return Err(config_error(
            "worktree.id",
            format!(
                "worktree path {path} is already registered on branch {existing_branch:?}; a new run cannot reuse an existing worktree"
            ),
        ));
    }
    create_new_worktree(
        &repo_root,
        &path,
        &branch,
        contents,
        SetupPlan {
            setup,
            setup_env,
            budget: SetupBudget {
                timeout_ms: setup_timeout_ms.unwrap_or(DEFAULT_SETUP_TIMEOUT_MS),
                capture_limit: setup_capture_bytes
                    .map_or(DEFAULT_SETUP_CAPTURE_BYTES, |bytes| bytes as usize),
            },
            progress: None,
        },
        worktree_add_timeout_ms.unwrap_or(crate::git_process::LONG_TIMEOUT_MS),
        &mut warnings,
    )
}

/// What a fresh worktree is populated with beyond the git checkout itself:
/// declared gitignored context (`seeds`, copied and baselined) and declared
/// regenerable artifacts (`warm`, copy-on-write cloned and never baselined).
/// Bundled for the same reason [`WorktreePrepareBudget`] is — adding P564's
/// warm list keeps these functions under clippy's argument-count ceiling
/// without a new `#[allow]`.
#[derive(Debug, Clone, Copy)]
pub struct WorktreeContents<'a> {
    pub seeds: &'a [String],
    pub warm: &'a [String],
}

/// Timeout/capture knobs for [`resume_or_prepare_worktree`], bundled so
/// adding the P551 `progress` observer alongside them keeps the function
/// under clippy's argument-count ceiling without a new `#[allow]`.
pub struct WorktreePrepareBudget {
    pub setup_timeout_ms: Option<u64>,
    pub setup_capture_bytes: Option<u64>,
    pub worktree_add_timeout_ms: Option<u64>,
}

/// Resume the dedicated worktree for `id` if one is already registered on its
/// expected branch, otherwise create it. Only a standalone `drive` invocation
/// resuming a run it already started should call this: it is the one caller
/// that legitimately re-enters an existing worktree rather than starting a
/// new run inside it.
///
/// `progress`, when given, is called with a short phase-boundary message
/// ("creating worktree", "seeding", "setup: <command>", "setup done (<Ns>)")
/// as each step of a freshly-created worktree runs — never for a resumed
/// one, since resuming does no work worth narrating. P551: this is how the
/// live run pane shows setup activity instead of sitting frozen while
/// `[worktree] setup` commands run underneath it.
pub fn resume_or_prepare_worktree(
    id: &str,
    contents: WorktreeContents<'_>,
    setup: &[Vec<String>],
    setup_env: &BTreeMap<String, String>,
    budget: WorktreePrepareBudget,
    progress: Option<&dyn Fn(&str)>,
) -> crate::Result<PreparedWorktree> {
    let (repo_root, path, branch) = resolve_worktree_location(id)?;
    let mut warnings = RetryWarnings::new();
    if let Some(existing_branch) = existing_worktree_branch(&repo_root, &path, &mut warnings)? {
        if existing_branch == branch {
            return Ok(PreparedWorktree {
                path,
                branch,
                resumed: true,
                seed_snapshots: Vec::new(),
                retry_warnings: warnings.into_vec(),
            });
        }
        return Err(config_error(
            "worktree.id",
            format!(
                "worktree path {path} is already registered on branch {existing_branch:?}, expected {branch:?}"
            ),
        ));
    }
    create_new_worktree(
        &repo_root,
        &path,
        &branch,
        contents,
        SetupPlan {
            setup,
            setup_env,
            budget: SetupBudget {
                timeout_ms: budget.setup_timeout_ms.unwrap_or(DEFAULT_SETUP_TIMEOUT_MS),
                capture_limit: budget
                    .setup_capture_bytes
                    .map_or(DEFAULT_SETUP_CAPTURE_BYTES, |bytes| bytes as usize),
            },
            progress,
        },
        budget
            .worktree_add_timeout_ms
            .unwrap_or(crate::git_process::LONG_TIMEOUT_MS),
        &mut warnings,
    )
}

/// Outcome of `git rebase <onto>` run inside a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// The rebase completed with no conflicts.
    Clean,
    /// The rebase stopped on conflicts; `paths` are the currently unmerged
    /// (conflicted) files, relative to the worktree root.
    Conflicts { paths: Vec<String> },
    /// The rebase failed for a reason other than a content conflict.
    Failed { message: String },
}

/// Resolve the repository root and confirm `id` is registered on exactly its
/// expected `ctx/run/<id>` branch. Used by `ctx traits merge` to verify a
/// resolved run's worktree provenance before touching any Git state.
pub fn verify_worktree_registration(
    id: &str,
    expected_branch: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<Utf8PathBuf> {
    let (repo_root, path, branch) = resolve_worktree_location(id)?;
    if branch != expected_branch {
        return Err(config_error(
            "worktree.id",
            format!(
                "worktree id {id:?} derives branch {branch:?}, but the run-session ledger recorded {expected_branch:?}"
            ),
        ));
    }
    match existing_worktree_branch(&repo_root, &path, warnings)? {
        Some(registered) if registered == expected_branch => Ok(path),
        Some(registered) => Err(config_error(
            "worktree.id",
            format!(
                "worktree {id:?} is registered on branch {registered:?}, expected {expected_branch:?}"
            ),
        )),
        None => Err(config_error(
            "worktree.id",
            format!("worktree {id:?} is not a registered git worktree"),
        )),
    }
}

/// `true` when `git status --porcelain` reports no pending changes in `dir`
/// (a worktree path or the repository root).
pub fn is_clean(dir: &Utf8Path) -> crate::Result<bool> {
    let output = run_git(dir, &["status", "--porcelain"])?;
    if !output.success {
        return Err(git_error("git status --porcelain", &output));
    }
    Ok(output.stdout.trim().is_empty())
}

/// Like [`is_clean`], but tolerates exactly one untracked path: a nested
/// `.ctx/.gitignore` this same `--worktree` invocation may have just created
/// via [`crate::gitignore::ensure_nested_gitignore`] (P446). That file is
/// deliberately left uncommitted for the owner to review and commit — see
/// its own module docs — so the automated main-side landing preflight this
/// invocation also owns must not itself refuse to land over its own
/// scaffolding. Any other untracked, modified, or staged path still blocks
/// exactly as `is_clean` does. Used only for the invocation checkout (main);
/// worktree-side cleanliness checks keep using plain `is_clean`.
pub fn is_clean_for_landing(dir: &Utf8Path) -> crate::Result<bool> {
    let output = run_git(dir, &["status", "--porcelain"])?;
    if !output.success {
        return Err(git_error("git status --porcelain", &output));
    }
    Ok(output
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim() == "?? .ctx/.gitignore"))
}

/// Resolve `rev` (e.g. `"HEAD"`, `"main"`) to its full commit hash in `dir`.
pub fn rev_parse(dir: &Utf8Path, rev: &str, warnings: &mut RetryWarnings) -> crate::Result<String> {
    let output = run_git_retrying(dir, &["rev-parse", rev], "rev-parse", warnings)?;
    if !output.success {
        return Err(git_error(&format!("git rev-parse {rev}"), &output));
    }
    Ok(output.stdout.trim().to_string())
}

/// Source layer that produced [`resolve_default_branch`]'s resolved branch
/// name, named in refusals/warnings/doctor output so a misresolution is
/// debuggable rather than a bare string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultBranchSource {
    Config,
    OriginHead,
    InitDefaultBranch,
    Fallback,
}

impl DefaultBranchSource {
    /// Stable, human-readable label for this source, used in refusal text,
    /// `MergeReport::warnings`, and `ctx traits doctor --config`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Config => "[merge] branch",
            Self::OriginHead => "origin/HEAD",
            Self::InitDefaultBranch => "init.defaultBranch",
            Self::Fallback => "fallback",
        }
    }
}

/// Resolve the repository's landing/default branch (P488), in priority
/// order: an explicit `configured` value (the `[merge] branch` config, P418
/// layering — the caller resolves layering, this function only takes the
/// winning value), then the `origin/HEAD` symref, then
/// `init.defaultBranch`, then a literal `"main"` fallback. Resolved once per
/// merge attempt by the caller (never cached across attempts) — a repaired
/// `origin/HEAD` is picked up on retry. Never validates the resolved name
/// exists as a real ref in this repository: an unresolvable name surfaces as
/// the existing rev-parse/rebase failure, now naming the branch and its
/// source.
pub fn resolve_default_branch(
    repo_root: &Utf8Path,
    configured: Option<&str>,
    warnings: &mut RetryWarnings,
) -> crate::Result<(String, DefaultBranchSource)> {
    if let Some(branch) = configured {
        return Ok((branch.to_string(), DefaultBranchSource::Config));
    }
    let symref = run_git_retrying(
        repo_root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        "resolve-default-branch-origin-head",
        warnings,
    )?;
    if symref.success {
        if let Some(branch) = symref
            .stdout
            .trim()
            .strip_prefix("refs/remotes/origin/")
            .filter(|branch| !branch.is_empty())
        {
            return Ok((branch.to_string(), DefaultBranchSource::OriginHead));
        }
    }
    let init_default = run_git_retrying(
        repo_root,
        &["config", "--get", "init.defaultBranch"],
        "resolve-default-branch-init-default-branch",
        warnings,
    )?;
    if init_default.success {
        let branch = init_default.stdout.trim();
        if !branch.is_empty() {
            return Ok((branch.to_string(), DefaultBranchSource::InitDefaultBranch));
        }
    }
    Ok(("main".to_string(), DefaultBranchSource::Fallback))
}

/// Current branch name in `dir` (e.g. the invocation checkout).
pub fn current_branch(dir: &Utf8Path) -> crate::Result<String> {
    let output = run_git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !output.success {
        return Err(git_error("git rev-parse --abbrev-ref HEAD", &output));
    }
    Ok(output.stdout.trim().to_string())
}

/// `true` when a `git rebase` is currently in progress in `dir`.
pub fn rebase_in_progress(dir: &Utf8Path) -> crate::Result<bool> {
    for marker in ["rebase-merge", "rebase-apply"] {
        let output = run_git(dir, &["rev-parse", "--git-path", marker])?;
        if !output.success {
            return Err(git_error("git rev-parse --git-path", &output));
        }
        let marker_path = output.stdout.trim();
        if marker_path.is_empty() {
            continue;
        }
        let resolved = if Utf8Path::new(marker_path).is_absolute() {
            Utf8PathBuf::from(marker_path)
        } else {
            dir.join(marker_path)
        };
        if std::fs::symlink_metadata(resolved.as_std_path()).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run `git rebase <onto>` inside `worktree_path`, classifying the result.
/// `timeout_ms` is explicit at the call site (see
/// [`crate::git_process::LONG_TIMEOUT_MS`]) rather than read from a
/// process-global config inside this module, so the policy stays visible
/// where it is decided.
pub fn rebase(
    worktree_path: &Utf8Path,
    onto: &str,
    warnings: &mut RetryWarnings,
    timeout_ms: u64,
) -> crate::Result<RebaseOutcome> {
    let output = run_git_retrying_with_timeout(
        worktree_path,
        &["rebase", onto],
        "rebase",
        warnings,
        timeout_ms,
    )?;
    if output.success {
        return Ok(RebaseOutcome::Clean);
    }
    let paths = conflicted_paths(worktree_path)?;
    if !paths.is_empty() {
        return Ok(RebaseOutcome::Conflicts { paths });
    }
    Ok(RebaseOutcome::Failed {
        message: format!(
            "git rebase {onto} failed: timed-out={} stderr={}",
            output.timed_out,
            output.stderr.trim()
        ),
    })
}

/// Continue an in-progress rebase in `worktree_path` after conflicts have
/// been staged, classifying the result exactly like [`rebase`]: a further
/// commit in the replay can itself conflict, so callers must be prepared to
/// loop. `core.editor=true` suppresses the interactive commit-message editor
/// `--continue` would otherwise try to open.
pub fn continue_rebase(
    worktree_path: &Utf8Path,
    warnings: &mut RetryWarnings,
    timeout_ms: u64,
) -> crate::Result<RebaseOutcome> {
    let output = run_git_retrying_with_timeout(
        worktree_path,
        &["-c", "core.editor=true", "rebase", "--continue"],
        "rebase-continue",
        warnings,
        timeout_ms,
    )?;
    if output.success {
        return Ok(RebaseOutcome::Clean);
    }
    let paths = conflicted_paths(worktree_path)?;
    if !paths.is_empty() {
        return Ok(RebaseOutcome::Conflicts { paths });
    }
    Ok(RebaseOutcome::Failed {
        message: format!(
            "git rebase --continue failed: timed-out={} stderr={}",
            output.timed_out,
            output.stderr.trim()
        ),
    })
}

/// Currently unmerged (conflicted) paths in `worktree_path`, relative to its
/// root, sorted for determinism.
pub fn conflicted_paths(worktree_path: &Utf8Path) -> crate::Result<Vec<String>> {
    let output = run_git(worktree_path, &["diff", "--name-only", "--diff-filter=U"])?;
    if !output.success {
        return Err(git_error("git diff --name-only --diff-filter=U", &output));
    }
    let mut paths: Vec<String> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Resolve the merge base of `a` and `b` (e.g. the run branch's fork point
/// from `main`) in `dir`.
pub fn merge_base(
    dir: &Utf8Path,
    a: &str,
    b: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<String> {
    let output = run_git_retrying(dir, &["merge-base", a, b], "merge-base", warnings)?;
    if !output.success {
        return Err(git_error(&format!("git merge-base {a} {b}"), &output));
    }
    Ok(output.stdout.trim().to_string())
}

/// Quote `path` as a literal Git pathspec (`:(literal)<path>`) so pathspec
/// magic characters (`*`, `?`, `[`, a leading `:`) in a discovered path are
/// matched exactly rather than reinterpreted as glob/magic syntax.
fn literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

/// Paths changed between `base` and `head` in `dir`, sorted and deduplicated
/// for deterministic overlap comparison. Uses NUL-delimited output and
/// disables rename detection so unusual filenames (embedded newlines) and
/// rename/delete pairs are both reported as their plain before/after paths
/// rather than collapsed into a single rename record — a stale-base overlap
/// check must not miss a path just because it was also renamed.
pub fn changed_paths(
    dir: &Utf8Path,
    base: &str,
    head: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<Vec<String>> {
    let command = format!("git diff --no-renames --name-only -z {base} {head}");
    let output = run_git_retrying(
        dir,
        &["diff", "--no-renames", "--name-only", "-z", base, head],
        "diff-changed-paths",
        warnings,
    )?;
    if !output.success {
        return Err(git_error(&command, &output));
    }
    output.refuse_if_truncated(&command)?;
    // `-z` NUL-delimits and terminates every record, including the last, so
    // splitting on '\0' yields exactly one trailing empty token to drop.
    // Paths are taken verbatim (no trimming) — leading/trailing whitespace in
    // a filename is a valid, if unusual, byte of the path.
    let mut paths: Vec<String> = output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Commits in `base..head` (in `dir`) that touched any of `paths`, newest
/// first as `git log` reports them, deduplicated. Used to name the `main`
/// commits responsible for a detected stale-base overlap. Each path is
/// queried as a literal pathspec so overlap paths containing pathspec magic
/// characters are matched exactly rather than reinterpreted by Git.
pub fn commits_touching_paths(
    dir: &Utf8Path,
    base: &str,
    head: &str,
    paths: &[String],
    warnings: &mut RetryWarnings,
) -> crate::Result<Vec<String>> {
    let range = format!("{base}..{head}");
    let literal_paths: Vec<String> = paths.iter().map(|path| literal_pathspec(path)).collect();
    let mut args: Vec<&str> = vec!["log", "--format=%H", &range, "--"];
    args.extend(literal_paths.iter().map(String::as_str));
    let command = "git log --format=%H <range> -- <literal paths>".to_string();
    let output = run_git_retrying(dir, &args, "log-touching-paths", warnings)?;
    if !output.success {
        return Err(git_error(&command, &output));
    }
    output.refuse_if_truncated(&command)?;
    let mut commits: Vec<String> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    commits.dedup();
    Ok(commits)
}

/// Exact bytes (or `None` for an add/delete conflict's absent side) of one
/// Git conflict stage for a currently-unmerged path, read via `git show
/// :<stage>:<path>` directly through `std::process::Command` rather than the
/// shared `String`-returning [`run_git`] boundary: a conflicted binary file's
/// stage content must reach a P420 `--deep` merger prompt byte-exact, and
/// `run_git`'s lossy UTF-8 capture would silently corrupt it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictBlobs {
    /// Stage 1: the common ancestor.
    pub base: Option<Vec<u8>>,
    /// Stage 2: during a `git rebase`, this is the branch being rebased
    /// *onto* (i.e. `main`) — NOT the run branch's own content, despite the
    /// "ours" label Git itself uses for this stage in a rebase.
    pub main_side: Option<Vec<u8>>,
    /// Stage 3: during a `git rebase`, this is the content of the commit
    /// currently being replayed from the run branch — NOT `main`, despite
    /// the "theirs" label Git itself uses for this stage in a rebase.
    pub branch_side: Option<Vec<u8>>,
}

/// Base64-encode raw bytes (standard alphabet, padded) for embedding exact
/// conflict-stage bytes in a P420 `--deep` merger prompt, where they must
/// survive as opaque text regardless of encoding or binary content.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The 1-based, inclusive line ranges of every distinct `<<<<<<< ... =======
/// ... >>>>>>>` conflict marker block currently present in `worktree_path`'s
/// working-tree copy of `path` — one entry per contiguous textual conflict
/// region, unlike [`conflict_marker_line_range`] (which spans first-to-last
/// for a single three-way-merge output). Used to give a P420 `--deep` merger
/// one hunk id per conflict region in a multi-conflict file rather than one
/// coarse whole-file hunk. Returns an empty `Vec` — meaning "treat this path
/// as one whole-file hunk" — when the working-tree file is missing (a
/// pure add/delete conflict form with no merged content to mark up) or is
/// not valid UTF-8 (a binary conflict, which Git never marks up with
/// textual conflict markers).
pub fn conflict_marker_regions(
    worktree_path: &Utf8Path,
    path: &str,
) -> crate::Result<Vec<(usize, usize)>> {
    let full_path = worktree_path.join(path);
    let bytes = match std::fs::read(full_path.as_std_path()) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: full_path.to_string(),
                source,
            }
            .into());
        }
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(Vec::new());
    };
    let mut regions = Vec::new();
    let mut current_start: Option<usize> = None;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.starts_with("<<<<<<<") && current_start.is_none() {
            current_start = Some(line_number);
        } else if line.starts_with(">>>>>>>") {
            if let Some(start) = current_start.take() {
                regions.push((start, line_number));
            }
        }
    }
    Ok(regions)
}

/// Read all three conflict stages for `path` (currently unmerged in
/// `worktree_path`'s index). See [`ConflictBlobs`] for the rebase-specific
/// stage-to-side mapping this deliberately spells out rather than reusing
/// Git's own ours/theirs vocabulary. Stage presence is determined from the
/// unmerged index itself (`git ls-files --stage --unmerged`), not from
/// whether `git show` happens to succeed: an index-confirmed absent stage
/// (the ordinary add/delete-conflict shape) yields `None`, while a `git show`
/// failure for an index-confirmed *present* stage (a real Git/process/path
/// failure) is a typed error, never silently downgraded to `<absent>` — a
/// P420 `--deep` merger prompt's exact-byte contract depends on this
/// distinction.
pub fn conflict_stage_blobs(worktree_path: &Utf8Path, path: &str) -> crate::Result<ConflictBlobs> {
    let present = unmerged_stages(worktree_path, path)?;
    Ok(ConflictBlobs {
        base: read_stage_blob_if_present(worktree_path, 1, path, &present)?,
        main_side: read_stage_blob_if_present(worktree_path, 2, path, &present)?,
        branch_side: read_stage_blob_if_present(worktree_path, 3, path, &present)?,
    })
}

/// The Git conflict stages (a subset of `{1, 2, 3}`) the unmerged index
/// actually records for `path`, via `git ls-files --stage --unmerged --
/// <literal path>` (`<mode> <sha> <stage>\t<path>` lines). A stage absent
/// from this list is an index-confirmed absent conflict side (add/delete);
/// any other read failure for `path` is a real error, never an absent side.
fn unmerged_stages(worktree_path: &Utf8Path, path: &str) -> crate::Result<Vec<u8>> {
    let pathspec = literal_pathspec(path);
    let command = format!("git ls-files --stage --unmerged -- {pathspec}");
    let output = run_git(
        worktree_path,
        &["ls-files", "--stage", "--unmerged", "--", &pathspec],
    )?;
    if !output.success {
        return Err(git_error(&command, &output));
    }
    output.refuse_if_truncated(&command)?;
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| {
            let (fields, _) = line.split_once('\t')?;
            let stage_field = fields.split_whitespace().nth(2)?;
            stage_field.parse::<u8>().ok()
        })
        .collect())
}

fn read_stage_blob_if_present(
    worktree_path: &Utf8Path,
    stage: u8,
    path: &str,
    present_stages: &[u8],
) -> crate::Result<Option<Vec<u8>>> {
    if !present_stages.contains(&stage) {
        return Ok(None);
    }
    let object = format!(":{stage}:{path}");
    let command = format!("git show {object}");
    let output = crate::git_process::run_bytes(crate::git_process::Request {
        exec_dir: Some(worktree_path),
        cwd: None,
        args: &["show", &object],
        success_exit_code: &[0],
        timeout_ms: 30_000,
        capture_limit: 64 * 1024 * 1024,
    })?;
    if !output.success {
        return Err(crate::git_process::error_bytes(&command, &output));
    }
    output.refuse_if_truncated(&command)?;
    Ok(Some(output.stdout))
}

/// Commits in `base..main` (in `dir`) that touched `path`, oldest first, as
/// `(commit, subject)` pairs — the per-path `main`-side history a P420
/// `--deep` merger needs to judge whether a branch-side change conflicts with
/// an intentional `main` decision. Queried as a literal pathspec so a path
/// containing pathspec magic characters is matched exactly.
pub fn main_history_for_path(
    dir: &Utf8Path,
    base: &str,
    main: &str,
    path: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<Vec<(String, String)>> {
    let range = format!("{base}..{main}");
    let pathspec = literal_pathspec(path);
    let command = format!("git log --format=%H%x1f%s --reverse {range} -- {pathspec}");
    let output = run_git_retrying(
        dir,
        &[
            "log",
            "--format=%H%x1f%s",
            "--reverse",
            &range,
            "--",
            &pathspec,
        ],
        "log-main-history-for-path",
        warnings,
    )?;
    if !output.success {
        return Err(git_error(&command, &output));
    }
    output.refuse_if_truncated(&command)?;
    Ok(output
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once('\u{1f}') {
            Some((sha, subject)) => (sha.to_string(), subject.to_string()),
            None => (line.to_string(), String::new()),
        })
        .collect())
}

/// A worktree-side path's on-disk identity, captured explicitly rather than
/// collapsing "absent" and "exists but isn't a plain file" into the same
/// `None` a bare `Option<Digest>` would produce. A P420 `--deep` merger's
/// undeclared-supporting-edit check must be able to tell "this seed path
/// never existed" apart from "this seed path now exists as a directory" or
/// "...as a symlink" — both real, touchable entries a rebase replay or the
/// merger itself could have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeEntry {
    /// No filesystem entry at this path.
    Absent,
    /// A plain file, identified by the digest of its bytes.
    Regular(Digest),
    /// A directory, symlink, or other non-regular entry. Git itself only
    /// ever stores blobs (regular files, optionally executable) and symlinks
    /// as blob-shaped entries; a directory or other special file appearing
    /// here is inherently a worktree-only, non-trackable identity distinct
    /// from every [`Regular`](Self::Regular) content digest.
    Other,
}

/// One repository-relative path's content/object identity, captured on both
/// the index side (the raw `git ls-files --stage` blob sha, or a
/// stage-keyed `"<stage>=<sha>,..."` string while the path is still an
/// unmerged conflict — a distinct value per stage combination so a rebase
/// progressing through further conflict stages on the same path is itself
/// visible) and the worktree side (a [`WorktreeEntry`] capturing the file's
/// actual on-disk identity, not just its bytes when it happens to be a plain
/// file). Two fingerprints for the same path compare equal only when both
/// the index object and the on-disk entry are unchanged — unlike a
/// two-character `git status` code, which stays the same (e.g. `"M "`)
/// across a second edit to an already-staged path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathFingerprint {
    pub index_object: Option<String>,
    pub worktree_entry: WorktreeEntry,
}

/// The explicit [`WorktreeEntry`] identity of `full_path` on disk:
/// [`WorktreeEntry::Absent`] when nothing exists there,
/// [`WorktreeEntry::Regular`] with a content digest for a plain file, and
/// [`WorktreeEntry::Other`] for a directory, symlink, or any other kind —
/// checked with `symlink_metadata` so a symlink is identified as itself,
/// never silently followed and misreported as the digest of whatever it
/// happens to point at.
fn worktree_entry_at(full_path: &Utf8Path) -> crate::Result<WorktreeEntry> {
    let metadata = match std::fs::symlink_metadata(full_path.as_std_path()) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorktreeEntry::Absent);
        }
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: full_path.to_string(),
                source,
            }
            .into());
        }
    };
    if !metadata.is_file() {
        return Ok(WorktreeEntry::Other);
    }
    let bytes = std::fs::read(full_path.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: full_path.to_string(),
            source,
        }
    })?;
    Ok(WorktreeEntry::Regular(Digest::from_bytes(&bytes)))
}

/// Snapshot `worktree_path`'s tracked-and-untracked-path content identity
/// immediately before or after dispatching a P420 `--deep` merger call: one
/// [`PathFingerprint`] per path present in the index (`git ls-files
/// --stage`, including every currently unmerged conflict stage), as an
/// untracked, non-ignored file (`git ls-files --others --exclude-standard`),
/// or materialized under one of `declared_seed_roots` — the run's declared
/// seed roots (see [`plan_harvest_seeds`]), which are typically
/// `.gitignore`d and so invisible to both `ls-files` queries above but are
/// still real, harvestable paths a `--deep` merger call can edit. Paired
/// with a second snapshot taken right after the call and
/// [`touched_paths_delta`] to compute exactly the paths that ONE call's
/// edits touched, by actual content/object identity rather than a status
/// label that can stay unchanged across a real edit.
pub fn worktree_content_fingerprint(
    worktree_path: &Utf8Path,
    declared_seed_roots: &[String],
) -> crate::Result<std::collections::BTreeMap<String, PathFingerprint>> {
    let staged = run_git(worktree_path, &["ls-files", "--stage", "-z"])?;
    if !staged.success {
        return Err(git_error("git ls-files --stage -z", &staged));
    }
    let mut index_objects: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for record in staged
        .stdout
        .split('\0')
        .filter(|record| !record.is_empty())
    {
        let Some((fields, path)) = record.split_once('\t') else {
            continue;
        };
        let mut parts = fields.split_whitespace();
        let _mode = parts.next();
        let Some(sha) = parts.next() else {
            continue;
        };
        let stage = parts.next().unwrap_or("0");
        if stage == "0" {
            index_objects.insert(path.to_string(), sha.to_string());
        } else {
            let entry = index_objects.entry(path.to_string()).or_default();
            if !entry.is_empty() {
                entry.push(',');
            }
            entry.push_str(&format!("{stage}={sha}"));
        }
    }

    let others = run_git(
        worktree_path,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    if !others.success {
        return Err(git_error(
            "git ls-files --others --exclude-standard -z",
            &others,
        ));
    }
    let mut all_paths: std::collections::BTreeSet<String> = index_objects.keys().cloned().collect();
    for path in others.stdout.split('\0').filter(|path| !path.is_empty()) {
        all_paths.insert(path.to_string());
    }

    // Every declared seed root is typically `.gitignore`d (see
    // `plan_harvest_seeds`'s own doc), so neither `ls-files --stage` nor
    // `ls-files --others --exclude-standard` above will ever surface a path
    // materialized under one. Walk them the same way `plan_harvest_seeds`
    // does so a supporting edit a `--deep` merger makes there is still part
    // of the monitored path universe.
    let mut seed_files: std::collections::BTreeMap<String, Utf8PathBuf> =
        std::collections::BTreeMap::new();
    for seed in declared_seed_roots {
        let root = validate_seed_path(seed)?;
        let worktree_root = worktree_path.join(&root);
        collect_regular_files(&worktree_root, &root, &mut seed_files)?;
    }
    for path in seed_files.keys() {
        all_paths.insert(path.clone());
    }

    let mut fingerprint = std::collections::BTreeMap::new();
    for path in all_paths {
        let index_object = index_objects
            .get(&path)
            .cloned()
            .filter(|sha| !sha.is_empty());
        let full_path = worktree_path.join(&path);
        let worktree_entry = worktree_entry_at(&full_path)?;
        fingerprint.insert(
            path,
            PathFingerprint {
                index_object,
                worktree_entry,
            },
        );
    }
    Ok(fingerprint)
}

/// The repository-relative paths a P420 `--deep` merger call actually
/// touched: every path whose [`PathFingerprint`] in `post` (captured right
/// after the call) differs from its fingerprint in `pre` (captured right
/// before it), by content/object identity rather than a status label — plus
/// every path present in `pre` but altogether absent from `post` (a path the
/// call deleted). A pre-staged, non-conflicting path the rebase replay
/// already touched before the merger ran appears in both snapshots with an
/// identical fingerprint and is correctly excluded. Compared against the
/// merger's declared decision receipt so an undeclared supporting edit is
/// rejected rather than silently trusted.
pub fn touched_paths_delta(
    pre: &std::collections::BTreeMap<String, PathFingerprint>,
    post: &std::collections::BTreeMap<String, PathFingerprint>,
) -> Vec<String> {
    let mut touched: Vec<String> = post
        .iter()
        .filter(|(path, fingerprint)| pre.get(path.as_str()) != Some(*fingerprint))
        .map(|(path, _)| path.clone())
        .collect();
    touched.extend(
        pre.keys()
            .filter(|path| !post.contains_key(path.as_str()))
            .cloned(),
    );
    touched.sort();
    touched.dedup();
    touched
}

/// Amend `worktree_path`'s current commit (its rebase-replayed tip, once a
/// reconciliation is fully complete and the tree is clean) with repeatable
/// `--trailer` entries, keeping the existing message and authorship
/// (`--no-edit`). Used to attach P420 `--deep` reconciliation's canonical
/// compact `Ctx-Merge-Decision` entries to the landed commit. Callers must
/// re-`rev_parse` `HEAD` after this: amending always produces a new commit
/// hash.
pub fn amend_head_with_trailers(
    worktree_path: &Utf8Path,
    trailers: &[String],
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    if trailers.is_empty() {
        return Ok(());
    }
    let mut args: Vec<String> = vec![
        "commit".to_string(),
        "--amend".to_string(),
        "--no-edit".to_string(),
    ];
    for trailer in trailers {
        args.push("--trailer".to_string());
        args.push(trailer.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git_retrying(worktree_path, &arg_refs, "commit-amend-trailers", warnings)?;
    if !output.success {
        return Err(git_error(
            "git commit --amend --no-edit --trailer ...",
            &output,
        ));
    }
    Ok(())
}

/// Amend `worktree_path`'s current commit to fold currently staged changes
/// into the tip, keeping the existing message and authorship (`--no-edit`),
/// with no trailer additions. Used by P463's `[[merge.generated]]` rebuild
/// step to fold regenerated bytes into the landing commit — sequenced BEFORE
/// [`amend_head_with_trailers`]'s (separate, optional) `Ctx-Merge-Decision`
/// amend, so the two collapse to one tip. Callers must re-`rev_parse` `HEAD`
/// after this: amending always produces a new commit hash.
pub fn amend_head(worktree_path: &Utf8Path, warnings: &mut RetryWarnings) -> crate::Result<()> {
    let output = run_git_retrying(
        worktree_path,
        &["commit", "--amend", "--no-edit"],
        "commit-amend",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git commit --amend --no-edit", &output));
    }
    Ok(())
}

/// Stage one repository-relative path (`git add`).
pub fn stage_path(
    worktree_path: &Utf8Path,
    path: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying(worktree_path, &["add", "--", path], "add", warnings)?;
    if !output.success {
        return Err(git_error("git add", &output));
    }
    Ok(())
}

/// Mechanically resolve one conflicted path a project's own
/// `[[merge.generated]]` declaration marks as generated (P463 item C): never
/// sent to the merger, resolved here instead. During a rebase, `--ours` is
/// the branch being rebased ONTO (i.e. `main`) — content does not matter,
/// since the rebuild step that runs once reconciliation completes
/// unconditionally overwrites and re-stages every declared path regardless
/// of which side this checkout picked (see `merge.rs`'s rebuild step; do not
/// "optimize" this side choice away).
pub fn resolve_generated_conflict(
    worktree_path: &Utf8Path,
    path: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying(
        worktree_path,
        &["checkout", "--ours", "--", path],
        "generated-conflict-checkout",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git checkout --ours", &output));
    }
    stage_path(worktree_path, path, warnings)
}

/// Abort an in-progress rebase in `worktree_path`, restoring the branch to
/// its pre-rebase state.
pub fn abort_rebase(worktree_path: &Utf8Path, warnings: &mut RetryWarnings) -> crate::Result<()> {
    let output = run_git_retrying(
        worktree_path,
        &["rebase", "--abort"],
        "rebase-abort",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git rebase --abort", &output));
    }
    Ok(())
}

/// Outcome of [`abort_rebase_if_in_progress`]: whether a rebase was found and
/// successfully aborted, or none was in progress. Callers must not report an
/// intact-branch park unless this resolves `Ok`; an `Err` means recovery
/// itself is unconfirmed and must be surfaced as its own distinct outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortRecovery {
    /// No rebase was in progress; nothing to restore.
    NotInProgress,
    /// A rebase was in progress and `git rebase --abort` succeeded.
    Aborted,
}

/// Only issues `git rebase --abort` when a rebase is actually still in
/// progress, so a park recorded after the rebase already completed (cleanly
/// or via a completed `--continue`) does not fire a pointless abort against a
/// finished rebase. Unlike a best-effort call, detection and abort failures
/// are propagated rather than swallowed: a caller that cannot confirm the
/// worktree was actually restored must not claim it is intact.
pub fn abort_rebase_if_in_progress(
    worktree_path: &Utf8Path,
    warnings: &mut RetryWarnings,
) -> crate::Result<AbortRecovery> {
    if rebase_in_progress(worktree_path)? {
        abort_rebase(worktree_path, warnings)?;
        Ok(AbortRecovery::Aborted)
    } else {
        Ok(AbortRecovery::NotInProgress)
    }
}

/// The commit currently being replayed by an in-progress rebase in
/// `worktree_path` (`git rev-parse REBASE_HEAD`), or `None` when no rebase is
/// in progress. Used to detect genuine reconciliation progress (the replay
/// advanced to a new commit) independent of which paths are conflicted, since
/// a later commit can legitimately conflict on the same or a superset of
/// paths the prior commit did.
pub fn rebase_head(worktree_path: &Utf8Path) -> crate::Result<Option<String>> {
    if !rebase_in_progress(worktree_path)? {
        return Ok(None);
    }
    let output = run_git(worktree_path, &["rev-parse", "REBASE_HEAD"])?;
    if !output.success {
        return Ok(None);
    }
    let head = output.stdout.trim();
    if head.is_empty() {
        Ok(None)
    } else {
        Ok(Some(head.to_string()))
    }
}

/// Fast-forward `main` to `branch` in `repo_root` (the invoking checkout, not
/// the worktree). Never merges, forces, or falls back to a merge commit.
pub fn fast_forward_merge(
    repo_root: &Utf8Path,
    branch: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying(
        repo_root,
        &["merge", "--ff-only", branch],
        "merge-ff-only",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git merge --ff-only", &output));
    }
    Ok(())
}

/// Remove a registered worktree after its branch has landed.
pub fn remove_worktree(
    repo_root: &Utf8Path,
    worktree_path: &Utf8Path,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying(
        repo_root,
        &["worktree", "remove", worktree_path.as_str()],
        "worktree-remove",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git worktree remove", &output));
    }
    Ok(())
}

/// Delete a merged branch. Uses `-d` (not `-D`): Git itself refuses this if
/// the branch is not actually merged, which is a useful last-ditch guard.
pub fn delete_branch(
    repo_root: &Utf8Path,
    branch: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying(
        repo_root,
        &["branch", "-d", branch],
        "branch-delete",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git branch -d", &output));
    }
    Ok(())
}

/// Why a planned seed-harvest file could not be resolved automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedConflictReason {
    /// `main` and the worktree both changed this seeded path since the seed
    /// baseline, and the same region was edited differently on both sides (or
    /// the content could not be merged at all — e.g. binary bytes).
    Overlap,
    /// `main` and the worktree both changed this seeded path since the seed
    /// baseline, but the seed-time ancestor bytes needed to attempt a
    /// three-way merge are unavailable — a legacy worktree prepared before
    /// private seed baselines existed. Never guessed: this fails closed
    /// exactly like an [`Overlap`](Self::Overlap) conflict.
    AncestorUnavailable,
    /// The re-read seed-time ancestor bytes no longer match the digest
    /// recorded in this worktree's [`SeedSnapshot`] at seed time — a
    /// concurrent edit or tamper between snapshot-time and harvest-time read
    /// desynced the private baseline mirror from what was recorded. Fails
    /// closed exactly like [`AncestorUnavailable`](Self::AncestorUnavailable):
    /// a real three-way merge must never be attempted against ancestor bytes
    /// that cannot be trusted.
    AncestorDigestMismatch,
}

/// One repository-relative seeded path that could not be harvested
/// automatically, with the merged-output line range (1-based, inclusive)
/// bracketing every conflict marker `git merge-file` produced — `(0, 0)` when
/// no ancestor was available to even attempt a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedConflict {
    pub relative: String,
    pub start_line: usize,
    pub end_line: usize,
    pub reason: SeedConflictReason,
}

/// One seeded file planned to land in `repo_root`, with its final byte
/// content already resolved (a plain copy of the worktree's bytes, or the
/// clean three-way merge result), the `main`-side digest observed at
/// planning time (so [`apply_harvest_plan`] can detect a race against a
/// concurrent change to `main` before writing), and the worktree-side file's
/// permission mode — carried through so the harvested file lands with the
/// same executable bit or restrictive permissions it had in the worktree,
/// rather than the destination process's default umask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSeedCopy {
    pub relative: String,
    pub bytes: Vec<u8>,
    pub expected_main_digest: Option<Digest>,
    pub mode: u32,
}

/// Outcome of [`plan_harvest_seeds`]: `copies` always carries every file this
/// plan resolves cleanly, WHETHER OR NOT `conflicts` is also non-empty — a
/// conflict on some seeded paths never discards the plan for every other
/// changed seeded path. Applying this plan (via [`apply_harvest_plan`]) while
/// `conflicts` is non-empty is still forbidden — that invariant is enforced
/// by the caller (`ctx traits merge`), which must resolve or adjudicate every
/// conflict (restoring an empty `conflicts` list, `copies` unchanged or
/// extended) before ever calling `apply_harvest_plan`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SeedHarvestPlan {
    pub copies: Vec<PlannedSeedCopy>,
    pub conflicts: Vec<SeedConflict>,
}

/// Plan (without writing anything to `repo_root`) the seed harvest for
/// changed or newly created regular files under the declared seed roots in
/// `worktree_path`, using `snapshots` as the seed-time byte-digest baseline
/// `B` and, when both sides changed, the private per-worktree seed baseline
/// (see [`seed_baseline_root`]) as the three-way merge ancestor. For each
/// current worktree regular file with digest `W` and invocation-repository
/// digest `M` (`None` if absent): `W == B` is unchanged (nothing planned);
/// otherwise `M == B` plans a plain copy of the worktree's bytes; otherwise
/// `W == M` means a prior harvest retry already landed it (nothing planned);
/// any other case attempts a real three-way merge (`git merge-file`) using
/// the seed-time ancestor bytes, `M` as "ours", and `W` as "theirs" — a clean
/// merge is planned with the merged bytes, a textual or unmergeable conflict
/// (or an unavailable ancestor, for a legacy worktree with digest-only
/// snapshots) is recorded as a [`SeedConflict`] instead. Every changed seeded
/// file is resolved independently and always contributes to the returned
/// plan — a conflict on one path never discards another path's clean copy,
/// so a `--deep` adjudication that resolves the conflicts (P463 item A) can
/// merge its result into a plan that still carries every other already-clean
/// copy, rather than having to re-derive them. Worktree-side deletions are
/// not propagated: this only plans changed/new files, matching P368's scope.
pub fn plan_harvest_seeds(
    repo_root: &Utf8Path,
    worktree_path: &Utf8Path,
    snapshots: &[SeedSnapshot],
    default_branch: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<SeedHarvestPlan> {
    let mut baseline: BTreeMap<String, Digest> = BTreeMap::new();
    let mut roots: Vec<Utf8PathBuf> = Vec::new();
    for snapshot in snapshots {
        roots.push(validate_seed_path(&snapshot.root)?);
        for file in &snapshot.files {
            baseline.insert(file.path.clone(), file.digest.clone());
        }
    }
    roots.sort();
    roots.dedup();

    let mut worktree_files: BTreeMap<String, Utf8PathBuf> = BTreeMap::new();
    for root in &roots {
        let worktree_root = worktree_path.join(root);
        if let Some(parent) = worktree_root.parent() {
            crate::path_safety::ensure_no_symlink_ancestors(parent, "worktree seed root ancestor")?;
        }
        collect_regular_files(&worktree_root, root, &mut worktree_files)?;
    }

    let baseline_root = seed_baseline_root(worktree_path, warnings)?;
    // Distinct subdirectories for the mirrored-ancestor and merge-scratch
    // namespaces (see the constants' docs): a seeded repository-relative path,
    // however it is spelled, can only ever land under `ancestors/`, never
    // under `scratch/`, so it can never alias an internal scratch control
    // file.
    let ancestor_root = baseline_root.join(SEED_BASELINE_ANCESTOR_SUBDIR);
    let scratch_root = baseline_root.join(SEED_BASELINE_SCRATCH_SUBDIR);

    let mut copies: Vec<PlannedSeedCopy> = Vec::new();
    let mut conflicts: Vec<SeedConflict> = Vec::new();
    for (relative, worktree_file) in &worktree_files {
        let worktree_bytes =
            std::fs::read(worktree_file.as_std_path()).map_err(|source_error| {
                crate::environment::Error::Filesystem {
                    path: worktree_file.to_string(),
                    source: source_error,
                }
            })?;
        let worktree_mode = std::fs::metadata(worktree_file.as_std_path())
            .map_err(|source_error| crate::environment::Error::Filesystem {
                path: worktree_file.to_string(),
                source: source_error,
            })?
            .permissions()
            .mode();
        let worktree_digest = Digest::from_bytes(&worktree_bytes);
        let baseline_digest = baseline.get(relative);
        if baseline_digest == Some(&worktree_digest) {
            continue;
        }
        let repo_file = repo_root.join(relative);
        let main_digest = digest_of_existing_regular_file(&repo_file)?;
        if main_digest.as_ref() == baseline_digest {
            copies.push(PlannedSeedCopy {
                relative: relative.clone(),
                bytes: worktree_bytes,
                expected_main_digest: main_digest,
                mode: worktree_mode,
            });
            continue;
        }
        if main_digest.as_ref() == Some(&worktree_digest) {
            // Already landed by a prior harvest retry; nothing to plan.
            continue;
        }
        // Genuine both-sides-changed case: attempt a real three-way merge
        // rather than an unconditional conflict.
        let ancestor_bytes = if let Some(expected_baseline_digest) = baseline_digest {
            match read_baseline_bytes(&ancestor_root, relative)? {
                Some(bytes) => {
                    // Fail closed if the baseline mirror re-read right before
                    // this merge no longer matches the digest recorded at
                    // seed-snapshot time: a concurrent edit or tamper between
                    // then and now must never be silently trusted as the
                    // three-way merge ancestor.
                    if &Digest::from_bytes(&bytes) != expected_baseline_digest {
                        conflicts.push(SeedConflict {
                            relative: relative.clone(),
                            start_line: 0,
                            end_line: 0,
                            reason: SeedConflictReason::AncestorDigestMismatch,
                        });
                        continue;
                    }
                    bytes
                }
                None => {
                    conflicts.push(SeedConflict {
                        relative: relative.clone(),
                        start_line: 0,
                        end_line: 0,
                        reason: SeedConflictReason::AncestorUnavailable,
                    });
                    continue;
                }
            }
        } else {
            // Never seeded (no baseline digest at all): the seed-time
            // ancestor is legitimately empty, not merely unrecorded.
            Vec::new()
        };
        let main_bytes = match std::fs::read(repo_file.as_std_path()) {
            Ok(bytes) => bytes,
            Err(source_error) if source_error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source_error) => {
                return Err(crate::environment::Error::Filesystem {
                    path: repo_file.to_string(),
                    source: source_error,
                }
                .into());
            }
        };
        match merge_file_three_way(
            &scratch_root,
            relative,
            &main_bytes,
            &ancestor_bytes,
            &worktree_bytes,
            default_branch,
            warnings,
        )? {
            ThreeWayMerge::Clean(bytes) => copies.push(PlannedSeedCopy {
                relative: relative.clone(),
                bytes,
                expected_main_digest: main_digest,
                mode: worktree_mode,
            }),
            ThreeWayMerge::Conflict {
                start_line,
                end_line,
            } => conflicts.push(SeedConflict {
                relative: relative.clone(),
                start_line,
                end_line,
                reason: SeedConflictReason::Overlap,
            }),
        }
    }
    conflicts.sort_by(|left, right| left.relative.cmp(&right.relative));
    copies.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(SeedHarvestPlan { copies, conflicts })
}

/// The seed-time ancestor, current `main`, and current worktree bytes for one
/// [`SeedConflictReason::Overlap`] conflict (P463 item A) — re-read
/// independently of [`plan_harvest_seeds`], which discards these bytes once
/// it has classified a conflict, since only a `--deep` adjudication actually
/// needs them. `main` and `worktree` are `Vec::new()` for a side that is
/// legitimately absent (deleted or never created), never conflated with
/// "unreadable".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedConflictInputs {
    pub ancestor: Vec<u8>,
    pub main: Vec<u8>,
    pub worktree: Vec<u8>,
}

/// Re-read the three-way adjudication inputs for one seeded
/// [`SeedConflictReason::Overlap`] conflict at `relative`. Only valid for an
/// `Overlap` conflict: `plan_harvest_seeds` only reaches that classification
/// after successfully reading and digest-verifying the seed-time ancestor
/// bytes, so this re-read is expected to succeed for the same conflict
/// (barring a fresh race, which fails closed exactly like
/// `plan_harvest_seeds` itself). A path with no recorded seed-time digest at
/// all (never seeded) legitimately has empty ancestor bytes, mirroring
/// `plan_harvest_seeds`'s own "never seeded" branch.
pub fn read_seed_conflict_inputs(
    repo_root: &Utf8Path,
    worktree_path: &Utf8Path,
    snapshots: &[SeedSnapshot],
    relative: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<SeedConflictInputs> {
    let baseline_digest = snapshots
        .iter()
        .flat_map(|snapshot| &snapshot.files)
        .find(|file| file.path == relative)
        .map(|file| &file.digest);
    let ancestor = match baseline_digest {
        Some(expected_digest) => {
            let baseline_root = seed_baseline_root(worktree_path, warnings)?;
            let ancestor_root = baseline_root.join(SEED_BASELINE_ANCESTOR_SUBDIR);
            let bytes = read_baseline_bytes(&ancestor_root, relative)?.ok_or_else(|| {
                crate::environment::Error::Filesystem {
                    path: relative.to_string(),
                    source: std::io::Error::other(
                        "seed-time ancestor bytes unavailable for adjudication (race against plan_harvest_seeds)",
                    ),
                }
            })?;
            if &Digest::from_bytes(&bytes) != expected_digest {
                return Err(crate::environment::Error::Filesystem {
                    path: relative.to_string(),
                    source: std::io::Error::other(
                        "seed-time ancestor bytes no longer match the recorded digest (race against plan_harvest_seeds)",
                    ),
                }
                .into());
            }
            bytes
        }
        None => Vec::new(),
    };
    let main_file = repo_root.join(relative);
    let main = match std::fs::read(main_file.as_std_path()) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: main_file.to_string(),
                source,
            }
            .into());
        }
    };
    let worktree_file = worktree_path.join(relative);
    let worktree = match std::fs::read(worktree_file.as_std_path()) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: worktree_file.to_string(),
                source,
            }
            .into());
        }
    };
    Ok(SeedConflictInputs {
        ancestor,
        main,
        worktree,
    })
}

/// Apply a conflict-free [`SeedHarvestPlan`] to `repo_root`, atomically. Each
/// destination's current digest is rechecked against the digest observed at
/// planning time immediately before it is replaced: a mismatch means `main`
/// changed again after the plan was built (e.g. a concurrent ignored-file
/// edit) and is a genuine post-fast-forward cleanup failure for the caller to
/// report as such, never a pre-landing park (the fast-forward this plan is
/// applied after has already happened by the time this runs).
pub fn apply_harvest_plan(repo_root: &Utf8Path, plan: &SeedHarvestPlan) -> crate::Result<()> {
    for copy in &plan.copies {
        let dest = repo_root.join(&copy.relative);
        let current_digest = digest_of_existing_regular_file(&dest)?;
        if current_digest != copy.expected_main_digest {
            return Err(crate::environment::Error::Filesystem {
                path: dest.to_string(),
                source: std::io::Error::other(format!(
                    "the repository's content at {} changed since the seed harvest plan was built",
                    copy.relative
                )),
            }
            .into());
        }
        atomic_replace_regular_file(&dest, &copy.bytes, copy.mode)?;
    }
    Ok(())
}

/// Outcome of [`merge_file_three_way`].
enum ThreeWayMerge {
    Clean(Vec<u8>),
    Conflict { start_line: usize, end_line: usize },
}

/// Run `git merge-file` on staged scratch files to merge `ancestor -> theirs`
/// changes into `ours`, returning the merged bytes on a clean merge or the
/// conflict-marker line range on a textual conflict. Staged files (rather
/// than `--stdout` through the existing capped process-output capture) are
/// used deliberately: the merged result can be large, and a truncated capture
/// here would risk publishing an incomplete file. Binary or otherwise
/// unmergeable input surfaces as a non-zero, non-conflict-marked exit and is
/// treated as a whole-file conflict — never as license to prefer one side.
fn merge_file_three_way(
    scratch_root: &Utf8Path,
    relative: &str,
    ours: &[u8],
    base: &[u8],
    theirs: &[u8],
    default_branch: &str,
    warnings: &mut RetryWarnings,
) -> crate::Result<ThreeWayMerge> {
    let stem = scratch_root.join(relative);
    if let Some(parent) = stem.parent() {
        crate::path_safety::create_dir_all_no_symlinks(parent, "seed harvest scratch directory")?;
    }
    let ours_path = Utf8PathBuf::from(format!("{stem}.ours"));
    let base_path = Utf8PathBuf::from(format!("{stem}.base"));
    let theirs_path = Utf8PathBuf::from(format!("{stem}.theirs"));
    for (path, bytes) in [
        (&ours_path, ours),
        (&base_path, base),
        (&theirs_path, theirs),
    ] {
        write_regular_file_no_follow(path, bytes, "seed harvest merge-scratch file")?;
    }
    let output = run_git_retrying(
        scratch_root,
        &[
            "merge-file",
            "-L",
            default_branch,
            "-L",
            "seed-base",
            "-L",
            "worktree",
            ours_path.as_str(),
            base_path.as_str(),
            theirs_path.as_str(),
        ],
        "merge-file",
        warnings,
    )?;
    let merged = std::fs::read(ours_path.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: ours_path.to_string(),
            source: source_error,
        }
    })?;
    for path in [&ours_path, &base_path, &theirs_path] {
        let _ = std::fs::remove_file(path.as_std_path());
    }
    if output.success {
        return Ok(ThreeWayMerge::Clean(merged));
    }
    match conflict_marker_line_range(&merged) {
        Some((start_line, end_line)) => Ok(ThreeWayMerge::Conflict {
            start_line,
            end_line,
        }),
        // No parseable conflict markers at all (e.g. `git merge-file`
        // rejected binary input outright): the whole file is unresolved.
        None => Ok(ThreeWayMerge::Conflict {
            start_line: 0,
            end_line: 0,
        }),
    }
}

/// The 1-based, inclusive line range from the first `<<<<<<<` conflict marker
/// to the last `>>>>>>>` conflict marker in `merged`, or `None` if `merged` is
/// not valid UTF-8 or contains no conflict markers at all.
fn conflict_marker_line_range(merged: &[u8]) -> Option<(usize, usize)> {
    let text = std::str::from_utf8(merged).ok()?;
    let mut start_line: Option<usize> = None;
    let mut end_line: Option<usize> = None;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.starts_with("<<<<<<<") && start_line.is_none() {
            start_line = Some(line_number);
        }
        if line.starts_with(">>>>>>>") {
            end_line = Some(line_number);
        }
    }
    match (start_line, end_line) {
        (Some(start), Some(end)) => Some((start, end)),
        _ => None,
    }
}

/// Read one seeded file's private seed-time ancestor bytes back from
/// `ancestor_root` (the [`SEED_BASELINE_ANCESTOR_SUBDIR`] subdirectory of the
/// per-worktree baseline directory [`seed_baseline_root`] resolves), or
/// `None` if this worktree has no physical baseline for `relative` (a legacy
/// worktree prepared before private baselines existed). Callers must verify
/// the returned bytes' digest against the recorded [`SeedSnapshot`] digest
/// before trusting them as a three-way merge ancestor — this function only
/// reads, it does not verify.
fn read_baseline_bytes(ancestor_root: &Utf8Path, relative: &str) -> crate::Result<Option<Vec<u8>>> {
    let path = ancestor_root.join(relative);
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "seed baseline ancestor")?;
    }
    if !crate::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "seed baseline file")? {
        return Ok(None);
    }
    let bytes = std::fs::read(path.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: source_error,
        }
    })?;
    Ok(Some(bytes))
}

/// Write `bytes` to `path`, rejecting a pre-existing symlink or any other
/// non-regular-file leaf at `path` (never following it) before writing. The
/// single no-follow staging-write primitive shared by every regular-file
/// write in this module — seed copies, three-way-merge scratch files, and the
/// atomic harvest replacement temp file — so a symlink planted at any of
/// those destination leaves is rejected instead of silently followed.
fn write_regular_file_no_follow(path: &Utf8Path, bytes: &[u8], label: &str) -> crate::Result<()> {
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, label)?;
    std::fs::write(path.as_std_path(), bytes).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: source_error,
        }
    })?;
    Ok(())
}

/// Atomically replace (or create) a regular file's bytes at `dest`, setting
/// `mode` on the staged content before publishing it: write to a sibling
/// temporary file first (through [`write_regular_file_no_follow`], so a
/// pre-existing symlink at the temp leaf is rejected rather than followed),
/// set its permission mode, recheck `dest` itself immediately before renaming
/// (a symlink could have been planted there between the first check and now),
/// then rename it into place — so a reader never observes a partially written
/// destination and the harvested file keeps its worktree-side mode (e.g. an
/// executable bit or a restrictive 0600) instead of the process umask's
/// default. Every existing ancestor and the `dest` leaf itself are walked
/// no-follow first, matching every other destination write in this module.
fn atomic_replace_regular_file(dest: &Utf8Path, bytes: &[u8], mode: u32) -> crate::Result<()> {
    if let Some(parent) = dest.parent() {
        crate::path_safety::create_dir_all_no_symlinks(parent, "harvest destination directory")?;
    }
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(dest, "harvest destination")?;
    let tmp = Utf8PathBuf::from(format!("{dest}.ctx-harvest-tmp"));
    write_regular_file_no_follow(&tmp, bytes, "harvest replacement temp file")?;
    std::fs::set_permissions(tmp.as_std_path(), std::fs::Permissions::from_mode(mode)).map_err(
        |source_error| crate::environment::Error::Filesystem {
            path: tmp.to_string(),
            source: source_error,
        },
    )?;
    // Recheck the destination leaf immediately before publication: a symlink
    // planted at `dest` between the check above and now must still be
    // rejected rather than followed by the rename.
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(dest, "harvest destination")?;
    std::fs::rename(tmp.as_std_path(), dest.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: dest.to_string(),
            source: source_error,
        }
    })?;
    Ok(())
}

/// The digest of `path` if it currently exists as a regular file in the
/// invocation repository, or `None` if it does not exist. Every existing
/// ancestor is walked no-follow via [`crate::path_safety`] before the leaf is
/// inspected, so a symlinked destination directory cannot be silently
/// traversed; the leaf itself is rejected if it is a symlink or any other
/// non-regular-file state, matching the seed side's restriction.
fn digest_of_existing_regular_file(path: &Utf8Path) -> crate::Result<Option<Digest>> {
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "harvest destination ancestor")?;
    }
    if !crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, "harvest destination")? {
        return Ok(None);
    }
    let bytes = std::fs::read(path.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: source_error,
        }
    })?;
    Ok(Some(Digest::from_bytes(&bytes)))
}

/// Recursively collect every current regular file rooted at `path` into
/// `files`, keyed by its path relative to the worktree root. `path` may
/// itself be a regular file (a single-file seed root, collected directly) or
/// a directory (recursed); any other existing kind — including a symlink at
/// any level, checked with `symlink_metadata` before it is trusted — is
/// rejected. A missing `path` (a seed root never seeded, or removed in the
/// worktree) contributes nothing rather than erroring: harvest does not
/// propagate deletions. This is the one traversal both the directory-root and
/// file-root cases share; there is no separate file-root implementation.
fn collect_regular_files(
    path: &Utf8Path,
    relative: &Utf8Path,
    files: &mut BTreeMap<String, Utf8PathBuf>,
) -> crate::Result<()> {
    let metadata = match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(metadata) => metadata,
        Err(source_error) if source_error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(source_error) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: source_error,
            }
            .into());
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(config_error(
            "worktree.seed",
            format!("worktree seed path {path} is a symlink, which is not supported"),
        ));
    }
    if metadata.is_file() {
        files.insert(relative.to_string(), path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(config_error(
            "worktree.seed",
            format!("worktree seed path {path} is neither a regular file nor a directory"),
        ));
    }
    let entries = std::fs::read_dir(path.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: source_error,
        }
    })?;
    for entry in entries {
        let entry = entry.map_err(|source_error| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: source_error,
        })?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::other("directory entry name is not valid UTF-8"),
            })?
            .to_string();
        let child = path.join(&name);
        let child_relative = relative.join(&name);
        collect_regular_files(&child, &child_relative, files)?;
    }
    Ok(())
}

/// The path `id`'s worktree will occupy, derived from the id alone without
/// touching or requiring the worktree itself.
///
/// P564: the `[worktree.env]` `{worktree}` token has to resolve BEFORE
/// [`prepare_worktree`] runs, because the resolved overlay is what setup
/// commands are handed. The location is a pure function of the id (see
/// [`resolve_worktree_location`]), so asking for it early is sound — this is
/// the same path the preparation below goes on to create.
pub fn worktree_path_for(id: &str) -> crate::Result<Utf8PathBuf> {
    let (_, path, _) = resolve_worktree_location(id)?;
    Ok(path)
}

fn resolve_worktree_location(id: &str) -> crate::Result<(Utf8PathBuf, Utf8PathBuf, String)> {
    validate_worktree_id(id)?;
    let repo_root = crate::repository::discover_repo_root()?;
    let path = repo_root.join(crate::layout::worktree_root()).join(id);
    let branch = format!("ctx/run/{id}");
    Ok((repo_root, path, branch))
}

/// Explicit, resolved wall-clock/capture budget for every `[worktree] setup`
/// command in one create-worktree call — bundled so the budget threads as a
/// single argument rather than growing `create_new_worktree`'s parameter list
/// unboundedly.
#[derive(Debug, Clone, Copy)]
struct SetupBudget {
    timeout_ms: u64,
    capture_limit: usize,
}

/// Declared `[worktree] setup` commands, their environment overlay, and the
/// resolved budget every one of them runs under — bundled for the same
/// reason as [`SetupBudget`]. `progress`, when given, is called with a short
/// phase-boundary message ("creating worktree", "seeding", "setup:
/// <command>", "setup done (<Ns>)") as [`create_new_worktree`] runs — see
/// [`resume_or_prepare_worktree`].
struct SetupPlan<'a> {
    setup: &'a [Vec<String>],
    setup_env: &'a BTreeMap<String, String>,
    budget: SetupBudget,
    progress: Option<&'a dyn Fn(&str)>,
}

fn create_new_worktree(
    repo_root: &Utf8Path,
    path: &Utf8Path,
    branch: &str,
    contents: WorktreeContents<'_>,
    setup_plan: SetupPlan<'_>,
    worktree_add_timeout_ms: u64,
    warnings: &mut RetryWarnings,
) -> crate::Result<PreparedWorktree> {
    if path.exists() {
        return Err(config_error(
            "worktree.id",
            format!("worktree path {path} already exists but is not a registered git worktree"),
        ));
    }
    if let Some(progress) = setup_plan.progress {
        progress("creating worktree");
    }
    // Ensured against the invocation repository itself, before the fresh
    // `git worktree add` below — never inside the worktree being created —
    // so a project's first `--worktree` run leaves the canonical nested
    // ignore file committable in the checkout that will actually commit it
    // (P446).
    crate::gitignore::ensure_nested_gitignore(repo_root)?;
    create_worktree(repo_root, path, branch, worktree_add_timeout_ms, warnings)?;
    // Private baseline directory under this worktree's own Git administrative
    // directory (never visible as working-tree content, never serialized into
    // the run ledger), resolved before any seed is copied so every seed's
    // ancestor mirror can be written from the exact same read that populates
    // the worktree below — never a second, later, independent read of
    // `repo_root` that a concurrent edit could race against.
    let baseline_root = seed_baseline_root(path, warnings)?;
    let ancestor_root = baseline_root.join(SEED_BASELINE_ANCESTOR_SUBDIR);
    let seeds = contents.seeds;
    if !seeds.is_empty() {
        if let Some(progress) = setup_plan.progress {
            progress("seeding");
        }
    }
    let mut seed_snapshots = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let mut files = copy_seed(repo_root, seed, path, &ancestor_root)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        seed_snapshots.push(SeedSnapshot {
            root: seed.clone(),
            files,
        });
    }
    // Warmed BEFORE setup commands, so a setup step that itself builds
    // (installing a toolchain, priming a cache) already sees the warm cache
    // rather than racing it. Warnings never block the run — see
    // [`warm_worktree_paths`].
    for warning in warm_worktree_paths(repo_root, path, contents.warm, setup_plan.progress)? {
        warnings.push(warning);
    }
    let scope_id = path.file_name().unwrap_or("worktree");
    for command in setup_plan.setup {
        if let Some(progress) = setup_plan.progress {
            progress(&format!("setup: {}", command.join(" ")));
        }
        let started = std::time::Instant::now();
        run_setup_command(
            path,
            command,
            setup_plan.setup_env,
            setup_plan.budget.timeout_ms,
            setup_plan.budget.capture_limit,
            scope_id,
        )?;
        if let Some(progress) = setup_plan.progress {
            progress(&format!(
                "setup done ({:.0}s)",
                started.elapsed().as_secs_f64()
            ));
        }
    }
    Ok(PreparedWorktree {
        path: path.to_path_buf(),
        branch: branch.to_string(),
        resumed: false,
        seed_snapshots,
        retry_warnings: warnings.as_slice().to_vec(),
    })
}

/// Resolve the private, per-worktree directory this worktree's seed-time
/// ancestor bytes are mirrored into. `"ctx-seed-baseline"` is not one of
/// Git's fixed per-worktree paths (like `rebase-merge`), but an arbitrary
/// custom path passed to `git rev-parse --git-path` from inside a linked
/// worktree still resolves under that worktree's own
/// `.git/worktrees/<id>/` directory rather than the shared repository `.git`
/// — verified against Git's own path-resolution behavior — so this is
/// private to the worktree and never collides with a concurrent sibling
/// worktree's baseline.
fn seed_baseline_root(
    worktree_path: &Utf8Path,
    warnings: &mut RetryWarnings,
) -> crate::Result<Utf8PathBuf> {
    let output = run_git_retrying(
        worktree_path,
        &["rev-parse", "--git-path", "ctx-seed-baseline"],
        "rev-parse-git-path",
        warnings,
    )?;
    if !output.success {
        return Err(git_error(
            "git rev-parse --git-path ctx-seed-baseline",
            &output,
        ));
    }
    let raw = output.stdout.trim();
    let resolved = if Utf8Path::new(raw).is_absolute() {
        Utf8PathBuf::from(raw)
    } else {
        worktree_path.join(raw)
    };
    Ok(resolved)
}

/// Run one declared `[worktree] setup` command as literal argv (never a
/// shell string) inside the freshly created `worktree_path`, after all seeds
/// have been copied. Any spawn failure, timeout, or non-zero exit is mapped
/// to a typed worktree-preparation process error naming the command and
/// worktree path, so a broken setup step blocks the run instead of leaving
/// frames to execute in an incomplete checkout.
/// Default per-command wall-clock ceiling (milliseconds) for a declared
/// `[worktree] setup` command, used when `[worktree] setup-seconds` is
/// absent. Inherited by every `run_setup_command` call.
pub const DEFAULT_SETUP_TIMEOUT_MS: u64 = 120_000;
/// Default stdout/stderr capture ceiling (bytes) for a declared `[worktree]
/// setup` command, used when `[worktree] setup-capture-bytes` is absent.
/// Generous (4 MiB) so a failing installer's (e.g. `pnpm install`) own
/// diagnostic output survives rather than the former invisible 16 KiB
/// default.
pub const DEFAULT_SETUP_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

fn run_setup_command(
    worktree_path: &Utf8Path,
    argv: &[String],
    setup_env: &BTreeMap<String, String>,
    timeout_ms: u64,
    capture_limit: usize,
    scope_id: &str,
) -> crate::Result<()> {
    let command_display = argv.join(" ");
    let output = crate::command::run_with_env(
        crate::command::RunRequest {
            argv,
            cwd: None,
            exec_dir: Some(worktree_path),
            success_exit_code: &[0],
            timeout_ms: Some(timeout_ms),
            capture_limit,
            tick_observer: None,
        },
        setup_env,
    )
    .map_err(|error| {
        crate::Error::from(crate::environment::Error::Process {
            command: Some(command_display.clone()),
            path: Some(worktree_path.to_string()),
            exit_status: None,
            timed_out: false,
            message: format!(
                "worktree setup command {command_display:?} failed to run in {worktree_path}: {error}"
            ),
        })
    })?;
    // A truncated-but-successful setup command is not itself refused here:
    // setup commands run only for side effects (installing deps etc.) and
    // feed no slot or digest, unlike the capture-consuming sites elsewhere in
    // this phase. Truncation still names itself in the failure message below
    // when the command also failed, since `persist_failure_capture` treats
    // either condition as failure-worthy for diagnosability.
    if !output.success {
        let capture_path =
            crate::command::persist_failure_capture("worktree-setup", scope_id, &output);
        return Err(crate::environment::Error::Process {
            command: Some(command_display.clone()),
            path: Some(worktree_path.to_string()),
            exit_status: output.exit_code,
            timed_out: output.timed_out,
            message: format!(
                "stdout-truncated={} stderr-truncated={} capture={}",
                output.stdout_truncated,
                output.stderr_truncated,
                capture_path.map_or_else(|| "unavailable".to_string(), |path| path.to_string()),
            ),
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod setup_budget_tests {
    use super::*;

    /// A setup command that fails with more stdout than its declared capture
    /// ceiling must name the truncation in its error AND persist a capture
    /// file containing the tail — the honest-failure-output half of P489's
    /// site C, exercised directly against `run_setup_command` rather than
    /// only inferred from a green `cargo test`. Unlike the slot-feeding
    /// capture sites (A, B), a setup command's output feeds no slot or
    /// digest, so the error need not itself restate the numeric cap — the
    /// persisted capture file (asserted below to exist and hold the tail) is
    /// the diagnosability contract here, not the error text.
    #[test]
    fn failing_setup_command_names_cap_and_persists_capture() {
        let worktree_path = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-setup-budget-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        std::fs::create_dir_all(worktree_path.as_std_path()).expect("create scratch dir");
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "head -c 400 /dev/zero; exit 1".to_string(),
        ];
        let error = run_setup_command(
            &worktree_path,
            &argv,
            &BTreeMap::new(),
            DEFAULT_SETUP_TIMEOUT_MS,
            100,
            "setup-budget-test",
        )
        .expect_err("a failing setup command must return Err");
        let message = error.to_string();
        assert!(
            message.contains("stdout-truncated=true"),
            "expected the error to name the truncation: {message}"
        );
        let capture_marker = "capture=";
        let start = message
            .find(capture_marker)
            .unwrap_or_else(|| panic!("expected a capture= field in: {message}"))
            + capture_marker.len();
        let capture_path = &message[start..];
        assert_ne!(
            capture_path, "unavailable",
            "expected a persisted capture path, not an unavailable marker: {message}"
        );
        let contents =
            std::fs::read_to_string(capture_path).expect("persisted capture file must exist");
        assert!(
            !contents.is_empty(),
            "persisted capture file must hold the command's output"
        );
        let _ = std::fs::remove_file(capture_path);
        let _ = std::fs::remove_dir_all(worktree_path.as_std_path());
    }
}

fn validate_worktree_id(id: &str) -> crate::Result<()> {
    if id.is_empty()
        || id.len() > 64
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(config_error(
            "worktree.id",
            "worktree id must contain only letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

/// One registration row parsed out of `git worktree list --porcelain`
/// (P462 doctor debris sweep): the registered path and the branch checked
/// out there, when the porcelain output names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRegistration {
    pub path: Utf8PathBuf,
    pub branch: Option<String>,
}

/// Parse `git worktree list --porcelain` stdout into one row per registered
/// worktree, shared by [`existing_worktree_branch`] and
/// [`list_worktree_registrations`] so the porcelain format is parsed in
/// exactly one place.
fn parse_worktree_list_porcelain(stdout: &str) -> Vec<WorktreeRegistration> {
    let mut registrations = Vec::new();
    let mut current_path: Option<Utf8PathBuf> = None;
    let mut current_branch: Option<String> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                registrations.push(WorktreeRegistration {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(Utf8PathBuf::from(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(
                rest.trim()
                    .strip_prefix("refs/heads/")
                    .unwrap_or(rest.trim())
                    .to_string(),
            );
        }
    }
    if let Some(path) = current_path.take() {
        registrations.push(WorktreeRegistration {
            path,
            branch: current_branch.take(),
        });
    }
    registrations
}

fn existing_worktree_branch(
    repo_root: &Utf8Path,
    path: &Utf8Path,
    warnings: &mut RetryWarnings,
) -> crate::Result<Option<String>> {
    let output = run_git_retrying(
        repo_root,
        &["worktree", "list", "--porcelain"],
        "worktree-list",
        warnings,
    )?;
    if !output.success {
        return Err(git_error("git worktree list --porcelain", &output));
    }
    Ok(parse_worktree_list_porcelain(&output.stdout)
        .into_iter()
        .find(|registration| registration.path == path)
        .and_then(|registration| registration.branch))
}

/// Every worktree Git currently has registered against this repository
/// (P462 doctor debris sweep) — including one whose directory no longer
/// exists on disk, which is exactly the debris class doctor looks for.
pub fn list_worktree_registrations(
    repo_root: &Utf8Path,
) -> crate::Result<Vec<WorktreeRegistration>> {
    let output = run_git(repo_root, &["worktree", "list", "--porcelain"])?;
    if !output.success {
        return Err(git_error("git worktree list --porcelain", &output));
    }
    Ok(parse_worktree_list_porcelain(&output.stdout))
}

/// Drop Git's own bookkeeping for worktrees whose directory is gone (P462
/// doctor `--apply`). Never touches a worktree whose directory still exists,
/// even if it is otherwise unrelated to ctx.
pub fn prune_worktree_registrations(repo_root: &Utf8Path) -> crate::Result<()> {
    let output = run_git(repo_root, &["worktree", "prune", "--expire", "now"])?;
    if !output.success {
        return Err(git_error("git worktree prune", &output));
    }
    Ok(())
}

/// Every `refs/heads/ctx/run/*` branch (ctx's own run-branch namespace) whose
/// tip is already an ancestor of `default_branch` (P462 doctor debris
/// sweep) — a completed, landed run's branch that a merge's cleanup step
/// failed to remove. Never enumerates or reports on branches outside this
/// namespace, and never a branch checked out in any worktree Git currently
/// has registered — a freshly seeded run branch is trivially an ancestor of
/// `default_branch` (zero commits) but is a live, in-flight run rather than
/// debris, and `git branch -d` refuses (and would abort the caller's sweep
/// on) any branch checked out anywhere, present directory or not.
///
/// `default_branch` is [`resolve_default_branch`]'s best-effort guess and,
/// by that function's own contract, is never validated to exist — treat a
/// `--merged` probe against a branch name Git cannot resolve the same way
/// the prior per-branch `merge-base --is-ancestor` loop treated it: nothing
/// is reported as merged, not a hard error.
pub fn run_branches_merged_into(
    repo_root: &Utf8Path,
    default_branch: &str,
) -> crate::Result<Vec<String>> {
    let checked_out: std::collections::HashSet<String> = list_worktree_registrations(repo_root)?
        .into_iter()
        .filter_map(|registration| registration.branch)
        .collect();
    let output = run_git(
        repo_root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("--merged={default_branch}"),
            "refs/heads/ctx/run/",
        ],
    )?;
    if !output.success {
        return Ok(Vec::new());
    }
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|branch| !checked_out.contains(*branch))
        .map(str::to_string)
        .collect())
}

fn create_worktree(
    repo_root: &Utf8Path,
    path: &Utf8Path,
    branch: &str,
    worktree_add_timeout_ms: u64,
    warnings: &mut RetryWarnings,
) -> crate::Result<()> {
    let output = run_git_retrying_with_timeout(
        repo_root,
        &["worktree", "add", path.as_str(), "-b", branch, "HEAD"],
        "worktree-add",
        warnings,
        worktree_add_timeout_ms,
    )?;
    if !output.success {
        return Err(git_error("git worktree add", &output));
    }
    Ok(())
}

/// Every `git` invocation used to build state a decision depends on runs
/// through here: a generous capture ceiling plus a hard error on truncation,
/// instead of silently parsing a cut-off listing. Folded from a former
/// two-helper split (P463) — `worktree_content_fingerprint` built both
/// `--deep` fingerprints from a 65,536-byte capture of a 73,943-byte `git
/// ls-files --stage -z` listing (2026-07-25): pre and post
/// truncated at slightly different offsets once the merger's `git add`
/// shortened the index records, so the paths straddling the cut
/// ("scripts/golde", `scripts/goldens/drive.json`) flickered between the two
/// maps and parked three substantively-correct deep-merge receipts as
/// phantom uncovered "touched paths". The prior 65,536-byte-capped
/// `run_git`/`run_git_full_listing` split let a future listing call site pick
/// the silently-cutting helper by mistake — one helper closes that off.
fn run_git(repo_root: &Utf8Path, args: &[&str]) -> crate::Result<crate::command::RunOutput> {
    run_git_with_timeout(repo_root, args, crate::git_process::PLUMBING_TIMEOUT_MS)
}

/// Same capture policy as [`run_git`], with an explicit timeout — see
/// [`run_git_retrying_with_timeout`].
fn run_git_with_timeout(
    repo_root: &Utf8Path,
    args: &[&str],
    timeout_ms: u64,
) -> crate::Result<crate::command::RunOutput> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args,
        success_exit_code: &[0],
        timeout_ms,
        capture_limit: 16 * 1024 * 1024,
    })?;
    output.refuse_if_truncated(&format!("git {}", args.join(" ")))?;
    Ok(output)
}

fn git_error(command: &str, output: &crate::command::RunOutput) -> crate::Error {
    crate::git_process::error(command, output)
}

/// Copy one declared gitignored seed (a repository-relative file or
/// directory) from the invocation repository into the prepared worktree,
/// returning the repository-relative path and byte digest of every regular
/// file materialized. Tracked package-relative resources are excluded from
/// this list — the checkout that `git worktree add` performs already
/// materializes them. Every materialized file's ancestor mirror under
/// `ancestor_root` is written from the exact same source read used to
/// populate the worktree (see [`copy_regular_file`]) — never a later,
/// independent second read of `repo_root` that a concurrent edit between the
/// two reads could desync from the digest this function returns.
fn copy_seed(
    repo_root: &Utf8Path,
    seed: &str,
    worktree_path: &Utf8Path,
    ancestor_root: &Utf8Path,
) -> crate::Result<Vec<SeedFileDigest>> {
    let relative = validate_seed_path(seed)?;
    let source = repo_root.join(&relative);
    let dest = worktree_path.join(&relative);
    let metadata = std::fs::symlink_metadata(source.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_error,
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(config_error(
            "worktree.seed",
            format!("declared worktree seed {seed:?} is a symlink, which is not supported"),
        ));
    }
    if metadata.is_dir() {
        let mut files = Vec::new();
        copy_dir_recursive(&source, &dest, &relative, ancestor_root, &mut files)?;
        Ok(files)
    } else if metadata.is_file() {
        let mirror = ancestor_root.join(&relative);
        let digest = copy_regular_file(&source, &dest, Some(&mirror))?;
        Ok(vec![SeedFileDigest {
            path: relative.to_string(),
            digest,
        }])
    } else {
        Err(config_error(
            "worktree.seed",
            format!("declared worktree seed {seed:?} is neither a regular file nor a directory"),
        ))
    }
}

/// Copy a regular file's bytes and permissions from `source` to `dest`,
/// creating `dest`'s parent directory if needed, and return the digest of the
/// bytes materialized at `dest`. Shared by the seed root's own file case and
/// the recursive seed-directory walk, so every caller computes the digest
/// from the same read/write pass rather than a second pass over `dest` after
/// the write. When `mirror_dest` is `Some`, the exact same in-memory bytes
/// (not a second, later, independent read of `source`) are also written
/// there — the private seed-baseline ancestor mirror `ctx traits merge`'s
/// seed harvest later three-way-merges against, so its bytes can never race a
/// concurrent edit to `source` between two separate reads. Every existing
/// ancestor of `source`, `dest`, and `mirror_dest`, and the `dest`/
/// `mirror_dest` leaves themselves, are walked no-follow via
/// [`crate::path_safety`] before any read or write, so a symlinked ancestor
/// or a pre-existing symlink at either destination cannot redirect the copy
/// outside the intended tree; missing ancestors are created (also no-follow)
/// rather than trusted afterward.
fn copy_regular_file(
    source: &Utf8Path,
    dest: &Utf8Path,
    mirror_dest: Option<&Utf8Path>,
) -> crate::Result<Digest> {
    if let Some(parent) = source.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "copy source ancestor")?;
    }
    if let Some(parent) = dest.parent() {
        crate::path_safety::create_dir_all_no_symlinks(parent, "copy destination directory")?;
    }
    let bytes = std::fs::read(source.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_error,
        }
    })?;
    write_regular_file_no_follow(dest, &bytes, "copy destination")?;
    let permissions = std::fs::metadata(source.as_std_path())
        .map_err(|source_error| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_error,
        })?
        .permissions();
    std::fs::set_permissions(dest.as_std_path(), permissions).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: dest.to_string(),
            source: source_error,
        }
    })?;
    if let Some(mirror_dest) = mirror_dest {
        if let Some(parent) = mirror_dest.parent() {
            crate::path_safety::create_dir_all_no_symlinks(
                parent,
                "seed baseline ancestor mirror directory",
            )?;
        }
        write_regular_file_no_follow(mirror_dest, &bytes, "seed baseline ancestor mirror")?;
    }
    Ok(Digest::from_bytes(&bytes))
}

fn validate_seed_path(seed: &str) -> crate::Result<Utf8PathBuf> {
    validate_repo_relative_path(seed, "worktree.seed", "worktree seed")
}

fn validate_repo_relative_path(value: &str, field: &str, noun: &str) -> crate::Result<Utf8PathBuf> {
    if value.trim().is_empty() {
        return Err(config_error(field, format!("{noun} must not be empty")));
    }
    let path = Utf8Path::new(value);
    if path.is_absolute() {
        return Err(config_error(
            field,
            format!("{noun} {value:?} must be repository-relative"),
        ));
    }
    for component in path.components() {
        if !matches!(component, Utf8Component::Normal(_)) {
            return Err(config_error(
                field,
                format!("{noun} {value:?} must not contain '..' or other traversal segments"),
            ));
        }
    }
    Ok(path.to_path_buf())
}

/// Wall-clock ceiling for one `[worktree] warm` clone. A copy-on-write clone
/// is metadata-only, so this bounds a pathological entry (a filesystem that
/// accepted the flag but is copying bytes anyway, an enormous file count)
/// rather than a normal one — a 100k-file tree clones in seconds.
const WARM_CLONE_TIMEOUT_MS: u64 = 300_000;

/// Capture ceiling for a failed clone's diagnostic output. Small on purpose:
/// `cp`'s failure output is one line, and this never feeds a slot.
const WARM_CLONE_CAPTURE_BYTES: usize = 64 * 1024;

/// P564: copy-on-write clone each declared `[worktree] warm` directory from
/// the invocation checkout into the freshly created worktree, at the same
/// relative path, so a per-worktree build cache starts warm rather than cold.
///
/// **Never falls back to a byte copy.** The clone flag is `-c` (macOS
/// `clonefile`) or `--reflink=always` (Linux), both of which FAIL rather than
/// degrade on a filesystem without copy-on-write support. That is deliberate:
/// the directories this exists for are build caches measured in tens of
/// gigabytes, so a silent byte-copy fallback would turn a fast worktree
/// creation into a disk-filling one. A refused clone means a cold build,
/// which is slow but correct.
///
/// Every failure is a WARNING, never an error: warming is an optimization,
/// and a run that cannot warm must still start. A partial clone is removed
/// before returning, so cargo never inspects a half-populated cache and
/// concludes it is complete.
///
/// Returns the warnings raised, in declaration order.
pub fn warm_worktree_paths(
    repo_root: &Utf8Path,
    worktree_path: &Utf8Path,
    warm: &[String],
    progress: Option<&dyn Fn(&str)>,
) -> crate::Result<Vec<String>> {
    let mut warnings = Vec::new();
    for entry in warm {
        let relative = validate_repo_relative_path(entry, "worktree.warm", "worktree warm path")?;
        let source = repo_root.join(&relative);
        let metadata = match std::fs::symlink_metadata(source.as_std_path()) {
            Ok(metadata) => metadata,
            // Nothing to warm from is the ordinary first-run case, not a
            // problem: the worktree simply builds cold.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warnings.push(format!(
                    "worktree warm path {entry:?} could not be read at {source} ({error}); the worktree starts cold"
                ));
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            warnings.push(format!(
                "worktree warm path {entry:?} is a symlink, which is not cloned; the worktree starts cold"
            ));
            continue;
        }
        if !metadata.is_dir() {
            warnings.push(format!(
                "worktree warm path {entry:?} is not a directory; only directories are cloned, so the worktree starts cold"
            ));
            continue;
        }
        let dest = worktree_path.join(&relative);
        if dest.exists() {
            warnings.push(format!(
                "worktree warm path {entry:?} already exists in the new worktree at {dest}; leaving it untouched"
            ));
            continue;
        }
        if let Some(progress) = progress {
            progress(&format!("warming {entry}"));
        }
        if let Some(failure) = clone_directory(&source, &dest)? {
            remove_partial_clone(&dest, &mut warnings);
            warnings.push(format!(
                "worktree warm path {entry:?} was not cloned ({failure}); the worktree starts cold"
            ));
        }
    }
    Ok(warnings)
}

/// Copy-on-write clone `source` to `dest`. `Ok(None)` on success; `Ok(Some(
/// reason))` when the clone was refused or failed, which every caller treats
/// as a warning rather than an error.
fn clone_directory(source: &Utf8Path, dest: &Utf8Path) -> crate::Result<Option<String>> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source_error| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source: source_error,
            }
        })?;
    }
    // `-c` clones on APFS; `--reflink=always` clones on btrfs/XFS. Both fail
    // loudly on a filesystem that cannot, which is the behavior this wants.
    let clone_flag = if cfg!(target_os = "macos") {
        "-c"
    } else {
        "--reflink=always"
    };
    let argv = vec![
        "cp".to_string(),
        "-R".to_string(),
        clone_flag.to_string(),
        source.to_string(),
        dest.to_string(),
    ];
    let output = match crate::command::run(crate::command::RunRequest {
        argv: &argv,
        cwd: None,
        exec_dir: None,
        success_exit_code: &[0],
        timeout_ms: Some(WARM_CLONE_TIMEOUT_MS),
        capture_limit: WARM_CLONE_CAPTURE_BYTES,
        tick_observer: None,
    }) {
        Ok(output) => output,
        Err(error) => return Ok(Some(format!("clone command failed to run: {error}"))),
    };
    if output.success {
        return Ok(None);
    }
    let detail = output.stderr.trim();
    let detail = if detail.is_empty() {
        "no diagnostic output".to_string()
    } else {
        detail.lines().next().unwrap_or(detail).to_string()
    };
    Ok(Some(format!(
        "copy-on-write clone is unsupported here or failed: {detail}"
    )))
}

/// Remove whatever a failed clone left behind. A partial build cache is worse
/// than none: cargo would read it as complete.
fn remove_partial_clone(dest: &Utf8Path, warnings: &mut Vec<String>) {
    if !dest.exists() {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(dest.as_std_path()) {
        warnings.push(format!(
            "a partially cloned worktree warm path could not be removed at {dest} ({error}); remove it by hand before trusting a build from this worktree"
        ));
    }
}

fn copy_dir_recursive(
    source: &Utf8Path,
    dest: &Utf8Path,
    relative: &Utf8Path,
    ancestor_root: &Utf8Path,
    files: &mut Vec<SeedFileDigest>,
) -> crate::Result<()> {
    crate::path_safety::create_dir_all_no_symlinks(dest, "seed destination directory")?;
    let entries = std::fs::read_dir(source.as_std_path()).map_err(|source_error| {
        crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_error,
        }
    })?;
    for entry in entries {
        let entry = entry.map_err(|source_error| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: source_error,
        })?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: source.to_string(),
                source: std::io::Error::other("directory entry name is not valid UTF-8"),
            })?
            .to_string();
        let child_source = source.join(&name);
        let child_dest = dest.join(&name);
        let child_relative = relative.join(&name);
        // `DirEntry::metadata` does not follow symlinks, matching
        // `symlink_metadata` semantics, so this is the same symlink check used
        // for the seed root above.
        let metadata =
            entry
                .metadata()
                .map_err(|source_error| crate::environment::Error::Filesystem {
                    path: child_source.to_string(),
                    source: source_error,
                })?;
        if metadata.file_type().is_symlink() {
            return Err(config_error(
                "worktree.seed",
                format!("worktree seed path {child_source} is a symlink, which is not supported"),
            ));
        }
        if metadata.is_dir() {
            copy_dir_recursive(
                &child_source,
                &child_dest,
                &child_relative,
                ancestor_root,
                files,
            )?;
        } else if metadata.is_file() {
            let mirror = ancestor_root.join(&child_relative);
            let digest = copy_regular_file(&child_source, &child_dest, Some(&mirror))?;
            files.push(SeedFileDigest {
                path: child_relative.to_string(),
                digest,
            });
        } else {
            return Err(config_error(
                "worktree.seed",
                format!(
                    "worktree seed path {child_source} is neither a regular file nor a directory"
                ),
            ));
        }
    }
    Ok(())
}

fn config_error(field_path: impl Into<String>, message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: field_path.into(),
            message: message.into(),
        }
        .into(),
    )
}

#[cfg(test)]
mod deep_merge_boundary_tests {
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
            "ctx-worktree-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if repo.exists() {
            std::fs::remove_dir_all(repo.as_std_path()).expect("clear stale scratch repo");
        }
        std::fs::create_dir_all(repo.as_std_path()).expect("create scratch repo dir");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.name", "ctx-worktree-test"]);
        git(
            &repo,
            &["config", "user.email", "worktree-test@example.invalid"],
        );
        repo
    }

    /// A stage the unmerged index confirms present, but whose object cannot
    /// actually be read (a real Git/object-store failure), must surface as a
    /// typed `Err` from [`conflict_stage_blobs`] — never silently downgraded
    /// to [`ConflictBlobs`]'s `None` "absent side" representation, which a
    /// P420 `--deep` merger prompt would otherwise misreport as an ordinary
    /// add/delete conflict. Constructed with `git update-index --index-info`
    /// so the index unambiguously reports the stage present while the SHA it
    /// names was never actually written to the object database — a
    /// deterministic, provider-free stand-in for real object-store
    /// corruption.
    #[test]
    fn conflict_stage_blobs_errors_rather_than_reporting_absent_for_a_present_but_unreadable_stage()
    {
        let repo = fresh_repo("conflict-blob-error");
        std::fs::write(repo.join("conflicted.txt").as_std_path(), b"base\n")
            .expect("write base file");
        git(&repo, &["add", "conflicted.txt"]);
        git(&repo, &["commit", "--quiet", "-m", "base"]);

        let missing_sha = "f".repeat(40);
        let index_info = format!("100644 {missing_sha} 2\tconflicted.txt\n");
        let mut child = Command::new("git")
            .args(["update-index", "--index-info"])
            .current_dir(repo.as_std_path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn git update-index");
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin piped")
                .write_all(index_info.as_bytes())
                .expect("write index-info");
        }
        let status = child.wait().expect("git update-index runs");
        assert!(status.success(), "git update-index --index-info failed");

        // The index now reports stage 2 present (`git ls-files --stage
        // --unmerged` lists it) but its object was never written, so `git
        // show :2:conflicted.txt` must fail.
        let result = conflict_stage_blobs(&repo, "conflicted.txt");
        let error = result.expect_err(
            "a present-but-unreadable stage must be a typed error, never an absent side",
        );
        let message = error.to_string();
        assert!(
            !message.contains("<absent>"),
            "error must never masquerade as an absent stage: {message}"
        );

        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }

    /// The ordinary add/delete-conflict shape — a stage the index genuinely
    /// does not record — must still resolve to `None`, not an error, so this
    /// regression's fix does not also break the legitimate absent case.
    #[test]
    fn conflict_stage_blobs_reports_none_for_a_genuinely_absent_stage() {
        let repo = fresh_repo("conflict-blob-absent");
        std::fs::write(repo.join("added.txt").as_std_path(), b"branch content\n")
            .expect("write file");
        git(&repo, &["add", "added.txt"]);
        git(&repo, &["commit", "--quiet", "-m", "base"]);

        // Only stage 3 (branch side) present; stages 1 and 2 genuinely absent
        // — an add/add-from-one-side shape.
        let sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD:added.txt"])
                .current_dir(repo.as_std_path())
                .output()
                .expect("rev-parse runs")
                .stdout,
        )
        .expect("utf8 sha")
        .trim()
        .to_string();
        let index_info = format!("100644 {sha} 3\tadded.txt\n");
        let mut child = Command::new("git")
            .args(["update-index", "--index-info"])
            .current_dir(repo.as_std_path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn git update-index");
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin piped")
                .write_all(index_info.as_bytes())
                .expect("write index-info");
        }
        let status = child.wait().expect("git update-index runs");
        assert!(status.success(), "git update-index --index-info failed");

        let blobs = conflict_stage_blobs(&repo, "added.txt").expect("present stage 3 reads fine");
        assert!(blobs.base.is_none());
        assert!(blobs.main_side.is_none());
        assert!(blobs.branch_side.is_some());

        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }

    /// [`worktree_content_fingerprint`] must see a materialized file under a
    /// declared, `.gitignore`d seed root — a `git ls-files --others
    /// --exclude-standard` scan alone would miss it entirely, which is
    /// exactly the recurrence this fixture pins closed.
    #[test]
    fn fingerprint_covers_materialized_declared_seed_root_paths() {
        let repo = fresh_repo("fingerprint-seed-root");
        std::fs::write(repo.join("README.md").as_std_path(), b"root\n").expect("write readme");
        std::fs::write(repo.join(".gitignore").as_std_path(), b"/seed-dir/\n")
            .expect("write gitignore");
        git(&repo, &["add", "README.md", ".gitignore"]);
        git(&repo, &["commit", "--quiet", "-m", "base"]);

        std::fs::create_dir_all(repo.join("seed-dir").as_std_path()).expect("mkdir seed-dir");
        std::fs::write(
            repo.join("seed-dir/seeded.txt").as_std_path(),
            b"seed content\n",
        )
        .expect("write seeded file");

        let seed_roots = vec!["seed-dir".to_string()];
        let before = worktree_content_fingerprint(&repo, &seed_roots).expect("fingerprint");
        assert!(
            before.contains_key("seed-dir/seeded.txt"),
            "a materialized declared-seed path must be part of the fingerprint universe at all, \
             even though it is `.gitignore`d and untracked"
        );

        std::fs::write(
            repo.join("seed-dir/seeded.txt").as_std_path(),
            b"seed content EDITED\n",
        )
        .expect("edit seeded file");
        let after = worktree_content_fingerprint(&repo, &seed_roots).expect("fingerprint");
        let touched = touched_paths_delta(&before, &after);
        assert!(
            touched.contains(&"seed-dir/seeded.txt".to_string()),
            "an edit to a materialized, ignored declared-seed path must be visible in the \
             touched-paths delta: {touched:?}"
        );

        std::fs::remove_dir_all(repo.as_std_path()).ok();
    }
}
