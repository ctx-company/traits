//! Harness subprocess runner for agent frame execution.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

use crate::confinement::SpawnSandbox;

pub const DEFAULT_CAPTURE_LIMIT: usize = 4 * 1024 * 1024;

pub type OutputObserver = Arc<dyn Fn(&[u8]) + Send + Sync + 'static>;
pub type TickObserver = Arc<dyn Fn() + Send + Sync + 'static>;

/// Incrementally extracts output-token usage without retaining harness output.
#[derive(Default)]
pub struct OutputTokenCounter {
    incomplete_line: Vec<u8>,
    discarding_line: bool,
    cumulative_tokens: u64,
    incremental_tokens: u64,
    /// claude-code `assistant` events report usage under `message.usage`,
    /// repeated per snapshot of the same message — tracked as a per-message
    /// maximum keyed by `message.id` (covers subagent messages too, which
    /// stream with their own ids).
    message_tokens: BTreeMap<String, u64>,
    /// claude-code `system/task_progress` events report a delegated Task's
    /// cumulative `total_tokens` (input+output) — the only live signal while
    /// a worker delegates to a subagent; tracked as a per-task maximum.
    task_tokens: BTreeMap<String, u64>,
}

impl OutputTokenCounter {
    // Harness result events include the full response alongside usage metadata.
    // Match the capture bound so a valid captured result does not lose its usage.
    const MAX_LINE_BYTES: usize = DEFAULT_CAPTURE_LIMIT;

    /// Returns the newly observed token delta, if this chunk advances the total.
    pub fn push(&mut self, chunk: &[u8]) -> Option<u64> {
        let previous = self.total();
        for line in chunk.split_inclusive(|byte| *byte == b'\n') {
            if self.discarding_line {
                if line.ends_with(b"\n") {
                    self.discarding_line = false;
                }
                continue;
            }
            self.incomplete_line.extend_from_slice(line);
            if self.incomplete_line.len() > Self::MAX_LINE_BYTES {
                self.incomplete_line.clear();
                self.discarding_line = !line.ends_with(b"\n");
                continue;
            }
            if self.incomplete_line.ends_with(b"\n") {
                self.parse_line();
            }
        }
        let total = self.total();
        (total != previous).then_some(total - previous)
    }

    /// Parse one final, unterminated JSON value after stdout closes and return its delta.
    pub fn finish(&mut self) -> Option<u64> {
        let previous = self.total();
        if !self.discarding_line {
            self.parse_line();
        }
        let total = self.total();
        (total != previous).then_some(total - previous)
    }

    fn parse_line(&mut self) {
        let Ok(value) = serde_json::from_slice::<Value>(&self.incomplete_line) else {
            self.incomplete_line.clear();
            return;
        };
        self.incomplete_line.clear();
        self.observe(&value);
    }

    fn observe(&mut self, value: &Value) {
        // Final `result` events (and any other top-level usage) report the
        // run-cumulative output total.
        if let Some(tokens) = top_level_output_tokens(value) {
            self.cumulative_tokens = self.cumulative_tokens.max(tokens);
        }
        // OpenCode `step_finish` reports one completed model step; each event
        // is an increment, not a run-wide total.
        if let Some(tokens) = opencode_step_output_tokens(value) {
            self.incremental_tokens = self.incremental_tokens.saturating_add(tokens);
        }
        if let Some((id, tokens)) = message_output_tokens(value) {
            let entry = self.message_tokens.entry(id).or_default();
            *entry = (*entry).max(tokens);
        }
        if let Some((key, tokens)) = task_progress_total_tokens(value) {
            let entry = self.task_tokens.entry(key).or_default();
            *entry = (*entry).max(tokens);
        }
    }

    /// Panel liveness meter, not billing: the channels differ in unit
    /// (claude `result`/`message.usage` count output tokens; `task_progress`
    /// counts a delegated Task's input+output total) and overlap (a task's
    /// total includes subagent messages also seen individually), so the
    /// honest single number that never double-counts is the max over
    /// channels. Before this, a claude worker that delegated to a subagent —
    /// or timed out before its `result` event — reported 0 tokens for the
    /// whole frame (an hour of visible work at "0 tok", seen live).
    fn total(&self) -> u64 {
        let message_sum: u64 = self.message_tokens.values().sum();
        let task_sum: u64 = self.task_tokens.values().sum();
        self.cumulative_tokens
            .max(self.incremental_tokens)
            .max(message_sum)
            .max(task_sum)
    }
}

