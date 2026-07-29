//! Harness stream parsing and optional live-output narration.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::tui;

use ctx_traits_io::debug_trace::{HarnessAttemptExit, HarnessAttemptStart, HarnessAttemptWriter};

/// Newest-tail cap on the narration window, in Unicode characters (not
/// bytes) so multibyte input is never split mid-codepoint.
const MAX_NARRATION_WINDOW_CHARS: usize = 150;
const NARRATION_CAPTURE_LIMIT: usize = 256 * 1024;
const NARRATOR_WARM_MAX_TURNS: u64 = 32;
const NARRATOR_WARM_MAX_PARSE_FAILURES: u8 = 3;
/// Pacing floor between narrations. Every narration is a fresh model call
/// carrying the harness's full per-spawn context, so back-to-back calls (the
/// natural cadence — one finishes, the next starts) dominate a drive's token
/// spend for lines nobody can read that fast. Chunks keep coalescing while
/// the floor holds.
const NARRATION_MIN_INTERVAL_MS: u64 = 15_000;
type NarrationSink = Arc<dyn Fn(String) + Send + Sync + 'static>;
/// Notified with each finished narrator call's own observed output-token
/// delta (P445), so a live `--progress tui` panel can show a narrator total
/// distinct from the work agent's — never the drive-wide narrator total
/// itself (that lives on [`NarratorTokenTracker`]).
type NarratorTokenSink = Arc<dyn Fn(u64) + Send + Sync + 'static>;
/// Notified with a successful P455 step-summary call's own context and
/// resulting text; a failed/timed-out/disabled attempt never reaches this
/// sink (silent presentation no-op).
type StepSummarySink = Arc<dyn Fn(StepSummaryContext, String) + Send + Sync + 'static>;
/// Notified with the narrator's running "settled + current" estimate of
/// in-progress thinking tokens (from the stream's own `estimated_tokens`
/// field), so a live panel can show an honest between-narrations pill
/// instead of a frozen placeholder. Display state only — never gates, never
/// costs an extra model call.
type ThinkingTokensSink = Arc<dyn Fn(u64) + Send + Sync + 'static>;

/// The four presentation sinks a [`StreamNarrator`] publishes through,
/// folded into one struct so `StreamNarrator::new` takes one argument
/// instead of a growing list of positional closures.
pub(crate) struct NarratorSinks {
    pub(crate) summary: NarrationSink,
    pub(crate) tokens: NarratorTokenSink,
    pub(crate) step_summary: StepSummarySink,
    pub(crate) thinking_tokens: ThinkingTokensSink,
}

/// Presentation-only context for one P455 finish-and-summarize request,
/// built entirely from panel-held display state (label/role/elapsed/observed
/// work tokens) — never from slot values or the ledger.
#[derive(Clone)]
pub(crate) struct StepSummaryContext {
    /// The completed step's own `structural_step_key`, so a P470 story row
    /// still joins the right step even though this call returns
    /// asynchronously — typically after the next step has already started.
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) role: String,
    pub(crate) elapsed: Duration,
    pub(crate) work_tokens: Option<u64>,
}

/// Drive-wide accumulator for output tokens observed across narrator calls
/// (P445), tracked separately from the work agent's [`WorkTokenTotal`]
/// (`drive.rs`) since narration is async garnish that never gates or joins
/// the drive. `in_flight` is a live count of narrator calls currently
/// dispatched — a ledger stamped while it is non-zero must record
/// `narration_complete = false`, since that call's usage may still be
/// missing from `total` above. All three fields live behind one `Mutex`
/// rather than independent atomics, so `snapshot` always reads a single
/// coherent point in time: a `begin_call`/`end_call` pair can never be
/// observed half-applied (e.g. `in_flight` already decremented but `total`
/// not yet carrying that call's delta).
#[derive(Clone, Default)]
pub(crate) struct NarratorTokenTracker {
    state: Arc<Mutex<NarratorTokenState>>,
}

#[derive(Default)]
struct NarratorTokenState {
    total: u64,
    in_flight: u64,
    ever_called: bool,
    /// Set only when some finished call's delta was actually nonzero —
    /// distinct from `ever_called`, so a narrator that ran but reported no
    /// usage (a silent harness) serializes an absent total rather than a
    /// fabricated zero.
    observed: bool,
}

pub(crate) struct NarratorTokenSnapshot {
    /// `None` when no narrator call was ever made this drive.
    pub(crate) tokens: Option<u64>,
    /// `None` when no narrator call was ever made this drive (there is
    /// nothing to be complete or incomplete about). `Some(false)` when a
    /// call was still in flight at the moment this snapshot was taken.
    pub(crate) complete: Option<bool>,
}

impl NarratorTokenTracker {
    fn begin_call(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.in_flight += 1;
        state.ever_called = true;
    }

    /// `delta` is this call's FULL observed total — every `push` delta plus
    /// the final `finish` delta, already summed by the caller's
    /// [`ctx_traits_io::harness::AttemptTokenAccumulator`] — never just the
    /// last chunk's increment, so a call is never undercounted.
    fn end_call(&self, delta: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.total += delta;
        state.in_flight = state.in_flight.saturating_sub(1);
        if delta > 0 {
            state.observed = true;
        }
    }

