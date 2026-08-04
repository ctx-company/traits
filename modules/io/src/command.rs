//! Controlled command execution at the IO boundary.
//!
//! Core only validates command declarations and produces argv plans. This module
//! is the process-execution edge used by CLI/MCP adapters after explicit
//! permission policy allows the current command frame.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

/// Inherited by every caller that passes `timeout_ms: None` on [`RunRequest`]
/// (both `apply_default_inputs`'s and command-step's requests set an explicit
/// value and never rely on this; a caller that wants a longer bound must set
/// `timeout_ms` explicitly rather than relying on this fallback).
/// Default silence window for a command step: how long it may produce NO
/// output before the runtime treats it as hung (0058). Liveness, not
/// duration, is what generalises across ecosystems — a command still
/// printing is working however long it takes, and one that has gone quiet
/// is stuck. Ten minutes because compilers legitimately go silent for
/// minutes on a single large unit or a long link (cargo prints
/// `Compiling <crate>` and then nothing; a slow `tsc` or bundler behaves the
/// same), so a shorter window would kill healthy work.
pub const DEFAULT_COMMAND_IDLE_MS: u64 = 600_000;

/// Absolute backstop for a command step (0058), applied however chatty the
/// command is. Idle detection alone would let a command that prints forever
/// — a retry loop, an accidental watch mode — run without end. Deliberately
/// loose enough never to fire in normal use.
pub const DEFAULT_COMMAND_WALL_MS: u64 = 14_400_000;
/// Inherited by every caller that passes `capture_limit: 0` on [`RunRequest`]
/// — `0` means "use this default", not "capture nothing". Every call site in
/// this tree that could plausibly capture non-trivial output sets an explicit
/// `capture_limit` instead of relying on this fallback; the sentinel exists
/// for callers (interactive/status probes) that only ever inspect success.
const DEFAULT_CAPTURE_LIMIT: usize = 16_384;

/// Run a literal command with interactive stdin. JSON-mode callers reserve
/// stdout for their own document, while npm's prompts and diagnostics remain
/// live on stderr and stdin remains inherited for authentication.
pub fn run_interactive(
    argv: &[String],
    cwd: &Utf8Path,
    json_mode: bool,
) -> std::io::Result<ExitStatus> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "empty argv"))?;
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).stdin(Stdio::inherit());
    command.stdout(if json_mode {
        Stdio::null()
    } else {
        Stdio::inherit()
    });
    command.stderr(Stdio::inherit());
    command.status()
}

/// Shared empty overlay so invocation-anchored (non-worktree) callers of
/// [`run`] apply no environment changes without allocating a fresh map each
/// call. `BTreeMap::new` is a const fn, so this is a true static.
static EMPTY_ENV_OVERLAY: BTreeMap<String, String> = BTreeMap::new();

/// Apply a deterministic environment overlay to a `std::process::Command` via
/// repeated `Command::env`. This is the ONE place any spawn site layers a
/// worktree `[worktree].env` overlay onto a child process; every worktree
/// spawn (setup commands, default-input/procedure command steps, harness
/// probes, cold/persistent harnesses, narrators, merger dispatch) routes its
/// overlay through here so the application semantics cannot drift between
/// call sites. It never mutates the current process environment and never
/// calls `std::env::set_var`. Iterating a `BTreeMap` yields keys in sorted
/// order, so the applied sequence is deterministic. Callers that also strip
/// credentials via `Command::env_remove` MUST apply this overlay first so a
/// removal always wins over a config-provided value.
pub fn apply_env_overlay(command: &mut Command, overlay: &BTreeMap<String, String>) {
    for (key, value) in overlay {
        command.env(key, value);
    }
}

