//! CLI harness drive loop.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::app::agent_dispatch;
use crate::app::frame_prompt::{
    RequestedSlotKey, ResolvedFramePrompt, frame_contract_section, frame_prompt, mcp_frame_prompt,
    requested_output_contract_section, requested_output_schema, requested_outputs,
    resolved_frame_prompt,
};
use crate::app::presentation::{Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::{harness_stream, run_view, surface::cli, tui};
use ctx_traits_io::debug_trace::{
    HarnessAttemptExit, HarnessAttemptStart, HarnessAttemptWriter, HarnessDecision,
    write_harness_decision,
};

const DEFAULT_MAX_FRAMES: u64 = 100;
/// Built-in default per-frame wall-clock budget when nothing in the P475 D2
/// chain (CLI flag > `[run]` > package sidecar > role budget) declares one.
/// `pub(crate)` so `doctor --config` (P475 D7) can display it as the
/// resolved value for a role with no declared `budget.frame-seconds`.
pub(crate) const DEFAULT_FRAME_SECONDS: u64 = 300;
const DEFAULT_TOTAL_SECONDS: u64 = 1800;
/// Built-in default per-frame retry ceiling — see [`DEFAULT_FRAME_SECONDS`].
pub(crate) const DEFAULT_MAX_RETRIES: u64 = 1;
// Narration is async garnish and its call rate is already bounded by the
// worker's pacing floor, so a taller ceiling costs nothing when healthy — but
// a ceiling below a slow model's turn time kills paid turns mid-generation
// (observed: 10-16s deepseek thinking turns against the old 12s budget).
// P475: this is now only the built-in default for the `narrator` seat's
// `budget.frame-seconds` — see [`narrator_timeout_ms`] — overridable via
// `[agent.role.narrator.budget]` with zero code changes.
pub(crate) const DEFAULT_NARRATOR_TIMEOUT_MS: u64 = 20_000;
// P455: bounded grace window `drive()` gives a just-enqueued terminal
// step-summary call before taking its final token snapshot — comfortably
// above every observed successful call (single-digit-to-tens of
// milliseconds) without meaningfully delaying exit on a healthy run, and far
// below the narrator's resolved timeout so a hung call still reports
// incomplete.
const NARRATOR_SETTLE_GRACE_MS: u64 = 1_000;

/// The `narrator` seat's resolved one-shot call budget (P475, D3): its own
/// `[agent.role.narrator].budget.frame-seconds` if declared, else
/// [`DEFAULT_NARRATOR_TIMEOUT_MS`]. Deliberately resolved from the seat's
/// own budget alone — never joined to the run-level `[run]`/CLI-flag chain
/// [`frame_budget`] uses for frame dispatch — since the narrator call is a
/// one-shot dispatch outside the drive frame loop, and a `[run]
/// frame-seconds` declared for the drive's own frames must never silently
/// cut narration too.
fn one_shot_timeout_ms(
    profile: &ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    seat: &str,
    default_ms: u64,
) -> u64 {
    match profile.budget_for_seat(seat, None).frame_seconds {
        Some(seconds) => one_shot_timeout_from_seconds(seconds),
        None => default_ms,
    }
}

fn one_shot_timeout_from_seconds(seconds: u64) -> u64 {
    seconds.saturating_mul(1000)
}

fn narrator_timeout_ms(profile: &ctx_traits_io::harness_config::ResolvedRuntimeAssignments) -> u64 {
    one_shot_timeout_ms(profile, "narrator", DEFAULT_NARRATOR_TIMEOUT_MS)
}

#[cfg(test)]
#[test]
fn guide_timeout_uses_guide_default_and_saturates_seconds() {
    // The two standing seats deliberately use distinct lookup names. A
    // configured guide budget must therefore not be mistaken for narrator's.
    assert_eq!(DEFAULT_NARRATOR_TIMEOUT_MS, 20_000);
    let guide_seconds = 7;
    let narrator_seconds = 11;
    assert_eq!(one_shot_timeout_from_seconds(guide_seconds), 7_000);
    assert_eq!(one_shot_timeout_from_seconds(narrator_seconds), 11_000);
    assert_ne!(
        one_shot_timeout_from_seconds(guide_seconds),
        one_shot_timeout_from_seconds(narrator_seconds)
    );
    // An undeclared guide budget is intentionally narrator-sized, not tied to
    // the run frame budget.
    assert_eq!(
        None::<u64>
            .map(one_shot_timeout_from_seconds)
            .unwrap_or(DEFAULT_NARRATOR_TIMEOUT_MS),
        DEFAULT_NARRATOR_TIMEOUT_MS
    );
    assert_eq!(one_shot_timeout_from_seconds(7), 7_000);
    assert_eq!(one_shot_timeout_from_seconds(u64::MAX), u64::MAX);
}

/// A concurrent wave's outcomes (P344 `--max-in-flight`), keyed by absolute
/// branch offset within the wave. Every offset dispatched into the wave gets
/// exactly one entry, success or failure alike (IO error or worker-thread
/// panic) — a branch's turn consumes and propagates whichever it holds
/// rather than silently redispatching a lost failure as an implicit retry.
type WaveOutcomes = BTreeMap<usize, crate::Result<ctx_traits_io::harness::HarnessRunOutcome>>;
/// Speculatively dispatched wave outcomes not yet consumed by the sequential
/// cursor, keyed by the owning `parallel` activation (see
/// `parallel_wave_activation_key`).
type PendingWaveCache = BTreeMap<String, WaveOutcomes>;

pub struct DriveInputs<'a> {
    pub file: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    pub assignments: &'a [String],
    pub max_frames: Option<u64>,
    pub frame_seconds: Option<u64>,
    pub total_seconds: Option<u64>,
    pub max_retries: Option<u64>,
    pub attach_wait_seconds: Option<u64>,
    pub idle_seconds: Option<u64>,
    /// Bounded concurrency for independent `parallel`-panel branches (P344).
    /// `1` (the default) is a hard no-op: no branch is ever peeked or
    /// dispatched ahead of the single sequential cursor, so drive's
    /// behavior and output are byte-identical to before this flag existed.
    /// Values above `1` opt into concurrently dispatching up to that many
    /// eligible sibling branches' harness calls at once (see
    /// `attempt_concurrent_wave`); every ledger write still applies
    /// strictly through the same sequential path, in authored order.
    pub max_in_flight: usize,
    /// P402: when this drive needs the per-session conductor lease (either
    /// because `max_in_flight > 1`, or because durable concurrent state
    /// already exists for this session from a prior conductor) and another
    /// process currently holds it, `wait = true` polls for the lease within
    /// the existing `total_seconds` budget instead of returning the typed
    /// busy outcome immediately. A session that has never used concurrency
    /// creates no lease/sidecars and never blocks on this at all.
    pub wait: bool,
    pub progress: cli::DriveProgress,
    /// Raw `--worktree[=<name>]` flag for a standalone drive: `None` (absent),
    /// `Some(None)` (bare flag, derive an id from the session id), or
    /// `Some(Some(name))` (explicit id). Resolved into `execution_dir` by
    /// `drive()` before the drive loop starts. Left `None` when the caller
    /// (e.g. `session start`) already prepared the worktree itself.
    pub worktree: Option<Option<&'a str>>,
    /// Already-prepared worktree execution directory. Set directly by
    /// `session start` (which prepared it during `run::start`) or by `drive()`
    /// itself after resolving `worktree` for a standalone drive.
    pub execution_dir: Option<&'a camino::Utf8Path>,
    /// P460 `drive --no-merge`: clear a persisted automatic-landing merge
    /// intent before resuming. Applied only once this invocation has
    /// actually acquired the per-session driver lock below (P460 review —
    /// an invocation that loses the lock race must never mutate the ledger
    /// a concurrent driver already holds), never before.
    pub clear_merge_intent: bool,
    /// P549: when set and this drive reaches its NORMAL completed exit (the
    /// single `Status::Completed` arm in `drive_loop`, never an early
    /// stop/interrupt/failure return), the live [`run_view::RunPanel`] — if
    /// one was created (`--progress tui` on a real terminal) — is released
    /// into this handoff instead of being closed by `RunPanelGuard`'s drop.
    /// The caller (`complete_after_drive`'s callers) takes it back out and
    /// re-wraps it in its own drop-close guard for the merge span, so the
    /// panel is never owned by anything but a guard at any hop (the
    /// 2026-07-22 terminal-restore incident discipline). `None` for every
    /// caller that hasn't opted in (e.g. `dashboard`'s `d`rive action) —
    /// byte-identical to today's unconditional close.
    pub panel_handoff: Option<PanelHandoff>,
    /// A startup pane created before session initialization. It is consumed by
    /// the first live frame rather than allocating a second terminal owner.
    pub startup: Option<crate::app::run_startup_view::StartupView>,
}

/// Cheap, cloneable one-shot slot a completed drive's live pane is handed
/// off through — see [`DriveInputs::panel_handoff`]. `Arc<Mutex<..>>` rather
/// than a channel: at most one handoff ever happens per drive, and the
/// receiving side (a synchronous caller sequenced right after `drive()`
/// returns) never blocks waiting for it.
#[derive(Clone, Default)]
pub(crate) struct PanelHandoff(std::sync::Arc<std::sync::Mutex<Option<run_view::RunPanel>>>);

impl std::fmt::Debug for PanelHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PanelHandoff(..)")
    }
}

impl PanelHandoff {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn give(&self, panel: run_view::RunPanel) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(panel);
        }
    }

    /// Takes the handed-off panel, if a completed drive released one.
    pub(crate) fn take(&self) -> Option<run_view::RunPanel> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// The one place presentation mode is resolved before a drive loop (and
/// therefore any [`run_view::RunPanel`] / `RatatuiPane`) starts: `--json`
/// and an explicit `--no-tui` both always win over `--progress tui`, so no
/// alternate-screen/control-frame byte can ever land in a structured or
/// explicitly-disabled-TUI invocation. When `--progress` is absent (P474)
/// this also picks the mode: `tui` on a fully interactive terminal, else
/// `status`, byte-identical to the pre-P474 default. Call-sites resolve
/// through this instead of adding their own `progress == Tui` checks or TTY
/// probes.
pub fn resolve_progress(
    progress: Option<cli::DriveProgress>,
    json: bool,
    no_tui: bool,
) -> cli::DriveProgress {
    let interactive =
        crate::app::dashboard::interactive_available() && std::io::stdout().is_terminal();
    resolve_progress_with(progress, json, no_tui, interactive)
}