    /// Best-effort grace window for a just-enqueued terminal step-summary
    /// call (P455) before the drive's own final [`Self::snapshot`]: such a
    /// call is detached and non-blocking by design (a slow/hung narrator must
    /// never stall the drive), but with no wait at all, even a call that
    /// completes in single-digit milliseconds can lose its race against the
    /// drive returning right after the run's last accepted frame — silently
    /// dropping the last step's tokens from every otherwise-healthy run.
    /// Returns immediately once nothing is outstanding; a call still running
    /// past `timeout` is left in flight and reported incomplete exactly as
    /// before, never blocking beyond this bound.
    pub(crate) fn settle(&self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let in_flight = self.state.lock().map(|state| state.in_flight).unwrap_or(0);
            if in_flight == 0 || std::time::Instant::now() >= deadline {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub(crate) fn snapshot(&self) -> NarratorTokenSnapshot {
        let Ok(state) = self.state.lock() else {
            return NarratorTokenSnapshot {
                tokens: None,
                complete: None,
            };
        };
        if !state.ever_called {
            return NarratorTokenSnapshot {
                tokens: None,
                complete: None,
            };
        }
        NarratorTokenSnapshot {
            tokens: state.observed.then_some(state.total),
            complete: Some(state.in_flight == 0),
        }
    }
}

#[derive(Clone)]
pub(crate) struct NarratorConfig {
    pub(crate) argv: Vec<String>,
    /// Resolved `[worktree].env` overlay applied to cold narrator spawns
    /// (before `env_remove`). Empty for non-worktree narration.
    pub(crate) env_overlay: BTreeMap<String, String>,
    pub(crate) env_remove: Vec<String>,
    pub(crate) warm: Option<NarratorWarmConfig>,
    pub(crate) prompt_delivery: ctx_traits_io::harness::PromptDelivery,
    pub(crate) output_id: Option<String>,
    pub(crate) task_label: String,
    pub(crate) timeout_ms: u64,
    /// Worktree-scoped execution directory for cold and persistent narrator
    /// subprocesses. `None` leaves the narrator on its inherited cwd.
    pub(crate) exec_dir: Option<camino::Utf8PathBuf>,
    /// P478: the payload actually applied to this narrator's harness kind
    /// (argv-delivered for claude-code, env-delivered for opencode), carried
    /// only for the debug trace.
    pub(crate) confinement: Option<serde_json::Value>,
    /// P480: this worktree's generated OS-level spawn sandbox, `None` for a
    /// non-worktree narration, `sandbox = false`, or an unsupported
    /// platform. Applied at the narrator's cold spawn seam directly; the
    /// warm seam gets its own copy via [`NarratorWarmConfig::new`].
    pub(crate) sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
    pub(crate) trace: Option<NarratorTraceContext>,
}

#[derive(Clone)]
pub(crate) struct NarratorTraceContext {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) item_id: Option<String>,
    pub(crate) frame_title: String,
    pub(crate) harness_id: String,
    pub(crate) sequence: Arc<AtomicU64>,
}

#[derive(Clone, Default)]
pub(crate) struct NarratorWarmPool {
    inner: Arc<Mutex<BTreeMap<String, NarratorWarmState>>>,
}

#[derive(Clone)]
pub(crate) struct NarratorWarmConfig {
    key: String,
    argv: Vec<String>,
    env_overlay: BTreeMap<String, String>,
    env_remove: Vec<String>,
    exec_dir: Option<camino::Utf8PathBuf>,
    pool: NarratorWarmPool,
    /// P480: this worktree's generated OS-level spawn sandbox, applied at
    /// the warm narrator's own spawn seam.
    sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
}

impl NarratorWarmConfig {
    pub(crate) fn new(
        argv: Vec<String>,
        env_overlay: BTreeMap<String, String>,
        env_remove: Vec<String>,
        exec_dir: Option<camino::Utf8PathBuf>,
        pool: NarratorWarmPool,
        sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
    ) -> Self {
        // The overlay is folded into the reuse identity key (sorted `k=v`
        // pairs, `BTreeMap` iteration is already sorted) so a persistent
        // narrator process spawned under one overlay is never reused for a
        // different one — a stale-env warm session must be impossible.
        let key = format!(
            "{}\0{}\0{}\0{}",
            argv.join("\0"),
            env_remove.join("\0"),
            exec_dir
                .as_deref()
                .map(camino::Utf8Path::as_str)
                .unwrap_or(""),
            ctx_traits_io::command::overlay_identity(&env_overlay)
        );
        Self {
            key,
            argv,
            env_overlay,
            env_remove,
            exec_dir,
            pool,
            sandbox,
        }
    }
}

#[derive(Default)]
struct NarratorWarmState {
    session: Option<ctx_traits_io::harness::HarnessSession>,
    disabled: bool,
    turns: u64,
    consecutive_parse_failures: u8,
}

pub(crate) struct StreamNarrator {
    sender: mpsc::Sender<NarratorMessage>,
    latest_request: Arc<AtomicU64>,
    tokens: NarratorTokenTracker,
}

/// A cheap, cloneable handle that forwards raw stream chunks to the narrator
/// worker. Held by the live panel's observer so a narrated panel keeps
/// feeding the narrator even though it no longer forwards raw chunks to
/// itself.
#[derive(Clone)]
pub(crate) struct NarratorFeeder {
    sender: mpsc::Sender<NarratorMessage>,
}

impl NarratorFeeder {
    pub(crate) fn feed(&self, chunk: &[u8]) {
        let _ = self.sender.send(NarratorMessage::Chunk(chunk.to_vec()));
    }
}

enum NarratorMessage {
    Chunk(Vec<u8>),
    Finish,
    /// P455: the one additional terminal request for an accepted step
    /// transition, carrying only presentation context — never a slot value.
    FinishWithSummary(StepSummaryContext),
}

impl StreamNarrator {
    pub(crate) fn new(
        config: NarratorConfig,
        sinks: NarratorSinks,
        tokens: NarratorTokenTracker,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let latest_request = Arc::new(AtomicU64::new(0));
        let worker_tokens = tokens.clone();
        {
            let latest_request = Arc::clone(&latest_request);
            thread::spawn(move || {
                run_narrator_worker(receiver, config, sinks, latest_request, worker_tokens)
            });
        }
        Self {
            sender,
            latest_request,
            tokens,
        }
    }

    pub(crate) fn feeder(&self) -> NarratorFeeder {
        NarratorFeeder {
            sender: self.sender.clone(),
        }
    }

    /// Stop narrating this frame. Invalidate every in-flight narration (so a late
    /// result can never repaint a later frame's panel) and signal the worker. We
    /// do NOT join: the worker may be blocked inside a serial model call, and
    /// finish must stay non-blocking; the detached worker exits on its own and its
    /// late result is dropped by the request check.
    pub(crate) fn finish(self) {
        self.latest_request.fetch_add(1, Ordering::Relaxed);
        let _ = self.sender.send(NarratorMessage::Finish);
    }

