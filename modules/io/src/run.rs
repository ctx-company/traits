//! Shared run/session orchestration for CLI and MCP adapters.
//!
//! Core owns the pure runtime transitions. This module owns the IO needed around
//! those transitions: trait file resolution/loading, session persistence,
//! declared resource evidence, and trusted local command-frame execution.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

/// A synchronous notification emitted while a run session is being prepared.
/// Adapters may render it, but it never changes orchestration semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StartupStage {
    Initialization,
    Trust,
    Harness,
    Worktree,
    Seeding,
    Warm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStageState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StartupUpdate {
    pub stage: StartupStage,
    pub state: StartupStageState,
    pub detail: String,
}

pub type StartupObserver = std::sync::Arc<dyn Fn(StartupUpdate) + Send + Sync>;

/// Worktree's progress callback intentionally remains textual for existing
/// line-mode callers. Keep its translation here exact so a setup command that
/// merely mentions "seed" or "warm" cannot change the active startup row.
fn startup_stage_for_worktree_phase(phase: &str) -> StartupStage {
    match phase {
        "seeding" => StartupStage::Seeding,
        phase if phase.starts_with("warming ") || phase.starts_with("warm validation ") => {
            StartupStage::Warm
        }
        _ => StartupStage::Worktree,
    }
}

/// Default-input command wall-clock ceiling, used when a port's
/// `default.command` declares no `timeout-ms`.
const DEFAULT_INPUT_TIMEOUT_MS: u64 = 120_000;

/// Default-input command capture ceiling, used when a port's `default.command`
/// declares no `capture-bytes`. Raised from the former silent 16 KiB (which
/// had no truncation check at all) to match [`COMMAND_CAPTURE_LIMIT`] so
/// fail-closed-on-truncation does not immediately break default commands
/// (`git status`, plan greps) that are merely chatty rather than pathological.
const DEFAULT_INPUT_CAPTURE_LIMIT: usize = 262_144;

/// Command-step capture ceiling, used when a command item declares no
/// `capture-bytes`. Command steps may emit structured payloads (annotation
/// JSON, tool reports) far larger than terminal-style output; 256 KiB keeps
/// them intact without letting a runaway stream flood the slot ledger. A
/// capture that exceeds the effective limit on a slot-feeding route
/// (Text/Typed/Envelope) fails the step rather than landing a truncated
/// value.
/// Raised 262_144 -> 327_680 on 2026-07-28 (owner: "roughly 100k tokens").
/// At ~3.5 chars/token that is ~94k tokens of captured stdout — a ceiling on
/// what a single command may forward into a slot, and therefore into a frame.
/// Note this is NOT the binding limit for a frame: `[drive] inline-prompt-bytes`
/// caps the assembled prompt, and a capture that fits here can still overflow
/// there. Truncation on a slot-feeding command is a typed failure, never a
/// silent trim (see `stdout_truncated` handling below).
pub const COMMAND_CAPTURE_LIMIT: usize = 327_680;

/// Entropy for clean-run session/run ids: wall clock + pid + an in-process
/// counter, so concurrent and rapid starts never collide.
fn fresh_identity_seed(trait_id: &str) -> String {
    static START_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}|{}|{}|{trait_id}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
        std::process::id(),
        START_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LoadedTrait {
    pub trait_ref: ctx_traits_core::Trait,
    pub trait_root: Utf8PathBuf,
    pub path: Utf8PathBuf,
    pub source_kind: String,
    pub source_digest: String,
    pub canonical_digest: String,
}

/// Read-only relationship between a recorded source path and the bytes the
/// session committed to run. It is display evidence only; replay always uses
/// a valid pinned document first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraitSourceDrift {
    Current,
    Rebuilt {
        current_source_digest: String,
    },
    Missing,
    /// A document was recorded, but it cannot prove the session's committed
    /// identity and digests. This is distinct from a pre-pinning ledger.
    UnrecoverableInvalidPin {
        current_source_digest: Option<String>,
        recorded_source_digest: Option<String>,
    },
    UnrecoverableLegacy {
        current_source_digest: Option<String>,
        recorded_source_digest: Option<String>,
    },
}

impl TraitSourceDrift {
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Current => None,
            Self::Rebuilt {
                current_source_digest,
            } => Some(format!(
                "trait source rebuilt to {current_source_digest}; pinned session bytes remain resumable"
            )),
            Self::Missing => Some(
                "trait source path disappeared; pinned session bytes remain resumable".to_string(),
            ),
            Self::UnrecoverableInvalidPin {
                current_source_digest,
                recorded_source_digest,
            } => {
                let recorded = recorded_source_digest.as_deref().unwrap_or("unknown");
                Some(match current_source_digest {
                    Some(digest) => format!(
                        "trait source rebuilt from {recorded} to {digest}; pinned session document is malformed or does not match the recorded digests and is unrecoverable — resume with `--file <path>` against bytes matching {recorded}, or start a fresh run and harvest this session's worktree"
                    ),
                    None => format!(
                        "trait source path disappeared; pinned session document (recorded digest {recorded}) is malformed or does not match the recorded digests and is unrecoverable — resume with `--file <path>` against bytes matching {recorded}, or start a fresh run and harvest this session's worktree"
                    ),
                })
            }
            Self::UnrecoverableLegacy {
                current_source_digest,
                recorded_source_digest,
            } => {
                let recorded = recorded_source_digest.as_deref().unwrap_or("unknown");
                Some(match current_source_digest {
                    Some(digest) => format!(
                        "trait source rebuilt from {recorded} to {digest}; legacy session has no pinned document and is unrecoverable — resume with `--file <path>` against bytes matching {recorded}, or start a fresh run and harvest this session's worktree"
                    ),
                    None => format!(
                        "trait source path disappeared; legacy session (recorded digest {recorded}) has no pinned document and is unrecoverable — resume with `--file <path>` against bytes matching {recorded}, or start a fresh run and harvest this session's worktree"
                    ),
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResourceEvidenceMode<'a> {
    ReadDeclared { root_override: Option<&'a str> },
    Unavailable { reason: &'a str },
}

pub struct StartRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub query: Option<&'a str>,
    pub trait_args: &'a [String],
    pub input_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
    pub out: Option<&'a str>,
    pub session_store: Option<&'a str>,
    pub ephemeral: bool,
    pub resource_evidence: ResourceEvidenceMode<'a>,
    pub assign_overrides: &'a [String],
    pub agent_assignments: Option<Vec<ctx_traits_core::procedure::session::AgentAssignment>>,
    pub provider_capability_reports: Vec<ctx_traits_core::response::CapabilityReport>,
    pub provider_warnings: Vec<String>,
    pub harness_probes: Vec<ctx_traits_core::procedure::session::HarnessProbeEvidence>,
    pub caller: ctx_traits_core::procedure::session::CallerProvenance,
    pub state_source: &'a str,
    pub trait_arg_evidence: &'a str,
    /// Raw `--worktree[=<name>]` flag: `None` (absent, unchanged default
    /// behavior), `Some(None)` (bare flag, derive an id from the minted
    /// session id), or `Some(Some(name))` (explicit id).
    pub worktree: Option<Option<&'a str>>,
    /// Leave leading command frames unexecuted so a conductor can surface
    /// them as visible in-progress steps before running them. Non-conductor
    /// callers keep the synchronous drain.
    pub defer_commands: bool,
    /// Narrate init phase boundaries (worktree creation, seeding, warm clone,
    /// setup commands) to stderr while they run. The work between invocation
    /// and the first frame takes tens of seconds on a big repository and no
    /// panel can exist yet — a session does not — so without this the terminal
    /// is blank and "slow" is indistinguishable from "hung". CLI-owned:
    /// `--json` callers and MCP hosts leave it false and stay silent.
    pub narrate_progress: bool,
    /// Optional structured startup progress sink. Line-oriented callers leave
    /// this unset and retain `narrate_progress` exactly as before.
    pub startup_observer: Option<StartupObserver>,
    /// User strictness override: every loop stops the run blocked when its
    /// exit condition never matched, regardless of its declared
    /// `on-exhausted` policy or signals — a continuing loop's declared
    /// signals are not emitted either, since the loop did not continue.
    pub strict_loops: bool,
    /// `--override-dependencies`: dispatch a task with an unmet `depends-on`
    /// anyway. Recorded in provenance rather than left silent — see
    /// [`ctx_traits_core::procedure::session::DependencyOverrideProvenance`].
    pub override_dependencies: bool,
    /// `--task-dispatch`: bind the `task` input through the trait's declared
    /// `task-board` resource, with the wall/closed-status/dependency
    /// preflights and worktree materialisation that binding carries. Set by
    /// the `[tasks] dispatch-trait` flow (the dashboard TASKS screen's
    /// dispatch seed); without it a `task` input is plain text like any
    /// other port value — task binding is never inferred from a trait id or
    /// port name.
    pub task_dispatch: bool,
    /// P460 resolved automatic-landing intent, already validated by the
    /// caller against an effective worktree. Written straight into this
    /// session's initial persisted `Provenance` (never a post-start ledger
    /// mutation) so a globally discoverable ledger is never briefly missing
    /// its requested landing intent. `None` for every caller that never
    /// requests automatic landing.
    pub merge_rung: Option<ctx_traits_core::procedure::session::MergeRung>,
}

#[derive(Debug, Clone)]
pub struct StartOutcome {
    pub session: ctx_traits_core::procedure::session::Session,
    pub session_path: Option<Utf8PathBuf>,
    pub resource_supported: bool,
    /// Prepared worktree execution directory, when `--worktree` was
    /// requested. Operational data only: never part of the core session,
    /// provenance, canonical TOML/JSON, digests, or any serialized report.
    pub execution_dir: Option<Utf8PathBuf>,
}

#[derive(Debug)]
pub struct InspectRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    /// Cumulative active-drive elapsed seconds observed by the caller for
    /// this invocation. Merged into the ledger (monotonically) before any
    /// guard-evaluating transition. Plain inspection with no fresh
    /// measurement (e.g. `ctx traits run status` outside a drive loop)
    /// passes `None` and accrues no time.
    pub elapsed_seconds: Option<u64>,
}

pub struct AdvanceCommandsRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    pub execution_dir: Option<&'a Utf8Path>,
    /// Resolved `[worktree].env` overlay for the command frames advanced by
    /// this request. Empty for non-worktree (host-side) advancement, in which
    /// case behavior is byte-identical to a run with no overlay. Re-resolved
    /// per drive/resume by the caller rather than persisted into the ledger.
    pub execution_env: &'a BTreeMap<String, String>,
    /// See [`InspectRequest::elapsed_seconds`].
    pub elapsed_seconds: Option<u64>,
    /// Polled while a command frame blocks, so a live `--progress tui` pane
    /// keeps detach input responsive. `None` for every non-interactive caller.
    pub tick_observer: Option<crate::harness::TickObserver>,
}

#[derive(Debug, Clone)]
pub struct InspectOutcome {
    pub session: ctx_traits_core::procedure::session::Session,
    pub resource_supported: bool,
    /// Set when command-frame advancement stopped on a failing command step;
    /// the session is persisted at that frame and the drive can retry it.
    pub command_failure: Option<CommandStepFailure>,
}

/// The command bound that killed a step (0058/0066.4), named by its config
/// key so a report can say "command-idle-seconds (2s)" instead of a bare
/// "timed out". Distinct from the frame/total/max-frames bounds drive stamps
/// on `DriveReport`, so a command kill and a frame kill can never read alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundFired {
    pub name: &'static str,
    pub millis: u64,
    pub kind: crate::command::TimeoutKind,
}

impl BoundFired {
    /// Render as the report vocabulary uses it: `command-idle-seconds (2s)`.
    pub fn describe(&self) -> String {
        format!("{} ({}s)", self.name, self.millis / 1_000)
    }
}

/// Evidence from a command step that failed while advancing the controlled run.
#[derive(Debug, Clone)]
pub struct CommandStepFailure {
    pub item_id: Option<String>,
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub report: String,
    /// Which named bound fired, when `timed_out`. `None` for a non-timeout
    /// failure (the command ran to completion and exited non-zero).
    pub bound_fired: Option<BoundFired>,
}

pub struct CallRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    pub submission: ctx_traits_core::procedure::session::CallSubmission,
    pub out: Option<&'a str>,
    pub execution_dir: Option<&'a Utf8Path>,
    /// Resolved `[worktree].env` overlay for command frames advanced after
    /// this call is accepted. Empty for non-worktree runs (byte-identical to
    /// no overlay). Re-resolved per drive/resume, never persisted.
    pub execution_env: &'a BTreeMap<String, String>,
    /// See [`InspectRequest::elapsed_seconds`].
    pub elapsed_seconds: Option<u64>,
    /// Polled while a trailing command frame blocks, so a live `--progress
    /// tui` pane keeps detach input responsive. `None` for every
    /// non-interactive caller.
    pub tick_observer: Option<crate::harness::TickObserver>,
}

#[derive(Debug, Clone)]
pub struct CallOutcome {
    pub response: ctx_traits_core::procedure::session::CallResponse,
    pub resource_supported: bool,
    /// Set when the acceptance advanced into a command frame whose command
    /// failed; the acceptance itself is already persisted.
    pub command_failure: Option<CommandStepFailure>,
    /// The session ledger path this call read from and (when persisted)
    /// wrote to (P421): the receipt evidence a CLI JSON projection reports
    /// without re-deriving machine-state layout.
    pub session_path: Utf8PathBuf,
}

#[derive(Debug)]
pub struct SetRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    pub target: &'a str,
    pub value: Value,
    pub out: Option<&'a str>,
    pub caller: ctx_traits_core::procedure::session::CallerProvenance,
    pub existing_input_evidence: &'a str,
}

#[derive(Debug, Clone)]
pub enum SetOutcome {
    Session {
        session: Box<ctx_traits_core::procedure::session::Session>,
        resource_supported: bool,
    },
    Call {
        response: Box<ctx_traits_core::procedure::session::CallResponse>,
        resource_supported: bool,
    },
}