/// Pure sibling of [`resolve_progress`], taking the TTY probe's answer as a
/// plain `bool` so precedence is unit-testable without a real terminal.
fn resolve_progress_with(
    progress: Option<cli::DriveProgress>,
    json: bool,
    no_tui: bool,
    interactive: bool,
) -> cli::DriveProgress {
    match progress {
        Some(cli::DriveProgress::Tui) if json || no_tui => cli::DriveProgress::Status,
        Some(mode) => mode,
        None if json || no_tui => cli::DriveProgress::Status,
        None if interactive => cli::DriveProgress::Tui,
        None => cli::DriveProgress::Status,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DriveReport {
    pub status: String,
    pub session: String,
    pub frames_attempted: u64,
    pub frames_accepted: u64,
    pub warnings: Vec<String>,
    pub capabilities: Vec<ctx_traits_core::response::CapabilityReport>,
    pub events: Vec<DriveEvent>,
    pub final_session_status: Option<ctx_traits_core::procedure::session::Status>,
    /// Normalized projection of `final_session_status`, retained beside the
    /// legacy status string for existing automation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_state: Option<ctx_traits_core::procedure::activity::SessionState>,
    /// Drive-local normalized observations, unbounded and uncoalesced —
    /// this in-memory snapshot is separate from the durable, coalesced
    /// activity sidecar `ctx_traits_io::activity_sidecar` persists
    /// alongside the ledger (P521), which `ctx traits story` reads back.
    #[serde(default)]
    pub activity: Vec<ctx_traits_core::procedure::activity::ActivityEvent>,
    /// Present only when `status` is `paused-provider-credits`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits_pause: Option<ctx_traits_core::procedure::runtime::ProviderCreditsPause>,
    /// P460: present only when a persisted merge intent made this drive
    /// eligible for automatic landing. Set by the caller (`ctx traits
    /// drive`'s command handler) after `drive()` returns, never by
    /// `drive_loop` itself — absent (and omitted from JSON) for every drive
    /// with no merge intent, so output stays byte-identical to before P460.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge: Option<crate::app::merge::MergeReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DriveEvent {
    pub event: String,
    pub role: Option<String>,
    pub harness: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
}

#[derive(Debug, Clone)]
struct Budget {
    max_frames: u64,
    frame_seconds: u64,
    total_seconds: u64,
    max_retries: u64,
    attach_wait_seconds: u64,
    idle_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
struct AssignmentPlan {
    harness_id: String,
    transport: ctx_traits_io::harness_config::RunTransport,
    mode: ctx_traits_io::harness_config::RunAssignmentMode,
    session_mode: ctx_traits_io::harness_config::RunSessionMode,
    model: Option<String>,
    reasoning_effort: Option<String>,
    system_prompt: Option<String>,
    extra_args: Vec<String>,
    model_resolution_evidence: Option<String>,
    from_session: bool,
    /// 1-based seat this plan was resolved for, and the role's configured
    /// list length, present only for a list-backed role (P456). Folded into
    /// the persistent-session and model-resolution keys so two seats of the
    /// same role never share a warm conversation or dedupe each other's
    /// evidence.
    seat_index: Option<u32>,
    list_length: Option<u32>,
}

#[derive(Debug, Clone)]
struct ParsedHarnessOutput {
    slots: BTreeMap<String, Value>,
    harness_session_id: Option<String>,
    /// Top-level keys of model-authored JSON objects that were inspected but
    /// did not satisfy the requested slots — so a correction can say what the
    /// model sent instead of only what was expected.
    observed_keys: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct DriveProbe {
    version: String,
    supported: bool,
}

enum WarmOutcomeFailure {
    Counted { reason: String },
    ImmediateFallback { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarmPromptKind {
    Frame,
    Narrator,
}

/// Drive-wide accumulator for output tokens observed across the work agent's
/// harness attempts (P445): cold, warm, retries, and concurrent-wave
/// dispatch alike. Counting happens per-attempt (see `WorkTokenCounterHandle`
/// at each of `begin_harness_trace`'s call sites, the only shared
/// preparation/finalization points every one of those paths goes through),
/// never depending on whether a live TUI/stream panel exists — this is why
/// accounting is independent of `LiveHarnessOutput` below, which is
/// presentation-only.
#[derive(Clone, Default)]
struct WorkTokenTotal(Arc<AtomicU64>);

impl WorkTokenTotal {
    fn add(&self, delta: u64) {
        if delta == 0 {
            return;
        }
        self.0.fetch_add(delta, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// One attempt's output-token counter (P445), created fresh per harness
/// dispatch by `begin_harness_trace`/`begin_cold_dispatch` regardless of
/// whether a debug trace or a live panel is active, so every attempt
/// contributes exactly once to the drive-wide `WorkTokenTotal` and (when a
/// `--progress tui` panel is live) to that panel's existing per-step token
/// display — the same shared `AttemptTokenAccumulator` (`ctx_traits_io::harness`,
/// also used by the narrator path in `harness_stream.rs`) feeds both, never
/// two counters parsing the same stream.
#[derive(Clone)]
struct WorkTokenCounterHandle {
    accumulator: ctx_traits_io::harness::AttemptTokenAccumulator,
}

/// Bound (in characters) on the text a coalesced `Thinking`/`StreamingOutput`
/// sidecar record carries — same order of magnitude as
/// `harness_activity::MAX_TEXT_CHARS` per individual delta, but this is a
/// per-frame *total* across however many deltas coalesced into it.
const COALESCE_TEXT_BOUND: usize = 2048;

/// The drive-local activity collector (P504). Its in-memory `events`
/// (`snapshot()`, feeding `DriveReport.activity`) are unbounded and
/// uncoalesced, unchanged from before P521. Durable persistence (P521) is a
/// separate, optional concern: when a sink is attached, every event is also
/// appended to the ledger's activity sidecar, with consecutive same-kind
/// `Thinking`/`StreamingOutput` events for the same frame coalesced into one
/// persisted record at the append boundary — raw per-delta persistence would
/// blow the sidecar's per-event size budget by orders of magnitude.
/// `RunningTool`/`Dispatching`/`Retrying`/`Compacting`/etc. persist 1:1.
#[derive(Clone, Default)]
struct ActivityRecorder {
    inner: Arc<std::sync::Mutex<ActivityRecorderState>>,
}

#[derive(Default)]
struct ActivityRecorderState {
    sequence: u64,
    events: Vec<ctx_traits_core::procedure::activity::ActivityEvent>,
    sink: Option<ctx_traits_io::activity_sidecar::ActivitySidecarWriter>,
    pending: Option<PendingCoalesce>,
}

struct PendingCoalesce {
    frame_id: String,
    kind: ctx_traits_core::procedure::activity::ActivityKind,
    text: String,
    char_total: usize,
    tokens_total: u64,
}

fn coalesces(kind: &ctx_traits_core::procedure::activity::ActivityKind) -> bool {
    matches!(
        kind,
        ctx_traits_core::procedure::activity::ActivityKind::Thinking
            | ctx_traits_core::procedure::activity::ActivityKind::StreamingOutput
    )
}

impl ActivityRecorder {
    fn emit(
        &self,
        frame_id: impl Into<String>,
        kind: ctx_traits_core::procedure::activity::ActivityKind,
    ) {
        self.record(ctx_traits_core::procedure::activity::ActivityEvent {
            sequence: 0,
            frame_id: frame_id.into(),
            kind,
            text: None,
            tool: None,
            tokens: None,
        });
    }

    /// Emit a `Retrying` event with a `attempt N of M: <reason>` text — the
    /// kind has existed since P504 but was never emitted before P521.
    fn emit_retry(&self, frame_id: impl Into<String>, attempt: u64, max: u64, reason: &str) {
        self.record(ctx_traits_core::procedure::activity::ActivityEvent {
            sequence: 0,
            frame_id: frame_id.into(),
            kind: ctx_traits_core::procedure::activity::ActivityKind::Retrying,
            text: Some(format!("attempt {attempt} of {max}: {reason}")),
            tool: None,
            tokens: None,
        });
    }

    /// Attach the durable sidecar sink this drive appends to. Called at most
    /// once, before any frame dispatch.
    fn attach_sink(&self, sink: ctx_traits_io::activity_sidecar::ActivitySidecarWriter) {
        if let Ok(mut state) = self.inner.lock() {
            state.sink = Some(sink);
        }
    }

    /// Persist this drive's finished-step summary independent of whether a
    /// TUI panel exists to show it live (P521's summary-line resolution
    /// order needs it recorded regardless).
    fn record_step_summary(&self, key: impl Into<String>, role: impl Into<String>, text: &str) {
        if let Ok(mut state) = self.inner.lock()
            && let Some(sink) = state.sink.as_mut()
        {
            sink.append_step_summary(key.into(), role.into(), text.to_string());
        }
    }

    /// Flush any pending coalesced record for `frame_id` — called at frame
    /// completion so a frame's last thinking/output burst is never left
    /// unwritten until a later, unrelated frame's first event happens to
    /// trigger the flush.
    fn finish_frame(&self, frame_id: &str) {
        if let Ok(mut state) = self.inner.lock()
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.frame_id == frame_id)
        {
            flush_pending(&mut state);
        }
    }

    fn observer(
        &self,
        kind: ctx_traits_io::harness_activity::HarnessActivityAdapterKind,
        frame_id: String,
    ) -> ctx_traits_io::harness::OutputObserver {
        let adapter = Arc::new(std::sync::Mutex::new(
            ctx_traits_io::harness_activity::HarnessActivityAdapter::new(kind, frame_id),
        ));
        let recorder = self.clone();
        Arc::new(move |chunk| {
            let Ok(mut adapter) = adapter.lock() else {
                return;
            };
            for event in adapter.push(chunk) {
                recorder.record(event);
            }
        })
    }

    fn record(&self, mut event: ctx_traits_core::procedure::activity::ActivityEvent) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        state.sequence += 1;
        event.sequence = state.sequence;
        state.events.push(event.clone());
        if state.sink.is_none() {
            return;
        }
        if coalesces(&event.kind) {
            let matches_pending = state.pending.as_ref().is_some_and(|pending| {
                pending.frame_id == event.frame_id && pending.kind == event.kind
            });
            if !matches_pending {
                flush_pending(&mut state);
                state.pending = Some(PendingCoalesce {
                    frame_id: event.frame_id.clone(),
                    kind: event.kind.clone(),
                    text: String::new(),
                    char_total: 0,
                    tokens_total: 0,
                });
            }
            if let Some(pending) = state.pending.as_mut() {
                if let Some(text) = &event.text {
                    pending.char_total += text.chars().count();
                    let room = COALESCE_TEXT_BOUND.saturating_sub(pending.text.chars().count());
                    if room > 0 {
                        pending.text.extend(text.chars().take(room));
                    }
                }
                pending.tokens_total += event.tokens.unwrap_or(0);
            }
        } else {
            flush_pending(&mut state);
            if let Some(sink) = state.sink.as_mut() {
                sink.append_activity(event);
            }
        }
    }

    fn snapshot(&self) -> Vec<ctx_traits_core::procedure::activity::ActivityEvent> {
        self.inner
            .lock()
            .map(|state| state.events.clone())
            .unwrap_or_default()
    }
}

fn flush_pending(state: &mut ActivityRecorderState) {
    let Some(pending) = state.pending.take() else {
        return;
    };
    let Some(sink) = state.sink.as_mut() else {
        return;
    };
    let text = if pending.char_total > pending.text.chars().count() {
        format!(
            "{} … ({} chars total, ~{} tokens)",
            pending.text, pending.char_total, pending.tokens_total
        )
    } else {
        pending.text
    };
    sink.append_activity(ctx_traits_core::procedure::activity::ActivityEvent {
        sequence: 0,
        frame_id: pending.frame_id,
        kind: pending.kind,
        text: (!text.is_empty()).then_some(text),
        tool: None,
        tokens: (pending.tokens_total > 0).then_some(pending.tokens_total),
    });
}

#[cfg(test)]
mod activity_recorder_tests {
    use super::*;
    use ctx_traits_core::procedure::activity::ActivityKind;

    fn scratch_ledger_path(name: &str) -> camino::Utf8PathBuf {
        let dir = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-activity-recorder-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir.join("session-fixture.json")
    }

    #[test]
    fn coalesces_consecutive_thinking_deltas_into_one_sidecar_record() {
        let ledger_path = scratch_ledger_path("coalesce");
        let recorder = ActivityRecorder::default();
        recorder.attach_sink(
            ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(&ledger_path),
        );
        for i in 0..5 {
            recorder.record(ctx_traits_core::procedure::activity::ActivityEvent {
                sequence: 0,
                frame_id: "frame-a".to_string(),
                kind: ActivityKind::Thinking,
                text: Some(format!("chunk-{i}")),
                tool: None,
                tokens: Some(2),
            });
        }
        recorder.finish_frame("frame-a");
        let (records, skipped) = ctx_traits_io::activity_sidecar::read_activity(&ledger_path);
        assert_eq!(skipped, 0);
        assert_eq!(
            records.len(),
            1,
            "5 thinking deltas must coalesce to 1 record"
        );
        let ctx_traits_io::activity_sidecar::ActivityRecord::Activity { event, .. } = &records[0]
        else {
            panic!("expected an activity record");
        };
        assert_eq!(
            event.tokens,
            Some(10),
            "tokens must sum across coalesced deltas"
        );
    }

    #[test]
    fn non_coalescing_kinds_persist_one_to_one() {
        let ledger_path = scratch_ledger_path("no-coalesce");
        let recorder = ActivityRecorder::default();
        recorder.attach_sink(
            ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(&ledger_path),
        );
        recorder.emit("frame-a", ActivityKind::Dispatching);
        recorder.emit("frame-a", ActivityKind::Dispatching);
        let (records, _) = ctx_traits_io::activity_sidecar::read_activity(&ledger_path);
        assert_eq!(records.len(), 2);
    }
}

impl WorkTokenCounterHandle {
    fn new(total: WorkTokenTotal, panel: Option<run_view::RunPanel>) -> Self {
        Self {
            accumulator: ctx_traits_io::harness::AttemptTokenAccumulator::new(move |delta| {
                total.add(delta);
                if let Some(panel) = &panel {
                    panel.add_output_tokens(delta);
                }
            }),
        }
    }

    fn observer(&self) -> ctx_traits_io::harness::OutputObserver {
        self.accumulator.observer()
    }

    /// Flush the counter's final, unterminated line once this attempt's
    /// stdout has closed. Idempotent to call at most once per attempt.
    fn flush(&self) {
        self.accumulator.finish();
    }
}

enum LiveHarnessOutput {
    Stream {
        panel: tui::LiveOutputPanel,
        narrator: Option<harness_stream::StreamNarrator>,
    },
    Run {
        panel: run_view::RunPanel,
        narrator: Option<harness_stream::StreamNarrator>,
    },
}

#[derive(Clone)]
enum LivePanelSink {
    Stream(tui::LiveOutputPanel),
    Run(run_view::RunPanel),
}

impl LivePanelSink {
    fn push_bytes(&self, chunk: &[u8]) {
        match self {
            Self::Stream(panel) => panel.push_bytes(chunk),
            Self::Run(panel) => panel.push_bytes(chunk),
        }
    }
}

impl LiveHarnessOutput {
    fn passthrough(panel: tui::LiveOutputPanel) -> Self {
        Self::Stream {
            panel,
            narrator: None,
        }
    }

    fn narrated(panel: tui::LiveOutputPanel, narrator: harness_stream::StreamNarrator) -> Self {
        Self::Stream {
            panel,
            narrator: Some(narrator),
        }
    }

    fn run_passthrough(panel: run_view::RunPanel) -> Self {
        Self::Run {
            panel,
            narrator: None,
        }
    }

    fn run_narrated(panel: run_view::RunPanel, narrator: harness_stream::StreamNarrator) -> Self {
        Self::Run {
            panel,
            narrator: Some(narrator),
        }
    }

    fn observer(&self) -> ctx_traits_io::harness::OutputObserver {
        let (panel, narrator) = match self {
            Self::Stream { panel, narrator } => (LivePanelSink::Stream(panel.clone()), narrator),
            Self::Run { panel, narrator } => (LivePanelSink::Run(panel.clone()), narrator),
        };
        let feeder = narrator
            .as_ref()
            .map(harness_stream::StreamNarrator::feeder);
        std::sync::Arc::new(move |chunk: &[u8]| {
            // A narrated panel shows only compacted narration, never raw
            // harness text: feed the narrator instead of the passthrough
            // floor. A narrator-free panel keeps raw passthrough.
            match &feeder {
                Some(feeder) => feeder.feed(chunk),
                None => panel.push_bytes(chunk),
            }
        })
    }

    fn tick_observer(&self) -> Option<ctx_traits_io::harness::TickObserver> {
        match self {
            Self::Stream { .. } => None,
            Self::Run { panel, .. } => {
                let panel = panel.clone();
                Some(std::sync::Arc::new(move || panel.tick()))
            }
        }
    }

    /// P455 accepted-frame finalization, shared by CLI/MCP dispatch and a
    /// trailing command failure after an accepted submission: settle the old
    /// step's live completion text first, refresh the panel with the
    /// just-accepted `session` second (the panel's sole active-key
    /// transition detector), then either request one narrator step-summary
    /// call for a real transition or fall back to a plain finish. `--progress
    /// stream` has no `RunPanel` transition to summarize, so it always plain-
    /// finishes.
    fn finish_accepted(
        self,
        summary: &str,
        session: &ctx_traits_core::procedure::session::Session,
    ) {
        match self {
            Self::Stream { panel, narrator } => {
                if let Some(narrator) = narrator {
                    narrator.finish();
                }
                panel.finish(summary);
            }
            Self::Run { panel, narrator } => {
                panel.finish_live(summary);
                let completed = panel.refresh(session);
                match (narrator, completed) {
                    (Some(narrator), Some(context)) => {
                        narrator.finish_with_step_summary(harness_stream::StepSummaryContext {
                            key: context.key,
                            label: context.label,
                            role: context.role,
                            elapsed: context.elapsed,
                            work_tokens: context.work_tokens,
                        });
                    }
                    (Some(narrator), None) => narrator.finish(),
                    (None, _) => {}
                }
            }
        }
    }
}

/// Split into its own function purely so the two IO calls, each with a
/// large `ctx_traits_io::Error` case, compose via `?` instead of a clippy
/// `result_large_err`-tripping `Result::and_then` closure.
fn current_repo_key_and_path() -> crate::Result<(String, String)> {
    let key = ctx_traits_io::state::current_repo_key()?;
    let path = ctx_traits_io::state::canonical_repo_root(
        ctx_traits_io::state::discover_invocation_root()?.path(),
    )?;
    Ok((key, path.to_string()))
}

pub fn drive(input: DriveInputs<'_>) -> crate::Result<DriveReport> {
    // Cooperative SIGINT is the only interruption path this drive loop makes
    // any guarantee about (see `crate::app::interrupt`). `reset` then
    // `install` are both idempotent/cheap: a long-lived host process (a
    // future daemon mode, or a test harness driving more than one session in
    // one process) must not carry a stale interrupt from a prior drive into
    // this one.
    // A Ctrl-C received while the startup pane owned the terminal must not be
    // cleared when the live drive begins.
    if input
        .startup
        .as_ref()
        .is_some_and(crate::app::run_startup_view::StartupView::interrupted)
    {
        return Err(crate::Error::Command {
            message: "run startup interrupted".to_string(),
        });
    }
    crate::app::interrupt::reset();
    crate::app::interrupt::install();
    let session = input.session;
    let session_store = input.session_store;
    let mut input = input;
    // ONE deadline for this whole invocation, computed here before anything
    // else (lease wait, then execution) spends any of it — see P402
    // conductor-wait-bypasses-drive-deadline: lease polling and the
    // execution loop must share the SAME remaining budget, never two
    // independent timers, or a drive could wait almost the full
    // `total-seconds` for the lease and then get a fresh full budget for
    // execution afterward (up to ~2x the configured total).
    let drive_started = Instant::now();
    // P402 `conductor-wait-bypasses-drive-deadline`: resolve the effective
    // drive budget ONCE, here, through the authoritative profile path — the
    // SAME resolution `drive_loop` uses (there is no second best-effort
    // hint anymore). Both the conductor-lease wait and the execution loop are
    // handed this identical budget and share the single `drive_started`
    // anchor, so a waiter that acquires the lease near the deadline receives
    // only the actual remaining budget, never a fresh full one.
    let mut profile = resolve_drive_profile(&input).inspect_err(|error| {
        if let Some(startup) = input.startup.as_ref() {
            startup.fail(error.to_string());
        }
    })?;
    let budget = budget_from(&profile.budget, &input);
    // P402 conductor lease: held for the entire remaining scope of this
    // function (both the happy path and every early return below), so no
    // other process's drive can start writing this session's parent ledger
    // or sidecars while this one is active. `_conductor_lease` is `None`
    // when this session has never used concurrency (no lease/sidecars ever
    // created) — see `acquire_conductor_lease_if_needed`.
    let _conductor_lease = match acquire_conductor_lease_if_needed(&input, drive_started, &budget)
        .inspect_err(|error| {
        if let Some(startup) = input.startup.as_ref() {
            startup.fail(error.to_string());
        }
    })? {
        ConductorLeaseOutcome::NotNeeded => None,
        ConductorLeaseOutcome::Acquired(file) => Some(file),
        ConductorLeaseOutcome::Busy(busy_report) => return Ok(*busy_report),
        ConductorLeaseOutcome::Interrupted(interrupted_report) => {
            return Ok(*interrupted_report);
        }
    };
    // P551: paint the full pending procedure before worktree prep/setup
    // starts, not after. The session ledger already exists at this point (a
    // fresh session was created by the caller before `drive()` runs) and
    // `create_run_panel` only needs the ledger plus the invocation
    // checkout's trait — nothing worktree prep produces — so the panel can
    // be built here and reused for the rest of the invocation instead of
    // `drive_loop` constructing it lazily on the first frame. Placed AFTER
    // the conductor-lease wait (so a genuinely busy/refused drive never
    // flashes a pane) but BEFORE the worktree block. `run_panel` is a
    // `RunPanelGuard` from here on so every early return between here and
    // `drive_loop` — worktree conflicts, driver-lock-busy — closes it
    // deterministically instead of leaking raw mode/alt-screen state.
    let early_session =
        ctx_traits_io::run::read_session(session, session_store).inspect_err(|error| {
            if let Some(startup) = input.startup.as_ref() {
                startup.fail(error.to_string());
            }
        })?;
    let mut run_panel = RunPanelGuard(None);
    if input.progress == cli::DriveProgress::Tui {
        match create_run_panel(&mut input, &early_session) {
            Ok(panel) => run_panel.0 = Some(panel),
            Err(err) => {
                // No live panel will take ownership. Commit the failed startup
                // rows before the status fallback writes ordinary stderr.
                if let Some(startup) = input.startup.take() {
                    startup.fail(err.to_string());
                }
                input.progress = cli::DriveProgress::Status;
                eprintln!("run tui unavailable; falling back to status progress: {err}");
            }
        }
    }
    let restored_worktree = resolve_resume_worktree(&input).inspect_err(|error| {
        if let Some(startup) = input.startup.as_ref() {
            startup.fail(error.to_string());
        }
    })?;
    if let Some((path, _, recorded_id)) = restored_worktree.as_ref() {
        if let Some(requested) = input.worktree {
            let requested_id = requested.map(ToString::to_string).unwrap_or(
                ctx_traits_io::worktree::derive_worktree_id(&worktree_session_id(&input)?),
            );
            if &requested_id != recorded_id {
                let message = format!(
                    "requested worktree {requested_id:?} conflicts with recorded worktree {recorded_id:?}"
                );
                if let Some(startup) = input.startup.as_ref() {
                    startup.fail(message.clone());
                }
                return Err(crate::Error::Command { message });
            }
        }
        if input
            .execution_dir
            .is_some_and(|requested| requested != path)
        {
            let message = format!(
                "execution directory {} conflicts with recorded worktree path {}",
                crate::app::presentation::optional(input.execution_dir),
                path
            );
            if let Some(startup) = input.startup.as_ref() {
                startup.fail(message.clone());
            }
            return Err(crate::Error::Command { message });
        }
    }
    let prepared_worktree = match (restored_worktree.as_ref(), input.worktree) {
        (None, Some(requested)) => Some(
            prepare_standalone_worktree(&input, requested, run_panel.0.as_ref()).inspect_err(
                |error| {
                    if let Some(startup) = input.startup.as_ref() {
                        startup.fail(error.to_string());
                    }
                },
            )?,
        ),
        _ => None,
    };
    let worktree_retry_warnings = prepared_worktree
        .as_ref()
        .map(|prepared| prepared.retry_warnings.clone())
        .unwrap_or_default();
    if let Some(prepared) = prepared_worktree.as_ref() {
        input.execution_dir = Some(prepared.path.as_path());
    }
    if let Some((path, _, _)) = restored_worktree.as_ref() {
        input.execution_dir = Some(path.as_path());
    }
    let retention_worktree = input.execution_dir.map(camino::Utf8PathBuf::from);
    let resume_retry_warnings = restored_worktree
        .as_ref()
        .map(|(_, warnings, _)| warnings.clone())
        .unwrap_or_default();
    // P423 driver-lock-does-not-cover-ledger-writes: the driver lock is
    // acquired here, BEFORE `drive_loop` starts, and held through the
    // `record_drive_outcome` write below — covering the complete
    // ledger-writing transaction, not just the execution loop. A contender
    // that fails to acquire (`Busy`) returns immediately, WITHOUT calling
    // `record_drive_outcome`: it never held the lock, so it must never
    // mutate a ledger the actual lock holder is concurrently writing to.
    let ledger_path = ctx_traits_io::run_session::resolve_session_path(session, session_store)?;
    // P479: constructed here (once), never inside `drive_loop`, so every one
    // of that loop's early-return exits is still covered by the terminal
    // sweep below — see D1's two-call-site design. `None` for a run with no
    // worktree in play (the tripwire is meaningless without an invocation
    // repository distinct from the execution directory).
    let mut tripwire = match input.execution_dir {
        Some(worktree_root) => {
            let main_root = ctx_traits_io::repository::discover_main_repo_root(worktree_root)?;
            let sentinel_paths = ctx_traits_io::tripwire::resolve_sentinel_paths(
                &main_root,
                &profile.worktree.tripwire.sentinel,
            )?;
            Some(ctx_traits_io::tripwire::Tripwire::new(
                main_root,
                sentinel_paths,
                profile.worktree.tripwire.policy,
            ))
        }
        None => None,
    };
    // P551: reuses the session read hoisted above the worktree block for the
    // early panel — nothing between there and here mutates the ledger, so a
    // second read would just be a redundant disk hit.
    let session_for_lock = early_session;
    // Best-effort repository identity for the liveness-index row —
    // `LiveRunFacts` is display/orphan-diagnosis evidence only, so an
    // identity-resolution failure (e.g. no HOME) degrades to an empty
    // key/path rather than failing the drive itself.
    let (repo_key, repo_path) = current_repo_key_and_path().unwrap_or_default();
    let live_facts = ctx_traits_io::run_liveness::LiveRunFacts {
        session_id: session.to_string(),
        run_id: session_for_lock.run_id.as_str().to_string(),
        repo_key,
        repo_path,
        ledger_path: ledger_path.clone(),
        worktree_path: input.execution_dir.map(|dir| dir.to_string()).or_else(|| {
            session_for_lock
                .provenance
                .worktree
                .as_ref()
                .and_then(|w| w.path.clone())
        }),
        branch: session_for_lock
            .provenance
            .worktree
            .as_ref()
            .map(|w| w.branch.clone()),
        log_path: std::env::var(ctx_traits_io::run_liveness::SPAWNED_LOG_PATH_ENV).ok(),
    };
    let driver_lock = ctx_traits_io::run_control::try_acquire(
        &live_facts,
        std::sync::Arc::new(crate::app::interrupt::request_stop),
    )?;
    let Some(driver_lock) = driver_lock else {
        let mut report = DriveReport {
            status: "driver-lock-busy".to_string(),
            session: session.to_string(),
            frames_attempted: 0,
            frames_accepted: 0,
            warnings: vec![format!(
                "another process already holds the driver lock for {ledger_path}; retry once it releases"
            )],
            capabilities: Vec::new(),
            events: Vec::new(),
            final_session_status: None,
            session_state: Some(ctx_traits_core::procedure::activity::SessionState::Running),
            activity: Vec::new(),
            credits_pause: None,
            merge: None,
        };
        push_capability(
            &mut report,
            ctx_traits_core::response::CapabilityReport::unsupported(
                "runtime.harness-execution",
                "drive refused: another process already holds the P423 driver lock for this ledger",
            ),
        );
        return Ok(report);
    };
    // P460 `--no-merge`: only now, having actually acquired the driver
    // lock above (never on the `Busy` early return), clear a persisted
    // merge intent before the drive loop runs.
    if input.clear_merge_intent {
        ctx_traits_io::run_session::set_merge_intent(&ledger_path, None)?;
    }
    // P445: created here (outside `drive_loop`) so every one of its ~28
    // early-return exits still leaves this invocation's observed totals
    // readable afterward — the accumulators are cheap-clone `Arc` handles,
    // not part of `drive_loop`'s own return value.
    let work_total = WorkTokenTotal::default();
    let narrator_tokens = harness_stream::NarratorTokenTracker::default();
    let guide_tokens = harness_stream::OneShotTokenTracker::default();
    install_live_guide(
        run_panel.0.as_ref(),
        &mut profile,
        input.execution_dir,
        guide_tokens.clone(),
        ledger_path.clone(),
    )?;
    let activity = ActivityRecorder::default();
    activity
        .attach_sink(ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(&ledger_path));
    // P552: the one permitted session-title attempt, claimed and dispatched
    // here — after the first pane paint and worktree preparation, under the
    // just-acquired driver lock, and strictly before `drive_loop` starts
    // writing frames — so a detached narrator thread never races a frame's
    // whole-ledger write, and a resumed drive never dispatches a second call.
    maybe_dispatch_session_title(
        &input,
        run_panel.0.as_ref(),
        &session_for_lock,
        &ledger_path,
        &narrator_tokens,
        &mut profile,
    );
    let mut report = drive_loop(
        input,
        drive_started,
        &mut profile,
        &budget,
        &work_total,
        &narrator_tokens,
        session_for_lock,
        tripwire.as_mut(),
        &ledger_path,
        &activity,
        run_panel.0.take(),
    )?;
    report.activity = activity.snapshot();
    // P479 terminal sweep: one more checkpoint after the loop returns,
    // covering the interval between the last loop-top check and however this
    // invocation actually ended (including every early return inside
    // `drive_loop`) — placed BEFORE `record_drive_outcome` below so a run
    // whose last frame escaped is downgraded from `completed` and cannot
    // auto-land (P460's `complete_after_drive` gates on `outcome ==
    // "completed"`).
    let _ = tripwire_checkpoint(&mut report, &ledger_path, tripwire.as_mut());
    if let Some(worktree) = retention_worktree.as_deref() {
        let outcomes = ctx_traits_io::retention::prune_terminal_cheap_artifacts(
            worktree,
            &profile.worktree.retention.cheap,
        );
        let removed: Vec<_> = outcomes.iter().filter(|outcome| outcome.removed).collect();
        if !removed.is_empty() {
            report.warnings.push(format!(
                "retention removed cheap artifacts: {}",
                removed
                    .iter()
                    .map(|outcome| outcome.relative_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for outcome in outcomes
            .into_iter()
            .filter(|outcome| outcome.error.is_some())
        {
            report.warnings.push(format!(
                "retention cheap-artifact cleanup failed for {}: {}",
                outcome.relative_path,
                outcome.error.unwrap_or_default()
            ));
        }
    }
    report.warnings.extend(worktree_retry_warnings);
    report.warnings.extend(resume_retry_warnings);
    // P427: one grouped line per built-in harness the implicit `default`
    // (driver) seat (or any declared role re-resolved through `profile` this
    // drive) picked up automatically, reusing the existing warnings channel
    // — no separate announcement path, and JSON output already carries
    // `warnings` structurally as part of `DriveReport`.
    report.warnings.extend(profile.builtin_fallback_warnings());
    // P455: a run's last accepted frame can enqueue its terminal step-summary
    // call microseconds before `drive_loop` returns, with nothing after it to
    // share a thread slot with — give that detached call a short bounded
    // grace window so a normal (non-hung) narrator's tokens make this
    // snapshot instead of racing it. A no-op when nothing is in flight; a
    // genuinely slow/hung call still reports incomplete past this bound.
    narrator_tokens.settle(std::time::Duration::from_millis(NARRATOR_SETTLE_GRACE_MS));
    guide_tokens.settle(std::time::Duration::from_millis(NARRATOR_SETTLE_GRACE_MS));
    let narrator_snapshot = narrator_tokens.snapshot();
    let guide_snapshot = guide_tokens.snapshot();
    let work_tokens = work_total.get();
    let token_usage = (work_tokens > 0
        || narrator_snapshot.tokens.is_some()
        || narrator_snapshot.complete.is_some()
        || guide_snapshot.tokens.is_some()
        || guide_snapshot.complete.is_some())
    .then_some(ctx_traits_core::procedure::session::TokenUsageEvidence {
        work_tokens: (work_tokens > 0).then_some(work_tokens),
        narrator_tokens: narrator_snapshot.tokens,
        narration_complete: narrator_snapshot.complete,
        guide_tokens: guide_snapshot.tokens,
        guide_complete: guide_snapshot.complete,
    });
    let evidence = ctx_traits_core::procedure::session::DriveTerminalEvidence {
        effective_budget: Some(budget_evidence(&budget)),
        token_usage,
        exit_code: Some(drive_report_exit_code(&report.status)),
    };
    // Stamp why the conductor exited; the ledger status alone cannot tell a
    // timed-out drive from one that is still running. Best-effort: a marker
    // write failure must not mask the drive outcome itself — except for a
    // credits pause, whose entire point is a resumable persisted marker; if
    // that write fails, reporting the pause as resumable would be a lie.
    if let Err(error) = ctx_traits_io::run_session::record_drive_outcome(
        session,
        session_store,
        &report.status,
        report.credits_pause.clone(),
        evidence,
    ) {
        if report.credits_pause.is_some() {
            report.status = "harness-failed".to_string();
            report.credits_pause = None;
        }
        report
            .warnings
            .push(format!("drive outcome marker not recorded: {error}"));
    }
    if let Some(status) = report.final_session_status.as_ref() {
        let outcome =
            ctx_traits_core::procedure::session::DriveOutcomeKind::from_wire(report.status.clone());
        report.session_state = Some(ctx_traits_core::procedure::activity::SessionState::derive(
            status,
            Some(&outcome),
            false,
        ));
    }
    // Held through the outcome write above; dropped only now, releasing the
    // control socket and the flock together in one place.
    drop(driver_lock);
    Ok(report)
}

/// Numeric exit disposition for a terminal [`DriveReport::status`]:
/// `0` for the one success status, `1` for every other status this drive
/// loop can return control with. This is a distinct, minimal domain from
/// [`CompletionDisposition::exit_code`] (`run.rs`), which maps *merge*
/// dispositions to process exit codes for `ctx traits run` — raw drive
/// statuses (`"completed"`, `"harness-failed"`, `"awaiting-input"`, ...) have
/// no equivalent table anywhere in the tree today. Only ever `None` when the
/// driver crashed before reaching this point at all, which is the signal
/// P512's typed CANCELLED outcome is built on.
fn drive_report_exit_code(status: &str) -> u8 {
    if status == "completed" {
        0
    } else if status == "killed" {
        // P551: conventional SIGINT exit code, same as the headless escalation
        // ladder's own `_exit(130)` — a killed run is deliberately dead, not
        // a generic failure.
        130
    } else {
        1
    }
}

/// Build the P445 [`DriveBudgetEvidence`] this invocation's terminal ledger
/// records, from the same resolved [`Budget`] the execution loop itself
/// runs against — never a second best-effort resolution.
fn budget_evidence(budget: &Budget) -> ctx_traits_core::procedure::session::DriveBudgetEvidence {
    ctx_traits_core::procedure::session::DriveBudgetEvidence {
        max_frames: budget.max_frames,
        frame_seconds: budget.frame_seconds,
        total_seconds: budget.total_seconds,
        max_retries: budget.max_retries,
        attach_wait_seconds: budget.attach_wait_seconds,
        idle_seconds: budget.idle_seconds,
    }
}

enum ConductorLeaseOutcome {
    /// This session has never used concurrency: no lease or sidecar
    /// directory exists for it, and this drive is not requesting one
    /// (`max_in_flight <= 1`). No lease is acquired or even attempted.
    NotNeeded,
    /// This process is now the sole parent-ledger writer for this session;
    /// hold the file for the drive's entire duration.
    Acquired(std::fs::File),
    /// Another process holds the lease and this drive either did not opt
    /// into `--wait` or waited out its budget without acquiring it.
    Busy(Box<DriveReport>),
    /// A graceful `SIGINT` was observed while polling for the lease: the
    /// drive exits with the typed interrupted outcome immediately, with zero
    /// harness starts, rather than waiting out the remainder of the poll.
    Interrupted(Box<DriveReport>),
}

/// Acquire the P402 per-session conductor lease whenever `--max-in-flight >
/// 1` OR durable concurrent state already exists for this session (a prior
/// conductor's sidecar directory is non-empty) — the latter closes the P402
/// risk where a resumed *default-width* invocation could otherwise race an
/// active concurrent conductor for the same parent ledger. A session that
/// has never used concurrency creates no lease/sidecars and takes this path
/// at zero cost. `--wait` polls for the lease within the drive's configured
/// total-time budget (falling back to the default budget when
/// `total_seconds` was not supplied); without it, a contended lease returns
/// the typed busy outcome immediately rather than racing the other
/// conductor's writes.
fn acquire_conductor_lease_if_needed(
    input: &DriveInputs<'_>,
    drive_started: Instant,
    budget: &Budget,
) -> crate::Result<ConductorLeaseOutcome> {
    let ledger_path =
        ctx_traits_io::run_session::resolve_session_path(input.session, input.session_store)?;
    let sidecars_root = ctx_traits_io::run_branch::sidecars_root(&ledger_path);
    let durable_state_exists = std::fs::read_dir(sidecars_root.as_std_path())
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if input.max_in_flight <= 1 && !durable_state_exists {
        return Ok(ConductorLeaseOutcome::NotNeeded);
    }
    let lease_path = ctx_traits_io::run_branch::conductor_lease_path(&ledger_path);
    // Same anchor instant AND same budget `drive()` resolved once through the
    // authoritative profile path (P402 `conductor-wait-bypasses-drive-deadline`):
    // never a fresh timer, and never a second best-effort duration hint. The
    // execution loop is handed this identical `budget`, so the lease wait and
    // the execution phase can never disagree on the deadline — a wait that
    // acquires the lease near the deadline leaves only the actual remaining
    // budget for execution, never a fresh full one.
    let total_seconds = budget.total_seconds;
    loop {
        // Checked every poll tick (not just at acquisition/timeout) so a
        // SIGINT during the wait produces the typed interrupted exit
        // immediately, with zero harness starts, instead of waiting out the
        // remainder of the poll.
        if crate::app::interrupt::is_interrupted() {
            let mut report = busy_report(input);
            report.status = "interrupted".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.harness-execution",
                    "drive stopped after a graceful SIGINT while waiting for the P402 conductor lease; resume with the same --session to continue",
                ),
            );
            return Ok(ConductorLeaseOutcome::Interrupted(Box::new(report)));
        }
        let acquired = ctx_traits_io::file_lock::try_acquire_conductor_lease(&lease_path).map_err(
            |source| -> ctx_traits_io::Error {
                ctx_traits_io::environment::Error::Filesystem {
                    path: lease_path.to_string(),
                    source,
                }
                .into()
            },
        )?;
        if let Some(file) = acquired {
            return Ok(ConductorLeaseOutcome::Acquired(file));
        }
        if !input.wait || drive_started.elapsed().as_secs() >= total_seconds {
            let mut report = busy_report(input);
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.concurrency-conductor",
                    format!(
                        "another process holds the P402 conductor lease for this session at {lease_path}; retry, or pass --wait to block for it"
                    ),
                ),
            );
            return Ok(ConductorLeaseOutcome::Busy(Box::new(report)));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn busy_report(input: &DriveInputs<'_>) -> DriveReport {
    DriveReport {
        status: "concurrency-conductor-busy".to_string(),
        session: input.session.to_string(),
        frames_attempted: 0,
        frames_accepted: 0,
        warnings: Vec::new(),
        capabilities: Vec::new(),
        events: Vec::new(),
        final_session_status: None,
        session_state: Some(ctx_traits_core::procedure::activity::SessionState::Running),
        activity: Vec::new(),
        credits_pause: None,
        merge: None,
    }
}

/// Resolve the requested `--worktree` id for a standalone drive and create or
/// resume the worktree, reading the run-session ledger's own session id when
/// `input.session` is an explicit ledger path rather than a bare id.
fn prepare_standalone_worktree(
    input: &DriveInputs<'_>,
    requested: Option<&str>,
    run_panel: Option<&run_view::RunPanel>,
) -> crate::Result<ctx_traits_io::worktree::PreparedWorktree> {
    let session_id = worktree_session_id(input)?;
    let id = requested
        .map(ToString::to_string)
        .unwrap_or_else(|| ctx_traits_io::worktree::derive_worktree_id(&session_id));
    let profile = ctx_traits_io::harness_config::resolve_runtime_assignments(input.assignments)?;
    // Setup commands run inside the fresh worktree, so they receive the
    // resolved `[worktree].env` overlay (repository-relative path values
    // resolved against the invocation checkout, `{worktree}` values against
    // this run's own worktree path). Empty when none is declared.
    let planned_worktree_path = ctx_traits_io::worktree::worktree_path_for(&id)?;
    let setup_env = ctx_traits_io::harness_config::resolve_effective_worktree_env(
        &profile.worktree,
        Some(planned_worktree_path.as_path()),
    )?;
    // P551: the panel already exists (created before this worktree block), so
    // a fresh worktree's creation/seeding/setup steps are narrated in-pane
    // instead of the pane sitting frozen until the first frame starts.
    let note = |text: &str| {
        if let Some(panel) = run_panel {
            panel.note(text.to_string());
        }
    };
    let progress: Option<&dyn Fn(&str)> = Some(&note);
    Ok(ctx_traits_io::worktree::resume_or_prepare_worktree(
        &id,
        ctx_traits_io::worktree::WorktreeContents {
            seeds: &profile.worktree.seed,
            warm: &profile.worktree.warm,
        },
        &profile.worktree.setup,
        &setup_env,
        ctx_traits_io::worktree::WorktreePrepareBudget {
            setup_timeout_ms: profile.worktree.setup_seconds.map(|seconds| seconds * 1000),
            setup_capture_bytes: profile.worktree.setup_capture_bytes,
            worktree_add_timeout_ms: Some(
                ctx_traits_io::harness_config::resolve_git_long_timeout_ms(camino::Utf8Path::new(
                    ".",
                )),
            ),
        },
        progress,
    )?)
}

/// Resolve the implicit resume execution directory for a standalone `drive
/// --session <ledger>` invocation with no explicit `--worktree` override: read
/// the ledger's worktree provenance (if any) and verify it is still
/// registered on its recorded branch, per the same check `ctx traits merge`
/// uses before touching Git state. `None` when the session has no worktree
/// provenance, so legacy/non-worktree sessions keep invocation-checkout
/// behavior.
fn resolve_resume_worktree(
    input: &DriveInputs<'_>,
) -> crate::Result<Option<(camino::Utf8PathBuf, Vec<String>, String)>> {
    let session = ctx_traits_io::run::read_session(input.session, input.session_store)?;
    match session.provenance.worktree {
        Some(worktree) => {
            let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
            let path = ctx_traits_io::worktree::verify_worktree_registration(
                &worktree.id,
                &worktree.branch,
                &mut warnings,
            )?;
            Ok(Some((path, warnings.into_vec(), worktree.id)))
        }
        None => Ok(None),
    }
}

fn worktree_session_id(input: &DriveInputs<'_>) -> crate::Result<String> {
    match ctx_traits_io::run_session::explicit_run_session_path(input.session) {
        Some(path) => {
            let session =
                ctx_traits_io::run_session::read_run_session(camino::Utf8Path::new(path))?;
            Ok(session.session_id.as_str().to_string())
        }
        None => Ok(input.session.to_string()),
    }
}

/// Resolve the drive's effective runtime profile through the authoritative
/// trait/session/assignment path — the single source of truth for the drive
/// budget (P402 `conductor-wait-bypasses-drive-deadline`) and for every
/// per-frame harness assignment. Resolved once in `drive()` (before the
/// conductor-lease wait) and threaded into `drive_loop`, so there is exactly
/// one effective-budget resolution path, never a second best-effort hint.
fn resolve_drive_profile(
    input: &DriveInputs<'_>,
) -> crate::Result<ctx_traits_io::harness_config::ResolvedRuntimeAssignments> {
    let session = ctx_traits_io::run::read_session(input.session, input.session_store)?;
    let loaded_trait =
        ctx_traits_io::run::load_trait_for_session(input.file, None, &session, "drive")?;
    Ok(
        ctx_traits_io::harness_config::resolve_trait_runtime_assignments(
            &loaded_trait.trait_ref,
            &loaded_trait.trait_root,
            input.assignments,
        )?,
    )
}

// Terminal-restore guard (owner incident 2026-07-22): the live ratatui pane
// must be closed DETERMINISTICALLY wherever it can end — on every early
// return in both `drive()` (P551: the panel now exists before worktree
// prep, so pre-`drive_loop` error returns must close it too) and
// `drive_loop` — because per-frame narrator workers are detached threads
// holding panel clones; relying on the last `Arc` dropping leaves raw mode
// + the alternate screen stuck on the user's tty when such a worker
// outlives main. `close()` is idempotent and late renders no-op on the
// closed pane.
struct RunPanelGuard(Option<run_view::RunPanel>);
impl Drop for RunPanelGuard {
    fn drop(&mut self) {
        if let Some(panel) = self.0.as_ref() {
            panel.close();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drive_loop(
    input: DriveInputs<'_>,
    drive_started: Instant,
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    budget: &Budget,
    work_total: &WorkTokenTotal,
    narrator_tokens: &harness_stream::NarratorTokenTracker,
    session_for_baseline: ctx_traits_core::procedure::session::Session,
    mut tripwire: Option<&mut ctx_traits_io::tripwire::Tripwire>,
    ledger_path: &camino::Utf8Path,
    activity: &ActivityRecorder,
    initial_run_panel: Option<run_view::RunPanel>,
) -> crate::Result<DriveReport> {
    let mut input = input;
    let tui_degraded_to_status =
        input.progress == cli::DriveProgress::Tui && !tui::stderr_supports_live(false);
    let narration_requested = matches!(
        input.progress,
        cli::DriveProgress::Stream | cli::DriveProgress::Tui
    );
    if tui_degraded_to_status {
        input.progress = cli::DriveProgress::Status;
    }
    // Cumulative active-drive elapsed seconds already accrued across earlier
    // resumes of this run, read once from the ledger by the caller (`drive`)
    // before the P423 driver lock was acquired. Combined with this process's
    // own `Instant` below, this is the only place drive time is measured —
    // the core runtime never reads a clock (see `elapsed-seconds-at-least`
    // guards in `condition.rs`/`guards.rs`). Paused or unattached wall time
    // (between resumes) does not count: only the sum of each invocation's
    // own elapsed budget does. (The profile itself is resolved once in
    // `drive()` before the conductor-lease wait and passed in — P402
    // conductor-wait-bypasses-drive-deadline.)
    let baseline_elapsed_seconds = session_for_baseline.ledger.elapsed_seconds;
    // ONE effective `[worktree].env` overlay for this drive/resume, applied to
    // every subprocess launched inside the run worktree (command frames,
    // harness probe, cold/warm CLI + MCP harnesses, narrators). Only non-empty
    // when a worktree is actually in play (`execution_dir` set) and
    // `.ctx/config.toml [worktree]` declared an overlay; repository-relative
    // path values resolve against the invocation checkout, never the
    // worktree. Re-resolved here on every drive/resume rather than persisted
    // into the ledger.
    let (worktree_env, confinement_payloads) =
        resolve_effective_worktree_env(input.execution_dir, profile)?;
    // P478: the claude-code half of confinement is argv-delivered
    // (`--settings`); the opencode half is already folded into
    // `worktree_env` above (`OPENCODE_CONFIG_CONTENT`).
    let narrator_plan = if narration_requested {
        profile
            .resolved_narrator_assignment()?
            .map(|assignment| plan_from_assignment(assignment, None, None))
    } else {
        None
    };
    validate_narrator_assignment(profile, narrator_plan.as_ref())?;
    // Reuses the single deadline anchor `drive()` established before the
    // conductor-lease wait, rather than starting a fresh timer here — time
    // already spent waiting for the lease counts against this same
    // `total_seconds` budget (P402 conductor-wait-bypasses-drive-deadline).
    // Known imprecision: that same lease wait therefore also accrues toward
    // the P391 `elapsed-seconds-at-least` guard baseline below (making the
    // guard slightly more permissive); lease contention and elapsed guards
    // have no consumer overlap today, and attach-wait pauses are still
    // subtracted exactly.
    let started = drive_started;
    // Time spent inside `wait_for_attach_advance` below is this process
    // blocked polling for an external actor (a human, or another harness) to
    // advance the ledger — not active drive work — so it must not accrue
    // toward `elapsed-seconds-at-least`. Accumulated by that function via
    // this cell and subtracted from `started.elapsed()` below.
    let attach_wait_paused = std::cell::Cell::new(Duration::ZERO);
    // Recomputed at every guard-evaluating transition site below rather than
    // once, since the loop can run for a long time and each site should
    // observe the freshest active-drive elapsed value.
    let current_elapsed_seconds = || {
        Some(
            baseline_elapsed_seconds
                + started
                    .elapsed()
                    .saturating_sub(attach_wait_paused.get())
                    .as_secs(),
        )
    };
    let (runtime_warning, runtime_capability) = ctx_traits_core::launch::runtime_posture();
    let mut report = DriveReport {
        status: "running".to_string(),
        session: input.session.to_string(),
        frames_attempted: 0,
        frames_accepted: 0,
        warnings: vec![runtime_warning.message],
        capabilities: vec![runtime_capability],
        events: Vec::new(),
        final_session_status: None,
        session_state: Some(ctx_traits_core::procedure::activity::SessionState::Running),
        activity: Vec::new(),
        credits_pause: None,
        merge: None,
    };
    report
        .capabilities
        .extend_from_slice(profile.model_catalog_capability_reports());
    report.capabilities.sort();
    report.capabilities.dedup();
    // P516: seeded from the sidecar, filtered to entries anchored to this
    // process's own harness/exec-dir — a resumed process starts with the
    // conversations a prior process actually observed, instead of cold. A
    // missing/unreadable sidecar (including every pre-P516 repo) reads as
    // empty, exactly today's cold-start behavior.
    let harness_sessions_path = ctx_traits_io::run_branch::harness_sessions_path(ledger_path);
    let mut harness_sessions: BTreeMap<String, String> =
        ctx_traits_io::run_branch::read_harness_sessions(&harness_sessions_path)
            .unwrap_or_default()
            .sessions
            .into_iter()
            .filter(|(_, entry)| {
                entry.exec_dir.as_deref() == input.execution_dir.map(camino::Utf8Path::as_str)
            })
            .map(|(session_key, entry)| (session_key, entry.harness_session_id))
            .collect();
    let mut warm_harness_sessions =
        BTreeMap::<String, ctx_traits_io::harness::HarnessSession>::new();
    let mut warm_harness_respawn_used = BTreeSet::<String>::new();
    let mut warm_harness_disabled = BTreeSet::<String>::new();
    // First dispatched member of each declared `session:<id>` (P328
    // deliverable 3). Checked, not preflight-swept, at every later member's
    // first frame — see `SessionMembership` for the refuse/warn rules.
    let mut session_membership = BTreeMap::<String, SessionMembership>::new();
    let mut drive_probes = BTreeMap::<String, DriveProbe>::new();
    // Opt-in bounded concurrency (`--max-in-flight`, P344): outcomes from
    // `attempt_concurrent_wave`, keyed by the owning `parallel` panel's
    // `control_item_id` then by absolute branch offset. Populated ahead of
    // a branch's own turn, consumed (and removed) the one time the
    // sequential cursor actually reaches that branch. Empty and never
    // touched when `max_in_flight <= 1` (the default).
    let mut pending_wave_cache = PendingWaveCache::new();
    let narrator_warm_pool = harness_stream::NarratorWarmPool::default();
    let narrator_trace_sequence = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    // Terminal-restore guard (owner incident 2026-07-22; P551 hoisted to
    // module scope so `drive()`'s pre-worktree panel — see
    // `initial_run_panel` — is covered by the SAME close-on-drop guarantee
    // as this loop): the live ratatui pane must be closed DETERMINISTICALLY
    // when this loop ends — on every one of its early returns, not just the
    // happy path — because per-frame narrator workers are detached threads
    // holding panel clones; relying on the last Arc dropping leaves raw mode
    // + the alternate screen stuck on the user's tty when such a worker
    // outlives main. `close()` is idempotent and late renders no-op on the
    // closed pane.
    let mut run_panel = RunPanelGuard(initial_run_panel);
    let mut trace_warned = false;
    let mut trace_sequence = 0;
    let mut startup_announced = false;
    let mut reported_model_resolutions = BTreeSet::new();
    if let Some(plan) = narrator_plan.as_ref()
        && let Some(evidence) = plan.model_resolution_evidence.as_deref()
    {
        reported_model_resolutions.insert(format!("narrator:{}", plan.harness_id));
        report.events.push(DriveEvent {
            event: "model-resolution".to_string(),
            role: Some("narrator".to_string()),
            harness: Some(plan.harness_id.clone()),
            detail: evidence.to_string(),
            duration_ms: None,
        });
    }

    'frames: loop {
        if started
            .elapsed()
            .saturating_sub(attach_wait_paused.get())
            .as_secs()
            >= budget.total_seconds
        {
            report.status = "total-budget-exhausted".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.harness-execution",
                    "drive total-seconds budget exhausted",
                ),
            );
            return Ok(report);
        }
        // P402 cooperative SIGINT: only checked here, between frames/waves,
        // never inside one — this never interrupts an in-flight harness
        // call, it only stops the loop from starting a new reservation. Any
        // outcome already cached in `pending_wave_cache` is still drained
        // through the ordinary sequential path below before this loop exits,
        // so a call that was already made and paid for is never stranded.
        if crate::app::interrupt::is_interrupted() && pending_wave_cache.is_empty() {
            report.status = "interrupted".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.harness-execution",
                    "drive stopped after a graceful SIGINT; resume with the same --session to continue from the parent cursor",
                ),
            );
            return Ok(report);
        }
        // A concurrent wave (P344 `--max-in-flight`) charges every branch it
        // dispatches to `frames_attempted` up front, at launch time, not when
        // the sequential cursor later consumes each branch's outcome (see
        // `attempt_concurrent_wave`). So a wave that spends the last two
        // available attempts on two branches can legitimately push
        // `frames_attempted` to (or past) `max_frames` while one of those
        // already-paid-for outcomes is still sitting in `pending_wave_cache`
        // waiting for its turn. Exiting here in that case would strand a
        // call that was already made and paid for — never applied, and (on
        // resume) paid for again. Only exit once every reserved outcome has
        // been consumed.
        if report.frames_attempted >= budget.max_frames && pending_wave_cache.is_empty() {
            report.status = "max-frames-exhausted".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.harness-execution",
                    "drive max-frames budget exhausted",
                ),
            );
            return Ok(report);
        }

        // P479 loop-top checkpoint: runs BEFORE the next frame resolves or
        // dispatches anything, so an out-of-tree escape can never buy another
        // paid frame. On the first iteration this call only takes the
        // baseline (see `Tripwire::checkpoint`'s lazy-baseline doc) and
        // reports nothing.
        if tripwire_checkpoint(&mut report, ledger_path, tripwire.as_deref_mut()) {
            return Ok(report);
        }

        let mut outcome = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
            trait_file: input.file,
            trait_id: None,
            session: input.session,
            session_store: input.session_store,
            elapsed_seconds: current_elapsed_seconds(),
        })?;
        if outcome.session.status
            == ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired
        {
            // P479: label the tripwire's NEXT checkpoint with this command
            // frame BEFORE it dispatches — `advance_commands` below is the
            // one place in this loop that can itself perform I/O against the
            // invocation repository (a command step's own subprocess is not
            // covered by P478's harness-spawn confinement), so a finding it
            // causes must not fall back to the generic "before first frame"
            // label. Read from `outcome.session.next_frame` now: the call
            // below replaces `outcome` with a fresh one.
            if let (Some(wire), Some(next_frame)) = (
                tripwire.as_deref_mut(),
                outcome.session.next_frame.as_deref(),
            ) {
                wire.set_frame_label(format!(
                    "{} (item:{}, kind:command)",
                    frame_position_label(next_frame),
                    next_frame.item_id.as_deref().unwrap_or("-"),
                ));
            }
            // Paint the command step as running BEFORE the blocking
            // execution: interactive commands (ctx-annotate) can hold this
            // frame for a long time and the panel must not look frozen.
            refresh_run_panel(&mut run_panel.0, &mut input, &outcome.session);
            command_started_event(&outcome.session, run_panel.0.is_some());
            outcome =
                ctx_traits_io::run::advance_commands(ctx_traits_io::run::AdvanceCommandsRequest {
                    trait_file: input.file,
                    trait_id: None,
                    session: input.session,
                    session_store: input.session_store,
                    execution_dir: input.execution_dir,
                    execution_env: &worktree_env,
                    elapsed_seconds: current_elapsed_seconds(),
                    tick_observer: run_panel.0.as_ref().map(run_view::RunPanel::tick_observer),
                })?;
            if let Some(failure) = outcome.command_failure.as_ref() {
                report.final_session_status = Some(outcome.session.status.clone());
                refresh_run_panel(&mut run_panel.0, &mut input, &outcome.session);
                command_failure_event(&mut report, failure);
                return Ok(report);
            }
        }
        report.final_session_status = Some(outcome.session.status.clone());
        refresh_run_panel(&mut run_panel.0, &mut input, &outcome.session);
        match outcome.session.status {
            ctx_traits_core::procedure::session::Status::Completed => {
                report.status = "completed".to_string();
                // P549: the ONE normal-completion exit — hand the live pane
                // off instead of letting `RunPanelGuard` close it, so a
                // caller with a persisted merge intent can keep the same
                // pane running through the automatic merge. `take()` clears
                // `run_panel.0`, so the guard's drop becomes a no-op for
                // this exit; every other return in this function leaves
                // `run_panel.0` untouched and the guard closes as before.
                if let Some(handoff) = input.panel_handoff.as_ref()
                    && let Some(panel) = run_panel.0.take()
                {
                    handoff.give(panel);
                }
                return Ok(report);
            }
            ctx_traits_core::procedure::session::Status::Failed => {
                report.status = "failed".to_string();
                record_stop_reason(&mut report, &outcome.session, run_panel.0.as_ref());
                return Ok(report);
            }
            ctx_traits_core::procedure::session::Status::AwaitingInput => {
                report.status = "awaiting-input".to_string();
                return Ok(report);
            }
            ctx_traits_core::procedure::session::Status::WaitingOnHuman => {
                report.status = "waiting-on-human".to_string();
                return Ok(report);
            }
            ctx_traits_core::procedure::session::Status::Blocked
            | ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned
            | ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired => {
                report.status = format_status(&outcome.session.status);
                // A structured control-flow stop (loop exhausted, overflow) is
                // the real outcome — report it instead of a bare "blocked".
                if let Some(stop) = outcome.session.stop_reason.as_ref() {
                    report.status = stop.reason.clone();
                }
                record_stop_reason(&mut report, &outcome.session, run_panel.0.as_ref());
                return Ok(report);
            }
            ctx_traits_core::procedure::session::Status::AwaitingAgentOutput
            | ctx_traits_core::procedure::session::Status::Rejected => {}
        }

        let Some(mut frame) = outcome.session.next_frame.clone() else {
            report.status = "no-frame".to_string();
            return Ok(report);
        };
        if tui_degraded_to_status {
            progress(input.progress, "in-progress");
        }
        // Owned, not borrowed from `frame`: the correction-retry path below
        // rebinds `frame` to the rejection response's refreshed copy, and the
        // role cannot keep a borrow into the discarded one. The role itself
        // is stable across that rebind — a rejection never reassigns the
        // frame's agent.
        let role = frame.assigned_agent.as_ref().map_or(
            ctx_traits_io::harness_config::DEFAULT_SEAT.to_string(),
            |agent| agent.role.clone(),
        );
        let role = role.as_str();
        let structural_seat = frame
            .assigned_agent
            .as_ref()
            .and_then(|agent| agent.structural_seat);
        let role_budget = profile.budget_for_seat(role, structural_seat);
        let frame_role_budget = frame_budget(budget, &input, &profile.budget, &role_budget);
        let budget = &frame_role_budget;
        let plan = match assignment_for_role(profile, &outcome.session, role, structural_seat)? {
            Some(plan) => plan,
            None => {
                report.status = "blocked-agent-unassigned".to_string();
                let rows = profile.builtin_detection().to_vec();
                push_capability(
                    &mut report,
                    ctx_traits_core::response::CapabilityReport::unsupported(
                        "runtime.harness-execution",
                        format!(
                            "no harness assignment for role {role}; {} — {}",
                            ctx_traits_io::harness_config::unassigned_role_remediation(role),
                            ctx_traits_io::harness_config::no_builtin_harness_message(&rows, role)
                        ),
                    ),
                );
                return Ok(report);
            }
        };
        // P479: record the label for the frame about to dispatch, so the
        // NEXT checkpoint (the following loop-top, or the terminal sweep if
        // this is the last frame) attributes any mutation it observes to
        // this window rather than a generic "a frame ran" statement.
        if let Some(wire) = tripwire.as_deref_mut() {
            wire.set_frame_label(format!(
                "{} (item:{}, agent:{}@{})",
                frame_position_label(&frame),
                frame.item_id.as_deref().unwrap_or("-"),
                role,
                plan.harness_id,
            ));
        }
        for capability in profile.model_catalog_capability_reports() {
            push_capability(&mut report, capability.clone());
        }
        if plan.from_session {
            let model_catalog_capability = format!("runtime.model-catalog.{}", plan.harness_id);
            for capability in &outcome.session.provider_capability_reports {
                if capability.capability == model_catalog_capability {
                    push_capability(&mut report, capability.clone());
                }
            }
        }
        if let Some(evidence) = plan.model_resolution_evidence.as_deref() {
            let key = match plan.seat_index {
                Some(seat_index) => format!("{role}.{seat_index}:{}", plan.harness_id),
                None => format!("{role}:{}", plan.harness_id),
            };
            if reported_model_resolutions.insert(key) {
                let detail = match (plan.seat_index, plan.list_length) {
                    (Some(seat_index), Some(list_length)) => {
                        format!("{evidence} seat-index={seat_index} list-length={list_length}")
                    }
                    _ => evidence.to_string(),
                };
                report.events.push(DriveEvent {
                    event: "model-resolution".to_string(),
                    role: Some(role.to_string()),
                    harness: Some(plan.harness_id.clone()),
                    detail,
                    duration_ms: None,
                });
            }
        }
        if !startup_announced {
            let message = format!("starting session · dispatching {role}@{}…", plan.harness_id);
            match input.progress {
                cli::DriveProgress::Status | cli::DriveProgress::Stream => {
                    progress_startup(input.progress, &message);
                }
                cli::DriveProgress::Tui => {
                    if let Some(panel) = run_panel.0.as_ref() {
                        panel.push_summary(message);
                    }
                }
                cli::DriveProgress::None => {}
            }
            startup_announced = true;
        }
        if plan.mode == ctx_traits_io::harness_config::RunAssignmentMode::Attach {
            if wait_for_attach_advance(
                &input,
                &mut report,
                role,
                &outcome.session.state_digest,
                budget,
                run_panel.0.as_ref(),
                &attach_wait_paused,
            )? {
                continue;
            }
            return Ok(report);
        }
        let loaded_trait = ctx_traits_io::run::load_trait_for_session(
            input.file,
            None,
            &outcome.session,
            "drive",
        )?;
        let mut prompt_context =
            resolved_frame_prompt(&loaded_trait, &outcome.session, &frame, &[])?;
        // Cloned (not borrowed) because the current frame's concurrent-wave
        // attempt below needs `profile` mutably to resolve each peeked
        // sibling's own harness/CLI convention/plan (P456); an owned value
        // here means this frame's own harness/cli never keep an immutable
        // borrow of `profile` alive across that call.
        let Some(harness) = profile.registry.harness.get(&plan.harness_id).cloned() else {
            return Err(crate::Error::Command {
                message: format!("unknown harness {}", plan.harness_id),
            });
        };
        let probe = ensure_drive_probe(
            &mut drive_probes,
            &mut report,
            &outcome.session,
            &plan.harness_id,
            &harness,
            input.execution_dir,
            &worktree_env,
        );
        if !probe.supported {
            report.status = "blocked-harness-unprobed".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-execution.{}", plan.harness_id),
                    format!(
                        "harness {} has no successful probe evidence",
                        plan.harness_id
                    ),
                ),
            );
            return Ok(report);
        }
        if plan.model.is_some()
            && harness
                .cli
                .as_ref()
                .is_none_or(|cli| cli.model_flag.is_none())
        {
            report.status = "unsupported-harness-model-selection".to_string();
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-model-selection.{}", plan.harness_id),
                    format!(
                        "harness {} has a resolved model but no CLI model flag",
                        plan.harness_id
                    ),
                ),
            );
            return Ok(report);
        }
        // Resolved and membership-checked BEFORE the transport branch below,
        // regardless of transport: a shared `[[agent]].session` binding
        // (P328) must be caught whether the frame that carries it dispatches
        // over MCP or CLI, never only on the CLI path a Shared binding
        // happens to be compatible with. `check_session_membership`'s own
        // non-CLI refusal arm is reachable from here.
        let session = effective_session(&loaded_trait.trait_ref, role, &plan, &worktree_env);
        if let Some(session_id) = session.shared_id.as_deref() {
            match check_session_membership(&mut session_membership, session_id, role, &plan) {
                SessionMembershipCheck::First | SessionMembershipCheck::Compatible => {}
                SessionMembershipCheck::Warn { first_role } => {
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            "runtime.session-sharing",
                            format!(
                                "session:{session_id} member agent:{role} declares a different model/reasoning-effort than agent:{first_role}, the session's first dispatched member; a warm process spawned for {first_role} ignores {role}'s model/reasoning-effort flags"
                            ),
                        ),
                    );
                }
                SessionMembershipCheck::Refuse {
                    first_role,
                    first_harness_id,
                } => {
                    report.status = "session-membership-conflict".to_string();
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            "runtime.session-sharing",
                            format!(
                                "session:{session_id} member agent:{role}@{} is incompatible with agent:{first_role}@{first_harness_id}, the session's first dispatched member (shared-session members must agree on harness, transport, and attach-mode, and sharing is only supported over the CLI transport)",
                                plan.harness_id
                            ),
                        ),
                    );
                    return Ok(report);
                }
            }
        }
        if plan.transport == ctx_traits_io::harness_config::RunTransport::Mcp {
            let narrator_resolution = narrator_config(
                &input,
                profile,
                narrator_plan.as_ref(),
                NarratorFrameContext {
                    session: &outcome.session,
                    item_id: frame.item_id.as_deref(),
                    task_label: &frame.title,
                    trace_sequence: &narrator_trace_sequence,
                },
                &narrator_warm_pool,
                &worktree_env,
                confinement_payloads.as_ref(),
            );
            for capability in narrator_resolution.unsupported_confinement {
                push_capability(&mut report, capability);
            }
            // P480: an MCP transport spawn is wrapped by the same OS-level
            // sandbox as every other worktree spawn — report the same
            // unavailable-OS-layer gap the CLI transport does.
            if let Some(payloads) = confinement_payloads.as_ref()
                && let Some(capability) =
                    ctx_traits_io::confinement::spawn_sandbox_unsupported_capability(
                        payloads.sandbox_requested,
                        payloads.spawn_sandbox.as_ref(),
                    )
            {
                push_capability(&mut report, capability);
            }
            if drive_mcp_frame(
                &input,
                &mut report,
                budget,
                &harness,
                &plan,
                LiveFramePresentation {
                    narrator: narrator_resolution.config,
                    run_panel: run_panel.0.as_ref(),
                    narrator_tokens,
                },
                CurrentMcpFrame {
                    frame: &frame,
                    role,
                    session: &outcome.session,
                    prompt: &prompt_context,
                    env_overlay: &worktree_env,
                    elapsed_seconds: current_elapsed_seconds(),
                    sandbox: confinement_payloads
                        .as_ref()
                        .and_then(|payloads| payloads.spawn_sandbox.clone()),
                },
                activity,
            )? {
                continue;
            }
            return Ok(report);
        }
        let Some(cli) = harness.cli.as_ref() else {
            report.status = "unsupported-harness-cli".to_string();
            return Ok(report);
        };
        let output_id = cli.output.as_deref().unwrap_or("raw-json");
        let mut requested = requested_outputs(&frame)?;
        let mut schema = requested_output_schema(&requested, &loaded_trait);
        let mut contract = frame_contract_section(&frame);
        let mut prompt = frame_prompt(&prompt_context, &contract, &schema, None);
        // Standing instructions declared on the trait's `[[agent]]`. Delivered
        // through the harness system channel when the convention has one, and
        // folded into the prompt body when it does not (see below). Owned for
        // the same reason as `role`: the correction-retry path rebinds
        // `frame`, and the system text is identical on the refreshed copy.
        let agent_system = frame
            .assigned_agent
            .as_ref()
            .and_then(|agent| agent.system.clone());
        let agent_system = agent_system.as_deref();
        // The warm-session reuse key folds in the overlay identity so a
        // persistent process spawned under one `[worktree].env` is never
        // reused for a different one, and (P456) the 1-based seat for a
        // list-backed role so two seats of the same role never share a warm
        // conversation. When no overlay is declared and the role is a legacy
        // single table, the key is exactly the historical `role:harness` form.
        // A declared `[[agent]].session` binding (P328) is authoritative over
        // the configured `session_mode`/role-scoped key when present — see
        // `effective_session`. `session`/its membership check were already
        // resolved above, before the transport branch, so every dispatched
        // frame (MCP or CLI) is checked exactly once.
        let session_key = session.key;
        // P516: a conversation id resumed from the harness-sessions sidecar
        // (a prior process observed it, this one seeded it) that cannot
        // actually be dispatched — because this harness's CLI convention
        // declares neither `session-flag` nor `resume-flag` — is a reported
        // cold start, not a silent one. Same wording `prepare_correction_retry`
        // already uses, so the vocabulary does not fork.
        if harness_sessions.contains_key(&session_key)
            && cli.session_flag.is_none()
            && cli.resume_flag.is_none()
        {
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.harness-session-resume",
                    "harness declares no session-flag or resume-flag",
                ),
            );
        }
        let mut argv = harness_argv(
            &harness,
            cli,
            &plan,
            agent_system,
            HarnessArgvAttempt {
                schema: Some(&schema),
                harness_session_id: harness_sessions.get(&session_key),
                exec_dir: input.execution_dir,
                confinement: confinement_payloads.as_ref(),
            },
        );
        let mut warm_argv = (session.mode
            == ctx_traits_io::harness_config::RunSessionMode::Persistent)
            .then(|| {
                warm_harness_argv(
                    &harness,
                    cli,
                    &plan,
                    agent_system,
                    WarmPromptKind::Frame,
                    confinement_payloads.as_ref(),
                    harness_sessions.get(&session_key),
                )
            })
            .flatten();
        // P478: whichever payload actually applies to this attempt's harness
        // kind (argv-delivered for claude-code, env-delivered for opencode),
        // carried only for the debug trace — the argv/env above already
        // carry the actual enforcement.
        let confinement_trace = confinement_payloads.as_ref().and_then(|payloads| {
            ctx_traits_io::confinement::confinement_trace_payload(payloads, harness.kind())
        });
        // P480: this worktree's generated OS-level spawn sandbox, applied at
        // the spawn seam regardless of harness kind.
        let spawn_sandbox = confinement_payloads
            .as_ref()
            .and_then(|payloads| payloads.spawn_sandbox.clone());
        // P478/P480: an unsupported harness kind must never silently ship a
        // worktree spawn with zero write confinement, and a spawn that
        // actually requested OS enforcement must never silently run without
        // it — push_capability dedupes, so each reports once per kind/reason,
        // not once per frame.
        if let Some(payloads) = confinement_payloads.as_ref() {
            if let Some(capability) = ctx_traits_io::confinement::confinement_unsupported_capability(
                harness.kind(),
                spawn_sandbox.is_some(),
            ) {
                push_capability(&mut report, capability);
            }
            if let Some(capability) =
                ctx_traits_io::confinement::spawn_sandbox_unsupported_capability(
                    payloads.sandbox_requested,
                    payloads.spawn_sandbox.as_ref(),
                )
            {
                push_capability(&mut report, capability);
            }
        }
        // A harness without a system-prompt flag (opencode's convention) would
        // otherwise silently drop the role's standing instructions. Compose
        // them into the prompt body instead so the model sees identical text on
        // every harness, and record that the delivery channel degraded — same
        // fallback the merger uses for its mechanical-only instruction.
        if let Some(system) = agent_system.filter(|_| cli.system_prompt_flag.is_none()) {
            prompt = format!("{system}\n\n{prompt}");
            // push_capability dedupes, so a multi-round loop reports the
            // degraded channel once per role instead of once per frame.
            push_capability(
                &mut report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "harness.system-prompt-channel",
                    format!(
                        "harness {} declares no system-prompt flag; agent:{} standing instructions were composed into the prompt body (digest {})",
                        plan.harness_id,
                        role,
                        ctx_traits_core::digest::Digest::source(system),
                    ),
                ),
            );
        }
        let prompt_delivery = if cli.prompt_via.as_deref() == Some("stdin") {
            ctx_traits_io::harness::PromptDelivery::Stdin
        } else {
            ctx_traits_io::harness::PromptDelivery::Arg
        };
        // P402 durable-wave-recovery gate: runs BEFORE any width-based
        // scheduling decision, regardless of `--max-in-flight` — a
        // default-width (`max_in_flight <= 1`) resume must get the exact
        // same fail-closed recovery guarantee a concurrent one does, so a
        // prior conductor's still-`Running`/corrupt/mismatched/incomplete
        // wave is never silently redispatched just because this invocation
        // happens to be sequential. Validates the whole original wave span
        // against its immutable manifest (see `recover_wave_offset`); a
        // session that never used concurrency has no manifest at all, so this
        // is `Absent` (one cheap read) on every frame.
        if let Some(activation_key) = parallel_wave_activation_key(&outcome.session)
            && let Some(offset) = current_wave_offset(&outcome.session)
            && let std::collections::btree_map::Entry::Vacant(vacant) =
                pending_wave_cache.entry(activation_key)
            && let Ok(ledger_path) =
                ctx_traits_io::run_session::resolve_session_path(input.session, input.session_store)
        {
            match recover_wave_offset(&ledger_path, vacant.key(), offset, &outcome.session) {
                SidecarRecovery::Absent => {}
                SidecarRecovery::Terminal(cached_outcome) => {
                    let mut single = WaveOutcomes::new();
                    single.insert(offset, cached_outcome);
                    vacant.insert(single);
                }
                SidecarRecovery::Indeterminate => {
                    report.status = "concurrency-recovery-blocked".to_string();
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            "runtime.harness-concurrency",
                            WaveIneligible::SidecarRecoveryIndeterminate.detail(),
                        ),
                    );
                    return Ok(report);
                }
            }
        }
        // Opt-in bounded concurrency (`--max-in-flight`, P344): when the
        // current frame sits inside a `parallel` panel and this exact
        // branch has no cached wave result yet, try resolving and
        // concurrently dispatching this branch alongside its next
        // `max-in-flight - 1` siblings. `attempt_concurrent_wave` never
        // mutates session/ledger state and is strictly additive — it only
        // ever *populates* `pending_wave_cache` for the retry loop below to
        // opportunistically consume. `Ok(None)` means concurrency simply
        // does not apply here (nothing to report); `Err(reason)` means it
        // applied but could not complete, which is surfaced as an explicit
        // capability instead of silently falling back to sequential.
        if input.max_in_flight > 1 {
            match attempt_concurrent_wave(
                &mut report,
                &mut trace_sequence,
                &mut trace_warned,
                &pending_wave_cache,
                ConcurrentWaveRequest {
                    input: &input,
                    loaded_trait: &loaded_trait,
                    session: &outcome.session,
                    role,
                    plan: &plan,
                    profile,
                    budget,
                    max_in_flight: input.max_in_flight,
                    current_frame: &frame,
                    worktree_env: &worktree_env,
                    confinement_payloads: confinement_payloads.as_ref(),
                    run_panel: run_panel.0.as_ref(),
                    work_total,
                    drive_probes: &mut drive_probes,
                },
            ) {
                Ok(Some((wave_key, wave))) => {
                    pending_wave_cache.insert(wave_key, wave);
                    // P402 (`p402-proof-absent-and-tests-misplaced`): every
                    // unit in this wave already has a durably-persisted
                    // terminal sidecar (`Completed`/`TerminalFailure` —
                    // written inside `attempt_concurrent_wave` before it
                    // returns) and NOTHING has been applied to the parent
                    // ledger yet. A no-op outside the concurrency-proof
                    // fixtures; see `test_only_checkpoint`.
                    test_only_checkpoint(TESTHOOK_CHECKPOINT_WAVE_PERSISTED);
                }
                Ok(None) => {}
                // P402 `durable-sidecars-not-connected`: a durable-persistence
                // guarantee could not be met (a manifest/reservation/terminal
                // write failed, or a prior wave's span is indeterminate). This
                // is a HARD, fail-closed block, never a soft sequential
                // fallback: redispatching the current frame now would
                // double-pay a call the wave may already have made, and
                // applying a partially-persisted wave would advance the parent
                // past a span recovery can never prove complete. Stop with
                // zero further dispatch and typed recovery evidence.
                Err(WaveIneligible::SidecarRecoveryIndeterminate) => {
                    report.status = "concurrency-recovery-blocked".to_string();
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            "runtime.harness-concurrency",
                            WaveIneligible::SidecarRecoveryIndeterminate.detail(),
                        ),
                    );
                    return Ok(report);
                }
                // Every other ineligibility (persistent session, wave too
                // small, sibling command/role mismatch, …) is a genuine "stay
                // sequential here" — surface it once and fall through to the
                // ordinary one-at-a-time dispatch below.
                Err(reason) => {
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            "runtime.harness-concurrency",
                            reason.detail(),
                        ),
                    );
                }
            }
        }
        // Physical dispatches own trace identity and wave-cache consumption.
        // retry_count charges process failures and actionable or condition-
        // changed corrections, never stale-identity runtime redispatch.
        let mut dispatch_attempt = 0;
        let mut retry_count = 0;
        // Correction ordinal advances only when correction text is sent.
        // Process failures still charge retry_count, but must not cause a
        // later correction to skip the cheap first resumed reshape.
        let mut correction_ordinal = 0;
        loop {
            // The outer loop owns drive-wide ceilings, but same-position
            // runtime redispatch stays in this loop. Recheck before every
            // additional physical dispatch so stale races cannot bypass them.
            if dispatch_attempt > 0 && started.elapsed().as_secs() >= budget.total_seconds {
                report.status = "total-budget-exhausted".to_string();
                push_capability(
                    &mut report,
                    ctx_traits_core::response::CapabilityReport::unsupported(
                        "runtime.harness-execution",
                        "drive total-seconds budget exhausted",
                    ),
                );
                return Ok(report);
            }
            if dispatch_attempt > 0
                && report.frames_attempted >= budget.max_frames
                && pending_wave_cache.is_empty()
            {
                report.status = "max-frames-exhausted".to_string();
                push_capability(
                    &mut report,
                    ctx_traits_core::response::CapabilityReport::unsupported(
                        "runtime.harness-execution",
                        "drive max-frames budget exhausted",
                    ),
                );
                return Ok(report);
            }
            // Opt-in bounded concurrency (`--max-in-flight`, P344): a cached
            // wave run was already charged to `frames_attempted` at the
            // moment `attempt_concurrent_wave` dispatched it (see below);
            // charging it again here would double-count a call that was
            // only made once. Every live dispatch — the default path, and
            // any retry — is charged exactly once, right here, as before.
            // P402: remember which durable sidecar (if any) this cache hit
            // corresponds to BEFORE consuming it, so a successful submission
            // below can mark that sidecar `applied` — only ever after the
            // ordinary parent-ledger write it corresponds to has already
            // durably succeeded (see `mark_applied`'s own contract).
            let cached_wave_identity = (dispatch_attempt == 0)
                .then(|| wave_cache_identity(&pending_wave_cache, &outcome.session))
                .flatten();
            // Baseline for detecting, below, whether THIS call's submission
            // was ultimately rejected — even when routed through a P264
            // `skip`/`park`/`panel-fail` branch-failure policy rather than
            // surfaced as `RejectedCorrectionRequired` (see
            // `reject_step_output`'s docs: a routed rejection clears the
            // report's own `rejected_outputs` before it ever reaches this
            // response, so `response_kind` alone cannot distinguish "content
            // accepted" from "content rejected but the branch failure was
            // auto-routed"). `rejected_submissions` only ever grows for the
            // life of a run, so a length increase across exactly this call
            // is sound evidence the call's OWN content is what was rejected.
            let rejected_submissions_before = outcome.session.rejected_submissions.len();
            let cached_run = (dispatch_attempt == 0)
                .then(|| take_cached_wave_run(&mut pending_wave_cache, &outcome.session))
                .flatten();
            if cached_run.is_none() {
                report.frames_attempted += 1;
            }
            progress(
                input.progress,
                &format!("frame started {}@{}", role, plan.harness_id),
            );
            dispatch_attempt += 1;
            let narrator_resolution = narrator_config(
                &input,
                profile,
                narrator_plan.as_ref(),
                NarratorFrameContext {
                    session: &outcome.session,
                    item_id: frame.item_id.as_deref(),
                    task_label: &frame.title,
                    trace_sequence: &narrator_trace_sequence,
                },
                &narrator_warm_pool,
                &worktree_env,
                confinement_payloads.as_ref(),
            );
            for capability in narrator_resolution.unsupported_confinement {
                push_capability(&mut report, capability);
            }
            let live_output = live_harness_output(
                input.progress,
                role,
                &plan.harness_id,
                narrator_resolution.config,
                run_panel.0.as_ref(),
                narrator_tokens,
                activity,
            );
            // Opt-in bounded concurrency (`--max-in-flight`, P344): on the
            // very first attempt at a branch inside an eligible `parallel`
            // panel wave, a prior loop pass may already have dispatched this
            // branch's harness call concurrently with its siblings (see
            // `attempt_concurrent_wave`) and cached the outcome (taken above,
            // alongside the `frames_attempted` accounting) — reuse it
            // instead of dispatching again. Any retry attempt (`attempt >
            // 0`), and every branch when `max-in-flight` is left at its
            // default of `1`, always falls straight through to the same
            // live dispatch used today.
            let run = match cached_run {
                // The cached entry is exactly what a live dispatch at this
                // same call site would have produced: `Ok` falls straight
                // through. `Err` (IO error or worker panic caught by
                // `attempt_concurrent_wave`) is never silently reinterpreted
                // as a fresh attempt and, per P402
                // (`concurrent-terminal-failure-bypasses-p264`), is never a
                // bare propagated error either — it is routed through the
                // SAME P264 nested-recovery/branch-failure policy a serial
                // rejection already triggers, via the shared
                // `apply_concurrent_terminal_failure` adapter.
                Some(Ok(result)) => result,
                Some(Err(error)) => {
                    apply_concurrent_terminal_failure(
                        &mut report,
                        &input,
                        &worktree_env,
                        &format!("concurrent wave dispatch failed: {error}"),
                    )?;
                    return Ok(report);
                }
                None => {
                    let activity_observer = ctx_traits_io::harness_config::activity_adapter_kind(
                        &harness,
                    )
                    .map(|kind| {
                        activity.observer(
                            kind,
                            frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
                        )
                    });
                    activity.emit(
                        frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
                        ctx_traits_core::procedure::activity::ActivityKind::Dispatching,
                    );
                    run_cli_harness_with_warm_fallback(
                        &mut report,
                        &mut warm_harness_sessions,
                        &mut warm_harness_respawn_used,
                        &mut warm_harness_disabled,
                        &mut trace_sequence,
                        &mut trace_warned,
                        CliHarnessRun {
                            session_key: &session_key,
                            role,
                            harness_id: &plan.harness_id,
                            argv: argv.clone(),
                            env_overlay: worktree_env.clone(),
                            env_remove: agent_dispatch::harness_env_remove(&harness),
                            warm_argv: warm_argv.clone(),
                            prompt: prompt.clone(),
                            prompt_delivery: prompt_delivery.clone(),
                            timeout_ms: budget.frame_seconds.saturating_mul(1000),
                            idle_timeout_ms: budget
                                .idle_seconds
                                .map(|seconds| seconds.saturating_mul(1000)),
                            capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
                            stream: cli.stream(),
                            stdout_observer: combine_stdout_observers(
                                live_output.as_ref().map(LiveHarnessOutput::observer),
                                activity_observer,
                            ),
                            tick_observer: live_output
                                .as_ref()
                                .and_then(LiveHarnessOutput::tick_observer),
                            exec_dir: input.execution_dir,
                            confinement: confinement_trace,
                            sandbox: spawn_sandbox.clone(),
                            trace: HarnessTraceContext {
                                run_id: outcome.session.run_id.as_str(),
                                session_id: outcome.session.session_id.as_str(),
                                item_id: frame.item_id.as_deref(),
                                frame_title: &frame.title,
                                attempt: dispatch_attempt,
                            },
                            work_total: work_total.clone(),
                            token_panel: run_panel.0.clone(),
                        },
                    )?
                }
            };
            if live_output.is_none() {
                emit_output_progress(
                    input.progress,
                    output_id,
                    &run.stdout,
                    role,
                    &plan.harness_id,
                );
            }
            report.events.push(DriveEvent {
                event: "harness-run".to_string(),
                role: Some(role.to_string()),
                harness: Some(plan.harness_id.clone()),
                detail: format!(
                    "exit={} timed-out={} idle-timed-out={} stdout-truncated={} stderr-truncated={} argv={}",
                    crate::app::presentation::optional(run.exit_code),
                    run.timed_out,
                    run.idle_timed_out,
                    run.stdout_truncated,
                    run.stderr_truncated,
                    crate::app::presentation::argv_display(&run.argv)
                ),
                duration_ms: Some(run.duration_ms),
            });
            let provider_error_stream_events = provider_error_stream_events(output_id, &run.stdout);
            let provider_error_classification = classify_provider_error(
                output_id,
                &run.stdout,
                &run.stderr,
                run.exit_code,
                &provider_error_stream_events,
            );
            if let Some(ProviderErrorClassification::InvalidModel(detail)) =
                &provider_error_classification
            {
                // A model the harness cannot resolve answers identically on
                // every retry (it never spawns a real turn), so fail now
                // instead of burning correction retries confirming that.
                report.status = "blocked-harness-model-invalid".to_string();
                let combo = format!(
                    "{role}@{} model={}",
                    plan.harness_id,
                    plan.model.as_deref().unwrap_or("default")
                );
                let full_detail = format!("{combo} {detail}");
                report.events.push(DriveEvent {
                    event: "harness-provider-error".to_string(),
                    role: Some(role.to_string()),
                    harness: Some(plan.harness_id.clone()),
                    detail: full_detail.clone(),
                    duration_ms: Some(run.duration_ms),
                });
                push_capability(
                    &mut report,
                    ctx_traits_core::response::CapabilityReport::unsupported(
                        format!("runtime.harness-execution.{}", plan.harness_id),
                        full_detail,
                    ),
                );
                return Ok(report);
            }
            if let Some(ProviderErrorClassification::CreditsExhausted(credits)) =
                &provider_error_classification
            {
                // Every retry answers identically until the account is topped
                // up, and burns the dying balance doing so (P366 sibling:
                // InvalidModel above). Pause instead of failing: leave the
                // frame unsubmitted so `ctx traits drive --session <session>`
                // resumes it unchanged once credits return.
                apply_credits_pause(
                    &mut report,
                    &frame,
                    CreditsPauseEvent {
                        event_name: "harness-provider-credits-exhausted",
                        role,
                        harness_id: &plan.harness_id,
                        duration_ms: run.duration_ms,
                    },
                    credits,
                );
                return Ok(report);
            }
            if run.killed {
                // P551: a ctrl-c kill must never be retried or misreported as
                // a harness failure — this check sits ahead of every
                // retry/timeout/exit-code branch below.
                report.status = "killed".to_string();
                return Ok(report);
            }
            if run.timed_out || run.idle_timed_out || run.exit_code != Some(0) {
                retry_count += 1;
                if retry_count > budget.max_retries {
                    report.status = if run.idle_timed_out {
                        "idle-timeout".to_string()
                    } else if run.timed_out {
                        "frame-timeout".to_string()
                    } else {
                        "harness-failed".to_string()
                    };
                    let status = report.status.clone();
                    // P402 (`concurrent-terminal-failure-bypasses-p264`): a
                    // live dispatch exhausting its retries while the cursor
                    // sits inside a `parallel` panel or concurrent `for-each`
                    // is a branch/item terminal failure, not a whole-drive
                    // abort — route it through the same shared P264 adapter
                    // a cached wave outcome's failure uses, instead of just
                    // reporting an unrouted capability and returning.
                    if parallel_wave_activation_key(&outcome.session).is_some() {
                        apply_concurrent_terminal_failure(
                            &mut report,
                            &input,
                            &worktree_env,
                            &status,
                        )?;
                        return Ok(report);
                    }
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            format!("runtime.harness-execution.{}", plan.harness_id),
                            status,
                        ),
                    );
                    return Ok(report);
                }
                continue;
            }

            if run.stdout_truncated {
                retry_count += 1;
                if retry_count > budget.max_retries {
                    report.status = "harness-output-truncated".to_string();
                    push_capability(
                        &mut report,
                        ctx_traits_core::response::CapabilityReport::unsupported(
                            format!("runtime.harness-execution.{}", plan.harness_id),
                            "harness stdout exceeded capture limit",
                        ),
                    );
                    return Ok(report);
                }
                let correction = RejectionClass::OutputTruncated.format_correction(
                    &requested,
                    &schema,
                    &[],
                    &BTreeMap::new(),
                    &BTreeSet::new(),
                );
                correction_ordinal += 1;
                let preparation = match prepare_correction_retry(
                    CorrectionRetryContext {
                        prompt_context: &prompt_context,
                        contract: &contract,
                        schema: &schema,
                        harness: &harness,
                        cli,
                        plan: &plan,
                        agent_system,
                        execution_dir: input.execution_dir,
                        confinement_payloads: confinement_payloads.as_ref(),
                        // A truncated response must not resume its possibly
                        // poisoned conversation; retry only after a reset.
                        observed_session_id: None,
                    },
                    &correction,
                    correction_ordinal,
                ) {
                    Ok(preparation) => preparation,
                    Err(capability) => {
                        report.status = "schema-delivery-unsupported".to_string();
                        push_capability(&mut report, capability);
                        return Ok(report);
                    }
                };
                warm_harness_sessions.remove(&session_key);
                warm_harness_respawn_used.remove(&session_key);
                warm_harness_disabled.remove(&session_key);
                harness_sessions.remove(&session_key);
                warm_argv = (session.mode
                    == ctx_traits_io::harness_config::RunSessionMode::Persistent)
                    .then(|| {
                        warm_harness_argv(
                            &harness,
                            cli,
                            &plan,
                            agent_system,
                            WarmPromptKind::Frame,
                            confinement_payloads.as_ref(),
                            None,
                        )
                    })
                    .flatten();
                if let Ok(mut sidecar) =
                    ctx_traits_io::run_branch::read_harness_sessions(&harness_sessions_path)
                {
                    sidecar.sessions.remove(&session_key);
                    let _ = ctx_traits_io::run_branch::write_harness_sessions(
                        &harness_sessions_path,
                        &sidecar,
                    );
                }
                argv = preparation.argv.clone();
                prompt = preparation.prompt.clone();
                push_correction_retry_event(
                    &mut report,
                    role,
                    &plan.harness_id,
                    RejectionClass::OutputTruncated,
                    &correction,
                    &preparation,
                );
                announce_retry(
                    input.progress,
                    run_panel.0.as_ref(),
                    role,
                    &plan.harness_id,
                    retry_count,
                    budget.max_retries,
                    "output truncated; condition-change=fresh-conversation",
                    activity,
                    &frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
                );
                continue;
            }
            if let Some(ProviderErrorClassification::Generic(detail)) =
                &provider_error_classification
            {
                // The provider answers a retried frame identically until the
                // account state changes, so fail now instead of burning
                // correction retries on it.
                report.status = "harness-failed".to_string();
                report.events.push(DriveEvent {
                    event: "harness-provider-error".to_string(),
                    role: Some(role.to_string()),
                    harness: Some(plan.harness_id.clone()),
                    detail: detail.clone(),
                    duration_ms: Some(run.duration_ms),
                });
                push_capability(
                    &mut report,
                    ctx_traits_core::response::CapabilityReport::unsupported(
                        format!("runtime.harness-execution.{}", plan.harness_id),
                        detail.clone(),
                    ),
                );
                return Ok(report);
            }
            let parsed = match parse_harness_output(&run.stdout, output_id, &requested) {
                Ok(parsed) if !parsed.slots.is_empty() => parsed,
                Ok(parsed) => {
                    retry_count += 1;
                    let expected = requested
                        .iter()
                        .map(|slot| slot.property.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    if retry_count > budget.max_retries {
                        report.status = "harness-output-invalid".to_string();
                        push_capability(
                            &mut report,
                            ctx_traits_core::response::CapabilityReport::unsupported(
                                format!("runtime.harness-execution.{}", plan.harness_id),
                                format!(
                                    "harness output did not contain the required slot(s): {expected}"
                                ),
                            ),
                        );
                        return Ok(report);
                    }
                    let correction = RejectionClass::MissingSlot.format_correction(
                        &requested,
                        &schema,
                        &[],
                        &BTreeMap::new(),
                        &parsed.observed_keys,
                    );
                    let observed_session_id = parsed.harness_session_id.or_else(|| {
                        harness_stream::stream_harness_session_id(output_id, &run.stdout)
                    });
                    correction_ordinal += 1;
                    let preparation = match prepare_correction_retry(
                        CorrectionRetryContext {
                            prompt_context: &prompt_context,
                            contract: &contract,
                            schema: &schema,
                            harness: &harness,
                            cli,
                            plan: &plan,
                            agent_system,
                            execution_dir: input.execution_dir,
                            confinement_payloads: confinement_payloads.as_ref(),
                            observed_session_id: observed_session_id.as_deref(),
                        },
                        &correction,
                        correction_ordinal,
                    ) {
                        Ok(preparation) => preparation,
                        Err(capability) => {
                            report.status = "schema-delivery-unsupported".to_string();
                            push_capability(&mut report, capability);
                            return Ok(report);
                        }
                    };
                    argv = preparation.argv.clone();
                    prompt = preparation.prompt.clone();
                    push_correction_retry_event(
                        &mut report,
                        role,
                        &plan.harness_id,
                        RejectionClass::MissingSlot,
                        &correction,
                        &preparation,
                    );
                    announce_retry(
                        input.progress,
                        run_panel.0.as_ref(),
                        role,
                        &plan.harness_id,
                        retry_count,
                        budget.max_retries,
                        &format!(
                            "missing slot(s) {expected} (resumed={})",
                            preparation.resumed
                        ),
                        activity,
                        &frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
                    );
                    continue;
                }
                Err(reason) => {
                    retry_count += 1;
                    if retry_count > budget.max_retries {
                        report.status = "harness-output-invalid".to_string();
                        push_capability(
                            &mut report,
                            ctx_traits_core::response::CapabilityReport::unsupported(
                                format!("runtime.harness-execution.{}", plan.harness_id),
                                reason,
                            ),
                        );
                        return Ok(report);
                    }
                    let correction = RejectionClass::UnparseableOutput.format_correction(
                        &requested,
                        &schema,
                        &[],
                        &BTreeMap::new(),
                        &BTreeSet::new(),
                    );
                    let observed_session_id =
                        harness_stream::stream_harness_session_id(output_id, &run.stdout);
                    correction_ordinal += 1;
                    let preparation = match prepare_correction_retry(
                        CorrectionRetryContext {
                            prompt_context: &prompt_context,
                            contract: &contract,
                            schema: &schema,
                            harness: &harness,
                            cli,
                            plan: &plan,
                            agent_system,
                            execution_dir: input.execution_dir,
                            confinement_payloads: confinement_payloads.as_ref(),
                            observed_session_id: observed_session_id.as_deref(),
                        },
                        &correction,
                        correction_ordinal,
                    ) {
                        Ok(preparation) => preparation,
                        Err(capability) => {
                            report.status = "schema-delivery-unsupported".to_string();
                            push_capability(&mut report, capability);
                            return Ok(report);
                        }
                    };
                    argv = preparation.argv.clone();
                    prompt = preparation.prompt.clone();
                    push_correction_retry_event(
                        &mut report,
                        role,
                        &plan.harness_id,
                        RejectionClass::UnparseableOutput,
                        &correction,
                        &preparation,
                    );
                    announce_retry(
                        input.progress,
                        run_panel.0.as_ref(),
                        role,
                        &plan.harness_id,
                        retry_count,
                        budget.max_retries,
                        &format!("unparseable output (resumed={})", preparation.resumed),
                        activity,
                        &frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
                    );
                    continue;
                }
            };
            if let Some(session_id) = parsed.harness_session_id.as_ref()
                && session.mode == ctx_traits_io::harness_config::RunSessionMode::Persistent
                && harness_sessions.get(&session_key) != Some(session_id)
            {
                harness_sessions.insert(session_key.clone(), session_id.clone());
                // P516: written only when the value actually changed for
                // this key, so a steady-state frame (the common case)
                // writes nothing.
                let mut sidecar =
                    ctx_traits_io::run_branch::read_harness_sessions(&harness_sessions_path)
                        .unwrap_or_default();
                sidecar.sessions.insert(
                    session_key.clone(),
                    ctx_traits_io::run_branch::HarnessSessionEntry {
                        harness_id: plan.harness_id.clone(),
                        exec_dir: input.execution_dir.map(|dir| dir.to_string()),
                        harness_session_id: session_id.clone(),
                    },
                );
                if ctx_traits_io::run_branch::write_harness_sessions(
                    &harness_sessions_path,
                    &sidecar,
                )
                .is_err()
                {
                    report.warnings.push(format!(
                        "could not persist harness session {session_id} for {session_key} \
                             to {harness_sessions_path}; a resume will start cold"
                    ));
                }
            }
            // `observed_keys` only records failed object candidates. A complete
            // top-level object can therefore have no observed keys while its
            // submitted slots still provide content-rejection shape evidence.
            let received_shapes = received_slot_shapes(&parsed.slots, &requested, &schema);
            let response = submit_harness_output(
                &input,
                &frame,
                HarnessSubmissionEvidence {
                    role,
                    harness_id: &plan.harness_id,
                    version: probe_version(&outcome.session, &plan.harness_id),
                    fallback_version: Some(probe.version.clone()),
                    transport: "cli",
                    duration_ms: run.duration_ms,
                },
                parsed.slots,
                &worktree_env,
                current_elapsed_seconds(),
                run_panel.0.as_ref(),
            )?;
            report.final_session_status = Some(response.response.status.clone());
            if !matches!(
                response.response.response_kind,
                ctx_traits_core::procedure::session::CallResponseKind::RejectedCorrectionRequired
            ) {
                // P402: only now — after the ordinary parent-ledger write
                // this cache hit fed into has already durably succeeded
                // (`submit_harness_output` above persisted it) — mark the
                // corresponding durable sidecar terminal, so a later resume
                // never replays it again. Best-effort: a sidecar write
                // failure here must never fail an otherwise-successful
                // frame acceptance.
                //
                // `response_kind` not being `RejectedCorrectionRequired` does
                // NOT by itself mean this call's content was accepted: a
                // P264 `skip`/`park`/`panel-fail` branch-failure route
                // resolves the failure and advances the cursor (so the
                // response looks like an ordinary `AcceptedNextFrame`/
                // `AcceptedCompleted`) while still having rejected the
                // content that triggered it — see
                // `rejected_submissions_before`'s doc above. Mark those two
                // outcomes distinctly so an audit reader never mistakes a
                // routed rejection of a cached outcome for a normal apply.
                if let Some((wave_key, offset)) = cached_wave_identity.as_ref()
                    && let Ok(ledger_path) = ctx_traits_io::run_session::resolve_session_path(
                        input.session,
                        input.session_store,
                    )
                {
                    let sidecar_path =
                        ctx_traits_io::run_branch::sidecar_path(&ledger_path, wave_key, *offset);
                    let content_was_rejected = response.response.session.rejected_submissions.len()
                        > rejected_submissions_before;
                    let _ = if content_was_rejected {
                        ctx_traits_io::run_branch::mark_rejected_attempt(&sidecar_path)
                    } else {
                        ctx_traits_io::run_branch::mark_applied(&sidecar_path)
                    };
                    // P402 (`p402-proof-absent-and-tests-misplaced`):
                    // this cached wave unit's parent-ledger write has
                    // just durably succeeded and its sidecar is now
                    // terminal (`applied`/`rejected-attempt`), while any
                    // remaining sibling(s) still in `pending_wave_cache`
                    // for this same `wave_key` are still only
                    // `completed`/`terminal-failure` — unapplied. A
                    // no-op outside the concurrency-proof fixtures; see
                    // `test_only_checkpoint`.
                    if pending_wave_cache
                        .get(wave_key)
                        .is_some_and(|remaining| !remaining.is_empty())
                    {
                        test_only_checkpoint(TESTHOOK_CHECKPOINT_ONE_APPLIED);
                    }
                }
                if let Err(error) = write_harness_decision(&HarnessDecision {
                    run_id: outcome.session.run_id.as_str(),
                    sequence: trace_sequence,
                    item_id: frame.item_id.as_deref(),
                    frame_title: &frame.title,
                    role,
                    accepted_slot_values: &response.response.accepted_slot_values,
                }) {
                    warn_trace_once(&mut report, &mut trace_warned, &error);
                }
            }
            if let Some(failure) = response.command_failure.as_ref() {
                // The frame's output was accepted and persisted; a trailing
                // command step failed. End the drive with the real cause —
                // re-dispatching the frame would re-buy accepted output.
                report.frames_accepted += 1;
                activity
                    .finish_frame(&frame.item_id.clone().unwrap_or_else(|| frame.title.clone()));
                emit_structured_completion_progress(
                    input.progress,
                    &loaded_trait.trait_ref,
                    &frame,
                    &response.response.accepted_slot_values,
                    &response.response.session.accepted_output_port_values,
                    &frame.title,
                    role,
                );
                match live_output {
                    Some(live_output) => live_output.finish_accepted(
                        &format!("sequence step completed {role}@{}", plan.harness_id),
                        &response.response.session,
                    ),
                    None => {
                        refresh_existing_run_panel(
                            run_panel.0.as_ref(),
                            &response.response.session,
                        );
                    }
                }
                command_failure_event(&mut report, failure);
                return Ok(report);
            }
            if !matches!(
                response.response.response_kind,
                ctx_traits_core::procedure::session::CallResponseKind::RejectedCorrectionRequired
            ) {
                // Accepted (next frame or completed) — or a post-acceptance
                // control-flow stop (loop exhausted → blocked, procedure
                // failed): the submission itself was accepted, so only a real
                // rejection warrants a correction retry. Anything else breaks
                // to the outer loop, which reads the session status and
                // reports the stop truthfully.
                report.frames_accepted += 1;
                activity
                    .finish_frame(&frame.item_id.clone().unwrap_or_else(|| frame.title.clone()));
                emit_structured_completion_progress(
                    input.progress,
                    &loaded_trait.trait_ref,
                    &frame,
                    &response.response.accepted_slot_values,
                    &response.response.session.accepted_output_port_values,
                    &frame.title,
                    role,
                );
                match live_output {
                    Some(live_output) => live_output.finish_accepted(
                        &format!("sequence step completed {role}@{}", plan.harness_id),
                        &response.response.session,
                    ),
                    None => {
                        progress(
                            input.progress,
                            &format!("sequence step completed {role}@{}", plan.harness_id),
                        );
                        progress_finish(input.progress, role, &plan.harness_id);
                    }
                }
                break;
            }
            // P402 `durable-sidecars-not-connected`: an ordinary
            // `RejectedCorrectionRequired` is still a DURABLE consumption of
            // this cached wave outcome — the rejection persisted and
            // re-digested the parent ledger (see below). If we left the
            // sidecar `completed`, a later resume would replay this exact
            // outcome again against a cursor that already consumed it. Mark it
            // terminal (`rejected-attempt`) now, exactly as the P264-routed
            // rejection branch above does for an accepted-but-rejected route,
            // so it is never replayed. Best-effort: a sidecar write failure
            // here must not fail the correction retry itself. Only meaningful
            // on the first attempt, when this frame actually consumed a cached
            // wave outcome (`cached_wave_identity`).
            if let Some((wave_key, offset)) = cached_wave_identity.as_ref()
                && let Ok(ledger_path) = ctx_traits_io::run_session::resolve_session_path(
                    input.session,
                    input.session_store,
                )
            {
                let sidecar_path =
                    ctx_traits_io::run_branch::sidecar_path(&ledger_path, wave_key, *offset);
                let _ = ctx_traits_io::run_branch::mark_rejected_attempt(&sidecar_path);
            }
            refresh_existing_run_panel(run_panel.0.as_ref(), &response.response.session);
            // P464: `persist_session` distinguishes a persisted content
            // rejection (the submitted content was wrong; core already
            // digested and re-templated the frame) from a non-persisting
            // stale-identity rejection (this submission targeted state the
            // session had already moved past; nothing was written).
            let class = if response.response.persist_session {
                RejectionClass::ContentRejection
            } else {
                RejectionClass::StaleIdentity
            };
            // A persisted rejection re-digests the ledger and re-attaches a
            // fresh call template to the session's frame. Resubmitting through
            // the pre-rejection template can only bounce off the state-digest
            // preflight — invisibly, since identity rejections are
            // non-persisting — until the retry budget burns out
            // (one real schema rejection, then three VALID corrections
            // rejected as stale, seen live). Re-read the ledger on EVERY
            // rejection rather than trusting `response.response.session.next_frame`
            // alone: a concurrent caller may have advanced the session
            // further still, or a non-persisting preflight rejection may
            // supply no frame at all, and only a fresh read is authoritative
            // either way. Pass no fresh elapsed-seconds observation into this
            // read (`None`, never `current_elapsed_seconds()`): each such
            // observation only ratchets the persisted elapsed-seconds
            // forward, and for a harness whose real dispatch latency is
            // nontrivial, issuing a fresh one on every retry — only to have
            // the eventual resubmission itself take further real time —
            // chases a moving target instead of ever settling (confirmed
            // against a real, slow-harness fixture: observing elapsed
            // seconds here repeatedly failed a same-position retry that a
            // plain, unobserved re-read resolves cleanly).
            let Some((refreshed_session, refreshed_frame)) = refresh_frame_for_retry(&input, None)?
            else {
                continue 'frames;
            };
            // The refresh above only proves SOME frame still exists — never
            // that it is the same logical procedure position this attempt
            // targeted. A concurrent caller (or a rejection that itself
            // advances the cursor) can leave the ledger on a genuinely
            // different frame, with its own role/harness/contract/schema;
            // resubmitting THIS attempt's correction there would wrong-role
            // reject it or land a stale answer into a frame whose slot names
            // merely happen to match. When the position differs, abandon this
            // in-loop retry and let the outer per-frame loop resolve the new
            // frame's role/harness/contract/schema from scratch, exactly like
            // any other fresh dispatch — never keep dispatching the old
            // conversation/harness against it.
            if !same_frame_position(&frame, &refreshed_frame) {
                // No correction is ever dispatched here — resubmitting THIS
                // attempt's correction against a frame it was never prepared
                // for is exactly the wrong-role/stale-answer hazard this
                // branch exists to avoid. Record classification-only
                // evidence, distinct from `correction-retry`, which must
                // never claim a correction was sent when none was.
                announce_rejection_boundary_abandoned(
                    input.progress,
                    run_panel.0.as_ref(),
                    role,
                    &plan.harness_id,
                    class.label(),
                );
                push_rejection_boundary_abandoned_event(
                    &mut report,
                    role,
                    &plan.harness_id,
                    class.label(),
                );
                refresh_existing_run_panel(run_panel.0.as_ref(), &refreshed_session);
                continue 'frames;
            }
            frame = refreshed_frame;
            // The refreshed call template may carry updated resolved values.
            // Recompose every full-contract component from that authoritative
            // session before the escalation retry can dispatch it.
            prompt_context = resolved_frame_prompt(&loaded_trait, &refreshed_session, &frame, &[])?;
            requested = requested_outputs(&frame)?;
            schema = requested_output_schema(&requested, &loaded_trait);
            contract = frame_contract_section(&frame);
            refresh_existing_run_panel(run_panel.0.as_ref(), &refreshed_session);
            let observed_session_id = parsed.harness_session_id.clone();
            if class.handling() == RejectionHandling::RuntimeRedispatch {
                argv = harness_argv(
                    &harness,
                    cli,
                    &plan,
                    agent_system,
                    HarnessArgvAttempt {
                        schema: Some(&schema),
                        harness_session_id: observed_session_id
                            .as_ref()
                            .or_else(|| harness_sessions.get(&session_key)),
                        exec_dir: input.execution_dir,
                        confinement: confinement_payloads.as_ref(),
                    },
                );
                warm_argv = (session.mode
                    == ctx_traits_io::harness_config::RunSessionMode::Persistent)
                    .then(|| {
                        warm_harness_argv(
                            &harness,
                            cli,
                            &plan,
                            agent_system,
                            WarmPromptKind::Frame,
                            confinement_payloads.as_ref(),
                            observed_session_id
                                .as_ref()
                                .or_else(|| harness_sessions.get(&session_key)),
                        )
                    })
                    .flatten();
                prompt = frame_prompt(&prompt_context, &contract, &schema, None);
                if let Some(system) = agent_system.filter(|_| cli.system_prompt_flag.is_none()) {
                    prompt = format!("{system}\n\n{prompt}");
                }
                if report.frames_attempted < budget.max_frames || !pending_wave_cache.is_empty() {
                    push_runtime_redispatch_event(
                        &mut report,
                        role,
                        &plan.harness_id,
                        class.label(),
                        retry_count,
                    );
                    announce_runtime_redispatch(
                        input.progress,
                        run_panel.0.as_ref(),
                        role,
                        &plan.harness_id,
                        class.label(),
                        retry_count,
                    );
                }
                continue;
            }
            retry_count += 1;
            if retry_count > budget.max_retries {
                report.status = "rejected".to_string();
                progress(
                    input.progress,
                    &format!(
                        "frame rejected ({}) {}@{}: retry budget exhausted",
                        class.label(),
                        role,
                        plan.harness_id
                    ),
                );
                return Ok(report);
            }
            let correction = class.format_correction(
                &requested,
                &schema,
                &response.response.schema_validation,
                &received_shapes,
                &parsed.observed_keys,
            );
            correction_ordinal += 1;
            let preparation = match prepare_correction_retry(
                CorrectionRetryContext {
                    prompt_context: &prompt_context,
                    contract: &contract,
                    schema: &schema,
                    harness: &harness,
                    cli,
                    plan: &plan,
                    agent_system,
                    execution_dir: input.execution_dir,
                    confinement_payloads: confinement_payloads.as_ref(),
                    observed_session_id: observed_session_id.as_deref(),
                },
                &correction,
                correction_ordinal,
            ) {
                Ok(preparation) => preparation,
                Err(capability) => {
                    report.status = "schema-delivery-unsupported".to_string();
                    push_capability(&mut report, capability);
                    return Ok(report);
                }
            };
            argv = preparation.argv.clone();
            prompt = preparation.prompt.clone();
            push_correction_retry_event(
                &mut report,
                role,
                &plan.harness_id,
                class,
                &correction,
                &preparation,
            );
            let why = format!(
                "{} resumed={}{}",
                class.label(),
                preparation.resumed,
                preparation
                    .cold_start_reason
                    .as_deref()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default()
            );
            announce_retry(
                input.progress,
                run_panel.0.as_ref(),
                role,
                &plan.harness_id,
                retry_count,
                budget.max_retries,
                &why,
                activity,
                &frame.item_id.clone().unwrap_or_else(|| frame.title.clone()),
            );
        }
    }
}

