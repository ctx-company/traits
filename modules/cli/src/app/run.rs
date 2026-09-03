//! Run and session command handlers.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::app::entry::print_json_report;
use crate::app::run_format;
use crate::app::structured_output;
use crate::app::surface::cli;
use ctx_traits_core::response::{CapabilityReport, CommandOutput, Envelope};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct NotYourFrameReport<'a> {
    kind: &'static str,
    agent: &'a str,
    current_agent: Option<&'a str>,
    status: &'a ctx_traits_core::procedure::session::Status,
    session_id: &'a ctx_traits_core::procedure::session::SessionId,
    run_id: &'a ctx_traits_core::procedure::run::Id,
}

pub(crate) struct RunInputs<'a> {
    pub(crate) trait_id: Option<&'a str>,
    pub(crate) file: Option<&'a str>,
    pub(crate) input: Option<&'a str>,
    pub(crate) sets: &'a [String],
    pub(crate) session_store: Option<&'a str>,
    pub(crate) ephemeral: bool,
    pub(crate) strict_loops: bool,
    pub(crate) override_dependencies: bool,
    pub(crate) task_dispatch: bool,
    pub(crate) assignments: &'a [String],
    pub(crate) resource_root: Option<&'a str>,
    pub(crate) out: Option<&'a str>,
    pub(crate) worktree: Option<Option<&'a str>>,
    pub(crate) json: bool,
    pub(crate) trait_args: &'a [String],
    /// P460 resolved automatic-landing intent, already validated against an
    /// effective worktree by the caller. Threaded straight into the
    /// session's initial persisted provenance (never a post-start ledger
    /// mutation — P460 review) so it is durable before any concurrent
    /// `drive` could observe the ledger. `None` for `handle_run`'s
    /// `--no-drive` path, which rejects `--merge`/`--no-merge` earlier.
    pub(crate) merge_rung: Option<ctx_traits_core::procedure::session::MergeRung>,
    pub(crate) startup_observer: Option<ctx_traits_io::run::StartupObserver>,
}

pub(crate) struct SessionStartInputs<'a> {
    pub(crate) trait_id: Option<&'a str>,
    pub(crate) file: Option<&'a str>,
    /// Removed (P476): still threaded through so `handle_session_start` can
    /// reject it with a message naming `--assign default=...`. See
    /// `cli::SessionStartArgs::master`.
    pub(crate) master: Option<&'a str>,
    pub(crate) input: Option<&'a str>,
    pub(crate) sets: &'a [String],
    pub(crate) session_store: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) resource_root: Option<&'a str>,
    pub(crate) out: Option<&'a str>,
    pub(crate) max_frames: Option<u64>,
    pub(crate) frame_seconds: Option<u64>,
    pub(crate) total_seconds: Option<u64>,
    pub(crate) max_retries: Option<u64>,
    pub(crate) attach_wait_seconds: Option<u64>,
    pub(crate) idle_seconds: Option<u64>,
    pub(crate) max_in_flight: usize,
    /// P402 `--wait`: block for the per-session conductor lease within the
    /// total-time budget instead of returning the typed busy outcome
    /// immediately when another process already holds it.
    pub(crate) wait: bool,
    pub(crate) progress: cli::DriveProgress,
    pub(crate) worktree: Option<Option<&'a str>>,
    pub(crate) strict_loops: bool,
    pub(crate) override_dependencies: bool,
    pub(crate) task_dispatch: bool,
    pub(crate) json: bool,
    pub(crate) verbose: bool,
    pub(crate) trait_args: &'a [String],
    /// P460 resolved automatic-landing intent, already validated against an
    /// effective worktree by the caller. Persisted to the session's
    /// provenance before driving; `None` means this run never lands
    /// automatically.
    pub(crate) merge_rung: Option<ctx_traits_core::procedure::session::MergeRung>,
    /// P550 resolved `--story`/`[drive] story` level. `None` means the
    /// termination story hook is off. `Some` opens the pane on a fully
    /// interactive TTY (never under `--json`) and prints the plain story
    /// otherwise.
    pub(crate) story: Option<ctx_traits_core::procedure::story::StoryLevel>,
    pub(crate) startup: Option<crate::app::run_startup_view::StartupView>,
}

/// Bounded `ctx traits run --no-drive --json` projection (P421): pairs the
/// session with the receipt path so a cold agent can locate the ledger
/// without reconstructing session-store layout.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct RunSessionReport {
    session: ctx_traits_core::procedure::session::Session,
    session_path: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct SessionStartReport {
    session: ctx_traits_core::procedure::session::Session,
    session_path: Option<String>,
    drive: crate::app::drive::DriveReport,
    /// P460: present only when a merge intent was resolved for this run.
    /// Absent (and omitted from JSON) whenever `merge_rung` was `None`, so
    /// output without merge intent is byte-identical to before P460.
    #[serde(skip_serializing_if = "Option::is_none")]
    merge: Option<crate::app::merge::MergeReport>,
}

/// Bounded `ctx traits internal call --json` projection (P421): the fields an agent
/// operator loop needs to decide what happened and how to continue, without
/// re-embedding the full internal `Session` ledger `CallResponse` carries
/// for in-process callers. Full evidence stays on disk at `receipt_path`.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CallReport<'a> {
    schema_version: &'a str,
    session_id: &'a ctx_traits_core::procedure::session::SessionId,
    run_id: &'a ctx_traits_core::procedure::run::Id,
    status: &'a ctx_traits_core::procedure::session::Status,
    response_kind: &'a ctx_traits_core::procedure::session::CallResponseKind,
    #[serde(skip_serializing_if = "slice_is_empty")]
    accepted_slot_values: &'a [ctx_traits_core::procedure::runtime::Value],
    #[serde(skip_serializing_if = "slice_is_empty")]
    rejected_slot_values: &'a [ctx_traits_core::procedure::runtime::RejectedAttempt],
    #[serde(skip_serializing_if = "slice_is_empty")]
    accepted_signals: &'a [ctx_traits_core::procedure::runtime::SignalEmission],
    #[serde(skip_serializing_if = "slice_is_empty")]
    rejected_signals: &'a [ctx_traits_core::procedure::runtime::SignalEmission],
    #[serde(skip_serializing_if = "slice_is_empty")]
    schema_validation: &'a [ctx_traits_core::procedure::runtime::SchemaValidation],
    #[serde(skip_serializing_if = "slice_is_empty")]
    unexpected_outputs: &'a [String],
    #[serde(skip_serializing_if = "slice_is_empty")]
    missing_required_outputs: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    correction: Option<&'a str>,
    updated_session_digest: &'a ctx_traits_core::digest::Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_frame: Option<&'a ctx_traits_core::procedure::runtime::SequenceFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion: Option<&'a ctx_traits_core::procedure::session::CompletionNotification>,
    receipt_path: &'a str,
}

fn slice_is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

impl<'a> CallReport<'a> {
    fn new(
        response: &'a ctx_traits_core::procedure::session::CallResponse,
        receipt_path: &'a str,
    ) -> Self {
        CallReport {
            schema_version: &response.schema_version,
            session_id: &response.session_id,
            run_id: &response.run_id,
            status: &response.status,
            response_kind: &response.response_kind,
            accepted_slot_values: &response.accepted_slot_values,
            rejected_slot_values: &response.rejected_slot_values,
            accepted_signals: &response.accepted_signals,
            rejected_signals: &response.rejected_signals,
            schema_validation: &response.schema_validation,
            unexpected_outputs: &response.unexpected_outputs,
            missing_required_outputs: &response.missing_required_outputs,
            correction: response.correction.as_deref(),
            updated_session_digest: &response.updated_session_digest,
            next_frame: response.next_frame.as_deref(),
            completion: response.completion.as_ref(),
            receipt_path,
        }
    }
}

pub(crate) struct RunInfoInputs<'a> {
    pub(crate) trait_id: Option<&'a str>,
    pub(crate) file: Option<&'a str>,
    pub(crate) query: &'a [String],
    pub(crate) json: bool,
}

pub(crate) struct CallInputs<'a> {
    pub(crate) file: Option<&'a str>,
    pub(crate) session: &'a str,
    pub(crate) session_store: Option<&'a str>,
    pub(crate) data: &'a str,
    pub(crate) out: Option<&'a str>,
    pub(crate) agent: Option<&'a str>,
    pub(crate) json: bool,
}

pub(crate) struct SetInputs<'a> {
    pub(crate) file: Option<&'a str>,
    pub(crate) session: &'a str,
    pub(crate) session_store: Option<&'a str>,
    pub(crate) target: &'a str,
    pub(crate) value: &'a str,
    pub(crate) value_json: bool,
    pub(crate) agent: Option<&'a str>,
    pub(crate) json: bool,
}

/// Run-dispatch acceptance gate (0178 deliverable 2): a repo carrying a
/// committed `runtime.example.ts` that has never been accepted, or whose
/// example has changed since the last acceptance, refuses run dispatch and
/// names `ctx traits internal config accept`. Read-only commands (`check`, `doctor`,
/// `config build`, `config accept` itself) never call this. Non-TTY
/// contexts always refuse — acceptance is never automatic.
///
/// Scoped to the `.ts` example only, NOT the pre-existing `runtime.example.
/// toml` (0037): that convention predates this gate and every repo/fixture
/// that already relies on it (this repo included) would trip an unaccepted-
/// example refusal on every run with no migration path. `ctx traits internal config
/// accept` itself still accepts either format — this narrows only which
/// example blocks *dispatch*.
fn guard_runtime_acceptance() -> crate::Result<()> {
    let cwd = camino::Utf8PathBuf::from_path_buf(std::env::current_dir().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: ".".to_string(),
            source,
        })
    })?)
    .map_err(|_| crate::Error::Command {
        message: "current directory is not valid UTF-8".to_string(),
    })?;
    // `stable_repo_root` probes from its argument's *parent* — join a
    // synthetic leaf so the probe starts at `cwd` itself.
    let repo_root = match crate::app::cdk_build::stable_repo_root(&cwd.join(".ctx-accept-probe")) {
        Ok(root) => root,
        Err(_) => return Ok(()),
    };
    let (example_ts, _example_toml, _source) = crate::app::config_accept::repo_paths(&repo_root);
    if !example_ts.exists() {
        return Ok(());
    }
    let repo_key =
        ctx_traits_io::state::repo_key(&ctx_traits_io::state::canonical_repo_root(&repo_root)?);
    let acceptance = ctx_traits_io::runtime_acceptance::check_acceptance(&example_ts, &repo_key)?;
    if !matches!(
        acceptance,
        ctx_traits_io::runtime_acceptance::Acceptance::NeedsAcceptance { .. }
    ) {
        return Ok(());
    }

    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !interactive {
        return Err(crate::Error::Command {
            message: "this repo's runtime.example.ts/.toml has not been accepted — run `ctx traits internal config accept`".to_string(),
        });
    }

    let example_path = example_ts;
    let content = ctx_traits_io::read::read_text(&example_path)?;
    println!("{content}");
    print!("accept {example_path} as this repo's runtime configuration? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: "stdin".to_string(),
            source,
        })
    })?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        return Err(crate::Error::Command {
            message: format!(
                "declined — run `ctx traits internal config accept` when ready to accept {example_path}"
            ),
        });
    }

    let machine_path = if example_path.extension() == Some("ts") {
        example_path
            .parent()
            .map(|p| p.join("runtime.ts"))
            .unwrap_or_else(|| example_path.clone())
    } else {
        example_path
            .parent()
            .map(|p| p.join("runtime.toml"))
            .unwrap_or_else(|| example_path.clone())
    };
    ctx_traits_io::runtime_acceptance::accept(&example_path, &machine_path, &repo_key)?;
    if machine_path.extension() == Some("ts") {
        crate::app::config_build::handle_config_build(Some(machine_path.as_str()), true)?;
    }
    Ok(())
}

pub(crate) fn handle_run(input: RunInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let json = split_trailing_json_flag(input.trait_args, input.json).1;
    guard_runtime_acceptance()?;
    let outcome = start_run_session(input, false)?;

    if json {
        print_json_report(
            &run_envelope(
                RunSessionReport {
                    session: outcome.session.clone(),
                    session_path: outcome.session_path.as_ref().map(|path| path.to_string()),
                },
                outcome.session_path.is_some(),
                false,
                outcome.resource_supported,
            ),
            "run session",
        )?;
    } else {
        run_format::print_run_session(
            &outcome.session,
            outcome.session_path.as_ref().map(|path| path.as_str()),
        );
    }
    if outcome.session.status == ctx_traits_core::procedure::session::Status::Failed {
        return Err(crate::Error::Command {
            message: "run session failed".to_string(),
        });
    }
    Ok(CommandOutput::new(()))
}

