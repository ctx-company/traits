//! Generate, critique, and evaluation-generation command handlers.

use crate::app::entry::build_file_evidence_from_io;
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::surface::cli;
use ctx_traits_core::response::CommandOutput;

/// The `generate-trait` meta-trait's terminal output: the round-evidence
/// envelope its `derive-envelope` step assembles (task 0066.1/0066.2).
/// Deserializes `run_builtin_trait`'s single JSON output string.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct GenerateEnvelope {
    pub(crate) converged: bool,
    pub(crate) rounds_spent: u32,
    /// Absent only when a stale repo-local shadow package (built before
    /// 0066.2) supplied this envelope — degrades gracefully rather than
    /// failing decode.
    #[serde(default)]
    pub(crate) rounds_bound: Option<u32>,
    #[serde(default)]
    pub(crate) failing_rung: Option<String>,
    #[serde(default)]
    pub(crate) diagnostics: Vec<ctx_traits_core::assist::Diagnostic>,
    pub(crate) candidate_source: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct RefineAssistContext<'a> {
    pub(crate) change_request: &'a str,
    pub(crate) trait_id: String,
    pub(crate) source_digest: String,
    pub(crate) target_schema: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct RefineEvidenceReport {
    pub(crate) source_trait_id: String,
    pub(crate) source_path: String,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) candidate_trait_id: Option<String>,
    pub(crate) candidate_canonical_digest: Option<ctx_traits_core::digest::Digest>,
    pub(crate) canonical_text_differs: bool,
    pub(crate) target_path: String,
    pub(crate) apply: bool,
}

pub(crate) struct GenerateInputs<'a> {
    pub(crate) name: &'a str,
    pub(crate) brief: &'a str,
    pub(crate) model: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) out: Option<&'a str>,
    pub(crate) candidate_path: Option<&'a str>,
    pub(crate) check: bool,
    pub(crate) json: bool,
}

/// Live per-round progress for `handle_generate`'s guarded-loop drive (task
/// 0066.2): plain, minimal stderr lines in both human and `--json` modes —
/// stderr never corrupts the stdout JSON envelope. Tracks its own round
/// count from `evaluate-round` acceptances rather than trusting the
/// meta-trait's `rounds-spent` counter slot, which this observer never
/// reads.
pub(crate) struct RoundProgressObserver {
    round: std::cell::Cell<u32>,
}

impl RoundProgressObserver {
    pub(crate) fn new() -> Self {
        Self {
            round: std::cell::Cell::new(0),
        }
    }

    pub(crate) fn observe(&self, event: crate::app::drive::FrameObserverEvent<'_>) {
        match event {
            crate::app::drive::FrameObserverEvent::Dispatched(dispatch) => {
                self.observe_dispatch(dispatch);
            }
            crate::app::drive::FrameObserverEvent::Accepted(accepted) => {
                self.observe_accepted(accepted);
            }
        }
    }

    fn observe_dispatch(&self, dispatch: crate::app::drive::FrameDispatchEvent<'_>) {
        match dispatch.item_id {
            Some("produce-first") => {
                eprintln!(
                    "{}",
                    crate::app::tui::stderr_line(
                        "round 1 · drafting candidate",
                        crate::app::tui::Tone::Default
                    )
                );
            }
            Some("revise") => {
                let next_round = self.round.get() + 1;
                eprintln!(
                    "{}",
                    crate::app::tui::stderr_line(
                        &format!("round {next_round} · revising candidate"),
                        crate::app::tui::Tone::Default,
                    )
                );
            }
            _ => {}
        }
    }

    fn observe_accepted(&self, accepted: crate::app::drive::FrameAcceptedEvent<'_>) {
        if accepted.slot_ref != ROUND_REPORT_SLOT_REF {
            return;
        }
        let Ok(report) =
            serde_json::from_value::<ctx_traits_core::assist::RoundReport>(accepted.value.clone())
        else {
            return;
        };
        let round = self.round.get() + 1;
        self.round.set(round);
        if report.converged {
            eprintln!(
                "{}",
                crate::app::tui::stderr_line(
                    &format!("round {round} · converged"),
                    crate::app::tui::Tone::Pass,
                )
            );
            return;
        }
        let top_diagnostic = report
            .diagnostics
            .first()
            .map_or("no diagnostic recorded", |diagnostic| {
                diagnostic.message.as_str()
            });
        eprintln!(
            "{}",
            crate::app::tui::stderr_line(
                &format!(
                    "round {round} · failed at {} — {top_diagnostic}",
                    report.rung
                ),
                crate::app::tui::Tone::Fail,
            )
        );
    }
}

pub(crate) fn handle_generate(input: GenerateInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let GenerateInputs {
        name,
        brief,
        model,
        assignments,
        out,
        candidate_path,
        check,
        json,
    } = input;
    let trait_id = ctx_traits_core::synth::slugify_trait_id(name)?;

    // `--candidate` evaluates the supplied authoring source through exactly
    // one round of the rung ladder: no meta-trait run, no provider call, by
    // construction (this branch never reaches `run_builtin_trait`). This is
    // the path gate tests exercise (task 0066.1). The evidence surface is
    // uniform whether the loop ran once or to its bound (task 0066.2).
    if let Some(cand_path) = candidate_path {
        let candidate_source = ctx_traits_io::read::read_text(camino::Utf8Path::new(cand_path))?;
        let package_root = crate::app::assist_round::scratch_package_root(&trait_id);
        let report =
            crate::app::assist_round::evaluate_round(&candidate_source, &package_root, &trait_id)?;
        let evidence = ctx_traits_core::assist::RoundEvidence {
            converged: report.converged,
            rounds_spent: 1,
            rounds_bound: None,
            failing_rung: (!report.converged).then_some(report.rung),
            rounds: vec![ctx_traits_core::assist::RoundRecord {
                round: 1,
                rung: report.rung,
                converged: report.converged,
                diagnostics: report.diagnostics.clone(),
            }],
        };
        print_round_evidence(&evidence, &package_root, json)?;
        if !report.converged {
            return Err(crate::Error::Command {
                message: format!(
                    "generate --candidate failed at rung {}: candidate not written; scratch preserved at {package_root}",
                    report.rung
                ),
            });
        }
        return Ok(CommandOutput::new(()));
    }

    // The default path (and `--check`) drive the declared bounded loop in
    // `generate-trait`: the meta-trait iterates on its own typed rung
    // diagnostics, and this handler writes only a converged result. No
    // retry logic lives here — that is the meta-trait's job (task 0066.1).
    // A live observer reports each round as it happens instead of blocking
    // silently until the verdict (task 0066.2); it takes over drive's own
    // Status progress lines for this call (see `run_builtin_trait_observed`).
    let scratch_root = crate::app::assist_round::scratch_package_root(&trait_id);
    let round_progress = RoundProgressObserver::new();
    let observer: crate::app::drive::FrameObserver<'_> = &|event| round_progress.observe(event);
    let outcome = match run_builtin_trait_observed(
        "generate-trait",
        vec![
            runtime_input("name", name),
            runtime_input("brief", brief),
            runtime_input("trait-id", trait_id.clone()),
        ],
        assignments,
        model,
        None,
        Some(observer),
    )? {
        BuiltinTraitRun::Completed(outcome) => outcome,
        BuiltinTraitRun::Killed(killed) => {
            return Err(report_bound_kill(&killed, &scratch_root, "generate", json));
        }
    };
    let envelope: GenerateEnvelope = serde_json::from_str(&outcome.output)
        .map_err(|error| crate::Error::json("decode generate loop envelope", error))?;

    let evidence = round_evidence_from_envelope(&envelope, outcome.round_records);
    print_round_evidence(&evidence, &scratch_root, json)?;

    if !envelope.converged {
        return Err(crate::Error::Command {
            message: format!(
                "generate failed at rung {}: no package written; scratch preserved at {scratch_root}; {} round(s) spent",
                envelope.failing_rung.as_deref().unwrap_or("unknown"),
                envelope.rounds_spent
            ),
        });
    }

    if check {
        return Ok(CommandOutput::new(()));
    }

    let (package_root, _output_path) = trait_package_output_paths(&trait_id, out);
    let source_target =
        ctx_traits_io::layout::package_source_write_path(camino::Utf8Path::new(&package_root));
    ctx_traits_io::write::write_candidate(ctx_traits_io::write::CandidateWriteRequest {
        target_path: &source_target,
        trait_id: &trait_id,
        content: &envelope.candidate_source,
        mode: ctx_traits_io::write::CandidateWriteMode::NewCandidateSource,
    })?;
    // Only `build` writes `generated/` + `trait.lock` (the 0065 invariant);
    // the loop's own evaluate rung never mutates the real package.
    crate::app::schema_synth_build::handle_build(
        source_target.as_str(),
        "toml",
        None,
        false,
        false,
    )?;

    Ok(CommandOutput::new(()))
}