impl DriveReport {
    /// The one place a `DriveReport.status` string becomes a panel's closing
    /// tone: `"completed"` is the only value that reads as `Passed`
    /// (`Tone::Pass`), every other status — `"harness-failed"`,
    /// `"interrupted"`, `"max-frames-exhausted"`, and the rest — reads as
    /// `Blocked` (`Tone::Fail`), matching `PanelStatus`'s own contract that
    /// anything short of passing renders as a stop. Every caller that turns
    /// a `DriveReport` into a panel (`print_report` here, and `run.rs`'s
    /// end-of-run summary) must go through this accessor rather than
    /// re-deriving the mapping, so the two panels can never disagree about
    /// what one drive's status means.
    pub(crate) fn panel_status(&self) -> PanelStatus {
        if self.status == "completed" {
            PanelStatus::Passed(self.status.clone())
        } else {
            PanelStatus::Blocked(self.status.clone())
        }
    }
}

pub fn print_report(report: &DriveReport) -> crate::Result<()> {
    let status = report.panel_status();
    let mut panel = Panel::new("ctx", "drive", status)
        .row(PanelRow::toned(
            "session",
            report.session.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "frames-attempted",
            report.frames_attempted.to_string(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "frames-accepted",
            report.frames_accepted.to_string(),
            RowTone::Default,
        ));
    if let Some(status) = &report.final_session_status {
        panel = panel.row(PanelRow::toned(
            "final-session-status",
            format!("{status:?}"),
            RowTone::Default,
        ));
    }
    for warning in &report.warnings {
        panel = panel.row(PanelRow::toned("warning", warning, RowTone::Fail));
    }
    for event in &report.events {
        panel = panel.row(PanelRow::toned(
            "event",
            format!(
                "{} role={} harness={} {}",
                event.event,
                event.role.as_deref().unwrap_or("-"),
                event.harness.as_deref().unwrap_or("-"),
                event.detail
            ),
            RowTone::Default,
        ));
    }
    for capability in &report.capabilities {
        panel = panel.row(PanelRow::toned(
            "capability",
            format!(
                "{} supported={} {}",
                capability.capability,
                capability.supported,
                capability.reason.as_deref().unwrap_or("")
            ),
            RowTone::Default,
        ));
    }
    emit_human(
        false,
        &panel,
        crate::app::presentation::HumanOutputMode::Compact,
        || Ok(()),
    )?;
    if let Some(pause) = &report.credits_pause {
        print_credits_pause(pause, &report.session)?;
    }
    if let Some(merge) = &report.merge {
        crate::app::merge::print_report(merge)?;
    }
    Ok(())
}

/// One stable rendering of a credits-exhaustion pause, shared by standalone
/// `drive`, verbose `session start`, and non-verbose `session start` — so no
/// surface degrades to a bare status string that hides which provider, role,
/// and frame paused, or how to resume.
pub fn print_credits_pause(
    pause: &ctx_traits_core::procedure::runtime::ProviderCreditsPause,
    session: &str,
) -> crate::Result<()> {
    let panel = Panel::new(
        "ctx",
        "drive",
        PanelStatus::Blocked("paused (credits)".to_string()),
    )
    .row(PanelRow::toned(
        "provider",
        pause.provider.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "role",
        pause.role.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "frame",
        pause_frame_label(pause),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "top-up",
        pause.top_up_url.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "detail",
        pause.detail.as_str(),
        RowTone::Default,
    ))
    .next(PanelRow::toned(
        "resume",
        format!("ctx traits drive --session {session}"),
        RowTone::Default,
    ));
    emit_human(
        false,
        &panel,
        crate::app::presentation::HumanOutputMode::Compact,
        || Ok(()),
    )
}

/// The paused frame's identifying label: its title when non-empty, else the
/// frame's item id, else its `run_index` — a stable position every
/// dispatched frame carries — so the printed pause message always names the
/// frame instead of rendering a blank `frame: ` line, and two different
/// unnamed, id-less frames still print distinct labels.
fn pause_frame_label(pause: &ctx_traits_core::procedure::runtime::ProviderCreditsPause) -> String {
    if !pause.frame_title.is_empty() {
        return pause.frame_title.clone();
    }
    if let Some(item_id) = &pause.frame_item_id {
        return item_id.clone();
    }
    format!("sequence position {}", pause.frame_run_index)
}

/// Resolve the effective drive budget. Precedence (P312): CLI flags >
/// trait-package `config.toml` sidecar budget > built-in defaults.
/// `sidecar_budget` is `ResolvedRuntimeAssignments::budget`, which
/// `resolve_trait_runtime_assignments` already sources from the sidecar alone.
fn budget_from(
    sidecar_budget: &ctx_traits_io::harness_config::RunProfileBudget,
    input: &DriveInputs<'_>,
) -> Budget {
    let total_seconds = input
        .total_seconds
        .or(sidecar_budget.total_seconds)
        .unwrap_or(DEFAULT_TOTAL_SECONDS);
    Budget {
        max_frames: input
            .max_frames
            .or(sidecar_budget.max_frames)
            .unwrap_or(DEFAULT_MAX_FRAMES),
        frame_seconds: input
            .frame_seconds
            .or(sidecar_budget.frame_seconds)
            .unwrap_or(DEFAULT_FRAME_SECONDS),
        total_seconds,
        max_retries: input
            .max_retries
            .or(sidecar_budget.max_retries)
            .unwrap_or(DEFAULT_MAX_RETRIES),
        attach_wait_seconds: input
            .attach_wait_seconds
            .or(sidecar_budget.attach_wait_seconds)
            .unwrap_or(total_seconds),
        idle_seconds: input.idle_seconds.or(sidecar_budget.idle_seconds),
    }
}

/// Resolve `base` down to the dispatched role's own effective per-frame
/// budget (P475). Precedence: CLI flags > `.ctx/config.toml [run]` > package
/// `config.toml [budget]` sidecar (both folded into `sidecar_budget`,
/// `profile.budget`, which stays un-defaulted so a role budget is never
/// masked by [`DEFAULT_FRAME_SECONDS`]/[`DEFAULT_MAX_RETRIES`] themselves) >
/// `[agent.role.<role>].budget` > built-in default. `max-frames`,
/// `total-seconds`, and `attach-wait-seconds` stay whole-run — they have no
/// per-seat meaning and are copied through unchanged from `base`.
fn frame_budget(
    base: &Budget,
    input: &DriveInputs<'_>,
    sidecar_budget: &ctx_traits_io::harness_config::RunProfileBudget,
    role_budget: &ctx_traits_io::harness_config::RoleBudget,
) -> Budget {
    Budget {
        max_frames: base.max_frames,
        frame_seconds: input
            .frame_seconds
            .or(sidecar_budget.frame_seconds)
            .or(role_budget.frame_seconds)
            .unwrap_or(DEFAULT_FRAME_SECONDS),
        total_seconds: base.total_seconds,
        max_retries: input
            .max_retries
            .or(sidecar_budget.max_retries)
            .or(role_budget.max_retries)
            .unwrap_or(DEFAULT_MAX_RETRIES),
        attach_wait_seconds: base.attach_wait_seconds,
        idle_seconds: input
            .idle_seconds
            .or(sidecar_budget.idle_seconds)
            .or(role_budget.idle_seconds),
    }
}

fn wait_for_attach_advance(
    input: &DriveInputs<'_>,
    report: &mut DriveReport,
    role: &str,
    initial_digest: &str,
    budget: &Budget,
    run_panel: Option<&run_view::RunPanel>,
    attach_wait_paused: &std::cell::Cell<Duration>,
) -> crate::Result<bool> {
    let session_path =
        ctx_traits_io::run_session::resolve_session_path(input.session, input.session_store)?;
    let started = Instant::now();
    progress(input.progress, &format!("attach wait started {role}"));
    // Every exit path below accrues this wait's wall time into
    // `attach_wait_paused` before returning: waiting for an external actor
    // (human or another harness) to advance the ledger is not active drive
    // work and must not count toward `elapsed-seconds-at-least`.
    loop {
        if crate::app::interrupt::is_interrupted() {
            report.status = "interrupted".to_string();
            attach_wait_paused.set(attach_wait_paused.get() + started.elapsed());
            return Ok(false);
        }
        if started.elapsed() >= Duration::from_secs(budget.attach_wait_seconds) {
            report.status = "attach-wait-expired".to_string();
            report.events.push(DriveEvent {
                event: "attach-wait-expired".to_string(),
                role: Some(role.to_string()),
                harness: Some("attach".to_string()),
                detail: format!("digest unchanged after {}s", budget.attach_wait_seconds),
                duration_ms: Some(started.elapsed().as_millis()),
            });
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "runtime.attach-wait",
                    format!("attach wait expired for role {role}"),
                ),
            );
            attach_wait_paused.set(attach_wait_paused.get() + started.elapsed());
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(500));
        if let Some(panel) = run_panel {
            panel.tick();
        }
        let session = ctx_traits_io::run_session::read_run_session(&session_path)?;
        if session.state_digest.as_str() != initial_digest {
            report.events.push(DriveEvent {
                event: "attach-wait-advanced".to_string(),
                role: Some(role.to_string()),
                harness: Some("attach".to_string()),
                detail: format!("digest advanced to {}", session.state_digest),
                duration_ms: Some(started.elapsed().as_millis()),
            });
            progress(input.progress, &format!("attach advanced {role}"));
            attach_wait_paused.set(attach_wait_paused.get() + started.elapsed());
            return Ok(true);
        }
    }
}