/// Deterministic identity string for an env overlay (sorted `key=value`
/// pairs — `BTreeMap` iteration is already sorted — NUL-joined). Shared by
/// every persistent-process pool-reuse key (drive's per-role harness session
/// key, the narrator warm-pool key) so a warm process spawned under one
/// `[worktree].env` overlay can never be reused for a different one.
pub fn overlay_identity(overlay: &BTreeMap<String, String>) -> String {
    overlay
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

#[derive(Clone)]
pub struct RunRequest<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a str>,
    /// Real execution directory, used by worktree-scoped runs. When set, it
    /// takes precedence over `cwd` and is applied directly via
    /// `Command::current_dir`; `cwd` keeps validating the literal
    /// `"project-root"` convention for every invocation-anchored caller.
    pub exec_dir: Option<&'a Utf8Path>,
    pub success_exit_code: &'a [i32],
    /// Absolute wall-clock ceiling. `None` resolves to
    /// [`DEFAULT_COMMAND_WALL_MS`] — a backstop, not a duration estimate;
    /// [`RunRequest::idle_timeout_ms`] is what actually decides hung-ness.
    pub timeout_ms: Option<u64>,
    /// Silence window (0058): kill once the command has produced no output
    /// for this long. `None` resolves to [`DEFAULT_COMMAND_IDLE_MS`].
    pub idle_timeout_ms: Option<u64>,
    pub capture_limit: usize,
    /// Polled once per wait iteration (same cadence as the internal 10ms
    /// `try_wait` loop) while this command runs. Lets a live `--progress tui`
    /// pane keep detach input responsive across a blocking command frame,
    /// the same as it does for a blocking harness call. `None` for every
    /// non-interactive caller (byte-identical behavior).
    pub tick_observer: Option<crate::harness::TickObserver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    /// Which bound ended the command when `timed_out` (0058). `None` when it
    /// exited on its own. The two are different repository conditions and a
    /// reviewer must not read either as the worker's defect.
    pub timeout_reason: Option<&'static str>,
    pub success: bool,
    /// The effective capture ceiling this run actually applied (after the
    /// `capture_limit: 0` → [`DEFAULT_CAPTURE_LIMIT`] sentinel is resolved),
    /// so [`Self::refuse_if_truncated`] always names the real cap without a
    /// caller having to restate it.
    pub capture_limit: usize,
}

/// Build the one truncation-refusal error every capture-consuming call site
/// shares — same message vocabulary regardless of which output type observed
/// the truncation.
fn truncation_refusal_error(what: &str, exit_status: Option<i32>, limit: usize) -> crate::Error {
    crate::environment::Error::Process {
        command: Some(what.to_string()),
        path: None,
        exit_status,
        timed_out: false,
        message: format!(
            "output exceeded the {limit}-byte capture ceiling — refusing to build \
             state from a truncated capture"
        ),
    }
    .into()
}

impl RunOutput {
    /// Refuse to let a truncated stdout capture feed forward into decision
    /// state. `what` names the thing being captured (e.g. `"git ls-files
    /// --stage -z"`, `"npm pack listing"`) so the error is actionable rather
    /// than a bare byte count. The cap it names is `self.capture_limit`, the
    /// ceiling this specific run actually applied — never a value restated by
    /// the caller, so the two can never drift apart.
    pub fn refuse_if_truncated(&self, what: &str) -> crate::Result<()> {
        if self.stdout_truncated {
            return Err(truncation_refusal_error(
                what,
                self.exit_code,
                self.capture_limit,
            ));
        }
        Ok(())
    }
}

/// Run a command with no environment overlay. Invocation-anchored (non
/// worktree) callers keep byte-identical behavior by routing through here,
/// which applies the shared empty overlay.
pub fn run(request: RunRequest<'_>) -> crate::Result<RunOutput> {
    run_with_env(request, &EMPTY_ENV_OVERLAY)
}

/// Run a command with a worktree `[worktree].env` overlay applied to the child
/// process. Behavior is identical to [`run`] except for the additional
/// environment layered on via [`apply_env_overlay`]; passing an empty overlay
/// is exactly equivalent to [`run`].
pub fn run_with_env(
    request: RunRequest<'_>,
    env_overlay: &BTreeMap<String, String>,
) -> crate::Result<RunOutput> {
    let raw = run_raw(&request, env_overlay)?;
    let (stdout, stdout_truncated) = lossy_utf8(raw.stdout, raw.stdout_truncated);
    let (stderr, stderr_truncated) = lossy_utf8(raw.stderr, raw.stderr_truncated);
    Ok(RunOutput {
        timeout_reason: raw.timeout_kind.map(TimeoutKind::reason),
        exit_code: raw.exit_code,
        stdout,
        stdout_truncated,
        stderr,
        stderr_truncated,
        timed_out: raw.timed_out,
        success: raw.success,
        capture_limit: raw.capture_limit,
    })
}