pub(crate) struct CritiqueInputs<'a> {
    pub(crate) file: &'a str,
    pub(crate) source_map: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) candidate_path: Option<&'a str>,
    pub(crate) json: bool,
}

pub(crate) fn handle_critique(input: CritiqueInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let trait_path = camino::Utf8Path::new(input.file);
    let map_path = match input.source_map {
        Some(path) => camino::Utf8Path::new(path).to_path_buf(),
        None => crate::app::cdk_build::package_source_map(trait_path)?,
    };
    let source_map: ctx_traits_core::source_map::SourceMap =
        serde_json::from_str(&ctx_traits_io::read::read_text(&map_path)?)
            .map_err(|error| crate::Error::json(format!("decode source map {map_path}"), error))?;
    ctx_traits_core::source_map::validate_source_map(&source_map)?;
    let source_map = crate::app::cdk_build::rebase_source_map(
        source_map,
        &crate::app::cdk_build::stable_repo_root(trait_path)?,
    );

    let (source_trait, _, source_digest, _) = ctx_traits_io::run::load_trait(input.file)?;
    let source_text = ctx_traits_io::read::read_text(trait_path)?;
    let source_trait_id = source_trait.id.as_str().to_string();
    let candidate =
        ctx_traits_core::assist::plan_assist_boundary(ctx_traits_core::assist::BoundaryRequest {
            operation: ctx_traits_core::assist::Operation::Critique,
            source_trait_ids: vec![source_trait_id.clone()],
            source_paths: vec![input.file.to_string()],
            source_digests: vec![source_digest.clone()],
            user_request: "source-backed advisory design critique".to_string(),
            model: input.model.map(str::to_string),
            target_path: "advisory-no-write-target".to_string(),
            provider_available: input.candidate_path.is_none(),
            context: serde_json::json!({
                "trait-id": source_trait_id,
                "source-digest": source_digest,
                "source-map": source_map,
                "target-schema": "agent-traits/review-scaffold"
            }),
        })?;
    let raw = match input.candidate_path {
        Some(path) => ctx_traits_io::read::read_text(camino::Utf8Path::new(path))?,
        None => match run_builtin_trait(
            "critique-trait",
            vec![
                runtime_input("source", source_text),
                runtime_input("source-digest", source_digest.as_str()),
                runtime_input("source-path", input.file),
                runtime_input(
                    "source-map",
                    serde_json::to_string(&source_map).map_err(|error| {
                        crate::Error::json("serialize critique source map", error)
                    })?,
                ),
            ],
            input.assignments,
            input.model,
            None,
        ) {
            Ok(outcome) => outcome.output,
            Err(error) => {
                return blocked_assist_candidate(candidate, input.json, error.to_string());
            }
        },
    };
    let evaluation =
        ctx_traits_core::assist::evaluate_supplied_review_scaffold(candidate, &raw, &source_map);
    let mut candidate = evaluation.candidate;
    let scaffold = match evaluation.scaffold {
        Some(scaffold) => scaffold,
        None => {
            return blocked_assist_candidate(
                candidate,
                input.json,
                evaluation
                    .error
                    .unwrap_or_else(|| "review scaffold was rejected".to_string()),
            );
        }
    };
    if scaffold.source_trait_id != source_trait.id.as_str()
        || scaffold.source_digest != source_digest.as_str()
    {
        return blocked_assist_candidate(
            candidate,
            input.json,
            "critique scaffold identity or source digest does not match the reviewed trait"
                .to_string(),
        );
    }
    let eval_reports: Vec<String> = Vec::new();
    let check_report =
        crate::app::report_check::build_check_report(&crate::app::report_check::CheckInputs {
            file: input.file,
            locked: false,
            skip_cdk_drift: false,
            json: false,
            plain: true,
            no_animate: true,
            verbose: false,
            run_ledger: None,
            eval_reports: &eval_reports,
        })?;
    candidate = ctx_traits_core::assist::attach_check_report(candidate, check_report);
    if candidate.gate_summary.audit.ok {
        candidate = ctx_traits_core::assist::with_context_evidence(
            candidate,
            serde_json::json!({ "review-scaffold": scaffold }),
        );
    }
    print_assist_candidate(&candidate, input.json)?;
    if candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked {
        return Err(crate::Error::Command {
            message: "critique failed: candidate was blocked by validation gates".to_string(),
        });
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_generate_round(
    trait_id: &str,
    candidate: &str,
) -> crate::Result<CommandOutput<()>> {
    let package_root = crate::app::assist_round::scratch_package_root(trait_id);
    let report = crate::app::assist_round::evaluate_round(candidate, &package_root, trait_id)?;
    print_round_report(&report, &package_root, true)?;
    Ok(CommandOutput::new(()))
}

pub(crate) fn runtime_input(
    id: &str,
    value: impl Into<serde_json::Value>,
) -> ctx_traits_core::procedure::runtime::StepSlotOutput {
    ctx_traits_core::procedure::runtime::StepSlotOutput {
        ref_text: format!("port:{id}"),
        value: value.into(),
        source: None,
        producer_evidence: Some("native built-in trait runner input".to_string()),
        command_execution: None,
        producer_agent: None,
        producer_harness: None,
    }
}

/// [`run_builtin_trait`]'s result: the trait's single JSON output string,
/// plus (additive, ignored by every caller but `handle_generate`) the
/// session's `slot:round-report` acceptance history decoded in round order —
/// empty for every built-in trait that never writes that slot.
pub(crate) struct BuiltinTraitOutcome {
    pub(crate) output: String,
    pub(crate) round_records: Vec<ctx_traits_core::assist::RoundRecord>,
}

/// A drive that ended without completing (0066.4): a run bound killed it
/// before the meta-trait's own loop reached a verdict. Carries whatever
/// round evidence the session accumulated before the kill, plus the named
/// bound so the CLI join point can report it rather than a generic timeout.
pub(crate) struct BuiltinTraitKilled {
    pub(crate) status: String,
    pub(crate) bound_fired: Option<String>,
    pub(crate) round_records: Vec<ctx_traits_core::assist::RoundRecord>,
}

/// [`run_builtin_trait_observed`]'s typed result: either the drive completed
/// with a terminal output, or a run bound killed it first. Replaces a flat
/// `Error::Command` on non-completion so a loop join point can fold a kill
/// into round evidence instead of losing the round history (0066.4).
pub(crate) enum BuiltinTraitRun {
    Completed(BuiltinTraitOutcome),
    Killed(BuiltinTraitKilled),
}

const ROUND_REPORT_SLOT_REF: &str = "slot:round-report";

/// Decode the session's accepted `slot:round-report` revisions, in
/// acceptance order, into 1-based `RoundRecord`s. The revision payload's
/// shape is exactly `RoundReport` (task 0066.1's meta-trait schema mirrors
/// it field-for-field), so no re-parsing beyond a direct deserialize.
fn round_records_from_session(
    session: &ctx_traits_core::procedure::session::Session,
) -> crate::Result<Vec<ctx_traits_core::assist::RoundRecord>> {
    let mut records = Vec::new();
    for revision in &session.slot_revisions {
        if revision.slot_ref.as_str() != ROUND_REPORT_SLOT_REF {
            continue;
        }
        let Some(payload) = revision.submitted_payload.as_ref() else {
            continue;
        };
        let report: ctx_traits_core::assist::RoundReport =
            serde_json::from_value(payload.value.clone())
                .map_err(|error| crate::Error::json("decode round-report revision", error))?;
        records.push(ctx_traits_core::assist::RoundRecord {
            round: u32::try_from(records.len() + 1).unwrap_or(u32::MAX),
            rung: report.rung,
            converged: report.converged,
            diagnostics: report.diagnostics,
        });
    }
    Ok(records)
}

pub(crate) fn run_builtin_trait(
    trait_id: &str,
    input_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
    assignments: &[String],
    model: Option<&str>,
    budget_document: Option<&ctx_traits_io::harness_config::BudgetDocument>,
) -> crate::Result<BuiltinTraitOutcome> {
    match run_builtin_trait_observed(
        trait_id,
        input_values,
        assignments,
        model,
        budget_document,
        None,
    )? {
        BuiltinTraitRun::Completed(outcome) => Ok(outcome),
        // Non-loop builtins (critique, explain) have no
        // round-evidence join point to fold a kill into; preserve the prior
        // flat-failure behavior for them.
        BuiltinTraitRun::Killed(killed) => Err(crate::Error::Command {
            message: format!("{trait_id} run did not complete: {}", killed.status),
        }),
    }
}

pub(crate) fn run_builtin_trait_observed(
    trait_id: &str,
    input_values: Vec<ctx_traits_core::procedure::runtime::StepSlotOutput>,
    assignments: &[String],
    model: Option<&str>,
    budget_document: Option<&ctx_traits_io::harness_config::BudgetDocument>,
    frame_observer: Option<crate::app::drive::FrameObserver<'_>>,
) -> crate::Result<BuiltinTraitRun> {
    let agent_role = match trait_id {
        "generate-trait" => "generator",
        "refine-trait" => "refiner",
        "critique-trait" => "critic",
        "explain-trait" => "generator",
        "import-trait" => "generator",
        _ => {
            return Err(crate::Error::Command {
                message: format!("unsupported built-in trait runner {trait_id:?}"),
            });
        }
    };
    // Resolve through the universal trait-id resolver: repo-local
    // `.ctx/traits/<id>` shadows the built-in when present, otherwise the
    // embedded package is materialized to a real store path (P337).
    let (trait_file, _source_kind) =
        ctx_traits_io::run::resolve_trait_path(None, Some(trait_id), "run-builtin")?;
    let assignments = builtin_assignments(agent_role, assignments, model)?;
    let outcome = ctx_traits_io::run::start(ctx_traits_io::run::StartRequest {
        // Internal driving traits keep the declared loop policy.
        strict_loops: false,
        override_dependencies: false,
        task_dispatch: false,
        defer_commands: false,
        trait_file: Some(trait_file.as_str()),
        trait_id: None,
        query: None,
        trait_args: &[],
        input_values,
        out: None,
        session_store: None,
        ephemeral: false,
        resource_evidence: ctx_traits_io::run::ResourceEvidenceMode::ReadDeclared {
            root_override: None,
        },
        assign_overrides: &assignments,
        agent_assignments: None,
        provider_capability_reports: Vec::new(),
        provider_warnings: Vec::new(),
        harness_probes: Vec::new(),
        caller: ctx_traits_core::procedure::session::CallerProvenance::cli(),
        state_source: "ctx traits built-in meta-trait runner",
        trait_arg_evidence: "ctx traits built-in meta-trait runner inputs",
        worktree: None,
        merge_rung: None,
        // generate's eval runs are internal machinery, not a waiting human.
        narrate_progress: false,
        startup_observer: None,
    })?;
    let session = outcome
        .session_path
        .as_ref()
        .ok_or_else(|| crate::Error::Command {
            message: "built-in trait runner did not persist a driveable session".to_string(),
        })?;
    // A `--budget` document ranks above every config tier at this built-in
    // call site: injecting its fields here (rather than leaving them `None`)
    // occupies the CLI-flag slot in `budget_from`'s overlay.
    let profile_budget = budget_document.map(|document| &document.budget);
    let report = crate::app::drive::drive(crate::app::drive::DriveInputs {
        file: Some(trait_file.as_str()),
        session: session.as_str(),
        session_store: None,
        assignments: &assignments,
        max_frames: profile_budget.and_then(|budget| budget.max_frames),
        frame_seconds: profile_budget.and_then(|budget| budget.frame_seconds),
        total_seconds: profile_budget.and_then(|budget| budget.total_seconds),
        max_retries: profile_budget.and_then(|budget| budget.max_retries),
        attach_wait_seconds: profile_budget.and_then(|budget| budget.attach_wait_seconds),
        idle_seconds: profile_budget.and_then(|budget| budget.idle_seconds),
        max_in_flight: 1,
        // Built-in meta-trait runners are explicitly serial and never wait
        // on a conductor lease: they always run at the default width and
        // never contend with a concurrent conductor for this ephemeral
        // session.
        wait: false,
        // An installed observer owns the whole progress surface for this
        // call — drive's own Status lines would otherwise double-print
        // every accepted frame alongside the round-scoped commentary.
        progress: if frame_observer.is_some() {
            cli::DriveProgress::None
        } else {
            cli::DriveProgress::Status
        },
        worktree: None,
        execution_dir: None,
        clear_merge_intent: false,
        panel_handoff: None,
        startup: None,
        frame_observer,
    })?;
    let session_id = session;
    let session = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: Some(trait_file.as_str()),
        trait_id: None,
        session: session_id.as_str(),
        session_store: None,
        elapsed_seconds: None,
    })?
    .session;
    let round_records = round_records_from_session(&session)?;
    // 0066.4: every built-in meta-trait runner drives a session that exists
    // only to carry this one call — never a user-facing run someone would
    // later `ctx traits drive --session` or inspect with `ctx traits story`.
    // Leaving it in the store after this function returns (success or kill)
    // strands a phantom session; the candidate/scratch artifacts a caller
    // still needs live under the scratch package root, not the ledger.
    ctx_traits_io::run_session::delete_run_session(session_id.as_str(), None)?;
    if report.status != "completed" {
        return Ok(BuiltinTraitRun::Killed(BuiltinTraitKilled {
            status: report.status,
            bound_fired: report.bound_fired,
            round_records,
        }));
    }
    let outputs = session
        .completion
        .as_ref()
        .map(|completion| &completion.final_outputs)
        .ok_or_else(|| crate::Error::Command {
            message: format!("{trait_id} completed without a final output"),
        })?;
    let [output] = outputs.as_slice() else {
        return Err(crate::Error::Command {
            message: format!(
                "{trait_id} completed with {} final outputs; exactly one is required",
                outputs.len()
            ),
        });
    };
    let output = match &output.value {
        serde_json::Value::String(value) => value.clone(),
        value => serde_json::to_string(value)
            .map_err(|e| crate::Error::json("serialize built-in trait output", e))?,
    };
    Ok(BuiltinTraitRun::Completed(BuiltinTraitOutcome {
        output,
        round_records,
    }))
}