fn refresh_run_panel(
    run_panel: &mut Option<run_view::RunPanel>,
    input: &mut DriveInputs<'_>,
    session: &ctx_traits_core::procedure::session::Session,
) {
    if input.progress != cli::DriveProgress::Tui {
        return;
    }
    if let Some(panel) = run_panel.as_ref() {
        panel.refresh(session);
        return;
    }
    match create_run_panel(input, session) {
        Ok(panel) => *run_panel = Some(panel),
        Err(err) => {
            input.progress = cli::DriveProgress::Status;
            eprintln!("run tui unavailable; falling back to status progress: {err}");
        }
    }
}

fn create_run_panel(
    input: &mut DriveInputs<'_>,
    session: &ctx_traits_core::procedure::session::Session,
) -> crate::Result<run_view::RunPanel> {
    let loaded = ctx_traits_io::run::load_trait_for_session(input.file, None, session, "drive")?;
    let plan = ctx_traits_core::procedure::run::plan_procedure_run(
        &loaded.trait_ref,
        session.run_id.clone(),
    )?;
    let panel = match input.startup.take().and_then(|view| view.into_pane()) {
        Some(pane) => run_view::RunPanel::new_with_pane(
            loaded.trait_ref.name.as_str().to_string(),
            loaded.trait_ref,
            plan,
            session.clone(),
            pane,
        ),
        None => run_view::RunPanel::new(
            loaded.trait_ref.name.as_str().to_string(),
            loaded.trait_ref,
            plan,
            session.clone(),
        )
        .map_err(|source| crate::Error::Command {
            message: format!("start ratatui run pane: {source}"),
        })?,
    };
    Ok(panel)
}

/// P552: claim and dispatch the one permitted narrator session-title call for
/// this drive. This runs for every driven session regardless of `--progress`
/// mode — including a dashboard-spawned `--progress none` drive, which has no
/// `RunPanel` to derive a prompt from or to repaint — so the prompt context
/// and narrator resolution are both independent of `run_panel`; the panel
/// (when one exists) is only refreshed after a successful result. A no-op for
/// a session whose title was already claimed (resolved or permanently failed)
/// by an earlier invocation, and for a repository with no resolvable narrator
/// seat — each of those degrades to a permanently blank title row, never a
/// placeholder. Dispatches synchronously (bounded by the narrator's own
/// timeout) rather than on a detached thread, so its `record_session_title`
/// write can never race a frame's whole-ledger write from `drive_loop`, which
/// starts only after this call returns.
fn maybe_dispatch_session_title(
    input: &DriveInputs<'_>,
    run_panel: Option<&run_view::RunPanel>,
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
    narrator_tokens: &harness_stream::NarratorTokenTracker,
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
) {
    let claimed = ctx_traits_io::run_session::claim_session_title_attempt(ledger_path)
        .inspect_err(|err| {
            eprintln!("session title claim failed; leaving the title row blank: {err}");
        })
        .unwrap_or(false);
    if !claimed {
        return;
    }
    let Ok(loaded) = ctx_traits_io::run::load_trait_for_session(input.file, None, session, "drive")
    else {
        return;
    };
    let (trait_name, input_text) =
        run_view::title_prompt_context_for(loaded.trait_ref.name.as_str(), session);
    // Reuses `profile` — the exact authoritative, trait-variant-aware
    // resolution `resolve_drive_profile` produced for this driven session
    // (via `resolve_trait_runtime_assignments`) and that `drive_loop` uses
    // for ordinary frame narration — so a repository/trait-qualified
    // narrator seat is honored for the title call exactly as it would be
    // for a driven frame, rather than falling back to the unqualified
    // machine-default seat a second, independent resolution would produce.
    let Ok((worktree_env, confinement_payloads)) =
        resolve_effective_worktree_env(input.execution_dir, profile)
    else {
        return;
    };
    let Some(config) = cold_narrator_config_for_session_title(
        profile,
        ColdNarratorContext {
            run_id: session.run_id.as_str(),
            session_id: session.session_id.as_str(),
            env_overlay: &worktree_env,
            confinement_payloads: confinement_payloads.as_ref(),
            exec_dir: input.execution_dir,
            trace_sequence: &Arc::new(AtomicU64::new(0)),
        },
    ) else {
        // No resolvable narrator seat: the claim above already marked this
        // session permanently title-less, matching the missing-narrator
        // outcome documented on `SessionTitleState`.
        return;
    };
    let prompt = harness_stream::session_title_prompt(&trait_name, &input_text);
    narrator_tokens.begin_call();
    let (result, call_total) = harness_stream::dispatch_narration(&config, prompt);
    narrator_tokens.end_call(call_total);
    if call_total > 0
        && let Some(panel) = run_panel
    {
        panel.add_narrator_tokens(call_total);
    }
    let Ok(title) = result else {
        return;
    };
    if ctx_traits_io::run_session::record_session_title(ledger_path, title.clone()).is_ok()
        && let Some(panel) = run_panel
    {
        panel.set_title(title);
    }
}

fn refresh_existing_run_panel(
    run_panel: Option<&run_view::RunPanel>,
    session: &ctx_traits_core::procedure::session::Session,
) {
    if let Some(panel) = run_panel {
        panel.refresh(session);
    }
}

/// Resolve the ONE effective `[worktree].env` overlay for a drive/resume,
/// plus the P478 write-confinement payloads generated for this worktree
/// spawn (`None` for a non-worktree drive or when confinement is disabled).
/// Empty (no allocation of resolved values) unless a worktree is actually in
/// play (`execution_dir` set) and the resolved profile declared a non-empty
/// overlay; only then is the invocation repository root discovered so
/// repository-relative path values resolve against the invocation checkout.
fn resolve_effective_worktree_env(
    execution_dir: Option<&camino::Utf8Path>,
    profile: &ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
) -> crate::Result<(
    BTreeMap<String, String>,
    Option<ctx_traits_io::confinement::ConfinementPayloads>,
)> {
    if execution_dir.is_none() {
        return Ok((BTreeMap::new(), None));
    }
    Ok(ctx_traits_io::confinement::resolve_worktree_spawn(
        &profile.worktree,
        execution_dir,
    )?)
}

/// The two values every session-lifecycle decision in `drive` keys on,
/// resolved once per frame from the trait's declared session graph (P328).
#[derive(Debug, Clone)]
struct EffectiveSession {
    mode: ctx_traits_io::harness_config::RunSessionMode,
    key: String,
    /// The declared `session:<id>` this frame is bound to, when its binding
    /// is `Shared` — the identity membership compatibility is enforced
    /// against. `None` for an agent-local (`persistent`/`per-frame`/absent)
    /// binding.
    shared_id: Option<String>,
}

/// Resolve the effective session mode/key for `agent_id`'s frame.
///
/// The canonical `[[agent]].session` binding is authoritative when the
/// trait declares one. A `session:<id>` binding shares one `Persistent`
/// session — keyed by the session id alone, with no role or seat folded in,
/// since sharing is the point — across every agent bound to that id. A bare
/// `persistent`/`per-frame` binding is agent-local and keys exactly like the
/// unbound case. Absent a binding, this returns the *same* expression the
/// pre-P328 inline computation used, so every trait in the repo (none
/// declares `[[session]]`) keeps byte-identical dispatch behavior.
fn effective_session(
    trait_ref: &ctx_traits_core::Trait,
    agent_id: &str,
    plan: &AssignmentPlan,
    worktree_env: &BTreeMap<String, String>,
) -> EffectiveSession {
    let role_key = match plan.seat_index {
        Some(seat_index) => format!("{agent_id}.{seat_index}"),
        None => agent_id.to_string(),
    };
    let scoped_key = |key_root: &str| {
        if worktree_env.is_empty() {
            format!("{key_root}:{}", plan.harness_id)
        } else {
            format!(
                "{key_root}:{}\0{}",
                plan.harness_id,
                ctx_traits_io::command::overlay_identity(worktree_env)
            )
        }
    };

    let binding = trait_ref
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .and_then(|agent| agent.session.as_deref())
        .and_then(|raw| ctx_traits_core::r#trait::parse_session_binding(raw, "session").ok());

    match binding {
        Some(ctx_traits_core::r#trait::SessionBinding::Shared(id)) => EffectiveSession {
            mode: ctx_traits_io::harness_config::RunSessionMode::Persistent,
            key: scoped_key(&format!("session:{id}")),
            shared_id: Some(id),
        },
        Some(ctx_traits_core::r#trait::SessionBinding::Persistent) => EffectiveSession {
            mode: ctx_traits_io::harness_config::RunSessionMode::Persistent,
            key: scoped_key(&role_key),
            shared_id: None,
        },
        Some(ctx_traits_core::r#trait::SessionBinding::PerFrame) => EffectiveSession {
            mode: ctx_traits_io::harness_config::RunSessionMode::PerFrame,
            key: scoped_key(&role_key),
            shared_id: None,
        },
        // Decode-time validation already rejects a malformed or unresolved
        // binding, so a loaded trait cannot reach `drive` carrying one in
        // practice; treat it the same as absent rather than panicking.
        None => EffectiveSession {
            mode: plan.session_mode,
            key: scoped_key(&role_key),
            shared_id: None,
        },
    }
}

/// The first dispatched member of a declared `session:<id>`, recorded so a
/// later member can be checked for compatibility (P328 deliverable 3).
#[derive(Debug, Clone)]
struct SessionMembership {
    role: String,
    harness_id: String,
    transport: ctx_traits_io::harness_config::RunTransport,
    attach: bool,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

impl SessionMembership {
    fn from_plan(role: &str, plan: &AssignmentPlan) -> Self {
        Self {
            role: role.to_string(),
            harness_id: plan.harness_id.clone(),
            transport: plan.transport,
            attach: plan.mode == ctx_traits_io::harness_config::RunAssignmentMode::Attach,
            model: plan.model.clone(),
            reasoning_effort: plan.reasoning_effort.clone(),
        }
    }
}

/// Outcome of checking a shared-session member against the session's
/// already-recorded first member.
enum SessionMembershipCheck {
    /// First member of this session id — recorded, nothing to check.
    First,
    /// Compatible with the recorded first member.
    Compatible,
    /// Compatible harness/transport, but a model/reasoning-effort mismatch
    /// a warm process spawned for the first member will silently ignore.
    Warn { first_role: String },
    /// Harness, transport, or attach-ness disagree, or the binding resolves
    /// to a non-CLI transport at all — dispatching would silently split or
    /// cross-wire the declared conversation.
    Refuse {
        first_role: String,
        first_harness_id: String,
    },
}

/// Check (and, for a first member, record) `session_id`'s membership
/// compatibility for the member about to be dispatched. Lazy at dispatch,
/// not preflight: see P328 draft §3.4 for why a speculative preflight sweep
/// is the wrong shape here.
fn check_session_membership(
    session_membership: &mut BTreeMap<String, SessionMembership>,
    session_id: &str,
    role: &str,
    plan: &AssignmentPlan,
) -> SessionMembershipCheck {
    let candidate = SessionMembership::from_plan(role, plan);
    let Some(first) = session_membership.get(session_id) else {
        session_membership.insert(session_id.to_string(), candidate);
        return SessionMembershipCheck::First;
    };
    if candidate.transport != ctx_traits_io::harness_config::RunTransport::Cli
        || first.transport != ctx_traits_io::harness_config::RunTransport::Cli
        || candidate.harness_id != first.harness_id
        || candidate.attach != first.attach
    {
        return SessionMembershipCheck::Refuse {
            first_role: first.role.clone(),
            first_harness_id: first.harness_id.clone(),
        };
    }
    if candidate.model != first.model || candidate.reasoning_effort != first.reasoning_effort {
        return SessionMembershipCheck::Warn {
            first_role: first.role.clone(),
        };
    }
    SessionMembershipCheck::Compatible
}

fn ensure_drive_probe(
    drive_probes: &mut BTreeMap<String, DriveProbe>,
    report: &mut DriveReport,
    session: &ctx_traits_core::procedure::session::Session,
    harness_id: &str,
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    execution_dir: Option<&camino::Utf8Path>,
    env_overlay: &BTreeMap<String, String>,
) -> DriveProbe {
    if let Some(probe) = drive_probes.get(harness_id) {
        return probe.clone();
    }
    if let Some(version) = probe_version(session, harness_id) {
        let probe = DriveProbe {
            version,
            supported: true,
        };
        drive_probes.insert(harness_id.to_string(), probe.clone());
        return probe;
    }
    let mut argv = Vec::with_capacity(harness.version_probe.len() + 1);
    argv.push(harness.bin().to_string());
    argv.extend(harness.version_probe.clone());
    let probe = match ctx_traits_io::command::run_with_env(
        ctx_traits_io::command::RunRequest {
            argv: &argv,
            cwd: Some("project-root"),
            exec_dir: execution_dir,
            success_exit_code: &[0],
            timeout_ms: Some(10_000),
            idle_timeout_ms: None,
            capture_limit: 4096,
            tick_observer: None,
        },
        env_overlay,
    ) {
        Ok(outcome) if outcome.success => {
            let version = if outcome.stdout.trim().is_empty() {
                outcome.stderr.trim().to_string()
            } else {
                outcome.stdout.trim().to_string()
            };
            report.events.push(DriveEvent {
                event: "harness-probe".to_string(),
                role: None,
                harness: Some(harness_id.to_string()),
                detail: format!("bin={} version={}", harness.bin(), version),
                duration_ms: None,
            });
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::supported(format!(
                    "runtime.harness-probe.{harness_id}"
                )),
            );
            DriveProbe {
                version,
                supported: true,
            }
        }
        Ok(outcome) => {
            let reason = format!(
                "harness {harness_id} probe failed: exit={} timed-out={} stderr={}",
                crate::app::presentation::optional(outcome.exit_code),
                outcome.timed_out,
                outcome.stderr.trim()
            );
            report.warnings.push(reason.clone());
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-probe.{harness_id}"),
                    reason,
                ),
            );
            DriveProbe {
                version: "unknown".to_string(),
                supported: false,
            }
        }
        Err(err) => {
            let reason = format!("harness {harness_id} probe failed: {err}");
            report.warnings.push(reason.clone());
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-probe.{harness_id}"),
                    reason,
                ),
            );
            DriveProbe {
                version: "unknown".to_string(),
                supported: false,
            }
        }
    };
    drive_probes.insert(harness_id.to_string(), probe.clone());
    probe
}

/// The persisted session-provenance assignment for `role`, if any — set once
/// at this run's very first invocation (`ctx traits run`, from
/// `prepare_run_assignments`'s already-fallback-resolved plan) and never
/// recomputed on resume. Consulted by [`assignment_for_role`] ahead of
/// re-running automatic fallback, so a resumed dispatch never silently
/// re-routes to a different built-in than the one the session started with
/// just because `PATH` looks different on this invocation.
fn persisted_assignment_plan(
    session: &ctx_traits_core::procedure::session::Session,
    role: &str,
    structural_seat: Option<u32>,
) -> Option<AssignmentPlan> {
    let assignments = session.provenance.agent_assignments.as_ref()?;
    let selected = ctx_traits_core::procedure::session::select_agent_assignment(
        assignments,
        role,
        structural_seat,
    )?;
    Some(AssignmentPlan {
        harness_id: selected.harness.clone(),
        transport: if selected.transport == "mcp" {
            ctx_traits_io::harness_config::RunTransport::Mcp
        } else {
            ctx_traits_io::harness_config::RunTransport::Cli
        },
        mode: if selected.harness == "attach" {
            ctx_traits_io::harness_config::RunAssignmentMode::Attach
        } else {
            ctx_traits_io::harness_config::RunAssignmentMode::Harness
        },
        session_mode: ctx_traits_io::harness_config::RunSessionMode::PerFrame,
        model: selected.model.clone(),
        reasoning_effort: None,
        system_prompt: None,
        extra_args: Vec::new(),
        model_resolution_evidence: selected.model.as_ref().map(|_| selected.evidence.clone()),
        from_session: true,
        seat_index: selected.seat_index,
        list_length: selected.list_length,
    })
}

/// Pick one seat's plan by `structural_seat`, the same selection
/// [`assignment_for_role`] applies at each of its (explicit-config,
/// automatic-fallback) return points.
fn plan_from_seats(
    seats: &[(
        ctx_traits_io::harness_config::ProfileAssignment,
        Option<ctx_traits_io::harness_config::SeatInfo>,
    )],
    structural_seat: Option<u32>,
) -> AssignmentPlan {
    let index = match seats.len() {
        1 => 0,
        len => (structural_seat.unwrap_or(0) as usize) % len,
    };
    let (assignment, seat_info) = seats[index].clone();
    plan_from_assignment(
        assignment,
        seat_info.map(|info| info.seat_index),
        seat_info.map(|info| info.list_length),
    )
}

/// Runtime-assignment precedence for `role` (P427): resolve exactly as
/// before P427 (a real CLI/config assignment wins; failing that, automatic
/// built-in fallback fills the gap) — EXCEPT that when the live result came
/// from automatic fallback (`used_builtin_fallback`) AND a session
/// assignment already persisted for `role` exists, the persisted assignment
/// is dispatched instead of the freshly recomputed live one, so a resumed
/// run never silently re-routes through a different harness OR drops
/// already-resolved routing evidence (a persisted model, say) that a fresh
/// automatic-fallback recomputation would not itself carry — even when the
/// persisted and freshly recomputed harness ids happen to match.
///
/// The live resolution always runs first (rather than checking persisted
/// state up front) specifically so the grouped automatic-selection
/// announcement ([`ResolvedRuntimeAssignments::builtin_fallback_warnings`])
/// still fires for the ordinary case — including the very first, session-
/// creating invocation, where the persisted assignment is simply a copy of
/// what live fallback just computed. When persisted overrides a live
/// selection naming a DIFFERENT harness, that bookkeeping is undone with
/// `discard_builtin_selection` so the operator never sees an announcement
/// for a harness that was not actually dispatched; when the two harness ids
/// match, the announcement still accurately names the harness actually
/// dispatched, so it is left alone. `ensure_builtin_registered` covers the
/// persisted harness id either way, since a resumed invocation's fresh
/// `profile.registry` never itself ran the fallback that would otherwise
/// have registered it.
fn assignment_for_role(
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    session: &ctx_traits_core::procedure::session::Session,
    role: &str,
    structural_seat: Option<u32>,
) -> crate::Result<Option<AssignmentPlan>> {
    let live = if role == ctx_traits_io::harness_config::DEFAULT_SEAT {
        profile
            .resolved_default_assignment()?
            .map(|assignment| plan_from_assignment(assignment, None, None))
    } else {
        let seats = profile.resolved_seats_for_role(role)?;
        if seats.is_empty() {
            None
        } else {
            Some(plan_from_seats(&seats, structural_seat))
        }
    };
    let Some(live) = live else {
        return Ok(persisted_assignment_plan(session, role, structural_seat)
            .inspect(|plan| profile.ensure_builtin_registered(&plan.harness_id)));
    };
    if profile.used_builtin_fallback(role)
        && let Some(persisted) = persisted_assignment_plan(session, role, structural_seat)
    {
        if persisted.harness_id != live.harness_id {
            profile.discard_builtin_selection(&live.harness_id, role);
        }
        profile.ensure_builtin_registered(&persisted.harness_id);
        return Ok(Some(persisted));
    }
    Ok(Some(live))
}

fn plan_from_assignment(
    assignment: ctx_traits_io::harness_config::ProfileAssignment,
    seat_index: Option<u32>,
    list_length: Option<u32>,
) -> AssignmentPlan {
    let model_resolution_evidence = assignment.model_resolution_evidence();
    let ctx_traits_io::harness_config::ProfileAssignment {
        mode,
        harness,
        transport,
        session_mode,
        model,
        reasoning_effort,
        system_prompt,
        extra_args,
        ..
    } = assignment;
    AssignmentPlan {
        harness_id: harness.unwrap_or_else(|| "attach".to_string()),
        transport: transport.unwrap_or(ctx_traits_io::harness_config::RunTransport::Cli),
        mode,
        session_mode: session_mode.unwrap_or_default(),
        model,
        reasoning_effort,
        system_prompt,
        extra_args,
        model_resolution_evidence,
        from_session: false,
        seat_index,
        list_length,
    }
}

fn validate_narrator_assignment(
    profile: &ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    plan: Option<&AssignmentPlan>,
) -> crate::Result<()> {
    // No narrator configured is a valid, deliberate mode: the live panel runs
    // in passthrough and shows the agent's own stream text (owner decision,
    // Group 42.5 F10). Only a PRESENT-but-broken narrator config errors below —
    // that is the silent-no-op P202 rightly feared.
    let Some(plan) = plan else {
        return Ok(());
    };
    agent_dispatch::validate_cli_standing_agent(
        &profile.registry,
        plan.mode,
        &plan.harness_id,
        plan.transport,
        plan.model.as_deref(),
        plan.reasoning_effort.as_deref(),
        "progress narration",
    )
    .map(|_| ())
}

fn install_live_guide(
    panel: Option<&run_view::RunPanel>,
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    exec_dir: Option<&camino::Utf8Path>,
    tokens: harness_stream::OneShotTokenTracker,
    ledger_path: camino::Utf8PathBuf,
) -> crate::Result<()> {
    let Some(panel) = panel else {
        return Ok(());
    };
    let Some(assignment) = profile.resolved_guide_assignment()? else {
        return Ok(());
    };
    let plan = plan_from_assignment(assignment.clone(), None, None);
    let (harness, cli) = agent_dispatch::validate_cli_standing_agent(
        &profile.registry,
        plan.mode,
        &plan.harness_id,
        plan.transport,
        plan.model.as_deref(),
        plan.reasoning_effort.as_deref(),
        "live guide",
    )?;
    // The guide must opt into the narrow one-shot convention; normal argv may
    // carry tool capability and is intentionally never a fallback.
    if cli.narrator_argv.as_ref().is_none_or(Vec::is_empty) {
        return Err(crate::Error::Command {
            message: "live guide requires explicit narrator-argv".to_string(),
        });
    }
    let config = crate::app::guide::GuideConfig {
        harness,
        cli,
        assignment,
        timeout_ms: one_shot_timeout_ms(profile, "guide", DEFAULT_NARRATOR_TIMEOUT_MS),
        exec_dir: exec_dir.map(camino::Utf8PathBuf::from),
        tokens: tokens.clone(),
    };
    panel.set_guide(
        Arc::new(move |question, context| {
            crate::app::guide::dispatch(config.clone(), question, context).map(|reply| reply.text)
        }),
        tokens,
        ledger_path,
    );
    Ok(())
}

fn probe_version(
    session: &ctx_traits_core::procedure::session::Session,
    harness_id: &str,
) -> Option<String> {
    session
        .provenance
        .harness_probes
        .iter()
        .find(|probe| probe.harness_id == harness_id)
        .map(|probe| probe.version.clone())
}

/// Per-attempt extras for [`harness_argv`] — bundled into a struct (rather
/// than four more individual parameters) purely to stay under the arity
/// lint; every field is threaded straight from the caller's own per-attempt
/// state.
struct HarnessArgvAttempt<'a> {
    schema: Option<&'a Value>,
    harness_session_id: Option<&'a String>,
    exec_dir: Option<&'a camino::Utf8Path>,
    /// P478/P517: complete generated confinement payloads for this spawn.
    confinement: Option<&'a ctx_traits_io::confinement::ConfinementPayloads>,
}