fn start_run_session(
    input: RunInputs<'_>,
    defer_commands: bool,
) -> crate::Result<ctx_traits_io::run::StartOutcome> {
    let (trait_args, json) = split_trailing_json_flag(input.trait_args, input.json);
    let query = if input.file.is_none() && input.trait_id.is_none() && !trait_args.is_empty() {
        let query = trait_args.join(" ");
        // The inline startup pane must not inspect untrusted inventory itself:
        // `run::start` owns selection, warning capture, and authorization as one
        // operation so no candidate detail can reach the terminal beforehand.
        if input.startup_observer.is_some() {
            Some(query)
        } else {
            let pre_authorization = input.startup_observer.is_some();
            let report_pre_authorization_failure = |detail: &str| {
                if let Some(observer) = &input.startup_observer {
                    observer(ctx_traits_io::run::StartupUpdate {
                        stage: ctx_traits_io::run::StartupStage::Initialization,
                        state: ctx_traits_io::run::StartupStageState::Failed,
                        detail: detail.to_string(),
                    });
                }
            };
            let context =
                ctx_traits_io::inventory::InventoryContext::discover().inspect_err(|_| {
                    report_pre_authorization_failure(
                        "could not inspect trait inventory before authorization",
                    );
                })?;
            let selection =
                ctx_traits_io::run_query::select(&query, &context).inspect_err(|_| {
                    report_pre_authorization_failure(
                        "could not select a trait before authorization",
                    );
                })?;
            if selection.status != ctx_traits_core::run_info::RunInfoSelectionStatus::Selected {
                if json {
                    print_json_report(&selection.selection, "query run selection")?;
                } else if !pre_authorization {
                    run_format::print_run_selection("ctx traits run", &selection.selection);
                }
                if pre_authorization {
                    report_pre_authorization_failure("query did not select an authorized trait");
                }
                let gate_detail =
                    ctx_traits_core::run_info::selection_refusal_detail(&selection.selection);
                return Err(crate::Error::Command {
                    message: format!(
                        "query run did not select exactly one runnable trait ({}){}",
                        crate::app::presentation::wire_name(&selection.status),
                        gate_detail
                    ),
                });
            }
            Some(query)
        }
    } else {
        None
    };
    let mut initial_values = match input.input {
        Some(input_path) => {
            let initial_text = ctx_traits_io::read::read_text(camino::Utf8Path::new(input_path))?;
            let initial_json: serde_json::Value =
                serde_json::from_str(&initial_text).map_err(|e| {
                    crate::Error::json(format!("parse runtime input JSON {input_path}"), e)
                })?;
            ctx_traits_core::procedure::session::run_initial_values_from_json(initial_json)?
        }
        None => Vec::new(),
    };
    initial_values.extend(ctx_traits_io::run::parse_initial_sets(input.sets)?);
    Ok(ctx_traits_io::run::start(
        ctx_traits_io::run::StartRequest {
            trait_file: input.file,
            trait_id: input.trait_id,
            query: query.as_deref(),
            trait_args: &trait_args,
            input_values: initial_values,
            out: input.out,
            session_store: input.session_store,
            ephemeral: input.ephemeral,
            resource_evidence: ctx_traits_io::run::ResourceEvidenceMode::ReadDeclared {
                root_override: input.resource_root,
            },
            assign_overrides: input.assignments,
            agent_assignments: None,
            provider_capability_reports: Vec::new(),
            provider_warnings: Vec::new(),
            harness_probes: Vec::new(),
            caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
            // Both the driven and --no-drive paths come through here; a
            // machine reader (--json) gets silence, a human gets the init
            // phases named while they run.
            narrate_progress: !json && input.startup_observer.is_none(),
            startup_observer: input.startup_observer,
            state_source: "ctx traits run",
            trait_arg_evidence: "ctx traits run trait args",
            worktree: input.worktree,
            defer_commands,
            strict_loops: input.strict_loops,
            override_dependencies: input.override_dependencies,
            task_dispatch: input.task_dispatch,
            merge_rung: input.merge_rung,
        },
    )?)
}

pub(crate) fn handle_session_start(
    input: SessionStartInputs<'_>,
) -> crate::Result<CommandOutput<()>> {
    drive_session(input)?.into_command_output()
}

/// The full driven-session body `handle_session_start` runs, extracted so
/// 0195's `--task` queue orchestrator can drive one queued task exactly the
/// same way (same preflight, worktree, drive, and merge/close path) while
/// also getting the resulting [`CompletionOutcome`] back — `into_command_output`
/// consumes it into the plain single-run exit mapping, which the queue
/// needs to bypass to classify per-task outcomes and keep the queue going.
fn drive_session(input: SessionStartInputs<'_>) -> crate::Result<CompletionOutcome> {
    guard_runtime_acceptance()?;
    if input.master.is_some() {
        if let Some(view) = input.startup.as_ref() {
            view.fail("--master was removed; use --assign default=<harness> instead");
        }
        return Err(crate::Error::Command {
            message: "--master was removed; use --assign default=<harness>[:transport[:session-mode[:model[:reasoning-effort]]]] instead"
                .to_string(),
        });
    }
    let json = split_trailing_json_flag(input.trait_args, input.json).1;
    let assignment_overrides = input.assignments.to_vec();
    let mut startup = input.startup;
    let startup_observer = startup.as_ref().map(|view| view.observer());
    let mut outcome = match start_run_session(
        RunInputs {
            trait_id: input.trait_id,
            file: input.file,
            input: input.input,
            sets: input.sets,
            session_store: input.session_store,
            ephemeral: false,
            strict_loops: input.strict_loops,
            override_dependencies: input.override_dependencies,
            task_dispatch: input.task_dispatch,
            assignments: &assignment_overrides,
            resource_root: input.resource_root,
            out: input.out,
            worktree: input.worktree,
            json,
            trait_args: input.trait_args,
            // P460 (review): threaded into the session's initial persisted
            // provenance by `run::start` itself, so a credits-paused-then-
            // resumed drive lands with this same rung with no window where
            // a globally discoverable ledger carries no intent yet.
            merge_rung: input.merge_rung,
            startup_observer: startup_observer.clone(),
        },
        // Defer leading command frames to the drive loop so the TUI paints
        // the command step as running instead of freezing pre-drive.
        true,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            // `start` owns every startup-stage failure notification. In
            // particular, do not replace its fixed pre-authorization detail
            // with an error that may contain untrusted trait text.
            return Err(error);
        }
    };
    let mut session_path = outcome.session_path.as_ref().map(|path| path.to_string());
    let mut session_arg = session_path
        .clone()
        .unwrap_or_else(|| outcome.session.session_id.as_str().to_string());
    // `run::start` already prepared the worktree (if requested); pass its
    // execution directory straight through instead of re-resolving it.
    // Retained TUI attempts return their pane through this handoff on every
    // exit. The final completion path takes and closes it before reporting.
    let panel_handoff = crate::app::drive::PanelHandoff::new();
    // Only a retained TUI failure uses the compact modal line for its final
    // panel. Headless and piped output retain their established full detail.
    let mut retained_failure_line = None;
    macro_rules! drive_once {
        () => {
            crate::app::drive::drive(crate::app::drive::DriveInputs {
                file: input.file,
                session: &session_arg,
                session_store: input.session_store,
                assignments: &assignment_overrides,
                max_frames: input.max_frames,
                frame_seconds: input.frame_seconds,
                total_seconds: input.total_seconds,
                max_retries: input.max_retries,
                attach_wait_seconds: input.attach_wait_seconds,
                idle_seconds: input.idle_seconds,
                max_in_flight: input.max_in_flight,
                wait: input.wait,
                progress: input.progress,
                worktree: None,
                execution_dir: outcome.execution_dir.as_deref(),
                clear_merge_intent: false,
                retain_panel_on_failure: true,
                panel_handoff: Some(panel_handoff.clone()),
                startup: startup.take(),
                frame_observer: None,
            })
        };
    }
    // Restart is shared by both failed reports and propagated drive errors so
    // their fresh-session boundary cannot diverge.
    macro_rules! restart_session {
        () => {
            start_run_session(
                RunInputs {
                    trait_id: input.trait_id,
                    file: input.file,
                    input: input.input,
                    sets: input.sets,
                    session_store: input.session_store,
                    ephemeral: false,
                    strict_loops: input.strict_loops,
                    override_dependencies: input.override_dependencies,
                    task_dispatch: input.task_dispatch,
                    assignments: &assignment_overrides,
                    resource_root: input.resource_root,
                    out: restart_out(input.out),
                    worktree: restart_worktree(input.worktree),
                    json,
                    trait_args: input.trait_args,
                    merge_rung: input.merge_rung,
                    startup_observer: startup_observer.clone(),
                },
                true,
            )
        };
    }

    let (drive, final_session, drive_outcome) = loop {
        let drive = match drive_once!() {
            Ok(drive) => drive,
            Err(error) => {
                let Some(panel) = panel_handoff.take() else {
                    return Err(error);
                };
                let line = short_failure_line(Some(&error.to_string()));
                let abort_trait_id = outcome.session.trait_id.clone();
                let abort_session_id = outcome.session.session_id.as_str().to_string();
                panel.open_failure_modal(&line);
                resolve_propagated_drive_error_choice(
                    panel.wait_for_failure_choice(),
                    panel,
                    &panel_handoff,
                    &mut outcome,
                    || restart_session!(),
                    |panel| {
                        propagated_drive_error_abort(
                            panel,
                            PropagatedDriveError {
                                trait_id: &abort_trait_id,
                                session_id: &abort_session_id,
                                line: &line,
                            },
                            |panel| {
                                use crate::app::presentation::{HumanOutputMode, emit_human};
                                emit_human(false, panel, HumanOutputMode::Compact, || Ok(()))
                            },
                        )
                    },
                )?;
                session_path = outcome.session_path.as_ref().map(|path| path.to_string());
                session_arg = session_path
                    .clone()
                    .unwrap_or_else(|| outcome.session.session_id.as_str().to_string());
                continue;
            }
        };
        // `status` rebuilds the session projection and deliberately drops this
        // terminal marker. Retain the just-persisted typed record privately so
        // queue classification can distinguish a disk-full park afterward.
        let drive_outcome =
            ctx_traits_io::run_session::read_run_session(camino::Utf8Path::new(&session_arg))
                .ok()
                .and_then(|session| session.last_drive_outcome);
        let final_session = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
            trait_file: input.file,
            trait_id: None,
            session: &session_arg,
            session_store: input.session_store,
            elapsed_seconds: None,
        })
        .map(|inspected| inspected.session)
        .unwrap_or_else(|_| outcome.session.clone());
        if !human_terminal_failure(!json, &final_session, &drive) {
            break (drive, final_session, drive_outcome);
        }
        let Some(panel) = panel_handoff.take() else {
            break (drive, final_session, drive_outcome);
        };
        let line = short_failure_line(failure_reason(&final_session, &drive).as_deref());
        panel.open_failure_modal(&line);
        match panel.wait_for_failure_choice() {
            crate::app::run_view::FailureChoice::Resume => {
                panel.clear_failure_modal();
                panel_handoff.give(panel);
            }
            crate::app::run_view::FailureChoice::Restart => {
                restart_propagated_attempt(panel, &mut outcome, || restart_session!())?;
                session_path = outcome.session_path.as_ref().map(|path| path.to_string());
                session_arg = session_path
                    .clone()
                    .unwrap_or_else(|| outcome.session.session_id.as_str().to_string());
            }
            crate::app::run_view::FailureChoice::Abort => {
                retained_failure_line = Some(line);
                panel_handoff.give(panel);
                break (drive, final_session, drive_outcome);
            }
        }
    };

    let (merge_live, merger_stdout_observer, merge_span_guard) = merge_live_for_completion(
        panel_handoff.take(),
        final_session.run_id.as_str(),
        final_session.session_id.as_str(),
        &assignment_overrides,
    );
    // outcome.session is the pre-drive snapshot; re-inspect for the completed
    // state so both the JSON envelope and the plain-text final output reflect
    // what actually landed, not the pre-drive placeholder.
    // Load presentation inputs BEFORE a successful merge removes the
    // worktree the trait file may live under.
    let loaded_trait = if json {
        None
    } else {
        Some(ctx_traits_io::run::load_trait_for_session(
            input.file,
            None,
            &final_session,
            "final output rendering",
        )?)
    };
    // `handle_session_start` always drives a non-ephemeral session, so
    // `start()` always resolved a ledger path (P460 review — this is the
    // exact path `complete_after_drive` must read/write, not a re-scan of
    // `session_store` by run-id).
    let merge_session_path = outcome
        .session_path
        .clone()
        .ok_or_else(|| crate::Error::Command {
            message: "internal error: driven session start has no resolved session path"
                .to_string(),
        })?;
    let completion = complete_after_drive(
        input.session_store,
        &merge_session_path,
        &assignment_overrides,
        final_session,
        merge_live,
        merger_stdout_observer,
    )?;
    let completion = completion
        .with_drive_outcome(drive_outcome)
        .with_drive_report(&drive)
        .with_human_terminal_failure(!json, &drive);
    // Close (or no-op, if this run never got a panel) the merge span's live
    // surface BEFORE any of the plain-text reporting below, matching
    // `drive_loop`'s own guard-drops-before-caller-prints ordering.
    drop(merge_span_guard);
    if json {
        print_json_report(
            &run_envelope(
                SessionStartReport {
                    session: completion.session.clone(),
                    session_path,
                    drive,
                    merge: completion.merge.clone(),
                },
                true,
                true,
                outcome.resource_supported,
            ),
            "session start",
        )?;
    } else {
        if input.verbose {
            run_format::print_run_session(
                &completion.session,
                outcome.session_path.as_ref().map(|path| path.as_str()),
            );
            crate::app::drive::print_report(&drive, Some(&completion.session))?;
        }
        print_final_output(
            &completion.session,
            &drive,
            &merge_session_path,
            loaded_trait.as_ref().map(|loaded| &loaded.trait_ref),
            retained_failure_line
                .as_deref()
                .or(completion.failure_reason.as_deref()),
            input.verbose,
        )?;
        if input.verbose
            && let Some(report) = &completion.merge
        {
            crate::app::merge::print_report(
                report,
                crate::app::presentation::HumanOutputMode::Verbose,
            )?;
        }
        // P550: the story pane opens AFTER the merge report above, so the
        // story it renders covers the landing — and after every disposition
        // (not only success), since the pane must render a parked/blocked/
        // failed/cancelled run exactly as honestly as a completed one.
        if let Some(level) = input.story {
            print_story_at_termination(
                &completion.session,
                &merge_session_path,
                level,
                input.verbose,
            )?;
        }
    }
    Ok(completion)
}

