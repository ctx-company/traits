//! App entry: parse arguments, dispatch, convert to exit.
//!
//! The entry function is the thin app edge. It parses arguments via the
//! surface, calls the matching command handler, and converts the result to an
//! exit status.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;

use ctx_traits_core::response::CommandOutput;

use crate::app::surface::cli;

use crate::app::{
    context_cache::{
        handle_cache_prune, handle_cache_rebuild, handle_cache_status, handle_context_clear,
        handle_context_plan, handle_context_status, handle_pack,
    },
    explain_inspect::{ExplainInputs, handle_explain, handle_inspect},
    launch_reports::{
        handle_compatibility, handle_context_contracts, handle_cost, handle_evidence,
        handle_hygiene, handle_policy, handle_prepare_public, handle_subagent,
    },
    lifecycle_reporting::{
        current_utf8_dir, format_manifest_discovery, handle_init, handle_list, handle_sync,
        handle_sync_all, handle_trust,
    },
    schema_synth_build::{handle_build, handle_schema, handle_synth},
};

fn resolved_run_worktree(
    configured: bool,
    requested: Option<Option<&str>>,
    no_worktree: bool,
) -> Option<Option<&str>> {
    requested.or_else(|| (!no_worktree && configured).then_some(None))
}

fn resolved_run_wait(configured: bool, wait: bool, no_wait: bool) -> bool {
    if wait {
        true
    } else if no_wait {
        false
    } else {
        configured
    }
}

fn resolved_strict_loops(configured: bool, strict_loops: bool, no_strict_loops: bool) -> bool {
    if no_strict_loops {
        false
    } else {
        strict_loops || configured
    }
}

/// P550 `--story`/`--no-story`/`[drive] story` precedence: `--no-story` wins;
/// an explicit `--story[=<level>]` overrides config, defaulting the bare
/// flag's missing level to `StoryLevel::Default`; with no flag at all,
/// `[drive] story` supplies the default (absent = off, matching `--no-merge`'s
/// shape rather than `--wait`'s plain bool — the pane is opt-in, not
/// opt-out).
fn resolved_story_level(
    configured: Option<ctx_traits_core::procedure::story::StoryLevel>,
    requested: Option<Option<&str>>,
    no_story: bool,
) -> crate::Result<Option<ctx_traits_core::procedure::story::StoryLevel>> {
    if no_story {
        return Ok(None);
    }
    let Some(requested) = requested else {
        return Ok(configured);
    };
    match requested {
        None => Ok(Some(ctx_traits_core::procedure::story::StoryLevel::Default)),
        Some(level) => level
            .parse()
            .map(Some)
            .map_err(|message| crate::Error::Command { message }),
    }
}

/// P460 automatic-landing intent precedence: `--no-merge` wins; an explicit
/// `--merge=standard|deep` fixes the rung; a bare `--merge` uses `[merge]
/// deep` (falling back to standard); with no flag at all, `[merge] auto`
/// enables the configured rung. Resolved once here and never re-resolved
/// from config later, so a paused/resumed drive lands with the same rung
/// even if config changes in between.
fn resolved_merge_intent(
    policy: ctx_traits_io::harness_config::EffectiveMergePolicy,
    merge: Option<Option<cli::MergeRung>>,
    no_merge: bool,
) -> Option<ctx_traits_core::procedure::session::MergeRung> {
    use ctx_traits_core::procedure::session::MergeRung as CoreRung;
    if no_merge {
        return None;
    }
    let configured_rung = if policy.deep {
        CoreRung::Deep
    } else {
        CoreRung::Standard
    };
    match merge {
        Some(Some(cli::MergeRung::Standard)) => Some(CoreRung::Standard),
        Some(Some(cli::MergeRung::Deep)) => Some(CoreRung::Deep),
        Some(None) => Some(configured_rung),
        None if policy.auto => Some(configured_rung),
        None => None,
    }
}

struct ResolvedRunBudget {
    max_frames: Option<u64>,
    frame_seconds: Option<u64>,
    total_seconds: Option<u64>,
    max_retries: Option<u64>,
    attach_wait_seconds: Option<u64>,
    idle_seconds: Option<u64>,
    max_in_flight: usize,
}

struct RunBudgetInputs {
    max_frames: Option<u64>,
    frame_seconds: Option<u64>,
    total_seconds: Option<u64>,
    max_retries: Option<u64>,
    attach_wait_seconds: Option<u64>,
    idle_seconds: Option<u64>,
    max_in_flight: Option<usize>,
}

fn resolve_run_budget(
    policy: ctx_traits_io::harness_config::EffectiveRunPolicy,
    input: RunBudgetInputs,
) -> ResolvedRunBudget {
    ResolvedRunBudget {
        max_frames: input.max_frames.or(policy.max_frames),
        frame_seconds: input.frame_seconds.or(policy.frame_seconds),
        total_seconds: input.total_seconds.or(policy.total_seconds),
        max_retries: input.max_retries.or(policy.max_retries),
        attach_wait_seconds: input.attach_wait_seconds.or(policy.attach_wait_seconds),
        idle_seconds: input.idle_seconds.or(policy.idle_seconds),
        max_in_flight: input.max_in_flight.unwrap_or(policy.max_in_flight),
    }
}

use crate::app::{
    eval::EvalInputs,
    generate::{CritiqueInputs, GenerateInputs},
    refine::RefineInputs,
    resolve::ResolveInputs,
    run::{CallInputs, RunInfoInputs, RunInputs, SessionStartInputs, SetInputs},
};

pub(crate) use crate::app::explain_inspect::build_file_evidence_from_io;
pub(crate) use crate::app::generate::{
    attach_assist_check_report, print_assist_candidate, trait_package_output_paths,
};
pub(crate) use crate::app::launch_reports::print_lock_update;
pub(crate) use crate::app::run::run_envelope;
pub(crate) use crate::app::schema_synth_build::print_synth_provenance;

/// Run the CLI from the given arguments.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let command = match cli::parse(args) {
        Ok(command) => command,
        Err(report) => report.exit(),
    };

    match command {
        Some(command) => match handle(command) {
            Ok(output) => {
                for warning in &output.warnings {
                    eprintln!("warning: {}: {}", warning.code, warning.message);
                }
                ExitCode::SUCCESS
            }
            Err(crate::Error::AlreadyReported { exit_code, .. }) => ExitCode::from(exit_code),
            Err(err) => {
                eprintln!("{}", render_error(&err));
                ExitCode::FAILURE
            }
        },
        None => {
            cli::print_help();
            ExitCode::SUCCESS
        }
    }
}

/// Fold `ctx traits trust --json`'s namespace-level flag into whichever
/// subcommand was given, so `--json` before or after the subcommand name
/// reaches the same one output path in `handle_trust` rather than needing
/// duplicated JSON-selection logic per subcommand.
fn merge_trust_json(subcommand: cli::TrustCommand, namespace_json: bool) -> cli::TrustCommand {
    match subcommand {
        cli::TrustCommand::Approve {
            operand,
            digest,
            all_current,
            reason,
            json,
        } => cli::TrustCommand::Approve {
            operand,
            digest,
            all_current,
            reason,
            json: json || namespace_json,
        },
        cli::TrustCommand::Block {
            operand,
            digest,
            reason,
            json,
        } => cli::TrustCommand::Block {
            operand,
            digest,
            reason,
            json: json || namespace_json,
        },
        cli::TrustCommand::List { stale, json } => cli::TrustCommand::List {
            stale,
            json: json || namespace_json,
        },
    }
}

/// Fold `ctx traits config --json`'s namespace-level flag into whichever
/// subcommand was given; see [`merge_trust_json`].
fn merge_config_json(subcommand: cli::ConfigCommand, namespace_json: bool) -> cli::ConfigCommand {
    match subcommand {
        cli::ConfigCommand::Build { path, json } => cli::ConfigCommand::Build {
            path,
            json: json || namespace_json,
        },
        cli::ConfigCommand::Accept { yes, json } => cli::ConfigCommand::Accept {
            yes,
            json: json || namespace_json,
        },
        cli::ConfigCommand::Init { global, json } => cli::ConfigCommand::Init {
            global,
            json: json || namespace_json,
        },
    }
}

fn merge_task_json(subcommand: cli::TaskCommand, namespace_json: bool) -> cli::TaskCommand {
    match subcommand {
        cli::TaskCommand::Import { path, json } => cli::TaskCommand::Import {
            path,
            json: json || namespace_json,
        },
    }
}