fn harness_argv(
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    cli: &ctx_traits_io::harness_config::HarnessCliConvention,
    plan: &AssignmentPlan,
    agent_system: Option<&str>,
    attempt: HarnessArgvAttempt<'_>,
) -> Vec<String> {
    let HarnessArgvAttempt {
        schema,
        harness_session_id,
        exec_dir,
        confinement,
    } = attempt;
    let mut argv = Vec::new();
    argv.push(harness.bin().to_string());
    argv.extend(cli.argv.clone());
    // Spawn cwd is not enough for server-anchored harnesses (opencode resolves
    // the project from its server, not the client cwd), so the execution dir
    // is also passed as an explicit flag when the convention declares one.
    agent_dispatch::append_exec_dir(&mut argv, cli, exec_dir);
    if let (Some(flag), Some(model)) = (cli.model_flag.as_ref(), plan.model.as_ref()) {
        argv.push(flag.clone());
        argv.push(model.clone());
    }
    agent_dispatch::append_reasoning_effort(
        &mut argv,
        harness,
        cli,
        plan.reasoning_effort.as_deref(),
    );
    if let Some(flag) = cli.system_prompt_flag.as_ref() {
        argv.push(flag.clone());
        argv.push(composed_system_prompt(
            agent_system,
            plan.system_prompt.as_deref(),
        ));
    }
    agent_dispatch::append_session_resume(&mut argv, cli, harness_session_id);
    if let (Some(flag), Some(schema)) = (cli.json_schema_flag.as_ref(), schema) {
        argv.push(flag.clone());
        argv.push(schema.to_string());
    }
    argv.extend(plan.extra_args.clone());
    // Must be final: Codex normalizes configured sandbox/bypass selectors
    // from both cli.argv and assignment extras before selecting this mode.
    agent_dispatch::append_confinement(&mut argv, harness, confinement);
    argv
}

/// What kind of correction a retry is responding to, named in every
/// `correction-retry` [`DriveEvent`]. `ContentRejection`/`StaleIdentity` come
/// from `CallResponse.persist_session`; the parser-level variants come from a
/// harness dispatch this drive loop itself could not turn into a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectionClass {
    /// `persist_session = true`: the submitted content itself was rejected
    /// (schema/contract mismatch) and core persisted the rejection.
    ContentRejection,
    /// `persist_session = false`: this submission targeted frame identity the
    /// session had already advanced past; nothing was persisted.
    StaleIdentity,
    OutputTruncated,
    MissingSlot,
    UnparseableOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectionHandling {
    ModelCorrection,
    RuntimeRedispatch,
    FreshConversationCorrection,
}

impl RejectionHandling {
    fn event_label(self) -> &'static str {
        match self {
            Self::ModelCorrection => "model-fixable",
            Self::RuntimeRedispatch => "runtime-caused",
            Self::FreshConversationCorrection => "condition-change=fresh-conversation",
        }
    }
}

impl RejectionClass {
    fn handling(self) -> RejectionHandling {
        match self {
            Self::MissingSlot | Self::UnparseableOutput | Self::ContentRejection => {
                RejectionHandling::ModelCorrection
            }
            Self::StaleIdentity => RejectionHandling::RuntimeRedispatch,
            Self::OutputTruncated => RejectionHandling::FreshConversationCorrection,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ContentRejection => "content-rejection",
            Self::StaleIdentity => "stale-identity",
            Self::OutputTruncated => "output-truncated",
            Self::MissingSlot => "missing-slot",
            Self::UnparseableOutput => "unparseable-output",
        }
    }

    /// Build the model-facing correction from typed request/response evidence.
    /// Validator and runtime diagnostics remain in their reports and ledgers;
    /// none of their free-form wording is delivered back to the harness.
    fn format_correction(
        self,
        requested: &[RequestedSlotKey],
        schema: &Value,
        validations: &[ctx_traits_core::procedure::runtime::SchemaValidation],
        received_shapes: &BTreeMap<String, ReceivedShape>,
        observed_keys: &BTreeSet<String>,
    ) -> String {
        let complete_contract = complete_contract_request(requested);
        match self {
            Self::ContentRejection => {
                let evidence = validations
                    .iter()
                    .filter(|validation| {
                        !matches!(
                            validation.status,
                            ctx_traits_core::procedure::runtime::SchemaStatus::Accepted
                        )
                    })
                    .filter_map(|validation| {
                        let requested = requested
                            .iter()
                            .find(|requested| requested.ref_text == validation.ref_text)?;
                        let schema_ref = validation
                            .schema_ref
                            .as_ref()
                            .map(ToString::to_string)
                            .or_else(|| requested.schema_ref.clone());
                        let received = received_shapes.get(&validation.ref_text);
                        Some(format!(
                            "property `{}` ({}){} received {}; required shape {}",
                            truncate_name(&requested.property),
                            truncate_name(&requested.ref_text),
                            schema_ref
                                .as_deref()
                                .map(|schema_ref| format!(
                                    " with schema {}",
                                    truncate_name(schema_ref)
                                ))
                                .unwrap_or_default(),
                            received
                                .map(|shape| shape.description.as_str())
                                .unwrap_or("an unavailable received shape"),
                            requested_property_shape(schema, &requested.property),
                        ))
                    })
                    .collect::<Vec<_>>();
                if evidence.is_empty() {
                    format!(
                        "The submitted content was not accepted. Use the supplied output schema to repair it. {complete_contract}"
                    )
                } else {
                    let omitted = evidence.len().saturating_sub(MAX_CORRECTION_FIELDS);
                    let evidence = evidence
                        .into_iter()
                        .take(MAX_CORRECTION_FIELDS)
                        .collect::<Vec<_>>()
                        .join("; ");
                    let omitted = if omitted > 0 {
                        format!("; and {omitted} more rejected outputs")
                    } else {
                        String::new()
                    };
                    format!(
                        "The submitted content needs repair: {evidence}{omitted}. {complete_contract}",
                    )
                }
            }
            Self::StaleIdentity => {
                unreachable!("stale identity is runtime-redispatched, never corrected")
            }
            Self::OutputTruncated => format!(
                "The response ended before a complete JSON object was captured. Return a complete object matching the supplied output schema, retaining every required field; shorten only free-form content if necessary. {complete_contract}"
            ),
            Self::MissingSlot => {
                let missing = requested
                    .iter()
                    .filter(|requested| !observed_keys.contains(&requested.property))
                    .map(|requested| requested.property.as_str())
                    .collect::<Vec<_>>();
                let missing = bounded_names(&missing);
                let observed = if observed_keys.is_empty() {
                    "no model-authored top-level properties were observed".to_string()
                } else {
                    bounded_names(&observed_keys.iter().map(String::as_str).collect::<Vec<_>>())
                };
                format!(
                    "Required top-level properties were missing: {missing}. Model-authored top-level properties observed: {observed}. Return one complete object containing every required output. {complete_contract}"
                )
            }
            Self::UnparseableOutput => format!(
                "No complete JSON object could be parsed from the response. Return one complete object matching the supplied output schema, without prose or a code fence. {complete_contract}"
            ),
        }
    }
}

const MAX_CORRECTION_FIELDS: usize = 4;
const MAX_CORRECTION_NAME_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceivedShape {
    description: String,
}

fn complete_contract_request(requested: &[RequestedSlotKey]) -> String {
    let properties = requested
        .iter()
        .map(|requested| requested.property.as_str())
        .collect::<Vec<_>>();
    let properties = bounded_names(&properties);
    format!("Return every requested top-level output in one complete response: {properties}.")
}

fn requested_property_shape(schema: &Value, property: &str) -> String {
    requested_property_schema(schema, property)
        .map(schema_shape)
        .unwrap_or_else(|| "the supplied schema".to_string())
}

fn requested_property_schema<'a>(schema: &'a Value, property: &str) -> Option<&'a Value> {
    schema.get("properties")?.get(property)
}

fn schema_shape(schema: &Value) -> String {
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("the supplied schema");
    if kind != "object" {
        return kind.to_string();
    }
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|fields| fields.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if required.is_empty() {
        "object".to_string()
    } else {
        format!("object requiring {}", bounded_names(&required))
    }
}

fn received_slot_shapes(
    slots: &BTreeMap<String, Value>,
    requested: &[RequestedSlotKey],
    _schema: &Value,
) -> BTreeMap<String, ReceivedShape> {
    requested
        .iter()
        .filter_map(|requested| {
            slots
                .get(&requested.ref_text)
                .map(|value| (requested.ref_text.clone(), received_shape(value)))
        })
        .collect()
}

fn received_shape(value: &Value) -> ReceivedShape {
    ReceivedShape {
        description: json_shape(value),
    }
}

fn json_shape(value: &Value) -> String {
    match value {
        Value::Object(object) if object.is_empty() => "object with no fields".to_string(),
        Value::Object(object) => format!(
            "object with fields: {}",
            bounded_names(&object.keys().map(String::as_str).collect::<Vec<_>>())
        ),
        _ => json_kind(value).to_string(),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn bounded_names(names: &[&str]) -> String {
    let shown = names
        .iter()
        .take(MAX_CORRECTION_FIELDS)
        .map(|name| truncate_name(name))
        .collect::<Vec<_>>()
        .join(", ");
    let omitted = names.len().saturating_sub(MAX_CORRECTION_FIELDS);
    if omitted == 0 {
        shown
    } else {
        format!("{shown}, and {omitted} more")
    }
}

fn truncate_name(name: &str) -> String {
    let mut chars = name.chars();
    let prefix = chars
        .by_ref()
        .take(MAX_CORRECTION_NAME_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

/// Materials [`prepare_correction_retry`] needs to resume a harness
/// conversation or, failing that, produce a visible cold start — bundled
/// because every one of the four CLI correction retry sites (truncated
/// output, missing slot(s), unparseable output, `RejectedCorrectionRequired`)
/// needs the identical set.
struct CorrectionRetryContext<'a> {
    prompt_context: &'a ResolvedFramePrompt,
    contract: &'a str,
    schema: &'a Value,
    harness: &'a ctx_traits_io::harness_config::HarnessDefinition,
    cli: &'a ctx_traits_io::harness_config::HarnessCliConvention,
    plan: &'a AssignmentPlan,
    agent_system: Option<&'a str>,
    execution_dir: Option<&'a camino::Utf8Path>,
    /// P478/P517: this drive/resume's generated confinement payloads
    /// (`None` for a non-worktree drive or when confinement is disabled).
    confinement_payloads: Option<&'a ctx_traits_io::confinement::ConfinementPayloads>,
    /// This attempt's harness conversation id, extracted before slot parsing
    /// so it survives a parse failure or truncated/unusable output. `None`
    /// forces a cold start even when the harness convention declares a
    /// resume mechanism.
    observed_session_id: Option<&'a str>,
}

/// One retry's resolved delivery: the argv/prompt to dispatch next, whether
/// the harness conversation was actually resumed, and — only on a cold
/// start — why resuming was not possible.
struct CorrectionRetryPreparation {
    argv: Vec<String>,
    prompt: String,
    resumed: bool,
    cold_start_reason: Option<String>,
    correction_ordinal: u64,
    rung: CorrectionRung,
    schema_delivery: SchemaDelivery,
}

#[derive(Debug, Clone, Copy)]
enum CorrectionRung {
    ResumedReshape,
    CompleteFrame,
}

impl CorrectionRung {
    fn label(self) -> &'static str {
        match self {
            Self::ResumedReshape => "resumed-reshape",
            Self::CompleteFrame => "complete-frame",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SchemaDelivery {
    Flag,
    Inline,
    FlagAndInline,
}

impl SchemaDelivery {
    fn label(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Inline => "inline",
            Self::FlagAndInline => "flag+inline",
        }
    }
}

fn resolve_correction_delivery(
    correction_ordinal: u64,
    resumed: bool,
    has_schema_flag: bool,
    inline_output_contract: Option<&str>,
) -> Result<(CorrectionRung, SchemaDelivery), ctx_traits_core::response::CapabilityReport> {
    if !has_schema_flag && inline_output_contract.is_none() {
        return Err(ctx_traits_core::response::CapabilityReport::unsupported(
            "runtime.correction-schema-delivery",
            "correction retry blocked: harness has no json-schema-flag and inline output-contract delivery is unavailable",
        ));
    }
    let rung = if resumed && correction_ordinal == 1 {
        CorrectionRung::ResumedReshape
    } else {
        CorrectionRung::CompleteFrame
    };
    let channel = match (rung, has_schema_flag) {
        (CorrectionRung::ResumedReshape, true) => SchemaDelivery::Flag,
        (CorrectionRung::ResumedReshape, false) => SchemaDelivery::Inline,
        (CorrectionRung::CompleteFrame, true) => SchemaDelivery::FlagAndInline,
        (CorrectionRung::CompleteFrame, false) => SchemaDelivery::Inline,
    };
    Ok((rung, channel))
}

/// The one place every CLI correction retry (parser-level or
/// `RejectedCorrectionRequired`) decides whether to resume the harness
/// conversation that produced the rejected answer or fall back to a visible
/// cold start (P464). Resuming requires BOTH a declared `session-flag`/
/// `resume-flag` on the harness's CLI convention AND an observed session id
/// for this attempt — sending correction-only text without both would drop
/// the frame contract, so either gap is an explicit, reported cold start
/// rather than a silent one.
fn prepare_correction_retry(
    ctx: CorrectionRetryContext<'_>,
    correction: &str,
    correction_ordinal: u64,
) -> Result<CorrectionRetryPreparation, ctx_traits_core::response::CapabilityReport> {
    let resume_flag = ctx
        .cli
        .session_flag
        .as_ref()
        .or(ctx.cli.resume_flag.as_ref());
    let resumed = resume_flag.is_some() && ctx.observed_session_id.is_some();
    let inline_output_contract = requested_output_contract_section(ctx.schema);
    let (rung, schema_delivery) = resolve_correction_delivery(
        correction_ordinal,
        resumed,
        ctx.cli.json_schema_flag.is_some(),
        Some(&inline_output_contract),
    )?;
    if let (Some(_), Some(session_id)) = (resume_flag, ctx.observed_session_id) {
        let session_id = session_id.to_string();
        let argv = harness_argv(
            ctx.harness,
            ctx.cli,
            ctx.plan,
            ctx.agent_system,
            HarnessArgvAttempt {
                schema: Some(ctx.schema),
                harness_session_id: Some(&session_id),
                exec_dir: ctx.execution_dir,
                confinement: ctx.confinement_payloads,
            },
        );
        let prompt = match rung {
            CorrectionRung::CompleteFrame => frame_prompt(
                ctx.prompt_context,
                ctx.contract,
                ctx.schema,
                Some(correction),
            ),
            CorrectionRung::ResumedReshape => match schema_delivery {
                SchemaDelivery::Flag => correction.to_string(),
                SchemaDelivery::Inline => format!("{correction}\n\n{}", inline_output_contract),
                SchemaDelivery::FlagAndInline => unreachable!("a reshape cannot use both channels"),
            },
        };
        return Ok(CorrectionRetryPreparation {
            argv,
            prompt,
            resumed: true,
            cold_start_reason: None,
            correction_ordinal,
            rung,
            schema_delivery,
        });
    }
    let cold_start_reason = if resume_flag.is_none() {
        "harness declares no session-flag or resume-flag".to_string()
    } else {
        "no harness session id observed for this attempt".to_string()
    };
    // A resumed conversation already received the role's standing
    // instructions on its first turn; only a cold start (a fresh
    // conversation, or one degraded to the prompt body because the harness
    // has no system-prompt flag) needs them composed in again.
    let mut prompt = frame_prompt(
        ctx.prompt_context,
        ctx.contract,
        ctx.schema,
        Some(correction),
    );
    if let Some(system) = ctx
        .agent_system
        .filter(|_| ctx.cli.system_prompt_flag.is_none())
    {
        prompt = format!("{system}\n\n{prompt}");
    }
    let argv = harness_argv(
        ctx.harness,
        ctx.cli,
        ctx.plan,
        ctx.agent_system,
        HarnessArgvAttempt {
            schema: Some(ctx.schema),
            harness_session_id: None,
            exec_dir: ctx.execution_dir,
            confinement: ctx.confinement_payloads,
        },
    );
    Ok(CorrectionRetryPreparation {
        argv,
        prompt,
        resumed: false,
        cold_start_reason: Some(cold_start_reason),
        correction_ordinal,
        rung,
        schema_delivery,
    })
}

/// Emit retry delivery evidence (P464) for one actual correction retry —
/// never for an attempt that exhausted its budget instead of retrying. The
/// exact correction text is JSON-encoded so a multi-line or quote-bearing
/// correction stays a single well-formed `detail` field rather than breaking
/// the event's own line-oriented rendering.
fn push_correction_retry_event(
    report: &mut DriveReport,
    role: &str,
    harness_id: &str,
    class: RejectionClass,
    correction: &str,
    preparation: &CorrectionRetryPreparation,
) {
    let correction_json = serde_json::to_string(correction).unwrap_or_else(|_| "\"\"".to_string());
    report.events.push(DriveEvent {
        event: "correction-retry".to_string(),
        role: Some(role.to_string()),
        harness: Some(harness_id.to_string()),
        detail: format!(
            "class={} handling={} correction-ordinal={} rung={} schema-delivery={} resumed={} prompt-bytes={} prompt-estimated-tokens={} bare-correction-bytes={} bare-correction-estimated-tokens={} cold-start-reason={} correction={correction_json}",
            class.label(),
            class.handling().event_label(),
            preparation.correction_ordinal,
            preparation.rung.label(),
            preparation.schema_delivery.label(),
            preparation.resumed,
            preparation.prompt.len(),
            ctx_traits_core::discovery_index::estimate_tokens(&preparation.prompt),
            correction.len(),
            ctx_traits_core::discovery_index::estimate_tokens(correction),
            preparation.cold_start_reason.as_deref().unwrap_or("-"),
        ),
        duration_ms: None,
    });
}

fn push_runtime_redispatch_event(
    report: &mut DriveReport,
    role: &str,
    harness_id: &str,
    class: &str,
    retry_count: u64,
) {
    report.events.push(DriveEvent {
        event: "runtime-redispatch".to_string(),
        role: Some(role.to_string()),
        harness: Some(harness_id.to_string()),
        detail: format!(
            "class={class} handling=runtime-caused retry-budget-used={retry_count} correction=none"
        ),
        duration_ms: None,
    });
}

fn announce_runtime_redispatch(
    mode: cli::DriveProgress,
    run_panel: Option<&run_view::RunPanel>,
    role: &str,
    harness_id: &str,
    class: &str,
    retry_count: u64,
) {
    let note = format!(
        "frame rejected ({class}) {role}@{harness_id}: runtime redispatch; retry budget remains {retry_count}"
    );
    progress(mode, &note);
    if let Some(panel) = run_panel {
        panel.push_summary(note);
    }
}

/// Emit boundary-abandon evidence (P464) when a rejection's freshly re-read
/// current frame is a genuinely different procedure position than the one
/// this attempt targeted — never a `correction-retry` event, and never
/// carrying correction text: no correction is dispatched here at all, only
/// classification of the rejection that was just observed.
fn push_rejection_boundary_abandoned_event(
    report: &mut DriveReport,
    role: &str,
    harness_id: &str,
    class: &str,
) {
    report.events.push(DriveEvent {
        event: "rejection-boundary-abandoned".to_string(),
        role: Some(role.to_string()),
        harness: Some(harness_id.to_string()),
        detail: format!(
            "class={class} reason=session advanced to a different frame; abandoning this retry, resuming with a fresh dispatch"
        ),
        duration_ms: None,
    });
}

/// Progress/panel counterpart to [`push_rejection_boundary_abandoned_event`]
/// — deliberately not [`announce_retry`], whose "correction retry N/M"
/// framing would misstate that a correction was sent for this frame.
fn announce_rejection_boundary_abandoned(
    mode: cli::DriveProgress,
    run_panel: Option<&run_view::RunPanel>,
    role: &str,
    harness_id: &str,
    class: &str,
) {
    let note = format!(
        "frame rejected ({class}) {role}@{harness_id}: session advanced to a different frame; abandoning this retry, resuming with a fresh dispatch"
    );
    progress(mode, &note);
    if let Some(panel) = run_panel {
        panel.push_summary(note);
    }
}

/// P464: whether `a` and `b` are the same logical procedure position — the
/// exact identity a call template's own `expected-sequence-item-id`/
/// `expected-run-index`/`expected-source-index`/`expected-position-path`
/// preflight already checks. `position_path` alone is NOT sufficient: it is
/// empty for every frame in a flat (non-nested) procedure, so two distinct
/// top-level steps would otherwise compare equal.
fn same_frame_position(
    a: &ctx_traits_core::procedure::runtime::SequenceFrame,
    b: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> bool {
    a.run_index == b.run_index
        && a.sequence_index == b.sequence_index
        && a.item_id == b.item_id
        && a.position_path == b.position_path
}

/// P464: before another correction submission, re-read the persisted session
/// through the same authoritative path the top of the drive loop uses
/// (`ctx_traits_io::run::status`) rather than depending solely on the
/// rejection response's own `next_frame` — a concurrent caller may have
/// advanced the session further still, and only a fresh read decides whether
/// another retry is even valid.
///
/// Returns `Some` ONLY when the freshly re-read session is still sitting on
/// an actual pending `AwaitingAgentOutput`/`Rejected` frame — the sole
/// situation an in-loop retry can resubmit into. Every other outcome
/// (`Completed`, `Failed`, `BlockedCommandPermissionRequired` needing its own
/// `advance_commands` handling, `AwaitingInput`, `Blocked*`, or no frame at
/// all) returns `None` so the caller abandons the retry and lets the OUTER
/// loop's own top-of-iteration state machine — which already exhaustively
/// handles every one of those statuses correctly — resolve it fresh, instead
/// of this helper half-duplicating that machine and mis-terminating the
/// whole drive on a status it does not special-case.
fn refresh_frame_for_retry(
    input: &DriveInputs<'_>,
    elapsed_seconds: Option<u64>,
) -> crate::Result<
    Option<(
        ctx_traits_core::procedure::session::Session,
        Box<ctx_traits_core::procedure::runtime::SequenceFrame>,
    )>,
> {
    let outcome = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: input.file,
        trait_id: None,
        session: input.session,
        session_store: input.session_store,
        elapsed_seconds,
    })?;
    match outcome.session.status {
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput
        | ctx_traits_core::procedure::session::Status::Rejected => {}
        _ => return Ok(None),
    }
    match outcome.session.next_frame.clone() {
        Some(frame) => Ok(Some((outcome.session, frame))),
        None => Ok(None),
    }
}

/// P402 (`concurrent-terminal-failure-bypasses-p264`): route a concurrent
/// branch/item's terminal dispatch-level failure through core's P264 policy
/// (`ctx_traits_core::procedure::session::submit_terminal_frame_failure`)
/// instead of the CLI silently aborting the drive. Shared by BOTH a live
/// in-wave dispatch that exhausts its retries and a cached/recovered
/// concurrent-wave outcome that already failed before the sequential cursor
/// reached it, so there is exactly one place that turns "this branch/item
/// could not be dispatched" into the same nested-recovery /
/// `skip`/`park`/`panel-fail` transition a serial rejection already
/// triggers — never a second, CLI-only failure policy.
fn apply_concurrent_terminal_failure(
    report: &mut DriveReport,
    input: &DriveInputs<'_>,
    worktree_env: &std::collections::BTreeMap<String, String>,
    reason: &str,
) -> crate::Result<()> {
    let outcome =
        ctx_traits_io::run::terminal_failure_call(ctx_traits_io::run::TerminalFailureRequest {
            trait_file: input.file,
            trait_id: None,
            session: input.session,
            session_store: input.session_store,
            reason,
            execution_dir: input.execution_dir,
            execution_env: worktree_env,
            // No live tick wiring reaches this terminal-failure path (it runs
            // after the wave's own dispatch already failed); command-advance
            // proceeds unobserved, matching the recovery path above.
            tick_observer: None,
        })?;
    report.final_session_status = Some(outcome.response.status.clone());
    report.status = "concurrent-branch-terminal-failure".to_string();
    push_capability(
        report,
        ctx_traits_core::response::CapabilityReport::unsupported(
            "runtime.concurrent-terminal-failure",
            reason.to_string(),
        ),
    );
    Ok(())
}

/// Bundled request for [`attempt_concurrent_wave`], mirroring the
/// `CliHarnessRun` convention used by `run_cli_harness_with_warm_fallback`:
/// the long list of per-call materials lives in one struct instead of a
/// function argument list.
struct ConcurrentWaveRequest<'a> {
    input: &'a DriveInputs<'a>,
    loaded_trait: &'a ctx_traits_io::run::LoadedTrait,
    session: &'a ctx_traits_core::procedure::session::Session,
    role: &'a str,
    /// The base offset's already-resolved plan, used only for the cheap
    /// early-exit persistent-session check before any sibling is peeked.
    /// Every dispatched branch (base offset included) re-resolves its own
    /// harness/CLI convention/plan from its own frame's `assigned_agent`
    /// via `profile` below — no branch reuses this shared value (P456).
    plan: &'a AssignmentPlan,
    /// Mutable so every peeked sibling frame can resolve its own
    /// harness/model plan from its own structural seat, instead of the
    /// whole wave assuming one role implies one harness/model plan (P456).
    profile: &'a mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    budget: &'a Budget,
    max_in_flight: usize,
    current_frame: &'a ctx_traits_core::procedure::runtime::SequenceFrame,
    worktree_env: &'a std::collections::BTreeMap<String, String>,
    /// P478: generated write-confinement payloads for this drive/resume's
    /// worktree, `None` for a non-worktree drive or when confinement is
    /// disabled. Every sibling branch resolves its own harness/CLI
    /// convention independently (P456), but they all share the one plan
    /// generated for this worktree.
    confinement_payloads: Option<&'a ctx_traits_io::confinement::ConfinementPayloads>,
    /// The active `--progress tui` pane, if any. Wired into every branch's
    /// harness request as a tick observer so the quit key stays responsive
    /// while the wave's worker threads block on their harness calls — the
    /// same detachment guarantee a sequential dispatch gets from
    /// `LiveHarnessOutput::tick_observer`.
    run_panel: Option<&'a run_view::RunPanel>,
    /// Drive-wide work-token accumulator (P445), shared with the sequential
    /// path so a wave's siblings contribute to the same running total.
    work_total: &'a WorkTokenTotal,
    /// Shared with the sequential path's own `ensure_drive_probe` calls
    /// (same cache, same probe-once-per-harness guarantee) so a sibling
    /// dispatched through a harness the sequential path already probed
    /// this drive never re-probes it, and a sibling's own harness is held
    /// to the identical probe-required gate the sequential path enforces
    /// on the base offset (P456).
    drive_probes: &'a mut BTreeMap<String, DriveProbe>,
}

/// One peeked wave unit's own resolved dispatch identity (P456): every unit
/// resolves its own harness/CLI convention/plan from its own frame's
/// `assigned_agent`, so two seats of the same list-backed role can dispatch
/// through different eligible CLI harnesses/models within the same wave.
struct SiblingAssignment {
    role: String,
    harness: ctx_traits_io::harness_config::HarnessDefinition,
    cli: ctx_traits_io::harness_config::HarnessCliConvention,
    plan: AssignmentPlan,
    agent_system: Option<String>,
}

/// A concurrent wave was not attempted (or could not complete) for a reason
/// worth surfacing to the caller — see `push_capability` at the call site.
/// Every variant renders as a stable, instance-independent message so
/// repeated occurrences across a run collapse into one capability report.
enum WaveIneligible {
    NoActivationKey,
    WaveTooSmall,
    PersistentSession,
    SiblingIsCommand,
    SiblingRoleMismatch,
    SiblingUnresolvable,
    SiblingIncompatibleAssignment,
    SiblingHarnessUnprobed,
    SiblingModelUnsupported,
    ForEachIntraItemDependency,
    SidecarPathUnresolvable,
    SidecarRecoveryIndeterminate,
    DurablePreDispatchWriteFailed,
}

impl WaveIneligible {
    fn detail(&self) -> &'static str {
        match self {
            Self::NoActivationKey => {
                "parallel panel or concurrent for-each has no control-item id to key concurrent-wave results by"
            }
            Self::WaveTooSmall => {
                "fewer than two remaining branches/items fit in the wave (panel or for-each size, --max-in-flight, and remaining --max-frames budget all bound it); nothing to gain from concurrent dispatch"
            }
            Self::PersistentSession => {
                "role uses a persistent (warm) harness session; concurrent dispatch would fork that stateful conversation across branches, so this panel stays sequential until branch-local persistent sessions exist"
            }
            Self::SiblingIsCommand => {
                "a sibling branch's next step is a command frame, which concurrent dispatch does not support"
            }
            Self::SiblingRoleMismatch => {
                "a sibling branch is assigned a different role than the branch already being driven"
            }
            Self::SiblingUnresolvable => {
                "a sibling branch's frame or prompt could not be resolved ahead of its turn"
            }
            Self::SiblingIncompatibleAssignment => {
                "a sibling branch resolved to a persistent, attach, or MCP assignment, which concurrent CLI dispatch does not support"
            }
            Self::SiblingHarnessUnprobed => {
                "a sibling branch's harness has no successful probe evidence"
            }
            Self::SiblingModelUnsupported => {
                "a sibling branch resolved a model but its harness has no CLI model flag"
            }
            Self::ForEachIntraItemDependency => {
                "concurrent for-each body reads a slot that another step in the same body writes (e.g. an accumulator); items cannot be proven independent, so this for-each stays sequential"
            }
            Self::SidecarPathUnresolvable => {
                "the parent ledger path could not be resolved to derive this activation's durable sidecar directory"
            }
            Self::SidecarRecoveryIndeterminate => {
                "durable sidecar records for this activation are incomplete, still in flight, interrupted, or do not match the current session/position/state — refusing to redispatch or silently trust them"
            }
            Self::DurablePreDispatchWriteFailed => {
                "a durable reservation or wave-manifest record could not be written before any worker was spawned; starting zero concurrent workers and falling back to ordinary sequential dispatch (no provider call was made, so no outcome is lost)"
            }
        }
    }
}

/// Reproducible content digest for anything durably recorded in a P402
/// sidecar (position/base-state identity) — deterministic for a given value
/// so a resumed reader's freshly-recomputed digest can be byte-compared
/// against what a prior process persisted.
fn digest_json<T: serde::Serialize>(value: &T) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    ctx_traits_core::digest::Digest::source(&text).to_string()
}

/// The result of the durable-wave recovery gate for one wave/for-each ordinal
/// (see `recover_wave_offset`), used by the unconditional width-1 pre-check in
/// `drive_loop`. `attempt_concurrent_wave` never dispatches a second wave for
/// an activation whose manifest already exists, so this one gate covers both
/// widths — a default-width resume gets exactly the same fail-closed
/// guarantees a concurrent one does (P402 `durable-sidecars-not-connected`).
enum SidecarRecovery {
    /// No sidecar exists for this ordinal/activation: nothing durable to
    /// recover, safe to proceed to an ordinary fresh dispatch.
    Absent,
    /// A durably-completed (success or terminal-failure) outcome already
    /// exists for this ordinal: consume it instead of dispatching again.
    Terminal(crate::Result<ctx_traits_io::harness::HarnessRunOutcome>),
    /// The sidecar exists but is running/reserved/interrupted, corrupt, or
    /// identity-mismatched: this process must never redispatch or silently
    /// trust it. The caller stops with zero replacement workers.
    Indeterminate,
}

/// Whether one ordinal's sidecar is a durable *terminal* record for the
/// wave-completeness gate: any of the four statuses that mean "this unit's
/// outcome is known and its parent write has been decided" (an unapplied
/// terminal outcome, or an already-consumed `applied`/`rejected-attempt`
/// audit record). A `reserved`/`running`/`interrupted` record is NOT settled.
fn sidecar_status_is_settled(status: ctx_traits_io::run_branch::BranchSidecarStatus) -> bool {
    matches!(
        status,
        ctx_traits_io::run_branch::BranchSidecarStatus::Completed
            | ctx_traits_io::run_branch::BranchSidecarStatus::TerminalFailure
            | ctx_traits_io::run_branch::BranchSidecarStatus::Applied
            | ctx_traits_io::run_branch::BranchSidecarStatus::RejectedAttempt
    )
}

/// The single durable-wave recovery gate (P402 `durable-sidecars-not-connected`),
/// shared by BOTH the unconditional width-1 pre-check in `drive_loop` and the
/// width-N path in `attempt_concurrent_wave`, so a default-width resume gets
/// exactly the same fail-closed guarantees a concurrent one does — one gate,
/// not two.
///
/// Validates the recovered wave against its immutable, dispatch-time
/// [`ctx_traits_io::run_branch::WaveManifest`] — the authoritative record of
/// the original span and every unit's position/base identity — rather than
/// trusting any single mutable sidecar field:
///
/// * No manifest → no wave was ever dispatched for this activation → `Absent`.
/// * This ordinal is outside the manifest span → never speculated → `Absent`.
/// * EVERY unit in the manifest span must have a settled sidecar whose
///   identity and manifest-recorded digests validate; any gap, in-flight
///   (`reserved`/`running`/`interrupted`), corrupt, or digest-mismatched unit
///   → `Indeterminate` (fail closed, zero replay). The digests compared are
///   the sidecar's own recorded values against the immutable manifest's — a
///   stored-vs-stored comparison, so a forged/tampered sidecar is caught while
///   an honest resume is never rejected for legitimately-advanced live state.
/// * If THIS ordinal's own sidecar is already `applied`/`rejected-attempt`,
///   the parent already consumed its first (speculated) frame; the live cursor
///   is on a *later frame* of the same multi-frame branch/item, which was
///   never speculated → `Absent`, so it is dispatched fresh rather than
///   blocked against the already-consumed first frame. Later, still-unapplied
///   siblings are recovered in turn as the cursor reaches them.
/// * Otherwise this ordinal carries an unconsumed terminal outcome → `Terminal`.
fn recover_wave_offset(
    ledger_path: &camino::Utf8Path,
    activation_key: &str,
    ordinal: usize,
    session: &ctx_traits_core::procedure::session::Session,
) -> SidecarRecovery {
    let manifest_path = ctx_traits_io::run_branch::wave_manifest_path(ledger_path, activation_key);
    let manifest = match ctx_traits_io::run_branch::read_wave_manifest(&manifest_path) {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return SidecarRecovery::Absent,
        // A manifest file exists but failed to parse/read: corrupt. Never
        // trusted, never silently skipped as if no wave was dispatched.
        Err(_) => return SidecarRecovery::Indeterminate,
    };
    // The manifest itself must belong to this session/run/activation.
    if manifest.parent_session_id != session.session_id.as_str()
        || manifest.parent_run_id != session.run_id.as_str()
        || manifest.activation_key != activation_key
    {
        return SidecarRecovery::Indeterminate;
    }
    if manifest.unit(ordinal).is_none() {
        // This ordinal was never part of the dispatched wave (e.g. a later,
        // never-speculated offset): nothing durable to recover for it.
        return SidecarRecovery::Absent;
    }
    // Full-span completeness gate: replay NOTHING until every unit in the
    // original span is durably settled and validates against the manifest.
    for unit in &manifest.units {
        let path =
            ctx_traits_io::run_branch::sidecar_path(ledger_path, activation_key, unit.ordinal);
        let sidecar = match ctx_traits_io::run_branch::read_sidecar(&path) {
            Ok(Some(sidecar)) => sidecar,
            // A unit with no sidecar at all, or an unreadable/corrupt one, is
            // indeterminate — never treated as absent-and-safe.
            Ok(None) | Err(_) => return SidecarRecovery::Indeterminate,
        };
        if ctx_traits_io::run_branch::validate_recovered_identity(
            &sidecar,
            session.session_id.as_str(),
            session.run_id.as_str(),
            activation_key,
            unit.ordinal,
            &unit.position_digest,
            &unit.base_state_digest,
        )
        .is_err()
        {
            return SidecarRecovery::Indeterminate;
        }
        if !sidecar_status_is_settled(sidecar.status) {
            return SidecarRecovery::Indeterminate;
        }
    }
    // The whole span is settled; classify THIS ordinal's own record.
    let path = ctx_traits_io::run_branch::sidecar_path(ledger_path, activation_key, ordinal);
    let sidecar = match ctx_traits_io::run_branch::read_sidecar(&path) {
        Ok(Some(sidecar)) => sidecar,
        Ok(None) | Err(_) => return SidecarRecovery::Indeterminate,
    };
    match sidecar.status {
        ctx_traits_io::run_branch::BranchSidecarStatus::Completed
        | ctx_traits_io::run_branch::BranchSidecarStatus::TerminalFailure => {
            let Some(outcome_record) = sidecar.outcome else {
                return SidecarRecovery::Indeterminate;
            };
            let outcome = match outcome_record {
                ctx_traits_io::run_branch::BranchOutcomeRecord::Success(run) => Ok(run),
                ctx_traits_io::run_branch::BranchOutcomeRecord::Failure { message } => {
                    Err(crate::Error::Command { message })
                }
            };
            SidecarRecovery::Terminal(outcome)
        }
        // Already consumed: the live cursor is on a later frame of this
        // multi-frame branch/item (its first frame was applied) — dispatch
        // fresh rather than block. (A single-frame unit never returns to its
        // own offset once applied, so this only ever fires for multi-frame.)
        ctx_traits_io::run_branch::BranchSidecarStatus::Applied
        | ctx_traits_io::run_branch::BranchSidecarStatus::RejectedAttempt => {
            SidecarRecovery::Absent
        }
        // The full-span gate already proved every unit settled, so a
        // non-terminal status here is unreachable; fail closed if it happens.
        _ => SidecarRecovery::Indeterminate,
    }
}

/// The absolute branch offset / `for-each` item index the live cursor
/// currently sits on, if the top control frame is a `parallel` panel or a
/// concurrent `for-each` — the same offset `attempt_concurrent_wave` treats
/// as `current_offset`, exposed standalone so the width-1 recovery pre-check
/// in `drive_loop` can identify "this ordinal" without needing the rest of a
/// wave request.
fn current_wave_offset(session: &ctx_traits_core::procedure::session::Session) -> Option<usize> {
    let top = session.control_stack.last()?;
    let is_concurrent_for_each =
        top.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach && top.concurrent;
    if top.kind != ctx_traits_core::procedure::runtime::ControlKind::Parallel
        && !is_concurrent_for_each
    {
        return None;
    }
    if is_concurrent_for_each {
        top.item_index
    } else {
        top.iteration_index
    }
}