/// One `--task` queue member's terminal outcome (0195) — the row a
/// per-task outcome panel renders once the queue finishes or halts.
#[derive(Debug, Clone)]
pub(crate) enum TaskQueueOutcome {
    Landed {
        closed: bool,
    },
    Completed,
    NotMerged,
    Parked,
    MergeFailed,
    /// The drive parked before dispatch because available disk space was below
    /// the configured floor. This halts the queue, but is not a failure.
    DiskFull {
        park: Option<ctx_traits_core::procedure::session::DiskFullPark>,
    },
    /// The run reached a terminal state that is not `Completed` — rejected on
    /// a step, blocked, still waiting on an agent or a human, cancelled. The
    /// session status and the recorded drive outcome are carried verbatim so
    /// the row names what actually happened.
    NotCompleted {
        status: String,
        outcome: Option<String>,
    },
    Failed {
        message: String,
    },
}

impl TaskQueueOutcome {
    /// Row tone is driven by the failure classification, not queue control
    /// flow: a disk-full park halts but remains a warning.
    fn tone(&self) -> crate::app::presentation::RowTone {
        if self.fails() {
            crate::app::presentation::RowTone::Fail
        } else if matches!(self, TaskQueueOutcome::DiskFull { .. }) {
            crate::app::presentation::RowTone::Warn
        } else {
            crate::app::presentation::RowTone::Pass
        }
    }

    /// The failure family: the authority for failure-toned rows and failure
    /// panel closing text.
    fn fails(&self) -> bool {
        matches!(
            self,
            TaskQueueOutcome::Parked
                | TaskQueueOutcome::MergeFailed
                | TaskQueueOutcome::NotCompleted { .. }
                | TaskQueueOutcome::Failed { .. }
        )
    }

    /// A failed run, parked/failed merge, or disk-full park halts the queue
    /// by default; `--continue-on-failure` is the only override.
    fn halts(&self) -> bool {
        self.fails() || matches!(self, TaskQueueOutcome::DiskFull { .. })
    }

    fn label(&self) -> String {
        match self {
            TaskQueueOutcome::Landed { closed: true } => "landed, closed".to_string(),
            TaskQueueOutcome::Landed { closed: false } => "landed".to_string(),
            TaskQueueOutcome::Completed => "completed (no merge intent)".to_string(),
            TaskQueueOutcome::NotMerged => "committed, not merged".to_string(),
            TaskQueueOutcome::Parked => "parked".to_string(),
            TaskQueueOutcome::MergeFailed => "merge failed".to_string(),
            TaskQueueOutcome::DiskFull { park: Some(park) } => format!(
                "parked: disk-full (floor {} MiB, {} bytes free at {})",
                park.floor_mb, park.available_bytes, park.probed_path
            ),
            TaskQueueOutcome::DiskFull { park: None } => "parked: disk-full".to_string(),
            TaskQueueOutcome::NotCompleted { status, outcome } => match outcome {
                Some(outcome) => format!("not completed: {status} ({outcome})"),
                None => format!("not completed: {status}"),
            },
            TaskQueueOutcome::Failed { message } => format!("failed: {message}"),
        }
    }
}

/// `Some(NotCompleted)` unless the run actually reached `Status::Completed`.
///
/// The queue used to test only `Status::Failed` and route everything else
/// through [`landing_state`](ctx_traits_core::procedure::session::landing_state),
/// which returns `None` whenever there are no terminal merge frames — exactly
/// what a run that never got far enough to merge looks like. So `rejected`,
/// `blocked`, `awaiting-agent-output`, and every other non-terminal status
/// reported as `completed (no merge intent)`, did not halt the queue, and left
/// the whole command exiting 0. Observed 2026-08-19: five failed 0006.9 runs
/// in ctx-notify and two failed 0006.5 runs in ctx-codecheck, every one of
/// them carrying `merge-intent: deep` while the row claimed there was none.
///
/// `Status::Failed` keeps its own richer row upstream of this check; this is
/// for the other seven variants.
fn queue_not_completed(
    session: &ctx_traits_core::procedure::session::Session,
    drive_outcome: Option<&ctx_traits_core::procedure::session::DriveOutcome>,
) -> Option<TaskQueueOutcome> {
    if session.status == ctx_traits_core::procedure::session::Status::Completed {
        return None;
    }
    if let Some(recorded) = drive_outcome
        && recorded.outcome == ctx_traits_core::procedure::session::DriveOutcomeKind::DiskFull
    {
        return Some(TaskQueueOutcome::DiskFull {
            park: recorded.disk_full.clone(),
        });
    }
    Some(TaskQueueOutcome::NotCompleted {
        status: super::run_view::session_text::session_status(&session.status).to_string(),
        outcome: session
            .last_drive_outcome
            .as_ref()
            .map(|recorded| recorded.outcome.as_str().to_string()),
    })
}

pub(crate) struct TaskQueueInputs<'a> {
    pub(crate) queue: Vec<String>,
    pub(crate) continue_on_failure: bool,
    pub(crate) dispatch_trait: String,
    pub(crate) session_store: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) resource_root: Option<&'a str>,
    pub(crate) out: Option<&'a str>,
    pub(crate) max_frames: Option<u64>,
    pub(crate) frame_seconds: Option<u64>,
    pub(crate) total_seconds: Option<u64>,
    pub(crate) max_retries: Option<u64>,
    pub(crate) attach_wait_seconds: Option<u64>,
    pub(crate) idle_seconds: Option<u64>,
    pub(crate) max_in_flight: usize,
    pub(crate) wait: bool,
    pub(crate) progress: cli::DriveProgress,
    pub(crate) worktree: Option<Option<&'a str>>,
    pub(crate) strict_loops: bool,
    pub(crate) override_dependencies: bool,
    pub(crate) json: bool,
    pub(crate) verbose: bool,
    pub(crate) merge_rung: Option<ctx_traits_core::procedure::session::MergeRung>,
    pub(crate) story: Option<ctx_traits_core::procedure::story::StoryLevel>,
    pub(crate) repo_root: camino::Utf8PathBuf,
    pub(crate) board_dir: camino::Utf8PathBuf,
    pub(crate) startup: Option<crate::app::run_startup_view::StartupView>,
}

/// `ctx traits run --task ...` (0195): drive a board-resolved queue of
/// tasks sequentially through [`drive_session`] — the same preflight,
/// worktree, and merge path a single `--task-dispatch` run takes, so the
/// per-task ready/wall/dependency refusal always happens before any model
/// call, exactly as it does for one run. Halts on a failed run or a parked/
/// failed merge unless `continue_on_failure` was requested, in which case
/// the queue runs to completion and every task's outcome is reported. A
/// landed run is closed through the 0144 auto-close primitives
/// ([`super::task_queue::auto_close_landed_task`]) — never a parallel close
/// implementation.
pub(crate) fn handle_task_queue_run(
    input: TaskQueueInputs<'_>,
) -> crate::Result<CommandOutput<()>> {
    let mut startup = input.startup;
    let (outcomes, halted) = drive_task_queue(&input.queue, input.continue_on_failure, |key| {
        let sets = vec![format!("task={key}")];
        let session_inputs = SessionStartInputs {
            trait_id: Some(input.dispatch_trait.as_str()),
            file: None,
            master: None,
            input: None,
            sets: &sets,
            session_store: input.session_store,
            assignments: input.assignments,
            resource_root: input.resource_root,
            out: input.out,
            max_frames: input.max_frames,
            frame_seconds: input.frame_seconds,
            total_seconds: input.total_seconds,
            max_retries: input.max_retries,
            attach_wait_seconds: input.attach_wait_seconds,
            idle_seconds: input.idle_seconds,
            max_in_flight: input.max_in_flight,
            wait: input.wait,
            progress: input.progress,
            worktree: input.worktree,
            strict_loops: input.strict_loops,
            override_dependencies: input.override_dependencies,
            task_dispatch: true,
            json: input.json,
            verbose: input.verbose,
            trait_args: &[],
            merge_rung: input.merge_rung,
            story: input.story,
            startup: startup.take(),
        };
        match drive_session(session_inputs) {
            Ok(completion) => {
                if completion.session.status == ctx_traits_core::procedure::session::Status::Failed
                {
                    TaskQueueOutcome::Failed {
                        message: "run completed with status failed".to_string(),
                    }
                } else if let Some(not_completed) =
                    queue_not_completed(&completion.session, completion.drive_outcome.as_ref())
                {
                    not_completed
                } else {
                    use ctx_traits_core::procedure::session::LandingState;
                    match ctx_traits_core::procedure::session::landing_state(&completion.session) {
                        Some(LandingState::Landed { revision }) => {
                            let closed = super::task_queue::auto_close_landed_task(
                                &input.board_dir,
                                key,
                                &input.repo_root,
                                revision.as_deref(),
                                completion.session.run_id.as_str(),
                            );
                            TaskQueueOutcome::Landed { closed }
                        }
                        Some(LandingState::Parked) => TaskQueueOutcome::Parked,
                        Some(LandingState::MergeFailed) => TaskQueueOutcome::MergeFailed,
                        Some(LandingState::NotMerged) => TaskQueueOutcome::NotMerged,
                        None => TaskQueueOutcome::Completed,
                    }
                }
            }
            Err(error) => {
                eprintln!("ctx run --task {key}: {error}");
                TaskQueueOutcome::Failed {
                    message: error.to_string(),
                }
            }
        }
    });

    // An empty queue (every member closed before expansion) never invokes
    // the closure above, so `startup.take()` never fires — drop it here
    // unconditionally, before any report output, so a live pane can never
    // survive to compete with the task-queue panel's rows.
    drop(startup);

    if !input.json {
        use crate::app::presentation::{HumanOutputMode, emit_human};
        emit_human(
            false,
            &task_queue_panel(&outcomes, halted, input.queue.len()),
            HumanOutputMode::Compact,
            || Ok(()),
        )?;
    }

    let any_halting = outcomes.iter().any(|(_, outcome)| outcome.halts());
    let any_failed = outcomes.iter().any(|(_, outcome)| outcome.fails());
    if any_halting {
        return Err(crate::Error::AlreadyReported {
            message: if any_failed && halted {
                "task queue halted".to_string()
            } else if any_failed {
                "task queue completed with failures".to_string()
            } else {
                "task queue parked: disk-full".to_string()
            },
            exit_code: crate::app::error::EXIT_RUN_FAILED,
        });
    }
    Ok(CommandOutput::new(()))
}

/// The queue's own control flow (0195 Watch item: halt on a failed run or
/// a parked/failed merge before spending a model call on the next task,
/// unless `continue_on_failure`), factored out of [`handle_task_queue_run`]
/// so it is provable against synthetic per-task outcomes rather than only
/// through a real driven session — `produce_outcome` stands in for
/// [`drive_session`] in tests.
fn drive_task_queue(
    queue: &[String],
    continue_on_failure: bool,
    mut produce_outcome: impl FnMut(&str) -> TaskQueueOutcome,
) -> (Vec<(String, TaskQueueOutcome)>, bool) {
    let mut outcomes: Vec<(String, TaskQueueOutcome)> = Vec::new();
    let mut halted = false;
    for key in queue {
        let outcome = produce_outcome(key);
        let should_halt = outcome.halts() && !continue_on_failure;
        outcomes.push((key.clone(), outcome));
        if should_halt {
            halted = true;
            break;
        }
    }
    (outcomes, halted)
}

#[cfg(test)]
mod task_queue_drive_tests {
    use super::*;