fn builtin_assignments(
    role: &str,
    assignments: &[String],
    model: Option<&str>,
) -> crate::Result<Vec<String>> {
    if model.is_none() {
        return Ok(assignments.to_vec());
    }
    let resolved = ctx_traits_io::harness_config::resolve_runtime_assignments(assignments)?;
    // Layer every configured seat of the role (role/tier defaults <
    // `.ctx/traits/runtime.toml` role table/list < profile < `--assign`) without
    // resolving models against a harness catalog yet: `--model` still needs
    // to replace whatever selector this layering produced before any
    // resolution happens, so catalog resolution stays solely owned by the
    // existing start/drive path over the serialized overrides below. A
    // list-backed role (P456) resolves every seat here rather than the
    // role-only, first-entry-or-nothing lookup `assignment_for_role` gives.
    let seats =
        resolved
            .configured_seats_for_role(role)
            .map_err(|error| crate::Error::Command {
                message: format!(
                    "no harness assignment is configured for built-in role {role:?}: {error}"
                ),
            })?;
    if seats.is_empty() {
        return Err(crate::Error::Command {
            message: format!("no harness assignment is configured for built-in role {role:?}"),
        });
    }
    if model.is_some()
        && seats.iter().any(|(assignment, _)| {
            assignment.mode == ctx_traits_io::harness_config::RunAssignmentMode::Attach
        })
    {
        return Err(crate::Error::Command {
            message: format!("--model cannot be used with attach-mode role {role:?}"),
        });
    }
    // Drop this role's prior whole-role/seat overrides: they are already
    // folded into `seats` above, and re-emitting a fresh override per seat
    // below alongside the originals would trip the duplicate-selector
    // rejection.
    let mut overrides = assignments
        .iter()
        .filter(|item| {
            let target = item
                .split_once('=')
                .map_or(item.as_str(), |(target, _)| target);
            target != role && !target.starts_with(&format!("{role}."))
        })
        .cloned()
        .collect::<Vec<_>>();
    for (mut assignment, seat_info) in seats {
        if let Some(model) = model {
            assignment.model = Some(model.to_string());
        }
        let serialized = serde_json::to_string(&assignment)
            .map_err(|error| crate::Error::json("serialize built-in model assignment", error))?;
        let selector = match seat_info {
            Some(seat_info) => format!("{role}.{}", seat_info.seat_index),
            None => role.to_string(),
        };
        overrides.push(format!("{selector}=json:{serialized}"));
    }
    Ok(overrides)
}

