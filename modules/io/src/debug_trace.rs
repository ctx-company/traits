//! Live per-run harness invocation traces under `<run-id>/` in this
//! repository's global per-repository debug-trace root
//! (`~/.config/ctx/debug/<repo-key>/`, P426).
//!
//! One file per physical harness attempt: a flushed `{"trace":"dispatch",…}`
//! header, raw harness stdout flushed after every complete line, and a flushed
//! `{"trace":"exit",…}` footer. An interrupted drive therefore leaves useful
//! partial diagnostics instead of losing the attempt until the harness exits.
//! Traces are diagnostics, not evidence: they never feed ledgers or receipts,
//! and writes are best-effort.

use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};

use crate::harness::OutputObserver;

pub struct HarnessAttemptStart<'a> {
    /// Names the trace directory — runs are the identity users see and paste.
    pub run_id: &'a str,
    /// Recorded in the dispatch header for ledger cross-reference.
    pub session_id: &'a str,
    /// Drive-wide physical invocation counter (warm/cold fallbacks included).
    pub sequence: u64,
    pub item_id: Option<&'a str>,
    pub frame_title: &'a str,
    pub role: &'a str,
    pub harness_id: &'a str,
    /// 1-based correction attempt number within the frame.
    pub attempt: u64,
    /// Whether this physical invocation uses a persistent or one-shot process.
    pub invocation: &'a str,
    pub argv: &'a [String],
    pub prompt: &'a str,
    /// Maximum raw stdout bytes written before the trace stops accepting body data.
    pub stdout_limit: usize,
    /// P478: the generated write-confinement payload actually applied to
    /// this attempt's harness kind — never the env map itself (credentials),
    /// only the generated payload. `None` for a non-worktree attempt, a
    /// disabled confinement, or an unsupported harness kind.
    pub confinement: Option<&'a serde_json::Value>,
    /// P480: the generated OS-level (`sandbox-exec`) profile actually
    /// applied to this attempt's spawn, harness-kind-agnostic. `None` for a
    /// non-worktree attempt, `sandbox = false`, or an unsupported platform —
    /// so a missed threading site at a spawn seam is visibly `null` here
    /// rather than silently unconfined.
    pub spawn_sandbox: Option<&'a str>,
}

pub struct HarnessAttemptExit<'a> {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub idle_timed_out: bool,
    pub stdout_truncated: bool,
    pub duration_ms: u128,
    pub stderr: &'a str,
}

/// Open attempt trace whose observer can be attached to a running harness.
///
/// The observer accepts arbitrary byte chunks. Complete lines are written and
/// flushed immediately; an incomplete trailing line is retained until more
/// bytes arrive or [`Self::finish`] is called.
pub struct HarnessAttemptWriter {
    path: Utf8PathBuf,
    state: Arc<Mutex<WriterState>>,
}

struct WriterState {
    writer: Option<BufWriter<std::fs::File>>,
    pending: Vec<u8>,
    stdout_remaining: usize,
    failed: Option<String>,
    finished: bool,
}

/// `<debug-root>/<slug(run)>/<seq:03>-<slug(step)>-<slug(role)>` shared by the
/// attempt trace (`@<harness>.jsonl`) and the decision sidecar (`.decision.json`),
/// so the two files always name the same accepted invocation.
fn attempt_path_prefix(
    run_id: &str,
    sequence: u64,
    item_id: Option<&str>,
    frame_title: &str,
    role: &str,
) -> crate::Result<Utf8PathBuf> {
    let dir = crate::state::current_global_debug_root()?.join(slug(run_id));
    std::fs::create_dir_all(&dir).map_err(|source| crate::environment::Error::Filesystem {
        path: dir.to_string(),
        source,
    })?;
    let step = item_id
        .map(str::to_string)
        .unwrap_or_else(|| frame_title.to_string());
    Ok(dir.join(format!("{sequence:03}-{}-{}", slug(&step), slug(role))))
}

impl HarnessAttemptWriter {
    /// Create a trace and immediately expose its dispatch line to concurrent readers.
    pub fn start(trace: &HarnessAttemptStart<'_>) -> crate::Result<Self> {
        let prefix = attempt_path_prefix(
            trace.run_id,
            trace.sequence,
            trace.item_id,
            trace.frame_title,
            trace.role,
        )?;
        let path = Utf8PathBuf::from(format!("{prefix}@{}.jsonl", slug(trace.harness_id)));
        let file = std::fs::File::create(&path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
        })?;
        let mut writer = BufWriter::new(file);
        let header = serde_json::json!({
            "trace": "dispatch",
            "recorded-at-epoch": epoch_seconds(),
            "run-id": trace.run_id,
            "session-id": trace.session_id,
            "frame": trace.frame_title,
            "item": trace.item_id,
            "role": trace.role,
            "harness": trace.harness_id,
            "attempt": trace.attempt,
            "invocation": trace.invocation,
            "argv": trace.argv,
            "prompt": trace.prompt,
            "confinement": trace.confinement,
            "spawn-sandbox": trace.spawn_sandbox,
        });
        write_json_line(&mut writer, &header)
            .and_then(|()| writer.flush())
            .map_err(|source| crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            })?;

        Ok(Self {
            path,
            state: Arc::new(Mutex::new(WriterState {
                writer: Some(writer),
                pending: Vec::new(),
                stdout_remaining: if trace.stdout_limit == 0 {
                    crate::harness::DEFAULT_CAPTURE_LIMIT
                } else {
                    trace.stdout_limit
                },
                failed: None,
                finished: false,
            })),
        })
    }

    /// Cloneable harness stdout observer for this trace.
    pub fn stdout_observer(&self) -> OutputObserver {
        let state = Arc::clone(&self.state);
        std::sync::Arc::new(move |chunk: &[u8]| {
            let Ok(mut state) = state.lock() else {
                return;
            };
            state.push_stdout(chunk);
        })
    }

    /// Flush the trailing stdout fragment and append the terminal exit record.
    pub fn finish(&self, trace: &HarnessAttemptExit<'_>) -> crate::Result<Utf8PathBuf> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::environment::Error::Filesystem {
                path: self.path.to_string(),
                source: std::io::Error::other("debug trace writer lock poisoned"),
            })?;
        if state.finished {
            return state.result(&self.path);
        }
        state.finished = true;
        state.flush_remainder();
        if state.failed.is_none() {
            let footer = serde_json::json!({
                "trace": "exit",
                "exit-code": trace.exit_code,
                "timed-out": trace.timed_out,
                "idle-timed-out": trace.idle_timed_out,
                "stdout-truncated": trace.stdout_truncated,
                "duration-ms": trace.duration_ms as u64,
                "stderr": trace.stderr,
            });
            state.write_json(&footer);
        }
        state.result(&self.path)
    }
}