#[derive(Debug, Clone)]
pub enum RunInfoOutcome {
    Summary {
        summary: Box<ctx_traits_core::run_info::RunInfoSummary>,
        roles: Vec<String>,
        /// P451: the loaded trait plus its root, so a caller building
        /// dispatch reminders can resolve variant-qualified assignments via
        /// [`crate::harness_config::resolve_trait_runtime_assignments`]
        /// instead of the trait-blind [`crate::harness_config::resolve_runtime_assignments`].
        /// Every `Summary` construction site loads a trait to build it, so
        /// this is never absent — a non-optional field, not `Option`.
        trait_context: (Box<ctx_traits_core::Trait>, Utf8PathBuf),
    },
    Selection(RunInfoSelectionOutput),
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunInfoSelectionOutput {
    pub capabilities: Vec<ctx_traits_core::response::CapabilityReport>,
    pub selection: ctx_traits_core::run_info::RunInfoSelectionSummary,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn start(request: StartRequest<'_>) -> crate::Result<StartOutcome> {
    const PRE_AUTHORIZATION_FAILURE: &str = "trait authorization was refused";
    let pre_authorization = request.startup_observer.is_some();
    let generic_pre_authorization_error =
        |field| invalid_request_error(field, PRE_AUTHORIZATION_FAILURE);
    let update = |stage, state, detail: String| {
        if let Some(observer) = &request.startup_observer {
            observer(StartupUpdate {
                stage,
                state,
                detail,
            });
        }
    };
    update(
        StartupStage::Initialization,
        StartupStageState::Running,
        "loading trait".to_string(),
    );
    let mut selected_query = None;
    let loaded = if let Some(query) = request.query {
        if request.trait_file.is_some() || request.trait_id.is_some() {
            let message = invalid_request(
                "run.query",
                "query run is only accepted when no trait file or trait ID is supplied",
            );
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                "query run is only accepted when no trait file or trait ID is supplied".to_string(),
            );
            return message;
        }
        if request.trait_args.iter().any(|arg| arg.starts_with("--")) {
            let message = invalid_request(
                "run.query",
                "query run expects query text after --, not trait arguments; pass a trait ID or --file for trait arguments",
            );
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                "query run expects query text after --, not trait arguments; pass a trait ID or --file for trait arguments".to_string(),
            );
            return message;
        }
        let context = crate::inventory::InventoryContext::discover()
            .inspect_err(|_error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    "could not inspect trait inventory before authorization".to_string(),
                );
            })
            .map_err(|error| {
                if pre_authorization {
                    generic_pre_authorization_error("run.query")
                } else {
                    error
                }
            })?;
        let selection = crate::run_query::select(query, &context)
            .inspect_err(|_error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    "could not select a trait before authorization".to_string(),
                );
            })
            .map_err(|error| {
                if pre_authorization {
                    generic_pre_authorization_error("run.query")
                } else {
                    error
                }
            })?;
        // Inventory selection decodes every candidate. Its warnings describe
        // untrusted documents, so discard them before evaluating any outcome;
        // keep capture active for warnings from work authorized below.
        if pre_authorization {
            let _ = crate::decode_diagnostics::end_capture();
            crate::decode_diagnostics::begin_capture();
        }
        if selection.status != ctx_traits_core::run_info::RunInfoSelectionStatus::Selected {
            let gate_detail =
                ctx_traits_core::run_info::selection_refusal_detail(&selection.selection);
            let message = format!(
                "query run did not select exactly one runnable trait ({:?}){}",
                selection.status, gate_detail
            );
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                "query did not select an authorized trait".to_string(),
            );
            return if pre_authorization {
                invalid_request("run.query", PRE_AUTHORIZATION_FAILURE)
            } else {
                invalid_request("run.query", message)
            };
        }
        selected_query = Some(selection.selection.clone());
        let loaded = selection.loaded.ok_or_else(|| {
            let error = invalid_request_error(
                "run.query",
                "query selection did not include selected trait",
            );
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                PRE_AUTHORIZATION_FAILURE.to_string(),
            );
            if pre_authorization {
                generic_pre_authorization_error("run.query")
            } else {
                error
            }
        })?;
        LoadedTrait {
            trait_ref: loaded.trait_ref,
            trait_root: loaded.trait_root,
            path: loaded.path,
            source_kind: loaded.source_kind,
            source_digest: loaded.source_digest,
            canonical_digest: loaded.canonical_digest,
        }
    } else {
        load_trait_source(request.trait_file, request.trait_id, "run")
            .inspect_err(|_error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    // The document has not passed authorization yet. Keep the
                    // startup surface generic; the normal returned error remains
                    // available after the terminal is restored.
                    "could not load trait before authorization".to_string(),
                );
            })
            .map_err(|error| {
                if pre_authorization {
                    generic_pre_authorization_error("run.trait-source")
                } else {
                    error
                }
            })?
    };
    update(
        StartupStage::Trust,
        StartupStageState::Running,
        "checking lifecycle and trust".to_string(),
    );
    let authorization = crate::lifecycle::authorize_start(
        &loaded.trait_root,
        loaded.trait_ref.id.as_str(),
        &loaded.canonical_digest,
    )
    .inspect_err(|_error| {
        update(
            StartupStage::Trust,
            StartupStageState::Failed,
            "trait authorization could not be checked".to_string(),
        );
    })
    .map_err(|error| {
        if pre_authorization {
            generic_pre_authorization_error("run-session.lifecycle-trust")
        } else {
            error
        }
    })?;
    let gates = ctx_traits_core::r#trait::activation::lifecycle_trust_gates_for_check(
        loaded.trait_ref.id.as_str(),
        &authorization.status,
        &authorization.trust,
    );
    if !gates.is_empty() {
        update(
            StartupStage::Trust,
            StartupStageState::Failed,
            "trait authorization was refused".to_string(),
        );
        // The trust gate names the real condition — unreviewed/blocked trust,
        // the offending digest, and the exact approve command — instead of
        // the generic `invalid manifest at run-session.lifecycle-trust`
        // wrapping `invalid_request` produced. `<ref>` is the user-supplied
        // `trait_id`/`--file` spelling, never the decoded document's id: a
        // deliberate, narrow loosening of the pre-authorization generic-error
        // rule for this one gate, using only caller-supplied text plus a
        // computed digest — nothing document-derived (run.rs:543-546).
        let is_trust_gate = |gate: &ctx_traits_core::r#trait::activation::Gate| {
            gate.code == ctx_traits_core::r#trait::gate_code::TRUST_BLOCKED
                || gate.code == ctx_traits_core::r#trait::gate_code::TRUST_UNREVIEWED
        };
        if gates.iter().any(is_trust_gate) {
            let ref_display = request.trait_id.or(request.trait_file).unwrap_or("<trait>");
            let mut message = match &authorization.decision {
                crate::trust::StartTrust::Unreviewed => format!(
                    "trait trust refused: {ref_display} (digest {}) is unreviewed on this machine; review and approve with `ctx traits trust approve {ref_display}`",
                    loaded.canonical_digest
                ),
                crate::trust::StartTrust::Blocked(record) => format!(
                    "trait trust refused: {ref_display} (digest {}) is blocked{}; review with `ctx traits trust list`",
                    loaded.canonical_digest,
                    record
                        .reason
                        .as_deref()
                        .map(|reason| format!(" ({reason})"))
                        .unwrap_or_default()
                ),
                crate::trust::StartTrust::Verified(_) => format!(
                    "executable run blocked by lifecycle/trust gates: {}",
                    ctx_traits_core::r#trait::activation::format_gate_refusal(&gates)
                ),
            };
            let other_gates: Vec<_> = gates
                .iter()
                .filter(|gate| !is_trust_gate(gate))
                .cloned()
                .collect();
            if !other_gates.is_empty() {
                message.push_str(&format!(
                    "; also blocked by {}",
                    ctx_traits_core::r#trait::activation::format_gate_refusal(&other_gates)
                ));
            }
            return Err(crate::Error::Usage { message });
        }
        let message = format!(
            "executable run blocked by lifecycle/trust gates: {}",
            ctx_traits_core::r#trait::activation::format_gate_refusal(&gates)
        );
        return if pre_authorization {
            invalid_request("run-session.lifecycle-trust", PRE_AUTHORIZATION_FAILURE)
        } else {
            invalid_request("run-session.lifecycle-trust", message)
        };
    }
    update(
        StartupStage::Trust,
        StartupStageState::Done,
        "approved".to_string(),
    );

    // A `worktreeRequired` procedure can never actually be run outside a Git
    // repository (worktree preparation itself requires one): refuse it here,
    // before any session/run id is minted or worktree prepared, with a
    // direct repository-requirement error rather than reaching the generic
    // worktree/session failure path partway through setup (P439).
    if loaded
        .trait_ref
        .procedure
        .as_ref()
        .is_some_and(|procedure| procedure.worktree_required)
        && matches!(
            crate::state::discover_invocation_root().inspect_err(|error| {
                update(
                    StartupStage::Worktree,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?,
            crate::state::InvocationRoot::Adhoc(_)
        )
    {
        let message = format!(
            "trait {:?} requires a prepared worktree, which requires a Git repository; run inside a Git checkout",
            loaded.trait_ref.id
        );
        update(
            StartupStage::Worktree,
            StartupStageState::Failed,
            message.clone(),
        );
        return invalid_request("run.worktree", message);
    }

    // Trait-argument parsing happens before assignment preparation so
    // `port:task` is available downstream.
    update(
        StartupStage::Harness,
        StartupStageState::Running,
        "validating run inputs".to_string(),
    );
    let mut initial_values = request.input_values;
    if request.query.is_none() {
        initial_values.extend(
            ctx_traits_core::run_info::parse_trait_arguments(
                &loaded.trait_ref,
                request.trait_args,
                request.trait_arg_evidence,
            )
            .inspect_err(|error| {
                update(
                    StartupStage::Harness,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?,
        );
    }
    if let Some(query_selection) = selected_query
        .as_ref()
        .and_then(|selection| selection.query.as_ref())
        && let Some(value) =
            ctx_traits_core::run_info::query_text_initial_value(&loaded.trait_ref, query_selection)
    {
        initial_values.push(value);
    }

    // Every caller-supplied initial value targeting a port must name a
    // DECLARED INPUT port. `--set phase=…` against a trait whose port had
    // been renamed to `task` used to be accepted and silently dropped — the
    // run started with no inputs and hung awaiting-input (ctx-gate,
    // 2026-07-31). The trait-args path (`-- --port=value`) already refuses
    // unknown ports with the accepted list; this closes the `--set` path
    // the same way. Slot-qualified refs (`slot:x`) are untouched.
    let declared_input_ports: BTreeSet<&str> = loaded
        .trait_ref
        .ports
        .iter()
        .filter(|port| {
            matches!(
                port.direction,
                ctx_traits_core::r#trait::PortDirection::Input
            )
        })
        .map(|port| port.id.as_str())
        .collect();
    for value in &initial_values {
        if let Some(port_id) = value.ref_text.strip_prefix("port:")
            && !declared_input_ports.contains(port_id)
        {
            let message = format!(
                "unknown input port {port_id:?}; trait {} declares input port(s): {}",
                loaded.trait_ref.id,
                if declared_input_ports.is_empty() {
                    "(none)".to_string()
                } else {
                    declared_input_ports
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            update(
                StartupStage::Harness,
                StartupStageState::Failed,
                message.clone(),
            );
            return invalid_request("run.set", message);
        }
    }

    // Task binding is OPT-IN per dispatch (`--task-dispatch`, set by the
    // `[tasks] dispatch-trait` flow): only then is the `task` input read as
    // a task-board key and put through the wall/closed-status/dependency
    // preflights and worktree materialisation below. On every other
    // dispatch a `task` input is plain text with whatever semantics the
    // trait gives it — binding is never inferred from a trait id or port
    // name.
    let owned_task_value = request
        .task_dispatch
        .then(|| {
            crate::run_session::task_value_from_pairs(
                initial_values
                    .iter()
                    .map(|value| (value.ref_text.as_str(), &value.value)),
            )
        })
        .flatten();
    if request.task_dispatch && owned_task_value.is_none() {
        let message =
            "--task-dispatch requires a task value (--set task=<key>) to bind".to_string();
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            message.clone(),
        );
        return invalid_request("run.task", message);
    }
    let task_value = owned_task_value.as_deref();
    // 0061: resolve the dispatched task through the `TaskProvider` interface
    // exactly once — the wall (P414), closed-status, and dependency
    // preflights below, plus this run's later worktree materialisation, all
    // read the SAME document, so an edit landing mid-dispatch can never
    // split "what was checked" from "what was run".
    let dispatch_resolution = crate::dispatch_preflight::resolve_dispatch_task(
        &loaded.trait_ref,
        &loaded.trait_root,
        task_value,
    )
    .inspect_err(|error| {
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    // 0063.4: a requested task dispatch whose `task=` cannot bind through
    // the trait's `task-board` resource refuses — the same rule the
    // dashboard's TASKS screen enforces (0061), enforced here too since a
    // check that only lives in the UI is bypassed by the bare CLI.
    let dispatch_task = match dispatch_resolution {
        crate::dispatch_preflight::DispatchTaskResolution::NotRequested => None,
        crate::dispatch_preflight::DispatchTaskResolution::Bound(task) => Some(*task),
        crate::dispatch_preflight::DispatchTaskResolution::CannotBind { trait_id, reason } => {
            let message =
                crate::dispatch_preflight::cannot_bind_refusal_message(&trait_id, &reason);
            update(
                StartupStage::Harness,
                StartupStageState::Failed,
                message.clone(),
            );
            return invalid_request("run.task", message);
        }
    };

    if let Some(task) = dispatch_task.as_ref()
        && let Some(wall_id) = crate::dispatch_preflight::explicit_wall_id(task)
        && let Some(standing) =
            crate::dispatch_preflight::find_standing_wall(&wall_id, task_value.unwrap_or_default())
                .inspect_err(|error| {
                    update(
                        StartupStage::Harness,
                        StartupStageState::Failed,
                        error.to_string(),
                    );
                })?
    {
        let message = crate::dispatch_preflight::refusal_message(&standing);
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            message.clone(),
        );
        return invalid_request("run.task", message);
    }

    // Closed-status pre-flight (0047 mechanism 1's companion layer, rewired
    // onto the typed task document by 0059): refuse to dispatch an
    // `implement-*` task whose own task document's stored `status` field is
    // `done` or `cancelled` — a direct typed-field read, never a
    // derivation. Unmet-dependency (`blocked`) refusal is the dependency
    // preflight below. Fails open like the wall preflight above.
    if let Some(marker) = dispatch_task
        .as_ref()
        .and_then(crate::dispatch_preflight::closed_status_marker)
    {
        let message = crate::dispatch_preflight::closed_status_refusal_message(&marker);
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            message.clone(),
        );
        return invalid_request("run.task", message);
    }

    // Dependency pre-flight (0061): refuse to dispatch a task whose own
    // `depends-on` is unmet, in the same preflight family as the wall and
    // closed-status checks above — enforcement lives at dispatch, not in
    // the dashboard, since a check that only exists in the UI is bypassed
    // by the CLI. `--override-dependencies` dispatches anyway and leaves a
    // typed trace in provenance plus a human-readable warning.
    let mut preflight_warnings: Vec<String> = Vec::new();
    let mut dependency_override: Option<
        ctx_traits_core::procedure::session::DependencyOverrideProvenance,
    > = None;
    if let Some(task) = dispatch_task.as_ref()
        && let Some(unmet) = crate::dispatch_preflight::dependency_marker(task)
    {
        let message = crate::dispatch_preflight::dependency_refusal_message(
            &task.resolved.document.key,
            &unmet,
        );
        if request.override_dependencies {
            preflight_warnings.push(format!("dependency override recorded: {message}"));
            dependency_override = Some(
                ctx_traits_core::procedure::session::DependencyOverrideProvenance {
                    task_key: task.resolved.document.key.clone(),
                    unmet: unmet
                        .iter()
                        .map(
                            |dep| ctx_traits_core::procedure::session::UnmetDependencyEvidence {
                                key: dep.key.clone(),
                                status: crate::dispatch_preflight::status_word(dep.status)
                                    .to_string(),
                            },
                        )
                        .collect(),
                },
            );
        } else {
            update(
                StartupStage::Harness,
                StartupStageState::Failed,
                message.clone(),
            );
            return invalid_request("run.task", message);
        }
    }

    // Unrunnable-command pre-flight: a command step's argv lives in the trait,
    // so no agent can repair one this repository cannot execute. Refuse now
    // rather than let every round fail identically while the reviewer blames
    // the work for it.
    // Resolved here rather than reused from `roots`, which is built later, at
    // resource resolution. A run outside any repository simply skips the
    // check — it fails open, like every other rule in this preflight.
    let preflight_repo_root = crate::repository::discover_repo_root().ok();
    let unrunnable = match preflight_repo_root.as_deref() {
        Some(repo_root) => {
            crate::dispatch_preflight::unrunnable_check_commands(&loaded.trait_ref, repo_root)
        }
        None => Vec::new(),
    };
    if !unrunnable.is_empty() {
        let message = crate::dispatch_preflight::unrunnable_refusal_message(&unrunnable);
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            message.clone(),
        );
        return invalid_request("run.trait", message);
    }

    update(
        StartupStage::Harness,
        StartupStageState::Running,
        "probing configured harnesses".to_string(),
    );
    let prepared_assignments = crate::harness_config::prepare_run_assignments(
        &loaded.trait_ref,
        &loaded.trait_root,
        request.assign_overrides,
    )
    .inspect_err(|error| {
        update(
            StartupStage::Harness,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    update(
        StartupStage::Harness,
        StartupStageState::Done,
        "ready".to_string(),
    );
    let agent_assignments = match (request.agent_assignments, prepared_assignments.assignments) {
        (None, assignments) => assignments,
        (assignments @ Some(_), None) => assignments,
        (Some(_), Some(_)) => {
            let message = "agent assignments were supplied by both adapter request and resolved runtime configuration";
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                message.to_string(),
            );
            return invalid_request("run.agent-assignments", message);
        }
    };

    let mut provider_capability_reports = request.provider_capability_reports;
    provider_capability_reports.extend(prepared_assignments.capability_reports);
    let mut provider_warnings = request.provider_warnings;
    provider_warnings.extend(prepared_assignments.warnings);
    let mut harness_probes = request.harness_probes;
    harness_probes.extend(prepared_assignments.harness_probes);

    // Clean-run identity (owner decision, Group 42.5 F2): every start mints
    // fresh session/run ids. Deriving them from trait+inputs made re-runs
    // silently clobber parked sessions — a smoke run erased a paid mid-flight
    // run this way. `SessionId::deterministic` stays in core for a future
    // explicit opt-in; nothing calls it today. Identity is minted before any
    // worktree-dependent execution below, since a bare `--worktree` derives
    // its id from the session id.
    let unique_seed = fresh_identity_seed(loaded.trait_ref.id.as_str());
    let session_id = ctx_traits_core::procedure::session::SessionId::new(format!(
        "session-{}",
        ctx_traits_core::digest::Digest::source(&format!("session|{unique_seed}"))
            .as_str()
            .trim_start_matches("sha256:")
    ))
    .inspect_err(|error| {
        update(
            StartupStage::Initialization,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    let run_id = ctx_traits_core::procedure::run::Id::new(format!(
        "run-{}",
        ctx_traits_core::digest::Digest::source(&format!("run|{unique_seed}"))
            .as_str()
            .trim_start_matches("sha256:")
    ))
    .inspect_err(|error| {
        update(
            StartupStage::Initialization,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;

    // Prepare the dedicated worktree (if requested) before any default input
    // or command-frame subprocess runs, so their `exec_dir` is already known.
    // The prepared path itself stays out of the core session/provenance
    // (operational execution capability, returned only on `StartOutcome`), but
    // the id/branch are attached to provenance below so a later
    // `ctx traits merge <run-id>` can resolve back to this worktree.
    let mut worktree_retry_warnings: Vec<String> = Vec::new();
    // Effective worktree env overlay: only resolved (and only non-empty) when
    // a worktree is actually prepared and `.ctx/traits/runtime.toml [worktree]` declared one.
    // Repository-relative path values are resolved against the invocation
    // repository root, never the generated worktree. Empty otherwise, so
    // host-side runs stay byte-identical.
    let mut worktree_env: BTreeMap<String, String> = BTreeMap::new();
    let (execution_dir, worktree_provenance) = match request.worktree {
        Some(requested) => {
            update(
                StartupStage::Worktree,
                StartupStageState::Running,
                "creating worktree".to_string(),
            );
            let id = match requested {
                Some(name) => name.to_string(),
                None => crate::worktree::derive_worktree_id(session_id.as_str()),
            };
            // P564: resolved against THIS run's worktree path — derived from
            // the id before the worktree exists — so a `{worktree}`-scoped
            // build cache is per-run rather than shared with every concurrent
            // run in the same checkout.
            let planned_worktree_path =
                crate::worktree::worktree_path_for(&id).inspect_err(|error| {
                    update(
                        StartupStage::Worktree,
                        StartupStageState::Failed,
                        error.to_string(),
                    );
                })?;
            worktree_env = crate::harness_config::resolve_effective_worktree_env(
                &prepared_assignments.worktree,
                Some(planned_worktree_path.as_path()),
            )
            .inspect_err(|error| {
                update(
                    StartupStage::Worktree,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?;
            // The P551 observer already narrates every phase inside
            // worktree preparation ("creating worktree", "seeding", the warm
            // clone, each setup command); it was simply dropped here as
            // `None`, which is how a 25-second init produced one line of
            // output. Same stderr channel as the CLI's own
            // "ctx run · initialization" line, so the story reads as one.
            let narrate = |phase: &str| eprintln!("ctx run · {phase}");
            let observer = request.startup_observer.clone();
            let active_stage = Arc::new(Mutex::new(StartupStage::Worktree));
            let seen_stages = Arc::new(Mutex::new(BTreeSet::new()));
            let setup_parent_complete = Arc::new(Mutex::new(false));
            let progress_stage = Arc::clone(&active_stage);
            let progress_seen = Arc::clone(&seen_stages);
            let progress_setup_parent_complete = Arc::clone(&setup_parent_complete);
            let validation_stage = Arc::clone(&active_stage);
            let startup_warm_validation = |entry: &str| {
                if let Some(observer) = &observer {
                    if let Ok(mut active) = validation_stage.lock()
                        && *active != StartupStage::Warm
                    {
                        observer(StartupUpdate {
                            stage: *active,
                            state: StartupStageState::Done,
                            detail: "complete".to_string(),
                        });
                        *active = StartupStage::Warm;
                    }
                    observer(StartupUpdate {
                        stage: StartupStage::Warm,
                        state: StartupStageState::Running,
                        detail: format!("validating {entry}"),
                    });
                }
            };
            let startup_progress = |phase: &str| {
                if let Some(observer) = &observer {
                    let (stage, detail) = (startup_stage_for_worktree_phase(phase), phase);
                    if let Ok(mut active) = progress_stage.lock()
                        && *active != stage
                    {
                        // Setup runs after optional seed/warm operations. It
                        // belongs to the worktree preparation detail but must
                        // not reopen that already-completed parent row.
                        if stage == StartupStage::Worktree
                            && *active != StartupStage::Worktree
                            && phase.starts_with("setup")
                        {
                            // Setup is worktree preparation, but the parent
                            // row was completed when seeding/warming began.
                            // Keep it visually complete while making it the
                            // failure target for a setup command.
                            observer(StartupUpdate {
                                stage: *active,
                                state: StartupStageState::Done,
                                detail: "complete".to_string(),
                            });
                            *active = StartupStage::Worktree;
                            if let Ok(mut complete) = progress_setup_parent_complete.lock() {
                                *complete = true;
                            }
                            observer(StartupUpdate {
                                stage: StartupStage::Worktree,
                                state: StartupStageState::Done,
                                detail: detail.to_string(),
                            });
                            return;
                        }
                        observer(StartupUpdate {
                            stage: *active,
                            state: StartupStageState::Done,
                            detail: "complete".to_string(),
                        });
                        *active = stage;
                    }
                    // Once seed/warm has completed the parent Worktree row,
                    // later setup callbacks must only refine its detail. A
                    // Running update here would create a Done-to-Running
                    // transition in the shared startup pane.
                    if stage == StartupStage::Worktree
                        && phase.starts_with("setup")
                        && progress_setup_parent_complete
                            .lock()
                            .is_ok_and(|complete| *complete)
                    {
                        observer(StartupUpdate {
                            stage,
                            state: StartupStageState::Done,
                            detail: detail.to_string(),
                        });
                        return;
                    }
                    if let Ok(mut seen) = progress_seen.lock()
                        && !phase.starts_with("warm validation ")
                    {
                        seen.insert(stage);
                    }
                    observer(StartupUpdate {
                        stage,
                        state: StartupStageState::Running,
                        detail: detail.to_string(),
                    });
                }
            };
            let prepared = crate::worktree::prepare_worktree(
                &id,
                crate::worktree::WorktreeContents {
                    seeds: &prepared_assignments.worktree.seed,
                    warm: &prepared_assignments.worktree.warm,
                },
                crate::worktree::PrepareOptions {
                    setup: &prepared_assignments.worktree.setup,
                    setup_env: &worktree_env,
                    setup_timeout_ms: prepared_assignments
                        .worktree
                        .setup_seconds
                        .map(|s| s * 1000),
                    setup_capture_bytes: prepared_assignments.worktree.setup_capture_bytes,
                    worktree_add_timeout_ms: Some(
                        crate::harness_config::resolve_git_long_timeout_ms(Utf8Path::new(".")),
                    ),
                    progress: if request.narrate_progress {
                        Some(&narrate as &dyn Fn(&str))
                    } else if request.startup_observer.is_some() {
                        Some(&startup_progress as &dyn Fn(&str))
                    } else {
                        None
                    },
                    warm_validation: request
                        .startup_observer
                        .as_ref()
                        .map(|_| &startup_warm_validation as &dyn Fn(&str)),
                },
            )
            .inspect_err(|error| {
                if let Some(observer) = &request.startup_observer {
                    let stage = active_stage
                        .lock()
                        .map(|stage| *stage)
                        .unwrap_or(StartupStage::Worktree);
                    observer(StartupUpdate {
                        stage,
                        state: StartupStageState::Failed,
                        detail: error.to_string(),
                    });
                }
            })?;
            update(
                StartupStage::Worktree,
                StartupStageState::Done,
                "ready".to_string(),
            );
            update(
                StartupStage::Seeding,
                StartupStageState::Done,
                if seen_stages
                    .lock()
                    .is_ok_and(|seen| seen.contains(&StartupStage::Seeding))
                {
                    "complete".to_string()
                } else {
                    "not requested".to_string()
                },
            );
            update(
                StartupStage::Warm,
                StartupStageState::Done,
                if seen_stages
                    .lock()
                    .is_ok_and(|seen| seen.contains(&StartupStage::Warm))
                {
                    "complete".to_string()
                } else if !prepared_assignments.worktree.warm.is_empty() {
                    "configured; skipped".to_string()
                } else {
                    "not requested".to_string()
                },
            );
            worktree_retry_warnings = prepared.retry_warnings;
            let prepared_path = prepared.path.to_string();
            (
                Some(prepared.path),
                Some(ctx_traits_core::procedure::session::WorktreeProvenance {
                    id,
                    branch: prepared.branch,
                    seed_snapshots: prepared.seed_snapshots,
                    path: Some(prepared_path),
                }),
            )
        }
        None => {
            update(
                StartupStage::Worktree,
                StartupStageState::Done,
                "not requested".to_string(),
            );
            update(
                StartupStage::Seeding,
                StartupStageState::Done,
                "not requested".to_string(),
            );
            update(
                StartupStage::Warm,
                StartupStageState::Done,
                "not requested".to_string(),
            );
            (None, None)
        }
    };

    // 0061: materialise the dispatched task's CURRENT invocation-repository
    // working-tree bytes into the run's worktree (never HEAD — a task
    // amended but not yet committed must still be what the run executes)
    // and digest what was written, so "what was this run told to do" is
    // answerable from evidence alone. Only meaningful for `--worktree`
    // runs: a host-side run already executes against the working tree it
    // was just read from.
    let mut task_evidence: Option<(ctx_traits_core::digest::Digest, String)> = None;
    if let Some(task) = dispatch_task.as_ref()
        && let Some(current) = crate::dispatch_preflight::reread_current_document(task)
            .inspect_err(|error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?
    {
        let text = ctx_traits_core::task::serialize(&current.document)?;
        let digest = ctx_traits_core::digest::Digest::source(&text);
        if let Some(execution_dir) = execution_dir.as_deref() {
            match task
                .invocation_repo_root
                .as_deref()
                .and_then(|repo_root| task.board_dir.strip_prefix(repo_root).ok())
            {
                Some(relative_board_dir) => {
                    let target_dir = execution_dir.join(relative_board_dir);
                    if let Err(error) =
                        std::fs::create_dir_all(target_dir.as_std_path()).and_then(|()| {
                            std::fs::write(target_dir.join(&task.file_name).as_std_path(), &text)
                        })
                    {
                        preflight_warnings.push(format!(
                            "could not materialise task {} into the worktree: {error}",
                            current.document.key
                        ));
                    }
                }
                None => {
                    preflight_warnings.push(format!(
                        "task-board resource for task {} does not resolve under the invocation repository root; dispatch digested it but could not materialise it into the worktree",
                        current.document.key
                    ));
                }
            }
        }
        task_evidence = Some((digest, current.document.key));
    }

    provider_capability_reports.extend(
        apply_default_inputs(
            &loaded.trait_ref,
            &mut initial_values,
            &prepared_assignments.port_defaults,
            execution_dir.as_deref(),
            &worktree_env,
        )
        .inspect_err(|error| {
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                error.to_string(),
            );
        })?,
    );
    initial_values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));

    let resource_evidence = match request.resource_evidence {
        ResourceEvidenceMode::ReadDeclared { root_override } => {
            // Declared resource paths are package-jailed, so the trait package
            // root the loader already read trait.toml from is a safe default;
            // the override exists for out-of-package roots only.
            let resource_root = root_override
                .map(Utf8Path::new)
                .unwrap_or(loaded.trait_root.as_path());
            let roots =
                crate::resource::resolve_resource_roots(resource_root, &loaded.trait_ref.resources)
                    .inspect_err(|error| {
                        update(
                            StartupStage::Initialization,
                            StartupStageState::Failed,
                            error.to_string(),
                        );
                    })?;
            let mut evidence =
                declared_resource_evidence(&roots, &loaded.trait_ref).inspect_err(|error| {
                    update(
                        StartupStage::Initialization,
                        StartupStageState::Failed,
                        error.to_string(),
                    );
                })?;
            evidence.extend(
                declared_dependency_resource_evidence(
                    &loaded.trait_root,
                    roots.invocation_repo_root.as_deref(),
                    &loaded.trait_ref,
                )
                .inspect_err(|error| {
                    update(
                        StartupStage::Initialization,
                        StartupStageState::Failed,
                        error.to_string(),
                    );
                })?,
            );
            evidence.sort_by(|left, right| left.resource_ref.cmp(&right.resource_ref));
            evidence
        }
        ResourceEvidenceMode::Unavailable { reason } => {
            unavailable_resource_evidence(&loaded.trait_ref, reason).inspect_err(|error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?
        }
    };

    let session_path = session_output_path(
        request.out,
        request.session_store,
        request.ephemeral,
        session_id.as_str(),
    )
    .inspect_err(|error| {
        update(
            StartupStage::Initialization,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    // A relative selected source belongs to the checkout that resolved it,
    // not an arbitrary location chosen for the output ledger.
    let source_repository_root = loaded
        .path
        .parent()
        .and_then(|parent| {
            crate::repository::discover_repo_root_at(parent)
                .ok()
                .flatten()
        })
        .or_else(|| crate::repository::discover_repo_root().ok());
    // Store an absolute path so a later resume never reinterprets it from
    // another checkout. Do not canonicalize or collapse `..`: filesystem
    // traversal through a directory symlink changes what a following `..`
    // means, including its package context.
    let persisted_source_path = if loaded.path.is_relative() {
        let cwd = std::env::current_dir()
            .map_err(|source| crate::environment::Error::Filesystem {
                path: loaded.path.to_string(),
                source,
            })
            .inspect_err(|error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?;
        let cwd = Utf8PathBuf::from_path_buf(cwd)
            .map_err(|path| crate::environment::Error::Filesystem {
                path: path.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "current working directory is not valid UTF-8",
                ),
            })
            .inspect_err(|error| {
                update(
                    StartupStage::Initialization,
                    StartupStageState::Failed,
                    error.to_string(),
                );
            })?;
        cwd.join(&loaded.path)
    } else {
        loaded.path.clone()
    };
    let canonical_digest = ctx_traits_core::digest::Digest::parse(&loaded.canonical_digest)
        .inspect_err(|error| {
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                error.to_string(),
            );
        })?;
    let pinned_document = if request.ephemeral {
        None
    } else {
        // Re-read after selection so a file changed during start cannot be
        // pinned under the earlier digest.
        let text = crate::read::read_text(&loaded.path).inspect_err(|error| {
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                error.to_string(),
            );
        })?;
        if ctx_traits_core::digest::Digest::source(&text).as_str() != loaded.source_digest {
            let message = "trait source changed while starting; retry with stable source bytes";
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                message.to_string(),
            );
            return invalid_request("run.trait-source", message);
        }
        Some(text)
    };
    let trust_approval = authorization.approval.map(|record| {
        ctx_traits_core::procedure::session::TrustApprovalProvenance {
            trait_id: loaded.trait_ref.id.to_string(),
            canonical_digest: canonical_digest.clone(),
            seq: record.seq.unwrap_or(0),
            approved_at: record.updated_at,
        }
    });
    let mut session = ctx_traits_core::procedure::session::start_run_session(
        &loaded.trait_ref,
        &authorization.status,
        &authorization.trust,
        ctx_traits_core::procedure::session::StartRequest {
            session_id,
            run_id,
            initial_port_values: initial_values,
            resource_evidence,
            resolved_settings: prepared_assignments
                .resolved_settings
                .iter()
                .map(
                    |(id, resolved)| ctx_traits_core::procedure::runtime::ResolvedSettingRecord {
                        id: id.clone(),
                        value: resolved.value.clone(),
                        source: match resolved.source {
                            crate::harness_config::SettingSource::Declaration => {
                                ctx_traits_core::procedure::runtime::SettingSourceLayer::Declaration
                            }
                            crate::harness_config::SettingSource::Trait => {
                                ctx_traits_core::procedure::runtime::SettingSourceLayer::Trait
                            }
                            crate::harness_config::SettingSource::Variant => {
                                ctx_traits_core::procedure::runtime::SettingSourceLayer::Variant
                            }
                        },
                    },
                )
                .collect(),
            resolved_budgets: resolved_budget_records(
                &prepared_assignments.budget,
                &prepared_assignments.budget_provenance,
            ),
            provider_capability_reports,
            source_digest: Some(
                ctx_traits_core::digest::Digest::parse(&loaded.source_digest).inspect_err(
                    |error| {
                        update(
                            StartupStage::Initialization,
                            StartupStageState::Failed,
                            error.to_string(),
                        );
                    },
                )?,
            ),
            canonical_digest: Some(
                ctx_traits_core::digest::Digest::parse(&loaded.canonical_digest).inspect_err(
                    |error| {
                        update(
                            StartupStage::Initialization,
                            StartupStageState::Failed,
                            error.to_string(),
                        );
                    },
                )?,
            ),
            agent_assignments,
            provider_warnings,
            harness_probes,
            strict_loops: request.strict_loops,
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: request.caller,
                state_source: request.state_source.to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: {
                    let mut warnings = preflight_warnings;
                    warnings.extend(worktree_retry_warnings);
                    warnings
                },
                trait_source: Some(ctx_traits_core::procedure::session::TraitSource {
                    kind: loaded.source_kind,
                    path: persisted_source_path.to_string(),
                    repository_root: source_repository_root.map(|root| root.to_string()),
                    document: pinned_document,
                }),
                query_selection: selected_query,
                worktree: worktree_provenance,
                merge_frames: Vec::new(),
                merge_intent: request.merge_rung,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: Some(crate::run_liveness::epoch_secs()),
                trust_approval,
                session_title: None,
                task_digest: task_evidence.as_ref().map(|(digest, _)| digest.clone()),
                task_key: task_evidence.map(|(_, key)| key),
                dependency_override,
            },
        },
    )
    .inspect_err(|error| {
        update(
            StartupStage::Initialization,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    if !request.defer_commands {
        session = advance_command_frames(
            &loaded.trait_ref,
            &loaded.trait_root,
            session,
            session_path.as_deref(),
            execution_dir.as_deref(),
            &worktree_env,
            None,
        )
        .inspect_err(|error| {
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                error.to_string(),
            );
        })?
        .session;
    }
    if let Some(path) = session_path.as_ref() {
        crate::run_session::write_run_session(path, &session).inspect_err(|error| {
            update(
                StartupStage::Initialization,
                StartupStageState::Failed,
                error.to_string(),
            );
        })?;
    }
    // `checkouts.toml` is operational index evidence, not canonical ledger
    // state, but P426 requires it maintained on every accepted run —
    // ephemeral and explicit-`--out` runs included, not only ones that
    // write a default-path session ledger. An index failure (e.g. no HOME)
    // therefore fails the run rather than silently completing unindexed.
    crate::state::touch_repo_index().inspect_err(|error| {
        update(
            StartupStage::Initialization,
            StartupStageState::Failed,
            error.to_string(),
        );
    })?;
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &session.resource_evidence,
        );
    update(
        StartupStage::Initialization,
        StartupStageState::Done,
        "session created".to_string(),
    );
    Ok(StartOutcome {
        session,
        session_path,
        resource_supported,
        execution_dir,
    })
}

pub fn run_info(
    trait_file: Option<&str>,
    trait_id: Option<&str>,
    query: Option<&str>,
) -> crate::Result<RunInfoOutcome> {
    if let Some(query) = query {
        let context = crate::inventory::InventoryContext::discover()?;
        let selection = crate::run_query::select(query, &context)?;
        let Some(loaded) = selection.loaded else {
            return Ok(RunInfoOutcome::Selection(RunInfoSelectionOutput {
                capabilities: ctx_traits_core::run_info::run_info_capabilities(),
                selection: selection.selection,
            }));
        };
        return Ok(RunInfoOutcome::Summary {
            summary: Box::new(ctx_traits_core::run_info::summarize_run_info(
                &loaded.trait_ref,
                &loaded.status,
                &loaded.trust,
                Some(loaded.path.as_str()),
                Some(loaded.source_digest.as_str()),
                Some(loaded.canonical_digest.as_str()),
                Some(selection.selection),
            )),
            roles: loaded
                .trait_ref
                .agents
                .iter()
                .map(|agent| agent.id.clone())
                .collect(),
            trait_context: (Box::new(loaded.trait_ref), loaded.trait_root),
        });
    }
    let loaded = load_trait_source(trait_file, trait_id, "run-info")?;
    let (status, trust) = crate::lifecycle::resolve_named(
        &loaded.trait_root,
        loaded.trait_ref.id.as_str(),
        &loaded.canonical_digest,
    )?;
    Ok(RunInfoOutcome::Summary {
        summary: Box::new(ctx_traits_core::run_info::summarize_run_info(
            &loaded.trait_ref,
            &status,
            &trust,
            Some(loaded.path.as_str()),
            Some(loaded.source_digest.as_str()),
            Some(loaded.canonical_digest.as_str()),
            None,
        )),
        roles: loaded
            .trait_ref
            .agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect(),
        trait_context: (Box::new(loaded.trait_ref), loaded.trait_root),
    })
}

#[cfg(test)]
mod startup_observer_tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    const CHILD_TEST_ROOT: &str = "CTX_TRAITS_STARTUP_OBSERVER_TEST_ROOT";

    fn run_in_isolated_child() -> bool {
        if std::env::var_os(CHILD_TEST_ROOT).is_some() {
            return true;
        }

        // The timestamp alone is not unique: macOS quantizes `SystemTime::now()`
        // to microseconds, and every parent test here shares one pid, so two
        // test threads sampling the clock in the same tick once shared a root —
        // the faster child's cleanup deleted the slower child's fixtures
        // mid-test. The thread id makes the root unique within this process,
        // and `create_dir` turns any residual collision into a loud failure
        // instead of a silently shared directory.
        let root = std::env::temp_dir().join(format!(
            "ctx-traits-startup-observer-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let test_name = std::thread::current().name().unwrap().to_string();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .current_dir(&root)
            .env(CHILD_TEST_ROOT, &root)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .status()
            .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            status.success(),
            "isolated startup observer test failed: {test_name}"
        );
        false
    }

    fn child_test_root() -> std::path::PathBuf {
        std::env::var_os(CHILD_TEST_ROOT)
            .map(std::path::PathBuf::from)
            .expect("startup observer fixture must run in its isolated child")
    }

    struct StartupFixture {
        root: std::path::PathBuf,
        trait_path: String,
    }

    impl StartupFixture {
        fn approved() -> Self {
            let root = child_test_root();
            let generated = root.join(".ctx/traits/startup-fixture/generated");
            std::fs::create_dir_all(&generated).unwrap();
            let trait_path = generated.join("index.toml");
            std::fs::write(
                &trait_path,
                "id = \"startup-fixture\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"Startup fixture\"\nsummary = \"startup observer fixture\"\n\n[[port]]\nid = \"plan\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Plan path\"\n[port.default]\nvalue = \".plans/DECLARED.md\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
            )
            .unwrap();
            std::fs::write(
                generated.parent().unwrap().join("trait.toml"),
                "[package]\nid = \"startup-fixture\"\nversion = \"0.1.0\"\nname = \"Startup fixture\"\nstatus = \"ready\"\n",
            )
            .unwrap();
            let trait_path = trait_path.to_string_lossy().into_owned();
            let loaded = load_trait_source(Some(&trait_path), None, "test").unwrap();
            crate::trust::update_named_digest(
                "startup-fixture",
                &loaded.canonical_digest,
                crate::trust::TrustState::Verified,
                Some("startup observer fixture".to_string()),
            )
            .unwrap();
            assert!(matches!(
                crate::lifecycle::authorize_start(
                    &loaded.trait_root,
                    "startup-fixture",
                    &loaded.canonical_digest
                )
                .unwrap()
                .decision,
                crate::trust::StartTrust::Verified(_)
            ));
            Self { root, trait_path }
        }

        fn start(
            &self,
            worktree: Option<Option<&str>>,
            trait_args: &[String],
            expect_success: bool,
        ) -> Vec<StartupUpdate> {
            let updates = Arc::new(Mutex::new(Vec::new()));
            let observed = Arc::clone(&updates);
            let result = start(StartRequest {
                trait_file: Some(&self.trait_path),
                trait_id: None,
                query: None,
                trait_args,
                input_values: Vec::new(),
                out: None,
                session_store: None,
                ephemeral: true,
                resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
                assign_overrides: &[],
                agent_assignments: None,
                provider_capability_reports: Vec::new(),
                provider_warnings: Vec::new(),
                harness_probes: Vec::new(),
                caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
                state_source: "test",
                trait_arg_evidence: "test",
                worktree,
                defer_commands: false,
                narrate_progress: false,
                startup_observer: Some(Arc::new(move |update| {
                    observed.lock().unwrap().push(update)
                })),
                strict_loops: false,
                override_dependencies: false,
                task_dispatch: false,
                merge_rung: None,
            });
            if expect_success {
                result.unwrap();
            } else {
                assert!(result.is_err());
            }
            updates.lock().unwrap().clone()
        }

        fn start_with_inputs(
            &self,
            input_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
        ) -> StartOutcome {
            start(StartRequest {
                trait_file: Some(&self.trait_path),
                trait_id: None,
                query: None,
                trait_args: &[],
                input_values,
                out: None,
                session_store: None,
                ephemeral: true,
                resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
                assign_overrides: &[],
                agent_assignments: None,
                provider_capability_reports: Vec::new(),
                provider_warnings: Vec::new(),
                harness_probes: Vec::new(),
                caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
                state_source: "test",
                trait_arg_evidence: "test",
                worktree: None,
                defer_commands: false,
                narrate_progress: false,
                startup_observer: None,
                strict_loops: false,
                override_dependencies: false,
                task_dispatch: false,
                merge_rung: None,
            })
            .expect("startup fixture starts")
        }
    }

    #[test]
    fn startup_observer_does_not_classify_setup_text_as_seed_or_warm() {
        assert_eq!(
            startup_stage_for_worktree_phase("seeding"),
            StartupStage::Seeding
        );
        assert_eq!(
            startup_stage_for_worktree_phase("warming target/debug"),
            StartupStage::Warm
        );
        assert_eq!(
            startup_stage_for_worktree_phase("setup: install seed warmer"),
            StartupStage::Worktree
        );
        assert_eq!(
            startup_stage_for_worktree_phase("warm validation ../invalid"),
            StartupStage::Warm
        );
    }

    #[test]
    fn startup_observer_hides_untrusted_trait_details() {
        if !run_in_isolated_child() {
            return;
        }
        const SENTINEL: &str = "PREAUTH_SENTINEL";
        let root = child_test_root();
        let generated = root.join(format!(".ctx/traits/{SENTINEL}/generated"));
        std::fs::create_dir_all(&generated).unwrap();
        let trait_path = generated.join("index.toml");
        std::fs::write(
            &trait_path,
            format!(
                "id = \"{SENTINEL}\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"{SENTINEL}\"\nsummary = \"{SENTINEL}\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            generated.parent().unwrap().join("trait.toml"),
            format!(
                "[package]\nid = \"{SENTINEL}\"\nversion = \"0.1.0\"\nname = \"{SENTINEL}\"\nstatus = \"draft\"\n"
            ),
        )
        .unwrap();
        let updates = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&updates);
        let trait_path = trait_path.to_string_lossy().into_owned();
        let error = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values: Vec::new(),
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: Some(Arc::new(move |update| {
                observed.lock().unwrap().push(update)
            })),
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: false,
            merge_rung: None,
        })
        .unwrap_err();
        let details = updates.lock().unwrap();
        let error_text = error.to_string();
        assert!(
            error_text.starts_with("trait trust refused: ")
                && error_text.contains("is unreviewed on this machine")
                && error_text.contains("ctx traits trust approve"),
            "unexpected trust refusal message: {error_text}"
        );
        assert!(
            !error_text.contains("invalid manifest"),
            "trust refusal must not reuse the manifest-error framing: {error_text}"
        );
        assert_eq!(
            details
                .iter()
                .map(|update| (update.stage, update.state, update.detail.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    StartupStage::Initialization,
                    StartupStageState::Running,
                    "loading trait"
                ),
                (
                    StartupStage::Trust,
                    StartupStageState::Running,
                    "checking lifecycle and trust"
                ),
                (
                    StartupStage::Trust,
                    StartupStageState::Failed,
                    "trait authorization was refused"
                ),
            ]
        );
        assert!(
            details
                .iter()
                .all(|update| !update.detail.contains(SENTINEL))
        );
    }

    #[test]
    fn startup_observer_reports_no_worktree_stages_in_order_without_reopening_terminals() {
        if !run_in_isolated_child() {
            return;
        }
        let fixture = StartupFixture::approved();
        let updates = fixture.start(None, &[], true);
        let tuples = updates
            .iter()
            .map(|update| (update.stage, update.state))
            .collect::<Vec<_>>();
        assert_eq!(
            tuples,
            vec![
                (StartupStage::Initialization, StartupStageState::Running),
                (StartupStage::Trust, StartupStageState::Running),
                (StartupStage::Trust, StartupStageState::Done),
                (StartupStage::Harness, StartupStageState::Running),
                (StartupStage::Harness, StartupStageState::Running),
                (StartupStage::Harness, StartupStageState::Done),
                (StartupStage::Worktree, StartupStageState::Done),
                (StartupStage::Seeding, StartupStageState::Done),
                (StartupStage::Warm, StartupStageState::Done),
                (StartupStage::Initialization, StartupStageState::Done),
            ]
        );
        assert_eq!(
            updates[6..9]
                .iter()
                .map(|update| update.detail.as_str())
                .collect::<Vec<_>>(),
            vec!["not requested", "not requested", "not requested"]
        );
        assert_terminal_states_are_monotonic(&updates);
    }

    #[test]
    fn startup_observer_attributes_deterministic_harness_and_worktree_failures() {
        if !run_in_isolated_child() {
            return;
        }
        let fixture = StartupFixture::approved();
        let updates = fixture.start(None, &["--unknown=value".to_string()], false);
        assert_eq!(
            updates.last().map(|update| (update.stage, update.state)),
            Some((StartupStage::Harness, StartupStageState::Failed))
        );
        let updates = fixture.start(Some(Some("../invalid")), &[], false);
        assert_eq!(
            updates.last().map(|update| (update.stage, update.state)),
            Some((StartupStage::Worktree, StartupStageState::Failed))
        );
        assert_terminal_states_are_monotonic(&updates);
    }

    #[test]
    fn startup_observer_orders_configured_worktree_substages_without_reopening_them() {
        if !run_in_isolated_child() {
            return;
        }
        let fixture = StartupFixture::approved();
        std::fs::write(
            fixture.root.join(".gitignore"),
            ".ctx/worktrees/\nseed.txt\nwarm-valid/\n",
        )
        .unwrap();
        std::fs::write(fixture.root.join("seed.txt"), "seed\n").unwrap();
        std::fs::create_dir_all(fixture.root.join("warm-valid")).unwrap();
        std::fs::write(fixture.root.join("warm-valid/cache"), "warm\n").unwrap();
        std::fs::write(
            fixture.root.join(".ctx/traits/runtime.toml"),
            "[worktree]\nseed = [\"seed.txt\"]\nwarm = [\"warm-valid\"]\n",
        )
        .unwrap();
        // Serialised against tests that restrict the whole process: a child
        // `git` started while a Seatbelt profile is in force cannot see these
        // files, stages nothing, and the commit below fails for a reason that
        // has nothing to do with this test.
        let _process_wide = crate::lock_process_wide_test();
        // The `-c` flag keeps the fixture hermetic: the developer's global
        // excludes must not decide what `add -A` stages. The branch name is
        // deliberately NOT pinned — this fixture never refers to it, and
        // `proof_branch_literals` is right that a literal default-branch name
        // needs a justification nothing here can give.
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "startup@example.invalid"],
            vec!["config", "user.name", "Startup observer"],
            vec!["-c", "core.excludesFile=", "add", "-A"],
            vec!["commit", "-qm", "startup fixture"],
        ] {
            let output = Command::new("git")
                .args(&args)
                .current_dir(&fixture.root)
                .output()
                .unwrap_or_else(|err| panic!("could not spawn `git {}`: {err}", args.join(" ")));
            // Name the command and show what git said. The bare `assert!`
            // this replaces reported only "assertion failed", which is why a
            // real gate failure took a log dig to attribute.
            assert!(
                output.status.success(),
                "fixture `git {}` failed in {} with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                args.join(" "),
                fixture.root.display(),
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }

        let updates = fixture.start(Some(Some("observer-order")), &[], true);

        let stages = updates
            .iter()
            .map(|update| update.stage)
            .collect::<Vec<_>>();
        let worktree = stages
            .iter()
            .position(|stage| *stage == StartupStage::Worktree)
            .unwrap();
        let seeding = stages
            .iter()
            .position(|stage| *stage == StartupStage::Seeding)
            .unwrap();
        let warm = stages
            .iter()
            .position(|stage| *stage == StartupStage::Warm)
            .unwrap();
        assert!(
            worktree < seeding && seeding < warm,
            "configured worktree stages were not ordered Worktree -> Seeding -> Warm: {updates:?}"
        );
        assert_terminal_states_are_monotonic(&updates);
    }

    #[test]
    fn static_and_config_input_defaults_preserve_precedence_and_evidence() {
        let trait_ref = ctx_traits_core::encoding::decode_trait(
            ctx_traits_core::encoding::Encoding::Toml,
            "id = \"default-fixture\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Default fixture\"\nsummary = \"fixture\"\n\n[[port]]\nid = \"plan\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Plan path\"\n[port.default]\nvalue = \".plans/EXECUTION_PLAN.md\"\n",
        )
        .expect("fixture trait decodes");
        let mut values = Vec::new();
        apply_default_inputs(
            &trait_ref,
            &mut values,
            &BTreeMap::new(),
            None,
            &BTreeMap::new(),
        )
        .expect("declared default applies");
        assert_eq!(
            values[0].value,
            Value::String(".plans/EXECUTION_PLAN.md".to_string())
        );
        assert_eq!(
            values[0].source,
            Some(ctx_traits_core::procedure::runtime::ValueSource::DeclaredDefault)
        );
        assert_eq!(
            values[0].producer_evidence.as_deref(),
            Some("port[plan].default.value")
        );

        let config = BTreeMap::from([(
            "plan".to_string(),
            crate::harness_config::ConfiguredPortDefault {
                value: ".plans/OVERRIDE.md".to_string(),
                layer: crate::harness_config::ConfigLayer::Repo,
                evidence: ".ctx/traits/runtime.toml:trait.default-fixture.defaults.port.plan"
                    .to_string(),
            },
        )]);
        values.clear();
        apply_default_inputs(&trait_ref, &mut values, &config, None, &BTreeMap::new())
            .expect("config default applies");
        assert_eq!(
            values[0].value,
            Value::String(".plans/OVERRIDE.md".to_string())
        );
        assert_eq!(
            values[0].source,
            Some(ctx_traits_core::procedure::runtime::ValueSource::TraitConfig)
        );
        assert_eq!(
            values[0].producer_evidence.as_deref(),
            Some(".ctx/traits/runtime.toml:trait.default-fixture.defaults.port.plan")
        );

        values = parse_initial_sets(&["plan=.plans/EXPLICIT.md".to_string()]).unwrap();
        apply_default_inputs(&trait_ref, &mut values, &config, None, &BTreeMap::new())
            .expect("explicit input remains authoritative");
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].value,
            Value::String(".plans/EXPLICIT.md".to_string())
        );
        assert_eq!(
            values[0].source,
            Some(ctx_traits_core::procedure::runtime::ValueSource::CliSet)
        );
        assert_eq!(
            values[0].producer_evidence.as_deref(),
            Some("ctx traits run --set")
        );
    }

    #[test]
    fn startup_records_port_default_precedence_and_ledger_evidence() {
        if !run_in_isolated_child() {
            return;
        }
        let fixture = StartupFixture::approved();
        let accepted = |outcome: StartOutcome| {
            outcome
                .session
                .accepted_port_values
                .into_iter()
                .find(|value| value.ref_text == "port:plan")
                .expect("plan is accepted")
        };

        let declared = accepted(fixture.start_with_inputs(Vec::new()));
        assert_eq!(declared.value, Value::String(".plans/DECLARED.md".into()));
        assert_eq!(
            declared.source,
            ctx_traits_core::procedure::runtime::ValueSource::DeclaredDefault
        );
        assert_eq!(
            declared.producer_evidence.as_deref(),
            Some("port[plan].default.value")
        );

        std::fs::create_dir_all(fixture.root.join(".ctx/traits")).unwrap();
        std::fs::write(
            fixture.root.join(".ctx/traits/runtime.toml"),
            "[trait.startup-fixture.defaults.port]\nplan = \".plans/SCOPED.md\"\n",
        )
        .unwrap();
        let scoped = accepted(fixture.start_with_inputs(Vec::new()));
        assert_eq!(scoped.value, Value::String(".plans/SCOPED.md".into()));
        assert_eq!(
            scoped.source,
            ctx_traits_core::procedure::runtime::ValueSource::TraitConfig
        );
        assert!(scoped.producer_evidence.as_deref().is_some_and(|evidence| {
            evidence.ends_with(".ctx/traits/runtime.toml:trait.startup-fixture.defaults.port.plan")
        }));

        let explicit = accepted(fixture.start_with_inputs(
            parse_initial_sets(&["plan=.plans/EXPLICIT.md".to_string()]).unwrap(),
        ));
        assert_eq!(explicit.value, Value::String(".plans/EXPLICIT.md".into()));
        assert_eq!(
            explicit.source,
            ctx_traits_core::procedure::runtime::ValueSource::CliSet
        );
        assert_eq!(
            explicit.producer_evidence.as_deref(),
            Some("ctx traits run --set")
        );
    }

    #[test]
    fn set_preserves_existing_input_port_provenance() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let trait_root = root.join(".ctx/traits/awaiting-fixture");
        let generated = trait_root.join("generated");
        std::fs::create_dir_all(&generated).unwrap();
        let trait_path = generated.join("index.toml");
        std::fs::write(
            &trait_path,
            "id = \"awaiting-fixture\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"Awaiting fixture\"\nsummary = \"set provenance fixture\"\n\n[[port]]\nid = \"declared\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Declared default\"\n[port.default]\nvalue = \".plans/DECLARED.md\"\n\n[[port]]\nid = \"configured\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Configured default\"\n\n[[port]]\nid = \"cli\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"CLI input\"\n\n[[port]]\nid = \"missing\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Later input\"\n\n[procedure]\ndescription = \"Await inputs\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\ninput = [\"port:missing\"]\noutput = [\"slot:notified\"]\n",
        )
        .unwrap();
        std::fs::write(
            trait_root.join("trait.toml"),
            "[package]\nid = \"awaiting-fixture\"\nversion = \"0.1.0\"\nname = \"Awaiting fixture\"\nstatus = \"ready\"\n\n[defaults.port]\nconfigured = \".plans/CONFIGURED.md\"\n",
        )
        .unwrap();
        let trait_path = trait_path.to_string_lossy().into_owned();
        let loaded = load_trait_source(Some(&trait_path), None, "test").unwrap();
        crate::trust::update_named_digest(
            "awaiting-fixture",
            &loaded.canonical_digest,
            crate::trust::TrustState::Verified,
            Some("set provenance fixture".to_string()),
        )
        .unwrap();
        let session_store = root.join("sessions");
        let session_store_text = session_store.to_string_lossy().into_owned();
        let start = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values: parse_initial_sets(&["cli=.plans/CLI.md".to_string()]).unwrap(),
            out: None,
            session_store: Some(&session_store_text),
            ephemeral: false,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: false,
            merge_rung: None,
        })
        .expect("awaiting fixture starts");
        assert!(start.session.next_frame.is_none());
        let session_path = start.session_path.expect("session is persisted");
        let outcome = set(SetRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            session: session_path.as_str(),
            session_store: None,
            target: "missing",
            value: Value::String(".plans/SUPPLIED.md".to_string()),
            out: None,
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            existing_input_evidence: "must not replace existing evidence",
        })
        .expect("missing input is accepted");
        let SetOutcome::Session { session, .. } = outcome else {
            panic!("awaiting input set rebuilds the session");
        };
        let accepted: BTreeMap<_, _> = session
            .accepted_port_values
            .iter()
            .map(|value| (value.ref_text.as_str(), value))
            .collect();
        let configured_evidence = format!(
            "{}:defaults.port.configured",
            trait_root.join("trait.toml").display()
        );
        for (port, source, evidence) in [
            (
                "port:declared",
                ctx_traits_core::procedure::runtime::ValueSource::DeclaredDefault,
                "port[declared].default.value",
            ),
            (
                "port:configured",
                ctx_traits_core::procedure::runtime::ValueSource::TraitConfig,
                configured_evidence.as_str(),
            ),
            (
                "port:cli",
                ctx_traits_core::procedure::runtime::ValueSource::CliSet,
                "ctx traits run --set",
            ),
            (
                "port:missing",
                ctx_traits_core::procedure::runtime::ValueSource::HostInput,
                "cli:ctx traits set",
            ),
        ] {
            let value = accepted.get(port).expect("port value is accepted");
            assert_eq!(value.source, source, "{port} source");
            assert_eq!(
                value.producer_evidence.as_deref(),
                Some(evidence),
                "{port} evidence"
            );
        }
    }

    /// A minimal `implement-*` trait fixture declaring `task-board` at
    /// `.internal/tasks` (`root = "repo"`, matching every real
    /// `implement-*` package) and an input `task` port, for exercising the
    /// 0061 dependency preflight/materialisation through `start()` end to
    /// end. `board` is a closure so callers can write task files under the
    /// fixture repository root's `.internal/tasks` before `start` is
    /// called. `root` is initialised as a Git repository and everything
    /// written (trait files and board) is committed, since the fixture's
    /// `root = "repo"` resource requires a discoverable repository —
    /// callers that need an uncommitted edit make it AFTER this returns.
    /// Returns the trait path plus the held process-wide test lock (see
    /// `startup_observer_orders_configured_worktree_substages_without_reopening_them`):
    /// the caller must keep the guard alive for every git spawn its own
    /// test body makes afterward (resource resolution, worktree creation),
    /// not just the ones this function makes.
    fn dispatch_task_fixture(
        root: &std::path::Path,
        board: impl FnOnce(&std::path::Path),
    ) -> (String, std::sync::MutexGuard<'static, ()>) {
        let trait_root = root.join(".ctx/traits/implement-fixture");
        let generated = trait_root.join("generated");
        std::fs::create_dir_all(&generated).unwrap();
        let board_dir = root.join(".internal/tasks");
        std::fs::create_dir_all(&board_dir).unwrap();
        board(&board_dir);
        let trait_path = generated.join("index.toml");
        std::fs::write(
            &trait_path,
            "id = \"implement-fixture\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Implement fixture\"\nsummary = \"0061 dispatch fixture\"\n\n[[resource]]\nid = \"task-board\"\npath = \".internal/tasks\"\nroot = \"repo\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task value\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
        )
        .unwrap();
        std::fs::write(
            trait_root.join("trait.toml"),
            "[package]\nid = \"implement-fixture\"\nversion = \"0.1.0\"\nname = \"Implement fixture\"\nstatus = \"ready\"\n",
        )
        .unwrap();

        // Serialised against tests that restrict the whole process: see
        // `startup_observer_orders_configured_worktree_substages_without_reopening_them`.
        let _process_wide = crate::lock_process_wide_test();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "dispatch-task@example.invalid"],
            vec!["config", "user.name", "Dispatch task fixture"],
            vec!["-c", "core.excludesFile=", "add", "-A"],
            vec!["commit", "-qm", "dispatch task fixture"],
        ] {
            let output = Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .unwrap_or_else(|err| panic!("could not spawn `git {}`: {err}", args.join(" ")));
            assert!(
                output.status.success(),
                "fixture `git {}` failed: {}\n{}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr),
            );
        }

        let trait_path = trait_path.to_string_lossy().into_owned();
        let loaded = load_trait_source(Some(&trait_path), None, "test").unwrap();
        crate::trust::update_named_digest(
            "implement-fixture",
            &loaded.canonical_digest,
            crate::trust::TrustState::Verified,
            Some("0061 dispatch fixture".to_string()),
        )
        .unwrap();
        (trait_path, _process_wide)
    }

    #[test]
    fn dependency_preflight_refuses_then_override_dispatches_with_recorded_provenance() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let (trait_path, _process_wide) = dispatch_task_fixture(&root, |tasks| {
            std::fs::write(
                tasks.join("0001-dep.toml"),
                "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Dep\"\nstatus = \"ready\"\n",
            )
            .unwrap();
            std::fs::write(
                tasks.join("0002-dependent.toml"),
                "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Dependent\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
            )
            .unwrap();
        });
        let input_values = parse_initial_sets(&["task=0002".to_string()]).unwrap();

        let refused = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values: input_values.clone(),
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: true,
            merge_rung: None,
        })
        .unwrap_err();
        let message = refused.to_string();
        assert!(
            message.contains("0002 depends on 0001 (ready)"),
            "{message}"
        );
        assert!(message.contains("--override-dependencies"), "{message}");

        let overridden = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values,
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: true,
            task_dispatch: true,
            merge_rung: None,
        })
        .expect("override dispatches despite the unmet dependency");
        let provenance = &overridden.session.provenance;
        let dependency_override = provenance
            .dependency_override
            .as_ref()
            .expect("override is recorded in provenance");
        assert_eq!(dependency_override.task_key, "0002");
        assert_eq!(dependency_override.unmet.len(), 1);
        assert_eq!(dependency_override.unmet[0].key, "0001");
        assert_eq!(dependency_override.unmet[0].status, "ready");
        assert!(
            provenance
                .warnings
                .iter()
                .any(|warning| warning.contains("dependency override recorded")),
            "{:?}",
            provenance.warnings
        );
        assert_eq!(provenance.task_key.as_deref(), Some("0002"));
        assert!(provenance.task_digest.is_some());
    }

    /// 0063.4: an `implement-*` trait declaring no `task-board` resource at
    /// all, exercised without a `task=` value (starts) and with one
    /// (refuses naming the trait and the missing resource) — the explicit
    /// value cannot silently drop.
    fn boardless_implement_fixture(root: &std::path::Path) -> String {
        let trait_root = root.join(".ctx/traits/implement-boardless-fixture");
        let generated = trait_root.join("generated");
        std::fs::create_dir_all(&generated).unwrap();
        let trait_path = generated.join("index.toml");
        std::fs::write(
            &trait_path,
            "id = \"implement-boardless-fixture\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Implement boardless fixture\"\nsummary = \"0063.4 boardless dispatch fixture\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task value\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
        )
        .unwrap();
        std::fs::write(
            trait_root.join("trait.toml"),
            "[package]\nid = \"implement-boardless-fixture\"\nversion = \"0.1.0\"\nname = \"Implement boardless fixture\"\nstatus = \"ready\"\n",
        )
        .unwrap();
        let trait_path = trait_path.to_string_lossy().into_owned();
        let loaded = load_trait_source(Some(&trait_path), None, "test").unwrap();
        crate::trust::update_named_digest(
            "implement-boardless-fixture",
            &loaded.canonical_digest,
            crate::trust::TrustState::Verified,
            Some("0063.4 boardless dispatch fixture".to_string()),
        )
        .unwrap();
        trait_path
    }

    fn boardless_start_request<'a>(
        trait_path: &'a str,
        input_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
    ) -> StartRequest<'a> {
        StartRequest {
            trait_file: Some(trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values,
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: true,
            merge_rung: None,
        }
    }

    #[test]
    fn requested_task_dispatch_against_a_boardless_trait_refuses_naming_trait_and_resource() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let trait_path = boardless_implement_fixture(&root);
        let input_values = parse_initial_sets(&["task=0002".to_string()]).unwrap();
        let refused = start(boardless_start_request(&trait_path, input_values)).unwrap_err();
        let message = refused.to_string();
        assert!(message.contains("implement-boardless-fixture"), "{message}");
        assert!(message.contains("task-board"), "{message}");
    }

    #[test]
    fn requested_task_dispatch_without_a_task_value_refuses() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let trait_path = boardless_implement_fixture(&root);
        let refused = start(boardless_start_request(&trait_path, Vec::new())).unwrap_err();
        assert!(
            refused.to_string().contains("--task-dispatch requires"),
            "{refused}"
        );
    }

    /// The inverse of the refusals above, and the whole point of making task
    /// binding opt-in: WITHOUT `--task-dispatch`, a `task=` value is plain
    /// port text — no board resolution, no refusal, regardless of trait id
    /// or the value's shape.
    #[test]
    fn plain_dispatch_passes_a_prose_task_value_through_without_binding() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let trait_path = boardless_implement_fixture(&root);
        let input_values =
            parse_initial_sets(&["task=free-form prose, not a board key".to_string()]).unwrap();
        let mut request = boardless_start_request(&trait_path, input_values);
        request.task_dispatch = false;
        let outcome = start(request).expect("an unrequested task binding never refuses");
        let task = crate::run_session::session_task(&outcome.session)
            .expect("the task port accepted the prose value");
        assert_eq!(task, "free-form prose, not a board key");
    }

    #[test]
    fn task_digest_and_key_record_the_currently_resolved_document() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let (trait_path, _process_wide) = dispatch_task_fixture(&root, |tasks| {
            std::fs::write(
                tasks.join("0050-example.toml"),
                "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Example\"\nstatus = \"ready\"\n",
            )
            .unwrap();
        });
        let start = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values: parse_initial_sets(&["task=0050".to_string()]).unwrap(),
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: None,
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: true,
            merge_rung: None,
        })
        .expect("resolvable task dispatches");
        assert_eq!(start.session.provenance.task_key.as_deref(), Some("0050"));
        let expected_document = ctx_traits_core::task::TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: "0050".to_string(),
            title: "Example".to_string(),
            status: Some(ctx_traits_core::task::TaskStatus::Ready),
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: String::new(),
            scope: String::new(),
            validation: String::new(),
            relations: ctx_traits_core::task::Relations::default(),
            steps: Vec::new(),
            checks: Vec::new(),
            auto_close: None,
            closure: None,
        };
        let expected_digest = ctx_traits_core::digest::Digest::source(
            &ctx_traits_core::task::serialize(&expected_document).unwrap(),
        );
        assert_eq!(start.session.provenance.task_digest, Some(expected_digest));
    }

    #[test]
    fn worktree_materialisation_carries_an_uncommitted_task_edit() {
        if !run_in_isolated_child() {
            return;
        }
        let root = child_test_root();
        let (trait_path, _process_wide) = dispatch_task_fixture(&root, |board| {
            std::fs::write(
                board.join("0050-example.toml"),
                "schema-version = \"0.2\"\nkey = \"0050\"\ntitle = \"Committed\"\nstatus = \"ready\"\n",
            )
            .unwrap();
        });

        // Edit the task WITHOUT committing — the amended-but-uncommitted
        // trap task 0050 (this suite's own name is a coincidence) hit for
        // real on 2026-08-03 (see the task's own "Why materialise" note).
        let edited_document = ctx_traits_core::task::TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: "0050".to_string(),
            title: "Edited, not yet committed".to_string(),
            status: Some(ctx_traits_core::task::TaskStatus::Ready),
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: String::new(),
            scope: String::new(),
            validation: String::new(),
            relations: ctx_traits_core::task::Relations::default(),
            steps: Vec::new(),
            checks: Vec::new(),
            auto_close: None,
            closure: None,
        };
        let edited_text = ctx_traits_core::task::serialize(&edited_document).unwrap();
        std::fs::write(root.join(".internal/tasks/0050-example.toml"), &edited_text).unwrap();
        let expected_digest = ctx_traits_core::digest::Digest::source(&edited_text);

        let outcome = start(StartRequest {
            trait_file: Some(&trait_path),
            trait_id: None,
            query: None,
            trait_args: &[],
            input_values: parse_initial_sets(&["task=0050".to_string()]).unwrap(),
            out: None,
            session_store: None,
            ephemeral: true,
            resource_evidence: ResourceEvidenceMode::Unavailable { reason: "test" },
            assign_overrides: &[],
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            state_source: "test",
            trait_arg_evidence: "test",
            worktree: Some(Some("materialize-test")),
            defer_commands: false,
            narrate_progress: false,
            startup_observer: None,
            strict_loops: false,
            override_dependencies: false,
            task_dispatch: true,
            merge_rung: None,
        })
        .expect("worktree dispatch of a resolvable task succeeds");

        let execution_dir = outcome
            .execution_dir
            .expect("--worktree request prepares an execution directory");
        let materialized = std::fs::read_to_string(
            execution_dir
                .join(".internal/tasks/0050-example.toml")
                .as_std_path(),
        )
        .expect("the worktree carries the task's materialised copy");
        assert_eq!(
            materialized, edited_text,
            "the worktree's task file must carry the edited, uncommitted text, not the committed one"
        );
        assert_eq!(
            outcome.session.provenance.task_digest,
            Some(expected_digest),
            "the ledger's task digest must equal the digest of the materialised bytes"
        );
        assert_eq!(outcome.session.provenance.task_key.as_deref(), Some("0050"));
    }

    fn assert_terminal_states_are_monotonic(updates: &[StartupUpdate]) {
        let mut terminal = BTreeSet::new();
        for update in updates {
            assert!(
                !terminal.contains(&update.stage) || update.state != StartupStageState::Running,
                "terminal startup stage {:?} reopened: {updates:?}",
                update.stage
            );
            if update.state != StartupStageState::Running {
                terminal.insert(update.stage);
            }
        }
    }
}