    /// P455: enqueue the one terminal step-summary call for an accepted
    /// active-step transition. Reserves this call's token usage immediately —
    /// before the worker's own pacing wait — so a ledger snapshot taken while
    /// the summary is queued/running truthfully records incomplete narration
    /// rather than a premature total; cancels the reservation if the worker
    /// has already exited and can never complete it. Like [`Self::finish`],
    /// non-blocking: the detached worker makes the call and exits on its own.
    pub(crate) fn finish_with_step_summary(self, context: StepSummaryContext) {
        self.latest_request.fetch_add(1, Ordering::Relaxed);
        self.tokens.begin_call();
        if self
            .sender
            .send(NarratorMessage::FinishWithSummary(context))
            .is_err()
        {
            self.tokens.end_call(0);
        }
    }
}

/// Serial narrator: at most one model call is ever in flight. Chunks are
/// coalesced into the window while a narration runs, so we always narrate the
/// freshest activity and never cold-start overlapping processes. The panel
/// simply holds its prior line on the capture thread while this blocks.
fn run_narrator_worker(
    receiver: mpsc::Receiver<NarratorMessage>,
    config: NarratorConfig,
    sinks: NarratorSinks,
    latest_request: Arc<AtomicU64>,
    tokens: NarratorTokenTracker,
) {
    let NarratorSinks {
        summary: push_summary,
        tokens: on_tokens,
        step_summary: push_step_summary,
        thinking_tokens: on_thinking_tokens,
    } = sinks;
    let mut line_buffer = String::new();
    let mut window = String::new();
    let mut fallback_window = String::new();
    let mut has_new_thinking = false;
    let mut has_new_fallback = false;
    let mut token_progress = TokenProgress::default();
    let mut last_narration_start: Option<std::time::Instant> = None;
    macro_rules! ingest {
        ($chunk:expr) => {{
            let outcome =
                ingest_chunk(&mut line_buffer, &mut window, &mut fallback_window, &$chunk);
            has_new_thinking |= outcome.has_thinking;
            has_new_fallback |= outcome.has_fallback;
            if let Some(total) = token_progress.observe(&outcome.estimated_tokens) {
                on_thinking_tokens(total);
            }
        }};
    }
    loop {
        match receiver.recv() {
            Ok(NarratorMessage::Chunk(chunk)) => ingest!(chunk),
            Ok(NarratorMessage::Finish) | Err(_) => return,
            Ok(NarratorMessage::FinishWithSummary(context)) => {
                finish_with_summary_call(
                    context,
                    &config,
                    NarrationWindows {
                        thinking: &window,
                        fallback: &fallback_window,
                    },
                    last_narration_start,
                    &tokens,
                    &on_tokens,
                    &push_step_summary,
                );
                return;
            }
        }
        // Drain everything already queued so the next narration reflects the
        // latest window instead of falling behind a fast stream.
        loop {
            match receiver.try_recv() {
                Ok(NarratorMessage::Chunk(chunk)) => ingest!(chunk),
                Ok(NarratorMessage::Finish) | Err(mpsc::TryRecvError::Disconnected) => return,
                Ok(NarratorMessage::FinishWithSummary(context)) => {
                    finish_with_summary_call(
                        context,
                        &config,
                        NarrationWindows {
                            thinking: &window,
                            fallback: &fallback_window,
                        },
                        last_narration_start,
                        &tokens,
                        &on_tokens,
                        &push_step_summary,
                    );
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        // Hold the pacing floor, still coalescing whatever streams in while we
        // wait, so the eventual narration covers the freshest window.
        if let Some(started) = last_narration_start {
            let floor = std::time::Duration::from_millis(NARRATION_MIN_INTERVAL_MS);
            while let Some(remaining) = floor.checked_sub(started.elapsed()) {
                match receiver.recv_timeout(remaining) {
                    Ok(NarratorMessage::Chunk(chunk)) => ingest!(chunk),
                    Ok(NarratorMessage::Finish) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return;
                    }
                    Ok(NarratorMessage::FinishWithSummary(context)) => {
                        finish_with_summary_call(
                            context,
                            &config,
                            NarrationWindows {
                                thinking: &window,
                                fallback: &fallback_window,
                            },
                            last_narration_start,
                            &tokens,
                            &on_tokens,
                            &push_step_summary,
                        );
                        return;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                }
            }
        }
        // Thinking/reasoning text keeps today's diet unchanged: narrate on
        // `has_new_thinking`. Only when the stream yields no thinking text at
        // all (redacted-thinking Claude 5 seats) does a fresh assistant-text
        // or tool-activity chunk (`has_new_fallback`) justify narrating —
        // this clause is dead by construction on any stream whose thinking
        // window is non-empty.
        let (narration_window, is_fallback) = NarrationWindows {
            thinking: &window,
            fallback: &fallback_window,
        }
        .active();
        let should_narrate = if is_fallback {
            has_new_fallback
        } else {
            has_new_thinking
        };
        if !should_narrate || narration_window.trim().is_empty() {
            continue;
        }
        has_new_thinking = false;
        has_new_fallback = false;
        last_narration_start = Some(std::time::Instant::now());
        // finish() may bump latest_request while we are inside narrate_once; if so
        // we drop the result rather than repaint a settled or later frame.
        let request_id = latest_request.fetch_add(1, Ordering::Relaxed) + 1;
        let outcome = narrate_once(&config, &narration_window, is_fallback, &tokens, &on_tokens);
        if latest_request.load(Ordering::Relaxed) == request_id {
            // A genuine summary repaints the live line; a warm-fallback notice,
            // timeout, or failure is a presentation no-op — the line simply
            // holds its prior value. The debug trace (begin/finish/fail_narrator_trace)
            // is where these attempts are diagnosed, not the panel. Nothing here
            // disables narration: the next paced attempt tries again on its own,
            // so a narrator that recovers keeps narrating without intervention.
            if let Ok(summary) = outcome {
                push_summary(summary);
            }
        }
    }
}

/// The narrator's two bounded windows, borrowed together so
/// [`finish_with_summary_call`] and the worker's own trigger check share one
/// argument instead of two positional `&str`s.
#[derive(Clone, Copy)]
struct NarrationWindows<'a> {
    thinking: &'a str,
    fallback: &'a str,
}

impl NarrationWindows<'_> {
    /// Picks which bounded window a narration is built from: the thinking
    /// window when it holds anything, otherwise the fallback window. On a
    /// stream that exposes thinking/reasoning text this always resolves to
    /// the thinking window — the fallback window is never consulted.
    fn active(&self) -> (String, bool) {
        if self.thinking.trim().is_empty() {
            (self.fallback.to_string(), true)
        } else {
            (self.thinking.to_string(), false)
        }
    }
}

/// Accumulates the stream's own `estimated_tokens` field into an honest
/// running total even though the field itself resets to a small value at
/// the start of each new thinking content block: a raw `max` or `sum`
/// across blocks would misreport, so completed blocks are folded into
/// `settled` the moment a lower value proves the block rolled over.
#[derive(Default)]
struct TokenProgress {
    settled: u64,
    current: u64,
}

impl TokenProgress {
    /// Folds in every value observed this chunk, in order, and returns the
    /// new settled+current total — `None` when nothing was observed, so the
    /// caller only republishes the pill on an actual update.
    fn observe(&mut self, values: &[u64]) -> Option<u64> {
        if values.is_empty() {
            return None;
        }
        for &value in values {
            if value >= self.current {
                self.current = value;
            } else {
                self.settled += self.current;
                self.current = value;
            }
        }
        Some(self.settled + self.current)
    }
}

/// Terminal P455 step-summary dispatch: honors the existing pacing floor by
/// sleeping out any remainder (no further chunks are expected once a step has
/// been accepted, so there is nothing left to coalesce) before making the one
/// required call — regardless of whether the bounded thinking-window tail is
/// empty. `tokens` is already reserved by the caller
/// ([`StreamNarrator::finish_with_step_summary`]) at enqueue time; this only
/// completes that reservation with the call's observed delta.
fn finish_with_summary_call(
    context: StepSummaryContext,
    config: &NarratorConfig,
    windows: NarrationWindows<'_>,
    last_narration_start: Option<std::time::Instant>,
    tokens: &NarratorTokenTracker,
    on_tokens: &NarratorTokenSink,
    push_step_summary: &StepSummarySink,
) {
    if let Some(started) = last_narration_start {
        let floor = std::time::Duration::from_millis(NARRATION_MIN_INTERVAL_MS);
        if let Some(remaining) = floor.checked_sub(started.elapsed()) {
            thread::sleep(remaining);
        }
    }
    let (narration_window, is_fallback) = windows.active();
    let prompt = step_summary_prompt(&context, &narration_window, is_fallback);
    let (result, call_total) = dispatch_narration(config, prompt);
    tokens.end_call(call_total);
    if call_total > 0 {
        on_tokens(call_total);
    }
    if let Ok(summary) = result {
        push_step_summary(context, summary);
    }
}

/// Result of feeding one chunk into [`ingest_chunk`]: which of the two
/// windows actually gained new content this chunk, plus every
/// `thinking_delta`-bound `estimated_tokens` value observed along the way
/// (in stream order) — see [`collect_thinking_token_estimates`] for why this
/// is deliberately not every `estimated_tokens` key in the chunk.
#[derive(Default)]
struct IngestOutcome {
    has_thinking: bool,
    has_fallback: bool,
    estimated_tokens: Vec<u64>,
}

/// Feed a raw stream chunk into the narrator's two rolling windows. `window`
/// admits only structured `thinking`/`thinking_delta`/`reasoning`/
/// `reasoning_delta` content, exactly as before. `fallback_window` is fed
/// only when a stream yields no thinking text at all: assistant
/// `text`/`text_delta` prose (never from a claude `user`/`tool_result`
/// event, so a redacted-thinking stream's tool output is never slurped) plus
/// tool-call activity (name + one salient argument, never a tool result
/// body) via [`tool_activity_texts`]. Both windows are always fed from the
/// same parsed events; the caller decides which one is actually consulted.
fn ingest_chunk(
    line_buffer: &mut String,
    window: &mut String,
    fallback_window: &mut String,
    chunk: &[u8],
) -> IngestOutcome {
    line_buffer.push_str(&String::from_utf8_lossy(chunk));
    let mut outcome = IngestOutcome::default();
    let mut thinking = Vec::new();
    let mut fallback = Vec::new();
    for line in drain_ndjson_lines(line_buffer) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        collect_thinking_texts(&value, &mut thinking);
        // A claude tool-result event is carried under a top-level `user`
        // type; its `content` can itself hold `{"type":"text",...}` blocks,
        // which is exactly the shape the fallback prose collector matches.
        // Skipping the whole event here — rather than excluding by key — is
        // what keeps a tool result body out of the fallback window.
        if value.get("type").and_then(Value::as_str) != Some("user") {
            collect_kind_texts(&value, FALLBACK_PROSE_KINDS, &mut fallback);
        }
        // A claude `stream_event` carries a `content_block_start` the moment a
        // tool_use block opens, before its `input` has streamed in — reading
        // it here would yield a bare tool name. The same call's arguments
        // always also arrive complete inside a later `assistant` message, so
        // skipping the streaming copy here is lossless, not a narrowing.
        if value.get("type").and_then(Value::as_str) != Some("stream_event") {
            tool_activity_texts(&value, &mut fallback);
        }
        collect_thinking_token_estimates(&value, &mut outcome.estimated_tokens);
    }
    if !thinking.is_empty() {
        append_window(window, &thinking.join("\n"));
        outcome.has_thinking = true;
    }
    if !fallback.is_empty() {
        append_window(fallback_window, &fallback.join("\n"));
        outcome.has_fallback = true;
    }
    outcome
}

/// Run one live-narration call. Returns `Err(reason)` on a failed attempt — a
/// presentation no-op for the caller, but the reason still threads through
/// for the debug trace recorded inside `narrate_cold`/`narrate_warm`. Tracks
/// this logical call's own observed output tokens (P445), regardless of
/// outcome, against `tokens` and publishes the total to `on_tokens` — the
/// drive-wide narrator total and TUI display are updated for a
/// failed/timed-out call too, since a harness can emit usage before
/// erroring. Reserves and completes its own token accounting around
/// [`dispatch_narration`]'s shared warm/cold execution; the P455
/// step-summary call instead reserves at enqueue time (see
/// [`finish_with_summary_call`]) and completes through that same shared
/// dispatch.
fn narrate_once(
    config: &NarratorConfig,
    window: &str,
    is_fallback: bool,
    tokens: &NarratorTokenTracker,
    on_tokens: &NarratorTokenSink,
) -> Result<String, String> {
    let prompt = narrator_prompt(&config.task_label, window, is_fallback);
    tokens.begin_call();
    let (result, call_total) = dispatch_narration(config, prompt);
    tokens.end_call(call_total);
    if call_total > 0 {
        on_tokens(call_total);
    }
    result
}

/// Shared warm/cold execution for both the live present-continuous prompt and
/// the P455 completed-step prompt: attempts the warm session first (if
/// configured), falling back to a cold spawn, and returns the outcome
/// alongside this logical call's summed observed output-token delta. A warm
/// turn that falls back to cold is TWO actual harness attempts against two
/// independent providers/processes; each gets its own fresh
/// [`ctx_traits_io::harness::AttemptTokenAccumulator`] (an
/// `OutputTokenCounter` treats same-attempt totals as a per-channel maximum,
/// so sharing one counter across attempts would collide their independent
/// message/task ids instead of summing distinct attempts) and the two attempt
/// totals are summed here into the logical call's own total. Callers own
/// token reservation themselves ([`NarratorTokenTracker::begin_call`]/
/// `end_call`) — this function neither begins nor ends one.
/// P521 assisted level dispatches this directly (no live drive, no serial
/// worker thread) — same warm-then-cold-fallback logic a live drive's
/// `run_narrator_worker` uses, `pub(crate)` for exactly that one extra
/// caller.
pub(crate) fn dispatch_narration(
    config: &NarratorConfig,
    prompt: String,
) -> (Result<String, String>, u64) {
    let mut call_total: u64 = 0;
    let mut fallback_reason = None;
    let result = (|| {
        if let Some(warm) = config.warm.as_ref() {
            let warm_total = Arc::new(AtomicU64::new(0));
            let warm_accumulator = {
                let warm_total = warm_total.clone();
                ctx_traits_io::harness::AttemptTokenAccumulator::new(move |delta| {
                    warm_total.fetch_add(delta, Ordering::Relaxed);
                })
            };
            let warm_result = narrate_warm(config, warm, &prompt, &warm_accumulator);
            warm_accumulator.finish();
            call_total += warm_total.load(Ordering::Relaxed);
            match warm_result {
                Ok(Some(summary)) => return Ok(summary),
                Ok(None) => {}
                Err(reason) => {
                    fallback_reason =
                        Some(format!("warm turn failed: {reason}; using cold fallback"));
                }
            }
        }
        let cold_total = Arc::new(AtomicU64::new(0));
        let cold_accumulator = {
            let cold_total = cold_total.clone();
            ctx_traits_io::harness::AttemptTokenAccumulator::new(move |delta| {
                cold_total.fetch_add(delta, Ordering::Relaxed);
            })
        };
        let cold_result = narrate_cold(config, prompt, &cold_accumulator);
        cold_accumulator.finish();
        call_total += cold_total.load(Ordering::Relaxed);
        cold_result.map_err(|reason| match fallback_reason.take() {
            Some(warning) => format!("{warning}; cold fallback failed: {reason}"),
            None => reason,
        })
    })();
    (result, call_total)
}

fn narrate_cold(
    config: &NarratorConfig,
    prompt: String,
    accumulator: &ctx_traits_io::harness::AttemptTokenAccumulator,
) -> Result<String, String> {
    let mut trace_argv = config.argv.clone();
    if config.prompt_delivery == ctx_traits_io::harness::PromptDelivery::Arg {
        trace_argv.push(prompt.clone());
    }
    let trace = begin_narrator_trace(config, &prompt, &trace_argv, "cold");
    let stdout_observer = super::drive::combine_stdout_observers(
        trace.as_ref().map(|trace| trace.writer.stdout_observer()),
        Some(accumulator.observer()),
    );
    let output = ctx_traits_io::harness::run(ctx_traits_io::harness::HarnessRunRequest {
        argv: config.argv.clone(),
        env_overlay: config.env_overlay.clone(),
        env_remove: config.env_remove.clone(),
        prompt,
        prompt_delivery: config.prompt_delivery.clone(),
        timeout_ms: config.timeout_ms,
        idle_timeout_ms: None,
        capture_limit: NARRATION_CAPTURE_LIMIT,
        stream: false,
        stdout_observer,
        tick_observer: None,
        exec_dir: config.exec_dir.clone(),
        sandbox: config.sandbox.clone(),
    });
    let output = match output {
        Ok(output) => {
            finish_narrator_trace(trace, &output);
            output
        }
        Err(source) => {
            fail_narrator_trace(trace, &source);
            return Err(format!("spawn failed: {source}"));
        }
    };
    narrator_summary_from_output(config, &output)
}

fn narrate_warm(
    config: &NarratorConfig,
    warm: &NarratorWarmConfig,
    prompt: &str,
    accumulator: &ctx_traits_io::harness::AttemptTokenAccumulator,
) -> Result<Option<String>, String> {
    let mut states = warm
        .pool
        .inner
        .lock()
        .map_err(|_| "warm pool lock poisoned".to_string())?;
    let state = states.entry(warm.key.clone()).or_default();
    if state.disabled {
        return Ok(None);
    }
    let trace = begin_narrator_trace(config, prompt, &warm.argv, "warm");
    if state.turns >= NARRATOR_WARM_MAX_TURNS {
        state.session = None;
        state.turns = 0;
        state.consecutive_parse_failures = 0;
    }
    if state.session.is_none() {
        match ctx_traits_io::harness::HarnessSession::spawn(
            warm.argv.clone(),
            warm.env_overlay.clone(),
            warm.env_remove.clone(),
            warm.exec_dir.clone(),
            warm.sandbox.clone(),
        ) {
            Ok(session) => state.session = Some(session),
            Err(source) => {
                state.disabled = true;
                fail_narrator_trace(trace, &source);
                return Err(format!("spawn failed: {source}"));
            }
        }
    }
    let Some(session) = state.session.as_mut() else {
        state.disabled = true;
        return Err("session missing after warm spawn".to_string());
    };
    let stdout_observer = super::drive::combine_stdout_observers(
        trace.as_ref().map(|trace| trace.writer.stdout_observer()),
        Some(accumulator.observer()),
    );
    let output = match session.prompt(ctx_traits_io::harness::HarnessSessionPrompt {
        prompt: prompt.to_string(),
        timeout_ms: config.timeout_ms,
        idle_timeout_ms: None,
        capture_limit: NARRATION_CAPTURE_LIMIT,
        stream: false,
        stdout_observer,
        tick_observer: None,
    }) {
        Ok(output) => output,
        Err(source) => {
            state.session = None;
            state.disabled = true;
            fail_narrator_trace(trace, &source);
            return Err(format!("prompt failed: {source}"));
        }
    };
    finish_narrator_trace(trace, &output);
    if output.timed_out || output.idle_timed_out || output.exit_code != Some(0) {
        state.session = None;
        state.disabled = true;
        state.turns = state.turns.saturating_add(1);
        return narrator_summary_from_output(config, &output).map(Some);
    }
    state.turns = state.turns.saturating_add(1);
    match narrator_summary_from_output(config, &output) {
        Ok(summary) => {
            state.consecutive_parse_failures = 0;
            Ok(Some(summary))
        }
        Err(reason) => {
            state.consecutive_parse_failures = state.consecutive_parse_failures.saturating_add(1);
            if state.consecutive_parse_failures >= NARRATOR_WARM_MAX_PARSE_FAILURES {
                state.session = None;
                state.disabled = true;
                return Err(format!(
                    "{reason}; warm disabled after {NARRATOR_WARM_MAX_PARSE_FAILURES} parse failures"
                ));
            }
            Err(reason)
        }
    }
}

struct ActiveNarratorTrace {
    writer: HarnessAttemptWriter,
    started: std::time::Instant,
}

fn begin_narrator_trace(
    config: &NarratorConfig,
    prompt: &str,
    argv: &[String],
    invocation: &str,
) -> Option<ActiveNarratorTrace> {
    let trace = config.trace.as_ref()?;
    let sequence = trace.sequence.fetch_add(1, Ordering::Relaxed) + 1;
    HarnessAttemptWriter::start(&HarnessAttemptStart {
        run_id: &trace.run_id,
        session_id: &trace.session_id,
        sequence,
        item_id: trace.item_id.as_deref(),
        frame_title: &trace.frame_title,
        role: "narrator",
        harness_id: &trace.harness_id,
        attempt: sequence,
        invocation,
        argv,
        prompt,
        stdout_limit: NARRATION_CAPTURE_LIMIT,
        confinement: config.confinement.as_ref(),
        spawn_sandbox: config
            .sandbox
            .as_ref()
            .map(|sandbox| sandbox.profile.as_str()),
    })
    .ok()
    .map(|writer| ActiveNarratorTrace {
        writer,
        started: std::time::Instant::now(),
    })
}

fn finish_narrator_trace(
    trace: Option<ActiveNarratorTrace>,
    output: &ctx_traits_io::harness::HarnessRunOutcome,
) {
    let Some(trace) = trace else {
        return;
    };
    let _ = trace.writer.finish(&HarnessAttemptExit {
        exit_code: output.exit_code,
        timed_out: output.timed_out,
        idle_timed_out: output.idle_timed_out,
        stdout_truncated: output.stdout_truncated,
        duration_ms: output.duration_ms,
        stderr: &output.stderr,
    });
}

fn fail_narrator_trace(trace: Option<ActiveNarratorTrace>, error: &impl std::fmt::Display) {
    let Some(trace) = trace else {
        return;
    };
    let stderr = format!("narrator invocation failed before outcome: {error}");
    let _ = trace.writer.finish(&HarnessAttemptExit {
        exit_code: None,
        timed_out: false,
        idle_timed_out: false,
        stdout_truncated: false,
        duration_ms: trace.started.elapsed().as_millis(),
        stderr: &stderr,
    });
}

fn narrator_summary_from_output(
    config: &NarratorConfig,
    output: &ctx_traits_io::harness::HarnessRunOutcome,
) -> Result<String, String> {
    if output.timed_out {
        return Err(format!("timed out ({}ms)", config.timeout_ms));
    }
    if output.idle_timed_out {
        return Err(format!("idle timed out ({}ms)", config.timeout_ms));
    }
    if output.exit_code != Some(0) {
        // The harness's own error line is the gold diagnostic (e.g. an unknown
        // flag or model); surface a trimmed snippet of it.
        let detail: String = output
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .chars()
            .take(90)
            .collect();
        return Err(format!("exit {:?}: {detail}", output.exit_code));
    }
    if output.stdout_truncated {
        return Err("output truncated".to_string());
    }
    let text = narrator_stdout_text(&output.stdout, config.output_id.as_deref())
        .ok_or_else(|| "no text in narrator output".to_string())?;
    sanitize_summary(&text).ok_or_else(|| "empty narrator summary".to_string())
}

/// `is_fallback` selects the window label: `Recent agent output:` is today's
/// bytes (the thinking/reasoning window); `Recent agent activity:` is honest
/// over the fallback window, which holds assistant prose and tool activity
/// rather than thinking.
fn narrator_prompt(task_label: &str, window: &str, is_fallback: bool) -> String {
    let label = if is_fallback {
        "Recent agent activity:"
    } else {
        "Recent agent output:"
    };
    format!(
        "Summarize the agent's live activity as one short present-continuous status line.\n\
Rules:\n\
- Return only the status line.\n\
- Use a leading verb such as Reading, Searching, Editing, Planning, Checking, or Writing.\n\
- Keep it under 80 characters.\n\
- No quotes, bullets, markdown, or trailing punctuation.\n\
Task: {task_label}\n\
{label}\n{window}\n"
    )
}

/// P455 completed-step counterpart to [`narrator_prompt`]: same generic
/// verb-class instruction, past tense, built only from the panel-held
/// [`StepSummaryContext`] and the narrator's own bounded thinking-window
/// tail — never a slot value. An empty tail is expected for cached/concurrent
/// outcomes; the step metadata alone is still enough to require the one call.
/// `is_fallback` picks the window label exactly as in [`narrator_prompt`] —
/// `Recent agent thinking:` would be a lie over the fallback window.
fn step_summary_prompt(context: &StepSummaryContext, window: &str, is_fallback: bool) -> String {
    let tokens_line = context
        .work_tokens
        .map(|tokens| format!("Observed work: {}.\n", tui::token_text(tokens)))
        .unwrap_or_default();
    let label = if is_fallback {
        "Recent agent activity:"
    } else {
        "Recent agent thinking:"
    };
    format!(
        "Summarize the just-completed step as one short past-tense status line.\n\
Rules:\n\
- Return only the status line.\n\
- Use a leading past-tense verb such as Read, Searched, Edited, Planned, Checked, or Wrote.\n\
- Keep it under 80 characters.\n\
- No quotes, bullets, markdown, or trailing punctuation.\n\
Step: {} ({})\n\
Elapsed: {}\n\
{tokens_line}\
{label}\n{window}\n",
        context.label,
        context.role,
        tui::elapsed_text(context.elapsed),
    )
}

/// Shared by both the live present-continuous prompt and the P455 past-tense
/// completed-step prompt — tense-neutral so neither call's own instructions
/// are contradicted by the system channel.
pub(crate) fn narrator_system_prompt() -> &'static str {
    "You compact agent activity for terminal presentation only. Return exactly one short status line in the tense the request specifies, no markdown, no JSON, no quotes, and no trailing punctuation. Never submit ctx.traits outputs or call tools."
}