fn merge_cache_json(subcommand: cli::CacheCommand, namespace_json: bool) -> cli::CacheCommand {
    match subcommand {
        cli::CacheCommand::Rebuild {
            repo_root,
            cache_root,
            json,
        } => cli::CacheCommand::Rebuild {
            repo_root,
            cache_root,
            json: json || namespace_json,
        },
        cli::CacheCommand::Status {
            repo_root,
            cache_root,
            json,
        } => cli::CacheCommand::Status {
            repo_root,
            cache_root,
            json: json || namespace_json,
        },
        cli::CacheCommand::Prune {
            repo_root,
            cache_root,
            dry_run,
            build,
            build_target,
            json,
        } => cli::CacheCommand::Prune {
            repo_root,
            cache_root,
            dry_run,
            build,
            build_target,
            json: json || namespace_json,
        },
    }
}

/// Render the full `Display` of a command error, indenting continuation
/// lines so a multi-line reason still reads as one error on stderr. No
/// per-type opt-in one-liner exists yet — no type in the tree needs one.
fn render_error(error: &crate::Error) -> String {
    let text = error.to_string();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("command failed");
    let mut out = String::from(first);
    for line in lines {
        out.push('\n');
        out.push_str("  ");
        out.push_str(line);
    }
    out
}

pub(crate) fn to_json_value<T: serde::Serialize>(
    value: &T,
    label: &str,
) -> crate::Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| crate::Error::json(label, e))
}

pub(crate) fn print_json_report<T: serde::Serialize>(value: &T, label: &str) -> crate::Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| crate::Error::json(format!("serialize {label}"), e))?;
    crate::app::tui::write_plain_line(json)
}

