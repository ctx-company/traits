//! Normalized, provider-independent drive activity.

use std::collections::HashMap;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::session::{DriveOutcomeKind, Status};

/// The actionable state of a session. This is derived at read time; it is not
/// persisted alongside the ledger's more detailed runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SessionState {
    Running,
    WaitingOnHuman,
    WaitingOnAgent,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl SessionState {
    /// A held driver lock is authoritative over a stale persisted disposition.
    pub fn derive(status: &Status, outcome: Option<&DriveOutcomeKind>, live: bool) -> Self {
        if live {
            return Self::Running;
        }
        if !matches!(status, Status::WaitingOnHuman)
            && matches!(
                outcome,
                Some(DriveOutcomeKind::Interrupted) | Some(DriveOutcomeKind::Killed)
            )
        {
            return Self::Cancelled;
        }
        match status {
            Status::AwaitingInput
            | Status::WaitingOnHuman
            | Status::BlockedCommandPermissionRequired => Self::WaitingOnHuman,
            Status::AwaitingAgentOutput | Status::Rejected => Self::WaitingOnAgent,
            Status::Blocked | Status::BlockedAgentUnassigned => Self::Blocked,
            Status::Completed => Self::Completed,
            Status::Failed => Self::Failed,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn is_resumable(self) -> bool {
        !self.is_terminal() && !matches!(self, Self::Running)
    }
}

/// A provider-independent activity category for one frame attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ActivityKind {
    Dispatching,
    Thinking,
    RunningTool,
    StreamingOutput,
    ValidatingOutput,
    Retrying,
    Stalled,
    Compacting,
    NoActivityReported,
    RateLimited,
}

/// A decoded `rate_limit_event` observation from a subscription harness
/// (P556/0117). `limit_type` and `utilization` are optional because the
/// wire payload omits them on some observed events; `resets_at_epoch` is
/// an epoch and must never be converted to a duration downstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RateLimitObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_type: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
    pub resets_at_epoch: u64,
}

impl RateLimitObservation {
    /// Tolerant decode of a claude-code `rate_limit_info` payload
    /// (P556/0117). Shared by the activity adapter (`ctx-traits-io`) and the
    /// provider-error classifier (`ctx-traits-core`) so the wire shape is
    /// decoded in exactly one place. `status` and `resetsAt` are the only
    /// fields observed on every sampled event; everything else is optional.
    pub fn decode(info: &serde_json::Value) -> Option<Self> {
        let status = info.get("status").and_then(serde_json::Value::as_str)?;
        let resets_at_epoch = info.get("resetsAt").and_then(serde_json::Value::as_u64)?;
        Some(Self {
            limit_type: info
                .get("rateLimitType")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            status: status.to_string(),
            utilization: info.get("utilization").and_then(serde_json::Value::as_f64),
            resets_at_epoch,
        })
    }
}

/// One ordered activity observation. Text is deliberately bounded by adapters;
/// tool results are not represented in this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ActivityEvent {
    pub sequence: u64,
    pub frame_id: String,
    pub kind: ActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Estimated thinking/output tokens this event carries, when the
    /// adapter can compute one (P521). Additive and `skip_serializing_if`
    /// so a ledger/sidecar written before this field existed deserializes
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// Decoded `rate_limit_event` evidence (P556/0117). Additive and
    /// `skip_serializing_if` so pre-existing ledgers/sidecars deserialize
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitObservation>,
}

/// Human-facing label for an `ActivityKind`, shared between the CLI's
/// fallback tail (`ctx_traits_cli::app::run_view::projection`) and the
/// desktop's activity block so both name the same closed set of kinds the
/// same way.
pub fn activity_kind_label(kind: &ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Dispatching => "dispatching",
        ActivityKind::Thinking => "thinking",
        ActivityKind::RunningTool => "running tool",
        ActivityKind::StreamingOutput => "streaming output",
        ActivityKind::ValidatingOutput => "validating output",
        ActivityKind::Retrying => "retrying",
        ActivityKind::Stalled => "stalled",
        ActivityKind::Compacting => "compacting",
        ActivityKind::NoActivityReported => "no activity reported",
        ActivityKind::RateLimited => "rate limited",
    }
}

