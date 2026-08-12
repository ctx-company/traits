//! Pure resumable procedure run sessions.
//!
//! This module wraps the executable procedure runtime ledger with a stable
//! session/frame/call envelope for CLI, MCP, and WASM adapters. It does not
//! perform filesystem, process, network, provider, model, clock, UUID, MCP, or
//! host IO. Callers supply already-loaded trait sources, input values, and
//! resource evidence; adapters persist or transport the returned envelopes.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::digest::Digest;
use crate::procedure::run::Id;
use crate::procedure::runtime::{
    AcceptanceStatus, AgentRole, BudgetExhaustedPause, ControlFrame, ControlKind, EffectBuffer,
    NextSequenceFrameResult, OutputPortStatus, PathSegment, ProviderCreditsPause, RejectedAttempt,
    ResourceEvidence, SchemaStatus, SequenceCallTemplate, SequenceCallerTemplate, SequenceFrame,
    SequenceFrameKind, SequenceSignalTemplate, SignalEmission, SlotRevision, State, StepNextAction,
    StepOutputEnvelope, StepSignalOutput, StepSlotOutput, StepValidationReport, StopReason, Value,
    ValueSource, apply_step_output, apply_terminal_frame_failure, bind_current_for_each_item,
    next_sequence_frame, rollback_active_parallel_branch, start_procedure_run,
    validate_run_ledger_contract,
};
use crate::reference::{Kind, Reference};
use crate::response::CapabilityReport;
use crate::r#trait::{PortDirection, Trait};