fn handle(command: cli::Command) -> crate::Result<CommandOutput<()>> {
    match command {
        cli::Command::Traits {
            session,
            subcommand,
        } => match subcommand {
            None => {
                // Truly bare: no `--session` override either — an explicit
                // session target is a scripted/non-interactive intent even
                // on a TTY, so it keeps the exact line-mode output too.
                if session.is_none() && crate::app::dashboard::interactive_available() {
                    crate::app::dashboard::run()?;
                    return Ok(CommandOutput::new(()));
                }
                println!("ctx traits — use --help to list trait commands");
                Ok(CommandOutput::new(()))
            }
            Some(cli::TraitsCommand::Init { name, json }) => handle_init(name.as_deref(), json),
            Some(cli::TraitsCommand::Create { name, from, json }) => {
                match (name.as_deref(), from.as_deref()) {
                    (None, None) => crate::app::new::handle_list_templates(json),
                    (Some(name), Some(from)) => crate::app::new::handle_new(name, from, json),
                    (Some(_), None) => Err(crate::Error::Command {
                        message: "ctx traits create <name> requires --from <template>".to_string(),
                    }),
                    (None, Some(_)) => Err(crate::Error::Command {
                        message: "ctx traits create --from <template> requires a name".to_string(),
                    }),
                }
            }
            Some(cli::TraitsCommand::Fork { id, json }) => {
                crate::app::fork::handle_fork(&id, json)
            }
            Some(cli::TraitsCommand::List { json, verbose }) => handle_list(json, verbose),
            Some(cli::TraitsCommand::Stats {
                since,
                trait_id,
                json,
            }) => crate::app::stats::handle_stats(since, trait_id.as_deref(), json),
            Some(cli::TraitsCommand::Running { json }) => crate::app::running::handle_running(json),
            Some(cli::TraitsCommand::Story {
                run,
                session_store,
                json,
                markdown,
                level,
            }) => {
                let level = level
                    .map(|level| {
                        level
                            .parse::<ctx_traits_core::procedure::story::StoryLevel>()
                            .map_err(|message| crate::Error::Command { message })
                    })
                    .transpose()?;
                crate::app::story::handle_story(&run, session_store.as_deref(), json, markdown, level)
            }
            Some(cli::TraitsCommand::Doctor {
                path,
                config,
                json,
                migrate_config,
                apply,
                verbose,
            }) => {
                if migrate_config {
                    crate::app::doctor::handle_doctor_migrate_config(apply, json)
                } else if config {
                    if apply {
                        Err(crate::Error::Command {
                            message: "ctx traits doctor --config does not support --apply"
                                .to_string(),
                        })
                    } else {
                        crate::app::doctor::handle_doctor_config(json)
                    }
                } else {
                    crate::app::doctor::handle_doctor(path.as_deref(), json, verbose, apply)
                }
            }
            Some(cli::TraitsCommand::ClaimGate { json }) => {
                crate::app::report_handlers::handle_claim_gate(json)
            }
            // P567: `ctx traits dependency <verb>` is the real surface; the
            // six legacy top-level verbs below are hidden aliases for one
            // release. Both paths converge on `handle_dependency` so the two
            // spellings can never drift in behavior.
            Some(cli::TraitsCommand::Dependency { json, subcommand }) => {
                handle_dependency(merge_dependency_json(subcommand, json))
            }
            Some(cli::TraitsCommand::Vendor {
                trait_arg,
                manifest,
                file,
                locked,
                json,
            }) => handle_dependency(cli::DependencyCommand::Install {
                trait_arg,
                manifest,
                file,
                locked,
                json,
            }),
            Some(cli::TraitsCommand::Trust {
                trait_arg,
                file,
                json,
                subcommand,
            }) => match subcommand {
                // The outer `TRAIT`/`--file` target is valid only for bare
                // `ctx traits trust <trait>` status: `approve`/`block`/`list`
                // each take their own operand (`approve`/`block`) or none
                // (`list`). Enforced here, once, before any subcommand
                // dispatches — never silently ignoring an outer target the
                // caller may have believed was the command's subject.
                Some(_) if trait_arg.is_some() || file.is_some() => {
                    Err(crate::Error::Command {
                        message: "a trait name or --file <path> before the subcommand is only valid for bare `ctx traits trust <trait>` status; `approve`/`block` take their own operand and `list` takes none — remove the outer target".to_string(),
                    })
                }
                Some(subcommand) => handle_trust(merge_trust_json(subcommand, json)),
                None => crate::app::lifecycle_reporting::handle_trust_status(
                    trait_arg.as_deref(),
                    file.as_deref(),
                    json,
                ),
            },
            Some(cli::TraitsCommand::Hygiene { trait_files, json }) => {
                handle_hygiene(&trait_files, json)
            }
            Some(cli::TraitsCommand::Cost { file, budget, json }) => {
                handle_cost(&file, budget, json)
            }
            Some(cli::TraitsCommand::PreparePublic { file, json }) => {
                handle_prepare_public(&file, json)
            }
            Some(cli::TraitsCommand::ContextContracts { file, json }) => {
                handle_context_contracts(&file, json)
            }
            Some(cli::TraitsCommand::Policy {
                file,
                profile,
                json,
            }) => handle_policy(&file, &profile, json),
            Some(cli::TraitsCommand::Evidence {
                file,
                profile,
                json,
            }) => handle_evidence(&file, &profile, json),
            Some(cli::TraitsCommand::Compatibility { json }) => handle_compatibility(json),
            Some(cli::TraitsCommand::Subagent {
                file,
                profile,
                json,
            }) => handle_subagent(&file, &profile, json),
            Some(cli::TraitsCommand::Explain {
                trait_arg,
                task,
                scaffold,
                mut trait_files,
                files,
                mode,
                languages,
                signals,
                explicit_invocation,
                active_only,
                json,
                trait_id,
                source_map,
                verbose,
                llm_assisted,
                candidate,
                model,
                budget,
                assignments,
            }) => {
                if let Some(file) = resolve_optional_trait_target(trait_arg.as_deref(), None)? {
                    trait_files.push(file);
                }
                handle_explain(ExplainInputs {
                    task: task.as_deref(),
                    scaffold,
                    trait_files: &trait_files,
                    files: &files,
                    mode: mode.as_deref(),
                    languages: &languages,
                    signals: &signals,
                    explicit_invocation: explicit_invocation.as_deref(),
                    active_only,
                    json,
                    trait_id: trait_id.as_deref(),
                    source_map: source_map.as_deref(),
                    verbose,
                    llm_assisted,
                    candidate_path: candidate.as_deref(),
                    model: model.as_deref(),
                    budget_document: budget.as_deref(),
                    assignments: &assignments,
                })
            }
            Some(cli::TraitsCommand::Inspect {
                trait_arg,
                file,
                dry_plan,
                profile,
            }) => {
                let file = resolve_optional_trait_target(trait_arg.as_deref(), file.as_deref())?;
                handle_inspect(file.as_deref(), dry_plan, profile.as_deref())
            }
            Some(cli::TraitsCommand::TuiDemo) => {
                crate::app::tui_demo::run()?;
                Ok(CommandOutput::new(()))
            }
            Some(cli::TraitsCommand::Edit { trait_arg }) => {
                crate::app::trait_editor::run(&trait_arg)?;
                Ok(CommandOutput::new(()))
            }
            Some(cli::TraitsCommand::Manifest) => {
                let cwd = current_utf8_dir()?;
                let result = ctx_traits_io::discovery::manifest(&cwd)?;

                match &result {
                    ctx_traits_io::discovery::ManifestDiscovery::Found(_) => {
                        println!("{}", format_manifest_discovery(&cwd, &result));
                        Ok(CommandOutput::new(()))
                    }
                    ctx_traits_io::discovery::ManifestDiscovery::NotFound => {
                        eprintln!("{}", format_manifest_discovery(&cwd, &result));
                        Ok(CommandOutput::new(()))
                    }
                    ctx_traits_io::discovery::ManifestDiscovery::Conflict { .. } => {
                        eprintln!("{}", format_manifest_discovery(&cwd, &result));
                        Ok(CommandOutput::new(()))
                    }
                }
            }
            Some(cli::TraitsCommand::Schema {
                protocol,
                format,
                out,
            }) => handle_schema(&protocol, &format, &out),
            Some(cli::TraitsCommand::SdkGenerate { check }) => {
                crate::app::sdk_generate::handle(check)
            }
            Some(cli::TraitsCommand::Synth {
                path,
                format,
                out,
                check,
            }) => handle_synth(&path, &format, out.as_deref(), check),
            Some(cli::TraitsCommand::Build {
                path,
                format,
                out,
                json,
                relock,
            }) => handle_build(&path, &format, out.as_deref(), json, relock),
            Some(cli::TraitsCommand::Migrate {
                id_or_path,
                to,
                apply,
                json,
            }) => crate::app::migrate::handle_migrate(&id_or_path, to.as_deref(), apply, json),
            Some(cli::TraitsCommand::Generate {
                name,
                brief,
                model,
                assignments,
                out,
                candidate,
                check,
                json,
            }) => crate::app::generate::handle(GenerateInputs {
                name: &name,
                brief: &brief,
                model: model.as_deref(),
                assignments: &assignments,
                out: out.as_deref(),
                candidate_path: candidate.as_deref(),
                check,
                json,
            }),
            Some(cli::TraitsCommand::GenerateRound {
                trait_id,
                candidate,
            }) => crate::app::generate::handle_generate_round(&trait_id, &candidate),
            Some(cli::TraitsCommand::RefineRound {
                source_path,
                candidate,
            }) => crate::app::refine::handle_refine_round(&source_path, &candidate),
            Some(cli::TraitsCommand::ImportRound {
                trait_id,
                candidate,
            }) => crate::app::import_handlers::handle_import_round(&trait_id, &candidate),
            Some(cli::TraitsCommand::Refine {
                id_or_path,
                change_request,
                model,
                assignments,
                out,
                apply,
                candidate,
                check,
                json,
            }) => crate::app::refine::handle(RefineInputs {
                id_or_path: &id_or_path,
                change_request: &change_request,
                model: model.as_deref(),
                assignments: &assignments,
                out: out.as_deref(),
                apply,
                candidate_path: candidate.as_deref(),
                check,
                json,
            }),
            Some(cli::TraitsCommand::Critique {
                trait_arg,
                file,
                source_map,
                model,
                assignments,
                candidate,
                json,
            }) => crate::app::generate::critique(CritiqueInputs {
                file: &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "critique")?,
                source_map: source_map.as_deref(),
                model: model.as_deref(),
                assignments: &assignments,
                candidate_path: candidate.as_deref(),
                json,
            }),
            Some(cli::TraitsCommand::Import {
                source,
                profile,
                budget,
                out,
                check,
                llm_assisted,
                model,
                assignments,
                candidate,
                json,
                verbose,
            }) => crate::app::import_handlers::handle_import(
                crate::app::import_handlers::ImportInputs {
                    source: &source,
                    source_profile: profile.as_deref(),
                    out: out.as_deref(),
                    check,
                    llm_assisted,
                    model: model.as_deref(),
                    assignments: &assignments,
                    candidate_path: candidate.as_deref(),
                    budget_document: budget.as_deref(),
                    json,
                    verbose,
                },
            ),
            Some(cli::TraitsCommand::ImportRefresh {
                trait_id_or_package,
                source,
                check,
                out,
                json,
            }) => crate::app::import_handlers::handle_import_refresh(
                &trait_id_or_package,
                source.as_deref(),
                check,
                out.as_deref(),
                json,
            ),
            Some(cli::TraitsCommand::Review {
                trait_arg,
                file,
                approve,
                deny,
                reason,
                json,
            }) => {
                let file = resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "review")?;
                if approve && deny {
                    return Err(crate::Error::Command {
                        message: "review takes --approve or --deny, not both".to_string(),
                    });
                }
                if !approve && !deny {
                    return Err(crate::Error::Command {
                        message: "review requires --approve or --deny".to_string(),
                    });
                }
                let state = if approve {
                    ctx_traits_io::trust::TrustState::Verified
                } else {
                    ctx_traits_io::trust::TrustState::Blocked
                };
                crate::app::lifecycle_reporting::handle_trust_named_update(
                    &file, state, reason, json, "review",
                )
            }
            Some(cli::TraitsCommand::Activate {
                trait_arg,
                file,
                json,
            }) => crate::app::lifecycle_handlers::handle_lifecycle_transition(
                &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "activate")?,
                crate::app::lifecycle_handlers::LifecycleAction::Activate,
                json,
            ),
            Some(cli::TraitsCommand::Deactivate {
                trait_arg,
                file,
                json,
            }) => crate::app::lifecycle_handlers::handle_lifecycle_transition(
                &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "deactivate")?,
                crate::app::lifecycle_handlers::LifecycleAction::Deactivate,
                json,
            ),
            Some(cli::TraitsCommand::Deprecate {
                trait_arg,
                file,
                reason,
                json,
            }) => crate::app::lifecycle_handlers::handle_lifecycle_transition(
                &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "deprecate")?,
                crate::app::lifecycle_handlers::LifecycleAction::Deprecate {
                    reason: reason.as_deref(),
                },
                json,
            ),
            Some(cli::TraitsCommand::RunInfo {
                trait_id,
                file,
                query,
                json,
            }) => crate::app::run::handle_run_info(RunInfoInputs {
                trait_id: trait_id.as_deref(),
                file: file.as_deref(),
                query: &query,
                json,
            }),
            Some(cli::TraitsCommand::Run {
                args,
                no_drive,
                ephemeral,
            }) => {
                let mut progress = crate::app::drive::resolve_progress(
                    args.progress,
                    args.json,
                    args.no_tui,
                );
                // BEFORE anything can touch crossterm. Its event reader is a
                // process-global built ONCE, lazily, on first use — and if
                // stdin is not a terminal at that moment its source is `None`
                // permanently. Repairing stdin later (which is what this call
                // does) cannot revive it: the singleton is never rebuilt, so
                // every later `poll` returns "Failed to initialize input
                // reader" and no key is ever delivered. Adopting here, before
                // the first pane, is what makes the reader come up bound to a
                // real terminal.
                if progress == cli::DriveProgress::Tui && !no_drive && !args.json {
                    crate::app::tui_ratatui::adopt_controlling_terminal();
                }
                // Startup owns a pane only with fully interactive stdio. Keep
                // an explicit TUI mode intact otherwise: drive owns its
                // established allocation fallback and diagnostic.
                let interactive_tui = !no_drive
                    && progress == cli::DriveProgress::Tui
                    && std::io::stdin().is_terminal()
                    && std::io::stdout().is_terminal()
                    && std::io::stderr().is_terminal()
                    // Match the live run renderer: CI, NO_COLOR, and
                    // TERM=dumb are status-only even when attached to a PTY.
                    && crate::app::tui::stderr_supports_live(false);
                let startup = interactive_tui
                .then(crate::app::run_startup_view::StartupView::new)
                .transpose()
                .unwrap_or_else(|error| {
                    progress = cli::DriveProgress::Status;
                    eprintln!("run tui unavailable; falling back to status progress: {error}");
                    None
                });
                if startup.is_none() && !args.json {
                    eprintln!("ctx run · initialization");
                }
                let runtime = ctx_traits_io::harness_config::resolve_runtime_config(
                    camino::Utf8Path::new("."),
                )
                .inspect_err(|error| {
                    if let Some(view) = startup.as_ref() {
                        view.fail(error.to_string());
                    }
                })?;
                if startup
                    .as_ref()
                    .is_some_and(crate::app::run_startup_view::StartupView::interrupted)
                {
                    return Err(crate::Error::Command {
                        message: "run startup interrupted".to_string(),
                    });
                }
                let policy = runtime.effective_run_policy();
                let budget = resolve_run_budget(
                    policy,
                    RunBudgetInputs {
                        max_frames: args.max_frames,
                        frame_seconds: args.frame_seconds,
                        total_seconds: args.total_seconds,
                        max_retries: args.max_retries,
                        attach_wait_seconds: args.attach_wait_seconds,
                        idle_seconds: args.idle_seconds,
                        max_in_flight: args.max_in_flight,
                    },
                );
                let worktree = resolved_run_worktree(
                    policy.worktree,
                    args.worktree
                        .as_ref()
                        .map(|value| value.as_ref().map(String::as_str)),
                    args.no_worktree,
                );
                if ephemeral && !no_drive {
                    if let Some(view) = startup.as_ref() {
                        view.fail("--ephemeral requires --no-drive; driven runs persist their ledger");
                    }
                    return Err(crate::Error::Command {
                        message:
                            "--ephemeral requires --no-drive; driven runs persist their ledger"
                                .to_string(),
                    });
                }
                if no_drive && (args.merge.is_some() || args.no_merge) {
                    return Err(crate::Error::Command {
                        message: "--merge/--no-merge require a driven run; omit --no-drive"
                            .to_string(),
                    });
                }
                if !args.task.is_empty() {
                    let refuse = |message: String| {
                        if let Some(view) = startup.as_ref() {
                            view.fail(message.clone());
                        }
                        crate::Error::Command { message }
                    };
                    if no_drive {
                        return Err(refuse(
                            "--task requires a driven run; omit --no-drive".to_string(),
                        ));
                    }
                    let board_dir = crate::app::tasks::board_dir(None)
                        .inspect_err(|error| {
                            if let Some(view) = startup.as_ref() {
                                view.fail(error.to_string());
                            }
                        })?;
                    let repo_root = resolve_repo_root(None).inspect_err(|error| {
                        if let Some(view) = startup.as_ref() {
                            view.fail(error.to_string());
                        }
                    })?;
                    let provider = ctx_traits_io::task_files::FilesTaskBoard::open_read(board_dir.clone());
                    let queue = crate::app::task_queue::expand_task_queue(&provider, &args.task)
                        .map_err(refuse)?;
                    let dispatch_trait = runtime
                        .effective_dispatch_trait()
                        .ok_or_else(|| {
                            refuse(
                                "--task requires [tasks] dispatch-trait to be configured"
                                    .to_string(),
                            )
                        })?;
                    let merge_policy = runtime.effective_merge_policy();
                    let merge_rung = resolved_merge_intent(merge_policy, args.merge, args.no_merge);
                    let Some(merge_rung) = merge_rung else {
                        return Err(refuse("--task is a merge-gated pipeline and requires an effective merge intent (add --merge, or configure [merge] auto = true)".to_string()));
                    };
                    if worktree.is_none() {
                        return Err(refuse("an effective merge request requires an effective worktree (add --worktree, or configure [worktree] enabled = true)".to_string()));
                    }
                    let story = resolved_story_level(
                        policy.story,
                        args.story
                            .as_ref()
                            .map(|value| value.as_ref().map(String::as_str)),
                        args.no_story,
                    )?;
                    crate::app::run::handle_task_queue_run(crate::app::run::TaskQueueInputs {
                        queue,
                        continue_on_failure: args.continue_on_failure,
                        dispatch_trait,
                        session_store: args.session_store.as_deref(),
                        assignments: &args.assignments,
                        resource_root: args.resource_root.as_deref(),
                        out: args.out.as_deref(),
                        max_frames: budget.max_frames,
                        frame_seconds: budget.frame_seconds,
                        total_seconds: budget.total_seconds,
                        max_retries: budget.max_retries,
                        attach_wait_seconds: budget.attach_wait_seconds,
                        idle_seconds: budget.idle_seconds,
                        max_in_flight: budget.max_in_flight,
                        wait: resolved_run_wait(policy.wait, args.wait, args.no_wait),
                        progress,
                        worktree,
                        strict_loops: resolved_strict_loops(
                            policy.strict_loops,
                            args.strict_loops,
                            args.no_strict_loops,
                        ),
                        override_dependencies: args.override_dependencies,
                        json: args.json,
                        verbose: args.verbose,
                        merge_rung: Some(merge_rung),
                        story,
                        repo_root,
                        board_dir,
                        startup,
                    })
                } else if no_drive {
                    crate::app::run::handle_run(RunInputs {
                        trait_id: args.trait_id.as_deref(),
                        file: args.file.as_deref(),
                        input: args.input.as_deref(),
                        sets: &args.sets,
                        session_store: args.session_store.as_deref(),
                        ephemeral,
                        assignments: &args.assignments,
                        resource_root: args.resource_root.as_deref(),
                        out: args.out.as_deref(),
                        worktree,
                        strict_loops: resolved_strict_loops(
                            policy.strict_loops,
                            args.strict_loops,
                            args.no_strict_loops,
                        ),
                        override_dependencies: args.override_dependencies,
                        task_dispatch: args.task_dispatch,
                        json: args.json,
                        trait_args: &args.trait_args,
                        // `--merge`/`--no-merge` are already rejected above
                        // whenever `no_drive` is set, so `--no-drive` never
                        // requests automatic landing.
                        merge_rung: None,
                        startup_observer: None,
                    })
                } else {
                    let merge_policy = runtime.effective_merge_policy();
                    let merge_rung = resolved_merge_intent(merge_policy, args.merge, args.no_merge);
                    if merge_rung.is_some() && worktree.is_none() {
                        if let Some(view) = startup.as_ref() {
                            view.fail("an effective merge request requires an effective worktree (add --worktree, or configure [worktree] enabled = true)");
                        }
                        return Err(crate::Error::Command {
                            message: "an effective merge request requires an effective worktree (add --worktree, or configure [worktree] enabled = true)".to_string(),
                        });
                    }
                    let story = resolved_story_level(
                        policy.story,
                        args.story
                            .as_ref()
                            .map(|value| value.as_ref().map(String::as_str)),
                        args.no_story,
                    )
                    .inspect_err(|error| {
                        if let Some(view) = startup.as_ref() {
                            view.fail(error.to_string());
                        }
                    })?;
                    crate::app::run::handle_session_start(SessionStartInputs {
                        trait_id: args.trait_id.as_deref(),
                        file: args.file.as_deref(),
                        master: args.master.as_deref(),
                        input: args.input.as_deref(),
                        sets: &args.sets,
                        session_store: args.session_store.as_deref(),
                        assignments: &args.assignments,
                        resource_root: args.resource_root.as_deref(),
                        out: args.out.as_deref(),
                        max_frames: budget.max_frames,
                        frame_seconds: budget.frame_seconds,
                        total_seconds: budget.total_seconds,
                        max_retries: budget.max_retries,
                        attach_wait_seconds: budget.attach_wait_seconds,
                        idle_seconds: budget.idle_seconds,
                        max_in_flight: budget.max_in_flight,
                        wait: resolved_run_wait(policy.wait, args.wait, args.no_wait),
                        progress,
                        worktree,
                        strict_loops: resolved_strict_loops(
                            policy.strict_loops,
                            args.strict_loops,
                            args.no_strict_loops,
                        ),
                        override_dependencies: args.override_dependencies,
                        task_dispatch: args.task_dispatch,
                        json: args.json,
                        verbose: args.verbose,
                        trait_args: &args.trait_args,
                        merge_rung,
                        story,
                        startup,
                    })
                }
            }
            Some(cli::TraitsCommand::Session { subcommand }) => match subcommand {
                cli::SessionCommand::Start(args) => {
                    let runtime = ctx_traits_io::harness_config::resolve_runtime_config(
                        camino::Utf8Path::new("."),
                    )?;
                    let policy = runtime.effective_run_policy();
                    let budget = resolve_run_budget(
                        policy,
                        RunBudgetInputs {
                            max_frames: args.max_frames,
                            frame_seconds: args.frame_seconds,
                            total_seconds: args.total_seconds,
                            max_retries: args.max_retries,
                            attach_wait_seconds: args.attach_wait_seconds,
                            idle_seconds: args.idle_seconds,
                            max_in_flight: args.max_in_flight,
                        },
                    );
                    let worktree = resolved_run_worktree(
                        policy.worktree,
                        args.worktree
                            .as_ref()
                            .map(|value| value.as_ref().map(String::as_str)),
                        args.no_worktree,
                    );
                    let merge_policy = runtime.effective_merge_policy();
                    let merge_rung = resolved_merge_intent(merge_policy, args.merge, args.no_merge);
                    if merge_rung.is_some() && worktree.is_none() {
                        return Err(crate::Error::Command {
                            message: "an effective merge request requires an effective worktree (add --worktree, or configure [worktree] enabled = true)".to_string(),
                        });
                    }
                    crate::app::run::handle_session_start(SessionStartInputs {
                        trait_id: args.trait_id.as_deref(),
                        file: args.file.as_deref(),
                        master: args.master.as_deref(),
                        input: args.input.as_deref(),
                        sets: &args.sets,
                        session_store: args.session_store.as_deref(),
                        assignments: &args.assignments,
                        resource_root: args.resource_root.as_deref(),
                        out: args.out.as_deref(),
                        max_frames: budget.max_frames,
                        frame_seconds: budget.frame_seconds,
                        total_seconds: budget.total_seconds,
                        max_retries: budget.max_retries,
                        attach_wait_seconds: budget.attach_wait_seconds,
                        idle_seconds: budget.idle_seconds,
                        max_in_flight: budget.max_in_flight,
                        wait: resolved_run_wait(policy.wait, args.wait, args.no_wait),
                        progress: crate::app::drive::resolve_progress(
                            args.progress,
                            args.json,
                            args.no_tui,
                        ),
                        worktree,
                        strict_loops: resolved_strict_loops(
                            policy.strict_loops,
                            args.strict_loops,
                            args.no_strict_loops,
                        ),
                        override_dependencies: args.override_dependencies,
                        task_dispatch: args.task_dispatch,
                        json: args.json,
                        verbose: args.verbose,
                        trait_args: &args.trait_args,
                        merge_rung,
                        story: resolved_story_level(
                            policy.story,
                            args.story
                                .as_ref()
                                .map(|value| value.as_ref().map(String::as_str)),
                            args.no_story,
                        )?,
                        startup: None,
                    })
                }
                cli::SessionCommand::State(args) => crate::app::run::handle_run_status(
                    args.file.as_deref(),
                    &args.session,
                    args.session_store.as_deref(),
                    args.json,
                ),
                cli::SessionCommand::Frame { subcommand } => match subcommand {
                    cli::SessionFrameCommand::State {
                        file,
                        session,
                        session_store,
                        agent,
                        json,
                    } => crate::app::run::handle_run_frame(
                        file.as_deref(),
                        &session,
                        session_store.as_deref(),
                        agent.as_deref(),
                        json,
                    ),
                    cli::SessionFrameCommand::Set {
                        session,
                        file,
                        session_store,
                        key,
                        value,
                        value_json,
                        agent,
                        json,
                    } => crate::app::run::handle_set(SetInputs {
                        file: file.as_deref(),
                        session: &session,
                        session_store: session_store.as_deref(),
                        target: &key,
                        value: &value,
                        value_json,
                        agent: agent.as_deref(),
                        json,
                    }),
                },
            },
            Some(cli::TraitsCommand::Mcp) => {
                ctx_traits_io::mcp_server::serve_stdio()?;
                Ok(CommandOutput::new(()))
            }
            Some(cli::TraitsCommand::Drive {
                file,
                session,
                session_store,
                assignments,
                max_frames,
                frame_seconds,
                total_seconds,
                max_retries,
                attach_wait_seconds,
                idle_seconds,
                progress,
                no_tui,
                worktree,
                max_in_flight,
                wait,
                no_wait,
                no_worktree,
                no_merge,
                json,
            }) => {
                let runtime = ctx_traits_io::harness_config::resolve_runtime_config(
                    camino::Utf8Path::new("."),
                )?;
                let policy = runtime.effective_run_policy();
                let budget = resolve_run_budget(
                    policy,
                    RunBudgetInputs {
                        max_frames,
                        frame_seconds,
                        total_seconds,
                        max_retries,
                        attach_wait_seconds,
                        idle_seconds,
                        max_in_flight,
                    },
                );
                // P460: `--no-merge` clears a persisted merge intent before
                // resuming, so this drive completes without landing even
                // when the original `run`/`session start` requested one.
                // `drive()` applies the clear itself, only once this
                // invocation has actually acquired the per-session driver
                // lock (P460 review — a lock-losing invocation must never
                // mutate the ledger a concurrent driver already holds).
                // P549: installed unconditionally — see the analogous
                // comment at `run.rs`'s `session start` call site.
                let panel_handoff = crate::app::drive::PanelHandoff::new();
                let mut report = crate::app::drive::drive(crate::app::drive::DriveInputs {
                    file: file.as_deref(),
                    session: &session,
                    session_store: session_store.as_deref(),
                    assignments: &assignments,
                    max_frames: budget.max_frames,
                    frame_seconds: budget.frame_seconds,
                    total_seconds: budget.total_seconds,
                    max_retries: budget.max_retries,
                    attach_wait_seconds: budget.attach_wait_seconds,
                    idle_seconds: budget.idle_seconds,
                    max_in_flight: budget.max_in_flight,
                    wait: resolved_run_wait(policy.wait, wait, no_wait),
                    progress: crate::app::drive::resolve_progress(progress, json, no_tui),
                    worktree: resolved_run_worktree(
                        policy.worktree,
                        worktree
                            .as_ref()
                            .map(|value| value.as_ref().map(String::as_str)),
                        no_worktree,
                    ),
                    execution_dir: None,
                    clear_merge_intent: no_merge,
                    panel_handoff: Some(panel_handoff.clone()),
                    startup: None,
                    frame_observer: None,
                })?;
                let session_path = ctx_traits_io::run_session::resolve_session_path(
                    &session,
                    session_store.as_deref(),
                )?;
                let final_session =
                    ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
                        trait_file: file.as_deref(),
                        trait_id: None,
                        session: &session,
                        session_store: session_store.as_deref(),
                        elapsed_seconds: None,
                    })?
                    .session;
                let (merge_live, merger_stdout_observer, merge_span_guard) =
                    crate::app::run::merge_live_for_completion(
                        panel_handoff.take(),
                        final_session.run_id.as_str(),
                        final_session.session_id.as_str(),
                        &assignments,
                    );
                let completion = crate::app::run::complete_after_drive(
                    session_store.as_deref(),
                    &session_path,
                    &assignments,
                    final_session,
                    merge_live,
                    merger_stdout_observer,
                )?;
                drop(merge_span_guard);
                report.merge = completion.merge.clone();
                if json {
                    print_json_report(&run_envelope(report, true, true, false), "drive")?;
                } else {
                    crate::app::drive::print_report(&report, Some(&completion.session))?;
                }
                completion.into_command_output()
            }
            Some(cli::TraitsCommand::Merge {
                run_id,
                session_store,
                assignments,
                no_wait,
                wait_override,
                force_merger,
                park_on_overlap,
                land_on_overlap,
                allow_stale_overlap,
                deep,
                json,
            }) => crate::app::merge::handle_merge(crate::app::merge::MergeInputs {
                run_id: &run_id,
                session_store: session_store.as_deref(),
                session_path_override: None,
                assignments: &assignments,
                no_wait,
                force_wait: wait_override,
                json,
                force_merger,
                park_on_overlap: park_on_overlap && !land_on_overlap,
                force_land_on_overlap: land_on_overlap,
                allow_stale_overlap,
                deep,
                live: crate::app::merge::tty_stage_line_live(),
                merger_stdout_observer: None,
            }),
            Some(cli::TraitsCommand::Call {
                file,
                session,
                session_store,
                data,
                out,
                agent,
                json,
            }) => crate::app::run::handle_call(CallInputs {
                file: file.as_deref(),
                session: &session,
                session_store: session_store.as_deref(),
                data: &data,
                out: out.as_deref(),
                agent: agent.as_deref(),
                json,
            }),
            Some(cli::TraitsCommand::RunStatus {
                file,
                session,
                session_store,
                json,
            }) => crate::app::run::handle_run_status(
                file.as_deref(),
                &session,
                session_store.as_deref(),
                json,
            ),
            Some(cli::TraitsCommand::RunFrame {
                file,
                session,
                session_store,
                agent,
                json,
            }) => crate::app::run::handle_run_frame(
                file.as_deref(),
                &session,
                session_store.as_deref(),
                agent.as_deref(),
                json,
            ),
            Some(cli::TraitsCommand::Next {
                agent,
                session,
                session_store,
                wait_seconds,
                peek,
                json,
            }) => crate::app::run::handle_next(
                agent.as_deref(),
                session.as_deref(),
                session_store.as_deref(),
                wait_seconds,
                peek,
                json,
            ),
            Some(cli::TraitsCommand::Set {
                session: set_session,
                file,
                session_store,
                target,
                value,
                value_json,
                agent,
                json,
            }) => {
                let session = set_session
                    .as_deref()
                    .or(session.as_deref())
                    .ok_or_else(|| crate::Error::Command {
                        message: "set requires --session <id-or-path>".to_string(),
                    })?;
                crate::app::run::handle_set(SetInputs {
                    file: file.as_deref(),
                    session,
                    session_store: session_store.as_deref(),
                    target: &target,
                    value: &value,
                    value_json,
                    agent: agent.as_deref(),
                    json,
                })
            }
            Some(cli::TraitsCommand::Eval {
                trait_arg,
                file,
                eval_ids,
                variant,
                out,
                update_lock,
                json,
            }) => crate::app::eval::handle(EvalInputs {
                file: &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "eval")?,
                eval_ids: &eval_ids,
                variant: variant.as_deref(),
                out: out.as_deref(),
                update_lock,
                json,
            }),
            Some(cli::TraitsCommand::Prompt {
                trait_arg,
                allow_unreviewed,
                level,
                json,
            }) => crate::app::report_handlers::handle_prompt(
                &resolve_trait_target(trait_arg.as_deref(), None, "prompt")?,
                allow_unreviewed,
                match level {
                    cli::PromptLevel::Full => ctx_traits_core::resolve::LoadLevel::Full,
                    cli::PromptLevel::Summary => ctx_traits_core::resolve::LoadLevel::Summary,
                },
                json,
            ),
            Some(cli::TraitsCommand::Check {
                trait_arg,
                file,
                locked,
                skip_cdk_drift,
                json,
                plain,
                no_animate,
                verbose,
                run_ledger,
                eval_reports,
            }) => crate::app::report_handlers::handle_check(
                crate::app::report_handlers::CheckInputs {
                    file: &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "check")?,
                    locked,
                    skip_cdk_drift,
                    json,
                    plain,
                    no_animate,
                    verbose,
                    run_ledger: run_ledger.as_deref(),
                    eval_reports: &eval_reports,
                },
            ),
            Some(cli::TraitsCommand::Diff {
                trait_arg,
                file,
                from_lock,
                model_view,
                exports,
                resources,
                json,
                verbose,
            }) => crate::app::report_handlers::handle_diff(
                &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "diff")?,
                from_lock,
                model_view,
                exports,
                resources,
                json,
                verbose,
            ),
            Some(cli::TraitsCommand::Preview {
                trait_arg,
                file,
                step,
                session,
                session_store,
                json,
            }) => crate::app::preview::handle_preview(
                &resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "preview")?,
                step.as_deref(),
                session.as_deref(),
                session_store.as_deref(),
                json,
            ),
            Some(cli::TraitsCommand::Export {
                trait_arg,
                file,
                profile,
                format,
                out,
                update_skill_lock,
                update_gitignore,
                allow_unreviewed,
                json,
            }) => {
                let file = resolve_trait_target(trait_arg.as_deref(), file.as_deref(), "export")?;
                crate::app::report_handlers::handle_export(
                    crate::app::report_handlers::ExportInputs {
                        file: &file,
                        profile: &profile,
                        format: &format,
                        out: out.as_deref(),
                        update_skill_lock,
                        update_gitignore,
                        allow_unreviewed,
                        json,
                    },
                )
            }
            Some(cli::TraitsCommand::Host {
                json: namespace_json,
                subcommand,
            }) => match subcommand {
                cli::HostCommand::Install {
                    trait_arg,
                    file,
                    host,
                    global,
                    format,
                    archive,
                    allow_unreviewed,
                    allow_draft,
                    json,
                } => crate::app::host_install::handle_install(
                    crate::app::host_install::InstallInputs {
                        trait_arg: trait_arg.as_deref(),
                        file: file.as_deref(),
                        host: &host,
                        global,
                        format: format.as_deref(),
                        archive: archive.as_deref(),
                        allow_unreviewed,
                        allow_draft,
                        json: json || namespace_json,
                    },
                ),
                cli::HostCommand::Update {
                    global,
                    force,
                    json,
                } => crate::app::host_install::handle_update(
                    global,
                    force,
                    json || namespace_json,
                ),
                cli::HostCommand::Status { global, json } => {
                    crate::app::host_install::handle_status(global, json || namespace_json)
                }
                cli::HostCommand::Remove {
                    trait_id,
                    host,
                    global,
                    json,
                } => crate::app::host_install::handle_remove(
                    &trait_id,
                    &host,
                    global,
                    json || namespace_json,
                ),
            },
            Some(cli::TraitsCommand::Search {
                query,
                repo_root,
                json,
            }) => crate::app::search::handle(&query, repo_root.as_deref(), json),
            Some(cli::TraitsCommand::Resolve {
                task,
                trait_files,
                repo_root,
                files,
                mode,
                languages,
                budget,
                session,
                explicit_invocation,
                trait_id,
                json,
            }) => crate::app::resolve::handle(ResolveInputs {
                task: &task,
                trait_files: &trait_files,
                repo_root: repo_root.as_deref(),
                files: &files,
                mode: mode.as_deref(),
                languages: &languages,
                budget,
                session: session.as_deref(),
                explicit_invocation: explicit_invocation.as_deref(),
                trait_id: trait_id.as_deref(),
                json,
            }),
            Some(cli::TraitsCommand::Pack {
                task,
                trait_files,
                repo_root,
                profile,
                session,
                budget,
                json,
            }) => handle_pack(
                &task,
                &trait_files,
                repo_root.as_deref(),
                &profile,
                session.as_deref(),
                budget,
                json,
            ),
            Some(cli::TraitsCommand::Context { subcommand }) => match subcommand {
                cli::ContextCommand::Status {
                    host,
                    host_session,
                    json,
                } => handle_context_status(&host, &host_session, json),
                cli::ContextCommand::Plan {
                    host,
                    host_session,
                    task,
                    trait_files,
                    repo_root,
                    files,
                    mode,
                    languages,
                    budget,
                    commit,
                    json,
                } => handle_context_plan(crate::app::context_cache::ContextPlanInputs {
                    host: &host,
                    host_session: &host_session,
                    task: &task,
                    trait_files: &trait_files,
                    repo_root: repo_root.as_deref(),
                    files: &files,
                    mode: mode.as_deref(),
                    languages: &languages,
                    budget,
                    commit,
                    json,
                }),
                cli::ContextCommand::Clear {
                    host,
                    host_session,
                    reason,
                    json,
                } => handle_context_clear(&host, &host_session, reason.as_str(), json),
            },
            Some(cli::TraitsCommand::Hook { host, settings }) => {
                crate::app::hook::handle_hook(host, settings)
            }
            Some(cli::TraitsCommand::Config { json, subcommand }) => {
                match merge_config_json(subcommand, json) {
                    cli::ConfigCommand::Build { path, json } => {
                        crate::app::config_build::handle_config_build(path.as_deref(), json)
                    }
                    cli::ConfigCommand::Accept { yes, json } => {
                        crate::app::config_accept::handle_config_accept(yes, json)
                    }
                    cli::ConfigCommand::Init { global, json } => {
                        crate::app::config_build::handle_config_init(global, json)
                    }
                }
            }
            Some(cli::TraitsCommand::Cache { json, subcommand }) => {
                match merge_cache_json(subcommand, json) {
                    cli::CacheCommand::Rebuild {
                        repo_root,
                        cache_root,
                        json,
                    } => handle_cache_rebuild(repo_root.as_deref(), cache_root.as_deref(), json),
                    cli::CacheCommand::Status {
                        repo_root,
                        cache_root,
                        json,
                    } => handle_cache_status(repo_root.as_deref(), cache_root.as_deref(), json),
                    cli::CacheCommand::Prune {
                        repo_root,
                        cache_root,
                        dry_run,
                        build,
                        build_target,
                        json,
                    } => handle_cache_prune(
                        repo_root.as_deref(),
                        cache_root.as_deref(),
                        dry_run,
                        build,
                        build_target,
                        json,
                    ),
                }
            }
            Some(cli::TraitsCommand::Task { json, subcommand }) => {
                match merge_task_json(subcommand, json) {
                    cli::TaskCommand::Import { path, json } => {
                        crate::app::task::handle_task_import(&path, json)
                    }
                }
            }
            Some(cli::TraitsCommand::Help { json }) => crate::app::help_surface::handle_help(json),
        },
        cli::Command::Tasks { subcommand } => match subcommand {
            None => {
                println!("ctx tasks — use --help to list task board commands");
                Ok(CommandOutput::new(()))
            }
            Some(cli::TasksCommand::Sync { board, json }) => {
                crate::app::tasks::handle_tasks_sync(board.as_deref(), json)
            }
            Some(cli::TasksCommand::Proposals { board, json }) => {
                crate::app::tasks::handle_tasks_proposals(board.as_deref(), json)
            }
            Some(cli::TasksCommand::Reconcile { board, json }) => {
                crate::app::tasks::handle_tasks_reconcile(board.as_deref(), json)
            }
            Some(cli::TasksCommand::List { board, archived, json }) => {
                crate::app::tasks::handle_tasks_list(board.as_deref(), archived, json)
            }
            Some(cli::TasksCommand::Show { task, board, json }) => {
                crate::app::tasks::handle_tasks_show(&task, board.as_deref(), json)
            }
            Some(cli::TasksCommand::Update {
                task,
                board,
                title,
                status,
                content,
                scope,
                validation,
                wall,
                clear_wall,
                origin,
                clear_origin,
                parent,
                clear_parent,
                add_depends_on,
                remove_depends_on,
                step_done,
                step_open,
                release_dependents,
                json,
            }) => crate::app::tasks::handle_tasks_update(
                &task,
                board.as_deref(),
                title,
                status,
                content,
                scope,
                validation,
                wall,
                clear_wall,
                origin,
                clear_origin,
                parent,
                clear_parent,
                add_depends_on,
                remove_depends_on,
                step_done,
                step_open,
                release_dependents,
                json,
            ),
        },
    }
}