fn narrator_stdout_text(stdout: &str, output_id: Option<&str>) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let values = match output_id {
        Some(id) if GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => stream_values(trimmed),
        Some("raw-json" | "claude-json") => serde_json::from_str::<Value>(trimmed)
            .ok()
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    // Prefer a final `result` string — claude stream-json ends with one, and it is
    // the whole answer rather than a streamed partial-message fragment.
    for value in values.iter().rev() {
        if value.get("type").and_then(Value::as_str) == Some("result")
            && let Some(result) = value.get("result").and_then(Value::as_str)
            && !result.trim().is_empty()
        {
            return Some(result.to_string());
        }
    }
    // Otherwise reconstruct the answer by JOINING the streamed text pieces; a lone
    // `.last()` would only capture the final `--include-partial-messages` fragment.
    let joined = progress_texts(&values).join(" ");
    if !joined.trim().is_empty() {
        return Some(joined);
    }
    if let Ok(Value::String(text)) = serde_json::from_str::<Value>(trimmed) {
        return Some(text);
    }
    Some(trimmed.to_string())
}

fn sanitize_summary(text: &str) -> Option<String> {
    let mut line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .trim_matches(['"', '\'', '`', '*', '-'])
        .trim()
        .to_string();
    while line.ends_with(['.', '!', ':', ';']) {
        line.pop();
        line = line.trim_end().to_string();
    }
    if line.is_empty() {
        None
    } else {
        Some(line.chars().take(120).collect())
    }
}