    #[test]
    fn halts_on_first_parked_merge_and_skips_remaining_tasks() {
        let queue = vec![
            "0001.1".to_string(),
            "0001.2".to_string(),
            "0001.3".to_string(),
        ];
        let (outcomes, halted) = drive_task_queue(&queue, false, |key| {
            if key == "0001.2" {
                TaskQueueOutcome::Parked
            } else {
                TaskQueueOutcome::Landed { closed: true }
            }
        });
        assert!(halted);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].0, "0001.1");
        assert_eq!(outcomes[1].0, "0001.2");
        assert!(matches!(outcomes[1].1, TaskQueueOutcome::Parked));
    }

    #[test]
    fn continue_on_failure_runs_remaining_queue_and_reports_every_outcome() {
        let queue = vec![
            "0001.1".to_string(),
            "0001.2".to_string(),
            "0001.3".to_string(),
        ];
        let (outcomes, halted) = drive_task_queue(&queue, true, |key| {
            if key == "0001.2" {
                TaskQueueOutcome::Failed {
                    message: "boom".to_string(),
                }
            } else {
                TaskQueueOutcome::Landed { closed: true }
            }
        });
        assert!(!halted);
        assert_eq!(outcomes.len(), 3);
        assert_eq!(
            outcomes
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["0001.1", "0001.2", "0001.3"]
        );
        assert!(matches!(outcomes[1].1, TaskQueueOutcome::Failed { .. }));
        assert!(matches!(
            outcomes[2].1,
            TaskQueueOutcome::Landed { closed: true }
        ));
    }

    #[test]
    fn non_halting_outcomes_never_stop_the_queue() {
        let queue = vec!["0001.1".to_string(), "0001.2".to_string()];
        let (outcomes, halted) =
            drive_task_queue(&queue, false, |_key| TaskQueueOutcome::NotMerged);
        assert!(!halted);
        assert_eq!(outcomes.len(), 2);
    }

    /// The whole point of the row: a run that never completed must halt the
    /// queue and say so, instead of spending the next task's model budget
    /// behind a `completed (no merge intent)` label.
    #[test]
    fn a_run_that_never_completed_halts_the_queue_and_names_its_state() {
        let queue = vec!["0006.9".to_string(), "0007.1".to_string()];
        let (outcomes, halted) =
            drive_task_queue(&queue, false, |_key| TaskQueueOutcome::NotCompleted {
                status: "rejected".to_string(),
                outcome: Some("command-step-failed".to_string()),
            });
        assert!(halted, "a not-completed run halts by default");
        assert_eq!(outcomes.len(), 1, "the second task must not have been run");
        assert_eq!(
            outcomes[0].1.label(),
            "not completed: rejected (command-step-failed)"
        );
    }

    #[test]
    fn a_not_completed_row_without_a_recorded_outcome_still_names_the_status() {
        assert_eq!(
            TaskQueueOutcome::NotCompleted {
                status: "blocked".to_string(),
                outcome: None,
            }
            .label(),
            "not completed: blocked"
        );
    }

    #[test]
    fn queue_uses_current_typed_drive_record_after_session_refresh() {
        use ctx_traits_core::procedure::session::{DriveOutcome, Session};

        // This is the status projection returned after `refresh_run_session`:
        // it deliberately has no terminal marker of its own.
        let session: Session = serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "awaiting-agent-output",
            "provenance": {
                "started-by": {"surface": "test", "caller": "queue-fixture"},
                "state-source": "test",
                "started-at-epoch": 1000,
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "running",
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session deserializes");
        assert!(session.last_drive_outcome.is_none());

        let disk_full: DriveOutcome = serde_json::from_value(serde_json::json!({
            "outcome": "disk-full",
            "recorded-at-epoch": 1000,
            "disk-full": {
                "floor-mb": 512,
                "available-bytes": 42,
                "probed-path": "/worktree",
            },
        }))
        .expect("disk-full record deserializes");
        assert!(matches!(
            queue_not_completed(&session, Some(&disk_full)),
            Some(TaskQueueOutcome::DiskFull { park: Some(park) })
                if park.floor_mb == 512
                    && park.available_bytes == 42
                    && park.probed_path == "/worktree"
        ));

        let budget: DriveOutcome = serde_json::from_value(serde_json::json!({
            "outcome": "paused-budget-exhausted",
            "recorded-at-epoch": 1000,
        }))
        .expect("budget record deserializes");
        assert!(matches!(
            queue_not_completed(&session, Some(&budget)),
            Some(TaskQueueOutcome::NotCompleted { outcome: None, .. })
        ));
    }

    #[test]
    fn failure_tones_and_halts_are_classified_independently() {
        use crate::app::presentation::RowTone;

        assert_eq!(TaskQueueOutcome::Parked.tone(), RowTone::Fail);
        assert_eq!(TaskQueueOutcome::MergeFailed.tone(), RowTone::Fail);
        assert_eq!(
            TaskQueueOutcome::NotCompleted {
                status: "blocked".to_string(),
                outcome: None,
            }
            .tone(),
            RowTone::Fail
        );
        assert_eq!(
            TaskQueueOutcome::Failed {
                message: "boom".to_string(),
            }
            .tone(),
            RowTone::Fail
        );
        assert_eq!(
            TaskQueueOutcome::Landed { closed: true }.tone(),
            RowTone::Pass
        );
        assert_eq!(
            TaskQueueOutcome::Landed { closed: false }.tone(),
            RowTone::Pass
        );
        assert_eq!(TaskQueueOutcome::Completed.tone(), RowTone::Pass);
        assert_eq!(TaskQueueOutcome::NotMerged.tone(), RowTone::Pass);
        let disk_full = TaskQueueOutcome::DiskFull {
            park: Some(ctx_traits_core::procedure::session::DiskFullPark {
                floor_mb: 512,
                available_bytes: 42,
                probed_path: "/worktree".to_string(),
            }),
        };
        assert!(disk_full.halts());
        assert!(!disk_full.fails());
        assert_eq!(disk_full.tone(), RowTone::Warn);
        assert_eq!(
            disk_full.label(),
            "parked: disk-full (floor 512 MiB, 42 bytes free at /worktree)"
        );
    }

    #[test]
    fn panel_reports_success_with_every_row_and_no_hint_when_nothing_halts() {
        let outcomes = vec![
            (
                "0001".to_string(),
                TaskQueueOutcome::Landed { closed: true },
            ),
            ("0002".to_string(), TaskQueueOutcome::NotMerged),
        ];
        let lines = task_queue_panel(&outcomes, false, 2).plain_lines();
        assert!(lines.contains(&"  0001: landed, closed".to_string()));
        assert!(lines.contains(&"  0002: committed, not merged".to_string()));
        assert!(!lines.iter().any(|line| line.contains("remaining")));
        assert!(!lines.iter().any(|line| line.contains("next")));
        assert_eq!(lines.last(), Some(&"Success".to_string()));
    }

    #[test]
    fn panel_reports_failure_with_no_hint_when_continue_on_failure_ran_to_completion() {
        let outcomes = vec![
            ("0001".to_string(), TaskQueueOutcome::Parked),
            ("0002".to_string(), TaskQueueOutcome::NotMerged),
        ];
        let lines = task_queue_panel(&outcomes, false, 2).plain_lines();
        assert!(!lines.iter().any(|line| line.contains("remaining")));
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("--continue-on-failure"))
        );
        assert_eq!(lines.last(), Some(&"Failure".to_string()));
    }

    #[test]
    fn panel_names_remaining_work_when_the_queue_halted_early() {
        let outcomes = vec![("0001".to_string(), TaskQueueOutcome::Parked)];
        let lines = task_queue_panel(&outcomes, true, 2).plain_lines();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("remaining") && line.contains("1 not attempted"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("next") && line.contains("--continue-on-failure"))
        );
        assert_eq!(lines.last(), Some(&"Failure".to_string()));
    }

    #[test]
    fn panel_names_no_remaining_work_when_the_halt_lands_on_the_last_member() {
        let outcomes = vec![("0001".to_string(), TaskQueueOutcome::Parked)];
        let lines = task_queue_panel(&outcomes, true, 1).plain_lines();
        assert!(lines.iter().any(|line| line.contains("0001")));
        assert!(!lines.iter().any(|line| line.contains("remaining")));
        assert!(!lines.iter().any(|line| line.contains("next")));
        assert_eq!(lines.last(), Some(&"Failure".to_string()));
    }

    #[test]
    fn panel_reports_success_for_an_empty_queue() {
        let lines = task_queue_panel(&[], false, 0).plain_lines();
        assert!(!lines.iter().any(|line| line.starts_with("  ")));
        assert_eq!(lines.last(), Some(&"Success".to_string()));
    }

    #[test]
    fn disk_full_halts_as_a_park_and_continue_runs_past_it() {
        let queue = vec!["0001".to_string(), "0002".to_string()];
        let (outcomes, halted) =
            drive_task_queue(&queue, false, |_| TaskQueueOutcome::DiskFull { park: None });
        assert!(halted);
        assert_eq!(outcomes.len(), 1);
        let lines = task_queue_panel(&outcomes, halted, queue.len()).plain_lines();
        assert_eq!(lines.last(), Some(&"Parked".to_string()));
        assert!(!lines.iter().any(|line| line == "Failure"));

        let (outcomes, halted) =
            drive_task_queue(&queue, true, |_| TaskQueueOutcome::DiskFull { park: None });
        assert!(!halted);
        assert_eq!(outcomes.len(), 2);
    }
}

/// The final `--task` queue report (0195/0252.9), routed through the shared
/// panel kit for every non-JSON mode. Closing state is driven by whether any
/// outcome failed, never by `halted` alone; a disk-full park still halts but
/// closes as a park rather than a failure.
fn task_queue_panel(
    outcomes: &[(String, TaskQueueOutcome)],
    halted: bool,
    queue_len: usize,
) -> crate::app::presentation::Panel {
    use crate::app::presentation::{Panel, PanelRow, PanelStatus, RowTone};

    let failed = outcomes.iter().any(|(_, outcome)| outcome.fails());
    let parked = outcomes.iter().any(|(_, outcome)| outcome.halts());
    let mut panel = Panel::new(
        "task queue",
        "",
        if failed {
            PanelStatus::Blocked("Failure".to_string())
        } else if parked {
            PanelStatus::Blocked("Parked".to_string())
        } else {
            PanelStatus::Passed("Success".to_string())
        },
    );
    for (key, outcome) in outcomes {
        panel = panel.row(PanelRow::toned(key, outcome.label(), outcome.tone()));
    }
    let remaining = queue_len.saturating_sub(outcomes.len());
    if halted && remaining > 0 {
        panel = panel
            .row(PanelRow::toned(
                "remaining",
                format!("{remaining} not attempted"),
                RowTone::Warn,
            ))
            .next(PanelRow::toned(
                "next",
                "rerun with --continue-on-failure to attempt the rest",
                RowTone::Default,
            ));
    }
    panel
}

/// P550 run-termination story hook: interactive-TTY-only pane, plain-text
/// story otherwise. Never called under `--json` (the `json` branch above
/// returns before reaching here) — `--json` output stays byte-identical to
/// today, the story is already available separately via `ctx traits internal story
/// --json`. `stdio` uses the same three-way TTY rule the drive TUI default
/// applies (`dashboard::interactive_available` plus a stdout check), so
/// `[drive] story = "default"` stays inert in CI/scripts beyond this plain
/// text block.
fn print_story_at_termination(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
    level: ctx_traits_core::procedure::story::StoryLevel,
    verbose: bool,
) -> crate::Result<()> {
    use std::io::IsTerminal;

    let plan = crate::app::story::load_plan(session);
    let activity = crate::app::story::load_activity(ledger_path);
    let report =
        ctx_traits_core::procedure::story::build(session, plan.as_ref(), activity.as_ref());
    let interactive =
        crate::app::dashboard::interactive_available() && std::io::stdout().is_terminal();
    if interactive {
        let disposition = crate::app::story::disposition_sentence(session, &report);
        let title = format!("story · {} · {disposition}", session.run_id.as_str());
        return crate::app::story_view::run(session, &report, level, &title);
    }
    // The plain (non-interactive) termination story stays brief by default —
    // disposition, outcome, never-cleared blockers. The full section walk is
    // an explicit ask: `--verbose`, or a story level beyond the default.
    if verbose || level != ctx_traits_core::procedure::story::StoryLevel::Default {
        return crate::app::story::print_plain_story(session, &report, level);
    }
    crate::app::story::print_plain_story_brief(session, &report)
}

/// Typed terminal disposition of the P460 post-drive completion-to-landing
/// hook, separating every distinct outcome an observer must be able to tell
/// apart: no automatic-landing intent was ever recorded, a recorded intent's
/// drive never completed, a merge landed, a merge parked (branch/worktree
/// intact), or a merge reached a terminal non-park failure (cross-process
/// lock contention/timeout, or a post-fast-forward cleanup/recovery
/// failure). Reused for both a merge attempted by this invocation and a
/// prior invocation's terminal outcome discovered on resume, so a later
/// `drive` over an already-decided session reports that same outcome
/// honestly instead of collapsing it to "no intent" (P460 review).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionDisposition {
    NoIntent,
    DriveNotCompleted,
    Merged,
    Parked,
    Failed,
}

/// The branch and merge command a "committed but not merged" report line
/// names (0151) — extracted once so story, the run TUI, and the plain drive
/// report render the same fact instead of drifting apart across three
/// separate renderers.
#[derive(Debug, Clone)]
pub(crate) struct NotMergedFact {
    pub(crate) branch: String,
    pub(crate) merge_command: String,
}

/// `None` only when `session` was never a `--worktree` run, in which case
/// [`ctx_traits_core::procedure::session::landing_state`] never resolves to
/// `NotMerged` in the first place — this exists to hand the same two facts
/// to every caller, not to re-derive whether they apply.
pub(crate) fn not_merged_fact(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<NotMergedFact> {
    not_merged_fact_from_parts(
        session
            .provenance
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.as_str()),
        session.run_id.as_str(),
    )
}

/// Row-shaped counterpart of [`not_merged_fact`]. Center clients retain only
/// the branch and run id required to render this fact.
pub(crate) fn not_merged_fact_from_parts(
    branch: Option<&str>,
    run_id: &str,
) -> Option<NotMergedFact> {
    let branch = branch?;
    Some(NotMergedFact {
        branch: branch.to_string(),
        merge_command: format!("ctx traits merge {run_id}"),
    })
}

