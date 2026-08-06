//! Invocation Git repository discovery and read-only gitignore inspection.
//!
//! Shared by `worktree.rs` (dedicated-worktree root discovery) and the typed
//! `root = "repo"` resource resolver: both need the repository root of the
//! checkout `ctx` was invoked from, discovered via `git rev-parse
//! --show-toplevel` run at the process working directory — never inferred
//! from a trait package location, so external/dependency traits resolve
//! against the invoking repository, not their own package path.

use camino::{Utf8Path, Utf8PathBuf};

/// Discover the Git repository root of the invocation working directory.
///
/// Runs `git rev-parse --show-toplevel` with literal argv (no shell) at the
/// process cwd. Fails if the invocation directory is not inside a Git
/// worktree.
pub fn discover_repo_root() -> crate::Result<Utf8PathBuf> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: None,
        cwd: Some("project-root"),
        args: &["rev-parse", "--show-toplevel"],
        success_exit_code: &[0],
        timeout_ms: 10_000,
        capture_limit: 4096,
    })?;
    if !output.success {
        return Err(classify_repo_discovery_error(
            "git rev-parse --show-toplevel",
            &output,
        ));
    }
    Ok(Utf8PathBuf::from(output.stdout.trim().to_string()))
}

/// The exact stderr Git prints for "no containing repository" — the only
/// exit-128 condition [`is_not_a_repository`] recognises. Every other
/// exit-128 cause (unsafe/dubious ownership, corrupted `.git`, malformed
/// config) prints a different message and is a genuine repository-discovery
/// failure, not an absence of a repository.
const NOT_A_GIT_REPOSITORY_MARKER: &str = "not a git repository";

/// The single authority for "did Git say there is no repository here":
/// exit 128, no timeout, and the standard not-a-repository stderr. Shared by
/// [`discover_repo_root_at`] (which has a fallback and treats a match as
/// `Ok(None)`) and [`classify_repo_discovery_error`] (which has none and
/// attaches a remedy instead) — the two consumers legitimately branch
/// differently on the answer, but must ask the same question.
fn is_not_a_repository(output: &crate::command::RunOutput) -> bool {
    !output.timed_out
        && output.exit_code == Some(128)
        && output
            .stderr
            .to_ascii_lowercase()
            .contains(NOT_A_GIT_REPOSITORY_MARKER)
}

/// The single site that attaches the not-a-repository remedy to a failed
/// discovery: unlike `discover_repo_root_at` (which has a fallback and can
/// treat it as `Ok(None)`), `discover_repo_root` has no fallback — its
/// caller genuinely requires an invocation inside a Git checkout — so the
/// remedy is written here, once, rather than left for every caller to
/// rediscover from a bare `git` failure.
fn classify_repo_discovery_error(
    command: &str,
    output: &crate::command::RunOutput,
) -> crate::Error {
    if is_not_a_repository(output) {
        return crate::environment::Error::Git {
            command: Some(command.to_string()),
            path: None,
            exit_status: output.exit_code,
            timed_out: false,
            message: format!(
                "{}; run inside a git repository, or pass an explicit path",
                output.stderr.trim()
            ),
        }
        .into();
    }
    crate::git_process::error(command, output)
}

/// Discover the Git repository root containing `dir`, running `git
/// rev-parse --show-toplevel` with `dir` as the real execution directory
/// rather than the process cwd — used by callers whose stable base must
/// track a specific on-disk location (e.g. a CDK source file, which may sit
/// in a different Git worktree/root than wherever `ctx` was invoked from,
/// such as a nested workspace package). Returns `Ok(None)`, not an error,
/// only for the genuine "`dir` is not inside a Git worktree" case: Git's
/// exit code 128 with no timeout and its standard "not a git repository"
/// stderr, so callers can fall back to non-Git root inference for fresh
/// non-Git projects. Exit 128 alone is not sufficient — Git also uses it for
/// operational failures against an existing repository (unsafe/dubious
/// ownership, corrupted `.git`, malformed config), which this function
/// distinguishes by stderr text and returns as `Err` so a caller cannot
/// mistake "Git is broken here" for "this isn't a Git checkout" and fall
/// back to a too-narrow package-root inference that would emit a
/// machine-specific absolute path. Any other failure — a timed-out or
/// unexpected-exit-code invocation, or the process itself failing to spawn
/// (missing `git`, permission denied) — is likewise a genuine operational
/// error and is returned as `Err`.
pub fn discover_repo_root_at(dir: &Utf8Path) -> crate::Result<Option<Utf8PathBuf>> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(dir),
        cwd: None,
        args: &["rev-parse", "--show-toplevel"],
        success_exit_code: &[0],
        timeout_ms: 10_000,
        capture_limit: 4096,
    })?;
    if output.success {
        return Ok(Some(Utf8PathBuf::from(output.stdout.trim().to_string())));
    }
    if is_not_a_repository(&output) {
        return Ok(None);
    }
    Err(crate::git_process::error(
        "git rev-parse --show-toplevel",
        &output,
    ))
}