/// One harness attempt's shared output-token accounting helper (P445):
/// composes an [`OutputObserver`] that feeds every chunk into a fresh
/// [`OutputTokenCounter`] and reports each push's delta to `on_delta` as it
/// streams, then `finish` reports the counter's own final unterminated-line
/// delta the same way. This is the single per-attempt accounting shape used
/// by both the work-agent path (`drive.rs`, which folds every delta into a
/// drive-wide total and a live panel) and the narrator path
/// (`harness_stream.rs`, which folds every delta into that call's own
/// running total) — neither reimplements counter wiring, and neither can
/// drop a delta the other keeps: every `push`/`finish` delta this counter
/// ever returns reaches `on_delta` exactly once.
#[derive(Clone)]
pub struct AttemptTokenAccumulator {
    counter: Arc<std::sync::Mutex<OutputTokenCounter>>,
    on_delta: Arc<dyn Fn(u64) + Send + Sync + 'static>,
}

impl AttemptTokenAccumulator {
    pub fn new(on_delta: impl Fn(u64) + Send + Sync + 'static) -> Self {
        Self {
            counter: Arc::default(),
            on_delta: Arc::new(on_delta),
        }
    }

    pub fn observer(&self) -> OutputObserver {
        let counter = self.counter.clone();
        let on_delta = self.on_delta.clone();
        Arc::new(move |chunk: &[u8]| {
            let Ok(mut counter) = counter.lock() else {
                return;
            };
            if let Some(delta) = counter.push(chunk) {
                on_delta(delta);
            }
        })
    }