pub const SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(crate::procedure::invalid_field(
                "run-session.id",
                "must not be empty",
            ));
        }
        Ok(Self(id))
    }

    pub fn deterministic(
        trait_id: &str,
        source_digest: Option<&str>,
        canonical_digest: Option<&str>,
        initial_port_values: &[StepSlotOutput],
    ) -> crate::Result<Self> {
        let mut values = initial_port_values.to_vec();
        values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
        let seed = serde_json::json!({
            "trait-id": trait_id,
            "source-digest": source_digest,
            "canonical-digest": canonical_digest,
            "initial-port-values": values,
        });
        let text = serde_json::to_string(&seed).map_err(|e| {
            crate::procedure::serialization("run-session.id", "run-session seed", e)
        })?;
        let digest = Digest::source(&text);
        Ok(Self(format!(
            "session-{}",
            digest.as_str().trim_start_matches("sha256:")
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn deterministic_run_id(
    source_digest: Option<&str>,
    canonical_digest: Option<&str>,
) -> crate::Result<Id> {
    let seed = serde_json::json!({
        "source-digest": source_digest,
        "canonical-digest": canonical_digest,
    });
    let text = serde_json::to_string(&seed)
        .map_err(|e| crate::procedure::serialization("run-session.run-id", "run-id seed", e))?;
    let digest = Digest::source(&text);
    Id::new(format!(
        "run-{}",
        digest.as_str().trim_start_matches("sha256:")
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Status {
    AwaitingInput,
    WaitingOnHuman,
    AwaitingAgentOutput,
    BlockedCommandPermissionRequired,
    BlockedAgentUnassigned,
    Rejected,
    Blocked,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum CallResponseKind {
    AcceptedNextFrame,
    AcceptedCompleted,
    RejectedCorrectionRequired,
    BlockedMissingInput,
    AlreadyCompleted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CallerProvenance {
    pub surface: String,
    pub caller: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

impl CallerProvenance {
    pub fn cli() -> Self {
        Self {
            surface: "cli".to_string(),
            caller: "ctx traits".to_string(),
            agent: None,
            harness: None,
        }
    }

    pub fn mcp() -> Self {
        Self {
            surface: "mcp".to_string(),
            caller: "ctx traits mcp adapter".to_string(),
            agent: None,
            harness: None,
        }
    }

    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AgentAssignment {
    pub role: String,
    pub harness: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub evidence: String,
    /// 1-based seat position within the role's configured
    /// `[[agent.role.<role>]]` list (P456). Absent for a legacy
    /// single-table role, so its serialized bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_index: Option<u32>,
    /// The configured list length this seat was selected from. Present if
    /// and only if `seat_index` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_length: Option<u32>,
    /// P475: this seat's resolved `[agent.role.<role>].budget` at the run's
    /// first invocation. `None` when the seat declared no budget at all
    /// (every frame for it resolves through the built-in default chain
    /// instead) — additive and `skip_serializing_if`, so a ledger written
    /// before this phase deserializes unchanged and every existing row's
    /// serialized bytes stay identical until an operator declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<RoleBudgetEvidence>,
}

/// The declared subset of a seat's `[agent.role.<role>].budget` (P475),
/// recorded once per run at session creation — never the merged run-level
/// chain, which is only known at drive-frame time and can differ resume to
/// resume as CLI flags change; this records what the seat's OWN
/// configuration declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RoleBudgetEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
}

/// Select the one stored `AgentAssignment` row that a frame's `role` at
/// `structural_seat` binds to (P456), so every reporting/preview/reporting
/// surface picks the same row the drive path dispatched to instead of the
/// first (or last) role match. Mirrors [`super::runtime::AgentRole`]'s own
/// `entries[ordinal % len]` selection, but over the already-resolved,
/// persisted rows rather than the live config profile. A role with no
/// `seat_index` evidence (legacy single-table) returns its one row
/// unconditionally, regardless of `structural_seat`.
pub fn select_agent_assignment<'a>(
    rows: &'a [AgentAssignment],
    role: &str,
    structural_seat: Option<u32>,
) -> Option<&'a AgentAssignment> {
    let role_rows: Vec<&AgentAssignment> = rows.iter().filter(|row| row.role == role).collect();
    let list_length = role_rows.iter().find_map(|row| row.list_length);
    match list_length {
        Some(list_length) => {
            let target_seat = (structural_seat.unwrap_or(0) % list_length.max(1)) + 1;
            role_rows
                .iter()
                .find(|row| row.seat_index == Some(target_seat))
                .copied()
        }
        None => role_rows.first().copied(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HarnessProbeEvidence {
    pub harness_id: String,
    pub bin: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Provenance {
    pub started_by: CallerProvenance,
    pub state_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_assignments: Option<Vec<AgentAssignment>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness_probes: Vec<HarnessProbeEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_source: Option<TraitSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_selection: Option<crate::run_info::RunInfoSelectionSummary>,
    /// Prepared `--worktree` provenance, attached at start. `None` for runs
    /// started without `--worktree` (including legacy ledgers), which
    /// `ctx traits merge` must report as unresolvable rather than guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeProvenance>,
    /// Ordered `ctx traits merge` outcome history for this run. Preserved
    /// verbatim across session reconstruction (unlike `last_drive_outcome`,
    /// which core clears on every rebuild) because provenance itself is
    /// threaded through unchanged by every core transition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_frames: Vec<MergeFrame>,
    /// P460 resolved automatic-landing intent, persisted before driving so a
    /// credits-paused, later-resumed drive lands with the same rung
    /// regardless of `[merge]` config changes made in between. `None` for a
    /// run started without an effective `--merge` request (including every
    /// legacy ledger); `ctx traits drive --no-merge` clears it before
    /// resuming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_intent: Option<MergeRung>,
    /// P479 out-of-tree mutation tripwire evidence: every escape this run's
    /// drive loop observed at a frame boundary, in the order detected.
    /// Durable, unlike `[worktree]` overlay/confinement config values (which
    /// stay operational-only) — a finding here is deliberately canonical
    /// provenance, read by `ctx traits merge` to refuse landing a run whose
    /// invocation repository mutated under `policy = "park"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_of_tree_mutations: Vec<OutOfTreeMutationEvidence>,
    /// P510 liveness evidence: wall-clock epoch seconds this session was
    /// created, stamped once at the IO boundary (`ctx-traits-io::run::start`)
    /// so a driver's start time is available without parsing the ledger's
    /// full frame history. `None` for every ledger written before this field
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_epoch: Option<u64>,
    /// Trust evidence accepted at start. Continuations validate this durable
    /// pin and never reopen the mutable machine trust store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_approval: Option<TrustApprovalProvenance>,
    /// P552 one-time narrator session title. `None` means "not yet
    /// requested" (including every ledger written before this field
    /// existed); once `Some`, `attempted` is permanently `true` and a
    /// resumed drive must not dispatch a second title call regardless of
    /// whether `title` itself resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<SessionTitleState>,
    /// 0061: the byte digest of the task document materialised into this
    /// run's worktree at dispatch, alongside the resolved task key —
    /// "what was this run told to do" answerable from evidence alone
    /// without re-resolving the board. `None` for a run dispatched without
    /// a resolvable task (including every legacy ledger, and every run of
    /// a trait outside the `implement-*` family).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_key: Option<String>,
    /// 0061: recorded when `--override-dependencies` dispatched this run
    /// despite an unmet `depends-on`. `None` for every run that dispatched
    /// with no unmet dependency, or that never resolved a task at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_override: Option<DependencyOverrideProvenance>,
}

/// One `depends-on` edge that was unmet at dispatch time, as recorded by a
/// `--override-dependencies` dispatch — the edge's key and derived status
/// AT THAT MOMENT, not a live re-derivation (the dependency may since have
/// closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct UnmetDependencyEvidence {
    pub key: String,
    pub status: String,
}

/// A recorded dependency-preflight override (0061): the task that was
/// dispatched and every `depends-on` edge that was unmet at that moment.
/// An override that left no trace would be exactly the silent dishonesty
/// this product exists to remove, so this is typed provenance, not a
/// free-text warning alone (a human-readable line is additionally pushed
/// onto [`Provenance::warnings`] so every existing run view surfaces it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DependencyOverrideProvenance {
    pub task_key: String,
    pub unmet: Vec<UnmetDependencyEvidence>,
}

/// Where a resolved session title came from (task 0110): the same
/// provenance philosophy as a loop recording its exit mechanism. Absent on
/// every legacy ledger and every state P552's auto-title path still writes,
/// which decodes to [`Self::NarratorDefault`].
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SessionTitleSource {
    /// The pre-0110 auto-title path: narrator titles from trait name and
    /// input text, or a legacy ledger with no recorded source.
    #[default]
    NarratorDefault,
    /// A `[sink.session-title]` declaration with a `verbatim` mode: a
    /// deterministic render, never dispatched to a narrator.
    SinkVerbatim,
    /// A `[sink.session-title]` declaration with a `generated` mode: the
    /// assembled slot material became the narrator prompt's context.
    SinkGenerated,
}

/// Persisted lifecycle of the optional narrator title. `None` on
/// [`Provenance::session_title`] remains the unattempted state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[serde(tag = "state")]
pub enum SessionTitleState {
    InFlight {
        owner: String,
        attempts: u32,
    },
    Retryable {
        attempts: u32,
    },
    Resolved {
        attempts: u32,
        title: String,
        source: SessionTitleSource,
    },
    Terminal {
        attempts: u32,
        reason: String,
    },
}

impl SessionTitleState {
    pub fn resolved_title(&self) -> Option<&str> {
        match self {
            Self::Resolved { title, .. } => Some(title),
            _ => None,
        }
    }

    pub fn resolved_source(&self) -> Option<SessionTitleSource> {
        match self {
            Self::Resolved { source, .. } => Some(*source),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for SessionTitleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("session title must be an object"))?;
        if let Some(state) = object.get("state").and_then(JsonValue::as_str) {
            let attempts = object
                .get("attempts")
                .and_then(JsonValue::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| serde::de::Error::custom("session title state requires attempts"))?;
            return match state {
                "in-flight" => object
                    .get("owner")
                    .and_then(JsonValue::as_str)
                    .map(|owner| Self::InFlight {
                        owner: owner.to_string(),
                        attempts,
                    })
                    .ok_or_else(|| {
                        serde::de::Error::custom("in-flight session title requires owner")
                    }),
                "retryable" => Ok(Self::Retryable { attempts }),
                "resolved" => {
                    let source = match object.get("source").and_then(JsonValue::as_str) {
                        None => SessionTitleSource::NarratorDefault,
                        Some(raw) => raw.parse::<SessionTitleSource>().map_err(|_| {
                            serde::de::Error::custom("unknown session title source")
                        })?,
                    };
                    object
                        .get("title")
                        .and_then(JsonValue::as_str)
                        .map(|title| Self::Resolved {
                            attempts,
                            title: title.to_string(),
                            source,
                        })
                        .ok_or_else(|| {
                            serde::de::Error::custom("resolved session title requires title")
                        })
                }
                "terminal" => object
                    .get("reason")
                    .and_then(JsonValue::as_str)
                    .map(|reason| Self::Terminal {
                        attempts,
                        reason: reason.to_string(),
                    })
                    .ok_or_else(|| {
                        serde::de::Error::custom("terminal session title requires reason")
                    }),
                _ => Err(serde::de::Error::custom("unknown session title state")),
            };
        }
        // P552's old shape used `attempted`; an old claim without a title was
        // terminal, never a newly retryable request.
        if object.get("attempted").and_then(JsonValue::as_bool) == Some(true) {
            return Ok(match object.get("title").and_then(JsonValue::as_str) {
                Some(title) => Self::Resolved {
                    attempts: 1,
                    title: title.to_string(),
                    source: SessionTitleSource::NarratorDefault,
                },
                None => Self::Terminal {
                    attempts: 1,
                    reason: "legacy-attempted".to_string(),
                },
            });
        }
        Err(serde::de::Error::custom(
            "invalid legacy session title state",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TrustApprovalProvenance {
    pub trait_id: String,
    pub canonical_digest: Digest,
    pub seq: u64,
    pub approved_at: Option<String>,
}

/// One typed P479 finding: the offending paths, the frame-boundary label the
/// drive loop had current when the checkpoint first observed them, and the
/// policy actually applied (`"park"` or `"warn"` — what happened, not what
/// config says today, so a later config edit cannot rewrite past evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct OutOfTreeMutationEvidence {
    pub paths: Vec<String>,
    pub frame: String,
    pub policy: String,
    pub detected_at_epoch: u64,
}

/// Rung an automatic post-drive landing (P460) resolves to before driving:
/// `Standard` invokes `ctx traits merge` as usual, `Deep` invokes it with
/// `--deep`. Persisted once at start/resolution time — never re-resolved
/// from config at landing time — so an explicit `--merge=standard` request
/// is not silently upgraded by a later `[merge] deep = true` edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum MergeRung {
    Standard,
    Deep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitSource {
    pub kind: String,
    pub path: String,
    /// Repository that owned the selected source. This anchors
    /// relative paths when a session is resumed from another checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_root: Option<String>,
    /// Exact source text loaded at session start. This is deliberately kept
    /// with the ledger so a rebuilt source path cannot change a resumed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
}

/// Resolvable worktree provenance for a `--worktree` run: the prepared
/// worktree id and its `ctx/run/<id>` branch. Data-only — no path, process, or
/// Git behavior. Attached at the IO boundary when a worktree is prepared, so a
/// later `ctx traits merge <run-id>` can resolve back to the branch/worktree
/// without heuristic guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct WorktreeProvenance {
    pub id: String,
    pub branch: String,
    /// Byte digests of every regular file materialized under each declared
    /// seed root at prepare time. `None` for legacy ledgers recorded before
    /// this field existed — their baseline cannot be reconstructed, so
    /// `ctx traits merge` must treat them as unharvestable rather than guess.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seed_snapshots: Vec<SeedSnapshot>,
    /// P510 liveness evidence: the prepared worktree's filesystem path,
    /// stamped where the worktree is prepared (`ctx-traits-io::run::start`).
    /// `None` for legacy ledgers recorded before this field existed — their
    /// path cannot be reconstructed without shelling out to `git worktree
    /// list`, so callers must treat absence as unresolvable rather than guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Seed-time file digests for one declared `[worktree] seed` root, captured
/// when the root is copied into a prepared worktree. Data-only — the
/// filesystem walk that produces these lives in `ctx-traits-io`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SeedSnapshot {
    pub root: String,
    /// Repository-relative file path to seed-time byte digest, sorted by path
    /// for deterministic serialization.
    pub files: Vec<SeedFileDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SeedFileDigest {
    pub path: String,
    pub digest: Digest,
}

/// Stage of the `ctx traits merge` lifecycle a [`MergeFrame`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum MergeStage {
    /// Cross-process merge-lock acquisition, ahead of `Preflight`. Nonterminal:
    /// a `Lock`/`LockAcquired` frame always precedes further stages for the
    /// same merge attempt and never appears as the last frame for a landed or
    /// parked run.
    Lock,
    Preflight,
    Rebase,
    Reconciliation,
    Gates,
    Landing,
    Cleanup,
}

/// Outcome of a merge stage. `Parked` always leaves the branch and worktree
/// registered (rebase aborted first, if one was in progress); `Merged` is the
/// terminal success outcome after a fast-forward; `PostMergeCleanupFailure` is
/// the distinct terminal outcome for a cleanup failure after `main` has
/// already advanced — it must never be reported as parked; `RecoveryFailure`
/// is the distinct terminal outcome when an in-progress rebase's abort could
/// not itself be confirmed — it must never be reported as parked either,
/// since `Parked` promises the branch/worktree were left intact and that is
/// exactly what could not be verified here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum MergeStatus {
    /// Recorded once the cross-process merge lock (`.ctx/runs/merge.lock`) is
    /// held, before `Preflight` runs. Nonterminal and purely operational: wait
    /// duration and observed holder pid/run-id are ordered `evidence` strings
    /// on the same frame, not canonical trait state, so they are provenance
    /// excluded from [`run_session_digest`] (which digests only
    /// `session.ledger`, never `provenance.merge_frames`).
    LockAcquired,
    Parked,
    /// Recorded once the repository gates pass in the worktree, before `main`
    /// is touched — the atomic checkpoint the fast-forward re-reads `main`
    /// against so a lost race parks instead of landing behind stale evidence.
    GatesPassed,
    /// Recorded once P420 `--deep` reconciliation completes with at least one
    /// typed decision, before the branch tip is amended with trailers and
    /// `main` is touched — or (P463) once a `--deep` seed-harvest adjudication
    /// completes with at least one typed decision, on a `Cleanup`-stage frame
    /// instead, since gitignored seed paths are never amended onto the branch
    /// tip. Nonterminal, like `LockAcquired`. Never appears for a fast-forward
    /// or a standard no-flag merge — those carry no deep decisions to log.
    Reconciled,
    Merged,
    PostMergeCleanupFailure,
    RecoveryFailure,
}

impl MergeStatus {
    /// True for a stage outcome that ends a merge attempt for good — landed,
    /// parked, or a distinct cleanup/recovery failure — as opposed to
    /// `LockAcquired`/`GatesPassed`/`Reconciled`, which always precede a
    /// further stage within the same still-in-progress attempt. P460's
    /// one-shot automatic-landing check uses this to recognize that a prior
    /// attempt already ran, instead of re-deriving it from report strings.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            MergeStatus::Merged
                | MergeStatus::Parked
                | MergeStatus::PostMergeCleanupFailure
                | MergeStatus::RecoveryFailure
        )
    }
}

/// The first (in acceptance order) recorded slot revision whose command
/// evidence is a successful, non-timed-out `git commit` — the single fact
/// every landing-honesty surface reuses as "this run committed something",
/// without a second, independent shell-out to check. Built-in commit tails
/// are plain `argv: ["git", "commit", ...]` command steps, so this is a
/// direct argv scan, not a subprocess call: `argv[0] == "git"`, the first
/// non-flag argument is `"commit"`, `exit_code == Some(0)`, `!timed_out`.
/// `None` for a clean-tree run (the `maybe-commit` guard records no
/// revision at all) and for any ledger whose commit tail used a different
/// shape (e.g. `sh -c "git commit ..."`), which this deliberately does not
/// recognize — every shipped built-in uses the direct form.
pub fn commit_receipt(
    state: &State,
) -> Option<&crate::procedure::runtime::CommandExecutionEvidence> {
    state
        .slot_revisions
        .iter()
        .filter_map(|revision| revision.command_execution.as_ref())
        .find(|evidence| {
            evidence.exit_code == Some(0)
                && !evidence.timed_out
                && evidence.argv.first().map(String::as_str) == Some("git")
                && evidence
                    .argv
                    .iter()
                    .skip(1)
                    .find(|argument| !argument.starts_with('-'))
                    .map(String::as_str)
                    == Some("commit")
        })
}

/// Terminal landing outcome of a completed run, classified once from the
/// same evidence every reporting surface (story, TUI, plain drive report,
/// summary JSON, dashboard) reuses — never a second derivation of "did it
/// land". A merge frame history takes priority (it is the authoritative
/// record of an actual `ctx traits merge` attempt); absent one, a completed
/// `--worktree` drive with a [`commit_receipt`] is `NotMerged` — committed
/// on the run branch, never landed. `None` covers every case that is not
/// this defect's concern: a non-worktree run, a run still in progress, and
/// a completed run with nothing committed (the clean-tree skip case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingState {
    Landed { revision: Option<String> },
    NotMerged,
    Parked,
    MergeFailed,
}

/// Classify `session`'s landing state from its own persisted evidence. See
/// [`LandingState`] for the precedence and exclusions.
pub fn landing_state(session: &Session) -> Option<LandingState> {
    let last_terminal_frame = session
        .provenance
        .merge_frames
        .iter()
        .rev()
        .find(|frame| frame.status.is_terminal());
    if let Some(frame) = last_terminal_frame {
        return Some(match frame.status {
            MergeStatus::Merged => LandingState::Landed {
                revision: frame
                    .evidence
                    .first()
                    .and_then(|entry| entry.strip_prefix("landed="))
                    .map(str::to_string),
            },
            MergeStatus::Parked => LandingState::Parked,
            MergeStatus::PostMergeCleanupFailure | MergeStatus::RecoveryFailure => {
                LandingState::MergeFailed
            }
            MergeStatus::LockAcquired | MergeStatus::GatesPassed | MergeStatus::Reconciled => {
                unreachable!("last_terminal_frame is filtered to is_terminal() frames only")
            }
        });
    }
    let drive_completed = session
        .last_drive_outcome
        .as_ref()
        .is_some_and(|outcome| outcome.outcome.is_completed());
    if session.status == Status::Completed
        && drive_completed
        && session.provenance.worktree.is_some()
        && commit_receipt(&session.ledger).is_some()
    {
        return Some(LandingState::NotMerged);
    }
    None
}

#[cfg(test)]
mod landing_honesty_tests {
    use super::*;

    const FIXTURE: &str = r#"
id = "landing-honesty-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Landing Honesty Fixture"
description = "0151 unit-test fixture: one command step, so a Session is cheap to build."

[[slot]]
id = "commit-output"
schema = "schema:text"

[procedure]
description = "One command step, standing in for a commit tail."

[[procedure.sequence]]
id = "commit"
title = "Commit"
kind = "command"
output = ["slot:commit-output"]

[procedure.sequence.command]
argv = ["git", "commit", "-m", "fixture"]
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn start_session() -> Session {
        let trait_ref = fixture_trait();
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-landing-honesty",
            "run-id": "run-landing-honesty",
            "provenance": {
                "started-by": { "surface": "test", "caller": "landing-honesty" },
                "state-source": "test",
            },
        }))
        .expect("start request");
        start_run_session(
            &trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    fn commit_revision(argv: &[&str], exit_code: Option<i32>, timed_out: bool) -> SlotRevision {
        SlotRevision {
            slot_ref: Reference::parse("slot:commit-output").expect("slot ref parses"),
            value_digest: Digest::source("commit"),
            acceptance_order: 0,
            operation: None,
            submitted_payload: None,
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: Some(crate::procedure::runtime::CommandExecutionEvidence {
                argv: argv.iter().map(|part| (*part).to_string()).collect(),
                output_slot: "slot:commit-output".to_string(),
                executable_digest: None,
                exit_code,
                timed_out,
                output_tail: None,
            }),
            runtime_binding: false,
            projection: None,
            position_path: Vec::new(),
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    #[test]
    fn commit_receipt_finds_a_successful_git_commit_revision() {
        let mut session = start_session();
        session.ledger.slot_revisions.push(commit_revision(
            &["git", "commit", "-m", "msg"],
            Some(0),
            false,
        ));
        assert!(commit_receipt(&session.ledger).is_some());
    }

    #[test]
    fn commit_receipt_ignores_a_failed_or_timed_out_or_non_git_commit_revision() {
        let mut state = start_session().ledger;
        state.slot_revisions.push(commit_revision(
            &["git", "commit", "-m", "msg"],
            Some(1),
            false,
        ));
        assert!(commit_receipt(&state).is_none());

        let mut state = start_session().ledger;
        state.slot_revisions.push(commit_revision(
            &["git", "commit", "-m", "msg"],
            Some(0),
            true,
        ));
        assert!(commit_receipt(&state).is_none());

        let mut state = start_session().ledger;
        state
            .slot_revisions
            .push(commit_revision(&["git", "status"], Some(0), false));
        assert!(commit_receipt(&state).is_none());

        let mut state = start_session().ledger;
        state
            .slot_revisions
            .push(commit_revision(&["touch", "note.txt"], Some(0), false));
        assert!(commit_receipt(&state).is_none());
    }

    fn completed_worktree_session() -> Session {
        let mut session = start_session();
        session.status = Status::Completed;
        session.last_drive_outcome = Some(DriveOutcome {
            outcome: DriveOutcomeKind::Completed,
            recorded_at_epoch: 0,
            provider_credits_pause: None,
            effective_budget: None,
            token_usage: None,
            exit_code: None,
            rate_limit: None,
            budget_pause: None,
            tokens_by_model: None,
        });
        session.provenance.worktree = Some(WorktreeProvenance {
            id: "wt-fixture".to_string(),
            branch: "ctx/run/wt-fixture".to_string(),
            seed_snapshots: Vec::new(),
            path: None,
        });
        session.ledger.slot_revisions.push(commit_revision(
            &["git", "commit", "-m", "msg"],
            Some(0),
            false,
        ));
        session
    }

    #[test]
    fn landing_state_is_not_merged_for_a_completed_worktree_run_with_a_commit_receipt() {
        let session = completed_worktree_session();
        assert_eq!(landing_state(&session), Some(LandingState::NotMerged));
    }

    #[test]
    fn landing_state_is_none_for_a_clean_tree_completed_run() {
        let mut session = completed_worktree_session();
        session.ledger.slot_revisions.clear();
        assert_eq!(landing_state(&session), None);
    }

    #[test]
    fn landing_state_is_none_for_a_non_worktree_run() {
        let mut session = completed_worktree_session();
        session.provenance.worktree = None;
        assert_eq!(landing_state(&session), None);
    }

    #[test]
    fn landing_state_prefers_a_terminal_merge_frame_over_the_commit_receipt() {
        let mut session = completed_worktree_session();
        session.provenance.merge_frames.push(MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Merged,
            reason: None,
            evidence: vec!["landed=abc123".to_string()],
            park_reason: None,
            deep_decisions: Vec::new(),
        });
        assert_eq!(
            landing_state(&session),
            Some(LandingState::Landed {
                revision: Some("abc123".to_string())
            })
        );
    }
}

/// Typed detail for a [`MergeFrame`] whose `status` is [`MergeStatus::Parked`]
/// for a reason that itself carries structured evidence, beyond the free-text
/// `reason`/`evidence` every frame already has. `None` for every other park
/// cause — this is additive detail, not a replacement for `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[serde(tag = "kind")]
pub enum ParkReason {
    /// The run branch and `main` both changed the same repository-relative
    /// paths since the run branch's base — the run's review approved a diff
    /// against a base that has since moved on those exact paths, so the
    /// approval may no longer reflect what would actually land. By default the
    /// merge lands anyway (Git conflict handling and, if needed, the merger
    /// decide whether reconciliation is required) and records this detail
    /// alongside the landed evidence rather than parking; `--park-on-overlap`
    /// restores the strict prior behavior of parking here, before rebasing,
    /// with this typed detail. `--allow-stale-overlap` is a deprecated no-op
    /// kept for one release: it neither parks nor changes landing behavior.
    StaleBaseOverlap {
        paths: Vec<String>,
        main_commits: Vec<String>,
    },
    /// P479: the invocation repository mutated outside this run's worktree
    /// while `policy = "park"` was in effect. This finding is permanent by
    /// construction — nothing clears or acknowledges it — so it refuses
    /// every later `ctx traits merge <run-id>` of this same session, at
    /// `MergeStage::Preflight`, before the branch or worktree is touched.
    OutOfTreeMutation { paths: Vec<String>, frame: String },
}

/// One of the five stable P420 `--deep` reconciliation doctrines a deep
/// merger's per-hunk decision is filed under. Verbatim operational form lives
/// in the deep merger's system prompt; this is only the stable vocabulary a
/// decision receipt names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DeepMergeRule {
    /// Preserve a landed surface and port payload.
    PreserveLandedSurface,
    /// Union additive same-anchor changes from both sides.
    UnionAdditive,
    /// Unify duplicate components around the stronger primitive.
    UnifyDuplicate,
    /// Refuse a change that would revert a landed decision.
    RefuseRegression,
    /// Verify a contradictory factual claim against the code.
    VerifyFactualClaim,
}

impl DeepMergeRule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreserveLandedSurface => "preserve-landed-surface",
            Self::UnionAdditive => "union-additive",
            Self::UnifyDuplicate => "unify-duplicate",
            Self::RefuseRegression => "refuse-regression",
            Self::VerifyFactualClaim => "verify-factual-claim",
        }
    }
}

/// One typed, per-hunk `--deep` reconciliation decision: which repository-
/// relative path and supplied hunk id it resolves, which of the five stable
/// doctrines justified it, the choice actually made, why, and any supporting
/// edit made outside the conflicted path itself (permitted only when logged
/// here). Recorded on the `Reconciliation` [`MergeFrame`] and reproduced as a
/// compact `Ctx-Merge-Decision` trailer on the amended branch tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DeepMergeDecision {
    pub path: String,
    pub hunk: String,
    pub rule: DeepMergeRule,
    pub choice: String,
    pub rationale: String,
    /// Repository-relative paths edited outside the conflicted path itself,
    /// each permitted only because it is named here — an unbounded typed
    /// collection rather than a single optional field, since a supporting
    /// edit can span an arbitrary number of files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supporting_edits: Vec<String>,
}

/// One typed, byte-stable record of a `ctx traits merge` outcome at a stage.
/// Appended in order to [`Provenance::merge_frames`]; carries no timestamps,
/// shell history, or absolute paths — only stable stage/status/reason
/// vocabulary and ordered evidence strings (branch/revision, merger
/// assignment, gate results).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct MergeFrame {
    pub stage: MergeStage,
    pub status: MergeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// Structured detail for [`ParkReason`]-bearing park causes. `None` for
    /// every existing frame (this field is new and additive) and for parks
    /// that predate a typed reason, so old ledgers deserialize unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub park_reason: Option<ParkReason>,
    /// P420 `--deep` reconciliation's typed per-hunk decision log. Empty (and
    /// omitted from JSON) for every frame from a fast-forward or standard
    /// no-flag merge, and for every ledger written before P420 — old and
    /// standard ledgers stay byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deep_decisions: Vec<DeepMergeDecision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FinalOutput {
    pub port_ref: Reference,
    pub value_slot_ref: Reference,
    pub value_digest: Digest,
    pub value: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CompletionNotification {
    pub status: Status,
    pub event_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub final_outputs: Vec<FinalOutput>,
    pub final_session_digest: Digest,
}

/// Terminal outcome of the most recent drive conductor over this session,
/// stamped at the IO boundary when the conductor exits. Without it a ledger
/// left at `awaiting-agent-output` is indistinguishable from a live run.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
pub enum DriveOutcomeKind {
    Completed,
    Running,
    Rejected,
    Failed,
    AwaitingInput,
    Interrupted,
    /// P551: the live TUI's ctrl-c instant kill — distinct from `Interrupted`
    /// (a cooperative `SIGINT`/control-socket stop drained to a between-frame
    /// checkpoint) so the ledger reads as a deliberate kill of in-flight
    /// work, never a graceful stop.
    Killed,
    Blocked,
    PausedProviderCredits,
    /// P130: a declared run/seat token or estimated-cost ceiling was reached
    /// at the frame-dispatch boundary. See [`BudgetExhaustedPause`].
    PausedBudgetExhausted,
    DriverLockBusy,
    ConcurrencyConductorBusy,
    TotalBudgetExhausted,
    MaxFramesExhausted,
    NoFrame,
    BlockedAgentUnassigned,
    BlockedHarnessUnprobed,
    BlockedHarnessModelInvalid,
    UnsupportedHarnessModelSelection,
    UnsupportedHarnessCli,
    SessionMembershipConflict,
    ConcurrencyRecoveryBlocked,
    ConcurrentBranchTerminalFailure,
    HarnessFailed,
    HarnessOutputTruncated,
    HarnessOutputInvalid,
    AttachWaitExpired,
    WaitingOnHuman,
    OutOfTreeMutation,
    CommandStepFailed,
    /// A historical or harness-specific wire value. It remains readable so a
    /// newer ledger never makes an older installation's evidence opaque.
    Other(String),
}

impl DriveOutcomeKind {
    pub fn from_wire(value: impl Into<String>) -> Self {
        match value.into().as_str() {
            "completed" => Self::Completed,
            "running" => Self::Running,
            "rejected" => Self::Rejected,
            "failed" => Self::Failed,
            "awaiting-input" => Self::AwaitingInput,
            "interrupted" => Self::Interrupted,
            "killed" => Self::Killed,
            "blocked" => Self::Blocked,
            "paused-provider-credits" => Self::PausedProviderCredits,
            "paused-budget-exhausted" => Self::PausedBudgetExhausted,
            "driver-lock-busy" => Self::DriverLockBusy,
            "concurrency-conductor-busy" => Self::ConcurrencyConductorBusy,
            "total-budget-exhausted" => Self::TotalBudgetExhausted,
            "max-frames-exhausted" => Self::MaxFramesExhausted,
            "no-frame" => Self::NoFrame,
            "blocked-agent-unassigned" => Self::BlockedAgentUnassigned,
            "blocked-harness-unprobed" => Self::BlockedHarnessUnprobed,
            "blocked-harness-model-invalid" => Self::BlockedHarnessModelInvalid,
            "unsupported-harness-model-selection" => Self::UnsupportedHarnessModelSelection,
            "unsupported-harness-cli" => Self::UnsupportedHarnessCli,
            "session-membership-conflict" => Self::SessionMembershipConflict,
            "concurrency-recovery-blocked" => Self::ConcurrencyRecoveryBlocked,
            "concurrent-branch-terminal-failure" => Self::ConcurrentBranchTerminalFailure,
            "harness-failed" => Self::HarnessFailed,
            "harness-output-truncated" => Self::HarnessOutputTruncated,
            "harness-output-invalid" => Self::HarnessOutputInvalid,
            "attach-wait-expired" => Self::AttachWaitExpired,
            "waiting-on-human" => Self::WaitingOnHuman,
            "out-of-tree-mutation" => Self::OutOfTreeMutation,
            "command-step-failed" => Self::CommandStepFailed,
            value => Self::Other(value.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Running => "running",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::AwaitingInput => "awaiting-input",
            Self::Interrupted => "interrupted",
            Self::Killed => "killed",
            Self::Blocked => "blocked",
            Self::PausedProviderCredits => "paused-provider-credits",
            Self::PausedBudgetExhausted => "paused-budget-exhausted",
            Self::DriverLockBusy => "driver-lock-busy",
            Self::ConcurrencyConductorBusy => "concurrency-conductor-busy",
            Self::TotalBudgetExhausted => "total-budget-exhausted",
            Self::MaxFramesExhausted => "max-frames-exhausted",
            Self::NoFrame => "no-frame",
            Self::BlockedAgentUnassigned => "blocked-agent-unassigned",
            Self::BlockedHarnessUnprobed => "blocked-harness-unprobed",
            Self::BlockedHarnessModelInvalid => "blocked-harness-model-invalid",
            Self::UnsupportedHarnessModelSelection => "unsupported-harness-model-selection",
            Self::UnsupportedHarnessCli => "unsupported-harness-cli",
            Self::SessionMembershipConflict => "session-membership-conflict",
            Self::ConcurrencyRecoveryBlocked => "concurrency-recovery-blocked",
            Self::ConcurrentBranchTerminalFailure => "concurrent-branch-terminal-failure",
            Self::HarnessFailed => "harness-failed",
            Self::HarnessOutputTruncated => "harness-output-truncated",
            Self::HarnessOutputInvalid => "harness-output-invalid",
            Self::AttachWaitExpired => "attach-wait-expired",
            Self::WaitingOnHuman => "waiting-on-human",
            Self::OutOfTreeMutation => "out-of-tree-mutation",
            Self::CommandStepFailed => "command-step-failed",
            Self::Other(value) => value,
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

impl std::fmt::Display for DriveOutcomeKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for DriveOutcomeKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DriveOutcomeKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from_wire(String::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DriveOutcome {
    pub outcome: DriveOutcomeKind,
    pub recorded_at_epoch: u64,
    /// Present only when `outcome` is `paused-provider-credits`, so an
    /// existing ledger written before this field existed still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_credits_pause: Option<ProviderCreditsPause>,
    /// The effective drive budget actually applied to this invocation (P445):
    /// CLI overrides > package `config.toml` > built-in defaults. `None` on
    /// every existing ledger (this field is new and additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_budget: Option<DriveBudgetEvidence>,
    /// Observed output-token usage aggregated across this drive's harness
    /// attempts (P445), tracked separately for the work agent and the
    /// narrator. `None` on every existing ledger (this field is new and
    /// additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsageEvidence>,
    /// P510 liveness evidence: the driver's numeric exit disposition for this
    /// terminal outcome, stamped in `record_drive_outcome` through the
    /// existing `CompletionDisposition`-adjacent exit-code seam. `None` on
    /// every existing ledger (this field is new and additive), and also
    /// `None` for a driver that crashed before it could record an outcome at
    /// all — that absence, combined with a row present and the lock free, is
    /// itself the crashed-vs-exited signal P512 builds on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    /// Latest observed subscription rate-limit pressure per limit type
    /// (P556/0117), keyed by `RateLimitObservation::limit_type` (or
    /// `"unknown"` for the wire's typeless events). Carries evidence even on
    /// a drive that never paused — `None` when the dispatched harness emits
    /// no usage telemetry or this is an older ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<
        std::collections::BTreeMap<String, crate::procedure::activity::RateLimitObservation>,
    >,
    /// Present only when `outcome` is `paused-budget-exhausted` (0130). `None`
    /// on every existing ledger (this field is new and additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_pause: Option<BudgetExhaustedPause>,
    /// Observed output tokens attributed to the resolved model id that
    /// produced them (0130): the work seat's `AgentAssignment.model`,
    /// narrator/guide's own agent-table model, or `"unknown"` for a
    /// model-less/attach-transport seat. `None` on every existing ledger
    /// (this field is new and additive) and for a drive that observed no
    /// tokens at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_by_model: Option<std::collections::BTreeMap<String, u64>>,
}

/// The effective drive budget recorded as evidence alongside a
/// [`DriveOutcome`] (P445). Every knob but `idle_seconds` always has a
/// resolved value by the time a drive stamps this evidence (CLI overrides >
/// package `config.toml` > built-in defaults always settle on a concrete
/// ceiling), so those five fields are required — only `idle_seconds` is
/// optional, since that knob has no built-in default and can genuinely be
/// unset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct DriveBudgetEvidence {
    pub max_frames: u64,
    pub frame_seconds: u64,
    pub total_seconds: u64,
    pub max_retries: u64,
    pub attach_wait_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_seconds: Option<u64>,
}

/// Observed output-token usage evidence for one drive (P445). This is an
/// observational liveness signal (the same stream-shape-aware counter the
/// live TUI already uses), never a billing-grade provider usage total. Since
/// 0130 the ledger's own deterministic observations MAY gate dispatch when a
/// `[budget] max-tokens`/`max-cost-usd` ceiling is declared (see
/// [`BudgetExhaustedPause`]) — enforcement never reads a provider billing
/// API, only this same counted evidence. Each total is `None` when nothing
/// was observed (an unsupported harness, or narration never invoked) rather
/// than a fabricated zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct TokenUsageEvidence {
    /// Output tokens observed across the work agent's harness attempts
    /// (cold, warm, retries, and concurrent-wave dispatch alike).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_tokens: Option<u64>,
    /// Output tokens observed across narrator calls, tracked separately
    /// since narration is async garnish and never gates or joins the drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrator_tokens: Option<u64>,
    /// Whether every narrator call started during this drive had finished
    /// (and folded its usage in above) by the time this evidence was
    /// stamped. `None` when no narrator ran this drive. `Some(false)` means a
    /// narrator call was still in flight and its usage may be missing from
    /// `narrator_tokens` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narration_complete: Option<bool>,
    /// Output tokens observed across ephemeral live-guide calls. Guide text is
    /// never persisted; this aggregate is terminal accounting only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_tokens: Option<u64>,
    /// Whether every guide request had settled when terminal evidence was
    /// stamped. `None` when no guide request was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_complete: Option<bool>,
}

/// The two P445 evidence values a terminal [`DriveOutcome`] carries, bundled
/// into a single argument for `record_drive_outcome` so a caller supplies
/// exactly one evidence value rather than a growing list of independent
/// optional parameters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DriveTerminalEvidence {
    pub effective_budget: Option<DriveBudgetEvidence>,
    pub token_usage: Option<TokenUsageEvidence>,
    /// P510 liveness evidence, see [`DriveOutcome::exit_code`].
    pub exit_code: Option<u8>,
    /// See [`DriveOutcome::rate_limit`] (P556/0117).
    pub rate_limit: Option<
        std::collections::BTreeMap<String, crate::procedure::activity::RateLimitObservation>,
    >,
    /// See [`DriveOutcome::tokens_by_model`] (0130).
    pub tokens_by_model: Option<std::collections::BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Session {
    pub schema_version: String,
    pub session_id: SessionId,
    pub run_id: Id,
    pub trait_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,
    pub current_run_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_source_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_sequence_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_sequence_title: Option<String>,
    #[serde(
        default,
        rename = "current-agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_agent: Option<AgentRole>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_port_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_slot_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_output_port_values: Vec<Value>,
    #[serde(
        default,
        rename = "slot-revisions",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub slot_revisions: Vec<SlotRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitted_signals: Vec<SignalEmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_submissions: Vec<RejectedAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_evidence: Vec<ResourceEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_capability_reports: Vec<CapabilityReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_ports: Vec<crate::procedure::runtime::OutputPortCompletion>,
    #[serde(
        default,
        rename = "resolved-settings",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub resolved_settings: Vec<crate::procedure::runtime::ResolvedSettingRecord>,
    #[serde(default, rename = "active-path", skip_serializing_if = "Vec::is_empty")]
    pub active_path: Vec<PathSegment>,
    #[serde(
        default,
        rename = "control-stack",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub control_stack: Vec<ControlFrame>,
    #[serde(
        default,
        rename = "stop-reason",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub final_output_summary: Vec<FinalOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_frame: Option<Box<SequenceFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_validation_report: Option<StepValidationReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CompletionNotification>,
    /// Cleared on every transition rebuild, so a present marker always
    /// describes a conductor exit that nothing has advanced past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drive_outcome: Option<DriveOutcome>,
    pub provenance: Provenance,
    pub ledger: State,
    pub state_digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SignalSubmission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CallSubmission {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sequence_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_run_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_source_index: Option<usize>,
    #[serde(
        default,
        rename = "expected-position-path",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub expected_position_path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub produced_slots: BTreeMap<String, JsonValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, SignalSubmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub command_execution: Option<CommandExecutionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<CallerProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CommandExecutionEvidence {
    pub argv: Vec<String>,
    pub output_slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Bounded captured stdout, when the executing adapter captured it.
    /// Deterministic execution evidence only; never the accepted value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Bounded captured stderr, when the executing adapter captured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, rename = "stdout-truncated")]
    pub stdout_truncated: bool,
    #[serde(default, rename = "stderr-truncated")]
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CallResponse {
    pub schema_version: String,
    pub session_id: SessionId,
    pub run_id: Id,
    pub status: Status,
    pub response_kind: CallResponseKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_slot_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_slot_values: Vec<RejectedAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_signals: Vec<SignalEmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_signals: Vec<SignalEmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_validation: Vec<crate::procedure::runtime::SchemaValidation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexpected_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_required_outputs: Vec<String>,
    /// Declared optional output sinks (P105) left unfilled this call. A
    /// signed non-failure — never contributes to `correction` or rejection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unfilled_optional_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction: Option<String>,
    pub updated_session_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_frame: Option<Box<SequenceFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CompletionNotification>,
    pub session: Session,
    #[serde(
        default = "default_persist_session",
        skip_serializing,
        skip_deserializing
    )]
    #[schemars(skip)]
    pub persist_session: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetResolution {
    InitialPortValue(Box<StepSlotOutput>),
    CurrentFrameCall(Box<CallSubmission>),
}

struct RunCallPreflight {
    rejection: Option<StepValidationReport>,
    trusted_command_execution: bool,
    trusted_check_execution: bool,
    non_persisting_rejection: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StartRequest {
    pub session_id: SessionId,
    pub run_id: Id,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_port_values: Vec<StepSlotOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_evidence: Vec<ResourceEvidence>,
    /// 0172: activation-resolved `setting:` values, recorded to the run
    /// ledger as evidence. Empty for every caller that hasn't opted in
    /// (adapters with no config-layer access, e.g. `preview`) — the run
    /// still starts, just with no resolved-settings evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_settings: Vec<crate::procedure::runtime::ResolvedSettingRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_capability_reports: Vec<CapabilityReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,
    #[serde(
        default,
        rename = "agent-assignments",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_assignments: Option<Vec<AgentAssignment>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness_probes: Vec<HarnessProbeEvidence>,
    /// When true, every loop stops the run blocked at exhaustion regardless of
    /// its own `on-exhausted` policy — the caller's strictness override.
    #[serde(
        default,
        rename = "strict-loops",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub strict_loops: bool,
    pub provenance: Provenance,
}

fn default_persist_session() -> bool {
    true
}

/// `status`/`trust` are caller-resolved from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn start_run_session(
    trait_ref: &Trait,
    status: &crate::manifest::PackageStatus,
    trust: &crate::r#trait::TrustVerdict,
    request: StartRequest,
) -> crate::Result<Session> {
    let gates = crate::r#trait::activation::lifecycle_trust_gates_for_check(
        trait_ref.id.as_str(),
        status,
        trust,
    );
    if !gates.is_empty() {
        return Err(crate::procedure::invalid_field(
            "run-session.lifecycle-trust",
            format!(
                "executable run blocked by lifecycle/trust gates: {}",
                crate::r#trait::activation::format_gate_refusal(&gates)
            ),
        ));
    }
    // Synthetic core-only transitions may intentionally omit a canonical
    // source pin. Every executable adapter supplies one; only those starts
    // can claim a machine-trust approval.
    if matches!(trust, crate::r#trait::TrustVerdict::Verified) && request.canonical_digest.is_some()
    {
        let Some(approval) = request.provenance.trust_approval.as_ref() else {
            return Err(crate::procedure::invalid_field(
                "run-session.trust-approval",
                "verified executable starts require durable approval evidence",
            ));
        };
        if approval.trait_id != trait_ref.id.as_str()
            || approval.seq == 0
            || request
                .canonical_digest
                .as_ref()
                .is_none_or(|digest| digest != &approval.canonical_digest)
        {
            return Err(crate::procedure::invalid_field(
                "run-session.trust-approval",
                "verified executable start approval evidence does not match the canonical trait bytes",
            ));
        }
    }
    let mut provenance = request.provenance;
    provenance.agent_assignments = validate_run_agent_assignments(
        trait_ref,
        request.agent_assignments.or(provenance.agent_assignments),
    )?;
    provenance.harness_probes.extend(request.harness_probes);
    provenance.warnings.extend(request.provider_warnings);
    let mut state = start_procedure_run(
        trait_ref,
        request.run_id,
        request.initial_port_values,
        request.resource_evidence,
        request.provider_capability_reports,
        request.source_digest,
        request.canonical_digest,
        request.resolved_settings,
    )?;
    state.strict_loops = request.strict_loops;
    build_session(trait_ref, request.session_id, state, None, provenance)
}

fn validate_run_agent_assignments(
    trait_ref: &Trait,
    assignments: Option<Vec<AgentAssignment>>,
) -> crate::Result<Option<Vec<AgentAssignment>>> {
    let Some(assignments) = assignments else {
        return Ok(None);
    };
    // A role with no seat evidence (legacy single-table) allows exactly one
    // row, same as before P456. A role where every row carries `seat_index`
    // (P456 list-backed) allows one row per seat, 1..=`list_length`, all
    // rows agreeing on `list_length` — never a mix of the two shapes for one
    // role, and never two rows claiming the same seat.
    let mut seen_legacy = BTreeSet::new();
    let mut seen_seats: std::collections::BTreeMap<String, (u32, BTreeSet<u32>)> =
        std::collections::BTreeMap::new();
    let mut normalized = Vec::new();
    for (index, mut assignment) in assignments.into_iter().enumerate() {
        let role = normalize_agent_role(&assignment.role, index)?;
        if !trait_ref.agents.iter().any(|agent| agent.id == role) {
            return Err(crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].role"),
                format!(
                    "assigned role {:?} is not declared as [[agent]]",
                    assignment.role
                ),
            ));
        }
        match (assignment.seat_index, assignment.list_length) {
            (None, None) => {
                if !seen_legacy.insert(role.clone()) || seen_seats.contains_key(&role) {
                    return Err(crate::procedure::invalid_field(
                        format!("run-session.agent-assignments[{index}].role"),
                        format!("duplicate assignment for agent role {role:?}"),
                    ));
                }
            }
            (Some(seat_index), Some(list_length)) => {
                if seat_index == 0 || seat_index > list_length {
                    return Err(crate::procedure::invalid_field(
                        format!("run-session.agent-assignments[{index}].seat-index"),
                        format!(
                            "seat {seat_index} is out of range for role {role:?} (list-length {list_length})"
                        ),
                    ));
                }
                if seen_legacy.contains(&role) {
                    return Err(crate::procedure::invalid_field(
                        format!("run-session.agent-assignments[{index}].role"),
                        format!("duplicate assignment for agent role {role:?}"),
                    ));
                }
                let entry = seen_seats
                    .entry(role.clone())
                    .or_insert_with(|| (list_length, BTreeSet::new()));
                if entry.0 != list_length {
                    return Err(crate::procedure::invalid_field(
                        format!("run-session.agent-assignments[{index}].list-length"),
                        format!("role {role:?} assignments disagree on list-length"),
                    ));
                }
                if !entry.1.insert(seat_index) {
                    return Err(crate::procedure::invalid_field(
                        format!("run-session.agent-assignments[{index}].seat-index"),
                        format!("duplicate assignment for role {role:?} seat {seat_index}"),
                    ));
                }
            }
            _ => {
                return Err(crate::procedure::invalid_field(
                    format!("run-session.agent-assignments[{index}].seat-index"),
                    "seat-index and list-length must be present together or both absent",
                ));
            }
        }
        if assignment.harness.trim().is_empty() {
            return Err(crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].harness"),
                "assignment harness must not be empty",
            ));
        }
        if assignment.transport.trim().is_empty() {
            return Err(crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].transport"),
                "assignment transport must not be empty",
            ));
        }
        if assignment.evidence.trim().is_empty() {
            return Err(crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].evidence"),
                "assignment evidence must not be empty",
            ));
        }
        assignment.role = role;
        normalized.push(assignment);
    }
    for (role, (list_length, seats)) in &seen_seats {
        if seats.len() as u32 != *list_length {
            return Err(crate::procedure::invalid_field(
                "run-session.agent-assignments",
                format!(
                    "role {role:?} declares list-length {list_length} but only {} seat(s) were assigned",
                    seats.len()
                ),
            ));
        }
    }
    normalized.sort_by(|a, b| (&a.role, a.seat_index).cmp(&(&b.role, b.seat_index)));
    Ok(Some(normalized))
}