pub fn status(request: InspectRequest<'_>) -> crate::Result<InspectOutcome> {
    let session = read_session(request.session, request.session_store)?;
    // Status is read-only. Do not render a frame from elapsed evidence that is
    // not persisted: a later `call` reloads the ledger and must see the same
    // optimistic-concurrency digest the status response issued.
    let _ = request.elapsed_seconds;
    let loaded = load_trait_for_session(request.trait_file, request.trait_id, &session, "run");
    // A legacy ledger whose source has drifted cannot be refreshed without
    // matching recovery bytes, but status must still expose that fact without
    // changing or rejecting the ledger. Try an explicit recovery source first.
    let unrecoverable_source = matches!(
        trait_source_drift(&session),
        TraitSourceDrift::UnrecoverableLegacy { .. }
            | TraitSourceDrift::UnrecoverableInvalidPin { .. }
    );
    if unrecoverable_source && loaded.is_err() {
        let mut session = session;
        if let Some(warning) = trait_source_drift(&session).warning() {
            session.warnings.push(warning);
        }
        let resource_supported =
            ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                &session.resource_evidence,
            );
        return Ok(InspectOutcome {
            session,
            resource_supported,
            command_failure: None,
        });
    }
    let loaded = loaded?;
    validate_pinned_approval(&session, &loaded)?;
    let mut refreshed =
        ctx_traits_core::procedure::session::refresh_run_session(&loaded.trait_ref, session)?;
    if !unrecoverable_source && let Some(warning) = trait_source_drift(&refreshed).warning() {
        refreshed.warnings.push(warning);
    }
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &refreshed.resource_evidence,
        );
    Ok(InspectOutcome {
        session: refreshed,
        resource_supported,
        command_failure: None,
    })
}