    /// Flush the counter's final, unterminated line once this attempt's
    /// stdout has closed. Idempotent to call at most once per attempt.
    pub fn finish(&self) {
        let Ok(mut counter) = self.counter.lock() else {
            return;
        };
        if let Some(delta) = counter.finish() {
            (self.on_delta)(delta);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptDelivery {
    Arg,
    Stdin,
}

pub struct HarnessRunRequest {
    pub argv: Vec<String>,
    /// Worktree `[worktree].env` overlay applied to the spawned harness
    /// process before `env_remove`, so credential stripping always wins over a
    /// config-provided value. Empty for host-side/no-overlay runs.
    pub env_overlay: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub prompt: String,
    pub prompt_delivery: PromptDelivery,
    pub timeout_ms: u64,
    pub idle_timeout_ms: Option<u64>,
    pub capture_limit: usize,
    pub stream: bool,
    pub stdout_observer: Option<OutputObserver>,
    pub tick_observer: Option<TickObserver>,
    /// Real execution directory for a worktree-scoped run. `None` leaves the
    /// spawned process on the caller's own current directory, unchanged from
    /// today.
    pub exec_dir: Option<Utf8PathBuf>,
    /// P480: the generated OS-level spawn sandbox to wrap this argv under,
    /// `None` for a non-worktree run, disabled/unsupported OS enforcement, or
    /// a host-side call. Applied at the spawn seam only — never folded into
    /// `HarnessRunOutcome.argv` or any other reported/ledgered value (see
    /// [`crate::confinement::SpawnSandbox::argv_prefix`]).
    pub sandbox: Option<SpawnSandbox>,
}

pub struct HarnessSessionPrompt {
    pub prompt: String,
    pub timeout_ms: u64,
    pub idle_timeout_ms: Option<u64>,
    pub capture_limit: usize,
    pub stream: bool,
    pub stdout_observer: Option<OutputObserver>,
    pub tick_observer: Option<TickObserver>,
}

/// `Serialize`/`Deserialize` (added for P402) let a completed outcome be
/// persisted verbatim into a durable per-branch sidecar
/// ([`crate::run_branch`]) and read back on a resumed drive invocation
/// without re-dispatching the harness call. Every field is plain data
/// (strings, ints, bools), so this is a mechanical round-trip with no custom
/// (de)serialization logic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HarnessRunOutcome {
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub idle_timed_out: bool,
    /// P551: `true` when this outcome ended because
    /// `interrupt::request_kill()` `SIGKILL`ed the child's process group —
    /// distinct from `timed_out`/`idle_timed_out` and from a genuine
    /// non-zero exit, so callers never retry or misreport a deliberately
    /// killed run as a harness failure. `#[serde(default)]` so a sidecar
    /// persisted before this field existed still deserializes (as
    /// not-killed, the only sound default for historical data).
    #[serde(default)]
    pub killed: bool,
    pub duration_ms: u128,
}

#[derive(Debug)]
struct PipeCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

struct SessionOutput {
    stdout: PipeCapture,
    stderr: PipeCapture,
    stdout_lines: Vec<u8>,
}

struct SessionOutputDrain {
    output_seen: bool,
    result_seen: bool,
}

pub struct HarnessSession {
    argv: Vec<String>,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: mpsc::Receiver<Vec<u8>>,
    stderr: mpsc::Receiver<Vec<u8>>,
}

/// Linux's per-argument hard cap (`MAX_ARG_STRLEN` = 131072 bytes) is tighter
/// than macOS's ~1MiB shared `ARG_MAX`, so a prompt over this budget was
/// already unspawnable via argv on Linux — refusing it here before spawn
/// cannot regress a dispatch that previously worked anywhere.
const ARGV_PROMPT_BUDGET_BYTES: usize = 128 * 1024;

pub fn run(request: HarnessRunRequest) -> crate::Result<HarnessRunOutcome> {
    validate_argv(&request.argv)?;
    if matches!(request.prompt_delivery, PromptDelivery::Arg)
        && request.prompt.len() > ARGV_PROMPT_BUDGET_BYTES
    {
        return Err(crate::Error::Usage {
            message: format!(
                "refusing to dispatch {}: prompt is {} bytes, over the {}-byte argv budget \
                 (would risk \"Argument list too long\"); the full prompt is in the debug \
                 trace, not repeated here",
                request.argv[0],
                request.prompt.len(),
                ARGV_PROMPT_BUDGET_BYTES,
            ),
        });
    }

    let mut argv = request.argv.clone();
    if matches!(request.prompt_delivery, PromptDelivery::Arg) {
        argv.push(request.prompt.clone());
    }
    let spawn_argv = sandboxed_argv(&argv, request.sandbox.as_ref());
    let mut command = Command::new(&spawn_argv[0]);
    command.args(&spawn_argv[1..]);
    crate::command::apply_env_overlay(&mut command, &request.env_overlay);
    for key in &request.env_remove {
        command.env_remove(key);
    }
    apply_exec_dir(&mut command, request.exec_dir.as_deref());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(match request.prompt_delivery {
        PromptDelivery::Arg => Stdio::null(),
        PromptDelivery::Stdin => Stdio::piped(),
    });
    // P551: its own process group so an instant kill (`SIGKILL` to `-pgid`)
    // reaches every descendant this child forks, not just the direct child.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let started = Instant::now();
    let last_output_ms = Arc::new(AtomicU64::new(0));
    let mut child = command
        .spawn()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: spawn_failure_path(&argv),
            source,
        })?;
    let pgid = child.id() as i32;
    crate::run_kill::register(pgid);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: spawn_failure_path(&argv),
            source: std::io::Error::other("failed to open harness stdout"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: spawn_failure_path(&argv),
            source: std::io::Error::other("failed to open harness stderr"),
        })?;
    let limit = if request.capture_limit == 0 {
        DEFAULT_CAPTURE_LIMIT
    } else {
        request.capture_limit
    };
    let stdout_handle = spawn_capture(
        stdout,
        limit,
        started,
        Arc::clone(&last_output_ms),
        request.stdout_observer.clone(),
    );
    let stderr_handle = spawn_capture(stderr, limit, started, Arc::clone(&last_output_ms), None);

    if matches!(request.prompt_delivery, PromptDelivery::Stdin) {
        let mut stdin =
            child
                .stdin
                .take()
                .ok_or_else(|| crate::environment::Error::Filesystem {
                    path: spawn_failure_path(&argv),
                    source: std::io::Error::other("failed to open harness stdin"),
                })?;
        if let Err(source) = stdin
            .write_all(request.prompt.as_bytes())
            .and_then(|()| stdin.flush())
        {
            let _ = child.kill();
            let _ = child.wait();
            crate::run_kill::clear(pgid);
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(crate::environment::Error::Filesystem {
                path: spawn_failure_path(&argv),
                source,
            }
            .into());
        }
    }

    let timeout = Duration::from_millis(request.timeout_ms);
    let idle_timeout = request
        .idle_timeout_ms
        .filter(|_| request.stream)
        .map(Duration::from_millis);
    let mut timed_out = false;
    let mut idle_timed_out = false;
    loop {
        match child
            .try_wait()
            .map_err(|source| crate::environment::Error::Filesystem {
                path: spawn_failure_path(&argv),
                source,
            })? {
            Some(_) => break,
            None if started.elapsed() >= timeout => {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            None => {
                if let Some(observer) = request.tick_observer.as_ref() {
                    observer();
                }
                if let Some(idle_timeout) = idle_timeout {
                    let last = Duration::from_millis(last_output_ms.load(Ordering::Relaxed));
                    if started.elapsed().saturating_sub(last) >= idle_timeout {
                        idle_timed_out = true;
                        let _ = child.kill();
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: spawn_failure_path(&argv),
            source,
        })?;
    let killed = crate::run_kill::was_killed();
    crate::run_kill::clear(pgid);
    let command_text = spawn_failure_path(&argv);
    let stdout = stdout_handle
        .join()
        .map_err(|_| crate::environment::Error::Process {
            command: Some(command_text.clone()),
            path: None,
            exit_status: status.code(),
            timed_out: false,
            message: "stdout capture thread panicked".to_string(),
        })?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| crate::environment::Error::Process {
            command: Some(command_text),
            path: None,
            exit_status: status.code(),
            timed_out: false,
            message: "stderr capture thread panicked".to_string(),
        })?;
    Ok(HarnessRunOutcome {
        argv,
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr.bytes).to_string(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        timed_out,
        idle_timed_out,
        killed,
        duration_ms: started.elapsed().as_millis(),
    })
}

fn spawn_capture<R>(
    mut reader: R,
    limit: usize,
    started: Instant,
    last_output_ms: Arc<AtomicU64>,
    observer: Option<OutputObserver>,
) -> std::thread::JoinHandle<PipeCapture>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    last_output_ms.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    if let Some(observer) = observer.as_ref() {
                        observer(&buffer[..read]);
                    }
                    append_limited(&mut bytes, &mut truncated, &buffer[..read], limit);
                }
                Err(_) => break,
            }
        }
        PipeCapture { bytes, truncated }
    })
}