fn normalize_agent_role(role: &str, index: usize) -> crate::Result<String> {
    if role.trim().is_empty() {
        return Err(crate::procedure::invalid_field(
            format!("run-session.agent-assignments[{index}].role"),
            "assignment role must not be empty",
        ));
    }
    if role.contains(':') {
        let parsed = Reference::parse(role).map_err(|_| {
            crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].role"),
                format!("invalid assignment role ref {role:?}"),
            )
        })?;
        if parsed.kind() != Kind::Agent || parsed.is_qualified() {
            return Err(crate::procedure::invalid_field(
                format!("run-session.agent-assignments[{index}].role"),
                "assignment role refs must be local agent:* refs",
            ));
        }
        return Ok(parsed.id().to_string());
    }
    Ok(role.to_string())
}

pub fn run_initial_values_from_json(value: JsonValue) -> crate::Result<Vec<StepSlotOutput>> {
    if let Some(values) = value.get("values") {
        return serde_json::from_value(values.clone()).map_err(|e| {
            crate::procedure::invalid_field(
                "run-session.inputs.values",
                format!("failed to parse runtime input values array: {e}"),
            )
        });
    }
    let Some(object) = value.as_object() else {
        return Err(crate::procedure::invalid_field(
            "run-session.inputs",
            "runtime input JSON must be an object or {\"values\": [...] }",
        ));
    };
    let mut values = Vec::new();
    for (ref_text, value) in object {
        values.push(StepSlotOutput {
            ref_text: ref_text.clone(),
            value: value.clone(),
            source: Some(ValueSource::HostInput),
            producer_evidence: Some("run-session initial input".to_string()),
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        });
    }
    values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    Ok(values)
}

pub fn resolve_run_set_submission(
    trait_ref: &Trait,
    session: &Session,
    target: &str,
    value: JsonValue,
    caller: CallerProvenance,
) -> crate::Result<SetResolution> {
    require_worktree_provenance(trait_ref, session)?;
    let caller_agent =
        caller_agent_role(trait_ref, caller.agent.as_deref())?.map(|role| format!("agent:{role}"));
    let caller_harness = caller.harness.clone();
    if session.next_frame.is_none() {
        let port_ref = resolve_initial_input_target(session, target)?;
        return Ok(SetResolution::InitialPortValue(Box::new(StepSlotOutput {
            ref_text: port_ref,
            value,
            source: Some(ValueSource::HostInput),
            producer_evidence: Some(format!("{}:{} set", caller.surface, caller.caller)),
            command_execution: None,
            producer_agent: caller_agent,
            producer_harness: caller_harness,
        })));
    }

    let frame = session.next_frame.as_deref().ok_or_else(|| {
        crate::procedure::invalid_field(
            "run-session.current-frame",
            "run session has no current frame to accept set output",
        )
    })?;
    let slot_ref = resolve_current_frame_target(trait_ref, frame, target)?;
    let mut produced_slots = BTreeMap::new();
    produced_slots.insert(slot_ref, value);
    Ok(SetResolution::CurrentFrameCall(Box::new(CallSubmission {
        session_id: session.session_id.clone(),
        run_id: Some(session.run_id.clone()),
        state_digest: Some(session.state_digest.clone()),
        expected_sequence_item_id: session.current_sequence_item_id.clone(),
        expected_run_index: Some(session.current_run_index),
        expected_source_index: session.current_source_index,
        expected_position_path: frame.position_path.clone(),
        produced_slots,
        signals: BTreeMap::new(),
        warnings: Vec::new(),
        command_execution: None,
        caller: Some(caller),
    })))
}

pub fn run_session_capability_reports(
    trait_file_read: bool,
    run_session_persistence: bool,
    call_payload: bool,
    declared_resource_evidence: bool,
    bare_session_id_store: bool,
    command_execution: bool,
    external_mcp_tool_provider: bool,
) -> Vec<CapabilityReport> {
    let mut reports = vec![
        capability(
            "runtime.trait-file-read",
            trait_file_read,
            "this surface does not read trait files; callers must supply loaded trait source",
        ),
        capability(
            "runtime.run-session-persistence",
            run_session_persistence,
            "run sessions are persisted only when an explicit ledger path is supplied",
        ),
        CapabilityReport::supported("runtime.frame-return"),
        capability(
            "runtime.call-payload",
            call_payload,
            "this surface does not accept run-call payloads",
        ),
        capability(
            "runtime.declared-resource-evidence",
            declared_resource_evidence,
            "declared resource evidence was not supplied or accepted for this run surface",
        ),
        CapabilityReport::unsupported(
            "runtime.provider-call",
            "P89/P90 run-session surfaces do not call providers or models",
        ),
        capability(
            "runtime.bare-session-id-store",
            bare_session_id_store,
            "bare run-session IDs require a configured session store; use an explicit ledger path",
        ),
        capability(
            "runtime.command-execution",
            command_execution,
            "this surface does not execute approved command steps; command frames remain blocked for permission/runtime handling",
        ),
        capability(
            "runtime.external-mcp-tool-provider",
            external_mcp_tool_provider,
            "external MCP tools are not used as runtime providers in this surface",
        ),
    ];
    reports.sort();
    reports.dedup();
    reports
}

pub fn declared_resource_evidence_supported(evidence: &[ResourceEvidence]) -> bool {
    !evidence.is_empty()
        && evidence
            .iter()
            .all(|resource| resource.available && resource.digest.is_some())
}

fn resolve_initial_input_target(session: &Session, target: &str) -> crate::Result<String> {
    let candidates = target_variants("port", target);
    let unresolved: Vec<&String> = session
        .unresolved_inputs
        .iter()
        .filter(|input| candidates.iter().any(|candidate| candidate == *input))
        .collect();
    match unresolved.as_slice() {
        [only] => Ok((*only).clone()),
        [] => Err(crate::procedure::invalid_field(
            "run-session.set.target",
            format!(
                "target {target:?} is not a currently requested input; requested inputs: {}",
                session.unresolved_inputs.join(", ")
            ),
        )),
        _ => Err(crate::procedure::invalid_field(
            "run-session.set.target",
            format!("target {target:?} is ambiguous for current input frame"),
        )),
    }
}

fn resolve_current_frame_target(
    trait_ref: &Trait,
    frame: &SequenceFrame,
    target: &str,
) -> crate::Result<String> {
    let mut candidates = Vec::new();
    let requested: Vec<String> = frame
        .requested_outputs
        .iter()
        .map(|output| output.slot_ref.to_string())
        .collect();

    if frame.item_id.as_deref() == Some(target) && requested.len() == 1 {
        candidates.push(requested[0].clone());
    }

    for candidate in target_variants("slot", target) {
        if requested.iter().any(|slot_ref| slot_ref == &candidate) {
            candidates.push(candidate);
        }
    }

    for candidate in target_variants("port", target) {
        if let Ok(parsed) = Reference::parse(&candidate)
            && parsed.kind() == Kind::Port
            && !parsed.is_qualified()
            && let Some(port) = trait_ref.ports.iter().find(|port| {
                port.id == parsed.id() && matches!(port.direction, PortDirection::Output)
            })
            && let Some(value_slot) = port.value.as_ref()
            && requested.iter().any(|slot_ref| slot_ref == value_slot)
        {
            candidates.push(value_slot.clone());
        }
    }

    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(crate::procedure::invalid_field(
            "run-session.set.target",
            format!(
                "target {target:?} is not current; current item: {}, requested outputs: {}",
                frame.item_id.as_deref().unwrap_or("-"),
                requested.join(", ")
            ),
        )),
        _ => Err(crate::procedure::invalid_field(
            "run-session.set.target",
            format!("target {target:?} is ambiguous for current frame"),
        )),
    }
}

fn target_variants(kind: &str, target: &str) -> Vec<String> {
    if target.contains(':') {
        vec![target.to_string()]
    } else {
        vec![format!("{kind}:{target}")]
    }
}

fn capability(name: &str, supported: bool, unsupported_reason: &str) -> CapabilityReport {
    if supported {
        CapabilityReport::supported(name)
    } else {
        CapabilityReport::unsupported(name, unsupported_reason)
    }
}

/// Merge freshly-measured cumulative active-drive elapsed seconds into a
/// session's ledger before any transition that might evaluate an
/// `elapsed-seconds-at-least` guard. The core runtime never reads a clock —
/// the IO layer (CLI drive) owns measurement and calls this once per
/// resume/status/advance/call before handing the session to
/// `refresh_run_session`/`advance_command_frames`/`submit_run_call`.
/// Monotonic: a lower or absent observation never rewinds the ledger's value.
pub fn observe_elapsed_seconds(mut session: Session, elapsed_seconds: Option<u64>) -> Session {
    if let Some(elapsed_seconds) = elapsed_seconds {
        session.ledger.elapsed_seconds = session.ledger.elapsed_seconds.max(elapsed_seconds);
    }
    session
}

pub fn refresh_run_session(trait_ref: &Trait, session: Session) -> crate::Result<Session> {
    build_session(
        trait_ref,
        session.session_id,
        session.ledger,
        session.last_validation_report,
        session.provenance,
    )
}