/// Keep at most the newest [`MAX_NARRATION_WINDOW_CHARS`] Unicode characters,
/// trimming on character boundaries so multibyte UTF-8 is never split.
fn append_window(window: &mut String, text: &str) {
    if !window.is_empty() {
        window.push('\n');
    }
    window.push_str(text);
    let char_count = window.chars().count();
    if char_count <= MAX_NARRATION_WINDOW_CHARS {
        return;
    }
    let drop = char_count - MAX_NARRATION_WINDOW_CHARS;
    let start = window
        .char_indices()
        .nth(drop)
        .map(|(index, _)| index)
        .unwrap_or(window.len());
    window.drain(..start);
}

/// Frame the complete NDJSON lines currently in `buffer` (leaving any
/// trailing partial line for the next chunk) into non-empty trimmed line
/// strings, ready for content-specific extraction.
fn drain_ndjson_lines(buffer: &mut String) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(index) = buffer.find('\n') {
        let line = buffer[..index].trim_end_matches('\r').to_string();
        buffer.drain(..=index);
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    lines
}

/// Drain the complete NDJSON lines currently in `buffer` (leaving any
/// trailing partial line for the next chunk) and return every assistant /
/// thinking / reasoning text delta found, in order. Drives the passthrough
/// panel that shows the model's own words when no narrator is configured —
/// P470: `--progress tui`'s live run pane needs the FULL list (its
/// CURRENT-step stream is verbatim, not just-the-latest), while
/// `--progress stream`'s [`tui::LiveLine`] keeps using only the last one.
pub(crate) fn drain_text_deltas(buffer: &mut String) -> Vec<String> {
    let mut deltas = Vec::new();
    for line in drain_ndjson_lines(buffer) {
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            deltas.extend(progress_texts(&[value]));
        } else {
            deltas.push(line);
        }
    }
    deltas
}