pub(crate) fn decode_generate_candidate(
    raw: &str,
) -> crate::Result<(String, Option<serde_json::Value>)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok((raw.to_string(), None));
    };
    let Some(draft) = value.get("trait") else {
        return Ok((raw.to_string(), None));
    };
    let draft = draft.clone();
    let candidate = serde_json::to_string(&draft)
        .map_err(|e| crate::Error::json("serialize generated trait draft", e))?;
    Ok((candidate, Some(value)))
}

pub(crate) fn canonical_json_value(raw: &str) -> crate::Result<String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| crate::Error::json("parse structured candidate output", e))?;
    ctx_traits_core::digest::canonical_json(&value).map_err(|e| crate::Error::Command {
        message: format!("canonicalize structured candidate output: {e}"),
    })
}

pub(crate) fn blocked_assist_candidate(
    mut candidate: ctx_traits_core::assist::Candidate,
    json: bool,
    message: String,
) -> crate::Result<CommandOutput<()>> {
    candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
    candidate.warnings.push(message.clone());
    print_assist_candidate(&candidate, json)?;
    Err(crate::Error::Command { message })
}

pub(crate) fn blocked_wrapper_candidate(
    candidate: ctx_traits_core::assist::Candidate,
    raw: &str,
    encoding: ctx_traits_core::encoding::Encoding,
    trait_id: &str,
    json: bool,
    message: String,
) -> crate::Result<CommandOutput<()>> {
    let mut candidate =
        ctx_traits_core::assist::evaluate_supplied_candidate(candidate, raw, encoding).candidate;
    candidate = ctx_traits_core::assist::audit_wrapper_output(candidate, raw, trait_id);
    candidate.warnings.push(message.clone());
    print_assist_candidate(&candidate, json)?;
    Err(crate::Error::Command { message })
}

pub(crate) fn print_assist_candidate(
    candidate: &ctx_traits_core::assist::Candidate,
    json: bool,
) -> crate::Result<()> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let text = serde_json::to_string_pretty(candidate).unwrap_or_else(|e| {
                format!("{{\"error\": \"failed to serialize candidate: {e}\"}}")
            });
            println!("{text}");
        }
        OutputMode::Human(mode) => {
            let panel = assist_candidate_panel(candidate);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(())
}

pub(crate) fn print_round_report(
    report: &ctx_traits_core::assist::RoundReport,
    scratch_path: &camino::Utf8Path,
    json: bool,
) -> crate::Result<()> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let text = serde_json::to_string_pretty(report).unwrap_or_else(|e| {
                format!("{{\"error\": \"failed to serialize round report: {e}\"}}")
            });
            println!("{text}");
        }
        OutputMode::Human(mode) => {
            let panel = round_report_panel(report, scratch_path);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(())
}