/// `not_merged_fact` gated on `landing_state` actually being `NotMerged` —
/// the exact check that the dashboard's Sessions, Merges, and Tasks
/// surfaces (0197), story, and drive completion all need before they may
/// show the fact. Extracted so those sites cannot drift on the gate.
pub(crate) fn unmerged_fact(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<NotMergedFact> {
    matches!(
        ctx_traits_core::procedure::session::landing_state(session),
        Some(ctx_traits_core::procedure::session::LandingState::NotMerged)
    )
    .then(|| not_merged_fact(session))
    .flatten()
}

/// The single failure explanation used by terminal human output and the
/// command error mapping, ordered from durable session intent to drive-local
/// observations.
pub(crate) fn failure_reason(
    session: &ctx_traits_core::procedure::session::Session,
    drive: &crate::app::drive::DriveReport,
) -> Option<String> {
    let stop_message = session
        .stop_reason
        .as_ref()
        .and_then(|stop| stop.message.clone());
    failure_reason_from_values(
        stop_message.as_deref(),
        session
            .stop_reason
            .as_ref()
            .map(|stop| stop.reason.as_str()),
        drive.bound_fired.as_deref(),
        drive.warnings.first().map(String::as_str),
        Some(drive.status.as_str()),
    )
}

fn failure_reason_from_values(
    stop_message: Option<&str>,
    stop_reason: Option<&str>,
    bound_fired: Option<&str>,
    warning: Option<&str>,
    drive_status: Option<&str>,
) -> Option<String> {
    stop_message
        .or(stop_reason)
        .or(bound_fired)
        .or(warning)
        .or(drive_status)
        .map(str::to_string)
}

/// A terminal run succeeds only when the persisted session and this drive
/// invocation both reached completion.
fn run_completed(
    session: &ctx_traits_core::procedure::session::Session,
    drive: &crate::app::drive::DriveReport,
) -> bool {
    run_completed_status(session.status.clone(), drive.final_session_status.clone())
}

fn run_completed_status(
    session_status: ctx_traits_core::procedure::session::Status,
    drive_status: Option<ctx_traits_core::procedure::session::Status>,
) -> bool {
    session_status == ctx_traits_core::procedure::session::Status::Completed
        && drive_status == Some(ctx_traits_core::procedure::session::Status::Completed)
}

fn human_terminal_failure(
    human_output: bool,
    session: &ctx_traits_core::procedure::session::Session,
    drive: &crate::app::drive::DriveReport,
) -> bool {
    human_terminal_failure_values(
        human_output,
        drive.credits_pause.is_some(),
        drive.budget_pause.is_some(),
        drive.disk_full_park.is_some(),
        drive.status == "awaiting-owner",
        session.status.clone(),
        drive.final_session_status.clone(),
    )
}

/// `disk_full_parked` is carried only when the durable disk-full evidence was
/// retained in the report. `summons_parked` is keyed on the drive's own reported outcome string
/// (`report.status`, downgraded to `"harness-failed"` if the drive-outcome
/// marker failed to persist, mirroring `credits_paused`/`budget_paused`) —
/// never the session's raw `Status::WaitingOnHuman`, which reflects frame
/// readiness independent of whether the terminal outcome was actually
/// recorded. A summons whose evidence failed to write must still report a
/// failure (P0253.4).
fn human_terminal_failure_values(
    human_output: bool,
    credits_paused: bool,
    budget_paused: bool,
    disk_full_parked: bool,
    summons_parked: bool,
    session_status: ctx_traits_core::procedure::session::Status,
    drive_status: Option<ctx_traits_core::procedure::session::Status>,
) -> bool {
    human_output
        && !credits_paused
        && !budget_paused
        && !disk_full_parked
        && !summons_parked
        && !run_completed_status(session_status, drive_status)
}

pub(crate) fn short_failure_line(reason: Option<&str>) -> String {
    const LIMIT: usize = 64;
    let text = reason
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let line = crate::app::tui::clean_live_text(line)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (!line.is_empty()).then_some(line)
        })
        .unwrap_or_else(|| "run failed".to_string());
    crate::app::tui::truncate_display_width_end(&text, LIMIT)
}

fn restart_out(_: Option<&str>) -> Option<&str> {
    None
}

fn restart_worktree(worktree: Option<Option<&str>>) -> Option<Option<&str>> {
    match worktree {
        Some(Some(_)) => Some(None),
        other => other,
    }
}

/// Data needed to render a propagated drive error after its pane is closed.
struct PropagatedDriveError<'a> {
    trait_id: &'a str,
    session_id: &'a str,
    line: &'a str,
}

/// Resolves a propagated drive error after `drive()` has released its lock.
/// Startup and output remain call-site seams because they require CLI inputs.
fn resolve_propagated_drive_error_choice<T>(
    choice: crate::app::run_view::FailureChoice,
    panel: crate::app::run_view::RunPanel,
    handoff: &crate::app::drive::PanelHandoff,
    attempt: &mut T,
    start: impl FnOnce() -> crate::Result<T>,
    abort: impl FnOnce(crate::app::run_view::RunPanel) -> crate::Result<()>,
) -> crate::Result<()> {
    match choice {
        crate::app::run_view::FailureChoice::Resume => {
            panel.clear_failure_modal();
            handoff.give(panel);
            Ok(())
        }
        crate::app::run_view::FailureChoice::Restart => {
            restart_propagated_attempt(panel, attempt, start)
        }
        crate::app::run_view::FailureChoice::Abort => abort(panel),
    }
}

/// Closes the retained pane before crossing the fresh-session boundary and
/// replaces the active attempt only after startup succeeds.
fn restart_propagated_attempt<T>(
    panel: crate::app::run_view::RunPanel,
    attempt: &mut T,
    start: impl FnOnce() -> crate::Result<T>,
) -> crate::Result<()> {
    panel.close();
    *attempt = start()?;
    Ok(())
}

fn propagated_drive_error_abort(
    panel: crate::app::run_view::RunPanel,
    error: PropagatedDriveError<'_>,
    emit: impl FnOnce(&crate::app::presentation::Panel) -> crate::Result<()>,
) -> crate::Result<()> {
    panel.close();
    let (product, headline) = run_header_from_title(error.trait_id, None);
    let panel = failure_panel(&product, &headline, error.session_id, error.line);
    emit(&panel)?;
    Err(crate::Error::AlreadyReported {
        message: String::new(),
        exit_code: 1,
    })
}

/// The compact panel's landing fact comes exclusively from terminal merge
/// evidence. Post-merge cleanup failure still means main advanced.
fn merged_fact(session: &ctx_traits_core::procedure::session::Session) -> Option<String> {
    let terminal = session
        .provenance
        .merge_frames
        .iter()
        .rev()
        .find(|frame| frame.status.is_terminal());
    merged_fact_from_terminal(
        terminal.map(|frame| {
            (
                frame.status,
                frame
                    .evidence
                    .iter()
                    .find_map(|entry| entry.strip_prefix("landed=")),
            )
        }),
        unmerged_fact(session).map(|fact| fact.merge_command),
        not_merged_fact(session).map(|fact| fact.merge_command),
    )
}

fn merged_fact_from_terminal(
    terminal: Option<(
        ctx_traits_core::procedure::session::MergeStatus,
        Option<&str>,
    )>,
    unmerged_command: Option<String>,
    terminal_unmerged_command: Option<String>,
) -> Option<String> {
    use ctx_traits_core::procedure::session::MergeStatus;

    match terminal {
        Some((MergeStatus::Merged | MergeStatus::PostMergeCleanupFailure, revision)) => {
            revision.map(|revision| format!("yes ({revision})"))
        }
        Some((MergeStatus::Parked | MergeStatus::RecoveryFailure, _)) => {
            terminal_unmerged_command.map(|command| format!("no ({command})"))
        }
        None => unmerged_command.map(|command| format!("no ({command})")),
        Some((
            MergeStatus::LockAcquired | MergeStatus::GatesPassed | MergeStatus::Reconciled,
            _,
        )) => None,
    }
}

pub(crate) fn disposition_for_report_status(status: &str) -> CompletionDisposition {
    match status {
        "merged" => CompletionDisposition::Merged,
        "parked" => CompletionDisposition::Parked,
        _ => CompletionDisposition::Failed,
    }
}

impl CompletionDisposition {
    /// The single status→exit mapping both `run --merge` and standalone
    /// `ctx traits merge` use, so the two verbs cannot diverge on what a
    /// given terminal merge status means for the process exit code. `None`
    /// means exit 0 (no intent, or an actual landing).
    pub(crate) fn exit_code(self) -> Option<u8> {
        match self {
            CompletionDisposition::NoIntent | CompletionDisposition::Merged => None,
            CompletionDisposition::DriveNotCompleted => {
                Some(crate::app::error::EXIT_RUN_NOT_COMPLETED)
            }
            CompletionDisposition::Parked => Some(crate::app::error::EXIT_MERGE_PARKED),
            CompletionDisposition::Failed => Some(crate::app::error::EXIT_MERGE_FAILED),
        }
    }
}

/// Maps a prior *terminal* [`MergeStatus`](ctx_traits_core::procedure::session::MergeStatus)
/// frame to its disposition. Only ever called on a frame `is_terminal()`
/// already selected, so the nonterminal arms are unreachable in practice;
/// they still resolve to `Failed` (never `Parked`) rather than panic, since a
/// disposition mapping must not itself be a new park hazard.
pub(crate) fn disposition_for_merge_status(
    status: ctx_traits_core::procedure::session::MergeStatus,
) -> CompletionDisposition {
    use ctx_traits_core::procedure::session::MergeStatus;
    match status {
        MergeStatus::Merged => CompletionDisposition::Merged,
        MergeStatus::Parked => CompletionDisposition::Parked,
        MergeStatus::PostMergeCleanupFailure
        | MergeStatus::RecoveryFailure
        | MergeStatus::LockAcquired
        | MergeStatus::GatesPassed
        | MergeStatus::Reconciled => CompletionDisposition::Failed,
    }
}

/// Outcome of the P460 post-drive completion-to-landing hook: the final
/// session (re-read from the ledger after a successful merge appends its
/// evidence), the embedded merge report (present only when this invocation
/// itself attempted a merge — absent for "no intent" and for a prior
/// invocation's already-terminal outcome discovered on resume), and the
/// typed terminal disposition driving the exit-status mapping below.
pub(crate) struct CompletionOutcome {
    pub(crate) session: ctx_traits_core::procedure::session::Session,
    pub(crate) merge: Option<crate::app::merge::MergeReport>,
    /// The just-persisted marker retained across `run::status` reconstruction
    /// for private queue classification; it is never part of public output.
    drive_outcome: Option<ctx_traits_core::procedure::session::DriveOutcome>,
    failure_reason: Option<String>,
    human_terminal_failure: bool,
    disposition: CompletionDisposition,
}

impl CompletionOutcome {
    fn with_drive_outcome(
        mut self,
        drive_outcome: Option<ctx_traits_core::procedure::session::DriveOutcome>,
    ) -> Self {
        self.drive_outcome = drive_outcome;
        self
    }

    /// Attach the drive-local part of a terminal failure explanation before
    /// the outcome is converted into the command's established exit mapping.
    pub(crate) fn with_drive_report(mut self, drive: &crate::app::drive::DriveReport) -> Self {
        self.failure_reason = failure_reason(&self.session, drive);
        self
    }

    /// Human `run` output reports every terminal non-completion as Failure,
    /// without changing JSON or hidden-drive exit semantics.
    fn with_human_terminal_failure(
        mut self,
        human_output: bool,
        drive: &crate::app::drive::DriveReport,
    ) -> Self {
        self.human_terminal_failure = human_terminal_failure(human_output, &self.session, drive);
        self
    }

    /// Centralized P460 exit-status mapping, driven by the typed disposition
    /// rather than re-inferring it from `(intent, report)` optionality: no
    /// intent or a landed merge exits 0; a merge intent present on a run
    /// that never reached a completed drive exits
    /// [`crate::app::error::EXIT_RUN_NOT_COMPLETED`]; an actual park exits
    /// [`crate::app::error::EXIT_MERGE_PARKED`]; every other terminal merge
    /// failure (lock contention/timeout, cleanup/recovery failure) exits the
    /// distinct [`crate::app::error::EXIT_MERGE_FAILED`] rather than
    /// falsely claiming the park invariant.
    pub(crate) fn into_command_output(self) -> crate::Result<CommandOutput<()>> {
        let run_id = self.session.run_id.as_str().to_string();
        let reason_suffix = self
            .merge
            .as_ref()
            .and_then(|report| report.reason.as_deref())
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        match self.disposition {
            CompletionDisposition::NoIntent | CompletionDisposition::Merged => {
                // 0189: a session can complete its DRIVE and still declare
                // itself failed (an authored flow.error terminal, or
                // no-exit-reached past every success exit). With no merge in
                // play nothing downstream would surface that — the process
                // exit must.
                if self.session.status == ctx_traits_core::procedure::session::Status::Failed
                    || self.human_terminal_failure
                {
                    let reason = self
                        .failure_reason
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default();
                    let message = if self.session.status
                        == ctx_traits_core::procedure::session::Status::Failed
                    {
                        format!("run {run_id:?} failed{reason}")
                    } else {
                        format!("run {run_id:?} did not reach a completed drive{reason}")
                    };
                    return Err(crate::Error::AlreadyReported {
                        message,
                        exit_code: crate::app::error::EXIT_RUN_FAILED,
                    });
                }
                Ok(CommandOutput::new(()))
            }
            CompletionDisposition::DriveNotCompleted => Err(crate::Error::AlreadyReported {
                message: format!(
                    "run {run_id:?} did not reach a completed drive; merge was not attempted"
                ),
                exit_code: crate::app::error::EXIT_RUN_NOT_COMPLETED,
            }),
            CompletionDisposition::Parked => Err(crate::Error::AlreadyReported {
                message: format!(
                    "run {run_id:?} completed but did not land (merge status \"parked\"){reason_suffix}"
                ),
                exit_code: crate::app::error::EXIT_MERGE_PARKED,
            }),
            CompletionDisposition::Failed => Err(crate::Error::AlreadyReported {
                message: format!(
                    "run {run_id:?} completed but its automatic merge did not reach landing completion or a park{reason_suffix}"
                ),
                exit_code: crate::app::error::EXIT_MERGE_FAILED,
            }),
        }
    }
}