/// Discover the *main* checkout root for a linked worktree, i.e. the
/// repository `git worktree add` created `worktree` from — never `worktree`
/// itself. `rev-parse --show-toplevel` run inside a linked worktree returns
/// the worktree's own root, which is the wrong anchor for anything that must
/// name the main checkout (P478 write confinement: a confinement generated
/// against the worktree root would protect nothing). Instead runs `git -C
/// <worktree> rev-parse --path-format=absolute --git-common-dir`, which
/// always resolves to the shared `.git` directory regardless of which linked
/// worktree it is invoked from, and returns that path's parent.
pub fn discover_main_repo_root(worktree: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(worktree),
        cwd: None,
        args: &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        success_exit_code: &[0],
        timeout_ms: 10_000,
        capture_limit: 4096,
    })?;
    if !output.success {
        return Err(crate::git_process::error(
            "git rev-parse --git-common-dir",
            &output,
        ));
    }
    let common_dir = Utf8PathBuf::from(output.stdout.trim().to_string());
    common_dir
        .parent()
        .map(Utf8Path::to_path_buf)
        .ok_or_else(|| {
            crate::git_process::message_error(format!(
                "git-common-dir {common_dir} has no parent directory"
            ))
        })
}

/// Whether a repository-relative path matches a `.gitignore` rule, checked
/// read-only against the invocation repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreStatus {
    Ignored,
    NotIgnored,
}

/// Check whether `repo_relative_path` is ignored under `repo_root`, using
/// `git check-ignore --no-index -q --` (literal argv, no shell) so only
/// actual ignore-rule matches are reported, independent of tracked-file
/// status. Exit 0 means ignored, exit 1 means not ignored; any other exit
/// code is a Git error.
pub fn check_ignored(
    repo_root: &Utf8Path,
    repo_relative_path: &str,
) -> crate::Result<IgnoreStatus> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args: &["check-ignore", "--no-index", "-q", "--", repo_relative_path],
        success_exit_code: &[0, 1],
        timeout_ms: 10_000,
        capture_limit: 4096,
    })?;
    match output.exit_code {
        Some(0) => Ok(IgnoreStatus::Ignored),
        Some(1) => Ok(IgnoreStatus::NotIgnored),
        _ => Err(crate::git_process::error("git check-ignore", &output)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RunOutput;

    fn not_a_repo_output() -> RunOutput {
        RunOutput {
            exit_code: Some(128),
            stdout: String::new(),
            stderr: "fatal: not a git repository (or any of the parent directories): .git"
                .to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
            timeout_reason: None,
            timeout_kind: None,
            success: false,
            capture_limit: 4096,
        }
    }

    #[test]
    fn not_a_repo_error_carries_the_remedy_sentence() {
        let error =
            classify_repo_discovery_error("git rev-parse --show-toplevel", &not_a_repo_output());
        let text = error.to_string();
        assert!(
            text.contains("run inside a git repository, or pass an explicit path"),
            "{text}"
        );
        assert!(text.contains("not a git repository"), "{text}");
    }

    #[test]
    fn genuine_git_failure_keeps_the_bare_git_error_with_no_remedy() {
        let output = RunOutput {
            exit_code: Some(128),
            stdout: String::new(),
            stderr: "fatal: detected dubious ownership in repository at '/repo'".to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
            timeout_reason: None,
            timeout_kind: None,
            success: false,
            capture_limit: 4096,
        };
        let error = classify_repo_discovery_error("git rev-parse --show-toplevel", &output);
        let text = error.to_string();
        assert!(
            !text.contains("run inside a git repository"),
            "genuine operational failure must not claim the not-a-repo remedy: {text}"
        );
        assert!(text.contains("dubious ownership"), "{text}");
    }
}