pub fn advance_commands(request: AdvanceCommandsRequest<'_>) -> crate::Result<InspectOutcome> {
    let session_path =
        crate::run_session::resolve_session_path(request.session, request.session_store)?;
    let session = crate::run_session::read_run_session(&session_path)?;
    let session = ctx_traits_core::procedure::session::observe_elapsed_seconds(
        session,
        request.elapsed_seconds,
    );
    let restored_execution_dir = restore_session_execution_dir(&session, request.execution_dir)?;
    let execution_dir = request.execution_dir.or(restored_execution_dir.as_deref());
    let loaded = load_trait_for_session(request.trait_file, request.trait_id, &session, "run")?;
    validate_pinned_approval(&session, &loaded)?;
    let refreshed =
        ctx_traits_core::procedure::session::refresh_run_session(&loaded.trait_ref, session)?;
    let advanced = advance_command_frames(
        &loaded.trait_ref,
        &loaded.trait_root,
        refreshed,
        Some(&session_path),
        execution_dir,
        request.execution_env,
        request.tick_observer.as_ref(),
    )?;
    crate::run_session::write_run_session(&session_path, &advanced.session)?;
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &advanced.session.resource_evidence,
        );
    Ok(InspectOutcome {
        session: advanced.session,
        resource_supported,
        command_failure: advanced.failure,
    })
}

/// P402: request to apply a terminal dispatch-level failure (harness retries
/// exhausted, timeout, or a concurrent-wave worker panic/IO error) to a
/// session's current frame — the sibling of [`CallRequest`] for a call that
/// never produced a submittable output at all. Mirrors [`CallRequest`]'s
/// file/session-store resolution shape exactly so the same load/persist
/// discipline applies to both.
pub struct TerminalFailureRequest<'a> {
    pub trait_file: Option<&'a str>,
    pub trait_id: Option<&'a str>,
    pub session: &'a str,
    pub session_store: Option<&'a str>,
    pub reason: &'a str,
    pub execution_dir: Option<&'a Utf8Path>,
    pub execution_env: &'a BTreeMap<String, String>,
    pub tick_observer: Option<crate::harness::TickObserver>,
}

/// Apply a terminal dispatch-level failure to a session's current frame
/// (P402; see [`TerminalFailureRequest`]). Reuses
/// [`ctx_traits_core::procedure::session::submit_terminal_frame_failure`],
/// which itself reuses the same nested-recovery / P264 branch-failure
/// policy an ordinary rejected submission already triggers — this function
/// only adds the IO-layer load/persist/command-advance plumbing [`call`]
/// already has, so both entrypoints stay byte-identical in that regard.
pub fn terminal_failure_call(request: TerminalFailureRequest<'_>) -> crate::Result<CallOutcome> {
    let session_path =
        crate::run_session::resolve_session_path(request.session, request.session_store)?;
    let session = crate::run_session::read_run_session(&session_path)?;
    let restored_execution_dir = restore_session_execution_dir(&session, request.execution_dir)?;
    let execution_dir = request.execution_dir.or(restored_execution_dir.as_deref());
    let loaded = load_trait_for_session(request.trait_file, request.trait_id, &session, "run")?;
    validate_pinned_approval(&session, &loaded)?;
    let response = ctx_traits_core::procedure::session::submit_terminal_frame_failure(
        &loaded.trait_ref,
        session,
        request.reason,
    )?;
    let write_path = write_output_path(None, &session_path)?;
    if response.persist_session {
        crate::run_session::write_run_session(&write_path, &response.session)?;
    }
    let (response, command_failure) = rebuild_call_response_after_command_advance(
        &loaded.trait_ref,
        &loaded.trait_root,
        response,
        &write_path,
        execution_dir,
        request.execution_env,
        request.tick_observer.as_ref(),
    )?;
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &response.session.resource_evidence,
        );
    Ok(CallOutcome {
        response,
        resource_supported,
        command_failure,
        session_path: write_path,
    })
}