/// Read-only lookahead for an active `parallel` panel (P344's opt-in
/// `--max-in-flight` concurrency, CLI-driven only): given a session whose
/// live cursor is currently inside a `parallel` control frame, compute what
/// the FIRST frame of a *different* authored branch (`branch_offset` into
/// that frame's `parallel_branch_sequence_ids`) would look like if the
/// cursor were positioned there instead — without mutating `session` or
/// advancing anything for real.
///
/// This never touches the real cursor or ledger: it clones the in-memory
/// runtime state, retargets only the clone's top `parallel` control frame at
/// the requested branch (fresh isolation buffer, first step), and asks the
/// same [`next_sequence_frame`] the real cursor uses what that hypothetical
/// position's frame is. Safe because `parallel` branches are isolated from
/// each other by construction — a branch's own first frame never depends on
/// a sibling branch's effects, only on state already committed *before* the
/// panel was entered, which every branch shares — so retargeting the clone
/// to branch `branch_offset` before any branch has committed anything
/// reproduces exactly what that branch's real first frame will be once the
/// authoritative single cursor actually reaches it.
///
/// Callers MUST still submit every branch's outcome through the normal
/// sequential `submit_run_call`/`call` path, strictly in authored order —
/// this only lets a caller *resolve and dispatch* another branch's work
/// ahead of time (e.g. to run its harness call concurrently with the
/// current branch's), never to accept it out of order. Returns `Ok(None)`
/// when the session's live cursor is not currently inside a `parallel`
/// frame, or `branch_offset` is out of range for its branch list.
/// Returns the peeked frame together with the exact hypothetical ledger
/// `State` it was bound against (P402): a concurrent-wave caller must build
/// that unit's prompt from THIS bound state, not the live parent session —
/// the branch's own input digests were computed against it, and the live
/// session generally does not (yet) carry this branch's accepted values.
pub fn peek_parallel_branch_frame(
    trait_ref: &Trait,
    session: &Session,
    branch_offset: usize,
) -> crate::Result<Option<(SequenceFrame, State)>> {
    let mut state = session.ledger.clone();
    let Some(top) = state.control_stack.last_mut() else {
        return Ok(None);
    };
    if top.kind != ControlKind::Parallel {
        return Ok(None);
    }
    let Some(branch_sequence_id) = top.parallel_branch_sequence_ids.get(branch_offset).cloned()
    else {
        return Ok(None);
    };
    top.sequence_id = branch_sequence_id;
    top.next_index = 0;
    top.iteration_index = Some(branch_offset);
    top.parallel_buffer = EffectBuffer::default();
    match next_sequence_frame(trait_ref, &state)? {
        NextSequenceFrameResult::Frame(frame) => Ok(Some((*frame, state))),
        NextSequenceFrameResult::Blocked { .. }
        | NextSequenceFrameResult::Completed
        | NextSequenceFrameResult::Rejected
        | NextSequenceFrameResult::Failed => Ok(None),
    }
}

/// P402's read-only lookahead for a later `for-each` item, mirroring
/// [`peek_parallel_branch_frame`] above but for an authored `concurrent =
/// true` `for-each` rather than a `parallel` panel branch. Only ever
/// meaningful when the live cursor's top control frame is such a `for-each`;
/// a non-concurrent `for-each` peek is refused (`Ok(None)`) since its items
/// were never declared independent and the CLI drive loop must not
/// speculate on them.
///
/// Retargets the clone's frame at `item_index`, binds that item's value into
/// the clone's `item_slot` through the same [`bind_current_for_each_item`]
/// the real cursor uses, and asks [`next_sequence_frame`] what that
/// hypothetical position's frame is. Binding can only observe state already
/// committed *before* the `for-each` was entered (the bound-over list plus
/// anything the trait author placed ahead of it) — it never reads another
/// item's in-progress effects, so this is sound to call for any `item_index`
/// still within `item_total` regardless of which item the real cursor is on.
///
/// Returns `Ok(None)` when the cursor is not inside a concurrent `for-each`,
/// `item_index` is out of range, or binding/resolving that item's frame
/// rejects or blocks — the caller (`ctx-cli`'s drive loop) treats that as
/// "this item cannot be spoken for ahead of time" and abandons the wave
/// rather than treating it as a hard error.
/// Returns the peeked frame together with the exact hypothetical ledger
/// `State` it was bound against (P402), including the freshly-bound item
/// value in `accepted_slot_values` — a concurrent-wave caller must resolve
/// that item's prompt from THIS bound state, never the live parent session:
/// the live session's `item_slot` reflects whichever item the real cursor is
/// currently on, not this peeked `item_index`, so building the prompt
/// against the live session would look up the wrong (or a missing) item
/// value.
pub fn peek_for_each_item_frame(
    trait_ref: &Trait,
    session: &Session,
    item_index: usize,
) -> crate::Result<Option<(SequenceFrame, State)>> {
    let mut state = session.ledger.clone();
    let Some(top) = state.control_stack.last_mut() else {
        return Ok(None);
    };
    if top.kind != ControlKind::ForEach || !top.concurrent {
        return Ok(None);
    }
    let item_total = top.item_total.unwrap_or(0);
    if item_index >= item_total {
        return Ok(None);
    }
    top.item_index = Some(item_index);
    top.next_index = 0;
    bind_current_for_each_item(trait_ref, &mut state)?;
    match next_sequence_frame(trait_ref, &state)? {
        NextSequenceFrameResult::Frame(frame) => Ok(Some((*frame, state))),
        NextSequenceFrameResult::Blocked { .. }
        | NextSequenceFrameResult::Completed
        | NextSequenceFrameResult::Rejected
        | NextSequenceFrameResult::Failed => Ok(None),
    }
}

pub fn submit_run_call(
    trait_ref: &Trait,
    session: Session,
    submission: CallSubmission,
) -> crate::Result<CallResponse> {
    submit_run_submission(trait_ref, session, submission, false)
}

fn submit_run_submission(
    trait_ref: &Trait,
    session: Session,
    submission: CallSubmission,
    current_frame_set: bool,
) -> crate::Result<CallResponse> {
    require_worktree_provenance(trait_ref, &session)?;
    let preflight = preflight_call_rejection(trait_ref, &session, &submission, current_frame_set)?;
    if let Some(report) = preflight.rejection {
        if preflight.non_persisting_rejection {
            let next = reject_without_persisting(session, report);
            let mut response = call_response(next, CallResponseKind::RejectedCorrectionRequired);
            response.persist_session = false;
            return Ok(response);
        }
        let next = reject_without_advancing(trait_ref, session, report)?;
        return Ok(call_response(
            next,
            CallResponseKind::RejectedCorrectionRequired,
        ));
    }

    let command_execution = submission.command_execution.clone();
    let ledger_command_execution = command_execution.as_ref().map(|evidence| {
        crate::procedure::runtime::CommandExecutionEvidence {
            argv: evidence.argv.clone(),
            output_slot: evidence.output_slot.clone(),
            executable_digest: evidence.executable_digest.clone(),
            exit_code: evidence.exit_code,
            timed_out: evidence.timed_out,
            // Computed here, at the one place submission evidence becomes
            // ledger evidence, so the submitting runtime and a later replay
            // read a tail produced by the same function from the same bytes.
            output_tail: failure_tail(evidence),
        }
    });
    let producer_agent = caller_agent_role(
        trait_ref,
        submission
            .caller
            .as_ref()
            .and_then(|caller| caller.agent.as_deref()),
    )?
    .map(|role| format!("agent:{role}"));
    let producer_harness = submission
        .caller
        .as_ref()
        .and_then(|caller| caller.harness.clone());
    let slot_source = if preflight.trusted_command_execution {
        ValueSource::CommandOutput
    } else if producer_agent.is_some() || producer_harness.is_some() {
        ValueSource::ModelOutput
    } else {
        ValueSource::ManualOutput
    };
    let producer_evidence = if preflight.trusted_command_execution {
        let command = command_execution
            .as_ref()
            .expect("trusted command execution requires command evidence");
        let mut evidence = format!(
            "command execution argv={} exit={:?} timed-out={}",
            command.argv.join(" "),
            command.exit_code,
            command.timed_out
        );
        // Bounded stdout/stderr are only appended for checks: a repair step
        // downstream needs them to diagnose a failing verdict, but attaching
        // them to every ordinary command would change producer evidence — and
        // therefore FrameInput bytes — for ledgers that predate P274.
        if preflight.trusted_check_execution {
            if let Some(stdout) = command.stdout.as_deref() {
                evidence.push_str(&format!(
                    " stdout={stdout:?} stdout-truncated={}",
                    command.stdout_truncated
                ));
            }
            if let Some(stderr) = command.stderr.as_deref() {
                evidence.push_str(&format!(
                    " stderr={stderr:?} stderr-truncated={}",
                    command.stderr_truncated
                ));
            }
        }
        evidence
    } else {
        submission
            .caller
            .as_ref()
            .map(|caller| format!("{}:{}", caller.surface, caller.caller))
            .unwrap_or_else(|| "ctx traits call".to_string())
    };
    let envelope = StepOutputEnvelope {
        sequence_index: submission.expected_source_index,
        item_id: submission.expected_sequence_item_id,
        produced_slots: submission
            .produced_slots
            .into_iter()
            .map(|(ref_text, value)| StepSlotOutput {
                ref_text,
                value,
                source: Some(slot_source.clone()),
                producer_evidence: Some(producer_evidence.clone()),
                command_execution: ledger_command_execution.clone(),
                producer_agent: producer_agent.clone(),
                producer_harness: producer_harness.clone(),
            })
            .collect(),
        signals: submission
            .signals
            .into_iter()
            .map(|(ref_text, signal)| StepSignalOutput {
                ref_text,
                evidence: signal.evidence,
                producer_agent: producer_agent.clone(),
                producer_harness: producer_harness.clone(),
            })
            .collect(),
        warnings: submission.warnings,
    };

    let (candidate_state, mut report) =
        apply_step_output(trait_ref, session.ledger.clone(), envelope)?;
    let non_proven_schema: Vec<RejectedAttempt> = report
        .accepted_outputs
        .iter()
        .filter(|value| {
            value
                .schema_validation
                .iter()
                .any(|validation| validation.status != SchemaStatus::Accepted)
        })
        .map(|value| RejectedAttempt {
            sequence_index: report.sequence_index,
            position_path: Vec::new(),
            ref_text: Some(value.ref_text.clone()),
            value_digest: Some(value.value_digest.clone()),
            reason: "schema validation requires external evidence or is unsupported".to_string(),
        })
        .collect();

    if !non_proven_schema.is_empty() {
        report.rejected_outputs.extend(non_proven_schema);
        report.accepted_outputs.clear();
        report.next_action = StepNextAction::Rejected;
    }

    if report.next_action == StepNextAction::Rejected || !report.rejected_outputs.is_empty() {
        // A correction-required response must never claim accepted values:
        // the candidate that produced them is discarded below in favor of
        // the pre-call `session.ledger`, so any entries left in
        // `accepted_outputs` (e.g. a mixed valid/invalid envelope, where only
        // the invalid slot(s) triggered rejection) would otherwise leak into
        // `call_response`'s `accepted_slot_values` despite never having been
        // committed.
        report.accepted_outputs.clear();
        let next = reject_without_advancing(trait_ref, session, report)?;
        return Ok(call_response(
            next,
            CallResponseKind::RejectedCorrectionRequired,
        ));
    }

    let next = build_session(
        trait_ref,
        session.session_id,
        candidate_state,
        Some(report),
        session.provenance,
    )?;
    let kind = match next.status {
        Status::Completed => CallResponseKind::AcceptedCompleted,
        Status::Blocked | Status::BlockedAgentUnassigned => CallResponseKind::BlockedMissingInput,
        Status::Failed => CallResponseKind::Failed,
        _ => CallResponseKind::AcceptedNextFrame,
    };
    Ok(call_response(next, kind))
}

/// Current-frame `set` is the human submission boundary. It intentionally
/// shares call validation and persistence semantics while refusing a caller
/// that carries agent or harness attribution on an ask frame.
pub fn submit_current_frame_set(
    trait_ref: &Trait,
    session: Session,
    submission: CallSubmission,
) -> crate::Result<CallResponse> {
    submit_run_submission(trait_ref, session, submission, true)
}

/// P402: submit a terminal dispatch-level failure for the session's current
/// frame — a concurrent-wave branch/item whose harness call timed out,
/// exited nonzero, panicked, or exhausted retries without ever producing a
/// submittable output. Routes through [`apply_terminal_frame_failure`],
/// which reuses [`reject_step_output`]'s existing nested-recovery / P264
/// `skip`/`park`/`panel-fail` policy — the SAME transition a rejected
/// submission's content would already trigger — so this is the single
/// shared entrypoint for both a live in-wave dispatch and a recovered/cached
/// concurrent outcome; CLI must never apply that policy itself.
///
/// Returns the session unchanged (wrapped as `AcceptedNextFrame`, matching
/// `next_frame` still describing the SAME position) when there is no
/// current ready item to fail — e.g. the run already moved on — rather than
/// silently claiming to have routed a failure that had nothing to attach to.
pub fn submit_terminal_frame_failure(
    trait_ref: &Trait,
    session: Session,
    reason: &str,
) -> crate::Result<CallResponse> {
    require_worktree_provenance(trait_ref, &session)?;
    let Some((candidate_state, report)) =
        apply_terminal_frame_failure(trait_ref, session.ledger.clone(), reason)?
    else {
        return Ok(call_response(session, CallResponseKind::AcceptedNextFrame));
    };
    let next = build_session(
        trait_ref,
        session.session_id,
        candidate_state,
        Some(report),
        session.provenance,
    )?;
    let kind = match next.status {
        Status::Completed => CallResponseKind::AcceptedCompleted,
        Status::Blocked | Status::BlockedAgentUnassigned => CallResponseKind::BlockedMissingInput,
        Status::Failed => CallResponseKind::Failed,
        _ => CallResponseKind::AcceptedNextFrame,
    };
    Ok(call_response(next, kind))
}

pub fn run_session_digest(session: &Session) -> crate::Result<Digest> {
    ledger_digest(&session.ledger)
}

fn build_session(
    trait_ref: &Trait,
    session_id: SessionId,
    state: State,
    last_validation_report: Option<StepValidationReport>,
    provenance: Provenance,
) -> crate::Result<Session> {
    let validation = validate_run_ledger_contract(trait_ref, &state)?;
    if !validation.contract_valid {
        return Err(crate::procedure::invalid_field(
            "run-session.ledger",
            format!(
                "run ledger contract invalid: {}",
                validation.diagnostics.join("; ")
            ),
        ));
    }
    let worktree_provenance_missing = worktree_provenance_missing(trait_ref, &provenance);
    let frame_result = if worktree_provenance_missing {
        NextSequenceFrameResult::Blocked {
            missing_inputs: Vec::new(),
            capabilities: vec![CapabilityReport::unsupported(
                "runtime.worktree-provenance",
                "procedure requires prepared worktree provenance; start it with --worktree",
            )],
        }
    } else {
        next_sequence_frame(trait_ref, &state)?
    };
    let mut unresolved_inputs = Vec::new();
    let mut capabilities = state.provider_capability_reports.clone();
    let mut next_frame = None;
    let mut warnings = provenance.warnings.clone();
    if !trait_ref.agents.is_empty() && provenance.agent_assignments.is_none() {
        warnings.push(
            "trait declares agent roles but no run agent assignments were supplied; running in single-agent compatibility mode".to_string(),
        );
    }
    let status = match frame_result {
        NextSequenceFrameResult::Frame(mut frame) => {
            let state_digest = ledger_digest(&state)?;
            attach_call_template(&session_id, &state, &state_digest, frame.as_mut());
            let is_command_frame = frame.command.is_some();
            let is_ask_frame = frame.kind == SequenceFrameKind::Ask;
            let missing_assigned_agent = !is_command_frame
                && frame.assigned_agent.as_ref().is_some_and(|agent| {
                    provenance
                        .agent_assignments
                        .as_ref()
                        .is_some_and(|assignments| {
                            assignment_for_role(assignments, &agent.role).is_none()
                        })
                });
            if missing_assigned_agent {
                capabilities.push(CapabilityReport::unsupported(
                    "runtime.agent-assignment",
                    "current frame is assigned to an agent role without a run-session assignment",
                ));
            }
            next_frame = Some(frame);
            if missing_assigned_agent {
                Status::BlockedAgentUnassigned
            } else if is_command_frame {
                Status::BlockedCommandPermissionRequired
            } else if is_ask_frame {
                Status::WaitingOnHuman
            } else {
                Status::AwaitingAgentOutput
            }
        }
        NextSequenceFrameResult::Blocked {
            missing_inputs,
            capabilities: blocked_capabilities,
        } => {
            let awaiting_input = !worktree_provenance_missing
                && missing_inputs.iter().all(|item| item.starts_with("port:"));
            unresolved_inputs = missing_inputs;
            capabilities.extend(blocked_capabilities);
            if awaiting_input {
                Status::AwaitingInput
            } else {
                Status::Blocked
            }
        }
        NextSequenceFrameResult::Completed => Status::Completed,
        NextSequenceFrameResult::Rejected => Status::Rejected,
        NextSequenceFrameResult::Failed => Status::Failed,
    };

    capabilities.sort();
    capabilities.dedup();
    let final_output_summary = final_outputs(&state)?;
    let state_digest = ledger_digest(&state)?;
    let completion = if status == Status::Completed {
        Some(CompletionNotification {
            status: Status::Completed,
            event_code: "run.completed".to_string(),
            final_outputs: final_output_summary.clone(),
            final_session_digest: state_digest.clone(),
        })
    } else {
        None
    };
    let current = current_item_from_frame(next_frame.as_deref(), &state);
    let current_agent = next_frame
        .as_ref()
        .and_then(|frame| frame.assigned_agent.clone());

    Ok(Session {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id,
        run_id: state.run_id.clone(),
        trait_id: state.trait_id.clone(),
        source_digest: state.source_digest.clone(),
        canonical_digest: state.canonical_digest.clone(),
        current_run_index: state.current_run_index,
        current_source_index: current.source_index,
        current_sequence_item_id: current.item_id,
        current_sequence_title: current.title,
        current_agent,
        status,
        warnings,
        accepted_port_values: state.accepted_port_values.clone(),
        accepted_slot_values: state.accepted_slot_values.clone(),
        accepted_output_port_values: state.accepted_output_port_values.clone(),
        slot_revisions: state.slot_revisions.clone(),
        emitted_signals: state.emitted_signals.clone(),
        rejected_submissions: state.rejected_attempts.clone(),
        unresolved_inputs,
        resource_evidence: state.resource_evidence.clone(),
        provider_capability_reports: capabilities,
        output_ports: state.output_ports.clone(),
        resolved_settings: state.resolved_settings.clone(),
        active_path: state.active_path.clone(),
        control_stack: state.control_stack.clone(),
        stop_reason: state.stop_reason.clone(),
        final_output_summary,
        next_frame,
        last_validation_report,
        completion,
        last_drive_outcome: None,
        provenance,
        ledger: state,
        state_digest,
    })
}

fn assignment_for_role<'a>(
    assignments: &'a [AgentAssignment],
    role: &str,
) -> Option<&'a AgentAssignment> {
    assignments
        .iter()
        .find(|assignment| assignment.role == role)
}

fn caller_agent_role(trait_ref: &Trait, agent: Option<&str>) -> crate::Result<Option<String>> {
    normalize_caller_agent_role(trait_ref, agent)
        .map_err(|message| crate::procedure::invalid_field("run-call.caller.agent", message))
}

fn normalize_caller_agent_role(
    trait_ref: &Trait,
    agent: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(agent) = agent else {
        return Ok(None);
    };
    let role = if agent.contains(':') {
        let parsed = Reference::parse(agent)
            .map_err(|_| format!("caller agent {agent:?} is not a valid agent role"))?;
        if parsed.kind() != Kind::Agent || parsed.is_qualified() {
            return Err("caller agent refs must be local agent:* refs".to_string());
        }
        parsed.id().to_string()
    } else {
        agent.to_string()
    };
    if !trait_ref.agents.iter().any(|declared| declared.id == role) {
        return Err(format!(
            "caller agent {agent:?} is not declared in this trait"
        ));
    }
    Ok(Some(role))
}

fn require_worktree_provenance(trait_ref: &Trait, session: &Session) -> crate::Result<()> {
    if worktree_provenance_missing(trait_ref, &session.provenance) {
        return Err(crate::procedure::invalid_field(
            "run-session.provenance.worktree",
            "procedure requires prepared worktree provenance; start it with --worktree",
        ));
    }
    Ok(())
}

fn worktree_provenance_missing(trait_ref: &Trait, provenance: &Provenance) -> bool {
    trait_ref
        .procedure
        .as_ref()
        .is_some_and(|procedure| procedure.worktree_required)
        && provenance.worktree.is_none()
}