/// One line of rendered agent activity for `event`: quoted agent text for
/// `Thinking`/`StreamingOutput`, the tool label (never raw tool-input JSON)
/// for `RunningTool`, and the kind label otherwise. The exact semantics the
/// CLI's `activity_event_fallback_tail` (`run_view/projection.rs`) already
/// holds, extracted here so the desktop's activity block renders the same
/// text rather than forking a third formatter.
pub fn activity_event_line(event: &ActivityEvent) -> String {
    match event.kind {
        ActivityKind::StreamingOutput | ActivityKind::Thinking => event
            .text
            .as_deref()
            .map(crate::text::quote_line)
            .unwrap_or_else(|| activity_kind_label(&event.kind).to_string()),
        ActivityKind::RunningTool => event
            .tool
            .clone()
            .unwrap_or_else(|| activity_kind_label(&event.kind).to_string()),
        _ => activity_kind_label(&event.kind).to_string(),
    }
}

/// The latest event available for a frame, suitable for live surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CurrentActivity {
    pub frame_id: String,
    pub kind: ActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl From<&ActivityEvent> for CurrentActivity {
    fn from(event: &ActivityEvent) -> Self {
        Self {
            frame_id: event.frame_id.clone(),
            kind: event.kind.clone(),
            text: event.text.clone(),
        }
    }
}

/// A bounded fold of one (first-observed, last-observed) `at_epoch_ms` stamp
/// pair per frame id, extracted from
/// `ctx_traits_cli::app::story::beat_duration`'s span semantics so every
/// consumer (the CLI story, the desktop frame list) shares one derivation.
/// Clock-free: every stamp is a caller-supplied argument, so this type never
/// reads wall-clock time itself. "First"/"last" are call-order, not
/// magnitude-sorted — a caller that observes stamps out of chronological
/// order gets a non-increasing pair back, which `span` then correctly omits
/// rather than silently reordering evidence into a fabricated positive span.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameSpans {
    stamps: HashMap<String, (u64, u64)>,
}

impl FrameSpans {
    /// Unconditionally folds `at_epoch_ms` into the first/last stamp pair for
    /// `frame_id`, in call order — a plain, sequence-free append-order fold.
    /// This is the shared derivation used both by a full, single-pass
    /// historical scan and by a live delta stream; it is not itself
    /// identity-aware, so a caller that hands it the same durable record
    /// twice (a seed/pending overlap) is treated as two observations. A
    /// no-op skip is retained only for the trivial case of redelivering the
    /// stamp already stored as *last* — semantically inert either way, but
    /// avoids marking `changed` on a pure repeat of the tail.
    ///
    /// Deliberately not keyed on `ActivityEvent::sequence`: production does
    /// not provide `sequence` as a durable identity —
    /// `ActivityRecorder::flush_pending` persists every coalesced record
    /// with `sequence: 0`, and each drive's recorder restarts its own
    /// sequence counter — so a sequence-keyed dedup drops ordinary later
    /// durable records that happen to reuse or reset a sequence value.
    ///
    /// Reconciling a genuine seed/pending overlap (the exact same durable
    /// record folded once by a seed's historical read and once more as a
    /// replayed live delta) is deliberately **not** this fold's job: with
    /// only the first/last pair retained, an interior record's stamp is
    /// indistinguishable from a genuinely new one once it has been folded
    /// away, so any point-patch here that special-cases a stamp matching a
    /// stored endpoint either fails to recognize an interior replay (when it
    /// matches neither endpoint) or discards a genuine record that happens
    /// to share a stamp with an endpoint (see
    /// `historical_fold_keeps_a_final_stamp_equal_to_first` below).
    /// Reconciliation instead belongs at the caller's boundary, where the
    /// full set of already-folded records is still available to match
    /// against by bounded occurrence — see
    /// `desktop::detail_tree::ActivityOverlay` and
    /// `desktop::detail::RunDetail::apply`. Returns whether the fold
    /// actually changed the stored pair.
    pub fn observe(&mut self, frame_id: &str, at_epoch_ms: u64) -> bool {
        let mut changed = false;
        self.stamps
            .entry(frame_id.to_string())
            .and_modify(|(_, last)| {
                if *last != at_epoch_ms {
                    *last = at_epoch_ms;
                    changed = true;
                }
            })
            .or_insert_with(|| {
                changed = true;
                (at_epoch_ms, at_epoch_ms)
            });
        changed
    }