pub fn call(request: CallRequest<'_>) -> crate::Result<CallOutcome> {
    let session_path =
        crate::run_session::resolve_session_path(request.session, request.session_store)?;
    let session = crate::run_session::read_run_session(&session_path)?;
    let restored_execution_dir = restore_session_execution_dir(&session, request.execution_dir)?;
    let execution_dir = request.execution_dir.or(restored_execution_dir.as_deref());
    let loaded = load_trait_for_session(request.trait_file, request.trait_id, &session, "run")?;
    // `next` and `status` rebuild frames from the authoritative ledger without
    // persisting that derived session. Rebuild here too before checking the
    // submitted template, or a stale top-level digest can make `call` reject
    // the fresh template `next` just issued for the unchanged ledger.
    let persisted_digest = session.state_digest.clone();
    let refreshed = ctx_traits_core::procedure::session::refresh_run_session(
        &loaded.trait_ref,
        session.clone(),
    )?;
    let rendered_digest = refreshed.state_digest.clone();
    let mut submission = request.submission;
    // Verify the token rendered with the frame before merging fresh host clock
    // evidence. Then retarget the internal submission to the derived ledger
    // state so core can persist elapsed time atomically without accepting a
    // genuinely stale frame.
    let session = if submission.state_digest.as_deref() == Some(rendered_digest.as_str()) {
        let session = ctx_traits_core::procedure::session::observe_elapsed_seconds(
            refreshed,
            request.elapsed_seconds,
        );
        submission.state_digest = Some(session.state_digest.clone());
        session
    } else if submission.state_digest.as_deref() == Some(persisted_digest.as_str()) {
        let session = ctx_traits_core::procedure::session::observe_elapsed_seconds(
            session,
            request.elapsed_seconds,
        );
        submission.state_digest = Some(session.state_digest.clone());
        session
    } else {
        refreshed
    };
    validate_pinned_approval(&session, &loaded)?;
    let response = ctx_traits_core::procedure::session::submit_run_call(
        &loaded.trait_ref,
        session,
        submission,
    )?;
    let write_path = write_output_path(request.out, &session_path)?;
    // Persist the acceptance before advancing trailing command frames: a
    // failing command must never discard already-accepted agent output.
    if response.persist_session {
        crate::run_session::write_run_session(&write_path, &response.session)?;
    }
    let (response, command_failure) = rebuild_call_response_after_command_advance(
        &loaded.trait_ref,
        &loaded.trait_root,
        response,
        &write_path,
        execution_dir,
        request.execution_env,
        request.tick_observer.as_ref(),
    )?;
    let resource_supported =
        ctx_traits_core::procedure::session::declared_resource_evidence_supported(
            &response.session.resource_evidence,
        );
    Ok(CallOutcome {
        response,
        resource_supported,
        command_failure,
        session_path: write_path,
    })
}

pub fn set(request: SetRequest<'_>) -> crate::Result<SetOutcome> {
    let session_path =
        crate::run_session::resolve_session_path(request.session, request.session_store)?;
    let session = crate::run_session::read_run_session(&session_path)?;
    let restored_execution_dir = restore_session_execution_dir(&session, None)?;
    let loaded = load_trait_for_session(request.trait_file, request.trait_id, &session, "run")?;
    validate_pinned_approval(&session, &loaded)?;
    let resolution = ctx_traits_core::procedure::session::resolve_run_set_submission(
        &loaded.trait_ref,
        &session,
        request.target,
        request.value,
        request.caller,
    )?;
    let write_path = write_output_path(request.out, &session_path)?;
    match resolution {
        ctx_traits_core::procedure::session::SetResolution::InitialPortValue(initial) => {
            let mut initial_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput> =
                session
                    .accepted_port_values
                    .iter()
                    .map(
                        |value| ctx_traits_core::procedure::runtime::StepSlotOutput {
                            ref_text: value.ref_text.clone(),
                            value: value.value.clone(),
                            source: Some(value.source.clone()),
                            producer_evidence: value.producer_evidence.clone(),
                            command_execution: None,
                            producer_agent: value.producer_agent.clone(),
                            producer_harness: value.producer_harness.clone(),
                        },
                    )
                    .collect();
            initial_values.push(*initial);
            initial_values.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
            let mut refreshed = ctx_traits_core::procedure::session::start_run_session(
                &loaded.trait_ref,
                &ctx_traits_core::manifest::PackageStatus::Ready,
                &ctx_traits_core::r#trait::TrustVerdict::Verified,
                ctx_traits_core::procedure::session::StartRequest {
                    session_id: session.session_id.clone(),
                    run_id: session.run_id.clone(),
                    initial_port_values: initial_values,
                    resource_evidence: session.resource_evidence.clone(),
                    resolved_settings: session.resolved_settings.clone(),
                    resolved_budgets: session.resolved_budgets.clone(),
                    provider_capability_reports: session.provider_capability_reports.clone(),
                    source_digest: session.source_digest.clone(),
                    canonical_digest: session.canonical_digest.clone(),
                    agent_assignments: session.provenance.agent_assignments.clone(),
                    provider_warnings: session.provenance.warnings.clone(),
                    harness_probes: session.provenance.harness_probes.clone(),
                    // Rebuild preserves the policy the run was started with.
                    strict_loops: session.ledger.strict_loops,
                    provenance: session.provenance.clone(),
                },
            )?;
            refreshed = advance_command_frames(
                &loaded.trait_ref,
                &loaded.trait_root,
                refreshed,
                Some(&write_path),
                restored_execution_dir.as_deref(),
                &BTreeMap::new(),
                None,
            )?
            .session;
            crate::run_session::write_run_session(&write_path, &refreshed)?;
            let resource_supported =
                ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                    &refreshed.resource_evidence,
                );
            Ok(SetOutcome::Session {
                session: Box::new(refreshed),
                resource_supported,
            })
        }
        ctx_traits_core::procedure::session::SetResolution::CurrentFrameCall(submission) => {
            let response = ctx_traits_core::procedure::session::submit_current_frame_set(
                &loaded.trait_ref,
                session,
                *submission,
            )?;
            if response.persist_session {
                crate::run_session::write_run_session(&write_path, &response.session)?;
            }
            let (response, _command_failure) = rebuild_call_response_after_command_advance(
                &loaded.trait_ref,
                &loaded.trait_root,
                response,
                &write_path,
                restored_execution_dir.as_deref(),
                &BTreeMap::new(),
                None,
            )?;
            let resource_supported =
                ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                    &response.session.resource_evidence,
                );
            Ok(SetOutcome::Call {
                response: Box::new(response),
                resource_supported,
            })
        }
    }
}

/// Re-resolve the execution directory recorded by a worktree-backed session.
///
/// Paths stay operational rather than persisted: provenance carries only the
/// worktree id and expected branch, and every resume verifies that registration
/// before a command can run. An explicit caller-supplied directory must match
/// that registered path; it cannot redirect a resumed session.
fn restore_session_execution_dir(
    session: &ctx_traits_core::procedure::session::Session,
    explicit: Option<&Utf8Path>,
) -> crate::Result<Option<Utf8PathBuf>> {
    let Some(worktree) = session.provenance.worktree.as_ref() else {
        return Ok(None);
    };
    let mut warnings = crate::worktree::RetryWarnings::new();
    let registered = crate::worktree::verify_worktree_registration(
        &worktree.id,
        &worktree.branch,
        &mut warnings,
    )?;
    if explicit.is_some_and(|path| path != registered) {
        return invalid_request(
            "run.execution-dir",
            format!(
                "execution directory {explicit:?} conflicts with recorded worktree {} at {}",
                worktree.id, registered
            ),
        );
    }
    Ok(Some(registered))
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

pub fn load_trait_source(
    trait_file: Option<&str>,
    trait_id: Option<&str>,
    field_prefix: &str,
) -> crate::Result<LoadedTrait> {
    let (path, source_kind) = resolve_trait_path(trait_file, trait_id, field_prefix)?;
    let (trait_ref, trait_root, source_digest, canonical_digest) = load_trait(path.as_str())?;
    Ok(LoadedTrait {
        trait_ref,
        trait_root,
        path,
        source_kind,
        source_digest: source_digest.as_str().to_string(),
        canonical_digest: canonical_digest.as_str().to_string(),
    })
}

/// Resolve one candidate trait id against every tier — repo-authored,
/// repo-vendored, user-global, built-in — via the single shared
/// [`crate::inventory::InventoryContext`], WITHOUT consulting family-variant
/// fallbacks.
///
/// Returns `Ok(None)` when `id` is genuinely absent from every tier so a
/// caller may legitimately try a different candidate id. Any other failure
/// (a malformed/unreadable repo-authored package directory) is returned as
/// `Err` immediately: a bad local package must surface its own error rather
/// than being treated as absent and silently falling through to a fallback
/// id or a further tier.
/// Repo-authored `.ctx/traits/<id>` precheck shared by every resolution path
/// that must check whether `id` shadows every other tier before falling
/// through — `resolve_local_or_builtin_trait_id` and the merged ordinary/
/// family-tier resolution inside `try_resolve_trait_id` both call this one
/// implementation rather than each re-probing the filesystem independently.
///
/// Any outcome other than "does not exist at all" (a file, a directory with
/// a bad manifest, a permission error, a symlink) must surface its own error
/// rather than silently falling through. Only a confirmed `NotFound` may
/// consult a further tier. Mirrors `InventoryContext::resolve_tiers`: the
/// repo-authored precheck only applies when the invocation has a project
/// tier root (a Git repository, or a plain directory with machine-written
/// install markers). A marker-less ad-hoc invocation must not treat a
/// stray local `.ctx/traits/<id>` directory as malformed-or-shadowing; it
/// goes straight to the shared tier scan, which itself omits project tiers
/// there (P439).
fn repo_authored_precheck(
    context: &crate::inventory::InventoryContext,
    id: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    if !context.project_tiers_visible() {
        return Ok(None);
    }
    let repo_root = context.repo_root_for_paths();
    let local_package_root = crate::layout::trait_authoring_root_path(repo_root).join(id);
    let local_exists = match std::fs::symlink_metadata(local_package_root.as_std_path()) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    };
    if !local_exists {
        return Ok(None);
    }
    let path = crate::layout::trait_manifest_path(repo_root, id)?;
    if path.exists() {
        return Ok(Some((path, "trait-id".to_string())));
    }
    // A local package directory exists but has no manifest at the expected
    // path: this id is malformed/unreadable, not absent, so it must not be
    // treated as a fallback opportunity. The most common way here is a
    // package whose canonical was never built (or whose format is
    // mid-migration) — and `build` resolves sources, not canonicals, so it
    // is the way OUT, never circular.
    Err(invalid_request_error(
        "trait-id",
        format!(
            "trait ID {id:?} has a local package at {local_package_root} but no manifest at {path}; if the package has authoring sources, `ctx traits build {id}` regenerates it (a family package also needs its [family] table in trait.toml); otherwise fix or remove the package before retrying"
        ),
    ))
}

fn resolve_local_or_builtin_trait_id(
    context: &crate::inventory::InventoryContext,
    id: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    if let Some(resolved) = repo_authored_precheck(context, id)? {
        return Ok(Some(resolved));
    }
    match context.resolve_tiers(id)? {
        Some(resolution) => Ok(Some((resolution.winner.path, resolution.winner.origin))),
        None => Ok(None),
    }
}

/// Attempt to resolve `original_id` as a trait id across every tier
/// (desugared `family:variant` references, bare-family default-variant
/// fallback, then repo-authored/vendored/global/built-in precedence via
/// [`resolve_local_or_builtin_trait_id`]) — the one authoritative
/// resolution seam [`resolve_trait_path`] and every fallback-aware caller
/// (e.g. `trust approve`'s package fallback) share; neither reimplements
/// tier precedence or the local-shadow precheck independently.
///
/// Returns `Ok(None)` in the two senses of "not a trait": `original_id`
/// does not parse/validate as a trait reference at all (fails
/// [`ctx_traits_core::shared::desugar_variant_ref`] or
/// [`ctx_traits_core::shared::validate_slug_shape`]), or it validates but
/// resolves to no candidate in any tier. Both are legitimate reasons for a
/// fallback-aware caller to try something else (e.g. an installed
/// package). Any OTHER failure — in particular a malformed/unreadable
/// local package directory from [`resolve_local_or_builtin_trait_id`], or
/// any IO error — always propagates as `Err`: a genuinely broken candidate
/// must never be silently treated the same as absent.
pub fn try_resolve_trait_id(original_id: &str) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    let field_path = "trait-id";
    let Ok(desugared) = ctx_traits_core::shared::desugar_variant_ref(original_id, field_path)
    else {
        return Ok(None);
    };
    let id = desugared.as_deref().unwrap_or(original_id);
    if ctx_traits_core::shared::validate_slug_shape(id, field_path).is_err() {
        return Ok(None);
    }

    let context = crate::inventory::InventoryContext::discover()?;

    let mut id_parts = original_id.splitn(2, ':');
    let family = id_parts.next().unwrap_or_default();
    let explicit_variant = id_parts.next();
    let variant = explicit_variant.unwrap_or("default");

    // Repo-authored always outranks every other tier outright, so every
    // repo-authored candidate shape for this id is checked first, before any
    // lower-tier candidate is even built. A repo-authored family table's
    // variant for `variant` is checked first (P531 Stage 1: a folded family
    // package resolves straight to that variant's `generated/<selector>/`
    // output, ahead of an ordinary same-named package shape), then the exact
    // ordinary id ([`repo_authored_precheck`]), then a repo-authored family
    // table's legacy hyphenated alias. A malformed repo-authored package or
    // `[family]` table surfaces its own error immediately rather than ever
    // being masked by a lower-tier candidate.
    if let Some(resolved) = resolve_local_family_variant(&context, family, variant)? {
        return Ok(Some(resolved));
    }
    if let Some(resolved) = repo_authored_precheck(&context, id)? {
        return Ok(Some(resolved));
    }
    if !original_id.contains(':')
        && let Some(resolved) = resolve_local_family_alias(&context, original_id)?
    {
        return Ok(Some(resolved));
    }

    // `family:default` desugars to the bare `family` id above, so bare-family
    // precedence is tried first. Only when that bare package is genuinely
    // absent (no local package dir, no built-in) do we fall back to the
    // `family-default` package shape — never on a malformed/unreadable bare
    // package, which must surface its own error instead of silently
    // resolving elsewhere.
    //
    // This fallback only fires for an *explicit* `family:default` reference,
    // never a bare `family` id: `explicit_variant` is `None` for a bare id
    // and `variant` collapses both shapes to `"default"`, so matching on
    // `variant` here would make an unrelated ordinary `<id>-default`
    // package unintentionally resolve for every bare id that has no other
    // candidate — changing pre-existing resolution behavior outside P535.
    let default_fallback_id = match explicit_variant {
        Some("default") => Some(format!("{family}-default")),
        _ => None,
    };

    // Below repo-authored, merge ordinary desugared-id candidates, vendored
    // native-family-variant candidates, and vendored family-alias candidates
    // under one shared tier ordering (repo-vendored, user-global, built-in)
    // instead of resolving one kind fully across every tier before ever
    // consulting the others. This is what keeps a project-vendored family
    // variant or alias from losing to a global or built-in legacy package,
    // and a global vendored family variant or alias from losing to a
    // built-in package: each candidate is tagged with the tier it was found
    // at and
    // the lowest tier wins, exactly as
    // [`crate::inventory::InventoryContext::resolve_tiers`] already does for
    // purely-ordinary ids.
    let mut candidates = merged_lower_tier_candidates(&context, id, family, variant, original_id)?;
    if !candidates.is_empty() {
        candidates.sort_by_key(|candidate| candidate.tier);
        let winner = candidates
            .into_iter()
            .next()
            .expect("checked non-empty above");
        return Ok(Some((winner.path, winner.origin)));
    }

    if let Some(fallback_id) = &default_fallback_id
        && ctx_traits_core::shared::validate_slug_shape(fallback_id, field_path).is_ok()
        && let Some(resolved) = resolve_local_or_builtin_trait_id(&context, fallback_id)?
    {
        return Ok(Some(resolved));
    }
    Ok(None)
}

/// Resolve `family:variant` against a *repo-authored* local native family
/// package's `[family]` table only. Returns `Ok(None)` when `family` has no
/// repo-authored local package, or that package has no `[family]` table —
/// letting the caller fall through to ordinary desugared-id resolution and
/// then, if that also comes back empty, vendored family resolution
/// ([`resolve_vendored_family_variant`]). A repo-authored `[family]` table
/// that exists but is malformed, or that names `variant` but is missing the
/// variant's canonical file, always surfaces its own error here rather than
/// ever being treated as absent.
fn resolve_local_family_variant(
    context: &crate::inventory::InventoryContext,
    family: &str,
    variant: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    if ctx_traits_core::shared::validate_slug_shape(family, "trait-id.family").is_err() {
        return Ok(None);
    }
    if !context.project_tiers_visible() {
        return Ok(None);
    }
    let repo_root = context.repo_root_for_paths();
    let local_package_root = crate::layout::trait_authoring_root_path(repo_root).join(family);
    resolve_family_variant_at_package_root(&local_package_root, family, variant)
}

/// Given a package root, resolve `variant` to its canonical file through
/// that package's own `[family]` table (`<package_root>/trait.toml`) — the
/// core shared by [`resolve_local_family_variant`] (a repo-authored local
/// package root) and the built-in-tier branch of
/// [`merged_lower_tier_candidates`] (a materialized built-in store package
/// root). Returns `Ok(None)` when the package has no `[family]` table, or
/// the table has no `variant` entry — the caller's signal to fall through to
/// the next candidate kind/tier. A table that names `variant` but is missing
/// the variant's canonical file always surfaces its own error here rather
/// than ever being treated as absent.
fn resolve_family_variant_at_package_root(
    package_root: &Utf8Path,
    family: &str,
    variant: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    let root_manifest = crate::layout::package_manifest_path(package_root);
    let Some(table) = crate::family_manifest::read_family_table(&root_manifest)? else {
        return Ok(None);
    };
    let Some((_name, resolved_variant)) = table.variant(variant) else {
        return Ok(None);
    };
    let variant_path = package_root.join(&resolved_variant.relative_path);
    if !variant_path.is_file() {
        return Err(crate::Error::Usage {
            message: format!(
                "native family {family:?} declares variant {variant:?} at {variant_path}, but that canonical file does not exist"
            ),
        });
    }
    Ok(Some((variant_path, "trait-id".to_string())))
}

/// Build the merged candidate list for every tier *below* repo-authored —
/// repo-vendored, user-global, built-in — combining ordinary desugared-id
/// candidates, vendored native-family-variant candidates for
/// `family:variant`, and vendored family-alias candidates for the bare
/// hyphenated `alias` (P535), each tagged with the tier it was found at.
/// Callers sort by tier and take the lowest: this is what keeps a
/// project-vendored family variant or alias from losing to a global or
/// built-in legacy package, and a global vendored family variant or alias
/// from losing to a built-in package, instead of any one candidate kind
/// being resolved fully across every tier before the others are ever
/// consulted.
///
/// Called only after the repo-authored prechecks in `try_resolve_trait_id`
/// have all come back empty, so nothing here can shadow (or mask the error
/// from) a repo-authored candidate at the same nominal id.
fn merged_lower_tier_candidates(
    context: &crate::inventory::InventoryContext,
    id: &str,
    family: &str,
    variant: &str,
    alias: &str,
) -> crate::Result<Vec<crate::inventory::Candidate>> {
    let mut candidates: Vec<crate::inventory::Candidate> = Vec::new();
    if let Some(resolution) = context.resolve_tiers(id)? {
        candidates.push(resolution.winner);
        candidates.extend(resolution.shadowed);
    }
    // Repo-authored is already confirmed absent by the caller's prechecks,
    // so any `RepoAuthored` candidate `resolve_tiers` reports here would be
    // a contradiction; it is filtered out defensively rather than trusted to
    // outrank a real family-variant or alias candidate.
    candidates.retain(|candidate| candidate.tier != crate::inventory::Tier::RepoAuthored);

    let family_valid =
        ctx_traits_core::shared::validate_slug_shape(family, "trait-id.family").is_ok();
    let alias_valid = !alias.contains(':');
    let in_repo = context.project_tiers_visible();

    if in_repo {
        let repo_root = context.repo_root_for_paths();
        let project_scope = crate::distribution::DistributionScope::project(repo_root);
        if family_valid
            && crate::distribution::vendored_family_variant_exists(&project_scope, family, variant)?
            && let Some((path, origin)) = crate::distribution::resolve_vendored_trait_variant(
                &project_scope,
                family,
                Some(variant),
            )?
        {
            candidates.push(crate::inventory::Candidate {
                tier: crate::inventory::Tier::RepoVendored,
                path,
                origin,
            });
        }
        if alias_valid
            && let Some((path, origin)) =
                crate::distribution::resolve_vendored_trait_alias(&project_scope, alias)?
        {
            candidates.push(crate::inventory::Candidate {
                tier: crate::inventory::Tier::RepoVendored,
                path,
                origin,
            });
        }
    }

    let global_scope = crate::distribution::DistributionScope::global()?;
    if family_valid
        && crate::distribution::vendored_family_variant_exists(&global_scope, family, variant)?
        && let Some((path, origin)) = crate::distribution::resolve_vendored_trait_variant(
            &global_scope,
            family,
            Some(variant),
        )?
    {
        candidates.push(crate::inventory::Candidate {
            tier: crate::inventory::Tier::UserGlobal,
            path,
            origin,
        });
    }
    if alias_valid
        && let Some((path, origin)) =
            crate::distribution::resolve_vendored_trait_alias(&global_scope, alias)?
    {
        candidates.push(crate::inventory::Candidate {
            tier: crate::inventory::Tier::UserGlobal,
            path,
            origin,
        });
    }

    // Built-in tier (P0193 §Part B): a built-in shipped as a native family
    // (`[family]` table + variants) is otherwise invisible to `family:variant`
    // references — `resolve_tiers`/`context.resolve_tiers` above only
    // consults built-ins via an exact ordinary-id lookup
    // (`builtin_trait_packages::package(id)`). Materializing here reuses the
    // same self-healing store path every built-in lookup already takes; the
    // tier sort in `try_resolve_trait_id` already guarantees any
    // authored/vendored/global candidate above shadows it, so this never
    // changes precedence.
    let repo_root = context.repo_root_for_paths();
    if family_valid
        && let Some(package_root) =
            crate::builtin_store::resolve_builtin_package_root(repo_root, family)?
        && let Some((path, origin)) =
            resolve_family_variant_at_package_root(&package_root, family, variant)?
    {
        candidates.push(crate::inventory::Candidate {
            tier: crate::inventory::Tier::BuiltIn,
            path,
            origin: format!("built-in ({origin})"),
        });
    }
    if alias_valid {
        for candidate_id in builtin_alias_package_candidates(alias, |id| {
            ctx_traits_core::builtin_trait_packages::package(id).is_some()
        }) {
            if let Some(package_root) =
                crate::builtin_store::resolve_builtin_package_root(repo_root, &candidate_id)?
                && let Some((path, origin)) =
                    resolve_family_alias_at_package_root(&package_root, alias)?
            {
                candidates.push(crate::inventory::Candidate {
                    tier: crate::inventory::Tier::BuiltIn,
                    path,
                    origin: format!("built-in ({origin})"),
                });
                break;
            }
        }
    }

    Ok(candidates)
}

/// Derive the built-in package id(s) that could own a colon-less alias like
/// `implement-quick`, where `try_resolve_trait_id` sets `family` to the
/// *entire* alias (splitn(2, ':') on a colon-less id yields the whole
/// string), so `family` can never itself equal the owning package's shorter
/// id. Each strict hyphen-prefix of `alias` is checked against `exists`
/// (normally [`builtin_trait_packages::package`]) — the vendored analog
/// ([`crate::distribution::resolve_vendored_trait_alias`]) discovers the
/// owning package by scanning lock entries instead, since a vendored alias
/// has no compiled-in registry to prefix-match against.
fn builtin_alias_package_candidates(alias: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    alias
        .match_indices('-')
        .map(|(index, _)| &alias[..index])
        .filter(|prefix| exists(prefix))
        .map(str::to_string)
        .collect()
}

/// Given a package root, resolve a legacy hyphenated `alias` through that
/// package's own `[family]` table — the built-in-tier counterpart of
/// [`resolve_local_family_alias`]'s repo-authored directory walk, reused so
/// `implement-quick`-shaped ids also resolve against a built-in family.
fn resolve_family_alias_at_package_root(
    package_root: &Utf8Path,
    alias: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    let root_manifest = crate::layout::package_manifest_path(package_root);
    let Some(table) = crate::family_manifest::read_family_table(&root_manifest)? else {
        return Ok(None);
    };
    let Some((_name, variant)) = table.variant_for_alias(alias) else {
        return Ok(None);
    };
    let variant_path = package_root.join(&variant.relative_path);
    if !variant_path.is_file() {
        return Err(crate::Error::Usage {
            message: format!(
                "native family alias {alias:?} declares canonical variant at {variant_path}, but that file does not exist"
            ),
        });
    }
    Ok(Some((variant_path, "trait-id".to_string())))
}