impl HarnessSession {
    pub fn spawn(
        argv: Vec<String>,
        env_overlay: BTreeMap<String, String>,
        env_remove: Vec<String>,
        exec_dir: Option<Utf8PathBuf>,
        sandbox: Option<SpawnSandbox>,
    ) -> crate::Result<Self> {
        validate_argv(&argv)?;
        let spawn_argv = sandboxed_argv(&argv, sandbox.as_ref());
        let mut command = Command::new(&spawn_argv[0]);
        command.args(&spawn_argv[1..]);
        crate::command::apply_env_overlay(&mut command, &env_overlay);
        for key in env_remove {
            command.env_remove(key);
        }
        apply_exec_dir(&mut command, exec_dir.as_deref());
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child =
            command
                .spawn()
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: spawn_failure_path(&argv),
                    source,
                })?;
        crate::run_kill::register(child.id() as i32);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: spawn_failure_path(&argv),
                source: std::io::Error::other("failed to open warm harness stdin"),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: spawn_failure_path(&argv),
                source: std::io::Error::other("failed to open warm harness stdout"),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: spawn_failure_path(&argv),
                source: std::io::Error::other("failed to open warm harness stderr"),
            })?;

        Ok(Self {
            argv,
            child,
            stdin: Some(stdin),
            stdout: spawn_channel_reader(stdout),
            stderr: spawn_channel_reader(stderr),
        })
    }

    pub fn prompt(&mut self, request: HarnessSessionPrompt) -> crate::Result<HarnessRunOutcome> {
        if let Some(status) =
            self.child
                .try_wait()
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: spawn_failure_path(&self.argv),
                    source,
                })?
        {
            return Err(crate::environment::Error::Process {
                command: Some(self.argv.join(" ")),
                path: None,
                exit_status: status.code(),
                timed_out: false,
                message: "warm harness exited before prompt".to_string(),
            }
            .into());
        }
        discard_pending_session_output(&self.stdout, &self.stderr);

        let started = Instant::now();
        let timeout = Duration::from_millis(request.timeout_ms);
        let prompt = claude_stream_json_user_message(&request.prompt)?;
        let stdin = self
            .stdin
            .take()
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: spawn_failure_path(&self.argv),
                source: std::io::Error::other("warm harness stdin is unavailable"),
            })?;
        match write_stdin_with_deadline(stdin, prompt, timeout) {
            Ok((stdin, Ok(()))) => self.stdin = Some(stdin),
            Ok((stdin, Err(source))) => {
                self.stdin = Some(stdin);
                let _ = self.child.kill();
                return Err(crate::environment::Error::Filesystem {
                    path: spawn_failure_path(&self.argv),
                    source,
                }
                .into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                return Err(crate::environment::Error::Process {
                    command: Some(self.argv.join(" ")),
                    path: None,
                    exit_status: None,
                    timed_out: true,
                    message: format!("stdin write timed out after {}ms", request.timeout_ms),
                }
                .into());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = self.child.kill();
                return Err(crate::environment::Error::Process {
                    command: Some(self.argv.join(" ")),
                    path: None,
                    exit_status: None,
                    timed_out: false,
                    message: "warm harness stdin write thread exited without result".to_string(),
                }
                .into());
            }
        }

        let limit = if request.capture_limit == 0 {
            DEFAULT_CAPTURE_LIMIT
        } else {
            request.capture_limit
        };
        let idle_timeout = request
            .idle_timeout_ms
            .filter(|_| request.stream)
            .map(Duration::from_millis);
        let mut output = SessionOutput::new();
        let mut exit_code = None;
        let mut timed_out = false;
        let mut idle_timed_out = false;
        let mut result_seen = false;
        let mut last_output = started;
        loop {
            let drained = drain_session_output(
                &self.stdout,
                &self.stderr,
                &mut output,
                limit,
                request.stdout_observer.as_ref(),
                false,
            );
            if drained.output_seen {
                last_output = Instant::now();
            }
            if drained.result_seen {
                result_seen = true;
            }
            if result_seen {
                exit_code = Some(0);
                break;
            }
            if let Some(status) =
                self.child
                    .try_wait()
                    .map_err(|source| crate::environment::Error::Filesystem {
                        path: spawn_failure_path(&self.argv),
                        source,
                    })?
            {
                exit_code = status.code();
                break;
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                let _ = self.child.kill();
                break;
            }
            if let Some(idle_timeout) = idle_timeout
                && last_output.elapsed() >= idle_timeout
            {
                idle_timed_out = true;
                let _ = self.child.kill();
                break;
            }
            if let Some(observer) = request.tick_observer.as_ref() {
                observer();
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let _ = drain_session_output(
            &self.stdout,
            &self.stderr,
            &mut output,
            limit,
            request.stdout_observer.as_ref(),
            true,
        );

        Ok(HarnessRunOutcome {
            argv: self.argv.clone(),
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout.bytes).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr.bytes).to_string(),
            stdout_truncated: output.stdout.truncated,
            stderr_truncated: output.stderr.truncated,
            timed_out,
            idle_timed_out,
            killed: crate::run_kill::was_killed(),
            duration_ms: started.elapsed().as_millis(),
        })
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

fn top_level_output_tokens(value: &Value) -> Option<u64> {
    value
        .get("usage")
        .and_then(Value::as_object)
        .and_then(|usage| {
            usage
                .get("output_tokens")
                .or_else(|| usage.get("outputTokens"))
        })
        .and_then(Value::as_u64)
}

fn opencode_step_output_tokens(value: &Value) -> Option<u64> {
    value
        .get("part")
        .and_then(|part| part.get("tokens"))
        .and_then(|tokens| tokens.get("output"))
        .and_then(Value::as_u64)
}

/// claude-code `assistant` events: `{"type":"assistant","message":{"id":…,
/// "usage":{"output_tokens":…}}}` — snapshots repeat per message with a
/// growing total, and subagent messages stream with their own ids.
fn message_output_tokens(value: &Value) -> Option<(String, u64)> {
    let message = value.get("message")?;
    let id = message.get("id")?.as_str()?.to_string();
    let tokens = message
        .get("usage")
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)?;
    Some((id, tokens))
}