fn preflight_call_rejection(
    trait_ref: &Trait,
    session: &Session,
    submission: &CallSubmission,
    current_frame_set: bool,
) -> crate::Result<RunCallPreflight> {
    let frame_result = next_sequence_frame(trait_ref, &session.ledger)?;
    let recomputed_sequence_index = match &frame_result {
        NextSequenceFrameResult::Frame(frame) => {
            frame.sequence_index.unwrap_or(session.current_run_index)
        }
        _ => session
            .current_source_index
            .unwrap_or(session.current_run_index),
    };
    let mut report = StepValidationReport {
        sequence_index: recomputed_sequence_index,
        accepted_outputs: Vec::new(),
        rejected_outputs: Vec::new(),
        missing_required_outputs: Vec::new(),
        unfilled_optional_outputs: Vec::new(),
        unexpected_outputs: Vec::new(),
        schema_validation: Vec::new(),
        signal_validation: Vec::new(),
        warnings: Vec::new(),
        next_action: StepNextAction::Rejected,
    };
    let mut trusted_command_execution = false;
    let mut trusted_check_execution = false;
    let mut non_persisting_rejection = false;
    let mut semantic_rejection = false;
    if submission.session_id != session.session_id {
        report.rejected_outputs.push(rejected_envelope(
            report.sequence_index,
            "call session-id does not match loaded run session",
        ));
        non_persisting_rejection = true;
    }
    match (&frame_result, submission.command_execution.as_ref()) {
        (NextSequenceFrameResult::Frame(frame), Some(evidence)) => {
            if let Some(command) = frame.command.as_ref() {
                let is_check = frame.kind == SequenceFrameKind::Check;
                if command_evidence_matches_current_frame(evidence, command, submission, is_check) {
                    trusted_command_execution = true;
                    trusted_check_execution = is_check;
                } else {
                    report.rejected_outputs.push(rejected_envelope(
                        report.sequence_index,
                        if is_check {
                            "command execution evidence does not match current check frame, or the submitted verdict disagrees with the recomputed pass/fail"
                        } else {
                            "command execution evidence does not match current command frame"
                        },
                    ));
                    semantic_rejection = true;
                }
            } else {
                report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "command execution evidence is only accepted for the current command frame",
                ));
                semantic_rejection = true;
            }
        }
        (NextSequenceFrameResult::Frame(frame), None) if frame.command.is_some() => {
            report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "current command frame requires approved command execution evidence; manual call/set output is not accepted",
            ));
            semantic_rejection = true;
        }
        (_, Some(_)) => {
            report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "command execution evidence is only accepted for the current command frame",
            ));
            semantic_rejection = true;
        }
        _ => {}
    }
    let caller_agent = match normalize_caller_agent_role(
        trait_ref,
        submission
            .caller
            .as_ref()
            .and_then(|caller| caller.agent.as_deref()),
    ) {
        Ok(agent) => agent,
        Err(reason) => {
            report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                &format!("{reason}; caller agent routing is cooperative coordination, not an authentication or security boundary"),
            ));
            non_persisting_rejection = true;
            None
        }
    };
    if matches!(&frame_result, NextSequenceFrameResult::Frame(frame) if frame.kind == SequenceFrameKind::Ask)
        && (!current_frame_set
            || submission
                .caller
                .as_ref()
                .is_some_and(|caller| caller.agent.is_some() || caller.harness.is_some()))
    {
        report.rejected_outputs.push(rejected_envelope(
            report.sequence_index,
            "human-owned ask frames accept only session frame set submissions without agent or harness attribution",
        ));
        non_persisting_rejection = true;
    }
    if !trusted_command_execution
        && let NextSequenceFrameResult::Frame(frame) = &frame_result
        && frame.command.is_none()
        && let Some(assigned_agent) = frame.assigned_agent.as_ref()
        && caller_agent.as_deref() != Some(assigned_agent.role.as_str())
    {
        let supplied = caller_agent
            .as_ref()
            .map(|role| format!("agent:{role}"))
            .unwrap_or_else(|| "none".to_string());
        report.rejected_outputs.push(rejected_envelope(
                            report.sequence_index,
                            &format!(
                                "current frame is assigned to {}; caller agent was {supplied}; submit as --agent {}. This is cooperative routing, not an authentication or security boundary",
                                assigned_agent.ref_text, assigned_agent.role
                            ),
                        ));
        non_persisting_rejection = true;
    }
    let has_current_frame = session.next_frame.is_some()
        || session.current_source_index.is_some()
        || session.current_sequence_item_id.is_some();
    if has_current_frame {
        match submission.run_id.as_ref() {
            Some(run_id) if run_id == &session.run_id => {}
            Some(_) => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call run-id does not match loaded run session",
            )),
            None => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call missing run-id for current frame",
            )),
        }
        if submission.run_id.as_ref() != Some(&session.run_id) {
            non_persisting_rejection = true;
        }
        match submission.state_digest.as_deref() {
            Some(state_digest) if state_digest == session.state_digest.as_str() => {}
            Some(_) => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call state-digest does not match loaded run session",
            )),
            None => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call missing state-digest for current frame",
            )),
        }
        if submission.state_digest.as_deref() != Some(session.state_digest.as_str()) {
            non_persisting_rejection = true;
        }
        match submission.expected_run_index {
            Some(run_index) if run_index == session.current_run_index => {}
            Some(_) => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call expected-run-index does not match current run index",
            )),
            None => report.rejected_outputs.push(rejected_envelope(
                report.sequence_index,
                "call missing expected-run-index for current frame",
            )),
        }
        if submission.expected_run_index != Some(session.current_run_index) {
            non_persisting_rejection = true;
        }
        if let Some(current_source_index) = session.current_source_index {
            match submission.expected_source_index {
                Some(source_index) if source_index == current_source_index => {}
                Some(_) => report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call expected-source-index does not match current source index",
                )),
                None => report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call missing expected-source-index for current frame",
                )),
            }
            if submission.expected_source_index != Some(current_source_index) {
                non_persisting_rejection = true;
            }
        }
        if let Some(current_item_id) = session.current_sequence_item_id.as_deref() {
            match submission.expected_sequence_item_id.as_deref() {
                Some(item_id) if item_id == current_item_id => {}
                Some(_) => report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call expected-sequence-item-id does not match current item",
                )),
                None => report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call missing expected-sequence-item-id for current frame",
                )),
            }
            if submission.expected_sequence_item_id.as_deref() != Some(current_item_id) {
                non_persisting_rejection = true;
            }
        }
        if let NextSequenceFrameResult::Frame(frame) = &frame_result {
            if !frame.position_path.is_empty()
                && submission.expected_position_path != frame.position_path
            {
                report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call expected-position-path does not match current nested position",
                ));
                non_persisting_rejection = true;
            } else if frame.position_path.is_empty()
                && !submission.expected_position_path.is_empty()
            {
                report.rejected_outputs.push(rejected_envelope(
                    report.sequence_index,
                    "call expected-position-path must be empty for current flat position",
                ));
                non_persisting_rejection = true;
            }
        }
    }
    if report.rejected_outputs.is_empty() {
        Ok(RunCallPreflight {
            rejection: None,
            trusted_command_execution,
            trusted_check_execution,
            non_persisting_rejection: false,
        })
    } else {
        // Snapshot/identity rejections are non-persisting only when they are
        // purely transport contention. If semantic validation also failed,
        // persist the rejection so correction-required evidence is not lost.
        Ok(RunCallPreflight {
            rejection: Some(report),
            trusted_command_execution: false,
            trusted_check_execution: false,
            non_persisting_rejection: non_persisting_rejection && !semantic_rejection,
        })
    }
}

fn command_evidence_matches_current_frame(
    evidence: &CommandExecutionEvidence,
    command: &crate::procedure::runtime::CommandFrame,
    submission: &CallSubmission,
    is_check: bool,
) -> bool {
    if evidence.argv != command.argv
        || evidence.output_slot != command.output_slot
        || evidence.executable_digest != command.executable_digest
        || !submission.signals.is_empty()
    {
        return false;
    }
    if is_check {
        // A check always submits exactly one verdict record, pass or fail —
        // never the empty-output failure path ordinary commands use — and
        // core independently recomputes it from trusted exit evidence rather
        // than trusting a caller-supplied value. The argv is recomputed from
        // the frame's own command, never copied from the submission, so a
        // caller cannot mislabel which command produced the verdict.
        let verdict = check_verdict(evidence, command);
        return submission.produced_slots.len() == 1
            && submission
                .produced_slots
                .get(&command.output_slot)
                .is_some_and(|value| {
                    *value
                        == check_output_value(
                            verdict,
                            command,
                            &CheckEvidence::from_submission(evidence),
                        )
                });
    }
    if command_execution_succeeded(evidence, command) {
        submission.produced_slots.len() == 1
            && submission.produced_slots.contains_key(&command.output_slot)
    } else {
        // A trusted failed command submits no output. It follows the normal
        // rejected-output transition, which can activate its on-failure route.
        submission.produced_slots.is_empty()
    }
}

/// Recompute a check's pass/fail verdict from trusted exit evidence: a
/// timeout is always a fail, otherwise the accepted exit codes decide.
fn check_verdict(
    evidence: &CommandExecutionEvidence,
    command: &crate::procedure::runtime::CommandFrame,
) -> bool {
    command_execution_succeeded(evidence, command)
}

/// The value a check frame writes to its output slot: the pass/fail verdict
/// **and the argv that produced it**.
///
/// P565: this was a bare boolean, and the missing argv cost three runs. A
/// consumer handed `false` alone cannot tell which command failed, so it
/// falls back to whatever command the surrounding prose names — and when
/// documentation and the declared check disagree, every round re-validates
/// the wrong thing and the loop cannot converge. Carrying the argv makes the
/// gate self-describing: the command that decides is the command the reader
/// sees, with no second source of truth to drift against.
///
/// One constructor, used by both the runtime that submits the value
/// ([`crate::procedure::session`]'s caller in the IO layer) and the core
/// check here that re-derives it from trusted evidence, so the two can never
/// disagree about the shape.
pub fn check_output_value(
    verdict: bool,
    command: &crate::procedure::runtime::CommandFrame,
    evidence: &CheckEvidence,
) -> JsonValue {
    let mut value = serde_json::Map::new();
    value.insert("ok".to_string(), JsonValue::Bool(verdict));
    value.insert(
        "argv".to_string(),
        JsonValue::Array(
            command
                .argv
                .iter()
                .map(|argument| JsonValue::String(argument.clone()))
                .collect(),
        ),
    );
    if let Some(exit_code) = evidence.exit_code {
        value.insert("exit-code".to_string(), JsonValue::from(exit_code));
    }
    if evidence.timed_out {
        value.insert("timed-out".to_string(), JsonValue::Bool(true));
    }
    // Only on failure. A passing gate has nothing to diagnose, and every field
    // here is rendered into a frame a model pays for; a reader who wants the
    // full capture has the ledger. On failure the reader has no other channel
    // at all — which is exactly how a missing `just test` recipe produced six
    // identical rounds whose only evidence was `ok: false`, leaving the
    // reviewer to invent a cause and blame the worker for a command it never
    // chose.
    if !verdict && let Some(tail) = evidence.tail.as_deref() {
        value.insert("tail".to_string(), JsonValue::String(tail.to_string()));
    }
    JsonValue::Object(value)
}

/// The three facts a check's verdict record needs from its execution.
///
/// A named type rather than three positional arguments because the two sources
/// are different structs: a live submission carries captured stdout/stderr and
/// derives its tail, while a replayed ledger carries the already-derived tail
/// and no captured output at all. Both must produce a byte-identical record —
/// that equality is what the acceptance check and the ledger replay both
/// assert — so they converge here instead of at three call sites that could
/// drift apart.
#[derive(Debug, Clone)]
pub struct CheckEvidence {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub tail: Option<String>,
}

impl CheckEvidence {
    /// From a live submission, deriving the tail from captured output.
    pub fn from_submission(evidence: &CommandExecutionEvidence) -> Self {
        Self {
            exit_code: evidence.exit_code,
            timed_out: evidence.timed_out,
            tail: failure_tail(evidence),
        }
    }

    /// From persisted ledger evidence, reading the tail recorded when the
    /// command actually ran.
    pub fn from_ledger(evidence: &crate::procedure::runtime::CommandExecutionEvidence) -> Self {
        Self {
            exit_code: evidence.exit_code,
            timed_out: evidence.timed_out,
            tail: evidence.output_tail.clone(),
        }
    }
}

/// How much of a failed check's output travels in its verdict record.
///
/// Enough for a missing-recipe line, a compiler error, or a failed assertion;
/// far short of the capture limit, because this lands in a rendered frame.
const CHECK_TAIL_LIMIT: usize = 1_500;

/// The end of a failed command's output: stderr first, and stdout appended
/// when it carries something stderr does not.
///
/// Both streams, not stderr-or-stdout: the diagnosis is wherever the failing
/// tool put it, and they disagree. rustc and clippy explain themselves on
/// stderr — but `cargo fmt --check` prints its diff to STDOUT, and a launcher
/// like `just` adds only a one-line "Recipe failed" on stderr. A
/// stderr-preferred tail carried exactly that stub for a pure formatting
/// failure, which told the reader a recipe failed and not one character of
/// why. Stderr keeps first position because when it does explain, its
/// explanation is the sharper one.
///
/// The tail rather than the head — a build or test run prints its failure last,
/// under a long prologue of progress lines that carry no diagnosis.
fn failure_tail(evidence: &CommandExecutionEvidence) -> Option<String> {
    let stderr = evidence
        .stderr
        .as_deref()
        .filter(|text| !text.trim().is_empty());
    let stdout = evidence
        .stdout
        .as_deref()
        .filter(|text| !text.trim().is_empty());
    match (stderr, stdout) {
        (Some(stderr), Some(stdout)) => {
            // Split the budget: stderr gets first claim on half, stdout the
            // remainder — a short recipe stub on stderr leaves nearly the
            // whole budget to the stream that actually explains.
            let stderr_tail = clipped_tail(stderr, CHECK_TAIL_LIMIT / 2, evidence.stderr_truncated);
            let remaining = CHECK_TAIL_LIMIT.saturating_sub(stderr_tail.chars().count());
            let stdout_tail = clipped_tail(stdout, remaining, evidence.stdout_truncated);
            Some(format!("{stderr_tail}\n--- stdout ---\n{stdout_tail}"))
        }
        (Some(stderr), None) => Some(clipped_tail(
            stderr,
            CHECK_TAIL_LIMIT,
            evidence.stderr_truncated,
        )),
        (None, Some(stdout)) => Some(clipped_tail(
            stdout,
            CHECK_TAIL_LIMIT,
            evidence.stdout_truncated,
        )),
        (None, None) => None,
    }
}

/// The last `limit` characters of `text`, marked when anything was clipped —
/// by this cut or by the capture that produced `text` — so a reader never
/// mistakes a partial tail for the whole output.
fn clipped_tail(text: &str, limit: usize, capture_truncated: bool) -> String {
    let trimmed = text.trim_end();
    let characters: Vec<char> = trimmed.chars().collect();
    if characters.len() <= limit {
        if capture_truncated {
            format!("[earlier output truncated]\n{trimmed}")
        } else {
            trimmed.to_string()
        }
    } else {
        let tail: String = characters[characters.len() - limit..].iter().collect();
        format!("[earlier output truncated]\n{tail}")
    }
}

fn command_execution_succeeded(
    evidence: &CommandExecutionEvidence,
    command: &crate::procedure::runtime::CommandFrame,
) -> bool {
    !evidence.timed_out
        && evidence.exit_code.is_some_and(|exit_code| {
            if command.success_exit_code.is_empty() {
                exit_code == 0
            } else {
                command.success_exit_code.contains(&exit_code)
            }
        })
}

fn attach_call_template(
    session_id: &SessionId,
    state: &State,
    state_digest: &Digest,
    frame: &mut SequenceFrame,
) {
    let caller_agent = frame
        .assigned_agent
        .as_ref()
        .map(|agent| agent.role.clone());
    let produced_slots = frame
        .requested_outputs
        .iter()
        .map(|output| {
            let schema = output.schema_ref.as_deref().unwrap_or("schema:any");
            let instruction = match &output.operation {
                crate::r#trait::procedure::WriteOperation::Replace => {
                    format!("provide JSON value matching {schema}")
                }
                crate::r#trait::procedure::WriteOperation::Append => {
                    format!("provide one array element matching {schema}")
                }
                crate::r#trait::procedure::WriteOperation::Merge => {
                    format!("provide an object delta matching {schema}")
                }
                crate::r#trait::procedure::WriteOperation::SetField(field) => {
                    format!("provide field {field:?} matching {schema}")
                }
                crate::r#trait::procedure::WriteOperation::Increment => {
                    format!("provide a numeric delta matching {schema}")
                }
            };
            (output.slot_ref.to_string(), instruction)
        })
        .collect();
    let signals = frame
        .allowed_signals
        .iter()
        .map(|signal| {
            (
                signal.clone(),
                SequenceSignalTemplate {
                    evidence: Some("optional evidence string".to_string()),
                },
            )
        })
        .collect();
    frame.call_template = Some(SequenceCallTemplate {
        session_id: session_id.as_str().to_string(),
        run_id: state.run_id.as_str().to_string(),
        state_digest: state_digest.clone(),
        expected_run_index: state.current_run_index,
        expected_source_index: frame.sequence_index,
        expected_sequence_item_id: frame.item_id.clone(),
        expected_position_path: frame.position_path.clone(),
        produced_slots,
        signals,
        warnings: Vec::new(),
        required_agent: frame.assigned_agent.clone(),
        caller: SequenceCallerTemplate {
            surface: "cli|mcp|wasm".to_string(),
            caller: "caller identifier".to_string(),
            agent: caller_agent,
            harness: None,
        },
    });
}

fn reject_without_advancing(
    trait_ref: &Trait,
    session: Session,
    mut report: StepValidationReport,
) -> crate::Result<Session> {
    let rejection_path = current_rejection_path(&session);
    stamp_report_rejection_path(&mut report, &rejection_path);
    let mut state = session.ledger;
    rollback_active_parallel_branch(trait_ref, &mut state)?;
    state
        .rejected_attempts
        .extend(report.rejected_outputs.clone());
    for signal in report
        .signal_validation
        .iter()
        .filter(|signal| signal.acceptance == AcceptanceStatus::Rejected)
    {
        state.rejected_attempts.push(RejectedAttempt {
            sequence_index: report.sequence_index,
            position_path: rejection_path.clone(),
            ref_text: Some(signal.signal_ref.to_string()),
            value_digest: Some(signal.evidence_digest.clone()),
            reason: signal.reason.clone(),
        });
    }
    for missing in &report.missing_required_outputs {
        state.rejected_attempts.push(RejectedAttempt {
            sequence_index: report.sequence_index,
            position_path: rejection_path.clone(),
            ref_text: Some(missing.clone()),
            value_digest: None,
            reason: "required declared slot output was not supplied".to_string(),
        });
    }
    let mut next = build_session(
        trait_ref,
        session.session_id,
        state,
        Some(report),
        session.provenance,
    )?;
    // A rejected human answer leaves the same Ask frame answerable; agent
    // output rejections retain the existing correction-routing status.
    if next
        .next_frame
        .as_ref()
        .is_none_or(|frame| frame.kind != SequenceFrameKind::Ask)
    {
        next.status = Status::Rejected;
    }
    Ok(next)
}

fn reject_without_persisting(mut session: Session, mut report: StepValidationReport) -> Session {
    let rejection_path = current_rejection_path(&session);
    stamp_report_rejection_path(&mut report, &rejection_path);
    session.last_validation_report = Some(report);
    session
}

fn current_rejection_path(session: &Session) -> Vec<PathSegment> {
    session
        .next_frame
        .as_ref()
        .map(|frame| frame.position_path.clone())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| session.active_path.clone())
}

fn stamp_report_rejection_path(report: &mut StepValidationReport, path: &[PathSegment]) {
    if path.is_empty() {
        return;
    }
    for attempt in &mut report.rejected_outputs {
        if attempt.position_path.is_empty() {
            attempt.position_path = path.to_vec();
        }
    }
}