/// Resolve the repository root every default (non-`--repo-root`) state and
/// cache consumer must key off of. An explicit `repo_root` always wins
/// unchanged. Otherwise this resolves the invocation Git worktree root
/// (`ctx_traits_io::state::state_repo_root`) rather than the raw current
/// directory, so invoking from any subdirectory of one repository yields the
/// same root — and therefore the same `<repo-key>` — as invoking from the
/// repository root itself.
pub(crate) fn resolve_repo_root(repo_root: Option<&str>) -> crate::Result<camino::Utf8PathBuf> {
    match repo_root {
        Some(path) => Ok(camino::Utf8PathBuf::from(path)),
        None => Ok(ctx_traits_io::state::state_repo_root()?),
    }
}

/// Loaded trait with its source digest, for resolve/pack/cache.
pub(crate) struct LoadedTrait {
    pub(crate) trait_ref: ctx_traits_core::Trait,
    /// The manifest path this trait was loaded from — already available from
    /// `discovery::trait_packages`'s `trait_path` or the `--file` argument,
    /// carried here so `context plan` (P498) can re-render through
    /// `build_render_context` without a second discovery pass.
    pub(crate) trait_path: String,
    pub(crate) source_digest: String,
    /// Resolved from the package manifest's `[package].status`. The
    /// canonical trait document carries no status field of its own.
    pub(crate) status: ctx_traits_core::manifest::PackageStatus,
    /// Resolved from the machine trust store for the trait's canonical
    /// digest. The canonical trait document carries no trust field of its
    /// own.
    pub(crate) trust: ctx_traits_core::r#trait::TrustVerdict,
}