/// P427: harness `output` ids whose stdout is a stream of JSON values (one
/// object per line, or brace-balanced) decodable by [`stream_values`] with no
/// harness-specific shape knowledge — Claude Code's `stream-json`, OpenCode's
/// `--format json`, and Pi's `--mode json` event stream all satisfy this.
/// Centralized here so drive, merge, and this module's own narrator path
/// dispatch on one list instead of repeating the id set at each call site.
pub(crate) const GENERIC_STREAM_JSON_OUTPUTS: &[&str] =
    &["claude-stream-json", "opencode-json", "pi-json"];

/// Session id from a harness stream even when slot parsing failed — the init
/// event usually survives truncation and malformed payloads, and it is what
/// lets a correction retry resume the same conversation (a cheap reshape of an
/// answer the model already has) instead of restarting the step from scratch.
/// Moved from `drive.rs` (P463): this module already owns stream parsing, and
/// `ctx traits merge`'s merger dispatch (P463 item B) needs the same
/// extraction to link a merger call's harness-side session store.
pub(crate) fn stream_harness_session_id(output_id: &str, stdout: &str) -> Option<String> {
    let values = match output_id {
        id if GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => stream_values(stdout),
        "raw-json" | "claude-json" => serde_json::from_str::<Value>(stdout).into_iter().collect(),
        _ => Vec::new(),
    };
    values
        .iter()
        .find_map(|value| session_id_from_event(output_id, value))
}

/// Session extraction is output-format aware: Pi's `session` event carries its
/// id in a bare `id` field, which every other harness uses for message/event
/// identity rather than a session — so a generic root-`id` guess would treat
/// arbitrary Pi events as session headers. Only `pi-json`'s documented
/// `{"type":"session","id":...}` shape is recognized as a session for Pi.
pub(crate) fn session_id_from_event(output_id: &str, value: &Value) -> Option<String> {
    if output_id == "pi-json" {
        return pi_session_id_from(value);
    }
    session_id_from(value)
}

fn pi_session_id_from(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    object.get("id").and_then(Value::as_str).map(str::to_string)
}

fn session_id_from(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["session_id", "sessionId", "sessionID", "session-id"] {
        if let Some(session_id) = object.get(key).and_then(Value::as_str) {
            return Some(session_id.to_string());
        }
    }
    for value in object.values() {
        if let Value::Object(_) | Value::Array(_) = value
            && let Some(session_id) = session_id_from_nested(value)
        {
            return Some(session_id);
        }
    }
    None
}

fn session_id_from_nested(value: &Value) -> Option<String> {
    match value {
        Value::Object(_) => session_id_from(value),
        Value::Array(items) => items.iter().find_map(session_id_from_nested),
        _ => None,
    }
}

pub(crate) fn stream_values(stdout: &str) -> Vec<Value> {
    let mut values: Vec<Value> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    if values.is_empty() {
        values = balanced_json_objects(stdout)
            .into_iter()
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .collect();
    }
    values
}

pub(crate) fn progress_events(values: &[Value]) -> Vec<&'static str> {
    let mut events = Vec::new();
    for value in values {
        let kind = value
            .get("type")
            .or_else(|| value.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind.contains("tool") {
            events.push("tool-use");
        } else if kind.contains("message") || kind.contains("assistant") {
            events.push("message-completed");
        } else if kind.contains("result") || kind.contains("complete") {
            events.push("result-completed");
        }
    }
    events.dedup();
    events
}

pub(crate) fn progress_texts(values: &[Value]) -> Vec<String> {
    let mut texts = Vec::new();
    for value in values {
        collect_progress_texts(value, &mut texts);
    }
    texts
}

const PROGRESS_TEXT_KINDS: &[&str] = &[
    "text",
    "text_delta",
    "thinking",
    "thinking_delta",
    "reasoning",
    "reasoning_delta",
];

const THINKING_TEXT_KINDS: &[&str] =
    &["thinking", "thinking_delta", "reasoning", "reasoning_delta"];

/// Fallback-window prose kinds: assistant text only, never thinking —
/// admitting `thinking`/`reasoning` here would defeat the point of a
/// fallback that exists precisely because those are redacted.
const FALLBACK_PROSE_KINDS: &[&str] = &["text", "text_delta"];

fn collect_progress_texts(value: &Value, texts: &mut Vec<String>) {
    collect_kind_texts(value, PROGRESS_TEXT_KINDS, texts);
}