    pub fn from_events<'a>(events: impl Iterator<Item = (&'a str, u64)>) -> Self {
        let mut spans = Self::default();
        for (frame_id, at_epoch_ms) in events {
            spans.observe(frame_id, at_epoch_ms);
        }
        spans
    }

    /// `None` unless `frame_id` occurs exactly once in the reconstruction the
    /// caller counted `executions` over, and its first/last stamps actually
    /// advance — ambiguity (a second execution sharing the same identity, or
    /// a single non-increasing stamp pair) always resolves to "omit", never
    /// to a guessed duration.
    pub fn span(&self, frame_id: &str, executions: usize) -> Option<Duration> {
        if executions != 1 {
            return None;
        }
        let (first, last) = *self.stamps.get(frame_id)?;
        (last > first).then(|| Duration::from_millis(last - first))
    }
}

/// Format a relative duration in compact human-readable units (`4s`,
/// `3m 12s`, `2h 3m 4s`) — the shared home for what
/// `ctx_traits_cli::app::tui::human_elapsed_text` used to compute inline, so
/// the desktop gets the identical rendering without depending on the CLI
/// crate.
pub fn compact_elapsed_text(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_wins_and_terminal_states_are_not_resumable() {
        assert_eq!(
            SessionState::derive(&Status::Completed, None, true),
            SessionState::Running
        );
        assert!(SessionState::Completed.is_terminal());
        assert!(!SessionState::Completed.is_resumable());
        assert!(SessionState::WaitingOnAgent.is_resumable());
    }

    #[test]
    fn unheld_interrupted_or_killed_waiting_on_human_remains_resumable() {
        for outcome in [DriveOutcomeKind::Interrupted, DriveOutcomeKind::Killed] {
            let state = SessionState::derive(&Status::WaitingOnHuman, Some(&outcome), false);
            assert_eq!(state, SessionState::WaitingOnHuman);
            assert!(state.is_resumable());
        }
    }

    #[test]
    fn unheld_interrupted_or_killed_other_statuses_remain_cancelled() {
        for status in [
            Status::AwaitingInput,
            Status::AwaitingAgentOutput,
            Status::Rejected,
            Status::BlockedCommandPermissionRequired,
            Status::Blocked,
            Status::BlockedAgentUnassigned,
            Status::Completed,
            Status::Failed,
        ] {
            for outcome in [DriveOutcomeKind::Interrupted, DriveOutcomeKind::Killed] {
                assert_eq!(
                    SessionState::derive(&status, Some(&outcome), false),
                    SessionState::Cancelled
                );
                assert_eq!(
                    SessionState::derive(&status, Some(&outcome), true),
                    SessionState::Running
                );
            }
        }
    }

    #[test]
    fn disk_full_outcome_keeps_an_agent_waiting_session_resumable() {
        assert_eq!(
            SessionState::derive(
                &Status::AwaitingAgentOutput,
                Some(&DriveOutcomeKind::DiskFull),
                false,
            ),
            SessionState::WaitingOnAgent
        );
    }

    #[test]
    fn frame_spans_reports_a_positive_span_for_a_unique_frame() {
        let spans = FrameSpans::from_events(vec![("frame", 1_000), ("frame", 3_000)].into_iter());
        assert_eq!(spans.span("frame", 1), Some(Duration::from_millis(2_000)));
    }

    #[test]
    fn frame_spans_omits_when_no_records_match() {
        let spans = FrameSpans::default();
        assert_eq!(spans.span("frame", 1), None);
    }

    #[test]
    fn frame_spans_omits_a_single_record() {
        let spans = FrameSpans::from_events(vec![("frame", 1_000)].into_iter());
        assert_eq!(spans.span("frame", 1), None);
    }

    #[test]
    fn frame_spans_omits_equal_stamps() {
        let spans = FrameSpans::from_events(vec![("frame", 1_000), ("frame", 1_000)].into_iter());
        assert_eq!(spans.span("frame", 1), None);
    }

    #[test]
    fn frame_spans_omits_a_non_increasing_pair() {
        let mut spans = FrameSpans::default();
        spans.observe("frame", 5_000);
        spans.observe("frame", 1_000);
        assert_eq!(spans.span("frame", 1), None);
    }

    #[test]
    fn frame_spans_omits_when_the_frame_id_occurs_more_than_once() {
        let spans = FrameSpans::from_events(vec![("frame", 1_000), ("frame", 3_000)].into_iter());
        assert_eq!(spans.span("frame", 2), None);
    }

    #[test]
    fn observe_folds_every_new_stamp_even_when_it_does_not_advance() {
        // Append order 1_000, 3_000, 2_000: `observe` must fold every one of
        // them in call order — magnitude plays no role in whether a call
        // folds, only in what `span` later reports.
        let mut spans = FrameSpans::default();
        assert!(spans.observe("frame", 1_000));
        assert!(spans.observe("frame", 3_000));
        assert!(spans.observe("frame", 2_000));
        assert_eq!(spans.span("frame", 1), Some(Duration::from_millis(1_000)));
    }

    #[test]
    fn observe_reports_no_change_when_a_redelivered_stamp_is_reobserved() {
        let mut spans = FrameSpans::default();
        assert!(spans.observe("frame", 1_000));
        assert!(spans.observe("frame", 3_000));
        assert!(
            !spans.observe("frame", 3_000),
            "reobserving the same last stamp is idempotent, not a change"
        );
        assert_eq!(spans.span("frame", 1), Some(Duration::from_millis(2_000)));
    }

    #[test]
    fn historical_fold_keeps_a_final_stamp_equal_to_first() {
        // Append order 1_000, 3_000, 1_000 — a single-pass historical scan
        // (not a seed/pending overlap) whose last-folded stamp genuinely
        // equals the first. `observe` folds every call in order, with no
        // identity guard that could mistake this for a redelivered "first"
        // stamp and discard it: the fold must retain 1_000 as the final
        // `last`, making the pair non-increasing and `span` must omit it
        // (review-verdict-1 blocker `live-span-reopen-divergence` —
        // reconciliation of an actual seed/pending overlap belongs at the
        // caller's boundary, not inside this identity-free fold).
        let spans = FrameSpans::from_events(
            vec![("frame", 1_000), ("frame", 3_000), ("frame", 1_000)].into_iter(),
        );
        assert_eq!(spans.span("frame", 1), None);
    }

    #[test]
    fn observe_folds_every_durable_record_even_when_sequence_is_reused_as_zero() {
        // `ActivityRecorder::flush_pending` persists every coalesced record
        // with `sequence: 0`; `FrameSpans` must not depend on `sequence` at
        // all, so two same-frame records that both carry `sequence: 0` still
        // both fold.
        let spans = FrameSpans::from_events(vec![("frame", 1_000), ("frame", 4_000)].into_iter());
        assert_eq!(spans.span("frame", 1), Some(Duration::from_millis(3_000)));
    }

    #[test]
    fn compact_elapsed_text_uses_compact_units() {
        assert_eq!(compact_elapsed_text(Duration::from_secs(8)), "8s");
        assert_eq!(compact_elapsed_text(Duration::from_secs(128)), "2m 8s");
        assert_eq!(
            compact_elapsed_text(Duration::from_secs(93_784)),
            "26h 3m 4s"
        );
    }
}