pub fn call_response(session: Session, response_kind: CallResponseKind) -> CallResponse {
    let report = session.last_validation_report.clone();
    let accepted_signals = report
        .as_ref()
        .map(|report| {
            report
                .signal_validation
                .iter()
                .filter(|signal| signal.acceptance == AcceptanceStatus::Accepted)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let rejected_signals = report
        .as_ref()
        .map(|report| {
            report
                .signal_validation
                .iter()
                .filter(|signal| signal.acceptance == AcceptanceStatus::Rejected)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let correction = report.as_ref().and_then(correction_for_report);
    CallResponse {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id: session.session_id.clone(),
        run_id: session.run_id.clone(),
        status: session.status.clone(),
        response_kind,
        accepted_slot_values: report
            .as_ref()
            .map(|report| report.accepted_outputs.clone())
            .unwrap_or_default(),
        rejected_slot_values: report
            .as_ref()
            .map(|report| report.rejected_outputs.clone())
            .unwrap_or_default(),
        accepted_signals,
        rejected_signals,
        schema_validation: report
            .as_ref()
            .map(|report| report.schema_validation.clone())
            .unwrap_or_default(),
        unexpected_outputs: report
            .as_ref()
            .map(|report| report.unexpected_outputs.clone())
            .unwrap_or_default(),
        missing_required_outputs: report
            .as_ref()
            .map(|report| report.missing_required_outputs.clone())
            .unwrap_or_default(),
        unfilled_optional_outputs: report
            .as_ref()
            .map(|report| report.unfilled_optional_outputs.clone())
            .unwrap_or_default(),
        correction,
        updated_session_digest: session.state_digest.clone(),
        next_frame: session.next_frame.clone(),
        completion: session.completion.clone(),
        session,
        persist_session: true,
    }
}

fn correction_for_report(report: &StepValidationReport) -> Option<String> {
    if report.next_action != StepNextAction::Rejected && report.rejected_outputs.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !report.unexpected_outputs.is_empty() {
        parts.push(format!(
            "remove undeclared output(s): {}",
            report.unexpected_outputs.join(", ")
        ));
    }
    if !report.missing_required_outputs.is_empty() {
        parts.push(format!(
            "provide required output(s): {}",
            report.missing_required_outputs.join(", ")
        ));
    }
    for rejected in &report.rejected_outputs {
        parts.push(rejected.reason.clone());
    }
    for validation in &report.schema_validation {
        if validation.status != SchemaStatus::Accepted {
            parts.push(format!("{}: {}", validation.ref_text, validation.reason));
        }
    }
    if parts.is_empty() {
        parts.push("submit only the requested outputs/signals for the current frame".to_string());
    }
    parts.sort();
    parts.dedup();
    Some(parts.join("; "))
}

fn rejected_envelope(sequence_index: usize, reason: &str) -> RejectedAttempt {
    RejectedAttempt {
        sequence_index,
        position_path: Vec::new(),
        ref_text: None,
        value_digest: None,
        reason: reason.to_string(),
    }
}

struct CurrentItem {
    source_index: Option<usize>,
    item_id: Option<String>,
    title: Option<String>,
}

fn current_item_from_frame(frame: Option<&SequenceFrame>, state: &State) -> CurrentItem {
    if let Some(frame) = frame {
        return CurrentItem {
            source_index: frame.sequence_index,
            item_id: frame.item_id.clone(),
            title: Some(frame.title.clone()),
        };
    }
    let status = state
        .sequence_statuses
        .iter()
        .find(|status| status.run_index == state.current_run_index);
    CurrentItem {
        source_index: status.map(|status| status.sequence_index),
        item_id: status.and_then(|status| status.item_id.clone()),
        title: status.map(|status| status.title.clone()),
    }
}

fn final_outputs(state: &State) -> crate::Result<Vec<FinalOutput>> {
    let mut outputs = Vec::new();
    for output in &state.output_ports {
        if output.status != OutputPortStatus::Accepted {
            continue;
        }
        let value = state
            .accepted_slot_values
            .iter()
            .find(|value| value.ref_text == output.value_slot_ref.as_str())
            .or_else(|| {
                state
                    .accepted_output_port_values
                    .iter()
                    .find(|value| value.ref_text == output.value_slot_ref.as_str())
            });
        let Some(value) = value else { continue };
        // An accepted optional output whose value is an empty JSON array
        // carries nothing a completion consumer needs to act on (P409): the
        // ledger still records the accepted `[]` as signed evidence via
        // `accepted_slot_values`/`accepted_output_port_values` above, this
        // projection just omits it from the run's structured final outputs.
        // Required outputs and any non-empty value are unaffected.
        if !output.required && matches!(&value.value, JsonValue::Array(items) if items.is_empty()) {
            continue;
        }
        outputs.push(FinalOutput {
            port_ref: Reference::parse(&output.port_ref)?,
            value_slot_ref: Reference::parse(&output.value_slot_ref)?,
            value_digest: value.value_digest.clone(),
            value: value.value.clone(),
        });
    }
    outputs.sort_by(|a, b| a.port_ref.cmp(&b.port_ref));
    Ok(outputs)
}

fn ledger_digest(state: &State) -> crate::Result<Digest> {
    let text = serde_json::to_string(state).map_err(|e| {
        crate::procedure::serialization("run-session.state-digest", "run ledger", e)
    })?;
    Ok(Digest::source(&text))
}

#[cfg(test)]
mod check_output_tests {
    use super::*;

    fn command_frame(argv: &[&str]) -> crate::procedure::runtime::CommandFrame {
        crate::procedure::runtime::CommandFrame {
            cmd: None,
            argv: argv.iter().map(|part| (*part).to_string()).collect(),
            executable_digest: None,
            resource_argv: Vec::new(),
            cwd: None,
            timeout_ms: None,
            idle_timeout_ms: None,
            capture_bytes: None,
            success_exit_code: Vec::new(),
            output_slot: "slot:gate".to_string(),
            permission_code: String::new(),
            reason: String::new(),
        }
    }

    /// P565: the IO layer submits this value and core re-derives it here to
    /// compare. They call one constructor precisely so they cannot drift —
    /// but a drift would reject EVERY check frame in every run, so the shape
    /// is pinned rather than left to that convention alone.
    fn check_evidence(exit_code: Option<i32>, tail: Option<&str>) -> CheckEvidence {
        CheckEvidence {
            exit_code,
            timed_out: false,
            tail: tail.map(str::to_string),
        }
    }

    #[test]
    fn check_output_carries_the_verdict_and_the_argv_that_produced_it() {
        let command = command_frame(&["just", "implement-phase-gates"]);
        let value = check_output_value(false, &command, &check_evidence(Some(1), None));
        assert_eq!(
            value,
            serde_json::json!({
                "ok": false,
                "argv": ["just", "implement-phase-gates"],
                "exit-code": 1,
            }),
            "a failed gate must still name the command that failed"
        );
        assert_eq!(
            check_output_value(true, &command, &check_evidence(Some(0), None))["ok"],
            serde_json::json!(true)
        );
    }

    /// The tail is what stops a reader inventing a cause for a failed gate,
    /// and it is pure cost on a passing one.
    #[test]
    fn check_output_carries_the_failure_tail_only_on_failure() {
        let command = command_frame(&["just", "test"]);
        let evidence = check_evidence(
            Some(1),
            Some("error: Justfile does not contain recipe `test`"),
        );
        let failed = check_output_value(false, &command, &evidence);
        assert_eq!(
            failed["tail"],
            serde_json::json!("error: Justfile does not contain recipe `test`"),
            "a failed gate must state why it failed"
        );
        let passed = check_output_value(true, &command, &evidence);
        assert!(
            passed.get("tail").is_none(),
            "a passing gate has nothing to diagnose and must not spend frame budget on output"
        );
    }

    fn submission_evidence(stdout: &str, stderr: &str) -> CommandExecutionEvidence {
        CommandExecutionEvidence {
            argv: vec!["just".to_string(), "test".to_string()],
            output_slot: "slot:gate".to_string(),
            executable_digest: None,
            exit_code: Some(1),
            timed_out: false,
            stdout: (!stdout.is_empty()).then(|| stdout.to_string()),
            stderr: (!stderr.is_empty()).then(|| stderr.to_string()),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    /// A tail longer than the budget keeps its END — a build prints its
    /// failure last, under a prologue that carries no diagnosis.
    #[test]
    fn derived_tail_keeps_the_end_and_marks_the_clip() {
        let long = format!("{}FAILURE HERE", "prologue line\n".repeat(400));
        let derived = CheckEvidence::from_submission(&submission_evidence("", &long));
        let tail = derived
            .tail
            .expect("a failed command with output has a tail");
        assert!(tail.ends_with("FAILURE HERE"), "the end must survive");
        assert!(
            tail.starts_with("[earlier output truncated]"),
            "a clipped tail must say so rather than look complete"
        );
    }

    /// Both streams travel, stderr first: the diagnosis lives wherever the
    /// failing tool put it. The motivating case is a formatting gate —
    /// `cargo fmt --check` explains itself on STDOUT while the launcher adds
    /// only a one-line recipe stub on stderr; a stderr-only tail carried the
    /// stub and dropped the diff.
    #[test]
    fn derived_tail_carries_both_streams_stderr_first() {
        let both = CheckEvidence::from_submission(&submission_evidence(
            "Diff in modules/io/src/run.rs:530",
            "error: Recipe `lint` failed on line 51",
        ));
        let tail = both.tail.expect("both streams present");
        let recipe = tail.find("Recipe `lint` failed").expect("stderr present");
        let diff = tail.find("Diff in modules/io").expect("stdout present");
        assert!(recipe < diff, "stderr leads, stdout follows: {tail}");

        let only_stdout = CheckEvidence::from_submission(&submission_evidence("the reason", ""));
        assert_eq!(only_stdout.tail.as_deref(), Some("the reason"));
        let only_stderr = CheckEvidence::from_submission(&submission_evidence("", "the reason"));
        assert_eq!(only_stderr.tail.as_deref(), Some("the reason"));
        let silent = CheckEvidence::from_submission(&submission_evidence("", "   "));
        assert!(
            silent.tail.is_none(),
            "whitespace-only output is not a diagnosis and must not be carried"
        );
    }

    /// The argv is recomputed from the frame, never taken from the caller, so
    /// a submission cannot claim a verdict came from a different command than
    /// the one the trait declared.
    #[test]
    fn check_output_argv_comes_from_the_declared_command() {
        let declared = command_frame(&["just", "implement-phase-gates"]);
        let claimed = command_frame(&["just", "test"]);
        assert_ne!(
            check_output_value(true, &declared, &check_evidence(Some(0), None)),
            check_output_value(true, &claimed, &check_evidence(Some(0), None)),
            "two different gate commands must not produce the same check record"
        );
    }
}

#[cfg(test)]
mod unbounded_loop_tests {
    use super::*;

    /// (0093) A loop declaring `until` and no `max-iterations` — runtime
    /// must advance past a "revise" verdict and stop on the guard once
    /// "approved" lands, never touching exhaustion at all.
    const FIXTURE: &str = r#"
id = "unbounded-loop-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Unbounded Loop Fixture"
description = "Regression fixture: a loop with until and no bound stops on its own guard."

[[agent]]
id = "reviewer"
description = "Produces the typed verdict for the loop."
summary = "Reviewer role."

[[slot]]
id = "verdict"
schema = "schema:verdict"
description = "Typed verdict carrying a status field."

[[schema]]
id = "verdict"
description = "Verdict object with a status enum."

[schema.fields.status]
schema = "schema:text"
required = true
description = "approved or revise."
allowed = [
    "approved",
    "revise",
]

[prompt.review]
text = "Produce the typed verdict object."

[[sequence.loop-body.sequence]]
id = "produce-verdict"
title = "Produce verdict"
agent = "agent:reviewer"
prompt = "prompt:review"
output = ["slot:verdict"]

[procedure]
description = "One unbounded loop stopping on its own until guard."

[[procedure.sequence]]
id = "verdict-loop"
title = "Verdict loop"
kind = "loop"
sequence = "sequence:loop-body"

[procedure.sequence.until]
slot = "slot:verdict"
field = "status"
equals = "approved"
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn submission_from_template(
        template: &SequenceCallTemplate,
        verdict: serde_json::Value,
    ) -> CallSubmission {
        CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [("slot:verdict".to_string(), verdict)]
                .into_iter()
                .collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "unbounded-loop-regression".to_string(),
                agent: Some("reviewer".to_string()),
                harness: None,
            }),
        }
    }

    #[test]
    fn until_only_loop_advances_on_revise_and_completes_on_approved() {
        let trait_ref = fixture_trait();
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-unbounded-loop-test",
            "run-id": "run-unbounded-loop-test",
            "provenance": {
                "started-by": {
                    "surface": "test",
                    "caller": "unbounded-loop-regression",
                },
                "state-source": "test",
            },
        }))
        .expect("start request");
        let session = start_run_session(
            &trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts");

        let template = session
            .next_frame
            .as_ref()
            .expect("loop frame is current")
            .call_template
            .clone()
            .expect("call template attached");

        // A "revise" verdict must advance to another iteration, not exhaust
        // — there is no bound to exhaust against.
        let revise = submit_run_call(
            &trait_ref,
            session,
            submission_from_template(&template, serde_json::json!({"status": "revise"})),
        )
        .expect("revise call lands");
        assert_eq!(
            revise.response_kind,
            CallResponseKind::AcceptedNextFrame,
            "an unbounded loop must advance to the next iteration on a non-matching guard"
        );

        let next_template = revise
            .session
            .next_frame
            .as_ref()
            .expect("loop frame still current after revise")
            .call_template
            .clone()
            .expect("call template attached");

        // An "approved" verdict must stop the loop via the until guard.
        let approved = submit_run_call(
            &trait_ref,
            revise.session,
            submission_from_template(&next_template, serde_json::json!({"status": "approved"})),
        )
        .expect("approved call lands");
        assert_eq!(
            approved.response_kind,
            CallResponseKind::AcceptedCompleted,
            "the until guard must stop the unbounded loop once satisfied"
        );
    }
}

#[cfg(test)]
mod produced_checklist_loop_tests {
    use super::*;

    /// (0167) A produce step fills a `[schema:checklist-item]` slot; a
    /// bounded loop's body replace-writes the same slot each round; the
    /// loop's `until` guard is `count(slot:plan).where(status == "done") >=
    /// 2` — the runtime path a unit-level `checklist_coverage_validation`
    /// call can never exercise, since it never drives the prior-accepted
    /// plumbing (`transitions.rs` / `control_flow.rs`) or guard evaluation.
    const FIXTURE: &str = r#"
id = "produced-checklist-loop-fixture"
schema-version = "0.3"
version = "0.1.0"
name = "Produced Checklist Loop Fixture"
description = "Regression fixture: a count guard exits a loop once every produced item is done."

[[agent]]
id = "worker"
description = "Fills and updates the checklist."
summary = "Worker role."

[[slot]]
id = "plan"
schema = "[schema:checklist-item]"
description = "Produced checklist of work items."

[prompt.produce]
text = "Produce the checklist."

[prompt.update]
text = "Update checklist statuses."

[[sequence.loop-body.sequence]]
id = "update-plan"
title = "Update plan"
agent = "agent:worker"
prompt = "prompt:update"
output = ["slot:plan"]

[procedure]
description = "Produce a checklist, then loop updating it until every item is done."

[[procedure.sequence]]
id = "produce-plan"
title = "Produce plan"
agent = "agent:worker"
prompt = "prompt:produce"
output = ["slot:plan"]

[[procedure.sequence]]
id = "update-loop"
title = "Update loop"
kind = "loop"
sequence = "sequence:loop-body"
max-iterations = 3
on-exhausted = "continue"

[procedure.sequence.until]
count = "slot:plan"
field = "status"
field-equals = "done"
at-least = 2
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn item(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "text": "do the thing", "status": status})
    }

    fn submission_from_template(
        template: &SequenceCallTemplate,
        plan: serde_json::Value,
    ) -> CallSubmission {
        CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [("slot:plan".to_string(), plan)].into_iter().collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "produced-checklist-loop-regression".to_string(),
                agent: Some("worker".to_string()),
                harness: None,
            }),
        }
    }

    fn start_fixture_session(trait_ref: &crate::r#trait::Trait) -> Session {
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-produced-checklist-loop-test",
            "run-id": "run-produced-checklist-loop-test",
            "provenance": {
                "started-by": {
                    "surface": "test",
                    "caller": "produced-checklist-loop-regression",
                },
                "state-source": "test",
            },
        }))
        .expect("start request");
        start_run_session(
            trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    #[test]
    fn count_guard_exits_the_loop_once_every_item_is_done() {
        let trait_ref = fixture_trait();
        let session = start_fixture_session(&trait_ref);

        // 1. Produce step: mint the universe, all todo.
        let template = session
            .next_frame
            .as_ref()
            .expect("produce frame is current")
            .call_template
            .clone()
            .expect("call template attached");
        let produced = submit_run_call(
            &trait_ref,
            session,
            submission_from_template(
                &template,
                serde_json::json!([item("a", "todo"), item("b", "todo")]),
            ),
        )
        .expect("produce call lands");
        assert_eq!(
            produced.response_kind,
            CallResponseKind::AcceptedNextFrame,
            "produce step must hand off to the loop, not complete the run"
        );

        // 2. Loop round one: flip only one item to done — the guard must NOT
        //    exit yet.
        let loop_template = produced
            .session
            .next_frame
            .as_ref()
            .expect("loop frame is current")
            .call_template
            .clone()
            .expect("call template attached");
        let partial = submit_run_call(
            &trait_ref,
            produced.session,
            submission_from_template(
                &loop_template,
                serde_json::json!([item("a", "done"), item("b", "todo")]),
            ),
        )
        .expect("partial-round call lands");
        assert_eq!(
            partial.response_kind,
            CallResponseKind::AcceptedNextFrame,
            "the count guard must not exit before every item is done"
        );

        // 3. Loop round two: flip the remaining item to done — the guard
        //    must exit and complete the run.
        let next_loop_template = partial
            .session
            .next_frame
            .as_ref()
            .expect("loop frame still current after partial round")
            .call_template
            .clone()
            .expect("call template attached");
        let complete = submit_run_call(
            &trait_ref,
            partial.session,
            submission_from_template(
                &next_loop_template,
                serde_json::json!([item("a", "done"), item("b", "done")]),
            ),
        )
        .expect("completing call lands");
        assert_eq!(
            complete.response_kind,
            CallResponseKind::AcceptedCompleted,
            "the count guard must exit the loop once every item is done"
        );
    }

    #[test]
    fn loop_body_replace_write_dropping_a_prior_id_is_rejected_by_the_runtime() {
        let trait_ref = fixture_trait();
        let session = start_fixture_session(&trait_ref);

        let template = session
            .next_frame
            .as_ref()
            .expect("produce frame is current")
            .call_template
            .clone()
            .expect("call template attached");
        let produced = submit_run_call(
            &trait_ref,
            session,
            submission_from_template(
                &template,
                serde_json::json!([item("a", "todo"), item("b", "todo")]),
            ),
        )
        .expect("produce call lands");

        let loop_template = produced
            .session
            .next_frame
            .as_ref()
            .expect("loop frame is current")
            .call_template
            .clone()
            .expect("call template attached");

        // Drops id "b" from the prior accepted revision — must be rejected
        // by the real runtime path (transitions.rs / control_flow.rs prior-
        // accepted plumbing), not merely by a direct
        // `checklist_coverage_validation` call.
        let dropped = submit_run_call(
            &trait_ref,
            produced.session,
            submission_from_template(&loop_template, serde_json::json!([item("a", "done")])),
        )
        .expect("dropped-id call lands");
        assert_eq!(
            dropped.response_kind,
            CallResponseKind::RejectedCorrectionRequired,
            "dropping a prior item id must be rejected by the runtime, not silently accepted"
        );
    }
}

#[cfg(test)]
mod correction_retry_tests {
    use super::*;