/// Indexed inventory result: loaded full traits, index estimates, and index rejections.
pub(crate) struct IndexedInventory {
    pub(crate) loaded: Vec<LoadedTrait>,
    pub(crate) index_estimates: Vec<ctx_traits_core::resolve::CandidateEstimate>,
    pub(crate) index_rejections: Vec<ctx_traits_core::resolve::IndexRejection>,
    pub(crate) protocol_trait_ids: Vec<String>,
}

/// Discover, index, filter, and full-load trait inventory.
///
/// When explicit `--file` paths are supplied, loads them directly with fallback
/// estimates and no index prefiltering.
/// When no files are supplied, discovers repo-local trait packages, builds discovery
/// index records, applies `Filter` to prefilter, then full-loads
/// only surviving candidates.
pub(crate) fn discover_indexed_trait_inventory(
    trait_files: &[String],
    repo_root: Option<&str>,
    mode: Option<&str>,
    languages: &[String],
    trait_id_filter: Option<&str>,
) -> crate::Result<IndexedInventory> {
    if !trait_files.is_empty() {
        let mut loaded = Vec::new();
        for path_str in trait_files {
            let (trait_ref, trait_root, source_digest, canonical_digest) =
                ctx_traits_io::run::load_trait(path_str)?;
            let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
                &trait_root,
                trait_ref.id.as_str(),
                canonical_digest.as_str(),
            )?;
            loaded.push(LoadedTrait {
                trait_ref,
                trait_path: path_str.clone(),
                source_digest: source_digest.as_str().to_string(),
                status,
                trust,
            });
        }
        let discovered_ids: Vec<String> = loaded
            .iter()
            .map(|l| l.trait_ref.id.as_str().to_string())
            .collect();
        return Ok(IndexedInventory {
            loaded,
            index_estimates: Vec::new(),
            index_rejections: Vec::new(),
            protocol_trait_ids: discovered_ids,
        });
    }

    let root_path = resolve_repo_root(repo_root)?;
    let packages = ctx_traits_io::discovery::trait_packages(&root_path)?;
    let protocol_trait_ids = packages
        .iter()
        .map(|package| package.trait_id.clone())
        .collect();

    // Build index records from discovered trait manifests.
    let mut records = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut trait_texts: std::collections::BTreeMap<
        String,
        (
            String,
            ctx_traits_core::Trait,
            ctx_traits_core::manifest::PackageStatus,
            ctx_traits_core::r#trait::TrustVerdict,
            String,
        ),
    > = std::collections::BTreeMap::new();

    for pkg in &packages {
        let (trait_ref, trait_root, source_digest, canonical_digest) =
            ctx_traits_io::run::load_trait(pkg.trait_path.as_str())?;
        let source_digest_text = source_digest.as_str().to_string();
        let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
            &trait_root,
            trait_ref.id.as_str(),
            canonical_digest.as_str(),
        )?;

        let record = ctx_traits_core::discovery_index::build_index_record(
            &trait_ref,
            &status,
            &trust,
            Some(source_digest.as_str()),
            Some(canonical_digest.as_str()),
            Some(source_digest.as_str()),
            Some(canonical_digest.as_str()),
        );

        trait_texts.insert(
            trait_ref.id.as_str().to_string(),
            (
                source_digest_text,
                trait_ref,
                status,
                trust,
                pkg.trait_path.to_string(),
            ),
        );
        records.push(record);
    }

    // Apply discovery index filter. Only package status `ready` with a
    // `verified` machine trust record survives to full-load prefiltering
    // (Group 95, 2026-07-19): `draft`/`unreviewed`/`blocked` traits do not
    // auto-activate.
    let filter = ctx_traits_core::discovery_index::Filter {
        statuses: vec!["ready".to_string()],
        trusts: vec!["verified".to_string()],
        modes: mode.map(|m| vec![m.to_string()]).unwrap_or_default(),
        languages: languages.to_vec(),
        exclude_stale: true,
        trait_id: trait_id_filter.map(|s| s.to_string()),
    };

    let surviving = ctx_traits_core::discovery_index::filter_index_records(&records, &filter);

    let surviving_ids: std::collections::BTreeSet<&str> =
        surviving.iter().map(|r| r.trait_id.as_str()).collect();

    // Build index rejections for filtered-out records with specific reason codes.
    let mut index_rejections = Vec::new();
    for record in &records {
        if !surviving_ids.contains(record.trait_id.as_str()) {
            let mut reasons = Vec::new();
            let mut remedies = Vec::new();
            if !filter.statuses.is_empty() && !filter.statuses.contains(&record.status) {
                reasons.push(format!("index-status-filtered:{}", record.status));
                let package_status = if record.status == "ready" {
                    ctx_traits_core::manifest::PackageStatus::Ready
                } else {
                    ctx_traits_core::manifest::PackageStatus::Draft
                };
                remedies.extend(
                    ctx_traits_core::r#trait::activation::lifecycle_status_gates_for_check(
                        &record.trait_id,
                        &package_status,
                    )
                    .into_iter()
                    .filter_map(|gate| gate.remedy),
                );
            }
            if !filter.trusts.is_empty() && !filter.trusts.contains(&record.trust) {
                reasons.push(format!("index-trust-filtered:{}", record.trust));
                let trust_verdict = record
                    .trust
                    .parse::<ctx_traits_core::r#trait::TrustVerdict>()
                    .unwrap_or(ctx_traits_core::r#trait::TrustVerdict::Unreviewed);
                remedies.extend(
                    ctx_traits_core::r#trait::activation::trust_gates_for_check(
                        &record.trait_id,
                        &trust_verdict,
                    )
                    .into_iter()
                    .filter_map(|gate| gate.remedy),
                );
            }
            // Mirrors `record_passes_filter`: a record with no declared mode
            // predicate at all, or with at least one OR'd rule that declares
            // no mode predicate of its own, is unconstrained on mode, so it
            // is never the reason a record was excluded.
            if !filter.modes.is_empty()
                && !record.activation.mode_unconstrained
                && !record.activation.modes.is_empty()
                && !has_any_ignore_ascii_case(&record.activation.modes, &filter.modes)
            {
                let modes_str = sorted_join_or_none(&record.activation.modes);
                for requested_mode in &filter.modes {
                    reasons.push(format!(
                        "index-mode-filtered:{} (available: {})",
                        requested_mode, modes_str,
                    ));
                }
            }
            // Same unconstrained-when-absent semantics for language.
            if !filter.languages.is_empty()
                && !record.activation.language_unconstrained
                && !record.activation.languages.is_empty()
                && !has_any_ignore_ascii_case(&record.activation.languages, &filter.languages)
            {
                let langs_str = sorted_join_or_none(&record.activation.languages);
                for requested_lang in &filter.languages {
                    reasons.push(format!(
                        "index-language-filtered:{} (available: {})",
                        requested_lang, langs_str,
                    ));
                }
            }
            if filter.exclude_stale && record.stale {
                reasons.push("index-stale-filtered".to_string());
            }
            if let Some(id) = &filter.trait_id
                && record.trait_id != *id
            {
                reasons.push("index-trait-id-filtered".to_string());
            }
            if reasons.is_empty() {
                reasons.push("index-filtered".to_string());
            }
            remedies.sort();
            remedies.dedup();
            index_rejections.push(ctx_traits_core::resolve::IndexRejection {
                trait_id: record.trait_id.clone(),
                reason_codes: reasons,
                remedies,
            });
        }
    }

    // Full-load only surviving candidates.
    let mut loaded = Vec::new();
    let mut index_estimates = Vec::new();

    for record in &surviving {
        if let Some((loaded_source_digest, trait_ref, status, trust, trait_path)) =
            trait_texts.get(&record.trait_id)
        {
            let source_digest = record
                .source_digest
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| loaded_source_digest.clone());
            loaded.push(LoadedTrait {
                trait_ref: trait_ref.clone(),
                trait_path: trait_path.clone(),
                source_digest,
                status: *status,
                trust: *trust,
            });
            index_estimates.push(ctx_traits_core::resolve::CandidateEstimate {
                trait_id: record.trait_id.clone(),
                estimated_tokens: record.estimated_tokens,
            });
        }
    }

    Ok(IndexedInventory {
        loaded,
        index_estimates,
        index_rejections,
        protocol_trait_ids,
    })
}