impl WriterState {
    fn push_stdout(&mut self, chunk: &[u8]) {
        if self.failed.is_some() || self.finished || self.stdout_remaining == 0 {
            return;
        }
        let accepted = chunk.len().min(self.stdout_remaining);
        self.stdout_remaining -= accepted;
        self.pending.extend_from_slice(&chunk[..accepted]);
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=newline).collect::<Vec<_>>();
            self.write_and_flush(&line);
            if self.failed.is_some() {
                return;
            }
        }
    }

    fn flush_remainder(&mut self) {
        if self.failed.is_some() || self.pending.is_empty() {
            return;
        }
        let mut remainder = std::mem::take(&mut self.pending);
        remainder.push(b'\n');
        self.write_and_flush(&remainder);
    }

    fn write_json(&mut self, value: &serde_json::Value) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if let Err(source) = write_json_line(writer, value).and_then(|()| writer.flush()) {
            self.fail(source);
        }
    }

    fn write_and_flush(&mut self, bytes: &[u8]) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if let Err(source) = writer.write_all(bytes).and_then(|()| writer.flush()) {
            self.fail(source);
        }
    }

    fn fail(&mut self, source: std::io::Error) {
        if self.failed.is_none() {
            self.failed = Some(source.to_string());
        }
        self.writer = None;
        self.pending.clear();
    }

    fn result(&self, path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
        match self.failed.as_deref() {
            Some(message) => Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::other(message.to_string()),
            }
            .into()),
            None => Ok(path.to_path_buf()),
        }
    }
}

/// Accepted, normalized decision to write alongside an accepted attempt's trace.
pub struct HarnessDecision<'a> {
    pub run_id: &'a str,
    pub sequence: u64,
    pub item_id: Option<&'a str>,
    pub frame_title: &'a str,
    pub role: &'a str,
    pub accepted_slot_values: &'a [ctx_traits_core::procedure::runtime::Value],
}

/// Best-effort sidecar naming the accepted attempt's normalized output —
/// diagnostics only, never fed back into ledgers or receipts.
pub fn write_harness_decision(decision: &HarnessDecision<'_>) -> crate::Result<Utf8PathBuf> {
    let prefix = attempt_path_prefix(
        decision.run_id,
        decision.sequence,
        decision.item_id,
        decision.frame_title,
        decision.role,
    )?;
    let path = Utf8PathBuf::from(format!("{prefix}.decision.json"));
    let file =
        std::fs::File::create(&path).map_err(|source| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        })?;
    let mut writer = BufWriter::new(file);
    let payload = serde_json::json!({
        "accepted-at": epoch_seconds(),
        "accepted-slot-values": decision.accepted_slot_values,
    });
    write_json_line(&mut writer, &payload)
        .and_then(|()| writer.flush())
        .map_err(|source| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        })?;
    Ok(path)
}

fn write_json_line(writer: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")
}

/// Filesystem-safe slug (lowercase alphanumerics joined by single dashes).
/// Shared with [`crate::state::repo_key`] so the repo-key basename component
/// and attempt-path step/role components use exactly one slugging rule.
pub(crate) fn slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    let trimmed = slug.trim_end_matches('-');
    if trimmed.is_empty() {
        "step".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Best-effort tail of the most recently modified attempt trace file under a
/// run's debug-trace directory, for the P423 dashboard's attached SESSIONS
/// pane. Diagnostics only — never ledger evidence (see the module doc):
/// returns `Ok(None)` (never an error) whenever the directory or every file
/// in it is unreadable, missing, or empty, so a best-effort tail can never
/// make an otherwise-working attached pane fail.
pub fn tail_latest_attempt_trace(run_id: &str, max_lines: usize) -> Option<Vec<String>> {
    let dir = crate::state::current_global_debug_root()
        .ok()?
        .join(slug(run_id));
    let entries = std::fs::read_dir(dir.as_std_path()).ok()?;
    let mut latest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if latest.as_ref().is_none_or(|(best, _)| modified > *best) {
            latest = Some((modified, entry.path()));
        }
    }
    let (_, path) = latest?;
    let text = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].to_vec())
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