    /// Minimal canonical trait: one bounded loop whose body produces a typed
    /// slot with a required list field — the smallest shape that puts a
    /// schema-validatable frame at a NESTED position.
    const FIXTURE: &str = r#"
id = "retry-loop-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Retry Loop Fixture"
description = "Regression fixture: a typed slot produced inside a bounded loop."

[[agent]]
id = "reviewer"
description = "Produces the typed verdict for the loop."
summary = "Reviewer role."

[[slot]]
id = "verdict"
schema = "schema:verdict"
description = "Typed verdict carrying a required list field."

[[schema]]
id = "verdict"
description = "Verdict object with a status enum and a required list field."

[schema.fields.status]
schema = "schema:text"
required = true
description = "approved or revise."
allowed = [
    "approved",
    "revise",
]

[schema.fields.items]
schema = "[schema:text]"
required = true
description = "List-typed field; a scalar here must be rejected by schema validation."

[prompt.review]
text = "Produce the typed verdict object."

[[sequence.loop-body.sequence]]
id = "produce-verdict"
title = "Produce verdict"
agent = "agent:reviewer"
prompt = "prompt:review"
output = ["slot:verdict"]

[procedure]
description = "One bounded loop producing a typed verdict; exists to exercise correction retries at a nested position."

[[procedure.sequence]]
id = "verdict-loop"
title = "Verdict loop"
kind = "loop"
sequence = "sequence:loop-body"
max-iterations = 3
on-exhausted = "continue"

[procedure.sequence.until]
slot = "slot:verdict"
field = "status"
equals = "approved"
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn submission_from_template(
        template: &SequenceCallTemplate,
        verdict: serde_json::Value,
    ) -> CallSubmission {
        CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [("slot:verdict".to_string(), verdict)]
                .into_iter()
                .collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "correction-retry-regression".to_string(),
                agent: Some("reviewer".to_string()),
                harness: None,
            }),
        }
    }

    /// Regression: a schema rejection at a loop-nested frame is
    /// PERSISTED, which moves the state digest and re-attaches a fresh call
    /// template. A retry built from the pre-rejection template can then only
    /// bounce off the identity preflight — invisibly, since identity
    /// rejections are non-persisting — while a retry built from the
    /// response's refreshed template must land. The drive's correction-retry
    /// path relies on exactly this contract.
    #[test]
    fn valid_retry_lands_with_refreshed_template_but_not_stale_one() {
        let trait_ref = fixture_trait();
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-retry-fixture-test",
            "run-id": "run-retry-fixture-test",
            "provenance": {
                "started-by": {
                    "surface": "test",
                    "caller": "correction-retry-regression",
                },
                "state-source": "test",
            },
        }))
        .expect("start request");
        let session = start_run_session(
            &trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts");

        let frame = session.next_frame.clone().expect("loop frame is current");
        assert!(
            !frame.position_path.is_empty(),
            "fixture frame must sit at a nested position"
        );
        let original_template = frame.call_template.clone().expect("call template attached");

        // 1. Semantic rejection: the list field arrives as a scalar.
        let invalid = serde_json::json!({"status": "revise", "items": "not-a-list"});
        let first = submit_run_call(
            &trait_ref,
            session,
            submission_from_template(&original_template, invalid),
        )
        .expect("first call");
        assert!(
            matches!(
                first.response_kind,
                CallResponseKind::RejectedCorrectionRequired
            ),
            "scalar-for-list must be rejected"
        );

        // 2. Valid content through the STALE template still bounces on
        //    identity — this is what silently burned the live run's retries.
        let valid = serde_json::json!({"status": "approved", "items": ["ok"]});
        let stale = submit_run_call(
            &trait_ref,
            first.session,
            submission_from_template(&original_template, valid.clone()),
        )
        .expect("stale-template call");
        assert!(
            matches!(
                stale.response_kind,
                CallResponseKind::RejectedCorrectionRequired
            ),
            "valid content through the pre-rejection template must still be rejected"
        );

        // 3. The refreshed template carries a moved digest, and the same
        //    valid content through it must be accepted.
        let session_after = stale.session;
        let refreshed_template = session_after
            .next_frame
            .as_ref()
            .expect("frame still current after rejections")
            .call_template
            .clone()
            .expect("refreshed call template attached");
        assert_ne!(
            refreshed_template.state_digest, original_template.state_digest,
            "a persisted rejection must move the state digest"
        );
        let landed = submit_run_call(
            &trait_ref,
            session_after,
            submission_from_template(&refreshed_template, valid),
        )
        .expect("refreshed-template call");
        assert!(
            !matches!(
                landed.response_kind,
                CallResponseKind::RejectedCorrectionRequired
            ),
            "valid retry through the refreshed template must land; rejection: {:?}",
            landed.rejected_slot_values
        );
    }

    #[test]
    fn attributed_call_output_is_model_output() {
        let trait_ref = fixture_trait();
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-model-source-test",
            "run-id": "run-model-source-test",
            "provenance": { "started-by": { "surface": "test", "caller": "source-test" }, "state-source": "test" }
        }))
        .expect("start request");
        let session = start_run_session(
            &trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts");
        let template = session
            .next_frame
            .as_ref()
            .and_then(|frame| frame.call_template.as_ref())
            .expect("call template")
            .clone();
        let response = submit_run_call(
            &trait_ref,
            session,
            submission_from_template(
                &template,
                serde_json::json!({"status": "approved", "items": []}),
            ),
        )
        .expect("call lands");
        assert_eq!(
            response.accepted_slot_values[0].source,
            ValueSource::ModelOutput
        );
    }
}

/// P0047 regression fixture: an owner answer is scoped to the worker frame
/// immediately following an ask, then a deterministic post-review projection
/// removes it before the next refinement worker frame.
#[cfg(test)]
mod owner_answer_lifecycle_tests {
    use super::*;

    const FIXTURE: &str = r#"
id = "owner-answer-lifecycle-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Owner Answer Lifecycle Fixture"
description = "P0047 regression fixture for one-frame owner-answer delivery."

[[agent]]
id = "reviewer"
description = "Requests an owner answer."
summary = "Fixture reviewer role."

[[agent]]
id = "worker"
description = "Consumes the owner answer during refinement."
summary = "Fixture worker role."

[[slot]]
id = "verdict"
schema = "schema:verdict"
description = "Typed review result that triggers the owner ask."

[[slot]]
id = "owner-answer"
schema = "schema:text"
description = "Answer attached only to the first resumed worker frame."

[[slot]]
id = "first-work"
schema = "schema:text"
description = "Progressing refinement work after the owner answer."

[[slot]]
id = "second-work"
schema = "schema:text"
description = "Following refinement work, after owner-answer consumption."

[[schema]]
id = "verdict"
description = "Minimal review verdict."

[schema.fields.status]
schema = "schema:text"
required = true
description = "Whether the owner must answer."
allowed = ["revise"]

[prompt.review]
text = "Request the owner decision."

[prompt.ask]
text = "What owner decision clears the recurring blocker?"

[prompt.first-worker]
text = "Apply the owner answer to the progressing refinement round."

[prompt.second-worker]
text = "Continue refinement without a stale owner answer."

[procedure]
description = "Review, ask, resume one worker frame, consume the answer, then run the following worker frame."

[[procedure.sequence]]
id = "review"
title = "Review"
agent = "agent:reviewer"
prompt = "prompt:review"
output = ["slot:verdict"]

[[procedure.sequence]]
id = "owner-ask"
title = "Ask owner"
kind = "ask"
prompt = "prompt:ask"
output = ["slot:owner-answer"]

[[procedure.sequence.when.all]]
slot = "slot:verdict"
field = "status"
equals = "revise"

[[procedure.sequence]]
id = "first-worker"
title = "Apply owner answer"
agent = "agent:worker"
prompt = "prompt:first-worker"
input = [{ slot = "slot:owner-answer", optional = true }]
output = ["slot:first-work"]

[[procedure.sequence]]
id = "owner-answer-consume"
title = "Consume owner answer"
kind = "project"
output = ["slot:owner-answer"]

[[procedure.sequence.projection]]
destination = "slot:owner-answer"

[procedure.sequence.projection.source]
literal = ""

[[procedure.sequence]]
id = "second-worker"
title = "Continue refinement"
agent = "agent:worker"
prompt = "prompt:second-worker"
input = [{ slot = "slot:owner-answer", optional = true }]
output = ["slot:second-work"]
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn start_session(trait_ref: &crate::r#trait::Trait) -> Session {
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-owner-answer-lifecycle",
            "run-id": "run-owner-answer-lifecycle",
            "provenance": {
                "started-by": { "surface": "test", "caller": "owner-answer-lifecycle" },
                "state-source": "test",
            },
        }))
        .expect("start request");
        start_run_session(
            trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    fn submit_current(
        trait_ref: &crate::r#trait::Trait,
        session: Session,
        slot_ref: &str,
        value: serde_json::Value,
        agent: Option<&str>,
    ) -> CallResponse {
        let template = session
            .next_frame
            .as_ref()
            .and_then(|frame| frame.call_template.as_ref())
            .expect("current frame template")
            .clone();
        let submission = CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [(slot_ref.to_string(), value)].into_iter().collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "owner-answer-lifecycle".to_string(),
                agent: agent.map(str::to_string),
                harness: None,
            }),
        };
        if agent.is_none() {
            submit_current_frame_set(trait_ref, session, submission)
        } else {
            submit_run_call(trait_ref, session, submission)
        }
        .expect("call is accepted")
    }

    #[test]
    fn submitted_owner_answer_is_available_once_then_consumed_before_next_worker() {
        let trait_ref = fixture_trait();
        let review = submit_current(
            &trait_ref,
            start_session(&trait_ref),
            "slot:verdict",
            serde_json::json!({"status": "revise"}),
            Some("reviewer"),
        );
        assert_eq!(review.session.status, Status::WaitingOnHuman);
        assert_eq!(
            review.session.current_sequence_item_id.as_deref(),
            Some("owner-ask"),
            "the recurring review result must route directly to the human ask"
        );

        let answer = "Use the approved staging credential.";
        let resumed = submit_current(
            &trait_ref,
            review.session,
            "slot:owner-answer",
            serde_json::json!(answer),
            None,
        );
        let resumed_frame = resumed
            .session
            .next_frame
            .as_ref()
            .expect("resumed worker frame");
        assert_eq!(resumed_frame.item_id.as_deref(), Some("first-worker"));
        assert!(
            resumed_frame
                .available_inputs
                .iter()
                .any(|input| input.ref_text == "slot:owner-answer")
        );
        assert!(
            resumed
                .session
                .ledger
                .accepted_slot_values
                .iter()
                .any(|value| {
                    value.ref_text == "slot:owner-answer"
                        && value.value == serde_json::json!(answer)
                })
        );

        let following = submit_current(
            &trait_ref,
            resumed.session,
            "slot:first-work",
            serde_json::json!("progressed after applying the decision"),
            Some("worker"),
        );
        let following_frame = following
            .session
            .next_frame
            .as_ref()
            .expect("following worker frame");
        assert_eq!(following_frame.item_id.as_deref(), Some("second-worker"));
        // Optional slots remain visible once a value has been accepted, but
        // the deterministic projection must replace the actionable value.
        assert!(
            following_frame
                .available_inputs
                .iter()
                .any(|input| input.ref_text == "slot:owner-answer")
        );
        assert!(
            following
                .session
                .ledger
                .accepted_slot_values
                .iter()
                .any(|value| {
                    value.ref_text == "slot:owner-answer" && value.value == serde_json::json!("")
                })
        );
        assert!(
            following
                .session
                .ledger
                .accepted_slot_values
                .iter()
                .all(|value| {
                    value.ref_text != "slot:owner-answer"
                        || value.value != serde_json::json!(answer)
                }),
            "the following worker frame must not receive the prior owner answer"
        );
    }
}

/// P105 regression fixtures: a declared optional output sink left unfilled
/// completes the step normally and is ledger-distinguishable from a missing
/// required output, whose rejection behavior stays unchanged.
#[cfg(test)]
mod optional_output_sink_tests {
    use super::*;

    const FIXTURE: &str = r#"
id = "optional-output-sink-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Optional Output Sink Fixture"
description = "P105 regression fixture for an optional output sink left unfilled."

[[agent]]
id = "worker"
description = "Produces the required output and may skip the optional one."
summary = "Fixture worker role."

[[slot]]
id = "required-out"
schema = "schema:text"
description = "Always-produced output."

[[slot]]
id = "optional-out"
schema = "schema:text"
description = "Optionally-produced output."

[prompt.work]
text = "Produce the required output; the optional one may be left unfilled."

[procedure]
description = "One prompt step with one required and one declared-optional output sink."

[[procedure.sequence]]
id = "work"
title = "Work"
agent = "agent:worker"
prompt = "prompt:work"
output = ["slot:required-out", { slot = "slot:optional-out", optional = true }]
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn start_session(trait_ref: &crate::r#trait::Trait) -> Session {
        let request: StartRequest = serde_json::from_value(serde_json::json!({
            "session-id": "session-optional-output-sink",
            "run-id": "run-optional-output-sink",
            "provenance": {
                "started-by": { "surface": "test", "caller": "optional-output-sink" },
                "state-source": "test",
            },
        }))
        .expect("start request");
        start_run_session(
            trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    fn submit_current(
        trait_ref: &crate::r#trait::Trait,
        session: Session,
        produced_slots: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> CallResponse {
        let template = session
            .next_frame
            .as_ref()
            .and_then(|frame| frame.call_template.as_ref())
            .expect("current frame template")
            .clone();
        let submission = CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots,
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "optional-output-sink".to_string(),
                agent: Some("worker".to_string()),
                harness: None,
            }),
        };
        submit_run_call(trait_ref, session, submission).expect("call is accepted")
    }

    #[test]
    fn unfilled_optional_output_completes_the_step_without_rejection() {
        let trait_ref = fixture_trait();
        let response = submit_current(
            &trait_ref,
            start_session(&trait_ref),
            [("slot:required-out".to_string(), serde_json::json!("done"))]
                .into_iter()
                .collect(),
        );
        assert!(
            response.missing_required_outputs.is_empty(),
            "the required output was produced; nothing should be reported missing"
        );
        assert_eq!(
            response.unfilled_optional_outputs,
            vec!["slot:optional-out".to_string()],
            "the unfilled optional sink must be recorded as a signed non-failure"
        );
        assert!(
            response.rejected_slot_values.is_empty(),
            "an unfilled optional output must never be treated as a rejection"
        );
        assert!(
            response.correction.is_none(),
            "an unfilled optional output must not produce a correction"
        );
    }