/// Shared by [`handle_session_start`] and the standalone `drive` resume
/// (P460): after `drive()` has durably recorded its outcome and released the
/// driver lock, land a completed run that carries a persisted merge intent.
/// Reuses `merge::merge` unchanged; a no-op (returns `merge: None`) whenever
/// no intent was persisted, the drive did not complete, the last recorded
/// drive outcome was not itself `completed`, or a prior merge attempt for
/// this session already reached a terminal outcome (one-shot: a park is
/// never automatically retried, see [`MergeStatus::is_terminal`]).
///
/// `session_path` is the exact ledger path the caller already resolved
/// (`--out`, an explicit `drive --session <path>`, or the default
/// session-store path) — reading it directly, instead of re-scanning
/// `session_store` by run-id, keeps this working for a ledger written
/// outside `session_store` (P460 review).
///
/// [`MergeStatus::is_terminal`]: ctx_traits_core::procedure::session::MergeStatus::is_terminal
/// P549: owns the merge span's live surface for exactly as long as its
/// caller holds it — dropping it stops the presentation-only elapsed
/// ticker and closes the handed-off panel, mirroring `drive.rs`'s
/// `RunPanelGuard` (the 2026-07-22 terminal-restore incident discipline:
/// the panel is never owned by anything but a guard). Held even when
/// `complete_after_drive` turns out to be a no-op (no merge intent) — the
/// caller must always take a handoff panel back out of drive() and close
/// it exactly once, and this guard is the one place that happens.
pub(crate) struct MergeSpanGuard {
    panel: Option<crate::app::run_view::RunPanel>,
    stop: Option<Arc<AtomicBool>>,
    ticker: Option<std::thread::JoinHandle<()>>,
    /// P549: the merge span's own narrator, when a seat resolved — `finish`ed
    /// here (non-blocking by design; a late result is dropped by the
    /// narrator's own request-generation check) before the panel closes, so
    /// nothing keeps writing into a panel that is about to disappear.
    narrator: Option<crate::app::harness_stream::StreamNarrator>,
}

impl Drop for MergeSpanGuard {
    fn drop(&mut self) {
        if let Some(narrator) = self.narrator.take() {
            narrator.finish();
        }
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Release);
        }
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
        if let Some(panel) = self.panel.take() {
            panel.close();
        }
    }
}

/// P549: the merge span's live sink — folds into the handed-off `RunPanel`'s
/// merge rows when drive released one (a TUI run whose drive completed
/// normally), else the plain stage-boundary stderr line
/// ([`crate::app::merge::plain_stage_line_live`]) that replaces the deleted
/// dim tick line off-pane. Always returns a live sink (merge is never
/// silent on a surface that used to show the dim line) alongside the guard
/// that must outlive the `complete_after_drive` call it feeds, and the
/// merger-call stdout observer `complete_after_drive` must thread into
/// `MergeInputs::merger_stdout_observer`: when a panel is present, a
/// narrator seat resolved from `assignments` (`[agent.role.narrator]`,
/// exactly as `drive` resolves it) feeds a real `StreamNarrator` whose
/// summaries/tokens land on the panel; with a panel but no seat, the
/// merger's own stream text is shown verbatim via the panel's passthrough
/// (`RunPanel::push_bytes`) — the seat doctrine's "absent narrator table
/// means passthrough" extended to merge narration; with no panel at all
/// (status-mode, or standalone `ctx traits merge`), `None` — the plain/TTY
/// stage-boundary lines already cover that surface.
pub(crate) fn merge_live_for_completion(
    panel: Option<crate::app::run_view::RunPanel>,
    run_id: &str,
    session_id: &str,
    assignments: &[String],
) -> (
    crate::app::merge::MergeLive,
    Option<ctx_traits_io::harness::OutputObserver>,
    MergeSpanGuard,
) {
    let Some(panel) = panel else {
        return (
            crate::app::merge::plain_stage_line_live(),
            None,
            MergeSpanGuard {
                panel: None,
                stop: None,
                ticker: None,
                narrator: None,
            },
        );
    };
    let event_panel = panel.clone();
    let live = crate::app::merge::MergeLive::new(move |event| event_panel.merge_event(&event));
    // Presentation-only repaint every 500ms (panel.tick() itself throttles
    // the actual redraw to 1s): unlike a driven frame's `tick_observer`,
    // nothing else pumps the pane during a long gate command or merger
    // call, so a Running merge row's elapsed clock would otherwise freeze
    // between discrete events.
    let stop = Arc::new(AtomicBool::new(false));
    let ticker_panel = panel.clone();
    let ticker_stop = Arc::clone(&stop);
    let ticker = std::thread::spawn(move || {
        while !ticker_stop.load(Ordering::Acquire) {
            ticker_panel.tick();
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    let (merger_stdout_observer, narrator) =
        merger_narrator_or_passthrough(&panel, run_id, session_id, assignments);
    (
        live,
        merger_stdout_observer,
        MergeSpanGuard {
            panel: Some(panel),
            stop: Some(stop),
            ticker: Some(ticker),
            narrator,
        },
    )
}

/// P549: resolve `[agent.role.narrator]` exactly as `drive` does and, when
/// present, wire one cold [`crate::app::harness_stream::StreamNarrator`] over
/// the merge span whose summaries/tokens land on `panel` — otherwise fall
/// back to `panel`'s own raw passthrough so the merger's own stream text is
/// still visible with zero model spend (no seat configured is a valid,
/// deliberate mode — same posture `drive` takes for a driven frame). A
/// resolution failure (bad `[agent]` config) degrades to passthrough rather
/// than failing the merge itself, which is not this narration's to block.
fn merger_narrator_or_passthrough(
    panel: &crate::app::run_view::RunPanel,
    run_id: &str,
    session_id: &str,
    assignments: &[String],
) -> (
    Option<ctx_traits_io::harness::OutputObserver>,
    Option<crate::app::harness_stream::StreamNarrator>,
) {
    let config = ctx_traits_io::harness_config::resolve_runtime_assignments(assignments)
        .ok()
        .and_then(|mut profile| {
            crate::app::drive::cold_narrator_config_for_merge(
                &mut profile,
                crate::app::drive::ColdNarratorContext {
                    run_id,
                    session_id,
                    env_overlay: &std::collections::BTreeMap::new(),
                    confinement_payloads: None,
                    exec_dir: None,
                    trace_sequence: &Arc::new(std::sync::atomic::AtomicU64::new(0)),
                },
            )
        });
    let Some(config) = config else {
        let passthrough_panel = panel.clone();
        return (
            Some(Arc::new(move |chunk: &[u8]| {
                passthrough_panel.push_bytes(chunk)
            })),
            None,
        );
    };
    let summary_sink = panel.clone();
    let tokens_sink = panel.clone();
    let narrator = crate::app::harness_stream::StreamNarrator::new(
        config,
        crate::app::harness_stream::NarratorSinks {
            summary: Arc::new(move |summary| summary_sink.push_summary(summary)),
            tokens: Arc::new(move |tokens| tokens_sink.add_narrator_tokens(tokens)),
            // A merge span has no discrete "step" for a P455 finish-with-
            // summary call, and no separate live-line pill for in-progress
            // thinking tokens — both sinks are unreachable for this
            // narrator, same posture `--progress stream`'s narrator takes.
            step_summary: Arc::new(|_context, _summary| {}),
            thinking_tokens: Arc::new(|_tokens| {}),
        },
        crate::app::harness_stream::NarratorTokenTracker::default(),
    );
    let feeder = narrator.feeder();
    (
        Some(Arc::new(move |chunk: &[u8]| feeder.feed(chunk))),
        Some(narrator),
    )
}

pub(crate) fn complete_after_drive(
    session_store: Option<&str>,
    session_path: &camino::Utf8Path,
    assignments: &[String],
    final_session: ctx_traits_core::procedure::session::Session,
    live: crate::app::merge::MergeLive,
    merger_stdout_observer: Option<ctx_traits_io::harness::OutputObserver>,
) -> crate::Result<CompletionOutcome> {
    let intent = final_session.provenance.merge_intent;
    let Some(rung) = intent else {
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
            drive_outcome: None,
            failure_reason: None,
            human_terminal_failure: false,
            disposition: CompletionDisposition::NoIntent,
        });
    };
    if final_session.status != ctx_traits_core::procedure::session::Status::Completed {
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
            drive_outcome: None,
            failure_reason: None,
            human_terminal_failure: false,
            disposition: CompletionDisposition::DriveNotCompleted,
        });
    }
    // `final_session` is a rebuilt inspection snapshot: core's rebuild
    // (`refresh_run_session`) always clears `last_drive_outcome` (unlike
    // `provenance`, which threads through unchanged). Re-read the raw
    // persisted ledger to check the actual recorded drive outcome instead of
    // trusting a field that is never present here, and to check whether a
    // prior merge attempt already reached a terminal outcome (one-shot
    // landing: an automatic merge is never retried after it parks).
    let raw_session = ctx_traits_io::run_session::read_run_session(session_path)?;
    let raw_completed = raw_session
        .last_drive_outcome
        .as_ref()
        .is_some_and(|outcome| outcome.outcome.is_completed());
    if !raw_completed {
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
            drive_outcome: None,
            failure_reason: None,
            human_terminal_failure: false,
            disposition: CompletionDisposition::DriveNotCompleted,
        });
    }
    let prior_terminal_status = raw_session
        .provenance
        .merge_frames
        .iter()
        .rev()
        .find(|frame| frame.status.is_terminal())
        .map(|frame| frame.status);
    if let Some(status) = prior_terminal_status {
        // A prior invocation's automatic merge already reached a terminal
        // outcome for this session: one-shot landing means this resume must
        // not attempt (or report) a second one. Its exit status must still
        // reflect that prior outcome honestly (P460 review) — collapsing
        // every case to "no intent" let a later resume over an already
        // parked run silently exit 0. The persisted `merge_frames`/
        // `merge_intent` history stays readable via `ctx traits internal session
        // state`/`inspect` regardless.
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
            drive_outcome: None,
            failure_reason: None,
            human_terminal_failure: false,
            disposition: disposition_for_merge_status(status),
        });
    }
    let run_id = final_session.run_id.as_str().to_string();
    let deep = rung == ctx_traits_core::procedure::session::MergeRung::Deep;
    let report = crate::app::merge::merge(crate::app::merge::MergeInputs {
        run_id: &run_id,
        session_store,
        session_path_override: Some(session_path),
        assignments,
        no_wait: false,
        force_wait: false,
        json: false,
        verbose: false,
        force_merger: false,
        park_on_overlap: false,
        force_land_on_overlap: false,
        allow_stale_overlap: false,
        deep,
        live: Some(live),
        merger_stdout_observer,
    })?;
    let disposition = disposition_for_report_status(&report.status);
    // A successful merge removes the worktree but not the session ledger;
    // re-read it so the returned session's `merge_frames` reflect the
    // attempt just made, rather than the pre-merge snapshot. A merge that
    // reports `merged` deletes the ledger's worktree, never the ledger
    // itself, so `session_path` still resolves.
    let session =
        ctx_traits_io::run_session::read_run_session(session_path).unwrap_or(final_session);
    Ok(CompletionOutcome {
        session,
        merge: Some(report),
        drive_outcome: None,
        failure_reason: None,
        human_terminal_failure: false,
        disposition,
    })
}