fn round_report_panel(
    report: &ctx_traits_core::assist::RoundReport,
    scratch_path: &camino::Utf8Path,
) -> Panel {
    let status = if report.converged {
        PanelStatus::Passed("converged".to_string())
    } else {
        PanelStatus::Blocked("blocked".to_string())
    };
    let mut panel = Panel::new("ctx", "generate-round", status)
        .row(PanelRow::toned(
            "rung",
            report.rung.to_string(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "converged",
            report.converged.to_string(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "scratch-path",
            scratch_path.to_string(),
            RowTone::Default,
        ));
    for diagnostic in &report.diagnostics {
        panel = panel.row(PanelRow::toned(
            "diagnostic",
            format!(
                "[{}/{}] {}",
                diagnostic.gate, diagnostic.code, diagnostic.message
            ),
            RowTone::Warn,
        ));
    }
    panel
}

/// Decode an envelope's `failing-rung` string into the typed [`Rung`] the
/// uniform [`ctx_traits_core::assist::RoundEvidence`] carries.
fn parse_failing_rung(rung: &str) -> Option<ctx_traits_core::assist::Rung> {
    serde_json::from_value(serde_json::Value::String(rung.to_string())).ok()
}

/// The terminal shape every guarded-loop meta-trait's `derive-envelope` step
/// emits (task 0066.1/0066.2/0066.3): convergence, rounds spent against the
/// declared bound, and the failing rung — shared by `generate-trait`,
/// `refine-trait`, and `import-trait`'s otherwise command-specific envelope
/// structs so `round_evidence_from_envelope` has exactly one implementation.
pub(crate) trait LoopEnvelope {
    fn converged(&self) -> bool;
    fn rounds_spent(&self) -> u32;
    fn rounds_bound(&self) -> Option<u32>;
    fn failing_rung(&self) -> Option<&str>;
}

impl LoopEnvelope for GenerateEnvelope {
    fn converged(&self) -> bool {
        self.converged
    }
    fn rounds_spent(&self) -> u32 {
        self.rounds_spent
    }
    fn rounds_bound(&self) -> Option<u32> {
        self.rounds_bound
    }
    fn failing_rung(&self) -> Option<&str> {
        self.failing_rung.as_deref()
    }
}

/// Fold a run-bound kill into the uniform round-evidence shape (0066.4): the
/// rounds the session actually accepted before the kill, plus one more
/// failed round recording the kill itself, named with the bound that fired.
/// The honest rung for a mid-command kill is `Build` — the earliest rung,
/// since the driver cannot see which rung was in flight when the bound cut
/// the round short.
fn killed_round_evidence(killed: &BuiltinTraitKilled) -> ctx_traits_core::assist::RoundEvidence {
    let mut rounds = killed.round_records.clone();
    let round = u32::try_from(rounds.len() + 1).unwrap_or(u32::MAX);
    let bound = killed
        .bound_fired
        .clone()
        .unwrap_or_else(|| killed.status.clone());
    rounds.push(ctx_traits_core::assist::RoundRecord {
        round,
        rung: ctx_traits_core::assist::Rung::Build,
        converged: false,
        diagnostics: vec![ctx_traits_core::assist::Diagnostic {
            gate: ctx_traits_core::assist::Gate::Build,
            code: ctx_traits_core::assist::DiagnosticCode::RoundKilledByBound,
            field: None,
            message: format!(
                "round killed mid-ladder by run bound {bound} (drive status: {})",
                killed.status
            ),
        }],
    });
    let rounds_spent = u32::try_from(rounds.len()).unwrap_or(u32::MAX);
    ctx_traits_core::assist::RoundEvidence {
        converged: false,
        rounds_spent,
        rounds_bound: None,
        failing_rung: Some(ctx_traits_core::assist::Rung::Build),
        rounds,
    }
}

/// Print the killed-round evidence and build the non-zero-exit error for a
/// bound-killed loop join point (0066.4): the one function `handle_generate`,
/// `handle_refine`, and `handle_import`'s `--llm-assisted` path all share, so
/// a command-kill and a frame-kill are reported identically at every call
/// site rather than each join point inventing its own wording.
pub(crate) fn report_bound_kill(
    killed: &BuiltinTraitKilled,
    scratch_path: &camino::Utf8Path,
    verb: &str,
    json: bool,
) -> crate::Error {
    let evidence = killed_round_evidence(killed);
    if let Err(error) = print_round_evidence(&evidence, scratch_path, json) {
        return error;
    }
    let bound = killed
        .bound_fired
        .as_deref()
        .unwrap_or(killed.status.as_str());
    crate::Error::Command {
        message: format!(
            "{verb} killed by run bound {bound}: candidate preserved at {scratch_path}; {} round(s) spent",
            killed.round_records.len() + 1
        ),
    }
}

/// Assemble the loop's uniform round-evidence envelope from the terminal
/// envelope and the session's own accepted `round-report` history (task
/// 0066.2). `rounds.len() == rounds_spent` is verified downstream by the
/// panel/JSON printer, not silently trusted here.
pub(crate) fn round_evidence_from_envelope<E: LoopEnvelope>(
    envelope: &E,
    rounds: Vec<ctx_traits_core::assist::RoundRecord>,
) -> ctx_traits_core::assist::RoundEvidence {
    ctx_traits_core::assist::RoundEvidence {
        converged: envelope.converged(),
        rounds_spent: envelope.rounds_spent(),
        rounds_bound: envelope.rounds_bound(),
        failing_rung: envelope.failing_rung().and_then(parse_failing_rung),
        rounds,
    }
}

/// Print a [`ctx_traits_core::assist::RoundEvidence`]: JSON mode serializes
/// the struct verbatim, human mode renders it as a panel — the same struct
/// backs both, so the two surfaces agree on every fact by construction (task
/// 0066.2). Shared by the guarded-loop path and `--candidate`'s single-round
/// wrapper.
pub(crate) fn print_round_evidence(
    evidence: &ctx_traits_core::assist::RoundEvidence,
    scratch_path: &camino::Utf8Path,
    json: bool,
) -> crate::Result<()> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let text = serde_json::to_string_pretty(evidence).unwrap_or_else(|e| {
                format!("{{\"error\": \"failed to serialize round evidence: {e}\"}}")
            });
            println!("{text}");
        }
        OutputMode::Human(mode) => {
            let panel = round_evidence_panel(evidence, scratch_path);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(())
}

fn round_evidence_panel(
    evidence: &ctx_traits_core::assist::RoundEvidence,
    scratch_path: &camino::Utf8Path,
) -> Panel {
    let status = if evidence.converged {
        PanelStatus::Passed("converged".to_string())
    } else {
        PanelStatus::Blocked("blocked".to_string())
    };
    let mut panel = Panel::new("ctx", "generate", status)
        .row(PanelRow::toned(
            "converged",
            evidence.converged.to_string(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "rounds-spent",
            evidence.rounds_spent.to_string(),
            RowTone::Default,
        ));
    if let Some(bound) = evidence.rounds_bound {
        panel = panel.row(PanelRow::toned(
            "rounds-bound",
            bound.to_string(),
            RowTone::Default,
        ));
    }
    if let Some(rung) = evidence.failing_rung {
        panel = panel.row(PanelRow::toned(
            "failing-rung",
            rung.to_string(),
            RowTone::Default,
        ));
    }
    // A missing fact is a 0066.1 gap to surface, not to patch around — a
    // mismatch prints a warning row instead of silently trusting either
    // count.
    if evidence.rounds.len() as u32 != evidence.rounds_spent {
        panel = panel.row(PanelRow::toned(
            "warning",
            format!(
                "round history has {} entries but rounds-spent reports {}",
                evidence.rounds.len(),
                evidence.rounds_spent
            ),
            RowTone::Warn,
        ));
    }
    if !evidence.converged {
        panel = panel.row(PanelRow::toned(
            "scratch-path",
            scratch_path.to_string(),
            RowTone::Default,
        ));
    }
    for record in &evidence.rounds {
        let bound_suffix = evidence
            .rounds_bound
            .map(|bound| format!("/{bound}"))
            .unwrap_or_default();
        if record.converged {
            panel = panel.row(PanelRow::toned(
                format!("round {}{bound_suffix}", record.round),
                "converged",
                RowTone::Pass,
            ));
            continue;
        }
        let top_diagnostic = record
            .diagnostics
            .first()
            .map_or("no diagnostic recorded", |diagnostic| {
                diagnostic.message.as_str()
            });
        panel = panel.row(PanelRow::toned(
            format!("round {}{bound_suffix}", record.round),
            format!("failed at {} — {top_diagnostic}", record.rung),
            RowTone::Fail,
        ));
    }
    panel
}

fn assist_candidate_panel(candidate: &ctx_traits_core::assist::Candidate) -> Panel {
    let status = match candidate.status {
        ctx_traits_core::assist::CandidateStatus::Blocked => {
            PanelStatus::Blocked("blocked".to_string())
        }
        _ => PanelStatus::Passed("passed".to_string()),
    };
    let mut panel = Panel::new("ctx", candidate.operation.as_str(), status)
        .row(PanelRow::toned(
            "status",
            crate::app::presentation::wire_name(&candidate.status),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "provider-available",
            candidate.provider.provider_available.to_string(),
            RowTone::Default,
        ));
    if let Some(ref id) = candidate.provider.provider_id {
        panel = panel.row(PanelRow::toned("provider", id, RowTone::Default));
    }
    if let Some(ref id) = candidate.provider.model_id {
        panel = panel.row(PanelRow::toned("model", id, RowTone::Default));
    }
    if !candidate.provider.provider_parameters.is_empty() {
        let value = candidate
            .provider
            .provider_parameters
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        panel = panel.row(PanelRow::toned(
            "provider-parameters",
            value,
            RowTone::Default,
        ));
    }
    if let Some(ref reason) = candidate.provider.reason {
        panel = panel.row(PanelRow::toned("provider-reason", reason, RowTone::Default));
    }
    let plan = &candidate.request_plan;
    if !plan.source_trait_ids.is_empty() {
        panel = panel.row(PanelRow::toned(
            "source-trait-ids",
            plan.source_trait_ids.join(", "),
            RowTone::Default,
        ));
    }
    panel = panel
        .row(PanelRow::toned(
            "user-request-digest",
            plan.user_request_digest.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "prompt-context-digest",
            plan.prompt_context_digest.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "target-path",
            plan.target_path.as_str(),
            RowTone::Default,
        ));

    if let Some(ref d) = candidate.raw_output_digest {
        panel = panel.row(PanelRow::toned(
            "raw-output-digest",
            d.as_str(),
            RowTone::Default,
        ));
    }
    if let Some(ref d) = candidate.parsed_candidate_digest {
        panel = panel.row(PanelRow::toned(
            "parsed-candidate-digest",
            d.as_str(),
            RowTone::Default,
        ));
    }
    if let Some(ref d) = candidate.canonical_digest {
        panel = panel.row(PanelRow::toned(
            "canonical-digest",
            d.as_str(),
            RowTone::Default,
        ));
    }
    if let Some(ref d) = candidate.normalized_output_digest {
        panel = panel.row(PanelRow::toned(
            "normalized-output-digest",
            d.as_str(),
            RowTone::Default,
        ));
    }
    if let Some(ref id) = candidate.candidate_trait_id {
        panel = panel.row(PanelRow::toned("candidate-trait-id", id, RowTone::Default));
    }
    if let Some(ref before) = candidate.before {
        panel = panel.row(PanelRow::toned(
            "candidate-before",
            format!("status={}, trust={}", before.status, before.trust),
            RowTone::Default,
        ));
    }
    if let Some(ref after) = candidate.after {
        panel = panel.row(PanelRow::toned(
            "candidate-after",
            format!("status={}, trust={}", after.status, after.trust),
            RowTone::Default,
        ));
    }

    let gates = &candidate.gate_summary;
    panel = panel
        .row(PanelRow::toned(
            "gate-parse",
            gates.parse.ok.to_string(),
            if gates.parse.ok {
                RowTone::Pass
            } else {
                RowTone::Fail
            },
        ))
        .row(PanelRow::toned(
            "gate-audit",
            gates.audit.ok.to_string(),
            if gates.audit.ok {
                RowTone::Pass
            } else {
                RowTone::Fail
            },
        ))
        .row(PanelRow::toned(
            "gate-check",
            gates.check.ok.to_string(),
            if gates.check.ok {
                RowTone::Pass
            } else {
                RowTone::Fail
            },
        ));
    if gates.audit.blocking > 0 {
        panel = panel.row(PanelRow::toned(
            "audit-blocking",
            gates.audit.blocking.to_string(),
            RowTone::Fail,
        ));
    }
    if let Some(ref e) = gates.validation_error_code {
        panel = panel.row(PanelRow::toned("validation-error", e, RowTone::Fail));
    }
    if !gates.check.failed_sections.is_empty() {
        panel = panel.row(PanelRow::toned(
            "check-failed-sections",
            gates.check.failed_sections.join(", "),
            RowTone::Fail,
        ));
    }

    if let Some(ref report) = candidate.check_report {
        let rows = report
            .sections
            .iter()
            .map(|section| {
                PanelRow::toned(
                    section.name.as_str(),
                    format!("{} ({})", section.ok, section.summary),
                    if section.ok {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                )
            })
            .collect();
        panel = panel.section(crate::app::presentation::PanelSection::new(
            "check-sections",
            rows,
        ));
    }
    if !candidate.diagnostics.is_empty() {
        let rows = candidate
            .diagnostics
            .iter()
            .map(|diag| {
                let field = diag.field.as_deref().unwrap_or("-");
                PanelRow::toned(
                    format!("{}:{}", diag.gate, diag.code),
                    format!("{field}: {}", diag.message),
                    RowTone::Default,
                )
            })
            .collect();
        panel = panel.section(crate::app::presentation::PanelSection::new(
            "diagnostics",
            rows,
        ));
    }
    if let Some(ref evidence) = candidate.context_evidence {
        if evidence.get("plan-result").is_some() {
            panel = panel.section(crate::app::presentation::PanelSection::new(
                "context-evidence",
                compose_context_evidence_rows(evidence),
            ));
        } else {
            panel = panel.row(PanelRow::toned(
                "context-evidence",
                evidence.to_string(),
                RowTone::Default,
            ));
        }
    }

    if candidate.warnings.is_empty() {
        panel = panel.row(PanelRow::toned("warnings", "none", RowTone::Pass));
    } else {
        let rows = candidate
            .warnings
            .iter()
            .map(|w| PanelRow::toned("warning", w.as_str(), RowTone::Fail))
            .collect();
        panel = panel.section(crate::app::presentation::PanelSection::new(
            "warnings", rows,
        ));
    }
    if !candidate.unsupported_capabilities.is_empty() {
        panel = panel.row(PanelRow::toned(
            "unsupported-capabilities",
            candidate.unsupported_capabilities.join(", "),
            RowTone::Default,
        ));
    }
    panel = panel.row(PanelRow::toned(
        "write-status",
        crate::app::presentation::wire_name(&candidate.write_status),
        RowTone::Default,
    ));
    if let Some(ref p) = candidate.written_path {
        panel = panel.next(PanelRow::toned("written-path", p, RowTone::Default));
    }
    panel
}

fn compose_context_evidence_rows(evidence: &serde_json::Value) -> Vec<PanelRow> {
    let mut rows = vec![
        PanelRow::toned(
            "composition-result",
            evidence
                .get("plan-result")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
            RowTone::Default,
        ),
        PanelRow::toned(
            "composition-sources",
            format!(
                "ids={}, paths={}, digests={}",
                evidence
                    .get("source-trait-ids")
                    .map_or_else(|| "[]".to_string(), serde_json::Value::to_string),
                evidence
                    .get("source-paths")
                    .map_or_else(|| "[]".to_string(), serde_json::Value::to_string),
                evidence
                    .get("source-digests")
                    .map_or_else(|| "[]".to_string(), serde_json::Value::to_string),
            ),
            RowTone::Default,
        ),
        PanelRow::toned(
            "composition-counts",
            format!(
                "conflicts={}, warnings={}, bindings={}, port-compatibility={}",
                evidence
                    .get("conflict-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                evidence
                    .get("warning-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                evidence
                    .get("binding-proposal-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                evidence
                    .get("port-compatibility-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            ),
            RowTone::Default,
        ),
    ];
    if let Some(conflicts) = evidence
        .get("conflicts")
        .and_then(serde_json::Value::as_array)
    {
        for conflict in conflicts {
            rows.push(PanelRow::toned(
                "composition-conflict",
                format!(
                    "{} [{}]: {}",
                    conflict
                        .get("field-path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown"),
                    conflict
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown"),
                    conflict
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("redacted")
                ),
                RowTone::Fail,
            ));
        }
    }
    if let Some(warnings) = evidence
        .get("warnings")
        .and_then(serde_json::Value::as_array)
    {
        for warning in warnings {
            rows.push(PanelRow::toned(
                "composition-warning",
                format!(
                    "{}: {}",
                    warning
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown"),
                    warning
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("redacted")
                ),
                RowTone::Fail,
            ));
        }
    }
    rows
}

pub(crate) fn trait_package_output_paths(trait_id: &str, out: Option<&str>) -> (String, String) {
    match out {
        Some(out_path) => {
            let path = camino::Utf8Path::new(out_path);
            if path.extension().is_some() {
                let package = ctx_traits_io::layout::package_root_for_manifest(path)
                    .map_or(".", camino::Utf8Path::as_str);
                (package.to_string(), path.to_string())
            } else {
                (
                    path.to_string(),
                    ctx_traits_io::layout::package_manifest_write_path(path).to_string(),
                )
            }
        }
        None => {
            let package =
                camino::Utf8Path::new(ctx_traits_io::layout::trait_protocol_root()).join(trait_id);
            // The default package root under `trait_protocol_root()` is
            // always a canonical v2 package root, so its write path must go
            // through the same `is_canonical_package_root` branch every
            // other write path uses (`package_manifest_write_path`,
            // `write_complete_import_package`) rather than hardcoding the
            // legacy `generated/trait.toml` name.
            let output = ctx_traits_io::layout::package_manifest_write_path(&package);
            (package.to_string(), output.to_string())
        }
    }
}

pub(crate) fn attach_assist_check_report(
    candidate: ctx_traits_core::assist::Candidate,
    normalized_trait: Option<&ctx_traits_core::Trait>,
    normalized_text: Option<&str>,
    trait_root: &camino::Utf8Path,
) -> crate::Result<ctx_traits_core::assist::Candidate> {
    let Some(trait_ref) = normalized_trait else {
        return Ok(candidate);
    };
    let Some(text) = normalized_text else {
        return Ok(candidate);
    };

    let trait_id = trait_ref.id.as_str().to_string();
    let source_digest = ctx_traits_core::digest::Digest::source(text);
    let mut check_warnings = Vec::new();
    let mut resource_evidence_ok = true;
    let mut manifest_digest = None;
    let mut manifest_warning_count = 0usize;

    let roots_result =
        ctx_traits_io::resource::resolve_resource_roots(trait_root, &trait_ref.resources);
    let file_evidence = match roots_result.as_ref().ok().and_then(|roots| {
        ctx_traits_io::resource::digest_resources(
            roots,
            trait_ref.id.as_str(),
            &trait_ref.resources,
        )
        .ok()
    }) {
        Some(manifest) => {
            manifest_digest = Some(manifest.manifest_digest.as_str().to_string());
            manifest_warning_count = manifest.warnings.len();
            build_file_evidence_from_io(&manifest)
        }
        None => {
            resource_evidence_ok = false;
            check_warnings.push(ctx_traits_core::check::CheckWarning {
                section: ctx_traits_core::check::Section::Resources,
                code: "resource-io-unavailable".to_string(),
                field: Some(trait_root.to_string()),
                message: "candidate resource evidence unavailable; raw IO detail redacted"
                    .to_string(),
            });
            Vec::new()
        }
    };

    let (resource_body_evidence, body_read_warnings) =
        match roots_result.as_ref().ok().and_then(|roots| {
            crate::app::report_resources::scan_resource_bodies(roots, trait_ref).ok()
        }) {
            Some(result) => result,
            None => {
                resource_evidence_ok = false;
                check_warnings.push(ctx_traits_core::check::CheckWarning {
                    section: ctx_traits_core::check::Section::Resources,
                    code: "resource-body-io-unavailable".to_string(),
                    field: Some(trait_root.to_string()),
                    message: "candidate resource body evidence unavailable; raw IO detail redacted"
                        .to_string(),
                });
                (Vec::new(), Vec::new())
            }
        };
    let resource_plan = ctx_traits_core::resource_plan::plan_resource_inclusion_with_bodies(
        trait_ref,
        &file_evidence,
        &resource_body_evidence,
        &[],
    );
    let resource_budget = ctx_traits_core::resource_plan::estimate_context_budget(&resource_plan);
    let mut resource_read_warnings = Vec::new();
    resource_read_warnings.extend(crate::app::report_resources::resource_read_warning_strings(
        &body_read_warnings,
    ));
    let render_plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
        trait_ref,
        ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
        source_digest.as_str(),
        ctx_traits_core::render::ResourceEvidenceInputs {
            file_evidence: &file_evidence,
            body_evidence: &resource_body_evidence,
            dependency_resources: &[],
            manifest_digest: manifest_digest.as_deref(),
            read_warnings: resource_read_warnings,
        },
    );
    check_warnings.extend(
        crate::app::report_check::check_warnings_from_render_and_resources(
            &render_plan,
            &resource_plan,
            &body_read_warnings,
        ),
    );
    check_warnings.extend(crate::app::report_check::sequence_control_check_warnings(
        trait_ref,
    ));
    let resource_audit = match roots_result.as_ref().ok().and_then(|roots| {
        crate::app::report_resources::audit_declared_text_resources(roots, trait_ref).ok()
    }) {
        Some(evidence) => evidence,
        None => {
            resource_evidence_ok = false;
            check_warnings.push(ctx_traits_core::check::CheckWarning {
                section: ctx_traits_core::check::Section::Resources,
                code: "resource-audit-unavailable".to_string(),
                field: Some(trait_root.to_string()),
                message: "candidate resource audit evidence unavailable; raw IO detail redacted"
                    .to_string(),
            });
            crate::app::report_resources::ResourceTextAuditEvidence {
                findings: Vec::new(),
                warnings: Vec::new(),
                skipped: Vec::new(),
            }
        }
    };
    let mut audit = ctx_traits_core::audit::scan_hidden_content(text, &trait_id, Some("candidate"));
    audit.extend(resource_audit.findings);
    audit.extend(render_plan.model_view.post_audit_findings.clone());
    audit.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.code.cmp(&b.code))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.message.cmp(&b.message))
    });

    let audit_ok = audit
        .iter()
        .all(|finding| matches!(finding.severity, ctx_traits_core::audit::Severity::Advisory));
    // The canonical document carries no status/trust field (Group 95,
    // 2026-07-19), so there is nothing on `trait_ref` to check here:
    // candidate output is always package status=draft and machine
    // trust=unreviewed until an explicit, separate human review action.
    let lifecycle_ok = true;
    let resource_ok = resource_evidence_ok
        && resource_plan.warnings.iter().all(|warning| {
            !matches!(
                warning,
                ctx_traits_core::resource_plan::InclusionWarning::SymlinkDetected { .. }
                    | ctx_traits_core::resource_plan::InclusionWarning::DependencyResourceUnavailable {
                        ..
                    }
            )
        });
    let render_ok = resource_evidence_ok
        && render_plan.resource_warnings.is_empty()
        && !render_plan.model_view.has_blocking_post_audit_findings();
    let scenario_audit = ctx_traits_core::r#trait::scenario::audit_scenarios(&trait_ref.scenarios);
    let scenario_ids: std::collections::BTreeSet<&str> =
        trait_ref.scenarios.iter().map(|s| s.id.as_str()).collect();
    let eval_audit = ctx_traits_core::r#trait::eval::audit_evals(&trait_ref.evals, &scenario_ids);

    let report = ctx_traits_core::check::CheckReport::new(&trait_id, true)
        .with_section(
            ctx_traits_core::check::Section::Validation,
            "candidate decoded and normalized successfully",
            true,
        )
        .with_section(
            ctx_traits_core::check::Section::CandidateLifecycleTrust,
            "status=draft, trust=unreviewed (structural invariant; the canonical document carries \
             no status/trust field to normalize)",
            lifecycle_ok,
        )
        .with_section(
            ctx_traits_core::check::Section::Resources,
            &format!(
                "manifest={}, read-warnings={}, inclusion-warnings={}, budget={} bytes/~{} tokens, text-audit-skipped={}",
                manifest_digest.as_deref().unwrap_or("unavailable"),
                manifest_warning_count + resource_audit.warnings.len(),
                resource_plan.warnings.len(),
                resource_budget.total_bytes,
                resource_budget.estimated_tokens,
                resource_audit.skipped.len(),
            ),
            resource_ok,
        )
        .with_section(
            ctx_traits_core::check::Section::RenderReadiness,
            &format!(
                "capability-warnings={}, resource-warnings={}, model-view-warnings={}",
                render_plan.capability_warnings.len(),
                render_plan.resource_warnings.len(),
                render_plan.model_view.warnings.len(),
            ),
            render_ok,
        )
        .with_section(
            ctx_traits_core::check::Section::HiddenContentAudit,
            &format!("{} finding(s)", audit.len()),
            audit_ok,
        )
        .with_section(
            ctx_traits_core::check::Section::ScenarioEvalAudit,
            &format!(
                "scenario-warnings={}, eval-warnings={}",
                scenario_audit.len(),
                eval_audit.len(),
            ),
            true,
        )
        .with_section(
            ctx_traits_core::check::Section::ModelView,
            &format!(
                "digest={}, sections={}, normalizations={}",
                render_plan.model_view.content_digest,
                render_plan.model_view.sections.len(),
                render_plan.model_view.normalizations.len(),
            ),
            !render_plan.model_view.has_blocking_post_audit_findings(),
        )
        .with_unsupported_capabilities(vec![
            "activation.request-facts-missing".to_string(),
            "dependencies.project-manifest-check-not-wired".to_string(),
        ])
        .with_audit(audit)
        .with_warnings(check_warnings);

    Ok(ctx_traits_core::assist::attach_check_report(
        candidate, report,
    ))
}

pub(crate) fn apply_assist_check_drift(
    mut candidate: ctx_traits_core::assist::Candidate,
    target_path: &str,
    normalized_text: Option<&str>,
) -> ctx_traits_core::assist::Candidate {
    if candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked {
        return candidate;
    }
    let Some(expected_text) = normalized_text else {
        return candidate;
    };

    let path = camino::Utf8Path::new(target_path);
    let actual_text = match ctx_traits_io::read::read_text(path) {
        Ok(text) => text,
        Err(_) => {
            candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
            candidate
                .diagnostics
                .push(ctx_traits_core::assist::Diagnostic {
                    gate: ctx_traits_core::assist::Gate::Drift,
                    code: ctx_traits_core::assist::DiagnosticCode::TargetMissing,
                    field: Some(target_path.to_string()),
                    message: "target path is missing; check mode refuses to write".to_string(),
                });
            candidate
                .gate_summary
                .check
                .failed_sections
                .push("drift".to_string());
            candidate.gate_summary.check.ok = false;
            candidate
                .warnings
                .push("check drift gate failed: target is missing".to_string());
            return candidate;
        }
    };

    let expected = ctx_traits_core::digest::Digest::source(expected_text)
        .as_str()
        .to_string();
    let actual = ctx_traits_core::digest::Digest::source(&actual_text)
        .as_str()
        .to_string();
    if expected != actual {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate.diagnostics.push(ctx_traits_core::assist::Diagnostic {
            gate: ctx_traits_core::assist::Gate::Drift,
            code: ctx_traits_core::assist::DiagnosticCode::TargetDrift,
            field: Some(target_path.to_string()),
            message: format!(
                "target content differs from normalized candidate: expected {expected}, actual {actual}"
            ),
        });
        candidate
            .gate_summary
            .check
            .failed_sections
            .push("drift".to_string());
        candidate.gate_summary.check.ok = false;
        candidate
            .warnings
            .push("check drift gate failed: target differs from normalized candidate".to_string());
    }
    candidate
}

pub(crate) use handle_critique as critique;
pub(crate) use handle_generate as handle;