/// claude-code `system/task_progress` events: the cumulative `total_tokens`
/// of a delegated Task tool call — the only live usage signal while a
/// harness agent delegates its frame to a subagent. Keyed by the task's
/// `parent_tool_use_id` when present so sequential tasks each keep their
/// own running maximum.
fn task_progress_total_tokens(value: &Value) -> Option<(String, u64)> {
    if value.get("type").and_then(Value::as_str) != Some("system") {
        return None;
    }
    if value.get("subtype").and_then(Value::as_str) != Some("task_progress") {
        return None;
    }
    let tokens = value
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(Value::as_u64)?;
    let key = value
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("task")
        .to_string();
    Some((key, tokens))
}

impl Drop for HarnessSession {
    fn drop(&mut self) {
        let pgid = self.child.id() as i32;
        let _ = self.child.kill();
        let _ = self.child.try_wait();
        crate::run_kill::clear(pgid);
    }
}

impl SessionOutput {
    fn new() -> Self {
        Self {
            stdout: PipeCapture {
                bytes: Vec::new(),
                truncated: false,
            },
            stderr: PipeCapture {
                bytes: Vec::new(),
                truncated: false,
            },
            stdout_lines: Vec::new(),
        }
    }
}

fn write_stdin_with_deadline(
    mut stdin: ChildStdin,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<(ChildStdin, std::io::Result<()>), mpsc::RecvTimeoutError> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = stdin.write_all(&bytes).and_then(|()| stdin.flush());
        let _ = sender.send((stdin, result));
    });
    receiver.recv_timeout(timeout)
}

/// Apply a worktree-scoped execution directory to a harness `Command`. Shared
/// by cold (`run`) and persistent (`HarnessSession::spawn`) harness spawns so
/// the two paths cannot diverge; `None` leaves the child on its inherited cwd,
/// unchanged from today.
fn apply_exec_dir(command: &mut Command, exec_dir: Option<&Utf8Path>) {
    if let Some(dir) = exec_dir {
        command.current_dir(dir);
    }
}