/// P402 independence proof for a concurrent `for-each`: delegates entirely to
/// the core-owned recursive effect walk
/// [`ctx_traits_core::r#trait::procedure::for_each_body_has_cross_item_hazard`]
/// — the SAME traversal `parallel`-branch independence validation uses —
/// rather than a CLI-side hand-rolled direct-field walker. A direct-field-only
/// walker misses dependencies nested in sequences, branches, loops,
/// projections, and slot-backed guards, letting a structurally-nested
/// cross-item dependency slip through and run a later item against stale
/// pre-wave state; this is exactly the recurrence that fix must not
/// reintroduce, so this function must never grow its own direct-field
/// special-casing again.
fn for_each_body_has_intra_item_dependency(
    loaded_trait: &ctx_traits_io::run::LoadedTrait,
    top: &ctx_traits_core::procedure::runtime::ControlFrame,
) -> bool {
    ctx_traits_core::r#trait::procedure::for_each_body_has_cross_item_hazard(
        &loaded_trait.trait_ref,
        &top.sequence_id,
        top.item_slot.as_deref(),
    )
}

/// Identify "this specific activation" of a `parallel` panel so cached wave
/// results can never leak into a different activation of the same
/// `control_item_id` (e.g. a loop or `for-each` re-entering the same panel,
/// or nested panels that happen to share an id). Built from the existing
/// control-stack fields alone (no new session state): every ancestor
/// frame's `control_item_id`/`parent_run_index`/`sequence_id`/`next_index`/
/// `iteration_index`/`item_index`, plus the `parallel` frame's own
/// `control_item_id`/`parent_run_index`. Ancestor `sequence_id`/`next_index`
/// disambiguate positions within the same named sequence; `item_index`
/// (not `iteration_index`, which `for-each` frames leave unset) is what
/// actually advances per `for-each` item, so a panel nested inside a
/// `for-each` gets a distinct key for every item rather than colliding on
/// one shared key. The `parallel` frame's own `iteration_index` (the
/// in-wave branch offset) is deliberately excluded from its own segment —
/// that dimension is already the inner `BTreeMap<usize, _>` key.
fn parallel_wave_activation_key(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<String> {
    let stack = &session.control_stack;
    let top = stack.last()?;
    let is_concurrent_for_each =
        top.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach && top.concurrent;
    if top.kind != ctx_traits_core::procedure::runtime::ControlKind::Parallel
        && !is_concurrent_for_each
    {
        return None;
    }
    top.control_item_id.as_ref()?;
    let mut key = session.run_id.as_str().to_string();
    for frame in &stack[..stack.len() - 1] {
        key.push('|');
        key.push_str(frame.control_item_id.as_deref().unwrap_or(""));
        key.push(':');
        key.push_str(&frame.parent_run_index.to_string());
        key.push(':');
        key.push_str(&frame.sequence_id);
        key.push(':');
        key.push_str(&frame.next_index.to_string());
        key.push(':');
        if let Some(iteration) = frame.iteration_index {
            key.push_str(&iteration.to_string());
        }
        key.push(':');
        if let Some(item_index) = frame.item_index {
            key.push_str(&item_index.to_string());
        }
    }
    key.push_str("|top:");
    key.push_str(top.control_item_id.as_deref().unwrap_or(""));
    key.push(':');
    key.push_str(&top.parent_run_index.to_string());
    Some(key)
}

/// Build a view of `base` whose accepted-value fields are overridden with a
/// peeked/bound hypothetical `State`'s (P402), so prompt resolution
/// (`resolved_frame_prompt`/`accepted_input_value` in `frame_prompt.rs`)
/// looks up a concurrent-wave sibling's OWN bound values — e.g. its `for-each`
/// item — instead of whatever the live parent session's cursor position
/// happens to carry. Only the fields `frame_prompt.rs` actually reads from a
/// `Session` are overridden; every other field (ids, status, control stack,
/// etc.) is irrelevant to prompt text and is left as `base`'s so this stays a
/// minimal, targeted substitution rather than a second session-construction
/// path.
fn session_with_bound_state(
    base: &ctx_traits_core::procedure::session::Session,
    bound: &ctx_traits_core::procedure::runtime::State,
) -> ctx_traits_core::procedure::session::Session {
    let mut session = base.clone();
    session.accepted_port_values = bound.accepted_port_values.clone();
    session.accepted_slot_values = bound.accepted_slot_values.clone();
    session.accepted_output_port_values = bound.accepted_output_port_values.clone();
    session.resource_evidence = bound.resource_evidence.clone();
    session
}

/// Try to resolve and concurrently dispatch the current `parallel` branch or
/// concurrent `for-each` item together with up to `max_in_flight - 1` of its
/// next siblings (P344's `--max-in-flight`, generalized to `for-each` items
/// by P402). Returns `Ok(Some((activation_key, outcomes)))` — outcomes keyed
/// by absolute branch/item offset — when at least one unit in the wave
/// completed its cold, one-shot, non-streaming dispatch (success or
/// application-level failure alike: every outcome is preserved so the
/// caller's ordinary per-branch retry/result path — the same one a
/// sequential attempt uses — sees it exactly once, never rediscarded and
/// never re-run). Returns `Ok(None)` when concurrency simply does not apply
/// here (disabled, not inside a `parallel` panel or a `concurrent = true`
/// `for-each`, or this wave's result is already cached) — nothing to report.
/// Returns `Err(reason)` when the caller opted into concurrency and this
/// position is inside an eligible panel or for-each, but the wave could not
/// be built or fully dispatched — worth an explicit capability report
/// instead of a silent fallback.
///
/// This function never mutates `session` or the on-disk PARENT ledger (the
/// parent conductor remains the sole ledger writer — see
/// `ctx_traits_io::run_branch`'s module docs), or any of the drive loop's
/// warm harness / probe state: every branch's harness call goes through the
/// same shared cold-dispatch primitives
/// (`begin_harness_trace`/`ctx_traits_io::harness::run`/`finish_harness_trace`)
/// that `run_cold_cli_harness` uses for a sequential attempt, so trace and
/// report accounting are identical either way. It DOES write per-unit durable
/// sidecar records (P402) alongside the parent ledger — every unit's
/// `Reserved` record plus the single immutable [`ctx_traits_io::run_branch::WaveManifest`]
/// before any worker is spawned, then a terminal `Completed`/`TerminalFailure`
/// record once each outcome is known. It never dispatches a second wave for an
/// activation whose manifest already exists (recovering/consuming a prior
/// wave's durable outcomes is entirely `recover_wave_offset`'s job, which runs
/// on every frame before this), so there is exactly one recovery gate.
///
/// A unit is eligible only when peeking it (see
/// `ctx_traits_core::procedure::session::peek_parallel_branch_frame` and,
/// for a concurrent `for-each`, `peek_for_each_item_frame`) yields a
/// non-command frame assigned to the SAME role as the unit already being
/// driven (P456: same role no longer implies the same resolved plan — a
/// list-backed role's seats can each resolve to a different eligible CLI
/// harness/model, so every peeked unit resolves and validates its own
/// harness/CLI-convention/plan from its own frame's `assigned_agent` before
/// joining the wave). Units assigned to a different role, whose own resolved
/// assignment is MCP/attach/persistent, or whose first step is a command are
/// simply outside this pass's concurrency support: the wave as a whole is
/// abandoned and every unit in it falls back to sequential dispatch.
fn attempt_concurrent_wave(
    report: &mut DriveReport,
    trace_sequence: &mut u64,
    trace_warned: &mut bool,
    pending_wave_cache: &PendingWaveCache,
    request: ConcurrentWaveRequest<'_>,
) -> Result<Option<(String, WaveOutcomes)>, WaveIneligible> {
    if request.max_in_flight <= 1 {
        return Ok(None);
    }
    let Some(top) = request.session.control_stack.last() else {
        return Ok(None);
    };
    // P402: a wave is either a `parallel` panel's sibling branches or an
    // authored `concurrent = true` `for-each`'s sibling items — the only two
    // shapes `parallel_wave_activation_key` recognizes. Everything else
    // (including a non-concurrent `for-each`, which never opted into
    // speculative dispatch) stays on the ordinary sequential path.
    let is_concurrent_for_each =
        top.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach && top.concurrent;
    if top.kind != ctx_traits_core::procedure::runtime::ControlKind::Parallel
        && !is_concurrent_for_each
    {
        return Ok(None);
    }
    // A persistent (warm) harness session is one stateful conversation.
    // Concurrent siblings would each need their own branch-local session to
    // dispatch through it safely; until that exists, claiming equivalence
    // here would silently change conversation context and, with it, model
    // output. Stay sequential and say so explicitly. A declared shared
    // `[[agent]].session` binding (P328) resolves to `Persistent` too, so a
    // shared-session branch loses `--max-in-flight` the same way.
    if effective_session(
        &request.loaded_trait.trait_ref,
        request.role,
        request.plan,
        request.worktree_env,
    )
    .mode
        == ctx_traits_io::harness_config::RunSessionMode::Persistent
    {
        return Err(WaveIneligible::PersistentSession);
    }
    let Some(current_offset) = (if is_concurrent_for_each {
        top.item_index
    } else {
        top.iteration_index
    }) else {
        return Ok(None);
    };
    let Some(wave_key) = parallel_wave_activation_key(request.session) else {
        return Err(WaveIneligible::NoActivationKey);
    };
    // P402 durable sidecars are always resolved relative to the PARENT
    // ledger's own resolved path (see `ctx_traits_io::run_branch`'s module
    // docs) so a custom `--session-store`/explicit ledger path keeps its
    // sidecars alongside that same ledger, never a hardcoded default store.
    let Ok(ledger_path) = ctx_traits_io::run_session::resolve_session_path(
        request.input.session,
        request.input.session_store,
    ) else {
        return Err(WaveIneligible::SidecarPathUnresolvable);
    };
    // One coherent outstanding wave per panel activation: once a wave has
    // been dispatched for this activation, never dispatch another until
    // every branch it covered has been consumed (the entry is fully
    // removed — see `take_cached_wave_run`). Keying this off "does the
    // *current* offset still have a cached entry" instead would let a
    // multi-frame branch consume its own entry, fall through here again on
    // its next frame, and redispatch (and pay for) every still-pending
    // sibling a second time, clobbering their first outcome.
    if pending_wave_cache.contains_key(&wave_key) {
        return Ok(None);
    }
    // P402 `durable-sidecars-not-connected`: exactly one wave is ever
    // dispatched per activation, and its immutable manifest is the proof of
    // that. If a manifest already exists here, this activation's wave was
    // already dispatched (by a prior process, or an earlier frame of this
    // one) — never dispatch a second one. Recovering/consuming that wave's
    // durable outcomes, and fail-closing on any incomplete/mismatched span,
    // is entirely the width-agnostic `recover_wave_offset` pre-check's job
    // (it runs before this on every frame, for every `--max-in-flight`), so
    // there is one recovery gate, not two. A later frame of a multi-frame
    // branch/item falls through here to ordinary sequential dispatch, which
    // is exactly right — only first frames are ever speculated as a wave.
    let manifest_path = ctx_traits_io::run_branch::wave_manifest_path(&ledger_path, &wave_key);
    match ctx_traits_io::run_branch::read_wave_manifest(&manifest_path) {
        Ok(Some(_)) => return Ok(None),
        Ok(None) => {}
        Err(_) => return Err(WaveIneligible::SidecarRecoveryIndeterminate),
    }
    // P402: a concurrent for-each is only dispatched as a wave when its body
    // is statically provable to be free of cross-item read/write hazards —
    // readiness alone is not independence (see `for_each_body_has_intra_item_dependency`).
    if is_concurrent_for_each && for_each_body_has_intra_item_dependency(request.loaded_trait, top)
    {
        return Err(WaveIneligible::ForEachIntraItemDependency);
    }
    let branch_count = if is_concurrent_for_each {
        top.item_total.unwrap_or(0)
    } else {
        top.parallel_branch_sequence_ids.len()
    };
    // Bound the wave by whichever is smallest: branches actually remaining
    // in the panel, the requested `--max-in-flight`, or frames left in the
    // drive's own `--max-frames` budget. Every branch in the wave is
    // charged to `frames_attempted` at dispatch time (below) before it is
    // known to succeed, so an unbounded wave could pay for calls a
    // one-frame-remaining budget would never have allowed.
    let remaining_frames = request
        .budget
        .max_frames
        .saturating_sub(report.frames_attempted);
    let wave_span = usize::try_from(remaining_frames).unwrap_or(usize::MAX);
    let wave_end = branch_count
        .min(current_offset.saturating_add(request.max_in_flight))
        .min(current_offset.saturating_add(wave_span));
    // A wave of one buys nothing over the existing sequential path — skip
    // the machinery entirely rather than cold-dispatch a lone branch.
    if wave_end.saturating_sub(current_offset) <= 1 {
        return Err(WaveIneligible::WaveTooSmall);
    }

    // Each non-base unit's prompt must be resolved against the exact
    // hypothetical state it was peeked/bound against (P402 fix): the live
    // `request.session` reflects whichever item/branch offset the real
    // cursor currently sits on, not this sibling's — building its prompt
    // from the live session would look up the wrong (or a missing) item
    // value. `None` for the base offset, which is legitimately bound by the
    // live session already (it IS the current cursor position).
    let mut frames: Vec<ctx_traits_core::procedure::runtime::SequenceFrame> =
        Vec::with_capacity(wave_end - current_offset);
    let mut bound_states: Vec<Option<ctx_traits_core::procedure::runtime::State>> =
        Vec::with_capacity(wave_end - current_offset);
    // Every peeked unit's own harness/CLI-convention/plan, resolved from its
    // own frame's `assigned_agent` (role plus structural seat) rather than
    // reused from the base offset's plan — a list-backed role's seats can
    // each resolve to a different eligible CLI harness/model (P456).
    let mut sibling_assignments: Vec<SiblingAssignment> =
        Vec::with_capacity(wave_end - current_offset);
    for offset in current_offset..wave_end {
        let (frame, bound_state) = if offset == current_offset {
            (request.current_frame.clone(), None)
        } else if is_concurrent_for_each {
            match ctx_traits_core::procedure::session::peek_for_each_item_frame(
                &request.loaded_trait.trait_ref,
                request.session,
                offset,
            ) {
                Ok(Some((frame, state))) => (frame, Some(state)),
                _ => return Err(WaveIneligible::SiblingUnresolvable),
            }
        } else {
            match ctx_traits_core::procedure::session::peek_parallel_branch_frame(
                &request.loaded_trait.trait_ref,
                request.session,
                offset,
            ) {
                Ok(Some((frame, state))) => (frame, Some(state)),
                _ => return Err(WaveIneligible::SiblingUnresolvable),
            }
        };
        if frame.command.is_some() {
            return Err(WaveIneligible::SiblingIsCommand);
        }
        let frame_role = frame
            .assigned_agent
            .as_ref()
            .map_or(ctx_traits_io::harness_config::DEFAULT_SEAT, |agent| {
                agent.role.as_str()
            })
            .to_string();
        if frame_role != request.role {
            return Err(WaveIneligible::SiblingRoleMismatch);
        }
        let structural_seat = frame
            .assigned_agent
            .as_ref()
            .and_then(|agent| agent.structural_seat);
        let sibling_plan = assignment_for_role(
            request.profile,
            request.session,
            &frame_role,
            structural_seat,
        )
        .map_err(|_| WaveIneligible::SiblingUnresolvable)?
        .ok_or(WaveIneligible::SiblingUnresolvable)?;
        if sibling_plan.transport != ctx_traits_io::harness_config::RunTransport::Cli
            || sibling_plan.mode == ctx_traits_io::harness_config::RunAssignmentMode::Attach
            || effective_session(
                &request.loaded_trait.trait_ref,
                &frame_role,
                &sibling_plan,
                request.worktree_env,
            )
            .mode
                == ctx_traits_io::harness_config::RunSessionMode::Persistent
        {
            return Err(WaveIneligible::SiblingIncompatibleAssignment);
        }
        let Some(sibling_harness) = request
            .profile
            .registry
            .harness
            .get(&sibling_plan.harness_id)
            .cloned()
        else {
            return Err(WaveIneligible::SiblingUnresolvable);
        };
        let Some(sibling_cli) = sibling_harness.cli.clone() else {
            return Err(WaveIneligible::SiblingUnresolvable);
        };
        // Same probe-required and model-flag-support gates the sequential
        // path enforces on the base offset (P456) — a sibling never
        // dispatches through a harness the sequential path would itself
        // refuse.
        let sibling_probe = ensure_drive_probe(
            request.drive_probes,
            report,
            request.session,
            &sibling_plan.harness_id,
            &sibling_harness,
            request.input.execution_dir,
            request.worktree_env,
        );
        if !sibling_probe.supported {
            return Err(WaveIneligible::SiblingHarnessUnprobed);
        }
        if sibling_plan.model.is_some() && sibling_cli.model_flag.is_none() {
            return Err(WaveIneligible::SiblingModelUnsupported);
        }
        let sibling_agent_system = frame
            .assigned_agent
            .as_ref()
            .and_then(|agent| agent.system.clone());
        sibling_assignments.push(SiblingAssignment {
            role: frame_role,
            harness: sibling_harness,
            cli: sibling_cli,
            plan: sibling_plan,
            agent_system: sibling_agent_system,
        });
        frames.push(frame);
        bound_states.push(bound_state);
    }

    // Recovery of an already-dispatched wave is handled entirely by the
    // manifest-exists short-circuit above plus the `recover_wave_offset`
    // pre-check — reaching this point means no manifest exists yet, so this
    // is a genuinely fresh wave with nothing durable to recover.

    // P402 `durable-sidecars-not-connected`: the per-unit position/base
    // identity digests, computed ONCE here and reused verbatim for the
    // immutable manifest, every reservation, and every terminal write, so the
    // three records can never disagree. The base offset (`current_offset`) is
    // bound by the live session; every sibling by its own peeked state.
    let unit_digests: Vec<(String, String)> = (0..frames.len())
        .map(|index| {
            let position_digest = digest_json(&frames[index]);
            let base_state_digest = digest_json(
                bound_states[index]
                    .as_ref()
                    .unwrap_or(&request.session.ledger),
            );
            (position_digest, base_state_digest)
        })
        .collect();
    let scope_kind = if is_concurrent_for_each {
        ctx_traits_io::run_branch::BranchScopeKind::ForEach
    } else {
        ctx_traits_io::run_branch::BranchScopeKind::Parallel
    };
    let parent_session_id = request.session.session_id.as_str().to_string();
    let parent_run_id = request.session.run_id.as_str().to_string();

    let mut branch_runs: Vec<(usize, CliHarnessRun<'_>)> = Vec::with_capacity(frames.len());
    for (index, frame) in frames.iter().enumerate() {
        let offset = current_offset + index;
        let Ok(requested) = requested_outputs(frame) else {
            return Err(WaveIneligible::SiblingUnresolvable);
        };
        let sibling = &sibling_assignments[index];
        let schema = requested_output_schema(&requested, request.loaded_trait);
        let contract = frame_contract_section(frame);
        let peeked_session = bound_states[index]
            .as_ref()
            .map(|state| session_with_bound_state(request.session, state));
        let prompt_session = peeked_session.as_ref().unwrap_or(request.session);
        let Ok(prompt_context) =
            resolved_frame_prompt(request.loaded_trait, prompt_session, frame, &[])
        else {
            return Err(WaveIneligible::SiblingUnresolvable);
        };
        let mut prompt = frame_prompt(&prompt_context, &contract, &schema, None);
        if let Some(system) = sibling
            .agent_system
            .as_deref()
            .filter(|_| sibling.cli.system_prompt_flag.is_none())
        {
            prompt = format!("{system}\n\n{prompt}");
        }
        let confinement_trace = request.confinement_payloads.and_then(|payloads| {
            ctx_traits_io::confinement::confinement_trace_payload(payloads, sibling.harness.kind())
        });
        // P480: this worktree's generated OS-level spawn sandbox.
        let spawn_sandbox = request
            .confinement_payloads
            .and_then(|payloads| payloads.spawn_sandbox.clone());
        // P478/P480: same unsupported-kind/unsupported-OS-layer reports as
        // the sequential path — a concurrent-wave sibling must not silently
        // spawn with less confinement either.
        if let Some(payloads) = request.confinement_payloads {
            if let Some(capability) = ctx_traits_io::confinement::confinement_unsupported_capability(
                sibling.harness.kind(),
                spawn_sandbox.is_some(),
            ) {
                push_capability(report, capability);
            }
            if let Some(capability) =
                ctx_traits_io::confinement::spawn_sandbox_unsupported_capability(
                    payloads.sandbox_requested,
                    payloads.spawn_sandbox.as_ref(),
                )
            {
                push_capability(report, capability);
            }
        }
        let argv = harness_argv(
            &sibling.harness,
            &sibling.cli,
            &sibling.plan,
            sibling.agent_system.as_deref(),
            HarnessArgvAttempt {
                schema: Some(&schema),
                harness_session_id: None,
                exec_dir: request.input.execution_dir,
                confinement: request.confinement_payloads,
            },
        );
        let prompt_delivery = if sibling.cli.prompt_via.as_deref() == Some("stdin") {
            ctx_traits_io::harness::PromptDelivery::Stdin
        } else {
            ctx_traits_io::harness::PromptDelivery::Arg
        };
        branch_runs.push((
            offset,
            CliHarnessRun {
                session_key: "",
                role: sibling.role.as_str(),
                harness_id: &sibling.plan.harness_id,
                argv,
                env_overlay: request.worktree_env.clone(),
                env_remove: agent_dispatch::harness_env_remove(&sibling.harness),
                // A concurrent sibling never rides a shared warm session:
                // that channel is a single stateful conversation, and
                // dispatching several branches against it at once would
                // corrupt it. Every wave branch always takes the same cold
                // path a sequential attempt falls back to.
                warm_argv: None,
                prompt,
                prompt_delivery,
                timeout_ms: request.budget.frame_seconds.saturating_mul(1000),
                idle_timeout_ms: request
                    .budget
                    .idle_seconds
                    .map(|seconds| seconds.saturating_mul(1000)),
                capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
                stream: false,
                stdout_observer: None,
                tick_observer: request.run_panel.map(run_view::RunPanel::tick_observer),
                exec_dir: request.input.execution_dir,
                confinement: confinement_trace,
                sandbox: spawn_sandbox,
                trace: HarnessTraceContext {
                    run_id: request.session.run_id.as_str(),
                    session_id: request.session.session_id.as_str(),
                    item_id: frame.item_id.as_deref(),
                    frame_title: &frame.title,
                    attempt: 1,
                },
                work_total: request.work_total.clone(),
                token_panel: request.run_panel.cloned(),
            },
        ));
    }

    // Begin every branch's trace and build its request on this thread only
    // (report/trace_sequence/trace_warned are exclusive-borrowed and must
    // never be touched from more than one thread at a time), reusing the
    // exact same preparation `run_cold_cli_harness` uses for its one
    // sequential attempt; the spawned threads then run nothing but the
    // blocking, side-effect-free harness call itself.
    //
    // Each branch is charged to `frames_attempted` right here, at the
    // moment it is actually dispatched (paid for), not when its outcome is
    // later consumed by the sequential cursor. The branch at
    // `current_offset` would otherwise also be counted again by the main
    // loop's own increment when its turn comes — that call site skips its
    // increment for a cache hit specifically to avoid double-counting a
    // call that was only made once.
    // P402: persist EVERY unit's reservation record, then the single
    // immutable wave manifest, durably BEFORE spawning ANY worker — so the
    // whole wave's original extent (which ordinals were dispatched, against
    // what identity) exists on disk before the first provider call is made.
    // If any of these PRE-DISPATCH writes fails, ZERO workers have run and no
    // provider call has been made, so this is a safe soft fallback to the
    // ordinary one-at-a-time sequential path (never a double-pay): the
    // manifest is written LAST, so a failure here leaves at most orphaned
    // `Reserved` sidecars with no manifest, which `recover_wave_offset`
    // ignores (no manifest ⇒ `Absent`) rather than blocking.
    let mut reservations = Vec::with_capacity(branch_runs.len());
    for (offset, _run) in &branch_runs {
        let index = offset - current_offset;
        let sidecar_path =
            ctx_traits_io::run_branch::sidecar_path(&ledger_path, &wave_key, *offset);
        let (position_digest, base_state_digest) = unit_digests[index].clone();
        // P402 (`p402-proof-absent-and-tests-misplaced`) deterministic
        // partial-reservation injection: a no-op in every real invocation
        // (`CTX_INTERNAL_TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL` is only ever set by the
        // concurrency proof). Forcing a LATER ordinal's reservation to fail —
        // after an earlier one already landed — proves that a partial
        // reservation set still starts zero workers and falls back to
        // sequential (no double-pay), exactly as a whole-root failure does.
        if test_only_fail_reservation_write(*offset)
            || ctx_traits_io::run_branch::write_sidecar(
                &sidecar_path,
                &ctx_traits_io::run_branch::BranchSidecar {
                    parent_session_id: parent_session_id.clone(),
                    parent_run_id: parent_run_id.clone(),
                    scope_kind,
                    ordinal: *offset,
                    activation_key: wave_key.clone(),
                    position_digest,
                    base_state_digest,
                    attempt: 1,
                    status: ctx_traits_io::run_branch::BranchSidecarStatus::Reserved,
                    outcome: None,
                    recorded_at_epoch: epoch_seconds(),
                },
            )
            .is_err()
        {
            return Err(WaveIneligible::DurablePreDispatchWriteFailed);
        }
        reservations.push(sidecar_path);
    }
    // The manifest is the authoritative, immutable record every subsequent
    // recovery validates the complete span against. Written once, after all
    // reservations, still before any worker.
    if ctx_traits_io::run_branch::write_wave_manifest(
        &manifest_path,
        &ctx_traits_io::run_branch::WaveManifest {
            parent_session_id: parent_session_id.clone(),
            parent_run_id: parent_run_id.clone(),
            scope_kind,
            activation_key: wave_key.clone(),
            span_start: current_offset,
            span_end: wave_end,
            units: (0..frames.len())
                .map(|index| ctx_traits_io::run_branch::WaveManifestUnit {
                    ordinal: current_offset + index,
                    position_digest: unit_digests[index].0.clone(),
                    base_state_digest: unit_digests[index].1.clone(),
                })
                .collect(),
            recorded_at_epoch: epoch_seconds(),
        },
    )
    .is_err()
    {
        return Err(WaveIneligible::DurablePreDispatchWriteFailed);
    }

    let mut traces = BTreeMap::new();
    let mut counters = BTreeMap::new();
    let mut requests = Vec::with_capacity(branch_runs.len());
    for ((offset, run), sidecar_path) in branch_runs.iter().zip(reservations.iter()) {
        report.frames_attempted += 1;
        // Reservation already durably persisted above (before any worker was
        // spawned) — transition it to `Running` right before this unit's
        // call actually starts. Best-effort from here on: a failure to
        // record the in-flight transition does not itself invalidate the
        // wave (the reservation already proves the wave's extent was
        // durable), but a terminal outcome for this unit is still gated on
        // its OWN durable write below (see the join loop), never silently
        // treated as safe on a persistence failure there.
        if let Ok(Some(mut sidecar)) = ctx_traits_io::run_branch::read_sidecar(sidecar_path) {
            sidecar.status = ctx_traits_io::run_branch::BranchSidecarStatus::Running;
            let _ = ctx_traits_io::run_branch::write_sidecar(sidecar_path, &sidecar);
        }
        let (trace, request, counter) =
            begin_cold_dispatch(report, trace_sequence, trace_warned, run);
        traces.insert(*offset, trace);
        counters.insert(*offset, counter);
        requests.push((*offset, request));
    }

    let joined: Vec<(
        usize,
        ctx_traits_io::Result<ctx_traits_io::harness::HarnessRunOutcome>,
    )> = std::thread::scope(|scope| {
        let handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, _>)> = requests
            .into_iter()
            .map(|(offset, run_request)| {
                (
                    offset,
                    // Returned as a tuple (rather than a bare `Result`) so the
                    // outer offset survives a panic below, and so this
                    // closure's return type does not itself trip
                    // `clippy::result_large_err` on `ctx_traits_io::Error`.
                    scope.spawn(move || (offset, ctx_traits_io::harness::run(run_request))),
                )
            })
            .collect();
        handles
            .into_iter()
            .map(|(offset, handle)| {
                // A worker panic (as opposed to an `Err` the harness call
                // returns normally) must still produce a typed result for
                // this offset — same reasoning as preserving `Err` below:
                // dropping it here would let the branch silently redispatch
                // later as an unbudgeted implicit retry.
                match handle.join() {
                    Ok((_, result)) => (offset, result),
                    Err(panic) => (
                        offset,
                        Err(ctx_traits_io::environment::Error::Process {
                            command: None,
                            path: None,
                            exit_status: None,
                            timed_out: false,
                            message: format!(
                                "concurrent wave worker for branch {offset} panicked: {}",
                                panic_message(&panic)
                            ),
                        }
                        .into()),
                    ),
                }
            })
            .collect()
    });

    let mut outcomes = WaveOutcomes::new();
    let mut terminal_persist_failed = false;
    for (offset, result) in joined {
        let trace = traces.remove(&offset).flatten();
        let counter = counters.remove(&offset).unwrap_or_else(|| {
            WorkTokenCounterHandle::new(request.work_total.clone(), request.run_panel.cloned())
        });
        finish_cold_dispatch(report, trace_warned, trace, &counter, &result);
        // P402: persist this unit's terminal outcome durably, alongside the
        // in-memory cache — a resumed process recovers this exact record
        // (via `recover_wave_offset`) instead of relaunching the worker.
        let index = offset - current_offset;
        let sidecar_path = ctx_traits_io::run_branch::sidecar_path(&ledger_path, &wave_key, offset);
        let (position_digest, base_state_digest) = unit_digests[index].clone();
        let (status, outcome_record) = match &result {
            Ok(run) => (
                ctx_traits_io::run_branch::BranchSidecarStatus::Completed,
                ctx_traits_io::run_branch::BranchOutcomeRecord::Success(run.clone()),
            ),
            Err(error) => (
                ctx_traits_io::run_branch::BranchSidecarStatus::TerminalFailure,
                ctx_traits_io::run_branch::BranchOutcomeRecord::Failure {
                    message: error.to_string(),
                },
            ),
        };
        // P402 (`p402-proof-absent-and-tests-misplaced`) deterministic
        // terminal-write-failure injection: a no-op in every real invocation
        // (`CTX_INTERNAL_TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL` is only ever set by the
        // concurrency proof), it forces THIS ordinal's terminal persistence to
        // fail so the proof can assert that a later ordinal's write failure
        // applies zero wave outcomes. Reuses the exact same fail-closed path a
        // real `write_sidecar` error takes.
        let injected_write_failure = test_only_fail_terminal_write(offset);
        if injected_write_failure
            || ctx_traits_io::run_branch::write_sidecar(
                &sidecar_path,
                &ctx_traits_io::run_branch::BranchSidecar {
                    parent_session_id: parent_session_id.clone(),
                    parent_run_id: parent_run_id.clone(),
                    scope_kind,
                    ordinal: offset,
                    activation_key: wave_key.clone(),
                    position_digest,
                    base_state_digest,
                    attempt: 1,
                    status,
                    outcome: Some(outcome_record),
                    recorded_at_epoch: epoch_seconds(),
                },
            )
            .is_err()
        {
            // P402 `durable-sidecars-not-connected` (terminal-persistence
            // atomicity): if even ONE unit's terminal outcome cannot be
            // durably persisted, NO outcome from this wave may be applied to
            // the parent — applying the siblings whose writes DID land while
            // this ordinal's record is stuck non-terminal would advance the
            // cursor past a wave the manifest can never prove complete on
            // resume. Record the failure and, after this loop, discard every
            // in-memory outcome and fail closed. The already-persisted
            // terminal sidecars plus the still-non-terminal one leave an
            // incomplete span, so `recover_wave_offset` blocks (zero replay)
            // rather than redispatching on the next frame.
            terminal_persist_failed = true;
        }
        // Preserve every branch's outcome, success or failure alike: a
        // branch whose result errors here is cached as that same `Err` and
        // is propagated (not redispatched) the one time the sequential
        // cursor reaches it — see `take_cached_wave_run`'s consumer.
        outcomes.insert(offset, result.map_err(crate::Error::from));
    }
    if terminal_persist_failed {
        // Zero outcomes applied: hand the caller the hard, fail-closed block
        // rather than a partial `Ok(Some(..))` that would apply the
        // successfully-persisted siblings.
        return Err(WaveIneligible::SidecarRecoveryIndeterminate);
    }
    Ok(Some((wave_key, outcomes)))
}

// P492: the four P402 test-hook names, resolved per build profile so their
// bytes never reach a release binary through these call sites. In a debug
// build these resolve to the real `CTX_INTERNAL_TESTHOOK_*` names declared
// once in `ctx_traits_io::env_reference`; in a release build they resolve to
// these local, unrelated placeholders instead — the real names are not even
// defined in that profile (see their `#[cfg(debug_assertions)]` gate there).
#[cfg(debug_assertions)]
use ctx_traits_io::env_reference::{
    TESTHOOK_CHECKPOINT_ONE_APPLIED, TESTHOOK_CHECKPOINT_WAVE_PERSISTED,
    TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL, TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL,
};
#[cfg(not(debug_assertions))]
const TESTHOOK_CHECKPOINT_WAVE_PERSISTED: &str = "";
#[cfg(not(debug_assertions))]
const TESTHOOK_CHECKPOINT_ONE_APPLIED: &str = "";
#[cfg(not(debug_assertions))]
const TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL: &str = "";
#[cfg(not(debug_assertions))]
const TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL: &str = "";

/// P402 test-only durable-state-boundary checkpoint (blocker 3,
/// `p402-proof-absent-and-tests-misplaced`). A no-op in every real
/// invocation: `env_var` is never set outside `scripts/byte_compare.rs`'s
/// `--concurrency-proof` fixtures, and reading an unset variable is the only
/// cost paid on the ordinary path. When a proof sets `env_var` to a
/// filesystem path, this touches that path — letting the proof's own process
/// deterministically observe "the conductor reached this exact in-process
/// durable-state boundary" (e.g. every wave unit's terminal sidecar has been
/// persisted but nothing has been applied to the parent ledger yet, or
/// exactly one wave unit's parent-ledger write has landed while its
/// siblings' sidecars are still unapplied) — and then blocks, bounded to 10s
/// (matching `fixture_marker_worker_wait`'s own bound so a proof-side bug
/// that never kills the process fails fast instead of hanging). This is the
/// only sound way to prove a process-kill boundary between two synchronous
/// in-process steps without racing real wall-clock timing.
///
/// P492: compiled in only for `debug_assertions` builds (`Cargo.toml`
/// declares no `[profile]` tables, so this is on in dev and off in
/// `--release`). A `#[cfg(not(debug_assertions))]` no-op twin below removes
/// both the code and the hook name strings from a shipped release binary —
/// strictly stronger than an arming env var, which would still ship a live
/// hook behind a guessable second key. `just testhook-absence-check` proves
/// this split holds in both compiled profiles.
#[cfg(debug_assertions)]
fn test_only_checkpoint(env_var: &str) {
    let Ok(path) = std::env::var(env_var) else {
        return;
    };
    if std::fs::write(&path, b"reached\n").is_err() {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(not(debug_assertions))]
fn test_only_checkpoint(_env_var: &str) {}

/// P402 test-only terminal-write-failure injection (blocker 2,
/// `p402-proof-absent-and-tests-misplaced`). A no-op in every real invocation:
/// `CTX_INTERNAL_TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL` is never set outside the
/// concurrency proof. When set to an ordinal, the terminal sidecar write for
/// exactly that ordinal is forced to behave as a persistence failure, so the
/// proof can assert that a later ordinal's terminal-write failure applies zero
/// wave outcomes to the parent (terminal-persistence atomicity).
fn test_only_fail_terminal_write(ordinal: usize) -> bool {
    test_only_fail_write(TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL, ordinal)
}

/// P402 test-only reservation-write-failure injection (blocker 2), sibling of
/// [`test_only_fail_terminal_write`] for the pre-dispatch reservation loop. See
/// there; a no-op unless `CTX_INTERNAL_TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL` is set.
fn test_only_fail_reservation_write(ordinal: usize) -> bool {
    test_only_fail_write(TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL, ordinal)
}

/// P492: see [`test_only_checkpoint`]'s doc comment for why this is a
/// `debug_assertions`-gated compile-out rather than a cargo feature (a
/// release build can acquire an additive feature from an unrelated
/// dependency edge; a profile-driven `cfg` cannot).
#[cfg(debug_assertions)]
fn test_only_fail_write(env_var: &str, ordinal: usize) -> bool {
    std::env::var(env_var)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|target| target == ordinal)
}

#[cfg(not(debug_assertions))]
fn test_only_fail_write(_env_var: &str, _ordinal: usize) -> bool {
    false
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Best-effort human-readable message from a caught `std::thread` panic
/// payload — panics conventionally carry a `&str` or `String`.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "worker thread panicked with a non-string payload".to_string()
    }
}