    #[test]
    fn missing_required_output_still_fails_exactly_as_today() {
        let trait_ref = fixture_trait();
        let response = submit_current(
            &trait_ref,
            start_session(&trait_ref),
            [(
                "slot:optional-out".to_string(),
                serde_json::json!("skipped-required"),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            response.missing_required_outputs,
            vec!["slot:required-out".to_string()],
            "the required output was not produced and must be reported missing"
        );
        assert!(
            response
                .correction
                .as_deref()
                .is_some_and(|correction| correction.contains("provide required output(s)")),
            "a missing required output must still surface the existing correction: {:?}",
            response.correction
        );
    }
}

/// P399 regression fixtures: a loop's `on-exhausted` disposition end to end
/// (validate → runtime → emitted-signal evidence), reusing the correction
/// retry tests' full-session TOML fixture pattern above rather than a new
/// harness.
#[cfg(test)]
mod exhaustion_disposition_tests {
    use super::*;
    use crate::procedure::runtime::SignalSource;

    /// One bounded loop (budget exhausts after exactly one round, since
    /// there is no `until`/`abort-if` to short-circuit it) followed by a
    /// step, so exhaustion's continue path has somewhere to prove it
    /// actually proceeds. `{ON_EXHAUSTED}` is substituted per test.
    fn fixture_toml(on_exhausted_line: &str) -> String {
        format!(
            r#"
id = "loop-exhaustion-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Loop Exhaustion Fixture"
description = "P399 regression fixture: loop on-exhausted disposition and signal emission."

[[agent]]
id = "worker"
description = "Runs the loop body and the follow-up step."
summary = "Worker role."

[[slot]]
id = "note"
schema = "schema:text"
description = "Loop body output."

[[slot]]
id = "final"
schema = "schema:text"
description = "Follow-up step output."

[[signal]]
id = "refinement-exhausted"
description = "The loop spent its budget without meeting its exit condition."

[[signal]]
id = "refinement-exhausted-two"
description = "A second loop-exhaustion signal for the multi-signal fixture."

[prompt.loop-step]
text = "Write a note."

[prompt.after-loop]
text = "Write the final output."

[[sequence.loop-body.sequence]]
id = "write-note"
title = "Write note"
agent = "agent:worker"
prompt = "prompt:loop-step"
output = ["slot:note"]

[procedure]
description = "One bounded loop followed by a step; on-exhausted varies per test."

[[procedure.sequence]]
id = "note-loop"
title = "Note loop"
kind = "loop"
sequence = "sequence:loop-body"
max-iterations = 1
{on_exhausted_line}

[[procedure.sequence]]
id = "after-loop"
title = "After loop"
agent = "agent:worker"
prompt = "prompt:after-loop"
output = ["slot:final"]
"#
        )
    }

    fn fixture_trait(on_exhausted_line: &str) -> crate::r#trait::Trait {
        toml::from_str(&fixture_toml(on_exhausted_line)).expect("fixture trait parses")
    }

    fn start_session(trait_ref: &crate::r#trait::Trait, strict_loops: bool) -> Session {
        let request = StartRequest {
            session_id: SessionId::new("session-p399-fixture-test".to_string())
                .expect("session id"),
            run_id: Id::new("run-p399-fixture-test".to_string()).expect("run id"),
            initial_port_values: Vec::new(),
            resource_evidence: Vec::new(),
            resolved_settings: Vec::new(),
            provider_capability_reports: Vec::new(),
            source_digest: None,
            canonical_digest: None,
            agent_assignments: None,
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            strict_loops,
            provenance: Provenance {
                started_by: CallerProvenance {
                    surface: "test".to_string(),
                    caller: "p399-exhaustion-regression".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
        };
        start_run_session(
            trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    /// Submit the current frame's expected output verbatim, driving the run
    /// forward exactly one accepted call.
    fn submit_current(
        trait_ref: &crate::r#trait::Trait,
        session: Session,
        slot_ref: &str,
        value: serde_json::Value,
    ) -> CallResponse {
        let frame = session.next_frame.clone().expect("a current frame exists");
        let template = frame.call_template.clone().expect("call template attached");
        let submission = CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [(slot_ref.to_string(), value)].into_iter().collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "p399-exhaustion-regression".to_string(),
                agent: Some("worker".to_string()),
                harness: None,
            }),
        };
        submit_run_call(trait_ref, session, submission).expect("call is accepted")
    }

    #[test]
    fn continuing_signal_emits_evidence_and_proceeds_past_the_loop() {
        let trait_ref = fixture_trait("on-exhausted = \"signal:refinement-exhausted\"");
        let session = start_session(&trait_ref, false);

        let after_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        assert_eq!(
            after_loop.response_kind,
            CallResponseKind::AcceptedNextFrame
        );
        assert_eq!(
            after_loop.session.current_sequence_item_id.as_deref(),
            Some("after-loop"),
            "exhaustion must be transparent and hand control to the step after the loop"
        );
        let emitted: Vec<&SignalEmission> = after_loop
            .session
            .emitted_signals
            .iter()
            .filter(|signal| signal.signal_ref.as_str() == "signal:refinement-exhausted")
            .collect();
        assert_eq!(
            emitted.len(),
            1,
            "exactly one exhaustion signal must be recorded: {:?}",
            after_loop.session.emitted_signals
        );
        assert_eq!(emitted[0].source, Some(SignalSource::RuntimeControl));
        assert!(
            emitted[0].runtime_control.is_some(),
            "the emission must carry the loop's runtime-control identity"
        );

        let completed = submit_current(
            &trait_ref,
            after_loop.session,
            "slot:final",
            serde_json::json!("final answer"),
        );
        assert_eq!(completed.response_kind, CallResponseKind::AcceptedCompleted);
        assert_eq!(completed.session.status, Status::Completed);

        let report = crate::procedure::runtime::validate_run_ledger_contract(
            &trait_ref,
            &completed.session.ledger,
        )
        .expect("ledger contract check runs");
        assert!(
            report.diagnostics.is_empty(),
            "the runtime-emitted exhaustion signal must be a ledger-contract-allowed emission: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn multi_signal_list_emits_every_signal_in_authored_order() {
        let trait_ref = fixture_trait(
            "on-exhausted = [\"signal:refinement-exhausted\", \"signal:refinement-exhausted-two\"]",
        );
        let session = start_session(&trait_ref, false);

        let after_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        let order: Vec<&str> = after_loop
            .session
            .emitted_signals
            .iter()
            .map(|signal| signal.signal_ref.as_str())
            .collect();
        assert_eq!(
            order,
            vec![
                "signal:refinement-exhausted",
                "signal:refinement-exhausted-two",
            ],
            "signals must be emitted in authored order"
        );
    }

    #[test]
    fn block_still_stops_the_run_and_emits_no_signal() {
        let trait_ref = fixture_trait("on-exhausted = \"abort\"");
        let session = start_session(&trait_ref, false);

        let after_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        assert_eq!(after_loop.session.status, Status::Blocked);
        assert!(after_loop.session.emitted_signals.is_empty());
        assert_eq!(
            after_loop
                .session
                .stop_reason
                .as_ref()
                .map(|reason| reason.reason.as_str()),
            Some("max-iterations-exhausted")
        );
    }

    #[test]
    fn omitted_policy_continues_past_the_loop_with_no_signal() {
        let trait_ref = fixture_trait("");
        let session = start_session(&trait_ref, false);

        let after_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        assert_eq!(
            after_loop.response_kind,
            CallResponseKind::AcceptedNextFrame
        );
        assert_eq!(
            after_loop.session.current_sequence_item_id.as_deref(),
            Some("after-loop")
        );
        assert!(after_loop.session.emitted_signals.is_empty());
    }

    /// A loop nested inside another loop's body, with a branch reading the
    /// inner loop's exhaustion signal as a sibling item in the *enclosing*
    /// sequence — the position-path/identity pairing the runtime change
    /// pins on (activation path, not active path). Zero ledger-contract
    /// diagnostics is the proof the emission's scope matches the guard's.
    fn nested_fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(
            r#"
id = "loop-exhaustion-nested-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Loop Exhaustion Nested Fixture"
description = "P399 regression fixture: a loop nested inside another loop's body, exhaustion signal read by a sibling branch."

[[agent]]
id = "worker"
description = "Runs every step in the fixture."
summary = "Worker role."

[[slot]]
id = "note"
schema = "schema:text"
description = "Inner loop body output."

[[slot]]
id = "arm"
schema = "schema:text"
description = "Which branch arm ran."

[[slot]]
id = "final"
schema = "schema:text"
description = "Follow-up step output."

[[signal]]
id = "refinement-exhausted"
description = "The inner loop spent its budget without meeting its exit condition."

[prompt.loop-step]
text = "Write a note."

[prompt.arm-yes-step]
text = "Record that exhaustion was observed."

[prompt.arm-no-step]
text = "Record that exhaustion was not observed."

[prompt.after-loop]
text = "Write the final output."

[[sequence.inner-body.sequence]]
id = "write-note"
title = "Write note"
agent = "agent:worker"
prompt = "prompt:loop-step"
output = ["slot:note"]

[[sequence.arm-yes.sequence]]
id = "write-arm-yes"
title = "Record exhaustion observed"
agent = "agent:worker"
prompt = "prompt:arm-yes-step"
output = ["slot:arm"]

[[sequence.arm-no.sequence]]
id = "write-arm-no"
title = "Record exhaustion not observed"
agent = "agent:worker"
prompt = "prompt:arm-no-step"
output = ["slot:arm"]

[[sequence.outer-body.sequence]]
id = "inner-loop"
title = "Inner loop"
kind = "loop"
sequence = "sequence:inner-body"
max-iterations = 1
on-exhausted = "signal:refinement-exhausted"

[[sequence.outer-body.sequence]]
id = "exhaustion-branch"
title = "Branch on exhaustion signal"
kind = "branch"
when = "signal:refinement-exhausted"
sequence = "sequence:arm-yes"
otherwise = "sequence:arm-no"

[procedure]
description = "An inner loop nested inside an outer loop's body, with a sibling branch reading the inner loop's exhaustion signal."

[[procedure.sequence]]
id = "outer-loop"
title = "Outer loop"
kind = "loop"
sequence = "sequence:outer-body"
max-iterations = 1
on-exhausted = "continue"

[[procedure.sequence]]
id = "after-loop"
title = "After loop"
agent = "agent:worker"
prompt = "prompt:after-loop"
output = ["slot:final"]
"#,
        )
        .expect("nested fixture trait parses")
    }

    #[test]
    fn nested_loop_exhaustion_signal_scope_matches_the_enclosing_sibling_branch() {
        let trait_ref = nested_fixture_trait();
        let session = start_session(&trait_ref, false);

        let after_inner_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        assert_eq!(
            after_inner_loop.session.current_sequence_item_id.as_deref(),
            Some("write-arm-yes"),
            "the sibling branch must read the inner loop's just-emitted exhaustion signal and select its true arm"
        );
        let emitted: Vec<&SignalEmission> = after_inner_loop
            .session
            .emitted_signals
            .iter()
            .filter(|signal| signal.signal_ref.as_str() == "signal:refinement-exhausted")
            .collect();
        assert_eq!(
            emitted.len(),
            1,
            "exactly one exhaustion signal must be recorded: {:?}",
            after_inner_loop.session.emitted_signals
        );

        let after_branch = submit_current(
            &trait_ref,
            after_inner_loop.session,
            "slot:arm",
            serde_json::json!("yes"),
        );
        assert_eq!(
            after_branch.session.current_sequence_item_id.as_deref(),
            Some("after-loop"),
            "the outer loop must complete and hand control to the step after it"
        );

        let completed = submit_current(
            &trait_ref,
            after_branch.session,
            "slot:final",
            serde_json::json!("final answer"),
        );
        assert_eq!(completed.response_kind, CallResponseKind::AcceptedCompleted);
        assert_eq!(completed.session.status, Status::Completed);

        let report = crate::procedure::runtime::validate_run_ledger_contract(
            &trait_ref,
            &completed.session.ledger,
        )
        .expect("ledger contract check runs");
        assert!(
            report.diagnostics.is_empty(),
            "the nested exhaustion signal's activation-path scope must match the sibling branch's guard scope: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn strict_loops_blocks_a_continuing_loop_and_suppresses_its_signal() {
        let trait_ref = fixture_trait("on-exhausted = \"signal:refinement-exhausted\"");
        let session = start_session(&trait_ref, true);

        let after_loop = submit_current(
            &trait_ref,
            session,
            "slot:note",
            serde_json::json!("first draft"),
        );
        assert_eq!(after_loop.session.status, Status::Blocked);
        assert!(
            after_loop.session.emitted_signals.is_empty(),
            "strict loops must not emit a continuing loop's declared signal: {:?}",
            after_loop.session.emitted_signals
        );
    }

    /// P334 regression fixture: a `abort-if` guard with a declared `on-abort`
    /// signal, exercised end to end (runtime emission → ledger-contract
    /// acceptance) — the layer the recurrence breaker's own unit tests
    /// (validate.rs) never reach, reusing this module's `start_session`/
    /// `submit_current` harness rather than a new one.
    fn abort_if_fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(
            r#"
id = "loop-abort-if-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Loop Stop-If Fixture"
description = "P334 regression fixture: loop abort-if/on-abort disposition and signal emission."

[[agent]]
id = "worker"
description = "Produces the typed verdict for the loop."
summary = "Worker role."

[[slot]]
id = "verdict"
schema = "schema:verdict"
description = "Typed verdict carrying a status field."

[[schema]]
id = "verdict"
description = "Verdict object with a status enum."

[schema.fields.status]
schema = "schema:text"
required = true
description = "approved or revise."
allowed = [
    "approved",
    "revise",
]

[[signal]]
id = "recurring-blocker-unresolved"
description = "The abort-if guard matched: the same blocker recurred past the breaker's round threshold."

[prompt.review]
text = "Produce the typed verdict object."

[[sequence.loop-body.sequence]]
id = "produce-verdict"
title = "Produce verdict"
agent = "agent:worker"
prompt = "prompt:review"
output = ["slot:verdict"]

[procedure]
description = "One bounded loop that stops early on an abort-if match instead of spending its full iteration budget."

[[procedure.sequence]]
id = "verdict-loop"
title = "Verdict loop"
kind = "loop"
sequence = "sequence:loop-body"
max-iterations = 3
on-abort = "signal:recurring-blocker-unresolved"

[procedure.sequence.until]
slot = "slot:verdict"
field = "status"
equals = "approved"

[procedure.sequence.abort-if]
slot = "slot:verdict"
field = "status"
equals = "revise"
"#,
        )
        .expect("abort-if fixture trait parses")
    }

    #[test]
    fn declared_on_abort_signal_is_emitted_in_place_of_the_canonical_signal() {
        let trait_ref = abort_if_fixture_trait();
        let session = start_session(&trait_ref, false);

        let stopped = submit_current(
            &trait_ref,
            session,
            "slot:verdict",
            serde_json::json!({"status": "revise"}),
        );
        assert_eq!(
            stopped.session.status,
            Status::Blocked,
            "an abort-if match halts the run blocked"
        );
        assert_eq!(
            stopped
                .session
                .stop_reason
                .as_ref()
                .map(|reason| reason.reason.as_str()),
            Some("abort-if-matched"),
            "the runtime's stop reason stays the accurate mechanism regardless of authoring"
        );
        let emitted: Vec<&str> = stopped
            .session
            .emitted_signals
            .iter()
            .map(|signal| signal.signal_ref.as_str())
            .collect();
        assert_eq!(
            emitted,
            vec!["signal:recurring-blocker-unresolved"],
            "the declared on-abort signal must be emitted in place of the canonical signal:abort-if-matched"
        );

        let report = crate::procedure::runtime::validate_run_ledger_contract(
            &trait_ref,
            &stopped.session.ledger,
        )
        .expect("ledger contract check runs");
        assert!(
            report.diagnostics.is_empty(),
            "the declared on-abort signal must be a ledger-contract-allowed emission: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn killed_drive_outcome_kind_round_trips_the_wire_value() {
        assert_eq!(
            DriveOutcomeKind::from_wire("killed"),
            DriveOutcomeKind::Killed
        );
        assert_eq!(DriveOutcomeKind::Killed.as_str(), "killed");
    }

    #[test]
    fn budget_exhausted_drive_outcome_kind_round_trips_the_wire_value() {
        assert_eq!(
            DriveOutcomeKind::from_wire("paused-budget-exhausted"),
            DriveOutcomeKind::PausedBudgetExhausted
        );
        assert_eq!(
            DriveOutcomeKind::PausedBudgetExhausted.as_str(),
            "paused-budget-exhausted"
        );
    }

    #[test]
    fn budget_exhausted_pause_json_round_trips() {
        let pause = BudgetExhaustedPause {
            ceiling_kind: crate::procedure::runtime::BudgetCeilingKind::Tokens,
            ceiling: 1000.0,
            observed: 1200.0,
            role: Some("worker".to_string()),
            frame_title: "step".to_string(),
            frame_item_id: None,
            frame_run_index: 2,
            detail: "run token ceiling reached".to_string(),
        };
        let json = serde_json::to_string(&pause).expect("pause serializes");
        let decoded: BudgetExhaustedPause = serde_json::from_str(&json).expect("pause decodes");
        assert_eq!(decoded, pause);
        assert!(json.contains("\"ceiling-kind\":\"tokens\""));
    }

    /// A ledger written before 0130 (no `budget-pause`/`tokens-by-model`
    /// fields at all) must still deserialize, with both fields absent.
    #[test]
    fn drive_outcome_json_predating_0130_deserializes_with_budget_fields_absent() {
        let json = r#"{"outcome":"completed","recorded-at-epoch":0}"#;
        let outcome: DriveOutcome = serde_json::from_str(json).expect("old-shaped outcome decodes");
        assert_eq!(outcome.budget_pause, None);
        assert_eq!(outcome.tokens_by_model, None);
    }

    #[test]
    fn provenance_json_missing_session_title_deserializes_to_none() {
        let provenance: Provenance = serde_json::from_str(
            r#"{"started-by":{"surface":"test","caller":"c"},"state-source":"s"}"#,
        )
        .expect("old-shaped provenance JSON still deserializes");
        assert!(provenance.session_title.is_none());
    }

    #[test]
    fn resolved_session_title_round_trips_through_json() {
        let provenance = Provenance {
            started_by: CallerProvenance {
                surface: "test".to_string(),
                caller: "c".to_string(),
                agent: None,
                harness: None,
            },
            state_source: "s".to_string(),
            agent_assignments: None,
            harness_probes: Vec::new(),
            warnings: Vec::new(),
            trait_source: None,
            query_selection: None,
            worktree: None,
            merge_frames: Vec::new(),
            merge_intent: None,
            out_of_tree_mutations: Vec::new(),
            started_at_epoch: None,
            trust_approval: None,
            session_title: Some(SessionTitleState::Resolved {
                attempts: 2,
                title: "Refactor the merge story".to_string(),
                source: SessionTitleSource::NarratorDefault,
            }),
            task_digest: None,
            task_key: None,
            dependency_override: None,
        };
        let text = serde_json::to_string(&provenance).expect("serialize");
        let round_tripped: Provenance = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(round_tripped.session_title, provenance.session_title);
    }

    #[test]
    fn legacy_attempted_title_less_state_is_terminal() {
        let provenance: Provenance = serde_json::from_str(
            r#"{"started-by":{"surface":"test","caller":"c"},"state-source":"s","session-title":{"attempted":true}}"#,
        )
        .expect("attempted-with-no-title JSON deserializes");
        let state = provenance.session_title.expect("state present");
        assert_eq!(
            state,
            SessionTitleState::Terminal {
                attempts: 1,
                reason: "legacy-attempted".to_string(),
            }
        );
    }

    #[test]
    fn legacy_resolved_title_with_no_source_defaults_to_narrator_default() {
        let provenance: Provenance = serde_json::from_str(
            r#"{"started-by":{"surface":"test","caller":"c"},"state-source":"s","session-title":{"state":"resolved","attempts":1,"title":"Old title"}}"#,
        )
        .expect("pre-0110 resolved JSON with no source deserializes");
        assert_eq!(
            provenance.session_title,
            Some(SessionTitleState::Resolved {
                attempts: 1,
                title: "Old title".to_string(),
                source: SessionTitleSource::NarratorDefault,
            })
        );
    }

    #[test]
    fn legacy_attempted_with_title_defaults_to_narrator_default_source() {
        let provenance: Provenance = serde_json::from_str(
            r#"{"started-by":{"surface":"test","caller":"c"},"state-source":"s","session-title":{"attempted":true,"title":"Old style"}}"#,
        )
        .expect("attempted-with-title JSON deserializes");
        assert_eq!(
            provenance.session_title,
            Some(SessionTitleState::Resolved {
                attempts: 1,
                title: "Old style".to_string(),
                source: SessionTitleSource::NarratorDefault,
            })
        );
    }

    #[test]
    fn resolved_source_round_trips() {
        for source in [
            SessionTitleSource::NarratorDefault,
            SessionTitleSource::SinkVerbatim,
            SessionTitleSource::SinkGenerated,
        ] {
            let state = SessionTitleState::Resolved {
                attempts: 1,
                title: "Title".to_string(),
                source,
            };
            let json = serde_json::to_string(&state).expect("serialize");
            assert!(json.contains(&format!("\"source\":\"{source}\"")));
            assert_eq!(
                serde_json::from_str::<SessionTitleState>(&json).expect("deserialize"),
                state
            );
        }
    }

    #[test]
    fn every_session_title_lifecycle_state_round_trips() {
        for state in [
            SessionTitleState::InFlight {
                owner: "driver-a".to_string(),
                attempts: 1,
            },
            SessionTitleState::Retryable { attempts: 2 },
            SessionTitleState::Resolved {
                attempts: 3,
                title: "Title".to_string(),
                source: SessionTitleSource::SinkVerbatim,
            },
            SessionTitleState::Terminal {
                attempts: 3,
                reason: "failed".to_string(),
            },
        ] {
            let json = serde_json::to_string(&state).expect("serialize");
            assert!(json.contains("state"));
            assert_eq!(
                serde_json::from_str::<SessionTitleState>(&json).expect("deserialize"),
                state
            );
        }
    }
}

/// Proves task 0085's ledger-contract Done-when clause: a `project` step
/// lifts a nested field into a slot, a later step's `when` guard reads the
/// same nested field, and the recorded operand agrees with the re-derived
/// one across every transition and a serialize/deserialize resume cycle —
/// mirroring plannotator's real `hookSpecificOutput.decision.behavior`.
#[cfg(test)]
mod nested_field_path_session_tests {
    use super::*;

    const FIXTURE: &str = r#"
id = "nested-field-path-session-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Nested Field Path Session Fixture"
description = "0085 regression fixture: project and guard over a three-level field path."

[[agent]]
id = "worker"
description = "Produces the hook output and the gated follow-up."
summary = "Fixture worker role."

[[slot]]
id = "hook-output"
schema = "schema:hook-specific-output"
description = "Nested tool output; mirrors plannotator's hookSpecificOutput."

[[slot]]
id = "behavior"
schema = "schema:text"
description = "Nested field lifted out by the project step."

[[slot]]
id = "final"
schema = "schema:text"
description = "Only reachable when the nested field equals approve."

[[schema]]
id = "decision"
[schema.fields.behavior]
schema = "schema:text"
required = false

[[schema]]
id = "hook-specific-output"
[schema.fields.decision]
schema = "schema:decision"
required = false

[prompt.produce]
text = "Produce the hook output."

[prompt.gated]
text = "Only reachable when behavior is approve."

[procedure]
description = "Produce a nested hook output, project a nested field, then gate a step on it."

[[procedure.sequence]]
id = "produce-hook-output"
title = "Produce hook output"
agent = "agent:worker"
prompt = "prompt:produce"
output = ["slot:hook-output"]

[[procedure.sequence]]
id = "lift-behavior"
title = "Lift nested behavior field"
kind = "project"
output = ["slot:behavior"]

[[procedure.sequence.projection]]
source = "slot:hook-output"
field = "decision.behavior"
destination = "slot:behavior"

[[procedure.sequence]]
id = "gated-step"
title = "Gated step"
agent = "agent:worker"
prompt = "prompt:gated"
output = ["slot:final"]

[[procedure.sequence.when.all]]
slot = "slot:hook-output"
field = "decision.behavior"
equals = "approve"
"#;

    fn fixture_trait() -> crate::r#trait::Trait {
        toml::from_str(FIXTURE).expect("fixture trait parses")
    }

    fn start_session(trait_ref: &crate::r#trait::Trait) -> Session {
        let request = StartRequest {
            session_id: SessionId::new("session-0085-nested-field-path".to_string())
                .expect("session id"),
            run_id: Id::new("run-0085-nested-field-path".to_string()).expect("run id"),
            initial_port_values: Vec::new(),
            resource_evidence: Vec::new(),
            resolved_settings: Vec::new(),
            provider_capability_reports: Vec::new(),
            source_digest: None,
            canonical_digest: None,
            agent_assignments: None,
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            strict_loops: false,
            provenance: Provenance {
                started_by: CallerProvenance {
                    surface: "test".to_string(),
                    caller: "0085-nested-field-path".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
        };
        start_run_session(
            trait_ref,
            &crate::manifest::PackageStatus::Ready,
            &crate::r#trait::TrustVerdict::Verified,
            request,
        )
        .expect("session starts")
    }

    fn submit_current(
        trait_ref: &crate::r#trait::Trait,
        session: Session,
        slot_ref: &str,
        value: serde_json::Value,
    ) -> CallResponse {
        let frame = session.next_frame.clone().expect("a current frame exists");
        let template = frame.call_template.clone().expect("call template attached");
        let submission = CallSubmission {
            session_id: SessionId::new(template.session_id.clone()).expect("session id"),
            run_id: Some(Id::new(template.run_id.clone()).expect("run id")),
            state_digest: Some(template.state_digest.clone()),
            expected_sequence_item_id: template.expected_sequence_item_id.clone(),
            expected_run_index: Some(template.expected_run_index),
            expected_source_index: template.expected_source_index,
            expected_position_path: template.expected_position_path.clone(),
            produced_slots: [(slot_ref.to_string(), value)].into_iter().collect(),
            signals: Default::default(),
            warnings: Vec::new(),
            command_execution: None,
            caller: Some(CallerProvenance {
                surface: "test".to_string(),
                caller: "0085-nested-field-path".to_string(),
                agent: Some("worker".to_string()),
                harness: None,
            }),
        };
        submit_run_call(trait_ref, session, submission).expect("call is accepted")
    }

    #[test]
    fn project_lifts_a_nested_field_and_a_nested_guard_gates_the_following_step() {
        let trait_ref = fixture_trait();
        let session = start_session(&trait_ref);

        let after_produce = submit_current(
            &trait_ref,
            session,
            "slot:hook-output",
            serde_json::json!({ "decision": { "behavior": "approve" } }),
        );
        assert_eq!(
            after_produce.response_kind,
            CallResponseKind::AcceptedNextFrame
        );

        let lifted = after_produce
            .session
            .ledger
            .accepted_slot_values
            .iter()
            .find(|value| value.ref_text == "slot:behavior")
            .expect("project step lifted the nested field")
            .value
            .clone();
        assert_eq!(lifted, serde_json::json!("approve"));

        let completed = submit_current(
            &trait_ref,
            after_produce.session,
            "slot:final",
            serde_json::json!("done"),
        );
        assert_eq!(completed.response_kind, CallResponseKind::AcceptedCompleted);
        assert_eq!(completed.session.status, Status::Completed);

        // Every transition re-derived the recorded nested-field operand and
        // agreed with it (`build_session` runs this on every call already);
        // re-run it explicitly here as the task's named check.
        let report = crate::procedure::runtime::validate_run_ledger_contract(
            &trait_ref,
            &completed.session.ledger,
        )
        .expect("ledger contract check runs");
        assert!(
            report.diagnostics.is_empty(),
            "recorded and re-derived nested-field operands must agree: {:?}",
            report.diagnostics
        );

        // Serialize/resume cycle: the ledger round-trips through JSON and
        // the nested-field operand still re-derives cleanly afterward.
        let serialized = serde_json::to_string(&completed.session.ledger).expect("serialize");
        let resumed: crate::procedure::runtime::State =
            serde_json::from_str(&serialized).expect("deserialize");
        let resumed_report =
            crate::procedure::runtime::validate_run_ledger_contract(&trait_ref, &resumed)
                .expect("ledger contract check runs after resume");
        assert!(
            resumed_report.diagnostics.is_empty(),
            "nested-field operand must still re-derive cleanly after a resume: {:?}",
            resumed_report.diagnostics
        );
    }
}