/// Narrator-only counterpart to [`collect_progress_texts`]: walks the same
/// event shapes but admits only structured thinking/reasoning kinds, so
/// plain assistant `text`/`text_delta` never reaches the narration window.
fn collect_thinking_texts(value: &Value, texts: &mut Vec<String>) {
    collect_kind_texts(value, THINKING_TEXT_KINDS, texts);
}

fn collect_kind_texts(value: &Value, allowed_kinds: &[&str], texts: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            let text_kind = object
                .get("type")
                .or_else(|| object.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if allowed_kinds.contains(&text_kind) {
                // "thinking" is Claude's own thinking-payload key — shared by
                // `PROGRESS_TEXT_KINDS` and `THINKING_TEXT_KINDS` on purpose:
                // both want thinking text once it is no longer redacted. No
                // collision with `FALLBACK_PROSE_KINDS`: a `text`/`text_delta`
                // object never carries a `thinking` key.
                for key in ["text", "content", "delta", "thinking"] {
                    if let Some(text) = object.get(key).and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        texts.push(text.to_string());
                    }
                }
            }
            for value in object.values() {
                collect_kind_texts(value, allowed_kinds, texts);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_kind_texts(item, allowed_kinds, texts);
            }
        }
        _ => {}
    }
}

/// Tool-call activity for the fallback window: tool name plus one salient
/// argument, read off *complete* `tool_use` blocks — never `input_json_delta`
/// fragments (no cross-chunk reassembly needed) and never a tool result body
/// (`state.output`/`tool_result`/`tool_use_result` are never read). Both
/// provider shapes share the same top-level `"type":"tool_use"` marker:
/// claude's block carries `name`/`input` directly; opencode's carries them
/// nested under `part.tool`/`part.state.input`. Callers must not feed this a
/// claude `stream_event` — its `content_block_start` opens a `tool_use`
/// block with an empty placeholder `input` before arguments have streamed
/// in, and the same call's arguments always also arrive complete inside a
/// later `assistant` message, so the streaming copy is redundant to skip.
fn tool_activity_texts(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(text) =
                    claude_tool_use_text(object).or_else(|| opencode_tool_use_text(object))
            {
                texts.push(text);
            }
            for value in object.values() {
                tool_activity_texts(value, texts);
            }
        }
        Value::Array(items) => {
            for item in items {
                tool_activity_texts(item, texts);
            }
        }
        _ => {}
    }
}

/// Salient tool-input argument keys, checked in order — the first one
/// present wins. Covers both claude (`file_path`) and opencode
/// (`filePath`) casing.
const TOOL_ARG_KEYS: &[&str] = &[
    "command",
    "file_path",
    "filePath",
    "pattern",
    "path",
    "query",
];

/// Truthful cap so one long argument (a huge path or command) can never
/// consume the whole 150-char fallback window on its own.
const TOOL_ACTIVITY_LINE_CHARS: usize = 80;

fn claude_tool_use_text(object: &serde_json::Map<String, Value>) -> Option<String> {
    let name = object.get("name")?.as_str()?;
    let input = object.get("input")?.as_object()?;
    Some(tool_activity_line(name, input))
}

fn opencode_tool_use_text(object: &serde_json::Map<String, Value>) -> Option<String> {
    let part = object.get("part")?.as_object()?;
    let name = part.get("tool")?.as_str()?;
    let input = part.get("state")?.as_object()?.get("input")?.as_object()?;
    Some(tool_activity_line(name, input))
}

fn tool_activity_line(name: &str, input: &serde_json::Map<String, Value>) -> String {
    let arg = TOOL_ARG_KEYS
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str));
    let line = match arg {
        Some(arg) => format!("{name}: {arg}"),
        None => name.to_string(),
    };
    line.chars().take(TOOL_ACTIVITY_LINE_CHARS).collect()
}

