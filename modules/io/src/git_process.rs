//! Shared literal-argv Git process edge.
//!
//! Every module that shells out to `git` (dependency resolution, worktree
//! management, repository discovery) builds argv and maps failures through
//! this one implementation instead of copying its own `run_git`/`git_error`
//! pair, so timeout, capture, and error-mapping policy cannot drift between
//! call sites.

use camino::Utf8Path;

use crate::command::{RunBytesOutput, RunOutput, RunRequest};

/// Timeout for plumbing Git calls (`rev-parse`, `diff`, `ls-files`, `log`,
/// `show`) — bounded, interactive-speed operations that never legitimately
/// run long. Unchanged from the value every such call site already used
/// before this constant existed.
pub const PLUMBING_TIMEOUT_MS: u64 = 30_000;

/// Timeout for Git calls whose duration depends on remote network or working
/// tree size rather than being a fixed local plumbing cost: `clone`, `fetch`,
/// `rebase`, `rebase --continue`, `worktree add`. Applying the plumbing
/// timeout to these parks a merely-slow operation (a large clone, a rebase
/// replaying many commits) as a spurious failure.
pub const LONG_TIMEOUT_MS: u64 = 900_000;

/// One `git` invocation. Every field a caller might reasonably vary is
/// explicit here rather than defaulted, so each call site's actual policy
/// (timeout, capture limit, accepted exit codes) stays visible at the call.
pub struct Request<'a> {
    /// Real execution directory; mutually exclusive with `cwd` (see
    /// [`crate::command::RunRequest`]).
    pub exec_dir: Option<&'a Utf8Path>,
    /// Literal `"project-root"` convention for invocation-anchored callers
    /// that have no worktree-scoped `exec_dir`.
    pub cwd: Option<&'a str>,
    pub args: &'a [&'a str],
    pub success_exit_code: &'a [i32],
    pub timeout_ms: u64,
    pub capture_limit: usize,
}

/// Run `git` with the given arguments and execution context.
pub fn run(request: Request<'_>) -> crate::Result<RunOutput> {
    let argv: Vec<String> = std::iter::once("git".to_string())
        .chain(request.args.iter().map(ToString::to_string))
        .collect();
    crate::command::run(RunRequest {
        argv: &argv,
        cwd: request.cwd,
        exec_dir: request.exec_dir,
        success_exit_code: request.success_exit_code,
        timeout_ms: Some(request.timeout_ms),
        idle_timeout_ms: None,
        capture_limit: request.capture_limit,
        tick_observer: None,
    })
}

/// Byte-exact-stdout counterpart to [`run`], for a caller (P420 `--deep`
/// conflict-stage blob reads) whose stdout must reach its destination
/// byte-exact — see [`crate::command::run_bytes_with_env`].
pub fn run_bytes(request: Request<'_>) -> crate::Result<RunBytesOutput> {
    let argv: Vec<String> = std::iter::once("git".to_string())
        .chain(request.args.iter().map(ToString::to_string))
        .collect();
    crate::command::run_bytes_with_env(
        RunRequest {
            argv: &argv,
            cwd: request.cwd,
            exec_dir: request.exec_dir,
            success_exit_code: request.success_exit_code,
            timeout_ms: Some(request.timeout_ms),
            idle_timeout_ms: None,
            capture_limit: request.capture_limit,
            tick_observer: None,
        },
        &std::collections::BTreeMap::new(),
    )
}

/// Map a failed [`RunOutput`] to a typed Git error.
pub fn error(command: &str, output: &RunOutput) -> crate::Error {
    crate::environment::Error::Git {
        command: Some(command.to_string()),
        path: None,
        exit_status: output.exit_code,
        timed_out: output.timed_out,
        message: output.stderr.trim().to_string(),
    }
    .into()
}

/// Byte-output counterpart to [`error`], for a failed [`RunBytesOutput`].
pub fn error_bytes(command: &str, output: &RunBytesOutput) -> crate::Error {
    crate::environment::Error::Git {
        command: Some(command.to_string()),
        path: None,
        exit_status: output.exit_code,
        timed_out: output.timed_out,
        message: output.stderr.trim().to_string(),
    }
    .into()
}

/// A Git error not tied to a specific process invocation (e.g. a revision
/// that fails a syntactic check before any `git` command runs).
pub fn message_error(message: impl Into<String>) -> crate::Error {
    crate::environment::Error::Git {
        command: None,
        path: None,
        exit_status: None,
        timed_out: false,
        message: message.into(),
    }
    .into()
}