fn lossy_utf8(bytes: Vec<u8>, truncated: bool) -> (String, bool) {
    (String::from_utf8_lossy(&bytes).to_string(), truncated)
}

/// Byte-exact output from [`run_raw`], the one process-execution engine
/// shared by [`run_with_env`] and [`run_bytes_with_env`]. Both public entry
/// points are thin text/byte conversions over this single spawn, timeout,
/// and concurrent-drain implementation.
struct RawOutput {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stdout_truncated: bool,
    stderr: Vec<u8>,
    stderr_truncated: bool,
    timed_out: bool,
    /// Which bound fired, when `timed_out` (0058). The two are different
    /// repository conditions and must not read alike: silence means hung,
    /// the wall clock means genuinely endless.
    timeout_kind: Option<TimeoutKind>,
    success: bool,
    capture_limit: usize,
}

/// Which bound ended a command step (0058).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// No output for the configured silence window.
    Idle,
    /// Exceeded the absolute wall-clock backstop despite producing output.
    Wall,
}

impl TimeoutKind {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Idle => "command produced no output within its idle window",
            Self::Wall => "command exceeded its absolute wall-clock ceiling",
        }
    }
}

/// Read a pipe to EOF, keeping at most `limit` bytes but continuing to drain
/// everything past that so the writing child never blocks on a full OS pipe
/// buffer waiting for a reader that stopped early. A genuine read failure is
/// returned as a typed `Err` rather than treated as EOF: silently downgrading
/// it would let a real pipe failure masquerade as a short, successful
/// capture, which a byte-exact caller (e.g. a P420 `--deep` merge's
/// conflict-stage blob reads) must never observe.
/// [`read_capped`] plus a liveness stamp: every non-empty read records how
/// far into the run output last arrived, measured against the SAME clock the
/// waiting loop uses, so the loop can tell a working command from a hung one
/// (0058). `None` keeps the plain capture behaviour.
fn read_capped_observed<R: Read>(
    mut reader: R,
    limit: usize,
    last_output: Option<Arc<AtomicU64>>,
    started: Instant,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(stamp) = last_output.as_ref() {
                    stamp.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
                if buf.len() < limit {
                    let remaining = limit - buf.len();
                    let take = remaining.min(n);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(source) => return Err(source),
        }
    }
    Ok((buf, truncated))
}

fn spawn_piped(
    request: &RunRequest<'_>,
    env_overlay: &BTreeMap<String, String>,
) -> crate::Result<Child> {
    if request.argv.is_empty() || request.argv[0].trim().is_empty() {
        return Err(crate::Error::Usage {
            message: "command argv is empty".to_string(),
        });
    }

    let mut command = Command::new(&request.argv[0]);
    command.args(&request.argv[1..]);
    apply_env_overlay(&mut command, env_overlay);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(dir) = request.exec_dir {
        command.current_dir(dir);
    } else if let Some(cwd) = request.cwd {
        if cwd != "project-root" {
            return Err(crate::environment::Error::Filesystem {
                path: cwd.to_string(),
                source: std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "only cwd=project-root is supported",
                ),
            }
            .into());
        }
        command.current_dir(Utf8Path::new("."));
    }
    // Own SESSION, not merely an own process group (2026-08-03).
    //
    // P551 put every command child in its own process group so a live-pane
    // ctrl-c kill reaches it and anything it forks. That still left the
    // child sharing the caller's CONTROLLING TERMINAL, and a `--progress
    // tui` run holds that terminal's foreground group. Any descendant that
    // opened `/dev/tty` directly — bypassing the null stdin and piped
    // stdout/stderr below — was therefore a background job touching the
    // terminal, and the kernel suspended it with SIGTTIN/SIGTTOU. Measured
    // 2026-08-03: three concurrent gates sat in state `T` for half an hour,
    // doing nothing, until SIGCONT released them and they finished in
    // seconds; before that the same stall read as a gate timeout and parked
    // three runs at round 1.
    //
    // `setsid` creates a new session with NO controlling terminal, so a
    // `/dev/tty` open fails cleanly instead of stopping the process. The
    // session leader is also a process-group leader whose pgid equals its
    // pid, which is exactly what `run_kill`'s `kill(-pgid, …)` needs, so
    // P551's kill path is unchanged.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and only alters this child's
        // own session/process-group identity between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    command.spawn().map_err(|e| {
        crate::environment::Error::Filesystem {
            path: request.argv.join(" "),
            source: e,
        }
        .into()
    })
}