/// Discover the indexed trait inventory and run the resolver over it — the
/// one shared "inventory + request + resolve" pipeline every activation
/// entry point (`resolve`, `pack`, `context plan`) needs. Each caller
/// supplies its own fully-populated `Request` (the three verbs differ only
/// in which optional `Request` fields they set — `resolve` sets
/// `trait_id`/`explicit_invocation`, `pack` sets `session_hint`, `context
/// plan` sets neither); the discovery, `traits`/`all_ids`/`lifecycle`
/// projections, and the `resolve::resolve` call itself are identical across
/// all three and must never be re-transcribed at a call site.
pub(crate) fn resolve_activation(
    trait_files: &[String],
    repo_root: Option<&str>,
    mode: Option<&str>,
    languages: &[String],
    trait_id_filter: Option<&str>,
    request: &ctx_traits_core::resolve::Request,
) -> crate::Result<(IndexedInventory, ctx_traits_core::resolve::Response)> {
    let inventory =
        discover_indexed_trait_inventory(trait_files, repo_root, mode, languages, trait_id_filter)?;
    let traits: Vec<ctx_traits_core::Trait> = inventory
        .loaded
        .iter()
        .map(|l| l.trait_ref.clone())
        .collect();

    let all_ids: Vec<&str> = inventory
        .protocol_trait_ids
        .iter()
        .map(|s| s.as_str())
        .collect();

    let lifecycle: Vec<_> = inventory
        .loaded
        .iter()
        .map(|l| (l.status, l.trust))
        .collect();

    let response = ctx_traits_core::resolve::resolve(
        request,
        &traits,
        &lifecycle,
        &inventory.index_estimates,
        &inventory.index_rejections,
        &all_ids,
    );

    Ok((inventory, response))
}