fn print_final_output(
    session: &ctx_traits_core::procedure::session::Session,
    drive: &crate::app::drive::DriveReport,
    ledger_path: &camino::Utf8Path,
    trait_ref: Option<&ctx_traits_core::Trait>,
    failure_reason: Option<&str>,
    verbose: bool,
) -> crate::Result<()> {
    use crate::app::presentation::{
        HumanOutputMode, Panel, PanelRow, PanelSection, RowTone, emit_human,
    };

    if let Some(pause) = &drive.credits_pause {
        crate::app::drive::print_credits_pause(pause, &drive.session)?;
        return Ok(());
    }
    if let Some(pause) = &drive.budget_pause {
        crate::app::drive::print_budget_pause(
            pause,
            drive.tokens_by_model.as_ref(),
            &drive.session,
        )?;
        return Ok(());
    }
    if let Some(park) = &drive.disk_full_park {
        crate::app::drive::print_disk_full_park(park, &drive.session)?;
        return Ok(());
    }

    let mode = if verbose {
        HumanOutputMode::Verbose
    } else {
        HumanOutputMode::Compact
    };

    let (product, headline) = run_header(session, ledger_path);
    let failed = !run_completed(session, drive);
    let mut panel = if failed {
        failure_panel(
            &product,
            &headline,
            session.session_id.as_str(),
            failure_reason.unwrap_or("run failed"),
        )
    } else {
        let mut panel = Panel::new(
            product,
            headline,
            crate::app::presentation::PanelStatus::Passed("Success".to_string()),
        )
        .row(PanelRow::toned(
            "session",
            session.session_id.as_str(),
            RowTone::Default,
        ));
        if let Some(landing) = ctx_traits_core::procedure::session::landing_state(session) {
            if let Some(merged) = merged_fact(session) {
                panel = panel.row(PanelRow::toned("merged", merged, RowTone::Default));
            }
            if verbose {
                panel = panel.row(PanelRow::toned(
                    "landing",
                    landing_detail(&landing),
                    RowTone::Default,
                ));
            }
        }
        panel
    };
    if verbose {
        match &session.completion {
            Some(completion) if !completion.final_outputs.is_empty() => {
                for output in &completion.final_outputs {
                    if let Some(rendered) = trait_ref.and_then(|trait_ref| {
                        structured_output::resolve(trait_ref, output.port_ref.id(), &output.value)
                    }) {
                        let verdict =
                            structured_output::producer_verdict_for_output(session, output);
                        // Every line `compact_lines` returns — the count header,
                        // every item row, and the receipt line — becomes its own
                        // panel row; none are truncated, so `--verbose` (which
                        // prints this panel and then the full per-field stanzas
                        // as additional detail) stays a strict superset of the
                        // default rendering rather than replacing it.
                        let rows = rendered
                            .compact_lines("completed", verdict.as_deref(), Some(&drive.session))
                            .into_iter()
                            .enumerate()
                            .map(|(index, line)| compact_line_to_row(index, &line))
                            .collect();
                        panel = panel.section(PanelSection::new(output.port_ref.id(), rows));
                    } else {
                        panel = panel.row(PanelRow::toned(
                            output.port_ref.id(),
                            structured_output::clean_value(&output.value),
                            RowTone::Default,
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    emit_human(false, &panel, mode, || {
        match &session.completion {
            Some(completion) if !completion.final_outputs.is_empty() => {
                for output in &completion.final_outputs {
                    if let Some(rendered) = trait_ref.and_then(|trait_ref| {
                        structured_output::resolve(trait_ref, output.port_ref.id(), &output.value)
                    }) {
                        let verdict =
                            structured_output::producer_verdict_for_output(session, output);
                        let lines = rendered.verbose_lines("completed", verdict.as_deref());
                        println!("{}:", output.port_ref.id());
                        for line in lines {
                            println!("  {line}");
                        }
                    } else {
                        println!(
                            "{}: {}",
                            output.port_ref.id(),
                            structured_output::clean_value(&output.value)
                        );
                    }
                }
            }
            _ => println!("{}", drive.status),
        }
        Ok(())
    })
}

fn landing_detail(landing: &ctx_traits_core::procedure::session::LandingState) -> String {
    use ctx_traits_core::procedure::session::LandingState;

    match landing {
        LandingState::Landed {
            revision: Some(revision),
        } => format!("merged to main ({revision})"),
        LandingState::Landed { revision: None } => "merged to main".to_string(),
        LandingState::NotMerged => "committed but not merged".to_string(),
        LandingState::Parked => "merge parked".to_string(),
        LandingState::MergeFailed => "merge failed after landing attempt".to_string(),
    }
}

fn run_header(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> (String, String) {
    run_header_from_title(
        &session.trait_id,
        persisted_session_title(session, ledger_path),
    )
}

pub(crate) fn run_header_from_title(trait_id: &str, title: Option<String>) -> (String, String) {
    title
        .map(|title| (title, trait_id.to_string()))
        .unwrap_or_else(|| (trait_id.to_string(), String::new()))
}

pub(crate) fn failure_panel(
    product: &str,
    headline: &str,
    session_id: &str,
    error: &str,
) -> crate::app::presentation::Panel {
    use crate::app::presentation::{Panel, PanelRow, PanelStatus, RowTone};

    Panel::new(
        product,
        headline,
        PanelStatus::Blocked("Failure".to_string()),
    )
    .row(PanelRow::toned("session", session_id, RowTone::Default))
    .row(PanelRow::toned("error", error, RowTone::Fail))
}

/// Reads the resolved title, falling back to the narrator sidecar before the
/// ledger reaches a frame boundary.
pub(crate) fn persisted_session_title(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> Option<String> {
    session
        .provenance
        .session_title
        .as_ref()
        .and_then(ctx_traits_core::procedure::session::SessionTitleState::resolved_title)
        .map(str::to_string)
        .or_else(|| ctx_traits_io::activity_sidecar::read_session_title(ledger_path))
}

/// Maps one `StructuredOutput::compact_lines` line to a panel row without
/// dropping any of its content: line 0 is the count/verdict header, a
/// `"  receipt: <session>"` line becomes the `receipt` row, a `"  N. ..."`
/// item line becomes row `N`, and anything else (a future line shape)
/// survives as a `detail` row rather than being silently discarded.
fn compact_line_to_row(index: usize, line: &str) -> crate::app::presentation::PanelRow {
    use crate::app::presentation::{PanelRow, RowTone};

    if index == 0 {
        return PanelRow::toned("status", line, RowTone::Default);
    }
    let trimmed = line.trim_start();
    if let Some(receipt) = trimmed.strip_prefix("receipt: ") {
        return PanelRow::toned("receipt", receipt, RowTone::Default);
    }
    if let Some((number, rest)) = trimmed.split_once(". ")
        && number.chars().all(|ch| ch.is_ascii_digit())
    {
        return PanelRow::toned(number, rest, RowTone::Default);
    }
    PanelRow::toned("detail", trimmed, RowTone::Default)
}

pub(crate) fn handle_run_info(input: RunInfoInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let (query_tokens, json) = split_trailing_json_flag(input.query, input.json);
    if input.file.is_none() && input.trait_id.is_none() {
        let query = query_tokens.join(" ").trim().to_string();
        if query.is_empty() {
            return Err(crate::Error::Command {
                message:
                    "run-info requires a trait ID, --file <trait.toml>, or query text after --"
                        .to_string(),
            });
        }
        emit_run_info_outcome(
            ctx_traits_io::run::run_info(None, None, Some(&query))?,
            json,
        )?;
        return Ok(CommandOutput::new(()));
    }
    if !query_tokens.is_empty() {
        return Err(crate::Error::Command {
            message: "run-info query text is only accepted when no trait ID or --file is supplied"
                .to_string(),
        });
    }
    emit_run_info_outcome(
        ctx_traits_io::run::run_info(input.file, input.trait_id, None)?,
        json,
    )?;
    Ok(CommandOutput::new(()))
}

fn emit_run_info_outcome(
    outcome: ctx_traits_io::run::RunInfoOutcome,
    json: bool,
) -> crate::Result<()> {
    match outcome {
        ctx_traits_io::run::RunInfoOutcome::Summary {
            mut summary,
            roles,
            trait_context,
        } => {
            populate_run_info_dispatch_reminders(&mut summary, &roles, &trait_context);
            if json {
                print_json_report(&run_envelope(summary, false, false, false), "run info")?;
            } else {
                run_format::print_run_info(&summary);
            }
        }
        ctx_traits_io::run::RunInfoOutcome::Selection(output) => {
            if json {
                print_json_report(
                    &run_envelope(output, false, false, false),
                    "run info selection",
                )?;
            } else {
                run_format::print_run_selection("ctx traits internal run-info", &output.selection);
            }
        }
    }
    Ok(())
}

fn populate_run_info_dispatch_reminders(
    summary: &mut ctx_traits_core::run_info::RunInfoSummary,
    roles: &[String],
    trait_context: &(Box<ctx_traits_core::Trait>, camino::Utf8PathBuf),
) {
    // P451: resolve trait-aware so a variant-qualified `[agent.variant.*]`
    // table is reflected here, not just at actual dispatch time — otherwise
    // run-info would under-report a variant-qualified seat.
    let (trait_ref, trait_root) = trait_context;
    let profile = match ctx_traits_io::harness_config::resolve_trait_runtime_assignments(
        trait_ref,
        trait_root,
        &[],
    ) {
        Ok(profile) => profile,
        Err(error) => {
            summary.capabilities.push(CapabilityReport::unsupported(
                "runtime.dispatch-resolution",
                format!("dispatch resolution unavailable: {error}"),
            ));
            summary.capabilities.sort();
            summary.capabilities.dedup();
            return;
        }
    };
    for role in roles {
        // Every configured seat of a list-backed role (P456), not just its
        // first: a role-only lookup here would silently under-report the
        // rest of the list as unassigned. Configuration only — no
        // model-catalog probe, since run-info never resolved models before
        // this field existed and must not start doing so now.
        let seats = match profile.configured_seats_for_role(role) {
            Ok(seats) => seats,
            Err(error) => {
                summary.capabilities.push(CapabilityReport::unsupported(
                    format!("runtime.dispatch-resolution.{role}"),
                    format!("dispatch resolution unavailable for role {role:?}: {error}"),
                ));
                continue;
            }
        };
        if seats.is_empty() {
            summary
                .dispatch_reminders
                .push(ctx_traits_core::run_info::RunInfoDispatchReminder {
                    role: role.clone(),
                    harness: None,
                    transport: None,
                    session_mode: None,
                    assigned: false,
                    seat_index: None,
                    list_length: None,
                });
            continue;
        }
        for (assignment, seat_info) in seats {
            let (harness, transport, session_mode) =
                if assignment.mode == ctx_traits_io::harness_config::RunAssignmentMode::Attach {
                    (
                        Some("attach".to_string()),
                        Some("attach".to_string()),
                        Some("attach".to_string()),
                    )
                } else {
                    (
                        assignment.harness.clone(),
                        Some(
                            assignment
                                .transport
                                .unwrap_or(ctx_traits_io::harness_config::RunTransport::Cli)
                                .as_str()
                                .to_string(),
                        ),
                        Some(
                            assignment
                                .session_mode
                                .unwrap_or_default()
                                .as_str()
                                .to_string(),
                        ),
                    )
                };
            summary
                .dispatch_reminders
                .push(ctx_traits_core::run_info::RunInfoDispatchReminder {
                    role: role.clone(),
                    harness,
                    transport,
                    session_mode,
                    assigned: true,
                    seat_index: seat_info.map(|info| info.seat_index),
                    list_length: seat_info.map(|info| info.list_length),
                });
        }
    }
    summary
        .dispatch_reminders
        .sort_by(|left, right| (&left.role, left.seat_index).cmp(&(&right.role, right.seat_index)));
}

fn split_trailing_json_flag(tokens: &[String], json: bool) -> (Vec<String>, bool) {
    let mut out = tokens.to_vec();
    let mut json = json;
    if out.last().is_some_and(|token| token == "--json") {
        out.pop();
        json = true;
    }
    (out, json)
}

pub(crate) fn handle_call(input: CallInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let data_text = ctx_traits_io::read::read_text(camino::Utf8Path::new(input.data))?;
    reject_user_command_execution_payload(&data_text, input.data)?;
    let mut submission: ctx_traits_core::procedure::session::CallSubmission =
        serde_json::from_str(&data_text).map_err(|e| {
            crate::Error::json(format!("parse call submission JSON {}", input.data), e)
        })?;
    let caller = submission
        .caller
        .get_or_insert_with(ctx_traits_core::procedure::session::CallerProvenance::cli);
    if let Some(agent) = input.agent {
        caller.agent = Some(agent.to_string());
    }
    let outcome = ctx_traits_io::run::call(ctx_traits_io::run::CallRequest {
        trait_file: input.file,
        trait_id: None,
        session: input.session,
        session_store: input.session_store,
        submission,
        out: input.out,
        execution_dir: None,
        // IO restores and verifies any worktree recorded by the session before
        // advancing command frames. The env overlay is still host-empty here.
        execution_env: &std::collections::BTreeMap::new(),
        elapsed_seconds: None,
        tick_observer: None,
    })?;
    if input.json {
        let receipt_path = outcome.session_path.to_string();
        print_json_report(
            &run_envelope(
                CallReport::new(&outcome.response, &receipt_path),
                true,
                true,
                outcome.resource_supported,
            ),
            "call response",
        )?;
    } else {
        run_format::print_call_response(&outcome.response, Some(input.session));
    }
    if outcome.response.status == ctx_traits_core::procedure::session::Status::Failed {
        return Err(crate::Error::Command {
            message: "run session failed".to_string(),
        });
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_run_status(
    file: Option<&str>,
    session_path: &str,
    session_store: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let outcome = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: file,
        trait_id: None,
        session: session_path,
        session_store,
        elapsed_seconds: None,
    })?;
    if json {
        print_json_report(
            &run_envelope(outcome.session, false, false, outcome.resource_supported),
            "run status",
        )?;
    } else {
        run_format::print_run_session(&outcome.session, Some(session_path));
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_run_frame(
    file: Option<&str>,
    session_path: &str,
    session_store: Option<&str>,
    agent: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let outcome = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: file,
        trait_id: None,
        session: session_path,
        session_store,
        elapsed_seconds: None,
    })?;
    if let Some(agent) = agent {
        let agent = agent.strip_prefix("agent:").unwrap_or(agent);
        let current = outcome
            .session
            .current_agent
            .as_ref()
            .map(|role| role.role.as_str());
        if current != Some(agent) {
            let value = NotYourFrameReport {
                kind: "not-your-frame",
                agent,
                current_agent: current,
                status: &outcome.session.status,
                session_id: &outcome.session.session_id,
                run_id: &outcome.session.run_id,
            };
            if json {
                print_json_report(
                    &run_envelope(value, false, false, outcome.resource_supported),
                    "run frame",
                )?;
            } else {
                println!("ctx traits internal run-frame");
                println!("  kind: not-your-frame");
                println!("  agent: {agent}");
                println!("  current-agent: {}", current.unwrap_or("none"));
            }
            return Ok(CommandOutput::new(()));
        }
    }
    if json {
        print_json_report(
            &run_envelope(
                outcome.session.next_frame.clone(),
                false,
                false,
                outcome.resource_supported,
            ),
            "run frame",
        )?;
    } else if let Some(frame) = outcome.session.next_frame.as_deref() {
        run_format::print_sequence_frame("  ", frame);
    } else {
        println!("ctx traits internal run-frame");
        println!(
            "  status: {}",
            crate::app::presentation::wire_name(&outcome.session.status)
        );
        println!("  frame: none");
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_next(
    agent: Option<&str>,
    session: Option<&str>,
    session_store: Option<&str>,
    wait_seconds: u64,
    peek: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let mut output = ctx_traits_io::run_queue::next(ctx_traits_io::run_queue::NextRequest {
        agent,
        session,
        session_store,
        wait_seconds,
        peek,
    })?;
    if json {
        print_json_report(&run_envelope(output, false, true, false), "run next")?;
    } else {
        output
            .warnings
            .push(ctx_traits_core::launch::runtime_posture().0.message);
        println!("ctx traits internal next");
        println!(
            "  kind: {}",
            crate::app::presentation::wire_name(&output.kind)
        );
        println!("  agent: {}", output.agent);
        if let Some(frame) = output.frame.as_deref() {
            run_format::print_sequence_frame("  ", frame);
        }
        if let Some(terminal) = &output.terminal {
            println!(
                "  terminal-status: {}",
                crate::app::presentation::wire_name(&terminal.status)
            );
        }
        if !output.candidates.is_empty() {
            println!("  candidates: {}", output.candidates.len());
            for candidate in &output.candidates {
                println!(
                    "    #{} {} {} {}",
                    candidate.queue_position,
                    candidate.session_id,
                    crate::app::presentation::wire_name(&candidate.status),
                    candidate.path
                );
            }
        }
        for warning in &output.warnings {
            println!("  warning: {warning}");
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_set(input: SetInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let outcome = ctx_traits_io::run::set(ctx_traits_io::run::SetRequest {
        trait_file: input.file,
        trait_id: None,
        session: input.session,
        session_store: input.session_store,
        target: input.target,
        value: ctx_traits_io::run::parse_set_value(input.value, input.value_json)?,
        out: None,
        caller: ctx_traits_core::procedure::session::CallerProvenance::cli()
            .with_agent(input.agent.map(str::to_string)),
        existing_input_evidence: "existing run-session input",
        advance_command_frames: true,
    })?;
    match outcome {
        ctx_traits_io::run::SetOutcome::Session {
            session,
            resource_supported,
        } => {
            if input.json {
                print_json_report(
                    &run_envelope(session, true, true, resource_supported),
                    "set response",
                )?;
            } else {
                run_format::print_run_session(&session, Some(input.session));
            }
        }
        ctx_traits_io::run::SetOutcome::Call {
            response,
            resource_supported,
        } => {
            if input.json {
                print_json_report(
                    &run_envelope(response.clone(), true, true, resource_supported),
                    "set response",
                )?;
            } else {
                run_format::print_call_response(&response, Some(input.session));
            }
        }
    }
    Ok(CommandOutput::new(()))
}

fn reject_user_command_execution_payload(data_text: &str, path: &str) -> crate::Result<()> {
    let value: serde_json::Value = serde_json::from_str(data_text)
        .map_err(|e| crate::Error::json(format!("parse call submission JSON {path}"), e))?;
    let Some(object) = value.as_object() else {
        return Err(crate::Error::Command {
            message: format!("call submission JSON {path} must be an object"),
        });
    };
    if object.contains_key("command-execution") || object.contains_key("command_execution") {
        return Err(crate::Error::Command {
            message: "call submission JSON must not contain command-execution; the trusted local runtime executes the current command frame".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn run_envelope<T: serde::Serialize>(
    value: T,
    run_session_persistence: bool,
    call_payload: bool,
    declared_resource_evidence: bool,
) -> Envelope<T> {
    let mut envelope = Envelope::ok(value);
    for capability in ctx_traits_core::procedure::session::run_session_capability_reports(
        true,
        run_session_persistence,
        call_payload,
        declared_resource_evidence,
        true,
        true,
        false,
    ) {
        envelope = envelope.with_capability(capability);
    }
    let (warning, capability) = ctx_traits_core::launch::runtime_posture();
    envelope = envelope.with_warning(warning).with_capability(capability);
    envelope
}

#[cfg(test)]
mod completion_disposition_tests {
    use super::{
        CompletionDisposition, PropagatedDriveError, disposition_for_merge_status,
        disposition_for_report_status, failure_reason_from_values, human_terminal_failure_values,
        merged_fact_from_terminal, propagated_drive_error_abort,
        resolve_propagated_drive_error_choice, restart_out, restart_worktree, run_completed_status,
        run_header_from_title, short_failure_line,
    };
    use ctx_traits_core::procedure::session::{MergeStatus, Status};

    #[test]
    fn run_header_uses_title_or_trait_only_fallback() {
        assert_eq!(
            run_header_from_title("demo-variant", Some("Demo run".to_string())),
            ("Demo run".to_string(), "demo-variant".to_string())
        );
        assert_eq!(
            run_header_from_title("demo-variant", None),
            ("demo-variant".to_string(), String::new())
        );
    }

    #[test]
    fn failure_reason_uses_documented_precedence() {
        assert_eq!(
            failure_reason_from_values(
                Some("stop message"),
                Some("stop reason"),
                Some("bound"),
                Some("warning"),
                Some("status"),
            ),
            Some("stop message".to_string())
        );
        for (message, reason, bound, warning, status, expected) in [
            (
                None,
                Some("stop reason"),
                Some("bound"),
                Some("warning"),
                Some("status"),
                "stop reason",
            ),
            (
                None,
                None,
                Some("bound"),
                Some("warning"),
                Some("status"),
                "bound",
            ),
            (None, None, None, Some("warning"), Some("status"), "warning"),
            (None, None, None, None, Some("status"), "status"),
        ] {
            assert_eq!(
                failure_reason_from_values(message, reason, bound, warning, status),
                Some(expected.to_string())
            );
        }
    }

    #[test]
    fn completion_requires_both_session_and_drive_to_complete() {
        assert!(run_completed_status(
            Status::Completed,
            Some(Status::Completed)
        ));
        assert!(!run_completed_status(
            Status::Failed,
            Some(Status::Completed)
        ));
        assert!(!run_completed_status(
            Status::Completed,
            Some(Status::Blocked)
        ));
        assert!(!run_completed_status(Status::Completed, None));
    }

    #[test]
    fn short_failure_line_is_sanitized_single_line_and_bounded() {
        assert_eq!(short_failure_line(None), "run failed");
        assert_eq!(short_failure_line(Some("\n\t\x1b[31m")), "run failed");
        assert_eq!(
            short_failure_line(Some("\x1b[31m bad\t drive\nignored")),
            "bad drive"
        );
        assert_eq!(short_failure_line(Some("left\u{202e}right")), "left right");
        let long = "x".repeat(80);
        let line = short_failure_line(Some(&long));
        assert_eq!(crate::app::tui::display_width(&line), 64);
        assert!(line.ends_with("..."));
        let wide = "界".repeat(40);
        assert_eq!(
            crate::app::tui::display_width(&short_failure_line(Some(&wide))),
            63
        );
    }

    #[test]
    fn restart_boundaries_preserve_prior_ledgers() {
        assert_eq!(restart_out(Some("prior-ledger.json")), None);
        assert_eq!(restart_out(None), None);
        assert_eq!(restart_worktree(Some(Some("named"))), Some(None));
        assert_eq!(restart_worktree(Some(None)), Some(None));
        assert_eq!(restart_worktree(None), None);
    }

    #[test]
    fn terminal_failure_excludes_non_terminal_presentation_cases() {
        assert!(!human_terminal_failure_values(
            false,
            false,
            false,
            false,
            false,
            Status::Failed,
            Some(Status::Failed),
        ));
        assert!(!human_terminal_failure_values(
            true,
            true,
            false,
            false,
            false,
            Status::Failed,
            Some(Status::Failed),
        ));
        assert!(!human_terminal_failure_values(
            true,
            false,
            true,
            false,
            false,
            Status::Failed,
            Some(Status::Failed),
        ));
        assert!(!human_terminal_failure_values(
            true,
            false,
            false,
            false,
            false,
            Status::Completed,
            Some(Status::Completed),
        ));
        assert!(human_terminal_failure_values(
            true,
            false,
            false,
            false,
            false,
            Status::Failed,
            Some(Status::Failed),
        ));
    }

    #[test]
    fn terminal_failure_exempts_a_recorded_summons_park_but_not_a_failed_write() {
        // The outcome-write marker succeeded: `report.status == "awaiting-owner"`
        // exempts the run, even though the raw session status also reads
        // `WaitingOnHuman` (P0253.4).
        assert!(!human_terminal_failure_values(
            true,
            false,
            false,
            true,
            false,
            Status::WaitingOnHuman,
            Some(Status::WaitingOnHuman),
        ));
        // The outcome-write marker failed: `drive.rs` downgrades
        // `report.status` away from `"awaiting-owner"`, but the session's raw
        // status can still read `WaitingOnHuman` since frame readiness is
        // independent of the marker write. This must NOT be exempted — an
        // unpersisted summons is a failed run, not a resumable park.
        assert!(human_terminal_failure_values(
            true,
            false,
            false,
            false,
            false,
            Status::WaitingOnHuman,
            Some(Status::WaitingOnHuman),
        ));
    }

    #[test]
    fn terminal_failure_exempts_a_recorded_disk_full_park() {
        assert!(!human_terminal_failure_values(
            true,
            false,
            false,
            true,
            false,
            Status::AwaitingAgentOutput,
            Some(Status::AwaitingAgentOutput),
        ));
        assert!(human_terminal_failure_values(
            true,
            false,
            false,
            false,
            false,
            Status::AwaitingAgentOutput,
            Some(Status::AwaitingAgentOutput),
        ));
    }

    #[test]
    fn propagated_drive_error_resume_reseeds_the_retained_panel() {
        let handoff = crate::app::drive::PanelHandoff::new();

        let panel = crate::app::run_view::tests::detached_panel_for_test();
        panel.clear_failure_modal();
        handoff.give(panel);

        assert!(
            handoff.take().is_some(),
            "Resume must seed the real handoff"
        );
    }

    #[test]
    fn propagated_drive_error_restart_closes_before_a_fresh_session() {
        let handoff = crate::app::drive::PanelHandoff::new();
        let panel = crate::app::run_view::tests::detached_panel_for_test();
        let observed = panel.clone();
        let mut attempt = "failed-session".to_string();

        resolve_propagated_drive_error_choice(
            crate::app::run_view::FailureChoice::Restart,
            panel,
            &handoff,
            &mut attempt,
            || {
                assert!(observed.was_closed_for_test());
                Ok("fresh-session".to_string())
            },
            |_| unreachable!("Restart must not report an Abort panel"),
        )
        .unwrap();

        assert!(observed.was_closed_for_test());
        assert_eq!(
            attempt, "fresh-session",
            "Restart must replace the failed attempt with the fresh startup result"
        );
    }

    #[test]
    fn propagated_drive_error_abort_closes_before_reporting_exit_one() {
        let panel = crate::app::run_view::tests::detached_panel_for_test();
        let observed = panel.clone();
        let report = std::sync::Arc::new(std::sync::Mutex::new(None));
        let reported = std::sync::Arc::clone(&report);

        let error = propagated_drive_error_abort(
            panel,
            PropagatedDriveError {
                trait_id: "trait",
                session_id: "session",
                line: "drive failed",
            },
            move |failure| {
                assert!(observed.was_closed_for_test());
                *reported.lock().unwrap() = Some(failure.styled_lines());
                Ok(())
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            crate::Error::AlreadyReported { exit_code: 1, .. }
        ));
        let report = report
            .lock()
            .unwrap()
            .take()
            .expect("failure panel emitted");
        assert!(
            report
                .iter()
                .flat_map(|line| line.segments())
                .any(|(text, _)| text.contains("drive failed"))
        );
    }

    #[test]
    fn merged_facts_preserve_terminal_landing_truth() {
        assert_eq!(
            merged_fact_from_terminal(Some((MergeStatus::Merged, Some("abc123"))), None, None),
            Some("yes (abc123)".to_string())
        );
        assert_eq!(
            merged_fact_from_terminal(None, Some("ctx traits merge run-1".to_string()), None),
            Some("no (ctx traits merge run-1)".to_string())
        );
        for status in [MergeStatus::Parked, MergeStatus::RecoveryFailure] {
            assert_eq!(
                merged_fact_from_terminal(
                    Some((status, None)),
                    None,
                    Some("ctx traits merge run-1".to_string())
                ),
                Some("no (ctx traits merge run-1)".to_string())
            );
        }
        assert_eq!(
            merged_fact_from_terminal(
                Some((MergeStatus::PostMergeCleanupFailure, Some("abc123"))),
                None,
                None
            ),
            Some("yes (abc123)".to_string())
        );
    }

    #[test]
    fn report_status_merged_and_parked_map_distinctly() {
        assert_eq!(
            disposition_for_report_status("merged"),
            CompletionDisposition::Merged
        );
        assert_eq!(
            disposition_for_report_status("parked"),
            CompletionDisposition::Parked
        );
    }

    /// Every non-park, non-merged terminal `MergeReport::status` — lock
    /// contention/timeout and post-fast-forward cleanup/recovery failure —
    /// must map to `Failed`, never `Parked`: only an actual park promises
    /// the branch and worktree were left intact.
    #[test]
    fn report_status_non_park_failures_never_map_to_parked() {
        for status in [
            "lock-unavailable",
            "lock-timeout",
            "post-merge-cleanup-failure",
            "recovery-failure",
        ] {
            assert_eq!(
                disposition_for_report_status(status),
                CompletionDisposition::Failed,
                "status {status:?} must not map to Parked"
            );
        }
    }

    /// Mirrors the report-status proof above for the prior-terminal-frame
    /// lookup path (a resume over an already-decided session): a persisted
    /// `PostMergeCleanupFailure`/`RecoveryFailure` frame must never be
    /// reported as a park either.
    #[test]
    fn merge_status_non_park_terminal_failures_never_map_to_parked() {
        for status in [
            MergeStatus::PostMergeCleanupFailure,
            MergeStatus::RecoveryFailure,
        ] {
            assert_eq!(
                disposition_for_merge_status(status),
                CompletionDisposition::Failed,
                "status {status:?} must not map to Parked"
            );
        }
    }

    #[test]
    fn merge_status_merged_and_parked_map_distinctly() {
        assert_eq!(
            disposition_for_merge_status(MergeStatus::Merged),
            CompletionDisposition::Merged
        );
        assert_eq!(
            disposition_for_merge_status(MergeStatus::Parked),
            CompletionDisposition::Parked
        );
    }
}