/// Consume this branch's or item's cached wave outcome (see
/// `attempt_concurrent_wave`), if one is present for the session's current
/// `parallel` branch or concurrent `for-each` item position. Removes the
/// entry so it is used exactly once. The cached entry is an `Err` whenever
/// the wave's dispatch of this branch failed (IO error or worker panic) —
/// the caller propagates that failure exactly as a live dispatch would,
/// rather than treating a missing cache entry as license to redispatch.
/// P402: `(activation-key, offset)` identity for the session's current
/// `parallel` branch or concurrent `for-each` item position, IF a cached
/// wave outcome is present for it — read-only sibling of
/// `take_cached_wave_run` used to remember which durable sidecar a cache hit
/// corresponds to before consuming it.
fn wave_cache_identity(
    pending_wave_cache: &PendingWaveCache,
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<(String, usize)> {
    let top = session.control_stack.last()?;
    let is_concurrent_for_each =
        top.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach && top.concurrent;
    let current_offset = if is_concurrent_for_each {
        top.item_index?
    } else {
        top.iteration_index?
    };
    let wave_key = parallel_wave_activation_key(session)?;
    pending_wave_cache
        .get(&wave_key)?
        .contains_key(&current_offset)
        .then_some((wave_key, current_offset))
}

fn take_cached_wave_run(
    pending_wave_cache: &mut PendingWaveCache,
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<crate::Result<ctx_traits_io::harness::HarnessRunOutcome>> {
    let top = session.control_stack.last()?;
    let is_concurrent_for_each =
        top.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach && top.concurrent;
    let current_offset = if is_concurrent_for_each {
        top.item_index?
    } else {
        top.iteration_index?
    };
    let wave_key = parallel_wave_activation_key(session)?;
    let branches = pending_wave_cache.get_mut(&wave_key)?;
    let run = branches.remove(&current_offset)?;
    if branches.is_empty() {
        pending_wave_cache.remove(&wave_key);
    }
    Some(run)
}

fn warm_harness_argv(
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    cli: &ctx_traits_io::harness_config::HarnessCliConvention,
    plan: &AssignmentPlan,
    agent_system: Option<&str>,
    kind: WarmPromptKind,
    confinement: Option<&ctx_traits_io::confinement::ConfinementPayloads>,
    resumed_session_id: Option<&String>,
) -> Option<Vec<String>> {
    if cli.output.as_deref() != Some("claude-stream-json") {
        return None;
    }
    let warm_argv = cli.warm_argv.as_ref()?;
    let mut argv = Vec::new();
    argv.push(harness.bin().to_string());
    argv.extend(warm_argv.clone());
    // P516: a resumed conversation crossing a `drive` resume dispatches
    // through this warm channel too — see the drive loop's persisted
    // `harness_sessions` seed/write and `append_session_resume`'s doc.
    agent_dispatch::append_session_resume(&mut argv, cli, resumed_session_id);
    if let (Some(flag), Some(model)) = (cli.model_flag.as_ref(), plan.model.as_ref()) {
        argv.push(flag.clone());
        argv.push(model.clone());
    }
    agent_dispatch::append_reasoning_effort(
        &mut argv,
        harness,
        cli,
        plan.reasoning_effort.as_deref(),
    );
    if let Some(flag) = cli.system_prompt_flag.as_ref() {
        argv.push(flag.clone());
        let prompt = match kind {
            WarmPromptKind::Frame => {
                composed_system_prompt(agent_system, plan.system_prompt.as_deref())
            }
            WarmPromptKind::Narrator => {
                composed_narrator_system_prompt(plan.system_prompt.as_deref())
            }
        };
        argv.push(prompt);
    }
    argv.extend(plan.extra_args.clone());
    agent_dispatch::append_confinement(&mut argv, harness, confinement);
    Some(argv)
}

struct CliHarnessRun<'a> {
    session_key: &'a str,
    role: &'a str,
    harness_id: &'a str,
    argv: Vec<String>,
    /// Resolved `[worktree].env` overlay applied to cold and persistent CLI
    /// harness spawns (before `env_remove`). Empty for non-worktree runs.
    env_overlay: BTreeMap<String, String>,
    env_remove: Vec<String>,
    warm_argv: Option<Vec<String>>,
    prompt: String,
    prompt_delivery: ctx_traits_io::harness::PromptDelivery,
    timeout_ms: u64,
    idle_timeout_ms: Option<u64>,
    capture_limit: usize,
    stream: bool,
    stdout_observer: Option<ctx_traits_io::harness::OutputObserver>,
    tick_observer: Option<ctx_traits_io::harness::TickObserver>,
    exec_dir: Option<&'a camino::Utf8Path>,
    /// P478: the payload actually applied to this attempt's harness kind
    /// (argv-delivered for claude-code, env-delivered for opencode) — carried
    /// here only so the debug trace can show it; the argv/env already carry
    /// the actual enforcement.
    confinement: Option<&'a serde_json::Value>,
    /// P480: this worktree's generated OS-level spawn sandbox, `None` for a
    /// non-worktree run, `sandbox = false`, or an unsupported platform.
    /// Applied at the actual spawn seam (`cold_harness_request`/
    /// `HarnessSession::spawn`), not just carried for the trace.
    sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
    trace: HarnessTraceContext<'a>,
    /// Drive-wide work-token accumulator (P445): shared by every attempt at
    /// every offset (sequential and concurrent-wave alike), so a resumed or
    /// widened drive still contributes to exactly one running total.
    work_total: WorkTokenTotal,
    /// The live `--progress tui` panel, if any, so this attempt's observed
    /// output tokens also feed the panel's existing per-step display —
    /// independent of `stdout_observer`'s live-narration wiring above (see
    /// `WorkTokenCounterHandle`).
    token_panel: Option<run_view::RunPanel>,
}

#[derive(Clone, Copy)]
struct HarnessTraceContext<'a> {
    run_id: &'a str,
    session_id: &'a str,
    item_id: Option<&'a str>,
    frame_title: &'a str,
    attempt: u64,
}

struct ActiveHarnessTrace {
    writer: HarnessAttemptWriter,
    started: Instant,
}

struct CountedWarmFailure<'a> {
    session_key: &'a str,
    role: &'a str,
    harness_id: &'a str,
    reason: String,
    duration_ms: Option<u128>,
}

fn run_cli_harness_with_warm_fallback(
    report: &mut DriveReport,
    warm_sessions: &mut BTreeMap<String, ctx_traits_io::harness::HarnessSession>,
    warm_respawn_used: &mut BTreeSet<String>,
    warm_disabled: &mut BTreeSet<String>,
    trace_sequence: &mut u64,
    trace_warned: &mut bool,
    run: CliHarnessRun<'_>,
) -> crate::Result<ctx_traits_io::harness::HarnessRunOutcome> {
    if let Some(warm_argv) = run.warm_argv.clone()
        && !warm_disabled.contains(run.session_key)
    {
        let mut warm_failure = None;
        if warm_sessions
            .get_mut(run.session_key)
            .is_some_and(|session| !session.is_alive())
        {
            warm_sessions.remove(run.session_key);
            if !record_counted_warm_failure(
                report,
                warm_respawn_used,
                warm_disabled,
                CountedWarmFailure {
                    session_key: run.session_key,
                    role: run.role,
                    harness_id: run.harness_id,
                    reason: "warm exited between turns".to_string(),
                    duration_ms: None,
                },
            ) {
                return run_cold_cli_harness(report, trace_sequence, trace_warned, &run);
            }
        }
        if !warm_sessions.contains_key(run.session_key) {
            match ctx_traits_io::harness::HarnessSession::spawn(
                warm_argv.clone(),
                run.env_overlay.clone(),
                run.env_remove.clone(),
                run.exec_dir.map(camino::Utf8Path::to_path_buf),
                run.sandbox.clone(),
            ) {
                Ok(session) => {
                    warm_sessions.insert(run.session_key.to_string(), session);
                }
                Err(err) => {
                    warm_failure = Some((format!("warm spawn failed: {err}"), None));
                }
            }
        }
        if warm_failure.is_none()
            && let Some(session) = warm_sessions.get_mut(run.session_key)
        {
            let (trace, stdout_observer, counter) = begin_harness_trace(
                report,
                trace_sequence,
                trace_warned,
                &run,
                "warm",
                &warm_argv,
            );
            match session.prompt(ctx_traits_io::harness::HarnessSessionPrompt {
                prompt: run.prompt.clone(),
                timeout_ms: run.timeout_ms,
                idle_timeout_ms: run.idle_timeout_ms,
                capture_limit: run.capture_limit,
                stream: run.stream,
                stdout_observer,
                tick_observer: run.tick_observer.clone(),
            }) {
                Ok(outcome) => {
                    finish_harness_trace(report, trace_warned, trace, &counter, &outcome);
                    match warm_outcome_failure(&outcome) {
                        None => return Ok(outcome),
                        Some(WarmOutcomeFailure::ImmediateFallback { reason }) => {
                            warm_failure = Some((reason, Some(outcome.duration_ms)));
                        }
                        Some(WarmOutcomeFailure::Counted { reason }) => {
                            warm_sessions.remove(run.session_key);
                            record_counted_warm_failure(
                                report,
                                warm_respawn_used,
                                warm_disabled,
                                CountedWarmFailure {
                                    session_key: run.session_key,
                                    role: run.role,
                                    harness_id: run.harness_id,
                                    reason,
                                    duration_ms: Some(outcome.duration_ms),
                                },
                            );
                            return Ok(outcome);
                        }
                    }
                }
                Err(err) => {
                    fail_harness_trace(report, trace_warned, trace, &counter, &err);
                    warm_failure = Some((format!("warm prompt failed: {err}"), None));
                }
            }
        }
        if let Some((reason, duration_ms)) = warm_failure {
            warm_sessions.remove(run.session_key);
            warm_disabled.insert(run.session_key.to_string());
            report.events.push(DriveEvent {
                event: "harness-warm-fallback".to_string(),
                role: Some(run.role.to_string()),
                harness: Some(run.harness_id.to_string()),
                detail: reason,
                duration_ms,
            });
        }
    }
    run_cold_cli_harness(report, trace_sequence, trace_warned, &run)
}

fn record_counted_warm_failure(
    report: &mut DriveReport,
    warm_respawn_used: &mut BTreeSet<String>,
    warm_disabled: &mut BTreeSet<String>,
    failure: CountedWarmFailure<'_>,
) -> bool {
    let still_enabled = warm_respawn_used.insert(failure.session_key.to_string());
    let event = if still_enabled {
        "harness-warm-respawn"
    } else {
        warm_disabled.insert(failure.session_key.to_string());
        "harness-warm-disabled"
    };
    report.events.push(DriveEvent {
        event: event.to_string(),
        role: Some(failure.role.to_string()),
        harness: Some(failure.harness_id.to_string()),
        detail: failure.reason,
        duration_ms: failure.duration_ms,
    });
    still_enabled
}

/// Build the actual `ctx_traits_io::harness::run` request from a resolved
/// `CliHarnessRun`. Shared by the sequential cold path
/// (`run_cold_cli_harness`) and the concurrent wave path
/// (`attempt_concurrent_wave`) so both go through one place that assembles
/// the request handed to the single real dispatch primitive.
fn cold_harness_request(
    run: &CliHarnessRun<'_>,
    stdout_observer: Option<ctx_traits_io::harness::OutputObserver>,
) -> ctx_traits_io::harness::HarnessRunRequest {
    ctx_traits_io::harness::HarnessRunRequest {
        argv: run.argv.clone(),
        env_overlay: run.env_overlay.clone(),
        env_remove: run.env_remove.clone(),
        prompt: run.prompt.clone(),
        prompt_delivery: run.prompt_delivery.clone(),
        timeout_ms: run.timeout_ms,
        idle_timeout_ms: run.idle_timeout_ms,
        capture_limit: run.capture_limit,
        stream: run.stream,
        stdout_observer,
        tick_observer: run.tick_observer.clone(),
        exec_dir: run.exec_dir.map(camino::Utf8Path::to_path_buf),
        sandbox: run.sandbox.clone(),
    }
}

/// Shared preparation for one cold, one-shot harness dispatch: builds the
/// trace-only argv, starts this call's trace, and assembles the actual
/// `HarnessRunRequest`. Used by both the sequential path
/// (`run_cold_cli_harness`) and the concurrent wave path
/// (`attempt_concurrent_wave`) so there is exactly one place that turns a
/// resolved `CliHarnessRun` into a dispatchable, traced request.
fn begin_cold_dispatch(
    report: &mut DriveReport,
    trace_sequence: &mut u64,
    trace_warned: &mut bool,
    run: &CliHarnessRun<'_>,
) -> (
    Option<ActiveHarnessTrace>,
    ctx_traits_io::harness::HarnessRunRequest,
    WorkTokenCounterHandle,
) {
    let mut trace_argv = run.argv.clone();
    if run.prompt_delivery == ctx_traits_io::harness::PromptDelivery::Arg {
        trace_argv.push(run.prompt.clone());
    }
    let (trace, stdout_observer, counter) = begin_harness_trace(
        report,
        trace_sequence,
        trace_warned,
        run,
        "cold",
        &trace_argv,
    );
    (trace, cold_harness_request(run, stdout_observer), counter)
}

/// Shared finalization for one cold dispatch outcome: flushes this attempt's
/// P445 token counter, then writes the finish/fail trace event for it. Used
/// by both `run_cold_cli_harness` and `attempt_concurrent_wave` so there is
/// exactly one place that converts a dispatch result into a trace event and
/// a token contribution. Takes `outcome` by reference (rather than
/// returning it) so this helper's own signature never repeats the large
/// `ctx_traits_io::Error` in a return position — each caller already owns
/// the result it just matched on.
fn finish_cold_dispatch(
    report: &mut DriveReport,
    trace_warned: &mut bool,
    trace: Option<ActiveHarnessTrace>,
    counter: &WorkTokenCounterHandle,
    outcome: &ctx_traits_io::Result<ctx_traits_io::harness::HarnessRunOutcome>,
) {
    match outcome {
        Ok(value) => finish_harness_trace(report, trace_warned, trace, counter, value),
        Err(error) => fail_harness_trace(report, trace_warned, trace, counter, error),
    }
}

fn run_cold_cli_harness(
    report: &mut DriveReport,
    trace_sequence: &mut u64,
    trace_warned: &mut bool,
    run: &CliHarnessRun<'_>,
) -> crate::Result<ctx_traits_io::harness::HarnessRunOutcome> {
    let (trace, request, counter) = begin_cold_dispatch(report, trace_sequence, trace_warned, run);
    let outcome = ctx_traits_io::harness::run(request);
    finish_cold_dispatch(report, trace_warned, trace, &counter, &outcome);
    outcome.map_err(Into::into)
}

/// Shared preparation for one harness attempt (cold or warm alike): starts
/// its debug trace (if any) and, independent of that trace's own success,
/// always creates a fresh P445 [`WorkTokenCounterHandle`] for this attempt —
/// so token counting never depends on `--progress tui`/`stream` or on the
/// debug trace succeeding, only on an attempt actually being dispatched.
fn begin_harness_trace(
    report: &mut DriveReport,
    trace_sequence: &mut u64,
    trace_warned: &mut bool,
    run: &CliHarnessRun<'_>,
    invocation: &str,
    argv: &[String],
) -> (
    Option<ActiveHarnessTrace>,
    Option<ctx_traits_io::harness::OutputObserver>,
    WorkTokenCounterHandle,
) {
    let counter = WorkTokenCounterHandle::new(run.work_total.clone(), run.token_panel.clone());
    *trace_sequence = trace_sequence.saturating_add(1);
    let trace = HarnessAttemptWriter::start(&HarnessAttemptStart {
        run_id: run.trace.run_id,
        session_id: run.trace.session_id,
        sequence: *trace_sequence,
        item_id: run.trace.item_id,
        frame_title: run.trace.frame_title,
        role: run.role,
        harness_id: run.harness_id,
        attempt: run.trace.attempt,
        invocation,
        argv,
        prompt: &run.prompt,
        stdout_limit: run.capture_limit,
        confinement: run.confinement,
        spawn_sandbox: run.sandbox.as_ref().map(|sandbox| sandbox.profile.as_str()),
    });
    let combined = match trace {
        Ok(writer) => {
            let observer = combine_stdout_observers(
                run.stdout_observer.clone(),
                Some(writer.stdout_observer()),
            );
            (
                Some(ActiveHarnessTrace {
                    writer,
                    started: Instant::now(),
                }),
                observer,
            )
        }
        Err(error) => {
            warn_trace_once(report, trace_warned, &error);
            (None, run.stdout_observer.clone())
        }
    };
    let observer = combine_stdout_observers(combined.1, Some(counter.observer()));
    (combined.0, observer, counter)
}

fn finish_harness_trace(
    report: &mut DriveReport,
    trace_warned: &mut bool,
    trace: Option<ActiveHarnessTrace>,
    counter: &WorkTokenCounterHandle,
    outcome: &ctx_traits_io::harness::HarnessRunOutcome,
) {
    counter.flush();
    let Some(trace) = trace else {
        return;
    };
    if let Err(error) = trace.writer.finish(&HarnessAttemptExit {
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        idle_timed_out: outcome.idle_timed_out,
        stdout_truncated: outcome.stdout_truncated,
        duration_ms: outcome.duration_ms,
        stderr: &outcome.stderr,
    }) {
        warn_trace_once(report, trace_warned, &error);
    }
}

fn fail_harness_trace(
    report: &mut DriveReport,
    trace_warned: &mut bool,
    trace: Option<ActiveHarnessTrace>,
    counter: &WorkTokenCounterHandle,
    error: &impl std::fmt::Display,
) {
    counter.flush();
    let Some(trace) = trace else {
        return;
    };
    let stderr = format!("harness invocation failed before outcome: {error}");
    if let Err(error) = trace.writer.finish(&HarnessAttemptExit {
        exit_code: None,
        timed_out: false,
        idle_timed_out: false,
        stdout_truncated: false,
        duration_ms: trace.started.elapsed().as_millis(),
        stderr: &stderr,
    }) {
        warn_trace_once(report, trace_warned, &error);
    }
}

pub(crate) fn combine_stdout_observers(
    first: Option<ctx_traits_io::harness::OutputObserver>,
    second: Option<ctx_traits_io::harness::OutputObserver>,
) -> Option<ctx_traits_io::harness::OutputObserver> {
    match (first, second) {
        (None, None) => None,
        (Some(observer), None) | (None, Some(observer)) => Some(observer),
        (Some(first), Some(second)) => Some(std::sync::Arc::new(move |chunk: &[u8]| {
            first(chunk);
            second(chunk);
        })),
    }
}

fn warn_trace_once(
    report: &mut DriveReport,
    trace_warned: &mut bool,
    error: &impl std::fmt::Display,
) {
    if !*trace_warned {
        *trace_warned = true;
        report
            .warnings
            .push(format!("debug trace not written: {error}"));
    }
}

fn warm_outcome_failure(
    outcome: &ctx_traits_io::harness::HarnessRunOutcome,
) -> Option<WarmOutcomeFailure> {
    if outcome.idle_timed_out {
        return Some(WarmOutcomeFailure::Counted {
            reason: "warm idle timeout".to_string(),
        });
    }
    if outcome.timed_out {
        return Some(WarmOutcomeFailure::Counted {
            reason: "warm frame timeout".to_string(),
        });
    }
    if outcome.exit_code != Some(0) {
        let detail = output_error_detail(outcome);
        return Some(WarmOutcomeFailure::Counted {
            reason: format!(
                "warm exit {}: {detail}",
                crate::app::presentation::optional(outcome.exit_code)
            ),
        });
    }
    if outcome.stdout_truncated {
        return None;
    }
    if !stdout_has_result_event(&outcome.stdout) {
        return Some(WarmOutcomeFailure::ImmediateFallback {
            reason: "warm protocol result event missing".to_string(),
        });
    }
    None
}

fn stdout_has_result_event(stdout: &str) -> bool {
    harness_stream::stream_values(stdout)
        .iter()
        .any(|value| value.get("type").and_then(Value::as_str) == Some("result"))
}

fn output_error_detail(outcome: &ctx_traits_io::harness::HarnessRunOutcome) -> String {
    outcome
        .stderr
        .lines()
        .chain(outcome.stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(120).collect())
        .unwrap_or_else(|| "no diagnostic output".to_string())
}

fn narrator_harness_argv(
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    cli: &ctx_traits_io::harness_config::HarnessCliConvention,
    plan: &AssignmentPlan,
    exec_dir: Option<&camino::Utf8Path>,
    confinement: Option<&ctx_traits_io::confinement::ConfinementPayloads>,
) -> Vec<String> {
    // A narrator wants a short one-shot answer, not the worker's streaming argv;
    // `standing_agent_argv` uses narrator-argv when configured so the reply
    // stays small and fast.
    let mut argv = agent_dispatch::standing_agent_argv(
        harness,
        cli,
        true,
        plan.model.as_deref(),
        plan.reasoning_effort.as_deref(),
        &composed_narrator_system_prompt(plan.system_prompt.as_deref()),
        &plan.extra_args,
    );
    agent_dispatch::append_exec_dir(&mut argv, cli, exec_dir);
    // P478: generated write confinement, composed with — never replacing —
    // the rest of this argv.
    agent_dispatch::append_confinement(&mut argv, harness, confinement);
    argv
}

fn mcp_harness_argv(
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    plan: &AssignmentPlan,
    agent_system: Option<&str>,
    mcp: Option<&ctx_traits_io::harness_config::HarnessMcpConvention>,
    cli: Option<&ctx_traits_io::harness_config::HarnessCliConvention>,
    elapsed_seconds: Option<u64>,
) -> crate::Result<Vec<String>> {
    let mut argv = Vec::new();
    argv.push(harness.bin().to_string());
    let mcp_has_reasoning = mcp.is_some_and(|mcp| mcp.reasoning_effort_flag.is_some());
    if let Some(cli) = cli {
        argv.extend(cli.argv.clone());
        if let (Some(flag), Some(model)) = (cli.model_flag.as_ref(), plan.model.as_ref()) {
            argv.push(flag.clone());
            argv.push(model.clone());
        }
        if !mcp_has_reasoning {
            agent_dispatch::append_reasoning_effort(
                &mut argv,
                harness,
                cli,
                plan.reasoning_effort.as_deref(),
            );
        }
    }
    if let Some(mcp) = mcp {
        if let Some(flag) = mcp.mcp_config_flag.as_ref() {
            let exe = std::env::current_exe().map_err(|source| crate::Error::Command {
                message: format!("cannot resolve current executable for MCP config: {source}"),
            })?;
            // The spawned server's own elapsed-evidence baseline: cumulative
            // active-drive seconds already accrued by this drive loop before
            // the harness started, so a call the harness submits directly
            // through this subprocess (never routed back through the parent
            // drive loop) observes a fresh, growing elapsed value instead of
            // the always-zero default. See `mcp::initialize_elapsed_baseline`
            // and this variable's contract in `ctx_traits_io::env_reference`.
            let config = serde_json::json!({
                "mcpServers": {
                    "ctx": {
                        "command": exe.to_string_lossy(),
                        "args": ["traits", "mcp"],
                        "env": {
                            "CTX_TRAITS_ELAPSED_SECONDS_BASELINE": elapsed_seconds.unwrap_or(0).to_string()
                        }
                    }
                }
            });
            argv.push(flag.clone());
            argv.push(config.to_string());
        }
        if let Some(flag) = mcp.allowed_tools_flag.as_ref() {
            argv.push(flag.clone());
            argv.push(if mcp.allowed_tools.is_empty() {
                "mcp__ctx__*".to_string()
            } else {
                mcp.allowed_tools.join(",")
            });
        }
        if let (Some(flag), Some(effort)) = (
            mcp.reasoning_effort_flag.as_ref(),
            plan.reasoning_effort.as_ref(),
        ) {
            argv.push(flag.clone());
            argv.push(effort.clone());
        }
        if let Some(flag) = mcp.system_prompt_flag.as_ref() {
            argv.push(flag.clone());
            argv.push(composed_system_prompt(
                agent_system,
                plan.system_prompt.as_deref(),
            ));
        }
    }
    argv.extend(plan.extra_args.clone());
    Ok(argv)
}

/// Layer the standing instructions a frame's agent will see, most specific
/// first: the trait's own `[[agent]] system` (what the role IS, digest-locked
/// and audited), then the operator's profile `system-prompt` (this machine's
/// overlay), then ctx's frame discipline. An operator adds to package intent;
/// nothing silently replaces it. Blank layers are dropped.
fn composed_system_prompt(agent_system: Option<&str>, configured: Option<&str>) -> String {
    let mut layers: Vec<&str> = Vec::new();
    layers.extend(agent_system.filter(|prompt| !prompt.trim().is_empty()));
    layers.extend(configured.filter(|prompt| !prompt.trim().is_empty()));
    layers.push(default_system_prompt());
    layers.join("\n\n")
}

fn composed_narrator_system_prompt(configured: Option<&str>) -> String {
    match configured.filter(|prompt| !prompt.trim().is_empty()) {
        Some(prompt) => format!("{prompt}\n\n{}", harness_stream::narrator_system_prompt()),
        None => harness_stream::narrator_system_prompt().to_string(),
    }
}

fn default_system_prompt() -> &'static str {
    "You are serving exactly one ctx.traits frame. Do only that frame's requested work, submit exactly the requested outputs with agent and harness provenance through the provided ctx.traits channel, then stop. Do not invent loop control; ctx owns advancement."
}

struct CurrentMcpFrame<'a> {
    frame: &'a ctx_traits_core::procedure::runtime::SequenceFrame,
    role: &'a str,
    session: &'a ctx_traits_core::procedure::session::Session,
    prompt: &'a ResolvedFramePrompt,
    /// Resolved `[worktree].env` overlay applied to the MCP harness spawn.
    env_overlay: &'a BTreeMap<String, String>,
    /// Cumulative active-drive elapsed seconds observed by this drive loop
    /// right before this frame's MCP harness is spawned. Handed to the
    /// spawned `ctx traits mcp` subprocess as its elapsed-evidence baseline
    /// (see `mcp_harness_argv`) so a call the harness submits directly
    /// through MCP observes the same growing cumulative value the CLI
    /// transport does, instead of accepting that frame against stale
    /// (always-zero) elapsed evidence.
    elapsed_seconds: Option<u64>,
    /// P480: this worktree's generated OS-level spawn sandbox, `None` for a
    /// non-worktree drive, `sandbox = false`, or an unsupported platform.
    sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
}

struct LiveFramePresentation<'a> {
    narrator: Option<harness_stream::NarratorConfig>,
    run_panel: Option<&'a run_view::RunPanel>,
    narrator_tokens: &'a harness_stream::NarratorTokenTracker,
}

/// Absolutize a resolved run-session ledger path so a worktree-scoped harness
/// subprocess (a different cwd than the invocation) still addresses the
/// original ledger.
fn absolute_session_path(path: &camino::Utf8Path) -> crate::Result<camino::Utf8PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|source| crate::Error::Command {
        message: format!("cannot resolve current directory: {source}"),
    })?;
    let cwd = camino::Utf8PathBuf::from_path_buf(cwd).map_err(|_| crate::Error::Command {
        message: "current directory is not valid UTF-8".to_string(),
    })?;
    Ok(cwd.join(path))
}

#[allow(clippy::too_many_arguments)]
fn drive_mcp_frame(
    input: &DriveInputs<'_>,
    report: &mut DriveReport,
    budget: &Budget,
    harness: &ctx_traits_io::harness_config::HarnessDefinition,
    plan: &AssignmentPlan,
    presentation: LiveFramePresentation<'_>,
    current: CurrentMcpFrame<'_>,
    activity: &ActivityRecorder,
) -> crate::Result<bool> {
    let cli = harness.cli.as_ref();
    let prompt_delivery = if cli.and_then(|cli| cli.prompt_via.as_deref()) == Some("stdin") {
        ctx_traits_io::harness::PromptDelivery::Stdin
    } else {
        ctx_traits_io::harness::PromptDelivery::Arg
    };
    let agent_system = current
        .frame
        .assigned_agent
        .as_ref()
        .and_then(|agent| agent.system.as_deref());
    let argv = mcp_harness_argv(
        harness,
        plan,
        agent_system,
        harness.mcp.as_ref(),
        cli,
        current.elapsed_seconds,
    )?;
    // A worktree-scoped MCP harness spawns its own `ctx traits mcp` subprocess
    // from a different cwd; normalize the ledger address to an absolute path so
    // that nested call still finds the collision-safe session ledger. Without a
    // worktree the prompt stays byte-for-byte unchanged.
    let (session_arg, session_store_arg) = if input.execution_dir.is_some() {
        let resolved =
            ctx_traits_io::run_session::resolve_session_path(input.session, input.session_store)?;
        (absolute_session_path(&resolved)?.to_string(), None)
    } else {
        (
            input.session.to_string(),
            input.session_store.map(ToString::to_string),
        )
    };
    let prompt = mcp_frame_prompt(
        &session_arg,
        session_store_arg.as_deref(),
        current.frame,
        current.prompt,
        current.role,
        &plan.harness_id,
    );
    let mut attempt = 0;
    loop {
        report.frames_attempted += 1;
        progress(
            input.progress,
            &format!("mcp frame started {}@{}", current.role, plan.harness_id),
        );
        let live_output = live_harness_output(
            input.progress,
            current.role,
            &plan.harness_id,
            presentation.narrator.clone(),
            presentation.run_panel,
            presentation.narrator_tokens,
            activity,
        );
        let run = ctx_traits_io::harness::run(ctx_traits_io::harness::HarnessRunRequest {
            argv: argv.clone(),
            env_overlay: current.env_overlay.clone(),
            env_remove: agent_dispatch::harness_env_remove(harness),
            prompt: prompt.clone(),
            prompt_delivery: prompt_delivery.clone(),
            timeout_ms: budget.frame_seconds.saturating_mul(1000),
            idle_timeout_ms: budget
                .idle_seconds
                .map(|seconds| seconds.saturating_mul(1000)),
            capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
            stream: cli.is_some_and(|cli| cli.stream()),
            stdout_observer: live_output.as_ref().map(LiveHarnessOutput::observer),
            tick_observer: live_output
                .as_ref()
                .and_then(LiveHarnessOutput::tick_observer),
            exec_dir: input.execution_dir.map(camino::Utf8Path::to_path_buf),
            sandbox: current.sandbox.clone(),
        })?;
        if live_output.is_none()
            && let Some(output_id) = cli.and_then(|cli| cli.output.as_deref())
        {
            emit_output_progress(
                input.progress,
                output_id,
                &run.stdout,
                current.role,
                &plan.harness_id,
            );
        }
        report.events.push(DriveEvent {
            event: "mcp-harness-run".to_string(),
            role: Some(current.role.to_string()),
            harness: Some(plan.harness_id.clone()),
            detail: format!(
                "exit={} timed-out={} idle-timed-out={} argv={}",
                crate::app::presentation::optional(run.exit_code),
                run.timed_out,
                run.idle_timed_out,
                crate::app::presentation::argv_display(&run.argv)
            ),
            duration_ms: Some(run.duration_ms),
        });
        let refreshed = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
            trait_file: input.file,
            trait_id: None,
            session: input.session,
            session_store: input.session_store,
            elapsed_seconds: current.elapsed_seconds,
        })?;
        report.final_session_status = Some(refreshed.session.status.clone());
        // The session's own state advancement is authoritative: if the
        // harness already submitted this frame through MCP before exiting
        // with a provider error, the ledger has moved on and must be
        // accepted, not overwritten with a pause for a frame that no longer
        // needs resuming.
        if refreshed.session.state_digest != current.session.state_digest {
            report.frames_accepted += 1;
            activity.finish_frame(
                &current
                    .frame
                    .item_id
                    .clone()
                    .unwrap_or_else(|| current.frame.title.clone()),
            );
            let loaded_trait = ctx_traits_io::run::load_trait_for_session(
                input.file,
                None,
                &refreshed.session,
                "mcp structured progress",
            )?;
            emit_structured_completion_progress(
                input.progress,
                &loaded_trait.trait_ref,
                current.frame,
                &refreshed.session.accepted_slot_values,
                &refreshed.session.accepted_output_port_values,
                &current.frame.title,
                current.role,
            );
            match live_output {
                Some(live_output) => live_output.finish_accepted(
                    &format!(
                        "sequence step completed {}@{}",
                        current.role, plan.harness_id
                    ),
                    &refreshed.session,
                ),
                None => {
                    progress(
                        input.progress,
                        &format!(
                            "sequence step completed {}@{}",
                            current.role, plan.harness_id
                        ),
                    );
                    progress_finish(input.progress, current.role, &plan.harness_id);
                }
            }
            report.events.push(DriveEvent {
                event: "mcp-state-advanced".to_string(),
                role: Some(current.role.to_string()),
                harness: Some(plan.harness_id.clone()),
                detail: format!("digest advanced to {}", refreshed.session.state_digest),
                duration_ms: Some(run.duration_ms),
            });
            return Ok(true);
        }
        let mcp_output_id = cli.and_then(|cli| cli.output.as_deref()).unwrap_or("");
        let mcp_stream_events = provider_error_stream_events(mcp_output_id, &run.stdout);
        if let Some(ProviderErrorClassification::CreditsExhausted(credits)) =
            classify_provider_error(
                mcp_output_id,
                &run.stdout,
                &run.stderr,
                run.exit_code,
                &mcp_stream_events,
            )
        {
            // Classify before the retry counter below increments: every
            // retry answers identically and burns the dying balance doing
            // so, same as the CLI harness loop above. The digest-unchanged
            // check above already ruled out a frame that already advanced.
            apply_credits_pause(
                report,
                current.frame,
                CreditsPauseEvent {
                    event_name: "mcp-harness-provider-credits-exhausted",
                    role: current.role,
                    harness_id: &plan.harness_id,
                    duration_ms: run.duration_ms,
                },
                &credits,
            );
            return Ok(false);
        }
        if run.killed {
            report.status = "killed".to_string();
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-execution.{}", plan.harness_id),
                    report.status.clone(),
                ),
            );
            return Ok(false);
        }
        refresh_existing_run_panel(presentation.run_panel, &refreshed.session);
        attempt += 1;
        if attempt > budget.max_retries {
            report.status = if run.idle_timed_out {
                "mcp-idle-timeout".to_string()
            } else if run.timed_out {
                "mcp-frame-timeout".to_string()
            } else if run.exit_code != Some(0) {
                "mcp-harness-failed".to_string()
            } else {
                "mcp-no-state-advance".to_string()
            };
            let status = report.status.clone();
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-execution.{}", plan.harness_id),
                    status,
                ),
            );
            return Ok(false);
        }
    }
}

use ctx_traits_core::procedure::runtime::ProviderErrorClassification;

/// Bounded event context for a single [`apply_credits_pause`] call: the
/// identifying strings and timing a `DriveEvent` records, grouped so the
/// builder itself takes one cohesive argument instead of several loose
/// scalars.
struct CreditsPauseEvent<'a> {
    event_name: &'a str,
    role: &'a str,
    harness_id: &'a str,
    duration_ms: u128,
}

/// Shared by the CLI harness loop and the MCP frame loop: build the typed
/// `ProviderCreditsPause` from the paused `SequenceFrame`, record the pause
/// event, and mark the report paused. One shared builder instead of two
/// near-identical ~20-line blocks.
fn apply_credits_pause(
    report: &mut DriveReport,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    event: CreditsPauseEvent<'_>,
    credits: &ctx_traits_core::procedure::runtime::CreditsExhaustedEvidence,
) {
    report.status = "paused-provider-credits".to_string();
    let pause = ctx_traits_core::procedure::runtime::ProviderCreditsPause {
        provider: credits.provider.clone(),
        role: event.role.to_string(),
        frame_title: frame.title.clone(),
        frame_item_id: frame.item_id.clone(),
        frame_run_index: frame.run_index.unwrap_or_default(),
        top_up_url: credits.top_up_url.clone(),
        detail: credits.detail.clone(),
    };
    report.events.push(DriveEvent {
        event: event.event_name.to_string(),
        role: Some(event.role.to_string()),
        harness: Some(event.harness_id.to_string()),
        detail: format!("{} {}", pause.provider, pause.detail),
        duration_ms: Some(event.duration_ms),
    });
    report.credits_pause = Some(pause);
}

/// Build the pure evidence the core classifier needs from a harness run,
/// decoding stdout into stream events only for the output shapes this module
/// knows how to parse — the extraction step the core classifier itself must
/// not perform (it has no knowledge of a given harness's NDJSON/balanced-JSON
/// output shape).
fn classify_provider_error<'a>(
    output_id: &'a str,
    stdout: &'a str,
    stderr: &'a str,
    exit_code: Option<i32>,
    stream_events: &'a [Value],
) -> Option<ProviderErrorClassification> {
    let evidence = ctx_traits_core::procedure::runtime::ProviderErrorEvidence {
        output_id,
        stream_events,
        stdout,
        stderr,
        exit_code,
    };
    ctx_traits_core::procedure::runtime::classify_provider_error(&evidence)
}

fn provider_error_stream_events(output_id: &str, stdout: &str) -> Vec<Value> {
    match output_id {
        id if harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => {
            harness_stream::stream_values(stdout)
        }
        _ => Vec::new(),
    }
}

/// Make correction retries visible: without this, a retried frame repaints as
/// the same step "looping" and gets interrupted by operators who cannot tell a
/// deliberate correction pass from a stuck run (observed live on the first
/// wrong-slot verdict).
#[allow(clippy::too_many_arguments)]
fn announce_retry(
    mode: cli::DriveProgress,
    run_panel: Option<&run_view::RunPanel>,
    role: &str,
    harness_id: &str,
    attempt: u64,
    max_retries: u64,
    why: &str,
    activity: &ActivityRecorder,
    frame_id: &str,
) {
    let note = format!("correction retry {attempt}/{max_retries} {role}@{harness_id}: {why}");
    progress(mode, &note);
    if let Some(panel) = run_panel {
        panel.push_summary(note);
    }
    activity.emit_retry(frame_id, attempt, max_retries, why);
}

fn parse_harness_output(
    stdout: &str,
    output_id: &str,
    requested: &[RequestedSlotKey],
) -> Result<ParsedHarnessOutput, String> {
    let mut result = ParsedHarnessOutput {
        slots: BTreeMap::new(),
        harness_session_id: None,
        observed_keys: BTreeSet::new(),
    };
    let values = match output_id {
        id if harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => {
            harness_stream::stream_values(stdout)
        }
        "raw-json" | "claude-json" => vec![
            serde_json::from_str::<Value>(stdout)
                .map_err(|err| format!("failed to parse {output_id} harness output JSON: {err}"))?,
        ],
        other => return Err(format!("unsupported harness output parser {other:?}")),
    };
    if values.is_empty() {
        return Err(format!(
            "{output_id} harness output did not contain parseable JSON events"
        ));
    }
    for value in values {
        if result.harness_session_id.is_none() {
            result.harness_session_id = harness_stream::session_id_from_event(output_id, &value);
        }
        if let Some(slots) = slots_from_value(
            &value,
            requested,
            &mut result.observed_keys,
            output_id == "raw-json",
        )? {
            result.slots = slots;
        }
    }
    Ok(result)
}

fn emit_output_progress(
    mode: cli::DriveProgress,
    output_id: &str,
    stdout: &str,
    role: &str,
    harness_id: &str,
) {
    if mode == cli::DriveProgress::None {
        return;
    }
    let values = match output_id {
        id if harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => {
            harness_stream::stream_values(stdout)
        }
        "raw-json" | "claude-json" => serde_json::from_str::<Value>(stdout).into_iter().collect(),
        _ => Vec::new(),
    };
    match mode {
        cli::DriveProgress::None => {}
        cli::DriveProgress::Status => {
            for event in harness_stream::progress_events(&values) {
                eprintln!("{event} {role}@{harness_id}");
            }
        }
        cli::DriveProgress::Stream => {
            for text in harness_stream::progress_texts(&values) {
                eprintln!("[{role}@{harness_id}] {text}");
            }
        }
        cli::DriveProgress::Tui => {}
    }
}