/// Resolve a legacy hyphenated selector published in a *repo-authored* local
/// native family's manifest only. Vendored family-alias candidates (P535)
/// are resolved together with ordinary and family-variant candidates by
/// [`merged_lower_tier_candidates`] instead, so a project- or global-tier
/// alias competes under the same tier ordering as every other candidate
/// kind rather than being checked only after ordinary resolution has
/// already picked a winner across all tiers. The alias is manifest data
/// rather than a guessed suffix, which keeps arbitrary package names from
/// accidentally becoming family variants.
fn resolve_local_family_alias(
    context: &crate::inventory::InventoryContext,
    alias: &str,
) -> crate::Result<Option<(Utf8PathBuf, String)>> {
    if alias.contains(':') {
        return Ok(None);
    }
    if !context.project_tiers_visible() {
        return Ok(None);
    }
    let repo_root = context.repo_root_for_paths();
    let traits_root = crate::layout::trait_authoring_root_path(repo_root);
    let entries = match std::fs::read_dir(&traits_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: traits_root.to_string(),
                source,
            }
            .into());
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: traits_root.to_string(),
            source,
        })?;
        let path = match Utf8PathBuf::from_path_buf(entry.path()) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if let Some(resolved) = resolve_family_alias_at_package_root(&path, alias)? {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

pub fn resolve_trait_path(
    trait_file: Option<&str>,
    trait_id: Option<&str>,
    field_prefix: &str,
) -> crate::Result<(Utf8PathBuf, String)> {
    match (trait_file, trait_id) {
        (Some(file), None) => Ok((Utf8PathBuf::from(file), "file".to_string())),
        (None, Some(original_id)) => {
            if let Some(resolved) = try_resolve_trait_id(original_id)? {
                return Ok(resolved);
            }

            // Not found anywhere (or not even trait-shaped): rebuild the
            // same rich typo/variant diagnostic `try_resolve_trait_id`
            // itself has no use for, by re-running the same desugar/
            // validate calls so THIS caller's error names the exact
            // malformed field when `original_id` was never trait-shaped.
            let field_path = format!("{field_prefix}.trait-id");
            let desugared = ctx_traits_core::shared::desugar_variant_ref(original_id, &field_path)?;
            let id = desugared.as_deref().unwrap_or(original_id);
            ctx_traits_core::shared::validate_slug_shape(id, &field_path)?;

            let family = original_id.split(':').next().unwrap_or(original_id);
            let context = crate::inventory::InventoryContext::discover()?;
            let repo_root_display = context.repo_root_for_paths();
            let family_root_manifest = crate::layout::package_manifest_path(
                &crate::layout::trait_authoring_root_path(repo_root_display).join(family),
            );
            let variants: Vec<String> =
                match crate::family_manifest::read_family_table(&family_root_manifest)? {
                    // A native family package: list its declared variants
                    // straight from the `[family]` table rather than
                    // scraping sibling `-suffix` directories.
                    Some(table) => table.variant_names(),
                    None => crate::discovery::trait_inventory_ids(repo_root_display)?
                        .into_iter()
                        .filter_map(|candidate| {
                            if candidate == family {
                                Some("default".to_string())
                            } else {
                                candidate
                                    .strip_prefix(&format!("{family}-"))
                                    .map(|suffix| suffix.to_string())
                            }
                        })
                        .collect(),
                };
            let path = crate::layout::trait_manifest_path(repo_root_display, id)?;
            let local_package_root =
                crate::layout::trait_authoring_root_path(repo_root_display).join(id);
            if !variants.is_empty() {
                let listing = variants.join(", ");
                return invalid_request(
                    &field_path,
                    format!(
                        "trait ID {id:?} did not resolve to {path}; available variants in the {family:?} family: {listing} — run as {family}:<variant>"
                    ),
                );
            }
            invalid_request(
                &field_path,
                format!(
                    "trait ID {id:?} did not resolve to {path}; check the id for a typo, create/build the package under {local_package_root} so {path} exists, or add {family:?} to [vendor.dependencies] in .ctx/traits/config.toml and sync, before retrying"
                ),
            )
        }
        (Some(_), Some(_)) => invalid_request(
            &format!("{field_prefix}.trait-source"),
            "pass either a trait ID or --file, not both",
        ),
        (None, None) => invalid_request(
            &format!("{field_prefix}.trait-source"),
            "run requires a trait ID or --file <trait.toml>",
        ),
    }
}

/// Load and decode a trait document from `file`.
///
/// This is the shared chokepoint nearly every CLI surface (check, review,
/// activate/deactivate, list, generate, import, eval, explain, drift,
/// render, search) loads a trait through, so it is also the one place a
/// transition-period legacy `status`/`trust` field on the canonical document
/// (see `decode_trait_with_warnings`) is surfaced to the user: printed once
/// to stderr per call rather than silently discarded, without every call
/// site re-parsing the document to recover the warnings `decode_trait`
/// throws away.
pub fn load_trait(
    file: &str,
) -> crate::Result<(
    ctx_traits_core::Trait,
    Utf8PathBuf,
    ctx_traits_core::digest::Digest,
    ctx_traits_core::digest::Digest,
)> {
    let path = Utf8Path::new(file);
    let text = crate::read::read_text(path)?;
    let (trait_ref, trait_root, source_digest, canonical_digest) = load_trait_text(path, &text)?;
    Ok((trait_ref, trait_root, source_digest, canonical_digest))
}

fn load_trait_text(
    path: &Utf8Path,
    text: &str,
) -> crate::Result<(
    ctx_traits_core::Trait,
    Utf8PathBuf,
    ctx_traits_core::digest::Digest,
    ctx_traits_core::digest::Digest,
)> {
    load_trait_text_with_context(path, path, text)
}

/// Decode `text` with the recorded document's encoding while resolving package
/// resources from the source path supplied by the current resume caller.
fn load_trait_text_with_context(
    encoding_path: &Utf8Path,
    package_path: &Utf8Path,
    text: &str,
) -> crate::Result<(
    ctx_traits_core::Trait,
    Utf8PathBuf,
    ctx_traits_core::digest::Digest,
    ctx_traits_core::digest::Digest,
)> {
    let encoding = ctx_traits_core::encoding::Encoding::from_path(encoding_path)?;
    let (trait_ref, warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, text)?;
    crate::decode_diagnostics::print_decode_warnings(encoding_path.as_str(), &warnings);
    let trait_root = crate::layout::package_root_for_manifest(package_path)
        .map(Utf8Path::to_path_buf)
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: package_path.to_string(),
            source: std::io::Error::other("trait file has no package root"),
        })?;
    let source_digest = ctx_traits_core::digest::Digest::source(text);
    let canonical_digest = ctx_traits_core::digest::canonical_digest(&trait_ref)?;
    Ok((trait_ref, trait_root, source_digest, canonical_digest))
}

pub fn load_trait_for_session(
    trait_file: Option<&str>,
    trait_id: Option<&str>,
    session: &ctx_traits_core::procedure::session::Session,
    field_prefix: &str,
) -> crate::Result<LoadedTrait> {
    let source = session.provenance.trait_source.as_ref().ok_or_else(|| {
        invalid_request_error(
            &format!("{field_prefix}.trait-source"),
            "run-session ledger does not record a trait file; pass --file <trait.toml>",
        )
    })?;
    let source_path = Utf8PathBuf::from(&source.path);
    // A ledger's recorded relative path belongs to the checkout where it was
    // started, never the checkout from which a later resume happens.
    let path = if source_path.is_relative() {
        source
            .repository_root
            .as_deref()
            .map(Utf8Path::new)
            .map(|root| root.join(&source_path))
            .unwrap_or(source_path)
    } else {
        source_path
    };
    let from_parts =
        |trait_ref: ctx_traits_core::Trait,
         trait_root: Utf8PathBuf,
         source_digest: ctx_traits_core::digest::Digest,
         canonical_digest: ctx_traits_core::digest::Digest| LoadedTrait {
            trait_ref,
            trait_root,
            path: path.clone(),
            source_kind: source.kind.clone(),
            source_digest: source_digest.as_str().to_string(),
            canonical_digest: canonical_digest.as_str().to_string(),
        };
    let explicit = (trait_file.is_some() || trait_id.is_some())
        .then(|| load_trait_source(trait_file, trait_id, field_prefix))
        .transpose()
        .ok()
        .flatten();
    // A valid pin is authoritative on every resume path. An explicit source
    // may supply package context only from the package recorded by the ledger;
    // matching IDs in another repository are not ownership evidence.
    if let Some(pinned) = source.document.as_deref()
        && let package_path = explicit
            .as_ref()
            .filter(|loaded| package_context_matches_session(loaded, session, &path))
            .map(|loaded| loaded.path.as_path())
            .unwrap_or(path.as_path())
        && let Ok((trait_ref, trait_root, source_digest, canonical_digest)) =
            load_trait_text_with_context(&path, package_path, pinned)
    {
        let pinned = from_parts(trait_ref, trait_root, source_digest, canonical_digest);
        if verify_loaded_trait_matches_session(&pinned, session, field_prefix).is_ok() {
            return Ok(pinned);
        }
    }
    // An explicit recovery file and the recorded legacy path are independent
    // candidates. A decodable mismatch from one must not conceal a matching
    // candidate from the other.
    let mut mismatch = None;
    if let Some(loaded) = explicit {
        if verify_loaded_trait_matches_session(&loaded, session, field_prefix).is_ok() {
            return Ok(loaded);
        }
        mismatch = Some(loaded);
    }
    if let Ok((trait_ref, trait_root, source_digest, canonical_digest)) = load_trait(path.as_str())
    {
        let loaded = from_parts(trait_ref, trait_root, source_digest, canonical_digest);
        if verify_loaded_trait_matches_session(&loaded, session, field_prefix).is_ok() {
            return Ok(loaded);
        }
        mismatch.get_or_insert(loaded);
    }
    Err(session_source_mismatch(
        session,
        mismatch.as_ref(),
        field_prefix,
    ))
}

/// An explicit rebuilt source may anchor pinned bytes only when both the
/// document and its enclosing package claim this session's trait identity.
fn package_context_matches_session(
    loaded: &LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    recorded_path: &Utf8Path,
) -> bool {
    if loaded.trait_ref.id.as_str() != session.trait_id {
        return false;
    }
    if crate::layout::package_root_for_manifest(recorded_path) != Some(loaded.trait_root.as_path())
    {
        return false;
    }
    match crate::distribution::read_package_manifest(&loaded.trait_root) {
        Ok(Some(manifest)) => manifest.package.id == session.trait_id,
        // Flat legacy documents are their package's sole identity: they have
        // no [package] manifest to validate separately.
        Ok(None) => loaded.path == loaded.trait_root.join(crate::layout::TRAIT_MANIFEST),
        Err(_) => false,
    }
}

/// Classify the recorded path without decoding or changing the session.
pub fn trait_source_drift(
    session: &ctx_traits_core::procedure::session::Session,
) -> TraitSourceDrift {
    trait_source_drift_from(session, None)
}

/// As [`trait_source_drift`], resolving a relative ledger path from its
/// owning repository rather than the dashboard process's current directory.
pub fn trait_source_drift_from(
    session: &ctx_traits_core::procedure::session::Session,
    repository_root: Option<&Utf8Path>,
) -> TraitSourceDrift {
    let Some(source) = session.provenance.trait_source.as_ref() else {
        return TraitSourceDrift::UnrecoverableLegacy {
            current_source_digest: None,
            recorded_source_digest: session.source_digest.as_ref().map(ToString::to_string),
        };
    };
    let source_path = Utf8Path::new(&source.path);
    let path = if source_path.is_relative() {
        source
            .repository_root
            .as_deref()
            .map(Utf8Path::new)
            .or(repository_root)
            .map(|root| root.join(source_path))
            .unwrap_or_else(|| source_path.to_path_buf())
    } else {
        source_path.to_path_buf()
    };
    let current = crate::read::read_text(&path)
        .ok()
        .map(|text| ctx_traits_core::digest::Digest::source(&text).to_string());
    let expected = session.source_digest.as_ref().map(ToString::to_string);
    let pin_present = source.document.is_some();
    let valid_pin = source.document.as_deref().is_some_and(|document| {
        // `path` is rooted at the repository that owns this session. Keep the
        // ledger path only for selecting its original encoding.
        load_trait_text_with_context(source_path, &path, document)
            .map(|(trait_ref, _, source_digest, canonical_digest)| {
                trait_ref.id.as_str() == session.trait_id
                    && session
                        .source_digest
                        .as_ref()
                        .is_some_and(|expected| source_digest.as_str() == expected.as_str())
                    && session
                        .canonical_digest
                        .as_ref()
                        .is_some_and(|expected| canonical_digest.as_str() == expected.as_str())
            })
            .unwrap_or(false)
    });
    let recorded_source_digest = expected.clone();
    match (valid_pin, current, expected) {
        (_, Some(current), Some(expected)) if current == expected => TraitSourceDrift::Current,
        (true, Some(current_source_digest), _) => TraitSourceDrift::Rebuilt {
            current_source_digest,
        },
        (true, None, _) => TraitSourceDrift::Missing,
        (false, current_source_digest, _) if pin_present => {
            TraitSourceDrift::UnrecoverableInvalidPin {
                current_source_digest,
                recorded_source_digest,
            }
        }
        (false, current_source_digest, _) => TraitSourceDrift::UnrecoverableLegacy {
            current_source_digest,
            recorded_source_digest,
        },
    }
}

fn session_source_mismatch(
    session: &ctx_traits_core::procedure::session::Session,
    loaded: Option<&LoadedTrait>,
    field_prefix: &str,
) -> crate::Error {
    let expected_source = session.source_digest.as_deref().unwrap_or("unknown");
    let expected_canonical = session.canonical_digest.as_deref().unwrap_or("unknown");
    let current_source = loaded
        .map(|loaded| loaded.source_digest.as_str())
        .unwrap_or("unavailable");
    let current_canonical = loaded
        .map(|loaded| loaded.canonical_digest.as_str())
        .unwrap_or("unavailable");
    invalid_request_error(
        &format!("{field_prefix}.trait-source"),
        format!(
            "session {} started at {} expects source {} and canonical {} but current source is {} and canonical is {}; recover the original bytes with --file, or start a fresh run/worktree harvest",
            session.session_id.as_str(),
            session
                .provenance
                .started_at_epoch
                .map(|epoch| epoch.to_string())
                .as_deref()
                .unwrap_or("unknown time"),
            expected_source,
            expected_canonical,
            current_source,
            current_canonical,
        ),
    )
}

pub fn read_session(
    session: &str,
    session_store: Option<&str>,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    let path = crate::run_session::resolve_session_path(session, session_store)?;
    crate::run_session::read_run_session(&path)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Verify the immutable trust evidence accepted at session creation. This is
/// deliberately ledger-only: later approval, block, supersession, or store
/// unreadability governs new starts, never an existing session.
pub(crate) fn validate_pinned_approval(
    session: &ctx_traits_core::procedure::session::Session,
    loaded: &LoadedTrait,
) -> crate::Result<()> {
    let Some(approval) = session.provenance.trust_approval.as_ref() else {
        return invalid_request(
            "run-session.trust-pin",
            "session has no start-time trust approval evidence",
        );
    };
    if approval.trait_id != loaded.trait_ref.id.as_str()
        || approval.canonical_digest.as_str() != loaded.canonical_digest
        || approval.seq == 0
    {
        return invalid_request(
            "run-session.trust-pin",
            "session trust approval evidence does not match its pinned trait bytes",
        );
    }
    Ok(())
}

pub fn parse_initial_sets(
    sets: &[String],
) -> crate::Result<Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>> {
    let mut values = Vec::new();
    for item in sets {
        let Some((target, value)) = item.split_once('=') else {
            return invalid_request(
                "run.set",
                format!("--set value {item:?} must use PORT=VALUE"),
            );
        };
        let ref_text = if target.contains(':') {
            target.to_string()
        } else {
            format!("port:{target}")
        };
        values.push(ctx_traits_core::procedure::runtime::StepSlotOutput {
            ref_text: ref_text.clone(),
            value: Value::String(value.to_string()),
            source: Some(ctx_traits_core::procedure::runtime::ValueSource::CliSet),
            producer_evidence: Some("ctx traits run --set".to_string()),
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        });
    }
    Ok(values)
}

pub fn parse_set_value(value: &str, value_json: bool) -> crate::Result<Value> {
    if value_json {
        Ok(
            serde_json::from_str(value).map_err(|source| crate::parse::Error::JsonDeserialize {
                context: "parse run.set.value --value-json payload".to_string(),
                source,
            })?,
        )
    } else {
        Ok(Value::String(value.to_string()))
    }
}

fn verify_loaded_trait_matches_session(
    loaded: &LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    field_prefix: &str,
) -> crate::Result<()> {
    if loaded.trait_ref.id.as_str() != session.trait_id {
        return invalid_request(
            &format!("{field_prefix}.trait-id"),
            format!(
                "stale run-session source: loaded trait {} does not match session trait {}",
                loaded.trait_ref.id.as_str(),
                session.trait_id
            ),
        );
    }
    if let Some(expected) = session.source_digest.as_deref()
        && loaded.source_digest != expected
    {
        return invalid_request(
            &format!("{field_prefix}.source-digest"),
            "stale run-session source: loaded source digest does not match session ledger",
        );
    }
    if let Some(expected) = session.canonical_digest.as_deref()
        && loaded.canonical_digest != expected
    {
        return invalid_request(
            &format!("{field_prefix}.canonical-digest"),
            "stale run-session source: loaded canonical digest does not match session ledger",
        );
    }
    Ok(())
}

fn session_output_path(
    out: Option<&str>,
    session_store: Option<&str>,
    ephemeral: bool,
    session_id: &str,
) -> crate::Result<Option<Utf8PathBuf>> {
    if let Some(out_path) = out {
        crate::run_session::ensure_explicit_run_session_output(out_path)?;
        return Ok(Some(Utf8PathBuf::from(out_path)));
    }
    if ephemeral {
        return Ok(None);
    }
    Ok(Some(crate::run_session::session_store_path(
        session_store,
        session_id,
    )?))
}

fn write_output_path(out: Option<&str>, fallback: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    if let Some(out) = out {
        crate::run_session::ensure_explicit_run_session_output(out)?;
        return Ok(Utf8PathBuf::from(out));
    }
    Ok(fallback.to_path_buf())
}

/// 0176: fold the resolved budget chain into run-ledger evidence records —
/// one row per field a tier actually stated, carrying the winning tier.
/// Mirrors the resolved-settings records above: evidence only, outside every
/// digest.
fn resolved_budget_records(
    budget: &crate::harness_config::RunProfileBudget,
    provenance: &std::collections::BTreeMap<String, crate::harness_config::BudgetSource>,
) -> Vec<ctx_traits_core::procedure::runtime::ResolvedBudgetRecord> {
    use ctx_traits_core::procedure::runtime::BudgetSourceLayer;
    let value_of = |field: &str| -> Option<serde_json::Value> {
        match field {
            "max-frames" => budget.max_frames.map(Into::into),
            "frame-seconds" => budget.frame_seconds.map(Into::into),
            "total-seconds" => budget.total_seconds.map(Into::into),
            "max-retries" => budget.max_retries.map(Into::into),
            "attach-wait-seconds" => budget.attach_wait_seconds.map(Into::into),
            "idle-seconds" => budget.idle_seconds.map(Into::into),
            "command-seconds" => budget.command_seconds.map(Into::into),
            "command-idle-seconds" => budget.command_idle_seconds.map(Into::into),
            "max-tokens" => budget.max_tokens.map(Into::into),
            "max-cost-usd" => budget.max_cost_usd.and_then(|value| {
                serde_json::Number::from_f64(value).map(serde_json::Value::Number)
            }),
            _ => None,
        }
    };
    provenance
        .iter()
        .filter_map(|(field, source)| {
            Some(ctx_traits_core::procedure::runtime::ResolvedBudgetRecord {
                field: field.clone(),
                value: value_of(field)?,
                source: match source {
                    crate::harness_config::BudgetSource::MachineFallback => {
                        BudgetSourceLayer::MachineFallback
                    }
                    crate::harness_config::BudgetSource::Author => BudgetSourceLayer::Author,
                    crate::harness_config::BudgetSource::TraitScoped => {
                        BudgetSourceLayer::TraitScoped
                    }
                    crate::harness_config::BudgetSource::VariantScoped => {
                        BudgetSourceLayer::VariantScoped
                    }
                },
            })
        })
        .collect()
}

fn apply_default_inputs(
    trait_ref: &ctx_traits_core::Trait,
    initial_values: &mut Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
    config_defaults: &BTreeMap<String, crate::harness_config::ConfiguredPortDefault>,
    exec_dir: Option<&Utf8Path>,
    env_overlay: &BTreeMap<String, String>,
) -> crate::Result<Vec<ctx_traits_core::response::CapabilityReport>> {
    let mut reports = Vec::new();
    let mut existing: std::collections::BTreeSet<String> = initial_values
        .iter()
        .map(|value| value.ref_text.clone())
        .collect();
    for port in trait_ref.ports.iter().filter(|port| {
        matches!(
            port.direction,
            ctx_traits_core::r#trait::PortDirection::Input
        )
    }) {
        let ref_text = format!("port:{}", port.id);
        if existing.contains(&ref_text) {
            continue;
        }
        if let Some(default) = config_defaults.get(&port.id) {
            initial_values.push(ctx_traits_core::procedure::runtime::StepSlotOutput {
                ref_text: ref_text.clone(),
                value: Value::String(default.value.clone()),
                source: Some(ctx_traits_core::procedure::runtime::ValueSource::TraitConfig),
                producer_evidence: Some(default.evidence.clone()),
                command_execution: None,
                producer_agent: None,
                producer_harness: None,
            });
            existing.insert(ref_text);
            continue;
        }
        if let Some(value) = port
            .default
            .as_ref()
            .and_then(|default| default.value.as_ref())
        {
            initial_values.push(ctx_traits_core::procedure::runtime::StepSlotOutput {
                ref_text: ref_text.clone(),
                value: Value::String(value.clone()),
                source: Some(ctx_traits_core::procedure::runtime::ValueSource::DeclaredDefault),
                producer_evidence: Some(format!("port[{}].default.value", port.id)),
                command_execution: None,
                producer_agent: None,
                producer_harness: None,
            });
            existing.insert(ref_text);
            continue;
        }
        let Some(default) = port
            .default
            .as_ref()
            .and_then(|default| default.command.as_ref())
        else {
            continue;
        };
        let (cmd_text, argv) =
            default_command_argv(default, &format!("port[{}].default.command", port.id))?;
        let command_text = cmd_text.unwrap_or_else(|| argv.join(" "));
        let outcome = crate::command::run_with_env(
            crate::command::RunRequest {
                argv: &argv,
                cwd: default.cwd.as_deref(),
                exec_dir,
                success_exit_code: &[0],
                timeout_ms: Some(default.timeout_ms.unwrap_or(DEFAULT_INPUT_TIMEOUT_MS)),
                idle_timeout_ms: None,
                capture_limit: default
                    .capture_bytes
                    .map_or(DEFAULT_INPUT_CAPTURE_LIMIT, |bytes| bytes as usize),
                tick_observer: None,
            },
            env_overlay,
        )?;
        if !outcome.success {
            reports.push(ctx_traits_core::response::CapabilityReport::unsupported(
                format!("runtime.input-default-command.{}", port.id),
                format!(
                    "{ref_text} default command {command_text:?} failed or timed out; provide explicit input with ctx traits --session <id> set {} <value>",
                    port.id
                ),
            ));
            continue;
        }
        // Deliberate asymmetry with the `!outcome.success` branch above: an
        // *absent* input (unsupported report, no slot pushed) is visible to
        // the operator and blocks the frame anyway. A *truncated* one would
        // silently push a cut-off value into a typed input slot that later
        // digests and receipts treat as complete — that corruption must fail
        // the operation instead. Default-input resolution runs before any
        // frame exists, so this `?` is a hard abort of the whole `run` call
        // (there is no individual frame yet to park) — a reasonable reading
        // of "hard frame error" for a pre-frame phase, not a scope narrowing.
        outcome.refuse_if_truncated(&format!("{ref_text} default command {command_text:?}"))?;
        initial_values.push(ctx_traits_core::procedure::runtime::StepSlotOutput {
            ref_text: ref_text.clone(),
            value: Value::String(outcome.stdout),
            source: Some(ctx_traits_core::procedure::runtime::ValueSource::CommandOutput),
            producer_evidence: Some(format!("default command: {}", command_text)),
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
        });
        existing.insert(ref_text);
    }
    reports.sort();
    reports.dedup();
    Ok(reports)
}

fn default_command_argv(
    command: &ctx_traits_core::r#trait::PortDefaultCommand,
    field_path: &str,
) -> crate::Result<(Option<String>, Vec<String>)> {
    if let Some(cmd) = command.cmd.as_deref() {
        let argv = ctx_traits_core::r#trait::procedure::parse_command_shorthand(
            cmd,
            &format!("{field_path}.cmd"),
        )?;
        return Ok((Some(cmd.to_string()), argv));
    }
    if !command.argv.is_empty() {
        return Ok((None, command.argv.clone()));
    }
    invalid_request(
        field_path,
        format!("{field_path}: default command must declare cmd or argv"),
    )
}