fn has_any_ignore_ascii_case(available: &[String], requested: &[String]) -> bool {
    available.iter().any(|value| {
        requested
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
    })
}

fn sorted_join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    let mut sorted = values.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted.join(",")
}

/// Resolve a positional trait name or explicit `--file` into a trait file
/// path; errors when neither is supplied.
pub(crate) fn resolve_trait_target(
    trait_arg: Option<&str>,
    file: Option<&str>,
    context: &str,
) -> crate::Result<String> {
    resolve_optional_trait_target(trait_arg, file)?.ok_or_else(|| crate::Error::Command {
        message: format!("{context} requires a trait name or --file <path>"),
    })
}

/// P567: the one implementation behind `ctx traits dependency <verb>` and the
/// six hidden legacy verbs it replaced (`vendor`, `install`, `remove`,
/// `update`, `outdated`, `info`). Both spellings route here, so an alias can
/// never drift from the command it aliases.
/// Push a group-level `ctx traits dependency --json` down into whichever
/// subcommand was given, matching how `cache`/`config` treat their own
/// namespace flag: the doctrine is that every command takes `--json`, and a
/// group is a command.
fn merge_dependency_json(
    subcommand: cli::DependencyCommand,
    namespace_json: bool,
) -> cli::DependencyCommand {
    use cli::DependencyCommand as Cmd;
    match subcommand {
        Cmd::Install {
            trait_arg,
            manifest,
            file,
            locked,
            json,
        } => Cmd::Install {
            trait_arg,
            manifest,
            file,
            locked,
            json: json || namespace_json,
        },
        Cmd::Init {
            path,
            name,
            registry,
            access,
            force,
            json,
        } => Cmd::Init {
            path,
            name,
            registry,
            access,
            force,
            json: json || namespace_json,
        },
        Cmd::Add {
            spec,
            alias,
            global,
            trait_ids,
            all,
            json,
        } => Cmd::Add {
            spec,
            alias,
            global,
            trait_ids,
            all,
            json: json || namespace_json,
        },
        Cmd::Remove {
            package,
            global,
            json,
        } => Cmd::Remove {
            package,
            global,
            json: json || namespace_json,
        },
        Cmd::Update {
            package,
            global,
            json,
        } => Cmd::Update {
            package,
            global,
            json: json || namespace_json,
        },
        Cmd::Outdated { json } => Cmd::Outdated {
            json: json || namespace_json,
        },
        Cmd::Info { spec, json } => Cmd::Info {
            spec,
            json: json || namespace_json,
        },
        Cmd::Publish {
            path,
            trait_id,
            dry_run,
            provenance,
            json,
        } => Cmd::Publish {
            path,
            trait_id,
            dry_run,
            provenance,
            json: json || namespace_json,
        },
    }
}