fn live_harness_output(
    mode: cli::DriveProgress,
    role: &str,
    harness_id: &str,
    narrator: Option<harness_stream::NarratorConfig>,
    run_panel: Option<&run_view::RunPanel>,
    narrator_tokens: &harness_stream::NarratorTokenTracker,
    activity: &ActivityRecorder,
) -> Option<LiveHarnessOutput> {
    if mode == cli::DriveProgress::Tui {
        let panel = run_panel?.clone();
        let Some(narrator) = narrator else {
            return Some(LiveHarnessOutput::run_passthrough(panel));
        };
        let sink = panel.clone();
        let tokens_sink = panel.clone();
        let step_summary_sink = panel.clone();
        let thinking_tokens_sink = panel.clone();
        let step_summary_activity = activity.clone();
        let stream_narrator = harness_stream::StreamNarrator::new(
            narrator,
            harness_stream::NarratorSinks {
                summary: Arc::new(move |summary| sink.push_summary(summary)),
                tokens: Arc::new(move |tokens| tokens_sink.add_narrator_tokens(tokens)),
                step_summary: Arc::new(
                    move |context: harness_stream::StepSummaryContext, summary: String| {
                        // Persist independent of whether the TUI panel is
                        // still live — the step-summary sidecar record is
                        // P521's highest-priority summary-line source.
                        step_summary_activity.record_step_summary(
                            context.key.clone(),
                            context.role.clone(),
                            &summary,
                        );
                        step_summary_sink.push_step_summary(
                            &run_view::CompletedStepContext {
                                key: context.key,
                                label: context.label,
                                role: context.role,
                                elapsed: context.elapsed,
                                work_tokens: context.work_tokens,
                            },
                            summary,
                        );
                    },
                ),
                thinking_tokens: Arc::new(move |tokens| {
                    thinking_tokens_sink.set_thinking_tokens(tokens)
                }),
            },
            narrator_tokens.clone(),
        );
        return Some(LiveHarnessOutput::run_narrated(panel, stream_narrator));
    }
    if mode != cli::DriveProgress::Stream || !tui::stderr_supports_live(false) {
        return None;
    }
    let panel = tui::LiveOutputPanel::new(format!("{role}@{harness_id}"));
    let Some(narrator) = narrator else {
        return Some(LiveHarnessOutput::passthrough(panel));
    };
    let sink = panel.clone();
    let thinking_tokens_sink = panel.clone();
    let stream_narrator = harness_stream::StreamNarrator::new(
        narrator,
        harness_stream::NarratorSinks {
            summary: Arc::new(move |summary| sink.push_summary(summary)),
            tokens: Arc::new(|_tokens| {}),
            // `--progress stream` has no `RunPanel`, so `finish_with_step_summary`
            // is never invoked on this narrator; this sink is unreachable.
            step_summary: Arc::new(|_context, _summary| {}),
            thinking_tokens: Arc::new(move |tokens| {
                thinking_tokens_sink.push_thinking_tokens(tokens)
            }),
        },
        narrator_tokens.clone(),
    );
    Some(LiveHarnessOutput::narrated(panel, stream_narrator))
}

struct NarratorFrameContext<'a> {
    session: &'a ctx_traits_core::procedure::session::Session,
    item_id: Option<&'a str>,
    task_label: &'a str,
    trace_sequence: &'a std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// [`narrator_config`]'s return: the resolved [`harness_stream::NarratorConfig`]
/// (`None` when no narrator applies at all) alongside a P478 capability
/// report for a worktree spawn whose narrator harness kind has no
/// write-confinement renderer. Split out rather than folded into a warning
/// buried in `NarratorConfig` because both call sites build `report` (a
/// `DriveReport`, not reachable from inside this function without a
/// conflicting mutable borrow at the struct-literal call sites) and must
/// push the capability themselves.
/// P521 assisted story level: resolve a `NarratorConfig` OFFLINE, outside
/// any live drive — the machine's current `[agent.narrator]` seat, cold-only
/// (`warm: None`), no worktree confinement, `exec_dir: None`. Reuses the
/// same `resolve_runtime_assignments`/`resolved_narrator_assignment` path a
/// live drive uses so the two never diverge, but deliberately skips
/// `narrator_config`'s worktree/warm-pool/confinement machinery — an
/// assisted-level story render must never spend a warm session turn or
/// borrow another seat's model when the resolved narrator harness is
/// missing or unsupported; it degrades instead of guessing. `trace` is
/// keyed `story-assisted` so any spend from this path leaves a debug trace
/// distinct from a live drive's narration.
pub(crate) fn resolve_offline_narrator_config(
    run_id: &str,
    session_id: &str,
    frame_key: &str,
) -> Option<harness_stream::NarratorConfig> {
    let mut profile = ctx_traits_io::harness_config::resolve_runtime_assignments(&[]).ok()?;
    let assignment = profile.resolved_narrator_assignment().ok()??;
    let plan = plan_from_assignment(assignment, None, None);
    if plan.mode != ctx_traits_io::harness_config::RunAssignmentMode::Harness
        || plan.transport != ctx_traits_io::harness_config::RunTransport::Cli
    {
        return None;
    }
    let harness = profile.registry.harness.get(&plan.harness_id)?;
    if !harness
        .transports
        .contains(&ctx_traits_io::harness_config::RunTransport::Cli)
    {
        return None;
    }
    let cli = harness.cli.as_ref()?;
    let prompt_delivery = if cli.prompt_via.as_deref() == Some("stdin") {
        ctx_traits_io::harness::PromptDelivery::Stdin
    } else {
        ctx_traits_io::harness::PromptDelivery::Arg
    };
    Some(harness_stream::NarratorConfig {
        argv: narrator_harness_argv(harness, cli, &plan, None, None),
        env_overlay: BTreeMap::new(),
        env_remove: agent_dispatch::harness_env_remove(harness),
        warm: None,
        prompt_delivery,
        output_id: cli.output.clone(),
        task_label: frame_key.to_string(),
        timeout_ms: narrator_timeout_ms(&profile),
        exec_dir: None,
        confinement: None,
        sandbox: None,
        trace: Some(harness_stream::NarratorTraceContext {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            item_id: Some(frame_key.to_string()),
            frame_title: frame_key.to_string(),
            harness_id: plan.harness_id.clone(),
            sequence: std::sync::Arc::new(AtomicU64::new(0)),
        }),
    })
}

struct NarratorResolution {
    config: Option<harness_stream::NarratorConfig>,
    /// P478/P480: 0-2 entries — the harness-native renderer gap, the
    /// OS-level sandbox gap, or both.
    unsupported_confinement: Vec<ctx_traits_core::response::CapabilityReport>,
}

fn narrator_config(
    input: &DriveInputs<'_>,
    profile: &ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    plan: Option<&AssignmentPlan>,
    frame: NarratorFrameContext<'_>,
    narrator_warm_pool: &harness_stream::NarratorWarmPool,
    env_overlay: &BTreeMap<String, String>,
    confinement_payloads: Option<&ctx_traits_io::confinement::ConfinementPayloads>,
) -> NarratorResolution {
    let none = NarratorResolution {
        config: None,
        unsupported_confinement: Vec::new(),
    };
    if !matches!(
        input.progress,
        cli::DriveProgress::Stream | cli::DriveProgress::Tui
    ) {
        return none;
    }
    let Some(plan) = plan else {
        return none;
    };
    if plan.mode != ctx_traits_io::harness_config::RunAssignmentMode::Harness
        || plan.transport != ctx_traits_io::harness_config::RunTransport::Cli
    {
        return none;
    }
    let Some(harness) = profile.registry.harness.get(&plan.harness_id) else {
        return none;
    };
    if !harness
        .transports
        .contains(&ctx_traits_io::harness_config::RunTransport::Cli)
    {
        return none;
    }
    let Some(cli) = harness.cli.as_ref() else {
        return none;
    };
    // P478/P480: a worktree spawn (confinement_payloads.is_some()) whose
    // harness kind has no renderer, or whose requested OS-level sandbox is
    // unavailable, must report that explicitly rather than dispatch silently
    // less confined than declared.
    let spawn_sandbox = confinement_payloads.and_then(|payloads| payloads.spawn_sandbox.clone());
    let mut unsupported_confinement = Vec::new();
    if let Some(payloads) = confinement_payloads {
        unsupported_confinement.extend(
            ctx_traits_io::confinement::confinement_unsupported_capability(
                harness.kind(),
                spawn_sandbox.is_some(),
            ),
        );
        unsupported_confinement.extend(
            ctx_traits_io::confinement::spawn_sandbox_unsupported_capability(
                payloads.sandbox_requested,
                payloads.spawn_sandbox.as_ref(),
            ),
        );
    }
    let exec_dir = input.execution_dir.map(camino::Utf8Path::to_path_buf);
    let confinement_trace = confinement_payloads
        .and_then(|payloads| {
            ctx_traits_io::confinement::confinement_trace_payload(payloads, harness.kind())
        })
        .cloned();
    let warm = (plan.session_mode == ctx_traits_io::harness_config::RunSessionMode::Persistent)
        .then(|| {
            warm_harness_argv(
                harness,
                cli,
                plan,
                None,
                WarmPromptKind::Narrator,
                confinement_payloads,
                // The narrator's own conversation identity is out of P516's
                // wiring scope (the draft's persisted-session fix targets
                // the assigned-agent dispatch path only); unchanged from
                // pre-P516 behavior.
                None,
            )
        })
        .flatten()
        .map(|argv| {
            harness_stream::NarratorWarmConfig::new(
                argv,
                env_overlay.clone(),
                agent_dispatch::harness_env_remove(harness),
                exec_dir.clone(),
                narrator_warm_pool.clone(),
                spawn_sandbox.clone(),
            )
        });
    let prompt_delivery = if cli.prompt_via.as_deref() == Some("stdin") {
        ctx_traits_io::harness::PromptDelivery::Stdin
    } else {
        ctx_traits_io::harness::PromptDelivery::Arg
    };
    NarratorResolution {
        config: Some(harness_stream::NarratorConfig {
            argv: narrator_harness_argv(
                harness,
                cli,
                plan,
                exec_dir.as_deref(),
                confinement_payloads,
            ),
            env_overlay: env_overlay.clone(),
            env_remove: agent_dispatch::harness_env_remove(harness),
            warm,
            prompt_delivery,
            output_id: cli.output.clone(),
            task_label: frame.task_label.to_string(),
            timeout_ms: narrator_timeout_ms(profile),
            exec_dir,
            confinement: confinement_trace,
            sandbox: spawn_sandbox,
            trace: Some(harness_stream::NarratorTraceContext {
                run_id: frame.session.run_id.as_str().to_string(),
                session_id: frame.session.session_id.as_str().to_string(),
                item_id: frame.item_id.map(str::to_string),
                frame_title: frame.task_label.to_string(),
                harness_id: plan.harness_id.clone(),
                sequence: std::sync::Arc::clone(frame.trace_sequence),
            }),
        }),
        unsupported_confinement,
    }
}

/// The context a cold (never persistent-session) one-shot narrator dispatch
/// needs, grouped into one struct so [`cold_narrator_config`]'s own
/// signature stays under clippy's argument-count lint without suppressing
/// it — shared by [`cold_narrator_config_for_merge`] and
/// [`cold_narrator_config_for_session_title`], which differ only in the
/// `task_label`/trace `item_id` they dispatch under.
pub(crate) struct ColdNarratorContext<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) env_overlay: &'a BTreeMap<String, String>,
    pub(crate) confinement_payloads: Option<&'a ctx_traits_io::confinement::ConfinementPayloads>,
    pub(crate) exec_dir: Option<&'a camino::Utf8Path>,
    pub(crate) trace_sequence: &'a Arc<AtomicU64>,
}

/// Resolve a cold [`harness_stream::NarratorConfig`], reusing the exact same
/// harness/argv/prompt-delivery/confinement resolution [`narrator_config`]
/// uses for a driven frame's narrator. `warm` is always `None` here — unlike
/// a driven frame, a cold dispatch's own harness call is a one-shot, so its
/// narrator never needs a persistent conversation regardless of the resolved
/// role's `session-mode`. `None` means "no narrator seat configured" — the
/// caller then degrades (passthrough for merge, a permanently blank title
/// row for session-title), matching the seat doctrine ("absent narrator
/// table means passthrough").
fn cold_narrator_config(
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    ctx: ColdNarratorContext<'_>,
    task_label: &str,
    trace_item_id: Option<String>,
) -> Option<harness_stream::NarratorConfig> {
    let assignment = profile.resolved_narrator_assignment().ok().flatten()?;
    let plan = plan_from_assignment(assignment, None, None);
    if plan.mode != ctx_traits_io::harness_config::RunAssignmentMode::Harness
        || plan.transport != ctx_traits_io::harness_config::RunTransport::Cli
    {
        return None;
    }
    let harness = profile.registry.harness.get(&plan.harness_id)?;
    if !harness
        .transports
        .contains(&ctx_traits_io::harness_config::RunTransport::Cli)
    {
        return None;
    }
    let cli = harness.cli.as_ref()?;
    let spawn_sandbox = ctx
        .confinement_payloads
        .and_then(|payloads| payloads.spawn_sandbox.clone());
    let confinement_trace = ctx
        .confinement_payloads
        .and_then(|payloads| {
            ctx_traits_io::confinement::confinement_trace_payload(payloads, harness.kind())
        })
        .cloned();
    let exec_dir_owned = ctx.exec_dir.map(camino::Utf8Path::to_path_buf);
    let prompt_delivery = if cli.prompt_via.as_deref() == Some("stdin") {
        ctx_traits_io::harness::PromptDelivery::Stdin
    } else {
        ctx_traits_io::harness::PromptDelivery::Arg
    };
    Some(harness_stream::NarratorConfig {
        argv: narrator_harness_argv(
            harness,
            cli,
            &plan,
            exec_dir_owned.as_deref(),
            ctx.confinement_payloads,
        ),
        env_overlay: ctx.env_overlay.clone(),
        env_remove: agent_dispatch::harness_env_remove(harness),
        warm: None,
        prompt_delivery,
        output_id: cli.output.clone(),
        task_label: task_label.to_string(),
        timeout_ms: narrator_timeout_ms(profile),
        exec_dir: exec_dir_owned,
        confinement: confinement_trace,
        sandbox: spawn_sandbox,
        trace: Some(harness_stream::NarratorTraceContext {
            run_id: ctx.run_id.to_string(),
            session_id: ctx.session_id.to_string(),
            item_id: trace_item_id,
            frame_title: task_label.to_string(),
            harness_id: plan.harness_id.clone(),
            sequence: Arc::clone(ctx.trace_sequence),
        }),
    })
}

/// P549: resolve a cold narrator config for the automatic merge span. See
/// [`cold_narrator_config`].
pub(crate) fn cold_narrator_config_for_merge(
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    ctx: ColdNarratorContext<'_>,
) -> Option<harness_stream::NarratorConfig> {
    cold_narrator_config(profile, ctx, "merge", None)
}

/// P552: resolve a cold, one-shot narrator config for the session-title
/// dispatch. Unlike [`resolve_offline_narrator_config`] (the P521
/// assisted-story path this used to reuse), the caller passes this drive's
/// own resolved profile, worktree environment overlay, and confinement
/// payloads, so a worktree-confined or CLI-override-assigned narrator seat is
/// honored for the title call exactly as it would be for ordinary live
/// narration — the title call is simply independent of `--progress` mode.
/// See [`cold_narrator_config`].
pub(crate) fn cold_narrator_config_for_session_title(
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    ctx: ColdNarratorContext<'_>,
) -> Option<harness_stream::NarratorConfig> {
    cold_narrator_config(
        profile,
        ctx,
        "session-title",
        Some("session-title".to_string()),
    )
}

/// Search a harness output value for an object satisfying every requested
/// slot key, walking the same wrapper-key/array/string-embedded-JSON shapes
/// as any other harness-event traversal (shared with `merge.rs`'s decision
/// parsing via [`harness_stream::find_nested_object`]), so a slot named in a
/// deeply nested OpenCode-style event payload is found just as reliably as
/// one at the top level of a Claude-style event.
fn slots_from_value(
    value: &Value,
    requested: &[RequestedSlotKey],
    observed: &mut BTreeSet<String>,
    direct_model_output: bool,
) -> Result<Option<BTreeMap<String, Value>>, String> {
    let mut matcher = |object: &serde_json::Map<String, Value>| -> Option<BTreeMap<String, Value>> {
        let mut slots = BTreeMap::new();
        for requested in requested {
            let value = object.get(&requested.property)?;
            slots.insert(requested.ref_text.clone(), value.clone());
        }
        Some(slots)
    };
    // JSON parsed out of message text is model-authored: when it fails the
    // slot match, its keys are what the model chose to send — exactly what a
    // specific correction should quote back.
    let mut record_observed = |value: &Value| record_observed_keys(value, requested, observed);
    let slots = harness_stream::find_nested_object(value, &mut matcher, &mut record_observed);
    if slots.is_none() && direct_model_output {
        record_observed_keys(value, requested, observed);
    }
    Ok(slots)
}

fn record_observed_keys(
    value: &Value,
    requested: &[RequestedSlotKey],
    observed: &mut BTreeSet<String>,
) {
    if let Some(object) = value.as_object() {
        let candidate = object
            .keys()
            .filter(|key| {
                !matches!(
                    key.as_str(),
                    "session_id" | "sessionId" | "sessionID" | "session-id"
                )
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let requested_overlap = |keys: &BTreeSet<String>| {
            requested
                .iter()
                .filter(|requested| keys.contains(&requested.property))
                .count()
        };
        // Keep one failed candidate rather than unioning unrelated attempts:
        // a union can make several partial objects look complete.
        if observed.is_empty() || requested_overlap(&candidate) > requested_overlap(observed) {
            *observed = candidate;
        }
    }
}

struct HarnessSubmissionEvidence<'a> {
    role: &'a str,
    harness_id: &'a str,
    version: Option<String>,
    fallback_version: Option<String>,
    transport: &'a str,
    duration_ms: u128,
}

fn submit_harness_output(
    input: &DriveInputs<'_>,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    evidence_input: HarnessSubmissionEvidence<'_>,
    produced_slots: BTreeMap<String, Value>,
    env_overlay: &BTreeMap<String, String>,
    elapsed_seconds: Option<u64>,
    run_panel: Option<&run_view::RunPanel>,
) -> crate::Result<ctx_traits_io::run::CallOutcome> {
    let template = frame
        .call_template
        .as_ref()
        .ok_or_else(|| crate::Error::Command {
            message: "current frame has no call template".to_string(),
        })?;
    let evidence = format!(
        "harness={} version={} transport={} duration-ms={}",
        evidence_input.harness_id,
        evidence_input
            .version
            .as_deref()
            .or(evidence_input.fallback_version.as_deref())
            .unwrap_or("unknown"),
        evidence_input.transport,
        evidence_input.duration_ms
    );
    Ok(ctx_traits_io::run::call(ctx_traits_io::run::CallRequest {
        trait_file: input.file,
        trait_id: None,
        session: input.session,
        session_store: input.session_store,
        execution_dir: input.execution_dir,
        execution_env: env_overlay,
        elapsed_seconds,
        tick_observer: run_panel.map(run_view::RunPanel::tick_observer),
        submission: ctx_traits_core::procedure::session::CallSubmission {
            session_id: ctx_traits_core::procedure::session::SessionId::new(
                template.session_id.clone(),
            )?,
            run_id: Some(ctx_traits_core::procedure::run::Id::new(
                template.run_id.clone(),
            )?),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots,
            signals: BTreeMap::new(),
            warnings: vec![evidence.clone()],
            command_execution: None,
            caller: Some(ctx_traits_core::procedure::session::CallerProvenance {
                surface: "cli-drive".to_string(),
                caller: format!(
                    "ctx traits drive {}@{}",
                    evidence_input.role, evidence_input.harness_id
                ),
                agent: frame
                    .assigned_agent
                    .is_some()
                    .then(|| evidence_input.role.to_string()),
                harness: Some(evidence),
            }),
        },
        out: None,
    })?)
}

fn push_capability(
    report: &mut DriveReport,
    capability: ctx_traits_core::response::CapabilityReport,
) {
    report.capabilities.push(capability);
    report.capabilities.sort();
    report.capabilities.dedup();
}

/// The `run <n> / source <m>: <title>` prefix shared by every P479 tripwire
/// frame label, whether the frame is a command step (labeled before
/// `advance_commands` dispatches) or an agent frame (labeled before harness
/// dispatch) — kept as one function so the two call sites cannot drift.
fn frame_position_label(frame: &ctx_traits_core::procedure::runtime::SequenceFrame) -> String {
    format!(
        "run {} / source {}: {}",
        frame.run_index.map_or(0, |index| index + 1),
        frame.sequence_index.map_or(0, |index| index + 1),
        frame.title,
    )
}

/// P479: run one `Tripwire::checkpoint`, applying its outcome to `report`.
/// Returns `true` when the caller must stop the drive loop now (a `park`
/// policy finding) — `false` for no tripwire, no finding, a `warn` policy
/// finding, or a best-effort snapshot failure (which must never itself mask
/// the drive's own outcome; it becomes a warning + a capability report
/// instead, per the non-negotiable that unsupported/failed runtime features
/// are explicit, never silent).
fn tripwire_checkpoint(
    report: &mut DriveReport,
    ledger_path: &camino::Utf8Path,
    tripwire: Option<&mut ctx_traits_io::tripwire::Tripwire>,
) -> bool {
    let Some(wire) = tripwire else {
        return false;
    };
    match wire.checkpoint() {
        Ok(Some(finding)) => {
            record_out_of_tree_mutation(report, ledger_path, wire.policy(), finding)
        }
        Ok(None) => false,
        Err(error) => {
            report.warnings.push(format!(
                "out-of-tree mutation tripwire snapshot failed: {error}"
            ));
            push_capability(
                report,
                ctx_traits_core::response::CapabilityReport::unsupported(
                    "worktree.tripwire",
                    format!("snapshot failed: {error}"),
                ),
            );
            false
        }
    }
}

/// Record one out-of-tree-mutation finding: typed ledger evidence (durable
/// even if the process exits right after), a `DriveEvent`, and the stderr-
/// visible warning every policy carries. Returns `true` (the caller must stop
/// the drive loop) only for `policy = "park"` — window-scoped wording only,
/// per the Watch clause: this never names a process or claims to know WHO
/// wrote, only that the invocation repository changed while the named frame
/// was the only ctx-dispatched work in this run's worktree.
fn record_out_of_tree_mutation(
    report: &mut DriveReport,
    ledger_path: &camino::Utf8Path,
    policy: ctx_traits_io::tripwire::TripwirePolicy,
    finding: ctx_traits_io::tripwire::Finding,
) -> bool {
    if let Err(error) = ctx_traits_io::run_session::append_out_of_tree_mutation(
        ledger_path,
        finding.paths.clone(),
        finding.frame.clone(),
        policy.as_str(),
    ) {
        report.warnings.push(format!(
            "out-of-tree-mutation evidence not recorded: {error}"
        ));
    }
    let message = format!(
        "invocation repository changed while frame {} was the only ctx-dispatched work in this run's worktree; paths: {}",
        finding.frame,
        finding.paths.join(", ")
    );
    report.events.push(DriveEvent {
        event: "out-of-tree-mutation".to_string(),
        role: None,
        harness: None,
        detail: message.clone(),
        duration_ms: None,
    });
    report.warnings.push(message);
    if matches!(policy, ctx_traits_io::tripwire::TripwirePolicy::Park) {
        report.status = "out-of-tree-mutation".to_string();
        true
    } else {
        false
    }
}

/// Stamp a failed command step into the drive report: the session stays parked
/// on the command frame (retryable by a later drive) with all prior
/// acceptances persisted, so the drive ends with the real cause instead of a
/// bubbled process error that skips outcome recording.
/// Announce a command frame before its blocking execution: the TUI panel
/// already renders the running step, so print a status line only when no
/// panel exists (status/stream modes) — interactive commands can hold the
/// frame for a long time and silence reads as a hang.
fn command_started_event(
    session: &ctx_traits_core::procedure::session::Session,
    panel_active: bool,
) {
    if panel_active {
        return;
    }
    let Some(frame) = session.next_frame.as_ref() else {
        return;
    };
    let Some(command) = frame.command.as_ref() else {
        return;
    };
    let step = frame.item_id.as_deref().unwrap_or("unnamed");
    println!(
        "command step {step} running: {} (waiting for it to finish)",
        crate::app::presentation::argv_display(&command.argv)
    );
}

fn command_failure_event(
    report: &mut DriveReport,
    failure: &ctx_traits_io::run::CommandStepFailure,
) {
    let step = failure.item_id.as_deref().unwrap_or("unnamed");
    let mut detail = format!(
        "command step {step} failed without advancing: argv={} exit={} timed-out={}",
        crate::app::presentation::argv_display(&failure.argv),
        crate::app::presentation::optional(failure.exit_code),
        failure.timed_out
    );
    let tail = failure.report.trim();
    if !tail.is_empty() {
        let tail = tail
            .chars()
            .take(400)
            .collect::<String>()
            .replace('\n', " | ");
        detail.push_str(&format!(" report: {tail}"));
    }
    report.events.push(DriveEvent {
        event: "command-step-failed".to_string(),
        role: None,
        harness: None,
        detail: detail.clone(),
        duration_ms: None,
    });
    report.status = "command-step-failed".to_string();
    push_capability(
        report,
        ctx_traits_core::response::CapabilityReport::unsupported(
            "runtime.command-execution",
            detail,
        ),
    );
}

fn record_stop_reason(
    report: &mut DriveReport,
    session: &ctx_traits_core::procedure::session::Session,
    run_panel: Option<&run_view::RunPanel>,
) {
    let Some(summary) = run_view::stop_reason_summary(session) else {
        return;
    };
    report.events.push(DriveEvent {
        event: "run-stopped".to_string(),
        role: None,
        harness: None,
        detail: summary.clone(),
        duration_ms: None,
    });
    if let Some(panel) = run_panel {
        panel.push_summary(summary);
    }
}

fn progress(mode: cli::DriveProgress, message: &str) {
    if mode == cli::DriveProgress::Status {
        eprintln!("{message}");
    }
}

fn progress_startup(mode: cli::DriveProgress, message: &str) {
    if matches!(
        mode,
        cli::DriveProgress::Status | cli::DriveProgress::Stream
    ) {
        eprintln!("{message}");
    }
}

fn progress_finish(mode: cli::DriveProgress, role: &str, harness_id: &str) {
    if mode == cli::DriveProgress::Stream {
        eprintln!("sequence step completed {role}@{harness_id}");
    }
}

fn emit_structured_completion_progress(
    mode: cli::DriveProgress,
    trait_ref: &ctx_traits_core::Trait,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    accepted_slots: &[ctx_traits_core::procedure::runtime::Value],
    accepted_output_ports: &[ctx_traits_core::procedure::runtime::Value],
    step_title: &str,
    role: &str,
) {
    if !matches!(
        mode,
        cli::DriveProgress::Status | cli::DriveProgress::Stream
    ) {
        return;
    }
    let values = crate::app::structured_output::accepted_frame_values(
        frame,
        accepted_slots,
        accepted_output_ports,
    );
    let verdict = crate::app::structured_output::verdict_for_values(values.iter().copied())
        .unwrap_or_else(|| "accepted".to_string());
    for value in values {
        let Some(port_id) =
            crate::app::structured_output::port_id_for_value(trait_ref, &value.ref_text)
        else {
            continue;
        };
        let Some(rendered) =
            crate::app::structured_output::resolve(trait_ref, &port_id, &value.value)
        else {
            continue;
        };
        eprintln!(
            "{} ({}): {} - {} {} declared",
            step_title, role, verdict, rendered.count, port_id
        );
    }
}

fn format_status(status: &ctx_traits_core::procedure::session::Status) -> String {
    match status {
        ctx_traits_core::procedure::session::Status::AwaitingInput => "awaiting-input",
        ctx_traits_core::procedure::session::Status::WaitingOnHuman => "waiting-on-human",
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput => "awaiting-agent-output",
        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired => {
            "blocked-command-permission-required"
        }
        ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned => {
            "blocked-agent-unassigned"
        }
        ctx_traits_core::procedure::session::Status::Rejected => "rejected",
        ctx_traits_core::procedure::session::Status::Blocked => "blocked",
        ctx_traits_core::procedure::session::Status::Completed => "completed",
        ctx_traits_core::procedure::session::Status::Failed => "failed",
    }
    .to_string()
}

#[cfg(test)]
mod resolve_progress_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::{Value, json};

    use super::{
        AssignmentPlan, HarnessArgvAttempt, RejectionClass, RejectionHandling, RequestedSlotKey,
        harness_argv, resolve_correction_delivery, resolve_progress_with,
    };
    use crate::app::surface::cli::DriveProgress;
    use ctx_traits_core::procedure::runtime::{SchemaStatus, SchemaValidation};
    use ctx_traits_core::r#trait::procedure::WriteOperation;

    fn requested() -> Vec<RequestedSlotKey> {
        vec![RequestedSlotKey {
            ref_text: "slot:answer".to_string(),
            property: "answer".to_string(),
            operation: WriteOperation::Replace,
            schema_ref: Some("schema:boolean".to_string()),
        }]
    }

    #[test]
    fn absent_headless_keeps_status() {
        assert_eq!(
            resolve_progress_with(None, false, false, false),
            DriveProgress::Status
        );
    }

    #[test]
    fn absent_interactive_selects_tui() {
        assert_eq!(
            resolve_progress_with(None, false, false, true),
            DriveProgress::Tui
        );
    }

    #[test]
    fn absent_interactive_json_keeps_status() {
        assert_eq!(
            resolve_progress_with(None, true, false, true),
            DriveProgress::Status
        );
    }

    #[test]
    fn absent_interactive_no_tui_keeps_status() {
        assert_eq!(
            resolve_progress_with(None, false, true, true),
            DriveProgress::Status
        );
    }

    #[test]
    fn explicit_status_interactive_stays_status() {
        assert_eq!(
            resolve_progress_with(Some(DriveProgress::Status), false, false, true),
            DriveProgress::Status
        );
    }

    #[test]
    fn explicit_tui_json_downgrades_to_status() {
        assert_eq!(
            resolve_progress_with(Some(DriveProgress::Tui), true, false, false),
            DriveProgress::Status
        );
    }

    #[test]
    fn explicit_stream_json_keeps_stream() {
        assert_eq!(
            resolve_progress_with(Some(DriveProgress::Stream), true, false, true),
            DriveProgress::Stream
        );
    }

    #[test]
    fn explicit_none_mode_interactive_keeps_none() {
        assert_eq!(
            resolve_progress_with(Some(DriveProgress::None), false, false, true),
            DriveProgress::None
        );
    }

    #[test]
    fn correction_delivery_blocks_without_a_schema_channel() {
        let error = resolve_correction_delivery(1, true, false, None)
            .expect_err("a correction retry must not dispatch without schema delivery");
        assert_eq!(error.capability, "runtime.correction-schema-delivery");
    }

    #[test]
    fn corrections_are_actionable_without_internal_diagnostics() {
        let requested = requested();
        let schema = json!({"properties": {"answer": {"type": "boolean"}}});
        let validations = vec![SchemaValidation {
            ref_text: "slot:answer".to_string(),
            schema_ref: None,
            status: SchemaStatus::Rejected,
            reason: "internal validator wording".to_string(),
        }];
        let shapes = BTreeMap::from([(
            "slot:answer".to_string(),
            super::ReceivedShape {
                description: "string".to_string(),
            },
        )]);
        let observed = BTreeSet::from(["wrong".to_string()]);

        for class in [
            RejectionClass::ContentRejection,
            RejectionClass::OutputTruncated,
            RejectionClass::MissingSlot,
            RejectionClass::UnparseableOutput,
        ] {
            let correction =
                class.format_correction(&requested, &schema, &validations, &shapes, &observed);
            assert!(
                !correction.contains("internal validator wording"),
                "{class:?} leaked diagnostics: {correction}"
            );
            assert!(
                correction.contains("Return") || correction.contains("Answer"),
                "{class:?} did not state a repair: {correction}"
            );
        }
    }

    #[test]
    fn rejection_handling_assigns_budget_ownership_explicitly() {
        assert_eq!(
            RejectionClass::ContentRejection.handling(),
            RejectionHandling::ModelCorrection
        );
        assert_eq!(
            RejectionClass::MissingSlot.handling(),
            RejectionHandling::ModelCorrection
        );
        assert_eq!(
            RejectionClass::UnparseableOutput.handling(),
            RejectionHandling::ModelCorrection
        );
        assert_eq!(
            RejectionClass::StaleIdentity.handling(),
            RejectionHandling::RuntimeRedispatch
        );
        assert_eq!(
            RejectionClass::OutputTruncated.handling(),
            RejectionHandling::FreshConversationCorrection
        );
    }

    #[test]
    fn content_shape_uses_submitted_slots_not_failed_candidate_keys() {
        let requested = requested();
        let shapes = super::received_slot_shapes(
            &BTreeMap::from([("slot:answer".to_string(), json!({"z": true, "a": false}))]),
            &requested,
            &json!({"properties": {"answer": {"type": "object"}}}),
        );
        assert_eq!(
            shapes
                .get("slot:answer")
                .map(|shape| shape.description.as_str()),
            Some("object with fields: a, z")
        );
        let observed: BTreeSet<String> = BTreeSet::new();
        assert!(
            observed.is_empty(),
            "complete slot matches need no failed-candidate keys"
        );
    }

    #[test]
    fn missing_slot_uses_direct_raw_json_keys_and_only_absent_outputs() {
        let requested = vec![
            requested().pop().unwrap(),
            RequestedSlotKey {
                ref_text: "slot:other".to_string(),
                property: "other".to_string(),
                operation: WriteOperation::Replace,
                schema_ref: Some("schema:text".to_string()),
            },
        ];
        let parsed = super::parse_harness_output(
            r#"{"answer":true,"sessionID":"harness-metadata"}"#,
            "raw-json",
            &requested,
        )
        .unwrap();
        assert_eq!(parsed.observed_keys, BTreeSet::from(["answer".to_string()]));
        let correction = RejectionClass::MissingSlot.format_correction(
            &requested,
            &json!({}),
            &[],
            &BTreeMap::new(),
            &parsed.observed_keys,
        );
        assert!(
            correction.contains("missing: other")
                && correction.contains("observed: answer")
                && !correction.contains("missing: answer")
                && !correction.contains("sessionID"),
            "missing-slot evidence must be direct, model-authored, and accurate: {correction}"
        );
    }

    #[test]
    fn missing_slot_uses_nested_claude_candidate_without_envelope_keys() {
        let parsed = super::parse_harness_output(
            r#"{"type":"result","result":"{\"wrong\":true}","usage":{"x":1}}"#,
            "claude-json",
            &requested(),
        )
        .unwrap();
        assert_eq!(parsed.observed_keys, BTreeSet::from(["wrong".to_string()]));
    }

    #[test]
    fn parses_pi_ndjson_session_and_nested_message_end_output() {
        let parsed = super::parse_harness_output(
            concat!(
                r#"{"type":"session","id":"pi-session-123"}"#, "\n",
                r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"answer\":true}"}]}}"#
            ),
            "pi-json",
            &requested(),
        )
        .expect("Pi NDJSON parses through the generic stream traversal");

        assert_eq!(parsed.harness_session_id.as_deref(), Some("pi-session-123"));
        assert_eq!(parsed.slots.get("slot:answer"), Some(&json!(true)));
    }

    #[test]
    fn parses_codex_ndjson_thread_and_agent_message_output() {
        let parsed = super::parse_harness_output(
            concat!(
                r#"{"type":"thread.started","thread_id":"codex-thread-123"}"#, "\n",
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"answer\":true}"}}"#
            ),
            "codex-json",
            &requested(),
        )
        .expect("Codex JSONL parses through the generic stream traversal");

        assert_eq!(
            parsed.harness_session_id.as_deref(),
            Some("codex-thread-123")
        );
        assert_eq!(parsed.slots.get("slot:answer"), Some(&json!(true)));
    }

    #[test]
    fn worker_codex_argv_retains_subagent_controls() {
        let harness = ctx_traits_io::harness_config::built_in_harness_definition(
            "codex",
            &ctx_traits_io::harness_config::HarnessRegistry::default(),
        );
        let cli = harness.cli.as_ref().expect("Codex has a CLI convention");
        let plan = AssignmentPlan {
            harness_id: "codex".to_string(),
            transport: ctx_traits_io::harness_config::RunTransport::Cli,
            mode: ctx_traits_io::harness_config::RunAssignmentMode::Harness,
            session_mode: ctx_traits_io::harness_config::RunSessionMode::PerFrame,
            model: None,
            reasoning_effort: Some("high".to_string()),
            system_prompt: None,
            extra_args: vec![],
            model_resolution_evidence: None,
            from_session: false,
            seat_index: None,
            list_length: None,
        };

        let argv = harness_argv(
            &harness,
            cli,
            &plan,
            None,
            HarnessArgvAttempt {
                schema: None,
                harness_session_id: None,
                exec_dir: None,
                confinement: None,
            },
        );

        assert_eq!(
            argv,
            [
                "codex",
                "exec",
                "--json",
                "--config",
                "approval_policy=\"never\"",
                "--config",
                "agents.enabled=false",
                "--config",
                "features.multi_agent_v2=false",
                "--config",
                "model_reasoning_effort=\"high\""
            ]
        );
    }

    #[test]
    fn missing_slot_keeps_one_best_partial_candidate() {
        let requested = vec![
            requested().pop().unwrap(),
            RequestedSlotKey {
                ref_text: "slot:other".to_string(),
                property: "other".to_string(),
                operation: WriteOperation::Replace,
                schema_ref: Some("schema:text".to_string()),
            },
        ];
        let mut observed = BTreeSet::new();
        super::slots_from_value(&json!({"answer": true}), &requested, &mut observed, true).unwrap();
        super::slots_from_value(&json!({"other": "value"}), &requested, &mut observed, true)
            .unwrap();
        assert_eq!(observed, BTreeSet::from(["answer".to_string()]));
        let correction = RejectionClass::MissingSlot.format_correction(
            &requested,
            &json!({}),
            &[],
            &BTreeMap::new(),
            &observed,
        );
        assert!(correction.contains("missing: other"), "{correction}");
    }

    #[test]
    fn shape_evidence_is_bounded_without_serializing_schemas() {
        let keys = (0..8)
            .map(|index| format!("key-{index}-abcdefghijklmnopqrstuvwxyz-0123456789"))
            .collect::<Vec<_>>();
        let object = keys
            .iter()
            .map(|key| (key.clone(), json!(true)))
            .collect::<serde_json::Map<_, _>>();
        let description = super::json_shape(&Value::Object(object));
        let type_less_schema = json!({"oneOf": ["a very large schema would be here"]});
        assert!(
            description.contains("and 4 more")
                && description.len() < 250
                && super::schema_shape(&type_less_schema) == "the supplied schema",
            "shape summaries must remain bounded and must not serialize schemas: {description}"
        );
    }

    #[test]
    fn content_correction_bounds_rejected_output_evidence() {
        let requested = (0..8)
            .map(|index| RequestedSlotKey {
                ref_text: format!("slot:very-long-output-name-{index}-abcdefghijklmnopqrstuvwxyz"),
                property: format!("very-long-output-name-{index}-abcdefghijklmnopqrstuvwxyz"),
                operation: WriteOperation::Replace,
                schema_ref: Some("schema:boolean".to_string()),
            })
            .collect::<Vec<_>>();
        let validations = requested
            .iter()
            .map(|requested| SchemaValidation {
                ref_text: requested.ref_text.clone(),
                schema_ref: None,
                status: SchemaStatus::Rejected,
                reason: "internal validator wording".to_string(),
            })
            .collect::<Vec<_>>();
        let correction = RejectionClass::ContentRejection.format_correction(
            &requested,
            &json!({"properties": {}}),
            &validations,
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert!(
            correction.contains("and 4 more rejected outputs")
                && correction.len() < 1_500
                && !correction.contains("internal validator wording"),
            "complete correction must be bounded: {correction}"
        );
    }
}