fn rebuild_call_response_after_command_advance(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    response: ctx_traits_core::procedure::session::CallResponse,
    persist_path: &Utf8Path,
    exec_dir: Option<&Utf8Path>,
    env_overlay: &BTreeMap<String, String>,
    tick_observer: Option<&crate::harness::TickObserver>,
) -> crate::Result<(
    ctx_traits_core::procedure::session::CallResponse,
    Option<CommandStepFailure>,
)> {
    if !response.persist_session {
        return Ok((response, None));
    }
    let advanced = advance_command_frames(
        trait_ref,
        trait_root,
        response.session,
        Some(persist_path),
        exec_dir,
        env_overlay,
        tick_observer,
    )?;
    let response_kind = run_call_response_kind(&advanced.session.status);
    Ok((
        ctx_traits_core::procedure::session::call_response(advanced.session, response_kind),
        advanced.failure,
    ))
}

fn run_call_response_kind(
    status: &ctx_traits_core::procedure::session::Status,
) -> ctx_traits_core::procedure::session::CallResponseKind {
    match status {
        ctx_traits_core::procedure::session::Status::Completed => {
            ctx_traits_core::procedure::session::CallResponseKind::AcceptedCompleted
        }
        ctx_traits_core::procedure::session::Status::Blocked
        | ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned => {
            ctx_traits_core::procedure::session::CallResponseKind::BlockedMissingInput
        }
        ctx_traits_core::procedure::session::Status::Failed => {
            ctx_traits_core::procedure::session::CallResponseKind::Failed
        }
        ctx_traits_core::procedure::session::Status::Rejected => {
            ctx_traits_core::procedure::session::CallResponseKind::RejectedCorrectionRequired
        }
        _ => ctx_traits_core::procedure::session::CallResponseKind::AcceptedNextFrame,
    }
}

/// A resolved command bound, named by the config key that produced it
/// (0058/0066.4): a step-declared `timeout-ms`/`idle-timeout-ms` names
/// itself, a repository-policy bound names `command-seconds`/
/// `command-idle-seconds`. Carried alongside the resolved millis so a fired
/// bound can be reported by its own name rather than a generic timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBound {
    pub millis: u64,
    pub name: &'static str,
}

/// 0084: resolve the effective wall/idle bounds for a command step. A step
/// that declares a bound owns it; otherwise the repository's configured
/// bound applies, and a step with neither gets `None` (the runner's own
/// defaults). Extracted so the precedence is unit-testable independent of
/// the config-loading and spawn machinery around it.
fn resolve_command_bounds(
    policy: Option<&crate::harness_config::EffectiveRunPolicy>,
    command: &ctx_traits_core::procedure::runtime::CommandFrame,
) -> (Option<ResolvedBound>, Option<ResolvedBound>) {
    let timeout_ms = command
        .timeout_ms
        .map(|millis| ResolvedBound {
            millis,
            name: "timeout-ms",
        })
        .or_else(|| {
            policy
                .and_then(|policy| policy.command_seconds)
                .map(|seconds| ResolvedBound {
                    millis: seconds.saturating_mul(1_000),
                    name: "command-seconds",
                })
        });
    let idle_timeout_ms = command
        .idle_timeout_ms
        .map(|millis| ResolvedBound {
            millis,
            name: "idle-timeout-ms",
        })
        .or_else(|| {
            policy
                .and_then(|policy| policy.command_idle_seconds)
                .map(|seconds| ResolvedBound {
                    millis: seconds.saturating_mul(1_000),
                    name: "command-idle-seconds",
                })
        });
    (timeout_ms, idle_timeout_ms)
}

#[cfg(test)]
mod resolve_command_bounds_tests {
    use super::*;

    fn command_frame(
        timeout_ms: Option<u64>,
        idle_timeout_ms: Option<u64>,
    ) -> ctx_traits_core::procedure::runtime::CommandFrame {
        ctx_traits_core::procedure::runtime::CommandFrame {
            cmd: None,
            argv: vec!["true".to_string()],
            executable_digest: None,
            resource_argv: Vec::new(),
            cwd: None,
            timeout_ms,
            idle_timeout_ms,
            capture_bytes: None,
            success_exit_code: Vec::new(),
            output_slot: "slot:gate".to_string(),
            permission_code: "permission".to_string(),
            reason: "reason".to_string(),
        }
    }

    fn policy(
        command_seconds: Option<u64>,
        command_idle_seconds: Option<u64>,
    ) -> crate::harness_config::EffectiveRunPolicy {
        crate::harness_config::EffectiveRunPolicy {
            worktree: false,
            max_frames: None,
            frame_seconds: None,
            total_seconds: None,
            max_retries: None,
            attach_wait_seconds: None,
            idle_seconds: None,
            command_seconds,
            command_idle_seconds,
            max_in_flight: 1,
            wait: false,
            strict_loops: false,
            inline_prompt_bytes: None,
            story: None,
            usage_warning_threshold: None,
        }
    }

    #[test]
    fn step_only_wins_over_absent_config() {
        let command = command_frame(Some(60_000), Some(5_000));
        let (timeout_ms, idle_timeout_ms) = resolve_command_bounds(None, &command);
        assert_eq!(
            timeout_ms,
            Some(ResolvedBound {
                millis: 60_000,
                name: "timeout-ms"
            })
        );
        assert_eq!(
            idle_timeout_ms,
            Some(ResolvedBound {
                millis: 5_000,
                name: "idle-timeout-ms"
            })
        );
    }

    #[test]
    fn config_only_applies_when_step_declares_nothing() {
        let command = command_frame(None, None);
        let policy = policy(Some(120), Some(10));
        let (timeout_ms, idle_timeout_ms) = resolve_command_bounds(Some(&policy), &command);
        assert_eq!(
            timeout_ms,
            Some(ResolvedBound {
                millis: 120_000,
                name: "command-seconds"
            })
        );
        assert_eq!(
            idle_timeout_ms,
            Some(ResolvedBound {
                millis: 10_000,
                name: "command-idle-seconds"
            })
        );
    }

    #[test]
    fn step_wins_over_config_when_both_declare() {
        let command = command_frame(Some(3_600_000), Some(30_000));
        let policy = policy(Some(120), Some(10));
        let (timeout_ms, idle_timeout_ms) = resolve_command_bounds(Some(&policy), &command);
        assert_eq!(
            timeout_ms,
            Some(ResolvedBound {
                millis: 3_600_000,
                name: "timeout-ms"
            })
        );
        assert_eq!(
            idle_timeout_ms,
            Some(ResolvedBound {
                millis: 30_000,
                name: "idle-timeout-ms"
            })
        );
    }

    #[test]
    fn neither_declares_yields_none_for_runner_defaults() {
        let command = command_frame(None, None);
        let policy = policy(None, None);
        let (timeout_ms, idle_timeout_ms) = resolve_command_bounds(Some(&policy), &command);
        assert_eq!(timeout_ms, None);
        assert_eq!(idle_timeout_ms, None);
    }
}

struct CommandAdvance {
    session: ctx_traits_core::procedure::session::Session,
    failure: Option<CommandStepFailure>,
}

fn advance_command_frames(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    mut session: ctx_traits_core::procedure::session::Session,
    persist_path: Option<&Utf8Path>,
    exec_dir: Option<&Utf8Path>,
    env_overlay: &BTreeMap<String, String>,
    tick_observer: Option<&crate::harness::TickObserver>,
) -> crate::Result<CommandAdvance> {
    loop {
        if session.status
            != ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired
        {
            return Ok(CommandAdvance {
                session,
                failure: None,
            });
        }
        let Some(frame) = session.next_frame.as_ref() else {
            return Ok(CommandAdvance {
                session,
                failure: None,
            });
        };
        let Some(command) = frame.command.as_ref() else {
            return Ok(CommandAdvance {
                session,
                failure: None,
            });
        };
        let argv = command.argv.clone();
        let resource_argv = command.resource_argv.clone();
        let executable_digest = command.executable_digest.clone();
        verify_command_executable_digest(&argv, executable_digest.as_ref())?;
        // Resolve declared resource argv positions through the shared
        // protected-resource verifier immediately before spawn: a
        // process-only argv gets the verified absolute path spliced in at
        // exactly the recorded positions, while `argv` above stays the
        // logical text persisted in command evidence below.
        let referenced_resources: Vec<ctx_traits_core::r#trait::Resource> = resource_argv
            .iter()
            .filter_map(|entry| {
                let resource_id = entry
                    .resource_ref
                    .strip_prefix("resource:")
                    .unwrap_or(entry.resource_ref.as_str());
                trait_ref
                    .resources
                    .iter()
                    .find(|r| r.id == resource_id)
                    .cloned()
            })
            .collect();
        let resource_roots =
            crate::resource::resolve_resource_roots(trait_root, &referenced_resources)?;
        let process_argv =
            resolve_resource_argv_for_spawn(&resource_roots, trait_ref, &argv, &resource_argv)?;
        let cwd = command.cwd.clone();
        let success_exit_code = command.success_exit_code.clone();
        // 0058/0084: how long a command step may take is a property of the
        // machine and the project by default, so the repository's own
        // config governs the silent majority of steps that declare neither
        // bound. A step that authors `timeout-ms`/`idle-timeout-ms` is the
        // one exception: it knows its own workload better than the repo's
        // blanket policy, so its declared bound wins over config for that
        // step alone. The wall stays the backstop regardless — it is
        // enforced independently of idle in the runner loop, so a step with
        // a long idle budget still dies on an unrelated, shorter wall.
        // Resolved here rather than threaded from the caller because a
        // command step is the only consumer, and this runs once per gate.
        let command_policy = exec_dir
            .or(Some(Utf8Path::new(".")))
            .and_then(|dir| crate::harness_config::resolve_runtime_config(dir).ok())
            .map(|config| config.effective_run_policy());
        let (wall_bound, idle_bound) = resolve_command_bounds(command_policy.as_ref(), command);
        let timeout_ms = wall_bound.map(|bound| bound.millis);
        let idle_timeout_ms = idle_bound.map(|bound| bound.millis);
        let capture_limit = command
            .capture_bytes
            .map_or(COMMAND_CAPTURE_LIMIT, |bytes| bytes as usize);
        let output_slot = command.output_slot.clone();
        let call_template = frame.call_template.clone();
        let run_id = frame.run_id.clone();
        let item_id = frame.item_id.clone();
        let run_index = frame.run_index;
        let sequence_index = frame.sequence_index;
        let position_path = frame.position_path.clone();
        let outcome = crate::command::run_with_env(
            crate::command::RunRequest {
                argv: &process_argv,
                cwd: cwd.as_deref(),
                exec_dir,
                success_exit_code: &success_exit_code,
                timeout_ms,
                idle_timeout_ms,
                // Command steps may emit structured payloads (annotation JSON,
                // tool reports) far larger than terminal-style output; 256 KiB
                // keeps them intact without letting a runaway stream flood the
                // slot ledger. A declared `capture-bytes` on the command item
                // overrides this default.
                capture_limit,
                tick_observer: tick_observer.cloned(),
            },
            env_overlay,
        )?;
        if frame.kind == ctx_traits_core::procedure::runtime::SequenceFrameKind::Check {
            let verdict = outcome.success;
            let mut warnings = vec![format!(
                "check verdict {verdict} accepted from trusted local runtime"
            )];
            if !outcome.stdout.trim().is_empty()
                || !outcome.stderr.trim().is_empty()
                || outcome.stdout_truncated
                || outcome.stderr_truncated
            {
                warnings.push(format!(
                    "command report: {}",
                    format_command_report(&outcome)
                ));
            }
            let mut produced_slots = std::collections::BTreeMap::new();
            // P565: built by core's one constructor, never assembled here —
            // core re-derives the same value from trusted evidence and
            // compares byte-for-byte, so a second local definition of the
            // shape would silently reject every check.
            // Built before the value, because the verdict record now carries
            // the exit code and — on failure — a bounded tail of the output,
            // all of which come from this evidence.
            let submission_evidence =
                ctx_traits_core::procedure::session::CommandExecutionEvidence {
                    argv: argv.clone(),
                    output_slot: output_slot.clone(),
                    executable_digest: executable_digest.clone(),
                    exit_code: outcome.exit_code,
                    timed_out: outcome.timed_out,
                    stdout: (!outcome.stdout.is_empty()).then(|| outcome.stdout.clone()),
                    stderr: (!outcome.stderr.is_empty()).then(|| outcome.stderr.clone()),
                    stdout_truncated: outcome.stdout_truncated,
                    stderr_truncated: outcome.stderr_truncated,
                };
            produced_slots.insert(
                output_slot.clone(),
                ctx_traits_core::procedure::session::check_output_value(
                    verdict,
                    command,
                    &ctx_traits_core::procedure::session::CheckEvidence::from_submission(
                        &submission_evidence,
                    ),
                ),
            );
            let response = ctx_traits_core::procedure::session::submit_run_call(
                trait_ref,
                session,
                ctx_traits_core::procedure::session::CallSubmission {
                    session_id: call_template
                        .as_ref()
                        .map(|template| template.session_id.clone())
                        .and_then(|id| ctx_traits_core::procedure::session::SessionId::new(id).ok())
                        .ok_or_else(|| {
                            invalid_request_error(
                                "run.command.call-template.session-id",
                                "check frame missing call template session id",
                            )
                        })?,
                    run_id: Some(ctx_traits_core::procedure::run::Id::new(run_id)?),
                    state_digest: call_template
                        .as_ref()
                        .map(|template| template.state_digest.clone()),
                    expected_sequence_item_id: item_id,
                    expected_run_index: run_index,
                    expected_source_index: sequence_index,
                    expected_position_path: position_path,
                    produced_slots,
                    signals: std::collections::BTreeMap::new(),
                    warnings,
                    command_execution: Some(submission_evidence),
                    caller: Some(ctx_traits_core::procedure::session::CallerProvenance {
                        surface: "local-runtime-command".to_string(),
                        caller: "ctx traits trusted local runtime".to_string(),
                        agent: None,
                        harness: None,
                    }),
                },
            )?;
            if response.persist_session
                && let Some(path) = persist_path
            {
                crate::run_session::write_run_session(path, &response.session)?;
            }
            session = response.session;
            continue;
        }
        // A typed output slot (anything beyond schema:text / schema:any /
        // absent) receives the parsed JSON stdout instead of the report
        // envelope, so declared schemas validate against the real payload.
        let output_route = classify_command_output_route(trait_ref, &output_slot);
        let mut typed_value: Option<Value> = None;
        let mut typed_parse_warning: Option<String> = None;
        if outcome.success && output_route == CommandOutputRoute::Typed {
            match serde_json::from_str::<Value>(outcome.stdout.trim()) {
                Ok(parsed) => typed_value = Some(parsed),
                // Submit the raw stdout instead: command evidence stays
                // consistent (one produced slot) and schema validation
                // rejects it with the typed reason.
                Err(error) => {
                    typed_parse_warning = Some(format!(
                        "command stdout is not valid JSON for typed output slot {output_slot}: {error}"
                    ));
                    typed_value = Some(Value::String(outcome.stdout.trim().to_string()));
                }
            }
        }
        // The Check route above already returned via `continue`: its verdict
        // derives from the exit code alone, which truncation cannot corrupt,
        // so a truncated Check capture stays warning-only by construction.
        // Every other route (Text, Typed, Envelope) feeds the captured
        // stdout forward into a slot the frame trusts as complete, so a
        // truncated capture is parked through the same failure machinery as
        // a genuine command failure rather than silently landing a cut-off
        // value.
        // Keys on stdout_truncated only: stdout is what lands in the slot, so
        // it is the only truncation that can corrupt forwarded state. A
        // truncated stderr is reported (both flags ride along in `report`
        // below) but never itself parks the step.
        if !outcome.success || outcome.stdout_truncated {
            let reason = if outcome.stdout_truncated {
                format!(
                    "command output truncated at {} bytes",
                    outcome.capture_limit
                )
            } else {
                "trusted local command failed".to_string()
            };
            let bound_fired = outcome.timeout_kind.map(|kind| match kind {
                crate::command::TimeoutKind::Idle => idle_bound.unwrap_or(ResolvedBound {
                    millis: crate::command::DEFAULT_COMMAND_IDLE_MS,
                    name: "default idle window",
                }),
                crate::command::TimeoutKind::Wall => wall_bound.unwrap_or(ResolvedBound {
                    millis: crate::command::DEFAULT_COMMAND_WALL_MS,
                    name: "default wall clock",
                }),
            });
            let bound_fired = bound_fired.map(|bound| BoundFired {
                name: bound.name,
                millis: bound.millis,
                kind: outcome
                    .timeout_kind
                    .expect("bound_fired is only Some when timeout_kind is Some"),
            });
            let report = format_command_report_with_bound(&outcome, bound_fired.as_ref());
            let response = ctx_traits_core::procedure::session::submit_run_call(
                trait_ref,
                session,
                ctx_traits_core::procedure::session::CallSubmission {
                    session_id: call_template
                        .as_ref()
                        .map(|template| template.session_id.clone())
                        .and_then(|id| ctx_traits_core::procedure::session::SessionId::new(id).ok())
                        .ok_or_else(|| {
                            invalid_request_error(
                                "run.command.call-template.session-id",
                                "command frame missing call template session id",
                            )
                        })?,
                    run_id: Some(ctx_traits_core::procedure::run::Id::new(run_id)?),
                    state_digest: call_template
                        .as_ref()
                        .map(|template| template.state_digest.clone()),
                    expected_sequence_item_id: item_id.clone(),
                    expected_run_index: run_index,
                    expected_source_index: sequence_index,
                    expected_position_path: position_path,
                    produced_slots: std::collections::BTreeMap::new(),
                    signals: std::collections::BTreeMap::new(),
                    warnings: vec![reason],
                    command_execution: Some(
                        ctx_traits_core::procedure::session::CommandExecutionEvidence {
                            argv: argv.clone(),
                            output_slot,
                            executable_digest: executable_digest.clone(),
                            exit_code: outcome.exit_code,
                            timed_out: outcome.timed_out,
                            stdout: None,
                            stderr: None,
                            stdout_truncated: outcome.stdout_truncated,
                            stderr_truncated: outcome.stderr_truncated,
                        },
                    ),
                    caller: Some(ctx_traits_core::procedure::session::CallerProvenance {
                        surface: "local-runtime-command".to_string(),
                        caller: "ctx traits trusted local runtime".to_string(),
                        agent: None,
                        harness: None,
                    }),
                },
            )?;
            if response.persist_session
                && let Some(path) = persist_path
            {
                crate::run_session::write_run_session(path, &response.session)?;
            }
            if matches!(
                response.response_kind,
                ctx_traits_core::procedure::session::CallResponseKind::AcceptedNextFrame
                    | ctx_traits_core::procedure::session::CallResponseKind::AcceptedCompleted
            ) {
                session = response.session;
                continue;
            }
            return Ok(CommandAdvance {
                session: response.session,
                failure: Some(CommandStepFailure {
                    item_id,
                    argv,
                    exit_code: outcome.exit_code,
                    timed_out: outcome.timed_out,
                    report,
                    bound_fired,
                }),
            });
        }
        let mut warnings = vec!["command output accepted from trusted local runtime".to_string()];
        if let Some(parse_warning) = typed_parse_warning {
            warnings.push(parse_warning);
        }
        let value = match output_route {
            CommandOutputRoute::Envelope => Value::String(format_command_report(&outcome)),
            CommandOutputRoute::Text => {
                // The envelope no longer reaches a schema:text slot; the slot
                // receives outcome.stdout verbatim (no trimming), and stderr /
                // truncation facts are preserved as submission warnings.
                if !outcome.stderr.trim().is_empty()
                    || outcome.stdout_truncated
                    || outcome.stderr_truncated
                {
                    warnings.push(format!(
                        "command report: {}",
                        format_command_report(&outcome)
                    ));
                }
                Value::String(outcome.stdout.clone())
            }
            CommandOutputRoute::Typed => {
                let parsed = typed_value.expect("typed route always sets typed_value on success");
                // The envelope no longer reaches the slot; keep stderr and
                // truncation facts as submission warnings so they persist.
                if !outcome.stderr.trim().is_empty()
                    || outcome.stdout_truncated
                    || outcome.stderr_truncated
                {
                    warnings.push(format!(
                        "command report: {}",
                        format_command_report(&outcome)
                    ));
                }
                parsed
            }
        };
        let mut produced_slots = std::collections::BTreeMap::new();
        produced_slots.insert(output_slot.clone(), value);
        let response = ctx_traits_core::procedure::session::submit_run_call(
            trait_ref,
            session,
            ctx_traits_core::procedure::session::CallSubmission {
                session_id: call_template
                    .as_ref()
                    .map(|template| template.session_id.clone())
                    .and_then(|id| ctx_traits_core::procedure::session::SessionId::new(id).ok())
                    .ok_or_else(|| {
                        invalid_request_error(
                            "run.command.call-template.session-id",
                            "command frame missing call template session id",
                        )
                    })?,
                run_id: Some(ctx_traits_core::procedure::run::Id::new(run_id)?),
                state_digest: call_template
                    .as_ref()
                    .map(|template| template.state_digest.clone()),
                expected_sequence_item_id: item_id,
                expected_run_index: run_index,
                expected_source_index: sequence_index,
                expected_position_path: position_path,
                produced_slots,
                signals: std::collections::BTreeMap::new(),
                warnings,
                command_execution: Some(
                    ctx_traits_core::procedure::session::CommandExecutionEvidence {
                        argv: argv.clone(),
                        output_slot,
                        executable_digest: executable_digest.clone(),
                        exit_code: outcome.exit_code,
                        timed_out: outcome.timed_out,
                        stdout: None,
                        stderr: None,
                        stdout_truncated: false,
                        stderr_truncated: false,
                    },
                ),
                caller: Some(ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "local-runtime-command".to_string(),
                    caller: "ctx traits trusted local runtime".to_string(),
                    agent: None,
                    harness: None,
                }),
            },
        )?;
        // Persist each accepted command step immediately so a later failing
        // step cannot roll back this one.
        if response.persist_session
            && let Some(path) = persist_path
        {
            crate::run_session::write_run_session(path, &response.session)?;
        }
        session = response.session;
    }
}