/// Apply a P480 OS-level spawn sandbox to an argv already fully assembled
/// (prompt-appended for `PromptDelivery::Arg`, confinement/exec-dir flags
/// already in place): prepend `sandbox.argv_prefix` so `Command::new`/`args`
/// exec the harness under `sandbox-exec`, e.g. `sandbox-exec` execs its
/// target via `execvp`, so a bare `argv[0]` still resolves through `PATH`.
/// The RETURNED vector is used only for the actual `Command` — callers must
/// keep using the unwrapped `argv` for `HarnessRunOutcome.argv`, warm-pool
/// identity, and every ledger/provenance surface (operational-only, never
/// canonical; see [`crate::confinement::SpawnSandbox::argv_prefix`]). Shared
/// by cold (`run`) and persistent (`HarnessSession::spawn`) harness spawns so
/// the two paths cannot diverge, mirroring [`apply_exec_dir`]'s precedent.
fn sandboxed_argv(argv: &[String], sandbox: Option<&SpawnSandbox>) -> Vec<String> {
    match sandbox {
        Some(sandbox) => sandbox
            .argv_prefix
            .iter()
            .cloned()
            .chain(argv.iter().cloned())
            .collect(),
        None => argv.to_vec(),
    }
}

/// A spawn-failure `path:` value that names the command shape without ever
/// repeating the prompt body: every caller of [`run`]/[`HarnessSession::spawn`]
/// may push a payload-derived prompt onto `argv`, and `environment::Error::Filesystem`'s
/// `path` field is rendered verbatim into callers' `raw-reason` text (e.g.
/// merge's `unexpected failure during merge: {error}`).
fn spawn_failure_path(argv: &[String]) -> String {
    let bytes: usize = argv.iter().map(|arg| arg.len()).sum();
    let head = argv.first().map(String::as_str).unwrap_or("<empty argv>");
    format!("{head} …({} args, {bytes} bytes)", argv.len())
}

fn validate_argv(argv: &[String]) -> crate::Result<()> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(crate::Error::Usage {
            message: "harness argv is empty".to_string(),
        });
    }
    Ok(())
}

fn claude_stream_json_user_message(prompt: &str) -> crate::Result<Vec<u8>> {
    let value = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{ "type": "text", "text": prompt }],
        },
    });
    let mut line =
        serde_json::to_vec(&value).map_err(|source| crate::parse::Error::JsonSerialize {
            context: "harness.warm.prompt".to_string(),
            source,
        })?;
    line.push(b'\n');
    Ok(line)
}

fn spawn_channel_reader<R>(mut reader: R) -> mpsc::Receiver<Vec<u8>>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

fn drain_session_output(
    stdout_rx: &mpsc::Receiver<Vec<u8>>,
    stderr_rx: &mpsc::Receiver<Vec<u8>>,
    output: &mut SessionOutput,
    limit: usize,
    observer: Option<&OutputObserver>,
    include_remainder: bool,
) -> SessionOutputDrain {
    let mut output_seen = false;
    let mut result_seen = false;
    while let Ok(chunk) = stdout_rx.try_recv() {
        output_seen = true;
        if let Some(observer) = observer {
            observer(&chunk);
        }
        append_limited(
            &mut output.stdout.bytes,
            &mut output.stdout.truncated,
            &chunk,
            limit,
        );
        output.stdout_lines.extend_from_slice(&chunk);
        result_seen |= drain_result_events(&mut output.stdout_lines, false);
    }
    while let Ok(chunk) = stderr_rx.try_recv() {
        output_seen = true;
        append_limited(
            &mut output.stderr.bytes,
            &mut output.stderr.truncated,
            &chunk,
            limit,
        );
    }
    if include_remainder {
        result_seen |= drain_result_events(&mut output.stdout_lines, true);
    }
    SessionOutputDrain {
        output_seen,
        result_seen,
    }
}

fn discard_pending_session_output(
    stdout_rx: &mpsc::Receiver<Vec<u8>>,
    stderr_rx: &mpsc::Receiver<Vec<u8>>,
) {
    while stdout_rx.try_recv().is_ok() {}
    while stderr_rx.try_recv().is_ok() {}
}

fn drain_result_events(buffer: &mut Vec<u8>, include_remainder: bool) -> bool {
    let mut result_seen = false;
    while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
        let mut line = buffer.drain(..=index).collect::<Vec<_>>();
        trim_json_line(&mut line);
        result_seen |= line_is_result_event(&line);
    }
    if include_remainder && !buffer.is_empty() {
        let mut line = std::mem::take(buffer);
        trim_json_line(&mut line);
        result_seen |= line_is_result_event(&line);
    }
    result_seen
}

