//! Normalized, provider-independent drive activity.

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
        if matches!(
            outcome,
            Some(DriveOutcomeKind::Interrupted) | Some(DriveOutcomeKind::Killed)
        ) {
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
    fn killed_outcome_derives_cancelled_like_interrupted() {
        assert_eq!(
            SessionState::derive(&Status::Failed, Some(&DriveOutcomeKind::Killed), false),
            SessionState::Cancelled
        );
    }
}