/// Build the process-only argv a command step actually spawns: every
/// declared resource argv position is verified against its pin and replaced
/// with the verified absolute path; every other position is copied through
/// from the logical argv unchanged. Refuses to spawn (fails closed) rather
/// than run when a resource argv reference is undeclared, unpinned, or its
/// bytes fail verification — this is the runtime's own gate, independent of
/// `ctx traits check`, so bypassing `check` can never execute an unprotected
/// or drifted resource.
fn resolve_resource_argv_for_spawn(
    roots: &crate::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
    argv: &[String],
    resource_argv: &[ctx_traits_core::procedure::runtime::ResourceArgvRef],
) -> crate::Result<Vec<String>> {
    if resource_argv.is_empty() {
        return Ok(argv.to_vec());
    }
    let mut process_argv = argv.to_vec();
    for entry in resource_argv {
        let resource_id = entry
            .resource_ref
            .strip_prefix("resource:")
            .unwrap_or(entry.resource_ref.as_str());
        let Some(resource) = trait_ref.resources.iter().find(|r| r.id == resource_id) else {
            return invalid_request(
                "run.command.resource-argv",
                format!(
                    "command argv resource {:?} is not declared in the loaded trait",
                    entry.resource_ref
                ),
            );
        };
        if !resource.is_protected() {
            return invalid_request(
                "run.command.resource-argv",
                format!(
                    "command argv resource {:?} must be pinned with digest before it can be launched as code",
                    entry.resource_ref
                ),
            );
        }
        match crate::resource::verify_protected_resource(roots, resource)? {
            crate::resource::ProtectionVerification::Verified { path } => {
                let Some(slot) = process_argv.get_mut(entry.index) else {
                    return invalid_request(
                        "run.command.resource-argv",
                        format!(
                            "command argv resource {:?} index {} is out of bounds",
                            entry.resource_ref, entry.index
                        ),
                    );
                };
                *slot = path.to_string();
            }
            crate::resource::ProtectionVerification::Unprotected => {
                unreachable!("is_protected() checked above")
            }
            crate::resource::ProtectionVerification::Failed(failure) => {
                return invalid_request("run.command.resource-argv", failure.to_string());
            }
        }
    }
    Ok(process_argv)
}

fn verify_command_executable_digest(
    argv: &[String],
    expected: Option<&ctx_traits_core::digest::Digest>,
) -> crate::Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let executable = argv.first().ok_or_else(|| {
        invalid_request_error("run.command.argv", "digest-verified command has empty argv")
    })?;
    let path = Utf8Path::new(executable);
    if !path.is_absolute() {
        return invalid_request(
            "run.command.argv[0]",
            "digest-verified command executable must be an absolute path",
        );
    }
    let bytes = std::fs::read(path).map_err(|error| {
        invalid_request_error(
            "run.command.executable-digest",
            format!("cannot read executable {path}: {error}"),
        )
    })?;
    let actual = ctx_traits_core::digest::Digest::from_bytes(&bytes);
    if &actual != expected {
        return invalid_request(
            "run.command.executable-digest",
            format!("executable {path} digest mismatch: expected {expected}, got {actual}"),
        );
    }
    Ok(())
}

/// How a successful command's stdout is routed into its declared output
/// slot. `schema:text` receives raw stdout verbatim; `schema:any` and an
/// absent schema keep the legacy formatted envelope; every other declared
/// schema parses stdout as JSON and validates it against that schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOutputRoute {
    Text,
    Envelope,
    Typed,
}

/// Classify a command's output slot into its P397 routing behavior by
/// inspecting the slot's declared schema (if any).
fn classify_command_output_route(
    trait_ref: &ctx_traits_core::Trait,
    output_slot: &str,
) -> CommandOutputRoute {
    let Some(slot_id) = output_slot.strip_prefix("slot:") else {
        return CommandOutputRoute::Envelope;
    };
    let Some(schema) = trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == slot_id)
        .and_then(|slot| slot.schema.as_ref())
    else {
        return CommandOutputRoute::Envelope;
    };
    match schema {
        ctx_traits_core::schema::form::Schema::Builtin(
            ctx_traits_core::schema::form::Builtin::Text,
        ) => CommandOutputRoute::Text,
        ctx_traits_core::schema::form::Schema::Builtin(
            ctx_traits_core::schema::form::Builtin::Any,
        ) => CommandOutputRoute::Envelope,
        _ => CommandOutputRoute::Typed,
    }
}

fn format_command_report(outcome: &crate::command::RunOutput) -> String {
    format_command_report_with_bound(outcome, None)
}

fn format_command_report_with_bound(
    outcome: &crate::command::RunOutput,
    bound_fired: Option<&BoundFired>,
) -> String {
    // 0058/0066.4: when a bound fired, name WHICH one and its configured
    // value — "no output within its idle window — command-idle-seconds (2s)"
    // — never a bare "timed out". Neither is the worker's defect.
    let timeout_line = outcome
        .timeout_reason
        .map(|reason| match bound_fired {
            Some(bound) => format!("timeout: {reason} — {}\n", bound.describe()),
            None => format!("timeout: {reason}\n"),
        })
        .unwrap_or_default();
    format!(
        "{timeout_line}exit-code: {:?}\nstdout-truncated: {}\nstderr-truncated: {}\nstdout:\n{}\nstderr:\n{}",
        outcome.exit_code,
        outcome.stdout_truncated,
        outcome.stderr_truncated,
        outcome.stdout,
        outcome.stderr
    )
}

fn declared_resource_evidence(
    roots: &crate::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<ctx_traits_core::procedure::runtime::ResourceEvidence>> {
    let manifest =
        crate::resource::digest_resources(roots, trait_ref.id.as_str(), &trait_ref.resources)?;
    let mut evidence = runtime_resource_evidence_from_manifest(&manifest)
        .into_iter()
        .map(|entry| {
            Ok(ctx_traits_core::procedure::runtime::ResourceEvidence {
                resource_ref: ctx_traits_core::reference::Reference::parse(&format!(
                    "resource:{}",
                    entry.resource_id
                ))?,
                digest: entry.digest.clone(),
                byte_size: entry.byte_size,
                is_binary: entry.is_binary,
                // A directory resource (the task-board shape, 2026-07-31) has
                // no file digest by construction and is still a presentable
                // on-demand input: agents open its files with their own tools.
                available: (entry.digest.is_some()
                    && !entry.missing_file
                    && !entry.symlink_detected)
                    || entry.is_directory,
                reason: if entry.is_directory {
                    "resource is a directory; agents read its files with their own tools"
                        .to_string()
                } else if entry.digest.is_some() {
                    "resource digest available".to_string()
                } else if entry.symlink_detected {
                    "resource path contains symlink".to_string()
                } else if entry.missing_file {
                    "resource file missing".to_string()
                } else {
                    "resource unavailable".to_string()
                },
            })
        })
        .collect::<Result<Vec<_>, ctx_traits_core::Error>>()?;
    evidence.extend(inline_resource_evidence(trait_ref)?);
    Ok(evidence)
}

/// Materialize resource evidence for local dependency packages at the IO
/// boundary. The pure runtime receives only qualified, digest-carrying evidence
/// and therefore never resolves paths or reads files itself.
fn declared_dependency_resource_evidence(
    trait_root: &Utf8Path,
    invocation_repo_root: Option<&Utf8Path>,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<ctx_traits_core::procedure::runtime::ResourceEvidence>> {
    let mut evidence = Vec::new();
    for dependency in &trait_ref.dependencies {
        let Some(ctx_traits_core::manifest::TraitSource::Local { path }) =
            dependency.source.as_ref()
        else {
            continue;
        };
        let dependency_root = trait_root.join(path);
        let dependency_manifest = crate::layout::resolve_package_manifest(&dependency_root)
            .unwrap_or_else(|| {
                dependency_root
                    .join(crate::layout::GENERATED)
                    .join(crate::layout::TRAIT_MANIFEST)
            });
        let text = crate::read::read_text(&dependency_manifest)?;
        let (dependency_trait, dependency_warnings) =
            ctx_traits_core::encoding::decode_trait_with_warnings(
                ctx_traits_core::encoding::Encoding::Toml,
                &text,
            )?;
        crate::decode_diagnostics::print_decode_warnings(
            dependency_manifest.as_str(),
            &dependency_warnings,
        );
        if dependency_trait.id.as_str() != dependency.id {
            return Err(crate::Error::from(ctx_traits_core::Error::from(
                ctx_traits_core::manifest::Error::InvalidField {
                    field_path: format!("dependency[{}].id", dependency.alias),
                    message: format!(
                        "local package identity mismatch: declared {}, found {}",
                        dependency.id, dependency_trait.id
                    ),
                },
            )));
        }
        if dependency_trait.version.as_str() != dependency.version {
            return Err(crate::Error::from(ctx_traits_core::Error::from(
                ctx_traits_core::manifest::Error::InvalidField {
                    field_path: format!("dependency[{}].version", dependency.alias),
                    message: format!(
                        "local package version mismatch: declared {}, found {}",
                        dependency.version, dependency_trait.version
                    ),
                },
            )));
        }
        // `ctx traits check` already reports lifecycle/trust for a
        // dependency (`dependency_trust_summary` in report_check.rs), but
        // that report is advisory: nothing enforced it before this
        // dependency's declared resource files were read off disk and
        // materialized into a running session's resource evidence (which the
        // model goes on to see). Re-resolve status and trust here through
        // the same shared lifecycle gate used for the primary trait, keyed
        // by the dependency's own current canonical digest, so a draft,
        // unreviewed, blocked, or digest-changed dependency is refused at
        // the point its resources would actually be read, not just reported
        // as a check-time warning.
        let dependency_canonical_digest =
            ctx_traits_core::digest::canonical_digest(&dependency_trait)?;
        let (dependency_status, dependency_trust) = crate::lifecycle::resolve_named(
            &dependency_root,
            dependency_trait.id.as_str(),
            dependency_canonical_digest.as_str(),
        )?;
        let dependency_gates =
            ctx_traits_core::r#trait::activation::lifecycle_trust_gates_for_check(
                dependency.id.as_str(),
                &dependency_status,
                &dependency_trust,
            );
        if !dependency_gates.is_empty() {
            return Err(crate::Error::from(ctx_traits_core::Error::from(
                ctx_traits_core::manifest::Error::InvalidField {
                    field_path: format!("dependency[{}]", dependency.alias),
                    message: format!(
                        "dependency {} (canonical-digest={}) is not ready+verified in this \
                         machine's lifecycle/trust state ({}); its declared resources will not \
                         be materialized",
                        dependency.id,
                        dependency_canonical_digest.as_str(),
                        ctx_traits_core::r#trait::activation::format_gate_refusal(
                            &dependency_gates
                        )
                    ),
                },
            )));
        }
        let dependency_roots = match invocation_repo_root {
            Some(repo_root) => crate::resource::ResourceRoots::with_invocation_repo(
                &dependency_root,
                Some(repo_root.to_path_buf()),
            ),
            None => crate::resource::resolve_resource_roots(
                &dependency_root,
                &dependency_trait.resources,
            )?,
        };
        let manifest = crate::resource::digest_resources(
            &dependency_roots,
            dependency_trait.id.as_str(),
            &dependency_trait.resources,
        )?;
        evidence.extend(
            runtime_resource_evidence_from_manifest(&manifest)
                .into_iter()
                .map(|entry| {
                    Ok(ctx_traits_core::procedure::runtime::ResourceEvidence {
                        resource_ref: ctx_traits_core::reference::Reference::parse(&format!(
                            "resource:{}/{}",
                            dependency.alias, entry.resource_id
                        ))?,
                        digest: entry.digest.clone(),
                        byte_size: entry.byte_size,
                        is_binary: entry.is_binary,
                        available: entry.digest.is_some()
                            && !entry.missing_file
                            && !entry.symlink_detected,
                        reason: if entry.digest.is_some() {
                            "dependency resource digest available".to_string()
                        } else if entry.symlink_detected {
                            "dependency resource path contains symlink".to_string()
                        } else if entry.missing_file {
                            "dependency resource file missing".to_string()
                        } else {
                            "dependency resource unavailable".to_string()
                        },
                    })
                })
                .collect::<Result<Vec<_>, ctx_traits_core::Error>>()?,
        );
        evidence.extend(
            inline_resource_evidence(&dependency_trait)?
                .into_iter()
                .map(|entry| {
                    let resource_id = entry.resource_ref.id();
                    Ok(ctx_traits_core::procedure::runtime::ResourceEvidence {
                        resource_ref: ctx_traits_core::reference::Reference::parse(&format!(
                            "resource:{}/{}",
                            dependency.alias, resource_id
                        ))?,
                        ..entry
                    })
                })
                .collect::<Result<Vec<_>, ctx_traits_core::Error>>()?,
        );
    }
    Ok(evidence)
}

fn inline_resource_evidence(
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<ctx_traits_core::procedure::runtime::ResourceEvidence>> {
    Ok(trait_ref
        .resources
        .iter()
        .filter_map(|resource| {
            // A checklist has no `content`: its body is the deterministic
            // rendering of its typed items, so the digest tracks the items
            // themselves and a reworded criterion shows up in the receipt.
            if resource.is_checklist() {
                let rendered = ctx_traits_core::r#trait::checklist::render_items(resource);
                return Some((resource, rendered, "checklist items available"));
            }
            resource.content.as_deref().map(|content| {
                (
                    resource,
                    content.to_string(),
                    "inline resource content available",
                )
            })
        })
        .map(|(resource, body, reason)| {
            Ok(ctx_traits_core::procedure::runtime::ResourceEvidence {
                resource_ref: ctx_traits_core::reference::Reference::parse(&format!(
                    "resource:{}",
                    resource.id
                ))?,
                digest: Some(ctx_traits_core::digest::Digest::source(&body)),
                byte_size: body.len() as u64,
                is_binary: false,
                available: true,
                reason: reason.to_string(),
            })
        })
        .collect::<Result<Vec<_>, ctx_traits_core::Error>>()?)
}

fn runtime_resource_evidence_from_manifest(
    manifest: &crate::resource::ResourceManifestDigest,
) -> Vec<ctx_traits_core::resource_plan::FileEvidence> {
    use crate::resource::ResourceReadWarning;

    let digest_map: std::collections::BTreeMap<&str, &crate::resource::ResourceFileDigest> =
        manifest
            .file_digests
            .iter()
            .map(|fd| (fd.resource_id.as_str(), fd))
            .collect();

    let mut missing_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut symlinks: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut special_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut directories: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for warning in &manifest.warnings {
        match warning {
            ResourceReadWarning::MissingFile { resource_id, .. } => {
                missing_files.insert(resource_id.clone());
            }
            ResourceReadWarning::SymlinkDetected { resource_id, .. } => {
                symlinks.insert(resource_id.clone());
            }
            ResourceReadWarning::SpecialFile { resource_id, .. } => {
                special_files.insert(resource_id.clone());
            }
            ResourceReadWarning::Directory { resource_id, .. } => {
                directories.insert(resource_id.clone());
            }
            ResourceReadWarning::BinaryContent { .. } => {}
        }
    }

    let mut all_ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for fd in &manifest.file_digests {
        all_ids.insert(fd.resource_id.as_str());
    }
    for id in &missing_files {
        all_ids.insert(id.as_str());
    }
    for id in &symlinks {
        all_ids.insert(id.as_str());
    }
    for id in &special_files {
        all_ids.insert(id.as_str());
    }
    for id in &directories {
        all_ids.insert(id.as_str());
    }

    all_ids
        .iter()
        .map(|&rid| {
            let fd = digest_map.get(rid);
            ctx_traits_core::resource_plan::file_evidence_from_io(
                rid,
                fd.map(|d| &d.digest),
                fd.map(|d| d.byte_size).unwrap_or(0),
                fd.map(|d| d.is_binary).unwrap_or(false),
                missing_files.contains(rid),
                symlinks.contains(rid),
                directories.contains(rid),
            )
        })
        .collect()
}

fn unavailable_resource_evidence(
    trait_ref: &ctx_traits_core::Trait,
    reason: &str,
) -> crate::Result<Vec<ctx_traits_core::procedure::runtime::ResourceEvidence>> {
    Ok(trait_ref
        .resources
        .iter()
        .map(|resource| {
            Ok(ctx_traits_core::procedure::runtime::ResourceEvidence {
                resource_ref: ctx_traits_core::reference::Reference::parse(&format!(
                    "resource:{}",
                    resource.id
                ))?,
                digest: None,
                byte_size: 0,
                is_binary: false,
                available: false,
                reason: reason.to_string(),
            })
        })
        .collect::<Result<Vec<_>, ctx_traits_core::Error>>()?)
}

fn invalid_request<T>(field_path: &str, message: impl Into<String>) -> crate::Result<T> {
    Err(invalid_request_error(field_path, message))
}

fn invalid_request_error(field_path: &str, message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: message.into(),
        }
        .into(),
    )
}

#[cfg(test)]
mod family_variant_at_package_root_tests {
    use super::*;

    fn scratch_root(tag: &str) -> Utf8PathBuf {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let dir = root.join(format!(
            "ctx-family-variant-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(dir.as_std_path()).expect("clear stale scratch dir");
        }
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    fn family_variant_trait_doc(id: &str, variant: &str) -> String {
        format!(
            r#"id = "{id}"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "scratch fixture"
variant = "{variant}"
"#
        )
    }

    /// A package root with a `[family]` table declaring `default` and
    /// `quick` variants, both backed by real canonical files — the
    /// unambiguous "hit" fixture every other test in this module starts
    /// from.
    fn write_family_package(root: &Utf8Path) {
        std::fs::create_dir_all(root.join("generated/quick").as_std_path()).unwrap();
        std::fs::create_dir_all(root.join("generated/default").as_std_path()).unwrap();
        std::fs::write(
            root.join("trait.toml").as_std_path(),
            r#"[package]
id = "family-demo"
version = "0.1.0"
name = "Family Demo"
status = "ready"

[family]
default = "default"

[family.variant.default]
path = "generated/default/index.toml"

[family.variant.quick]
path = "generated/quick/index.toml"
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("generated/default/index.toml").as_std_path(),
            family_variant_trait_doc("family-demo", "default"),
        )
        .unwrap();
        std::fs::write(
            root.join("generated/quick/index.toml").as_std_path(),
            family_variant_trait_doc("family-demo", "quick"),
        )
        .unwrap();
    }

    #[test]
    fn resolves_a_declared_variant_to_its_canonical_file() {
        let root = scratch_root("hit");
        write_family_package(&root);
        let resolved = resolve_family_variant_at_package_root(&root, "family-demo", "quick")
            .expect("resolves without error")
            .expect("variant is declared");
        assert_eq!(resolved.0, root.join("generated/quick/index.toml"));
        assert_eq!(resolved.1, "trait-id");
    }

    #[test]
    fn returns_none_for_an_undeclared_variant() {
        let root = scratch_root("miss");
        write_family_package(&root);
        let resolved = resolve_family_variant_at_package_root(&root, "family-demo", "nonexistent")
            .expect("no error for an absent variant");
        assert!(resolved.is_none());
    }

    #[test]
    fn returns_none_when_the_package_has_no_family_table() {
        let root = scratch_root("no-family");
        std::fs::create_dir_all(root.as_std_path()).unwrap();
        std::fs::write(
            root.join("trait.toml").as_std_path(),
            r#"[package]
id = "plain-demo"
version = "0.1.0"
name = "Plain Demo"
status = "ready"
"#,
        )
        .unwrap();
        let resolved = resolve_family_variant_at_package_root(&root, "plain-demo", "default")
            .expect("no error for a non-family package");
        assert!(resolved.is_none());
    }

    #[test]
    fn errors_when_a_declared_variant_names_a_missing_canonical_file() {
        let root = scratch_root("dangling");
        std::fs::create_dir_all(root.as_std_path()).unwrap();
        std::fs::write(
            root.join("trait.toml").as_std_path(),
            r#"[package]
id = "family-demo"
version = "0.1.0"
name = "Family Demo"
status = "ready"

[family]
default = "quick"

[family.variant.quick]
path = "generated/quick/index.toml"
"#,
        )
        .unwrap();
        let error = resolve_family_variant_at_package_root(&root, "family-demo", "quick")
            .expect_err("a declared-but-missing canonical file is a hard error");
        assert!(matches!(error, crate::Error::Usage { .. }));
    }

    #[test]
    fn resolves_a_declared_alias_to_its_canonical_file() {
        let root = scratch_root("alias-hit");
        std::fs::create_dir_all(root.join("generated/quick").as_std_path()).unwrap();
        std::fs::write(
            root.join("trait.toml").as_std_path(),
            r#"[package]
id = "family-demo"
version = "0.1.0"
name = "Family Demo"
status = "ready"

[family]
default = "quick"

[family.variant.quick]
path = "generated/quick/index.toml"
aliases = ["family-demo-quick"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("generated/quick/index.toml").as_std_path(),
            family_variant_trait_doc("family-demo", "quick"),
        )
        .unwrap();
        let resolved = resolve_family_alias_at_package_root(&root, "family-demo-quick")
            .expect("resolves without error")
            .expect("alias is declared");
        assert_eq!(resolved.0, root.join("generated/quick/index.toml"));
    }

    #[test]
    fn builtin_alias_candidates_matches_a_known_hyphen_prefix() {
        let candidates =
            builtin_alias_package_candidates("demo-family-quick", |id| id == "demo-family");
        assert_eq!(candidates, vec!["demo-family".to_string()]);
    }

    #[test]
    fn builtin_alias_candidates_is_empty_for_no_known_prefix() {
        let candidates = builtin_alias_package_candidates("demo-family-quick", |_id| false);
        assert!(candidates.is_empty());
    }

    #[test]
    fn builtin_alias_candidates_is_empty_when_the_alias_itself_is_a_known_id() {
        // A bare id equal to a known package id is resolved by ordinary
        // desugared-id resolution, not the alias path — it has no hyphen
        // split, so it must never appear as a derived candidate here.
        let candidates = builtin_alias_package_candidates("demo-family", |id| id == "demo-family");
        assert!(candidates.is_empty());
    }
}