/// The one spawn/wait/timeout/capture engine every command edge routes
/// through. Stdout and stderr are drained concurrently on dedicated threads
/// while the main thread polls `try_wait`, so a child that writes more than
/// the OS pipe buffer holds cannot deadlock waiting for a reader that only
/// starts after the process exits.
fn run_raw(
    request: &RunRequest<'_>,
    env_overlay: &BTreeMap<String, String>,
) -> crate::Result<RawOutput> {
    let mut child = spawn_piped(request, env_overlay)?;
    let pgid = child.id() as i32;
    crate::run_kill::register(pgid);
    let limit = if request.capture_limit == 0 {
        DEFAULT_CAPTURE_LIMIT
    } else {
        request.capture_limit
    };

    // One clock shared by both reader threads and the wait loop below, so a
    // "last output arrived at" stamp is directly comparable with elapsed
    // run time (0058).
    let started = Instant::now();
    let last_output = Arc::new(AtomicU64::new(0));

    let stdout_pipe: ChildStdout = child.stdout.take().expect("stdout piped at spawn");
    let stderr_pipe: ChildStderr = child.stderr.take().expect("stderr piped at spawn");
    let stdout_stamp = Arc::clone(&last_output);
    let stderr_stamp = Arc::clone(&last_output);
    let stdout_handle = thread::spawn(move || {
        read_capped_observed(stdout_pipe, limit, Some(stdout_stamp), started)
    });
    let stderr_handle = thread::spawn(move || {
        read_capped_observed(stderr_pipe, limit, Some(stderr_stamp), started)
    });

    // 0058: liveness first, duration only as a backstop. A command still
    // printing is working however long it takes — that is what a cold build
    // of a large workspace looks like — while one that has gone quiet is
    // stuck. A fixed wall-clock ceiling cannot be right for a TypeScript
    // project and a Rust workspace at once, and ours had to be retuned three
    // times before this landed.
    let wall = Duration::from_millis(request.timeout_ms.unwrap_or(DEFAULT_COMMAND_WALL_MS));
    let idle_limit =
        Duration::from_millis(request.idle_timeout_ms.unwrap_or(DEFAULT_COMMAND_IDLE_MS));
    let mut timed_out = false;
    let mut timeout_kind: Option<TimeoutKind> = None;
    let mut exited_status: Option<ExitStatus> = None;
    loop {
        let elapsed = started.elapsed();
        let quiet_for =
            elapsed.saturating_sub(Duration::from_millis(last_output.load(Ordering::Relaxed)));
        match child
            .try_wait()
            .map_err(|e| crate::environment::Error::Filesystem {
                path: request.argv.join(" "),
                source: e,
            })? {
            Some(status) => {
                exited_status = Some(status);
                break;
            }
            None if quiet_for >= idle_limit => {
                timed_out = true;
                timeout_kind = Some(TimeoutKind::Idle);
                let _ = child.kill();
                break;
            }
            None if elapsed >= wall => {
                timed_out = true;
                timeout_kind = Some(TimeoutKind::Wall);
                let _ = child.kill();
                break;
            }
            None => {
                if let Some(observer) = request.tick_observer.as_ref() {
                    observer();
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    let status = match exited_status {
        Some(status) => status,
        None => child
            .wait()
            .map_err(|e| crate::environment::Error::Filesystem {
                path: request.argv.join(" "),
                source: e,
            })?,
    };
    crate::run_kill::clear(pgid);

    let (stdout, stdout_truncated) = stdout_handle
        .join()
        .expect("stdout reader thread panicked")
        .map_err(|source| crate::environment::Error::Filesystem {
            path: format!("{} (stdout)", request.argv.join(" ")),
            source,
        })?;
    let (stderr, stderr_truncated) = stderr_handle
        .join()
        .expect("stderr reader thread panicked")
        .map_err(|source| crate::environment::Error::Filesystem {
            path: format!("{} (stderr)", request.argv.join(" ")),
            source,
        })?;

    let exit_code = status.code();
    let success_codes = if request.success_exit_code.is_empty() {
        &[0][..]
    } else {
        request.success_exit_code
    };
    let success = !timed_out && exit_code.is_some_and(|code| success_codes.contains(&code));
    Ok(RawOutput {
        exit_code,
        stdout,
        stdout_truncated,
        stderr,
        stderr_truncated,
        timed_out,
        timeout_kind,
        success,
        capture_limit: limit,
    })
}

/// Same shape as [`RunOutput`], but `stdout` is the raw captured bytes rather
/// than a lossily-decoded `String` — for a caller (a P420 `--deep` merge's
/// conflict-stage blob reads) whose stdout must reach its destination
/// byte-exact, since `RunOutput::stdout`'s UTF-8 lossy capture would silently
/// corrupt binary content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunBytesOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr: String,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub success: bool,
    /// See [`RunOutput::capture_limit`].
    pub capture_limit: usize,
}

impl RunBytesOutput {
    /// Byte-exact counterpart to [`RunOutput::refuse_if_truncated`] — same
    /// shared message vocabulary and the same self-reported cap, for the
    /// byte-exact capture path.
    pub fn refuse_if_truncated(&self, what: &str) -> crate::Result<()> {
        if self.stdout_truncated {
            return Err(truncation_refusal_error(
                what,
                self.exit_code,
                self.capture_limit,
            ));
        }
        Ok(())
    }
}

/// Best-effort persist a capture's stdout/stderr to a temp file for
/// diagnosability when a run failed or truncated, so a park is diagnosable
/// without re-running the command by hand (four live merge-gate parks on
/// 2026-07-24 each cost a full manual reproduction because the only record
/// was the exit code). Returns `None` when the run succeeded with no
/// truncation, or when the best-effort write itself fails — which must never
/// mask the run's own outcome. `label` names the caller/command (used
/// verbatim in the file name, e.g. `"gate-foo"`, `"worktree-setup"`) and
/// `scope_id` disambiguates concurrent runs (e.g. a run id); only its first
/// 16 characters are used. Shared by every caller that runs a declared,
/// repository-owned command and wants identical on-failure diagnosability —
/// the merge gate/rebuild step and worktree setup commands — rather than
/// reimplementing the capture-on-failure logic beside each other.
pub fn persist_failure_capture(
    label: &str,
    scope_id: &str,
    outcome: &RunOutput,
) -> Option<Utf8PathBuf> {
    if outcome.success && !outcome.stdout_truncated && !outcome.stderr_truncated {
        return None;
    }
    let path = std::env::temp_dir().join(format!(
        "ctx-{label}-{}.log",
        &scope_id[..scope_id.len().min(16)]
    ));
    std::fs::write(
        &path,
        format!("{}\n─── stderr ───\n{}", outcome.stdout, outcome.stderr),
    )
    .ok()?;
    Utf8PathBuf::from_path_buf(path).ok()
}

/// Prefix shared by every gate/rebuild failure-capture file
/// [`persist_failure_capture`] writes (its `label` argument is always
/// `"gate-*"` for a merge-gate or `[[merge.generated]]` rebuild command).
pub const GATE_CAPTURE_FILE_PREFIX: &str = "ctx-gate-";

/// One stale gate failure-capture file found in the host temp directory
/// (P462 doctor debris sweep).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleCaptureFile {
    pub path: Utf8PathBuf,
}

/// Every `ctx-gate-*.log` capture file in the host temp directory older than
/// `max_age` (P462 doctor debris sweep): [`persist_failure_capture`] writes
/// these on a failed/truncated gate run, but nothing else ever removes them.
/// Matches only ctx's own deterministic name prefix — never touches any
/// other file in the shared temp directory.
pub fn stale_gate_capture_files(max_age: Duration) -> crate::Result<Vec<StaleCaptureFile>> {
    let dir = std::env::temp_dir();
    let now = std::time::SystemTime::now();
    let entries =
        std::fs::read_dir(&dir).map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string_lossy().into_owned(),
            source,
        })?;
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string_lossy().into_owned(),
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.starts_with(GATE_CAPTURE_FILE_PREFIX) || !name.ends_with(".log") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() < max_age {
            continue;
        }
        if let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) {
            stale.push(StaleCaptureFile { path });
        }
    }
    Ok(stale)
}

