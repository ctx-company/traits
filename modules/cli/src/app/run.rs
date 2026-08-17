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

/// Bounded `ctx traits call --json` projection (P421): the fields an agent
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
/// names `ctx traits config accept`. Read-only commands (`check`, `doctor`,
/// `config build`, `config accept` itself) never call this. Non-TTY
/// contexts always refuse — acceptance is never automatic.
///
/// Scoped to the `.ts` example only, NOT the pre-existing `runtime.example.
/// toml` (0037): that convention predates this gate and every repo/fixture
/// that already relies on it (this repo included) would trip an unaccepted-
/// example refusal on every run with no migration path. `ctx traits config
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
            message: "this repo's runtime.example.ts/.toml has not been accepted — run `ctx traits config accept`".to_string(),
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
                "declined — run `ctx traits config accept` when ready to accept {example_path}"
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
    let startup = input.startup;
    let startup_observer = startup.as_ref().map(|view| view.observer());
    let outcome = match start_run_session(
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
            startup_observer,
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
    let session_path = outcome.session_path.as_ref().map(|path| path.to_string());
    let session_arg = session_path
        .clone()
        .unwrap_or_else(|| outcome.session.session_id.as_str().to_string());
    // `run::start` already prepared the worktree (if requested); pass its
    // execution directory straight through instead of re-resolving it.
    // P549: a handoff installed unconditionally — `drive_loop` only ever
    // releases a panel into it at its one normal `Status::Completed` exit,
    // and only when a panel exists at all (`--progress tui` on a real
    // terminal); every other outcome leaves it empty, so
    // `merge_live_for_completion` below falls back to the plain stage-line
    // sink exactly as it does for a caller that never installs a handoff.
    let panel_handoff = crate::app::drive::PanelHandoff::new();
    let drive = crate::app::drive::drive(crate::app::drive::DriveInputs {
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
        panel_handoff: Some(panel_handoff.clone()),
        startup,
        frame_observer: None,
    })?;

    // outcome.session is the pre-drive snapshot; re-inspect for the completed
    // state so both the JSON envelope and the plain-text final output reflect
    // what actually landed, not the pre-drive placeholder.
    let final_session = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: input.file,
        trait_id: None,
        session: &session_arg,
        session_store: input.session_store,
        elapsed_seconds: None,
    })
    .map(|inspected| inspected.session)
    .unwrap_or(outcome.session);
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
    let (merge_live, merger_stdout_observer, merge_span_guard) = merge_live_for_completion(
        panel_handoff.take(),
        final_session.run_id.as_str(),
        final_session.session_id.as_str(),
        &assignment_overrides,
    );
    let completion = complete_after_drive(
        input.session_store,
        &merge_session_path,
        &assignment_overrides,
        final_session,
        merge_live,
        merger_stdout_observer,
    )?;
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
        // A landed auto-merge folds into the run panel as its final state
        // (`landed` + one landing row) instead of printing a second panel —
        // one run, one state. The separate merge panel remains for every
        // non-landed outcome (parked/failed carry reason + next-action rows
        // the user must see) and under `--verbose`.
        let landed = completion
            .merge
            .as_ref()
            .is_some_and(|report| report.status == "merged");
        print_final_output(
            &completion.session,
            &drive,
            loaded_trait.as_ref().map(|loaded| &loaded.trait_ref),
            input.verbose,
            landed,
        )?;
        if let Some(report) = &completion.merge
            && (input.verbose || !landed)
        {
            crate::app::merge::print_report(report)?;
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
/// per-task outcome table renders once the queue finishes or halts.
#[derive(Debug, Clone)]
pub(crate) enum TaskQueueOutcome {
    Landed { closed: bool },
    Completed,
    NotMerged,
    Parked,
    MergeFailed,
    Failed { message: String },
}

impl TaskQueueOutcome {
    /// A failed run or a parked/failed merge halts the queue by default
    /// (owner ruling 2026-08-17) — `--continue-on-failure` is the only
    /// thing that lets the queue run past one of these.
    fn halts(&self) -> bool {
        matches!(
            self,
            TaskQueueOutcome::Parked
                | TaskQueueOutcome::MergeFailed
                | TaskQueueOutcome::Failed { .. }
        )
    }

    fn label(&self) -> String {
        match self {
            TaskQueueOutcome::Landed { closed: true } => "landed, closed".to_string(),
            TaskQueueOutcome::Landed { closed: false } => "landed".to_string(),
            TaskQueueOutcome::Completed => "completed (no merge intent)".to_string(),
            TaskQueueOutcome::NotMerged => "committed, not merged".to_string(),
            TaskQueueOutcome::Parked => "parked".to_string(),
            TaskQueueOutcome::MergeFailed => "merge failed".to_string(),
            TaskQueueOutcome::Failed { message } => format!("failed: {message}"),
        }
    }
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
            startup: None,
        };
        match drive_session(session_inputs) {
            Ok(completion) => {
                let session_failed = completion.session.status
                    == ctx_traits_core::procedure::session::Status::Failed;
                if session_failed {
                    TaskQueueOutcome::Failed {
                        message: "run completed with status failed".to_string(),
                    }
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

    if !input.json {
        print_task_queue_report(&outcomes, halted);
    }

    let any_halting = outcomes.iter().any(|(_, outcome)| outcome.halts());
    if any_halting {
        return Err(crate::Error::AlreadyReported {
            message: if halted {
                "task queue halted".to_string()
            } else {
                "task queue completed with failures".to_string()
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
}

fn print_task_queue_report(outcomes: &[(String, TaskQueueOutcome)], halted: bool) {
    println!("task queue:");
    for (key, outcome) in outcomes {
        println!("  {key}: {}", outcome.label());
    }
    if halted {
        println!("halted — pass --continue-on-failure to run the remaining queue anyway");
    }
}

/// P550 run-termination story hook: interactive-TTY-only pane, plain-text
/// story otherwise. Never called under `--json` (the `json` branch above
/// returns before reaching here) — `--json` output stays byte-identical to
/// today, the story is already available separately via `ctx traits story
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
    let worktree = session.provenance.worktree.as_ref()?;
    Some(NotMergedFact {
        branch: worktree.branch.clone(),
        merge_command: format!("ctx traits merge {}", session.run_id.as_str()),
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
    disposition: CompletionDisposition,
}

impl CompletionOutcome {
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
                if self.session.status == ctx_traits_core::procedure::session::Status::Failed {
                    let reason = self
                        .session
                        .stop_reason
                        .as_ref()
                        .and_then(|stop| stop.message.clone().or_else(|| Some(stop.reason.clone())))
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default();
                    return Err(crate::Error::AlreadyReported {
                        message: format!("run {run_id:?} failed{reason}"),
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
            disposition: CompletionDisposition::NoIntent,
        });
    };
    if final_session.status != ctx_traits_core::procedure::session::Status::Completed {
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
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
        // `merge_intent` history stays readable via `ctx traits session
        // state`/`inspect` regardless.
        return Ok(CompletionOutcome {
            session: final_session,
            merge: None,
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
        disposition,
    })
}

fn print_final_output(
    session: &ctx_traits_core::procedure::session::Session,
    drive: &crate::app::drive::DriveReport,
    trait_ref: Option<&ctx_traits_core::Trait>,
    verbose: bool,
    landed: bool,
) -> crate::Result<()> {
    use crate::app::presentation::{
        HumanOutputMode, Panel, PanelRow, PanelSection, RowTone, emit_human,
    };

    // P427: surface an automatic built-in harness selection even in the
    // default (non-`--verbose`) plain output — `--verbose` already prints
    // every warning via `print_report`, so only print these here to avoid a
    // duplicate line.
    if !verbose {
        for warning in &drive.warnings {
            if warning.starts_with("automatic harness selection: ") {
                println!("{warning}");
            }
        }
    }
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

    let mode = if verbose {
        HumanOutputMode::Verbose
    } else {
        HumanOutputMode::Compact
    };

    // A completed run whose auto-merge landed reports ONE terminal state:
    // `landed` subsumes `completed` (a run cannot land without completing),
    // and the landing revision becomes a row of this panel rather than a
    // second panel's worth of output.
    let status = if landed {
        crate::app::presentation::PanelStatus::Passed("landed".to_string())
    } else {
        drive.panel_status()
    };
    let mut panel = Panel::new("ctx", "run", status);
    if landed {
        use ctx_traits_core::procedure::session::{LandingState, landing_state};
        let landing = match landing_state(session) {
            Some(LandingState::Landed {
                revision: Some(revision),
            }) => format!("merged to main ({revision})"),
            _ => "merged to main".to_string(),
        };
        panel = panel.row(PanelRow::toned("landing", landing, RowTone::Default));
    }
    match &session.completion {
        Some(completion) if !completion.final_outputs.is_empty() => {
            for output in &completion.final_outputs {
                if let Some(rendered) = trait_ref.and_then(|trait_ref| {
                    structured_output::resolve(trait_ref, output.port_ref.id(), &output.value)
                }) {
                    let verdict = structured_output::producer_verdict_for_output(session, output);
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
    if mode == HumanOutputMode::Compact {
        panel = panel.next(PanelRow::toned(
            "next",
            "ctx traits run --verbose for the full output detail",
            RowTone::Default,
        ));
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
                run_format::print_run_selection("ctx traits run-info", &output.selection);
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
                println!("ctx traits run-frame");
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
        println!("ctx traits run-frame");
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
        println!("ctx traits next");
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
        CompletionDisposition, disposition_for_merge_status, disposition_for_report_status,
    };
    use ctx_traits_core::procedure::session::MergeStatus;

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