fn trim_json_line(line: &mut Vec<u8>) {
    while matches!(line.last(), Some(b'\n' | b'\r' | b' ' | b'\t')) {
        line.pop();
    }
    let leading = line
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(line.len());
    if leading > 0 {
        line.drain(..leading);
    }
}

fn line_is_result_event(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return false;
    };
    value.get("type").and_then(Value::as_str) == Some("result")
}

fn append_limited(bytes: &mut Vec<u8>, truncated: &mut bool, chunk: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(bytes.len());
    if chunk.len() > remaining {
        bytes.extend_from_slice(&chunk[..remaining]);
        *truncated = true;
    } else if remaining > 0 {
        bytes.extend_from_slice(chunk);
    } else {
        *truncated = true;
    }
}

#[cfg(test)]
mod output_token_counter_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        HarnessRunRequest, HarnessSession, HarnessSessionPrompt, OutputTokenCounter, PromptDelivery,
    };

    fn feed(counter: &mut OutputTokenCounter, line: &str) -> Option<u64> {
        let mut owned = line.to_string();
        owned.push('\n');
        counter.push(owned.as_bytes())
    }

    /// The live shapes from a delegating claude worker's stream: assistant events
    /// nest usage under `message.usage`, task_progress reports a delegated
    /// subagent's cumulative `total_tokens`, and the top-level `usage`
    /// arrives only on the final `result` event. The pre-fix counter read
    /// only the last shape, so a delegating or timed-out claude worker
    /// showed 0 tokens for the whole frame.
    #[test]
    fn claude_stream_counts_before_the_result_event() {
        let mut counter = OutputTokenCounter::default();
        assert_eq!(
            feed(&mut counter, r#"{"type":"system","subtype":"init"}"#),
            None
        );
        // Growing snapshots of one message: per-id maximum, not a sum.
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"assistant","message":{"id":"msg_a","usage":{"input_tokens":2,"output_tokens":5}}}"#
            ),
            Some(5)
        );
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"assistant","message":{"id":"msg_a","usage":{"input_tokens":2,"output_tokens":9}}}"#
            ),
            Some(4)
        );
        // A second message (a subagent's, streaming with its own id) adds on.
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"id":"msg_b","usage":{"output_tokens":6}}}"#
            ),
            Some(6)
        );
        // Delegated-task progress overtakes the per-message channel: the
        // total becomes the task's cumulative total_tokens (max over
        // channels, never a double-counting sum).
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"system","subtype":"task_progress","usage":{"total_tokens":21241}}"#
            ),
            Some(21241 - 15)
        );
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"system","subtype":"task_progress","usage":{"total_tokens":21500}}"#
            ),
            Some(259)
        );
        // The final result's run-cumulative output stays below the task
        // channel here, so the total does not move backwards.
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"result","usage":{"output_tokens":8000}}"#
            ),
            None
        );
    }

    #[test]
    fn opencode_step_finish_still_increments() {
        let mut counter = OutputTokenCounter::default();
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"step_finish","part":{"tokens":{"output":120}}}"#
            ),
            Some(120)
        );
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"step_finish","part":{"tokens":{"output":30}}}"#
            ),
            Some(30)
        );
    }

    #[test]
    fn timed_out_frame_without_result_event_still_reports() {
        let mut counter = OutputTokenCounter::default();
        feed(
            &mut counter,
            r#"{"type":"assistant","message":{"id":"msg_a","usage":{"output_tokens":7}}}"#,
        );
        // No result event ever arrives (frame timeout): finish() parses any
        // trailing partial line but the already-observed totals stand.
        assert_eq!(counter.finish(), None);
        assert_eq!(
            feed(
                &mut counter,
                r#"{"type":"assistant","message":{"id":"msg_c","usage":{"output_tokens":3}}}"#
            ),
            Some(3)
        );
    }

    #[test]
    // Timing headroom, deliberately generous (2026-08-03): the tick loop polls
    // every 20 ms, so a 120 ms child yields ~6 ticks and `>= 3` tolerated only
    // a 2x slowdown — which a shared macOS runner exceeds routinely, and this
    // failed there while passing everywhere else. A 500 ms child yields ~25,
    // so the same assertion now needs an 8x stall to trip. The property under
    // test is unchanged and if anything sharper: ticks arrive many times
    // within one second, which is what keeps a 1 s-resolution elapsed clock
    // moving. The timeout rises with it so a slow spawn cannot turn headroom
    // into a spurious timeout.
    fn cold_harness_ticks_multiple_times_before_a_second() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let observer = {
            let ticks = Arc::clone(&ticks);
            Arc::new(move || {
                ticks.fetch_add(1, Ordering::Relaxed);
            })
        };
        let outcome = super::run(HarnessRunRequest {
            argv: vec!["sh".to_string(), "-c".to_string(), "sleep 0.5".to_string()],
            env_overlay: BTreeMap::new(),
            env_remove: Vec::new(),
            prompt: String::new(),
            prompt_delivery: PromptDelivery::Arg,
            timeout_ms: 5_000,
            idle_timeout_ms: None,
            capture_limit: 0,
            stream: false,
            stdout_observer: None,
            tick_observer: Some(observer),
            exec_dir: None,
            sandbox: None,
        })
        .expect("cold harness succeeds");
        assert!(!outcome.timed_out);
        assert!(ticks.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    // Same headroom, same reason as the cold case above: identical shape, so
    // it is the next one to flake rather than a different risk.
    fn persistent_harness_ticks_multiple_times_before_a_second() {
        let mut session = HarnessSession::spawn(
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "while IFS= read -r _; do sleep 0.5; printf '{\"type\":\"result\"}\\n'; done"
                    .to_string(),
            ],
            BTreeMap::new(),
            Vec::new(),
            None,
            None,
        )
        .expect("warm harness starts");
        let ticks = Arc::new(AtomicUsize::new(0));
        let observer = {
            let ticks = Arc::clone(&ticks);
            Arc::new(move || {
                ticks.fetch_add(1, Ordering::Relaxed);
            })
        };
        let outcome = session
            .prompt(HarnessSessionPrompt {
                prompt: "hello".to_string(),
                timeout_ms: 5_000,
                idle_timeout_ms: None,
                capture_limit: 0,
                stream: false,
                stdout_observer: None,
                tick_observer: Some(observer),
            })
            .expect("warm prompt succeeds");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(ticks.load(Ordering::Relaxed) >= 3);
    }

    fn oversized_prompt() -> String {
        "x".repeat(super::ARGV_PROMPT_BUDGET_BYTES + 1)
    }

    #[test]
    fn oversized_arg_prompt_refuses_before_spawn() {
        let prompt = oversized_prompt();
        let error = super::run(HarnessRunRequest {
            argv: vec!["does-not-matter".to_string()],
            env_overlay: BTreeMap::new(),
            env_remove: Vec::new(),
            prompt: prompt.clone(),
            prompt_delivery: PromptDelivery::Arg,
            timeout_ms: 5_000,
            idle_timeout_ms: None,
            capture_limit: 0,
            stream: false,
            stdout_observer: None,
            tick_observer: None,
            exec_dir: None,
            sandbox: None,
        })
        .expect_err("an over-budget argv prompt must refuse before spawn");
        let message = error.to_string();
        assert!(message.contains("does-not-matter"), "{message}");
        assert!(message.contains(&(prompt.len()).to_string()), "{message}");
        assert!(!message.contains(&prompt), "{message}");
        assert!(!message.contains("os error 7"), "{message}");
    }

    #[test]
    fn oversized_stdin_prompt_still_dispatches() {
        let prompt = oversized_prompt();
        let outcome = super::run(HarnessRunRequest {
            argv: vec!["cat".to_string()],
            env_overlay: BTreeMap::new(),
            env_remove: Vec::new(),
            prompt: prompt.clone(),
            prompt_delivery: PromptDelivery::Stdin,
            timeout_ms: 5_000,
            idle_timeout_ms: None,
            capture_limit: 0,
            stream: false,
            stdout_observer: None,
            tick_observer: None,
            exec_dir: None,
            sandbox: None,
        })
        .expect("an over-budget stdin prompt bypasses the argv budget entirely");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.stdout, prompt);
    }

    #[test]
    fn spawn_failure_of_a_missing_binary_redacts_the_small_prompt_too() {
        let error = super::run(HarnessRunRequest {
            argv: vec!["ctx-does-not-exist-anywhere".to_string()],
            env_overlay: BTreeMap::new(),
            env_remove: Vec::new(),
            prompt: "small prompt body".to_string(),
            prompt_delivery: PromptDelivery::Arg,
            timeout_ms: 5_000,
            idle_timeout_ms: None,
            capture_limit: 0,
            stream: false,
            stdout_observer: None,
            tick_observer: None,
            exec_dir: None,
            sandbox: None,
        })
        .expect_err("spawning a nonexistent binary must fail");
        let message = error.to_string();
        assert!(message.contains("ctx-does-not-exist-anywhere"), "{message}");
        assert!(message.contains("2 args"), "{message}");
        assert!(!message.contains("small prompt body"), "{message}");
    }
}