fn handle_dependency(
    subcommand: cli::DependencyCommand,
) -> crate::Result<ctx_traits_core::response::CommandOutput<()>> {
    match subcommand {
        cli::DependencyCommand::Install {
            trait_arg,
            manifest,
            file,
            locked,
            json,
        } => match resolve_optional_trait_target(trait_arg.as_deref(), file.as_deref())? {
            Some(file) => handle_sync(manifest.as_deref(), Some(&file), locked, json),
            None => handle_sync_all(manifest.as_deref(), locked, json),
        },
        cli::DependencyCommand::Add {
            spec,
            alias,
            global,
            trait_ids,
            all,
            json,
        } => crate::app::distribution::handle_install(
            &spec,
            alias.as_deref(),
            global,
            &trait_ids,
            all,
            json,
        ),
        cli::DependencyCommand::Init {
            path,
            name,
            registry,
            access,
            force,
            json,
        } => crate::app::distribution::handle_dependency_init(
            path.as_deref(),
            name.as_deref(),
            registry.as_deref(),
            access.as_deref(),
            force,
            json,
        ),
        cli::DependencyCommand::Remove {
            package,
            global,
            json,
        } => crate::app::distribution::handle_remove(&package, global, json),
        cli::DependencyCommand::Update {
            package,
            global,
            json,
        } => crate::app::distribution::handle_update(package.as_deref(), global, json),
        cli::DependencyCommand::Outdated { json } => {
            crate::app::distribution::handle_outdated(json)
        }
        cli::DependencyCommand::Info { spec, json } => {
            crate::app::distribution::handle_info(&spec, json)
        }
        cli::DependencyCommand::Publish {
            path,
            trait_id,
            dry_run,
            provenance,
            json,
        } => crate::app::distribution::handle_publish(
            path.as_deref(),
            trait_id.as_deref(),
            dry_run,
            provenance,
            json,
        ),
    }
}

/// Resolve a positional trait name or explicit `--file`, allowing neither.
///
/// An extension-less positional resolves as a trait name from the repo-local
/// trait source root; anything with an extension is treated as a literal
/// file path.
pub(crate) fn resolve_optional_trait_target(
    trait_arg: Option<&str>,
    file: Option<&str>,
) -> crate::Result<Option<String>> {
    match (trait_arg, file) {
        (Some(_), Some(_)) => Err(crate::Error::Command {
            message: "pass a trait name or --file <path>, not both".to_string(),
        }),
        (None, Some(file)) => Ok(Some(file.to_string())),
        (Some(id_or_path), None) => {
            if camino::Utf8Path::new(id_or_path).extension().is_none() {
                Ok(Some(
                    ctx_traits_io::run::resolve_trait_path(None, Some(id_or_path), "trait")?
                        .0
                        .to_string(),
                ))
            } else {
                Ok(Some(id_or_path.to_string()))
            }
        }
        (None, None) => Ok(None),
    }
}