/// Every `estimated_tokens` value observed in `value`, in document order —
/// bound to exactly one counter source: an object whose own `type` is one of
/// [`THINKING_TEXT_KINDS`], i.e. the same `thinking_delta`/`reasoning_delta`
/// payload [`collect_thinking_texts`] already reads `thinking`/`text` off.
/// Claude-code streams also carry an unrelated, independently-resetting
/// `{"type":"system","subtype":"thinking_tokens","estimated_tokens":…}`
/// counter; that object's own type is `"system"`, never a member of
/// [`THINKING_TEXT_KINDS`], so it is never read here. Multiplexing the two
/// into one accumulator would violate [`TokenProgress`]'s rollover invariant
/// — "a lower value proves the block rolled over" — which only holds when
/// every value comes from one monotonic-with-resets counter.
fn collect_thinking_token_estimates(value: &Value, out: &mut Vec<u64>) {
    match value {
        Value::Object(object) => {
            let kind = object
                .get("type")
                .or_else(|| object.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if THINKING_TEXT_KINDS.contains(&kind)
                && let Some(estimate) = object.get("estimated_tokens").and_then(Value::as_u64)
            {
                out.push(estimate);
            }
            for value in object.values() {
                collect_thinking_token_estimates(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_thinking_token_estimates(item, out);
            }
        }
        _ => {}
    }
}

/// Harness-event wrapper keys checked, in order, when an object does not
/// itself satisfy `matcher`: covers both Claude-style (`result`/`message`)
/// and OpenCode-style (`part`/`parts`/`content`) nested event payloads.
const NESTED_OBJECT_WRAPPER_KEYS: &[&str] = &[
    "result", "output", "message", "content", "text", "data", "body", "response", "part", "parts",
];

/// Recursively search a JSON value for the first nested object matching
/// `matcher`, walking known harness-event wrapper keys and array items, and
/// pulling balanced `{...}` substrings out of string content (via
/// [`balanced_json_objects`]) — shared by slot extraction (`drive.rs`) and
/// merger-decision extraction (`merge.rs`) so both walk exactly the same
/// event/text shapes.
///
/// `on_text_value` is invoked with every JSON value recovered from string
/// content (whether or not it goes on to match), so a caller that wants
/// visibility into what was observed along the way (e.g. for a "requested
/// slot key was never present" diagnostic) can record it as a side effect;
/// pass a no-op closure when that bookkeeping is not needed.
pub(crate) fn find_nested_object<T>(
    value: &Value,
    matcher: &mut impl FnMut(&serde_json::Map<String, Value>) -> Option<T>,
    on_text_value: &mut impl FnMut(&Value),
) -> Option<T> {
    match value {
        Value::Object(object) => {
            if let Some(result) = matcher(object) {
                return Some(result);
            }
            for key in NESTED_OBJECT_WRAPPER_KEYS {
                if let Some(nested) = object.get(*key)
                    && let Some(result) = find_nested_object(nested, matcher, on_text_value)
                {
                    return Some(result);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(result) = find_nested_object(item, matcher, on_text_value) {
                    return Some(result);
                }
            }
            None
        }
        Value::String(text) => {
            // JSON parsed out of message text is model-authored: when it
            // fails to match, its keys are what the model chose to send.
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if let Some(result) = find_nested_object(&parsed, matcher, on_text_value) {
                    return Some(result);
                }
                on_text_value(&parsed);
            }
            for candidate in balanced_json_objects(text)
                .into_iter()
                .filter_map(|item| serde_json::from_str::<Value>(&item).ok())
            {
                if let Some(result) = find_nested_object(&candidate, matcher, on_text_value) {
                    return Some(result);
                }
                on_text_value(&candidate);
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn balanced_json_objects(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0_i32;
    let mut start = None;
    let mut in_string = false;
    let mut escape = false;
    for (index, ch) in text.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0
                    && let Some(start) = start.take()
                {
                    result.push(text[start..=index].to_string());
                }
            }
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        IngestOutcome, NarrationWindows, NarratorConfig, NarratorTraceContext, TokenProgress,
        collect_thinking_texts, ingest_chunk, narrate_cold,
    };
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    fn ingest(chunk: &str) -> (String, String, IngestOutcome) {
        let mut line_buffer = String::new();
        let mut window = String::new();
        let mut fallback_window = String::new();
        let outcome = ingest_chunk(
            &mut line_buffer,
            &mut window,
            &mut fallback_window,
            chunk.as_bytes(),
        );
        (window, fallback_window, outcome)
    }

    /// P496 key fix: `collect_thinking_texts` reads the `"thinking"` payload
    /// key (not just `text`/`content`/`delta`), so a claude thinking delta
    /// that actually carries text is no longer dropped.
    #[test]
    fn thinking_key_is_read_from_thinking_delta() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"Considering the fix"}}}"#,
        )
        .unwrap();
        let mut texts = Vec::new();
        collect_thinking_texts(&value, &mut texts);
        assert_eq!(texts, vec!["Considering the fix".to_string()]);
    }

    /// A redacted-thinking claude stream (empty `thinking_delta`s) leaves the
    /// thinking window empty and falls back to assistant text plus tool
    /// activity — the tool's salient argument, not its full input.
    #[test]
    fn redacted_stream_falls_back_to_assistant_text_and_tool_activity() {
        let chunk = concat!(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":""}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the file"},{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#,
            "\n",
        );
        let (window, fallback_window, outcome) = ingest(chunk);
        assert!(!outcome.has_thinking);
        assert!(window.is_empty());
        assert!(outcome.has_fallback);
        assert!(fallback_window.contains("Reading the file"));
        assert!(fallback_window.contains("Bash: ls -la"));
    }

    /// W1: a `stream_event` `content_block_start` opens a `tool_use` block
    /// before its `input` has streamed in (`{}`); the same call's arguments
    /// always also arrive complete inside a later `assistant` message. Only
    /// the complete copy should reach the fallback window — one line, not
    /// a bare placeholder plus a duplicate.
    #[test]
    fn streaming_tool_use_placeholder_is_skipped_in_favor_of_the_complete_block() {
        let chunk = concat!(
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Bash","input":{}}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#,
            "\n",
        );
        let (_, fallback_window, outcome) = ingest(chunk);
        assert!(outcome.has_fallback);
        let lines: Vec<&str> = fallback_window.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, vec!["Bash: ls -la"]);
    }

    /// Security-shaped assertion: a claude tool-result event never reaches
    /// the fallback window, and an opencode tool event carrying BOTH a
    /// salient `state.input` and a `state.output` body (the realistic
    /// shape — a tool call that has already completed) surfaces only the
    /// name/argument line, never the output body.
    #[test]
    fn tool_results_never_reach_the_fallback_window() {
        let chunk = concat!(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","content":[{"type":"text","text":"BIG FILE BODY"}]}]}}"#,
            "\n",
            r#"{"type":"tool_use","part":{"type":"tool","tool":"read","state":{"input":{"filePath":"README.md"},"output":"ANOTHER BIG BODY"}}}"#,
            "\n",
        );
        let (window, fallback_window, outcome) = ingest(chunk);
        assert!(!outcome.has_thinking);
        assert!(outcome.has_fallback);
        assert!(window.is_empty());
        assert!(fallback_window.contains("read: README.md"));
        assert!(!fallback_window.contains("BIG FILE BODY"));
        assert!(!fallback_window.contains("ANOTHER BIG BODY"));
    }

    /// An opencode stream with non-empty `reasoning.text` fills the thinking
    /// window, so [`NarrationWindows::active`] resolves to it and the
    /// fallback window is never consulted — byte-identical to today.
    #[test]
    fn opencode_reasoning_text_keeps_thinking_diet() {
        let chunk = concat!(
            r#"{"type":"reasoning","part":{"type":"reasoning","text":"Thinking about approach"}}"#,
            "\n",
        );
        let (window, fallback_window, outcome) = ingest(chunk);
        assert!(outcome.has_thinking);
        assert!(window.contains("Thinking about approach"));
        assert!(fallback_window.is_empty());

        let (active, is_fallback) = (NarrationWindows {
            thinking: &window,
            fallback: &fallback_window,
        })
        .active();
        assert!(!is_fallback);
        assert!(active.contains("Thinking about approach"));
    }

    /// `estimated_tokens` resets per thinking block; the settled/current rule
    /// must fold a rollover into `settled` rather than losing it to a raw
    /// max or sum.
    #[test]
    fn thinking_token_progress_accounts_for_block_resets() {
        let mut progress = TokenProgress::default();
        let mut last = None;
        for value in [50, 100, 150, 200, 50, 100] {
            last = progress.observe(&[value]);
        }
        assert_eq!(last, Some(300));
    }

    /// A claude-code stream carries two independent, unrelated
    /// `estimated_tokens` counters: the per-block `thinking_delta` payload
    /// counter, and an unrelated top-level
    /// `{"type":"system","subtype":"thinking_tokens",...}` counter that
    /// resets on its own schedule. Extraction must bind to exactly the
    /// former and never read the latter, or interleaved values from the two
    /// sources get multiplexed into one `TokenProgress` accumulator and its
    /// rollover rule (a lower value proves a block rolled over) reads
    /// cross-source alternation as rollovers, wildly inflating the total.
    #[test]
    fn pill_extraction_ignores_the_unrelated_system_token_counter() {
        let chunk = concat!(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"a","estimated_tokens":50}}}"#,
            "\n",
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":500}"#,
            "\n",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"b","estimated_tokens":100}}}"#,
            "\n",
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":900}"#,
            "\n",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"c","estimated_tokens":150}}}"#,
            "\n",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"d","estimated_tokens":200}}}"#,
            "\n",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"e","estimated_tokens":50}}}"#,
            "\n",
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"f","estimated_tokens":100}}}"#,
            "\n",
        );
        let (_window, _fallback_window, outcome) = ingest(chunk);
        assert_eq!(outcome.estimated_tokens, vec![50, 100, 150, 200, 50, 100]);
        let mut progress = TokenProgress::default();
        let total = progress.observe(&outcome.estimated_tokens);
        assert_eq!(total, Some(300));
    }

    #[test]
    fn cold_narration_persists_debug_trace() {
        let unique = format!(
            "narrator-trace-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let trace_dir = ctx_traits_io::state::current_global_debug_root()
            .unwrap()
            .join(&unique);
        let config = NarratorConfig {
            argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf 'Narrating work\\n'".to_string(),
            ],
            env_overlay: std::collections::BTreeMap::new(),
            env_remove: Vec::new(),
            warm: None,
            prompt_delivery: ctx_traits_io::harness::PromptDelivery::Arg,
            output_id: None,
            task_label: "test step".to_string(),
            timeout_ms: 5_000,
            exec_dir: None,
            confinement: None,
            sandbox: None,
            trace: Some(NarratorTraceContext {
                run_id: unique.clone(),
                session_id: "session-narrator-trace-test".to_string(),
                item_id: Some("test-step".to_string()),
                frame_title: "Test step".to_string(),
                harness_id: "fake".to_string(),
                sequence: Arc::new(AtomicU64::new(0)),
            }),
        };

        let accumulator = ctx_traits_io::harness::AttemptTokenAccumulator::new(|_delta| {});
        let summary = narrate_cold(&config, "Summarize this".to_string(), &accumulator).unwrap();
        assert_eq!(summary, "Narrating work");

        let paths = std::fs::read_dir(&trace_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let trace = std::fs::read_to_string(&paths[0]).unwrap();
        assert!(trace.contains(r#""trace":"dispatch""#));
        assert!(trace.contains("Narrating work"));
        assert!(trace.contains(r#""trace":"exit""#));

        std::fs::remove_dir_all(trace_dir).unwrap();
    }
}