/// Delete a stale gate capture file (P462 doctor `--apply`). A missing path
/// is a normal no-op, so a second `--apply` pass is idempotent.
pub fn remove_stale_capture_file(path: &Utf8Path) -> crate::Result<()> {
    match std::fs::remove_file(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

/// Byte-exact-stdout counterpart to [`run_with_env`]: identical bounded
/// timeout/spawn/wait/capture-limit policy (including the same
/// `tick_observer` cadence), but preserves `stdout` as raw bytes instead of
/// lossily decoding it to UTF-8. Routes through the same [`run_raw`] engine
/// as [`run_with_env`], so byte-exact reads keep the same bounded-execution
/// and typed-error guarantees as every other command edge.
pub fn run_bytes_with_env(
    request: RunRequest<'_>,
    env_overlay: &BTreeMap<String, String>,
) -> crate::Result<RunBytesOutput> {
    let raw = run_raw(&request, env_overlay)?;
    let (stderr, stderr_truncated) = lossy_utf8(raw.stderr, raw.stderr_truncated);
    Ok(RunBytesOutput {
        exit_code: raw.exit_code,
        stdout: raw.stdout,
        stdout_truncated: raw.stdout_truncated,
        stderr,
        stderr_truncated,
        timed_out: raw.timed_out,
        success: raw.success,
        capture_limit: raw.capture_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Read` implementation that yields a fixed number of bytes and then a
    /// genuine `io::Error`, never EOF — proving [`read_capped`] surfaces a
    /// real read failure as `Err` rather than downgrading it to a short,
    /// apparently-successful capture.
    struct FailingReader {
        remaining: &'static [u8],
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining.is_empty() {
                return Err(std::io::Error::other("simulated pipe read failure"));
            }
            let take = buf.len().min(self.remaining.len());
            buf[..take].copy_from_slice(&self.remaining[..take]);
            self.remaining = &self.remaining[take..];
            Ok(take)
        }
    }

    #[test]
    fn read_capped_propagates_reader_failure_instead_of_partial_success() {
        let reader = FailingReader {
            remaining: b"partial",
        };
        let result = read_capped_observed(reader, 1024, None, Instant::now());
        assert!(
            result.is_err(),
            "a real read error must never be reported as EOF/partial success"
        );
    }

    #[test]
    fn read_capped_reads_exact_bytes_under_limit() {
        let reader = std::io::Cursor::new(b"hello world".to_vec());
        let (bytes, truncated) =
            read_capped_observed(reader, 1024, None, Instant::now()).expect("read succeeds");
        assert_eq!(bytes, b"hello world");
        assert!(!truncated);
    }

    /// Binary stdout larger than any OS pipe buffer (well past the ~64KiB a
    /// pipe can hold unread) must still complete byte-exactly without
    /// deadlocking, proving the concurrent stdout/stderr drain in [`run_raw`]
    /// actually keeps the child unblocked while it writes.
    #[test]
    fn run_bytes_with_env_captures_binary_output_larger_than_pipe_capacity() {
        let byte_count = 5_000_000usize;
        let argv = vec![
            "dd".to_string(),
            "if=/dev/zero".to_string(),
            "bs=1000000".to_string(),
            "count=5".to_string(),
        ];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(30_000),
            idle_timeout_ms: None,
            capture_limit: byte_count + 1,
            tick_observer: None,
        };
        let output = run_bytes_with_env(request, &EMPTY_ENV_OVERLAY).expect("dd runs");
        assert!(output.success, "dd must exit successfully: {output:?}");
        assert!(!output.stdout_truncated);
        assert_eq!(output.stdout.len(), byte_count);
        assert!(output.stdout.iter().all(|byte| *byte == 0));
    }

    /// The same oversized output, captured with a small `capture_limit`, must
    /// report truncation while still draining the child to completion rather
    /// than deadlocking on the untaken remainder.
    #[test]
    fn run_bytes_with_env_reports_truncation_under_capture_limit() {
        let argv = vec![
            "dd".to_string(),
            "if=/dev/zero".to_string(),
            "bs=1000000".to_string(),
            "count=5".to_string(),
        ];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(30_000),
            idle_timeout_ms: None,
            capture_limit: 100,
            tick_observer: None,
        };
        let output = run_bytes_with_env(request, &EMPTY_ENV_OVERLAY).expect("dd runs");
        assert!(output.success, "dd must exit successfully: {output:?}");
        assert!(output.stdout_truncated);
        assert_eq!(output.stdout.len(), 100);
    }

    /// 0084: idle and wall are enforced independently, so a request with a
    /// generous idle window but a tight wall-clock ceiling must still be
    /// killed by the wall and report `TimeoutKind::Wall`'s reason — a
    /// silent-but-hung child is not the only way to overrun, and the wall
    /// stays the backstop regardless of how idle-forgiving a step is.
    #[test]
    fn a_small_wall_fires_before_a_large_idle_window_and_names_the_wall() {
        let argv = vec!["sleep".to_string(), "5".to_string()];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(50),
            idle_timeout_ms: Some(10_000),
            capture_limit: 1024,
            tick_observer: None,
        };
        let output = run_with_env(request, &EMPTY_ENV_OVERLAY).expect("sleep runs");
        assert!(output.timed_out);
        assert_eq!(
            output.timeout_reason,
            Some(TimeoutKind::Wall.reason()),
            "a tight wall under a generous idle window must be named as the wall bound"
        );
    }

    /// [`RunOutput::refuse_if_truncated`] fires on a genuinely truncated
    /// capture and names the real applied cap (`self.capture_limit`) in the
    /// error, never a value the caller would have to restate.
    #[test]
    fn run_output_refuse_if_truncated_names_the_applied_cap() {
        let argv = vec![
            "head".to_string(),
            "-c".to_string(),
            "300000".to_string(),
            "/dev/zero".to_string(),
        ];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(30_000),
            idle_timeout_ms: None,
            capture_limit: 1024,
            tick_observer: None,
        };
        let output = run_with_env(request, &EMPTY_ENV_OVERLAY).expect("head runs");
        assert!(output.stdout_truncated);
        let error = output
            .refuse_if_truncated("head -c 300000 /dev/zero")
            .expect_err("a truncated capture must refuse");
        let message = error.to_string();
        assert!(
            message.contains("head -c 300000 /dev/zero"),
            "expected the refusal to name the capture: {message}"
        );
        assert!(
            message.contains("1024"),
            "expected the refusal to name the applied 1024-byte cap: {message}"
        );
    }

    /// An untruncated capture never refuses, regardless of how close to the
    /// cap it lands.
    #[test]
    fn run_output_refuse_if_truncated_is_a_no_op_when_untruncated() {
        let argv = vec!["true".to_string()];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(30_000),
            idle_timeout_ms: None,
            capture_limit: 1024,
            tick_observer: None,
        };
        let output = run_with_env(request, &EMPTY_ENV_OVERLAY).expect("true runs");
        assert!(!output.stdout_truncated);
        output
            .refuse_if_truncated("true")
            .expect("an untruncated capture must never refuse");
    }

    /// Byte-exact counterpart: [`RunBytesOutput::refuse_if_truncated`] fires
    /// identically on the byte-exact capture path, sharing the same message
    /// vocabulary as the text path.
    #[test]
    fn run_bytes_output_refuse_if_truncated_names_the_applied_cap() {
        let argv = vec![
            "dd".to_string(),
            "if=/dev/zero".to_string(),
            "bs=1000000".to_string(),
            "count=5".to_string(),
        ];
        let request = RunRequest {
            argv: &argv,
            cwd: None,
            exec_dir: None,
            success_exit_code: &[0],
            timeout_ms: Some(30_000),
            idle_timeout_ms: None,
            capture_limit: 100,
            tick_observer: None,
        };
        let output = run_bytes_with_env(request, &EMPTY_ENV_OVERLAY).expect("dd runs");
        assert!(output.stdout_truncated);
        let error = output
            .refuse_if_truncated("dd if=/dev/zero bs=1000000 count=5")
            .expect_err("a truncated byte-exact capture must refuse");
        let message = error.to_string();
        assert!(
            message.contains("dd if=/dev/zero bs=1000000 count=5"),
            "expected the refusal to name the capture: {message}"
        );
        assert!(
            message.contains("100"),
            "expected the refusal to name the applied 100-byte cap: {message}"
        );
    }
}
