//! Context packing, ledger status/plan/clear, and cache lifecycle commands.

use ctx_traits_core::response::{CommandOutput, Envelope};

use crate::app::command_handlers::{
    discover_indexed_trait_inventory, print_json_report, resolve_activation, resolve_repo_root,
};
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextLedgerEntryReport<'a> {
    trait_id: &'a str,
    version: &'a str,
    content_digest: &'a str,
    load_level: &'a str,
    approximate_tokens: u64,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_reason: Option<&'a str>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextStatusReport<'a> {
    host: &'a str,
    host_session: &'a str,
    ledger_state: &'a str,
    entries: Vec<ContextLedgerEntryReport<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cleared_reason: Option<&'a str>,
    note: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CachePruneReport<'a> {
    action: &'static str,
    dry_run: bool,
    prune_mode: &'static str,
    metadata_action: &'a str,
    metadata_changed: bool,
    plan: &'a ctx_traits_core::cache::CachePlan,
    note: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct BuildCachePruneEntry<'a> {
    name: &'a str,
    path: &'a str,
    existed: bool,
    byte_size: u64,
    removed: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct BuildCachePruneReport<'a> {
    action: &'static str,
    dry_run: bool,
    prune_mode: &'static str,
    caches: Vec<BuildCachePruneEntry<'a>>,
    note: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CachePlanReport<'a> {
    action: &'a str,
    cache_artifact_mode: &'static str,
    plan: &'a ctx_traits_core::cache::CachePlan,
    note: &'static str,
}

pub(crate) fn handle_pack(
    task: &str,
    trait_files: &[String],
    repo_root: Option<&str>,
    profile: &str,
    session: Option<&str>,
    budget: Option<u64>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let render_profile = ctx_traits_core::render::ExtendedRenderProfile::parse(profile)
        .ok_or_else(|| crate::Error::Command {
            message: format!("unsupported render profile {profile:?}"),
        })?;

    let resolve_request = ctx_traits_core::resolve::Request {
        task: task.to_string(),
        budget_tokens: budget,
        session_hint: session.map(|s| s.to_string()),
        ..Default::default()
    };

    let (inventory, resolve_response) =
        resolve_activation(trait_files, repo_root, None, &[], None, &resolve_request)?;

    let traits: Vec<ctx_traits_core::Trait> = inventory
        .loaded
        .iter()
        .map(|l| l.trait_ref.clone())
        .collect();

    let source_digest_refs: Vec<(&str, &str)> = inventory
        .loaded
        .iter()
        .map(|l| (l.trait_ref.id.as_str(), l.source_digest.as_str()))
        .collect();

    let pack = ctx_traits_core::context::pack::plan_context_pack(
        &resolve_response,
        &traits,
        render_profile,
        &source_digest_refs,
    );

    if json {
        let json_text = serde_json::to_string_pretty(&pack)
            .map_err(|e| crate::Error::json("serialize context pack", e))?;
        println!("{json_text}");
    } else {
        println!("ctx traits pack");
        println!("  task: {task}");
        if let Some(session) = session {
            println!("  session: {session}");
        }
        println!("  profile: {profile}");
        println!("  frames: {}", pack.frames.len());
        for frame in &pack.frames {
            let source_digest = frame.source_digest.as_deref().unwrap_or("missing");
            let content_digest = frame.content_digest.as_deref().unwrap_or("none");
            let duplicate_of = frame
                .duplicate_of_content_digest
                .as_deref()
                .unwrap_or("none");
            println!(
                "    {} load={} tokens={} dedup={} source-digest={} content-digest={} duplicate-of={}",
                frame.trait_id,
                frame.load_level,
                frame.estimated_tokens,
                frame.deduplication_status,
                source_digest,
                content_digest,
                duplicate_of,
            );
        }
        println!("  total-tokens: {}", pack.total_estimated_tokens);
        println!(
            "  warning: [{} capability={} status={}] {}",
            pack.host_injection_warning.code,
            pack.host_injection_warning.capability,
            pack.host_injection_warning.status,
            pack.host_injection_warning.message,
        );
        if !pack.warnings.is_empty() {
            println!("  warnings:");
            for warning in &pack.warnings {
                println!("    {warning}");
            }
        }
        if !pack.de_duplication_notes.is_empty() {
            println!("  de-duplication:");
            for note in &pack.de_duplication_notes {
                println!("    {note}");
            }
        }
    }

    Ok(CommandOutput::new(()))
}

// ---------------------------------------------------------------------------
// P498: Session context ledger — status / plan / clear
// ---------------------------------------------------------------------------

/// The standing non-claim carried verbatim into every context-ledger report
/// (mirrors `ctx_traits_core::context::pack::HostInjectionWarning`): a
/// ledger entry is evidence of what this process supplied, never proof the
/// model retained it.
const LEDGER_NON_CLAIM: &str =
    "context ledger is operational evidence of what was supplied, not proof of model memory";

pub(crate) fn handle_context_status(
    host: &str,
    host_session: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let host_key = ctx_traits_io::context_ledger::HostKey::new(host, host_session)?;
    let ledger = ctx_traits_io::context_ledger::read(&host_key)?;
    let ledger_state = if ledger.entries.is_empty() {
        ctx_traits_core::context::ledger::State::Missing
    } else {
        ctx_traits_core::context::ledger::State::Loaded
    };
    let entries: Vec<ContextLedgerEntryReport<'_>> = ledger
        .entries
        .iter()
        .map(|entry| ContextLedgerEntryReport {
            trait_id: entry.trait_id.as_str(),
            version: entry.version.as_str(),
            content_digest: entry.content_digest.as_str(),
            load_level: entry.load_level.as_str(),
            approximate_tokens: entry.approximate_tokens,
            stale: entry.stale,
            stale_reason: entry.stale_reason.as_ref().map(|r| r.as_str()),
        })
        .collect();

    if json {
        let output = ContextStatusReport {
            host,
            host_session,
            ledger_state: ledger_state.as_str(),
            entries,
            last_cleared_reason: ledger.last_cleared_reason.as_deref(),
            note: LEDGER_NON_CLAIM,
        };
        print_json_report(&Envelope::ok(output), "context status")?;
    } else {
        println!("ctx traits context status");
        println!("  host: {host}");
        println!("  host-session: {host_session}");
        println!("  ledger-state: {}", ledger_state.as_str());
        println!("  entries: {}", entries.len());
        for entry in &entries {
            println!(
                "    {} ({}) load={} tokens={} stale={} reason={}",
                entry.trait_id,
                entry.version,
                entry.load_level,
                entry.approximate_tokens,
                entry.stale,
                entry.stale_reason.unwrap_or("none"),
            );
        }
        if let Some(reason) = ledger.last_cleared_reason.as_deref() {
            println!("  last-cleared-reason: {reason}");
        }
        println!("  note: {LEDGER_NON_CLAIM}");
    }

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextPlanRowReport<'a> {
    trait_id: &'a str,
    model_view_digest: &'a str,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_reason: Option<&'a str>,
    text: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextPlanReport<'a> {
    host: &'a str,
    host_session: &'a str,
    committed: bool,
    rows: Vec<ContextPlanRowReport<'a>>,
    note: &'static str,
}

pub(crate) struct ContextPlanInputs<'a> {
    pub(crate) host: &'a str,
    pub(crate) host_session: &'a str,
    pub(crate) task: &'a str,
    pub(crate) trait_files: &'a [String],
    pub(crate) repo_root: Option<&'a str>,
    pub(crate) files: &'a [String],
    pub(crate) mode: Option<&'a str>,
    pub(crate) languages: &'a [String],
    pub(crate) budget: Option<u64>,
    pub(crate) commit: bool,
    pub(crate) json: bool,
}

/// One trait rendered through the exact path `ctx traits prompt` uses (P498
/// decision on render fidelity): the injected bytes and their digest must be
/// byte-identical to what an adapter's follow-up `prompt` call would yield,
/// or the ledger's freshness claim breaks. Shared by every render call site
/// (`context plan`'s task path, the P499 hook's task and ledger-restore
/// paths) so the render/digest logic cannot drift between them.
pub(crate) struct RenderedTrait {
    pub(crate) trait_id: String,
    pub(crate) version: String,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) model_view_digest: ctx_traits_core::digest::Digest,
    pub(crate) estimated_tokens: u64,
    pub(crate) load_level: String,
    pub(crate) text: String,
}

/// A trait selected for rendering, independent of where the selection came
/// from (resolver-selected candidate vs. a ledger entry's remembered id).
pub(crate) struct RenderCandidate {
    pub(crate) trait_id: String,
    pub(crate) version: String,
    pub(crate) load_level: String,
    pub(crate) trait_path: String,
}

/// Render every `candidates` entry through `build_render_context`. In strict
/// mode (`tolerant = false`, the resolver-selected task path) a render/trust
/// refusal propagates — the resolver already gated lifecycle/trust before
/// selection, so this is a state that cannot occur. In tolerant mode
/// (`tolerant = true`, the P499 hook's `SessionStart(compact)` restore path,
/// D6) a refusal is reported on stderr and that one trait is skipped rather
/// than failing the whole restore, since a trait can go trust-blocked after
/// it was originally injected.
pub(crate) fn render_candidates(
    candidates: &[RenderCandidate],
    json: bool,
    tolerant: bool,
) -> crate::Result<Vec<RenderedTrait>> {
    let mut rendered = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let load_level = match candidate.load_level.as_str() {
            "summary" => ctx_traits_core::resolve::LoadLevel::Summary,
            "full" => ctx_traits_core::resolve::LoadLevel::Full,
            "discovery" => continue,
            other if tolerant => {
                eprintln!(
                    "ctx traits hook: skipping {} on restore (unsupported load level {other:?})",
                    candidate.trait_id
                );
                continue;
            }
            other => {
                return Err(crate::Error::Command {
                    message: format!(
                        "context plan: selected trait {} has unsupported load level {other:?}",
                        candidate.trait_id
                    ),
                });
            }
        };
        let posture = crate::app::report_render::RenderTrustPosture::prompt(false, json);
        match crate::app::report_render::build_render_context(
            candidate.trait_path.as_str(),
            ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
            &posture,
        ) {
            Ok(render_context) => {
                let (text, model_view_digest) = render_context
                    .plan
                    .model_view
                    .artifact_for_load_level(load_level)
                    .expect("validated content-bearing level has an artifact");
                rendered.push(RenderedTrait {
                    trait_id: candidate.trait_id.clone(),
                    version: candidate.version.clone(),
                    source_digest: render_context.source_digest.clone(),
                    model_view_digest: model_view_digest.clone(),
                    estimated_tokens: ctx_traits_core::discovery_index::estimate_tokens(text),
                    load_level: candidate.load_level.clone(),
                    text: text.to_string(),
                });
            }
            Err(error) if tolerant => {
                eprintln!(
                    "ctx traits hook: skipping {} on restore ({error})",
                    candidate.trait_id
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(rendered)
}

/// One `context plan` decision, host-key-agnostic: the trait's rendered
/// identity, the dedup decision against the ledger, and the exact bytes to
/// inject. Shared between `context plan`'s printer and the P499 hook, which
/// caps/filters this list before emitting and commits only the emitted
/// subset (never "all rendered rows" — a capped-out trait must never be
/// marked fresh).
pub(crate) struct PlannedRow {
    pub(crate) trait_id: String,
    pub(crate) version: String,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) model_view_digest: ctx_traits_core::digest::Digest,
    pub(crate) estimated_tokens: u64,
    pub(crate) load_level: String,
    pub(crate) action: ctx_traits_core::context::ledger::Action,
    pub(crate) stale_reason: Option<ctx_traits_core::context::ledger::StaleReason>,
    pub(crate) text: String,
}

pub(crate) struct PlanFromTaskInputs<'a> {
    pub(crate) host_key: &'a ctx_traits_io::context_ledger::HostKey,
    pub(crate) task: &'a str,
    pub(crate) trait_files: &'a [String],
    pub(crate) repo_root: Option<&'a str>,
    pub(crate) files: &'a [String],
    pub(crate) mode: Option<&'a str>,
    pub(crate) languages: &'a [String],
    pub(crate) budget: Option<u64>,
    pub(crate) json: bool,
}

/// Resolve `task` against the trait inventory, render every selected trait,
/// and decide `inject`/`skip-fresh`/`reinject` for each against the
/// persisted ledger. Touches no ledger state — the caller commits whichever
/// subset of rows it actually emits.
pub(crate) fn plan_from_task(input: PlanFromTaskInputs<'_>) -> crate::Result<Vec<PlannedRow>> {
    let request = ctx_traits_core::resolve::Request {
        task: input.task.to_string(),
        files: input.files.to_vec(),
        mode: input.mode.map(|s| s.to_string()),
        language_hints: input.languages.to_vec(),
        budget_tokens: input.budget,
        ..Default::default()
    };

    let (inventory, response) = resolve_activation(
        input.trait_files,
        input.repo_root,
        input.mode,
        input.languages,
        None,
        &request,
    )?;

    let candidates: Vec<RenderCandidate> = response
        .selected
        .iter()
        .map(|candidate| {
            let loaded_trait = inventory
                .loaded
                .iter()
                .find(|l| l.trait_ref.id.as_str() == candidate.trait_id.as_str())
                .ok_or_else(|| crate::Error::Command {
                    message: format!(
                        "context plan: selected trait {} has no loaded trait path",
                        candidate.trait_id
                    ),
                })?;
            Ok(RenderCandidate {
                trait_id: candidate.trait_id.clone(),
                version: candidate.version.clone(),
                load_level: candidate.load_level.clone(),
                trait_path: loaded_trait.trait_path.clone(),
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;

    let rendered = render_candidates(&candidates, input.json, false)?;

    let current: Vec<ctx_traits_core::context::ledger::CurrentRender> = rendered
        .iter()
        .map(|r| ctx_traits_core::context::ledger::CurrentRender {
            trait_id: r.trait_id.clone(),
            model_view_digest: r.model_view_digest.clone(),
            load_level: r.load_level.clone(),
        })
        .collect();

    let ledger = ctx_traits_io::context_ledger::read(input.host_key)?;
    let decisions = ctx_traits_core::context::ledger::plan_actions(
        &ledger,
        &current,
        &input.host_key.combined(),
    );

    Ok(rendered
        .into_iter()
        .map(|r| {
            let decision = decisions
                .iter()
                .find(|d| d.trait_id == r.trait_id)
                .expect("plan_actions returns one decision per current render");
            PlannedRow {
                trait_id: r.trait_id,
                version: r.version,
                source_digest: r.source_digest,
                model_view_digest: r.model_view_digest,
                estimated_tokens: r.estimated_tokens,
                load_level: r.load_level,
                action: decision.action,
                stale_reason: decision.stale_reason.clone(),
                text: r.text,
            }
        })
        .collect())
}

/// Re-render exactly the traits the persisted ledger says this host session
/// was already given (the P499 `SessionStart(compact)` repair, D4). A trait
/// no longer discoverable, or one that now refuses the render-trust gate
/// (D6), is skipped with a stderr line rather than failing the whole
/// restore. Every row's `action` is `Inject` — this path re-delivers a
/// remembered set, it does not compare against the ledger it just read.
pub(crate) fn plan_from_ledger(
    host_key: &ctx_traits_io::context_ledger::HostKey,
    trait_files: &[String],
    repo_root: Option<&str>,
) -> crate::Result<Vec<PlannedRow>> {
    let ledger = ctx_traits_io::context_ledger::read(host_key)?;
    if ledger.entries.is_empty() {
        return Ok(Vec::new());
    }

    let inventory = discover_indexed_trait_inventory(trait_files, repo_root, None, &[], None)?;

    let mut candidates = Vec::with_capacity(ledger.entries.len());
    for entry in &ledger.entries {
        match inventory
            .loaded
            .iter()
            .find(|l| l.trait_ref.id.as_str() == entry.trait_id.as_str())
        {
            Some(loaded_trait) => candidates.push(RenderCandidate {
                trait_id: entry.trait_id.clone(),
                version: loaded_trait.trait_ref.version.as_str().to_string(),
                load_level: entry.load_level.clone(),
                trait_path: loaded_trait.trait_path.clone(),
            }),
            None => eprintln!(
                "ctx traits hook: skipping {} on restore (no longer discoverable)",
                entry.trait_id
            ),
        }
    }

    let rendered = render_candidates(&candidates, false, true)?;

    Ok(rendered
        .into_iter()
        .map(|r| PlannedRow {
            trait_id: r.trait_id,
            version: r.version,
            source_digest: r.source_digest,
            model_view_digest: r.model_view_digest,
            estimated_tokens: r.estimated_tokens,
            load_level: r.load_level,
            action: ctx_traits_core::context::ledger::Action::Inject,
            stale_reason: None,
            text: r.text,
        })
        .collect())
}

/// Commit `rows` to the ledger under `host_key` (the `--commit` / hook
/// write path): construct one [`ctx_traits_core::context::ledger::Entry`]
/// per row and upsert. Callers pass only the subset actually emitted — a
/// trait dropped by a cap must never be committed, or it is marked fresh and
/// never injected again.
pub(crate) fn commit_rows(
    host_key: &ctx_traits_io::context_ledger::HostKey,
    rows: &[&PlannedRow],
) -> crate::Result<()> {
    let entries: Vec<ctx_traits_core::context::ledger::Entry> = rows
        .iter()
        .map(|r| ctx_traits_core::context::ledger::Entry {
            session_id: host_key.host_session().to_string(),
            trait_id: r.trait_id.clone(),
            version: r.version.clone(),
            source_digest: r.source_digest.clone(),
            render_profile: "agent-skills".to_string(),
            load_level: r.load_level.clone(),
            content_digest: r.model_view_digest.clone(),
            approximate_tokens: r.estimated_tokens,
            injected_turn: None,
            host_key: Some(host_key.combined()),
            stale: false,
            stale_reason: None,
        })
        .collect();
    ctx_traits_io::context_ledger::upsert_entries(host_key, entries)?;
    Ok(())
}

pub(crate) fn handle_context_plan(
    input: ContextPlanInputs<'_>,
) -> crate::Result<CommandOutput<()>> {
    let host_key = ctx_traits_io::context_ledger::HostKey::new(input.host, input.host_session)?;

    let rows = plan_from_task(PlanFromTaskInputs {
        host_key: &host_key,
        task: input.task,
        trait_files: input.trait_files,
        repo_root: input.repo_root,
        files: input.files,
        mode: input.mode,
        languages: input.languages,
        budget: input.budget,
        json: input.json,
    })?;

    if input.commit {
        let all: Vec<&PlannedRow> = rows.iter().collect();
        commit_rows(&host_key, &all)?;
    }

    let rows: Vec<ContextPlanRowReport<'_>> = rows
        .iter()
        .map(|r| ContextPlanRowReport {
            trait_id: r.trait_id.as_str(),
            model_view_digest: r.model_view_digest.as_str(),
            action: r.action.as_str(),
            stale_reason: r.stale_reason.as_ref().map(|reason| reason.as_str()),
            text: r.text.as_str(),
        })
        .collect();

    if input.json {
        let output = ContextPlanReport {
            host: input.host,
            host_session: input.host_session,
            committed: input.commit,
            rows,
            note: LEDGER_NON_CLAIM,
        };
        print_json_report(&Envelope::ok(output), "context plan")?;
    } else {
        println!("ctx traits context plan");
        println!("  host: {}", input.host);
        println!("  host-session: {}", input.host_session);
        println!("  committed: {}", input.commit);
        for row in &rows {
            println!(
                "    {} action={} reason={} digest={}",
                row.trait_id,
                row.action,
                row.stale_reason.unwrap_or("none"),
                row.model_view_digest,
            );
        }
        println!("  note: {LEDGER_NON_CLAIM}");
    }

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextClearReport<'a> {
    host: &'a str,
    host_session: &'a str,
    reason: &'a str,
    cleared_entry_count: usize,
    note: &'static str,
}

pub(crate) fn handle_context_clear(
    host: &str,
    host_session: &str,
    reason: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let host_key = ctx_traits_io::context_ledger::HostKey::new(host, host_session)?;
    let cleared_entry_count = ctx_traits_io::context_ledger::clear(&host_key, reason)?;

    if json {
        let output = ContextClearReport {
            host,
            host_session,
            reason,
            cleared_entry_count,
            note: LEDGER_NON_CLAIM,
        };
        print_json_report(&Envelope::ok(output), "context clear")?;
    } else {
        println!("ctx traits context clear");
        println!("  host: {host}");
        println!("  host-session: {host_session}");
        println!("  reason: {reason}");
        println!("  cleared-entry-count: {cleared_entry_count}");
        println!("  note: {LEDGER_NON_CLAIM}");
    }

    Ok(CommandOutput::new(()))
}

// ---------------------------------------------------------------------------
// P65: Cache lifecycle
// ---------------------------------------------------------------------------

fn build_cache_keys_from_repo(
    repo_root: &camino::Utf8Path,
) -> crate::Result<Vec<ctx_traits_core::cache::CacheArtifactKey>> {
    let packages = ctx_traits_io::discovery::trait_packages(repo_root)?;
    let mut keys = Vec::new();
    for pkg in &packages {
        let (trait_ref, _trait_root, source_digest, _canonical_digest) =
            ctx_traits_io::run::load_trait(pkg.trait_path.as_str())?;
        keys.push(ctx_traits_core::cache::CacheArtifactKey {
            trait_id: trait_ref.id.as_str().to_string(),
            version: trait_ref.version.as_str().to_string(),
            source_digest,
            render_profile: "agent-skills".to_string(),
            load_level: "discovery".to_string(),
            resource_digests: Vec::new(),
            resource_digest_coverage: "not-loaded".to_string(),
        });
    }
    Ok(keys)
}
pub(crate) fn handle_cache_rebuild(
    repo_root: Option<&str>,
    cache_root_override: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let root_path = resolve_repo_root(repo_root)?;
    let cache_root = ctx_traits_io::cache::cache_root(&root_path, cache_root_override)?;
    let keys = build_cache_keys_from_repo(&root_path)?;

    let artifacts: Vec<ctx_traits_io::cache::StoredCacheArtifact> = keys
        .iter()
        .map(|k| ctx_traits_io::cache::StoredCacheArtifact {
            artifact_id: k.artifact_id(),
            key: k.clone(),
            artifact_size: None,
        })
        .collect();
    let metadata = ctx_traits_io::cache::StoredCacheMetadata { artifacts };
    ctx_traits_io::cache::write_cache_metadata(&cache_root, &metadata)?;

    let plan = ctx_traits_core::cache::plan_cache_rebuild(&keys);
    emit_cache_plan("rebuild", &plan, json)
}
pub(crate) fn handle_cache_status(
    repo_root: Option<&str>,
    cache_root_override: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let root_path = resolve_repo_root(repo_root)?;
    let cache_root = ctx_traits_io::cache::cache_read_root(&root_path, cache_root_override)?;
    let current_keys = build_cache_keys_from_repo(&root_path)?;
    let stored = ctx_traits_io::cache::read_cache_metadata(&cache_root)?;

    let plan = ctx_traits_core::cache::compare_cache_status(&stored.artifacts, &current_keys);
    emit_cache_plan("status", &plan, json)
}
pub(crate) fn handle_cache_prune(
    repo_root: Option<&str>,
    cache_root_override: Option<&str>,
    dry_run: bool,
    build: Option<Option<String>>,
    build_target: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    // Opt-in build-cache prune is a distinct, constrained destructive path,
    // normalized from both CLI spellings into one selector before dispatch:
    // `--build=<name>` selects one declared cache, bare `--build` (or the
    // hidden `--build-target` compatibility alias) selects every declared
    // cache. It returns before any of the metadata-prune logic runs and
    // before `resolve_repo_root` (which falls back to CWD, not the
    // invocation git root) is even consulted, so ordinary cache prune output
    // stays byte-identical when neither flag is passed.
    if build.is_some() && build_target {
        return Err(crate::Error::Command {
            message: "cache prune accepts either --build or --build-target, not both".to_string(),
        });
    }
    if build_target {
        if cache_root_override.is_some() {
            return Err(crate::Error::Command {
                message: "cache prune --build-target does not accept --cache-root; the build-target directory is always this repository's global build-cache root".to_string(),
            });
        }
        let repo_root = resolve_build_cache_repo_root(repo_root)?;
        return handle_cache_prune_build_target(&repo_root, dry_run, json);
    }
    let selector = build;
    if let Some(name) = selector {
        if cache_root_override.is_some() {
            return Err(crate::Error::Command {
                message: "cache prune --build does not accept --cache-root; a build cache's directory is always this repository's global build-cache root".to_string(),
            });
        }
        let repo_root = resolve_build_cache_repo_root(repo_root)?;
        return handle_cache_prune_build_cache(&repo_root, name.as_deref(), dry_run, json);
    }
    let root_path = resolve_repo_root(repo_root)?;
    // Pruning always writes to the global cache root, but may read stale
    // metadata from the one-release legacy fallback (never write there): a
    // repository upgraded to P426 with only legacy metadata still gets a
    // correct prune plan, and any resulting write lands in global state.
    let cache_root = ctx_traits_io::cache::cache_root(&root_path, cache_root_override)?;
    let cache_read_root = ctx_traits_io::cache::cache_read_root(&root_path, cache_root_override)?;
    let current_keys = build_cache_keys_from_repo(&root_path)?;
    let stored = ctx_traits_io::cache::read_cache_metadata(&cache_read_root)?;

    let plan =
        ctx_traits_core::cache::plan_cache_prune_from_stored(&stored.artifacts, &current_keys);
    let metadata_action = if dry_run {
        "dry-run-no-change"
    } else if plan.prune_count > 0 {
        "removed-stale-unreachable-records"
    } else {
        "no-stale-unreachable-records"
    };
    let metadata_changed = !dry_run && plan.prune_count > 0;

    if !dry_run && plan.prune_count > 0 {
        let surviving = ctx_traits_core::cache::surviving_artifacts(&plan);
        if surviving.is_empty() {
            ctx_traits_io::cache::remove_cache_metadata(&cache_root)?;
        } else {
            let updated = ctx_traits_io::cache::StoredCacheMetadata {
                artifacts: surviving,
            };
            ctx_traits_io::cache::write_cache_metadata(&cache_root, &updated)?;
        }
    }
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = CachePruneReport {
                action: "prune",
                dry_run,
                prune_mode: "metadata-only",
                metadata_action,
                metadata_changed,
                plan: &plan,
                note: "cache commands never mutate canonical trait state",
            };
            print_json_report(&Envelope::ok(output), "cache prune plan")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new(
                "ctx",
                "cache prune",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "dry-run",
                dry_run.to_string(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "prune-mode",
                "metadata-only",
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "metadata-action",
                metadata_action,
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "metadata-changed",
                metadata_changed.to_string(),
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "note",
                "cache commands never mutate canonical trait state",
                RowTone::Default,
            ))
            .row(PanelRow::toned(
                "counts",
                format!(
                    "total: {} · fresh: {} · stale: {} · prune: {}",
                    plan.total_count, plan.fresh_count, plan.stale_count, plan.prune_count
                ),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

fn handle_cache_prune_build_target(
    root_path: &camino::Utf8Path,
    dry_run: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let outcome = ctx_traits_io::cache::prune_build_target_cache(root_path, dry_run)?;
    emit_build_cache_prune_report("build-target", dry_run, &[outcome], json)
}

/// Resolve the repository root for `--build`/`--build-target` prune: an
/// explicit `--repo-root` override still wins (matching `resolve_repo_root`'s
/// contract for every other cache subcommand), but the no-override default
/// discovers the invocation git repository root (`git rev-parse
/// --show-toplevel`, the same helper the `[worktree].env` overlay uses)
/// instead of falling back to the CWD like `resolve_repo_root` does. Without
/// this, `cache prune --build` run from a nested subdirectory would key its
/// global build-cache root off the wrong (subdirectory) canonical path
/// instead of the one true repository root's.
fn resolve_build_cache_repo_root(repo_root: Option<&str>) -> crate::Result<camino::Utf8PathBuf> {
    let selected = match repo_root {
        Some(path) => camino::Utf8PathBuf::from(path),
        None => ctx_traits_io::repository::discover_repo_root()?,
    };
    Ok(ctx_traits_io::repository::discover_main_repo_root(
        &selected,
    )?)
}

/// Prune one declared build cache (`name` given) or every declared build
/// cache (`name` absent). Both the one-name and all-names paths derive their
/// targets from the SAME effective `[run.build-cache.<name>]` declaration set
/// — resolved once here, rooted at `root_path` (the selected repository, not
/// the invocation CWD) — so an explicit undeclared name is rejected before
/// any path is touched, and an undeclared sibling directory is never
/// scanned or deleted.
fn handle_cache_prune_build_cache(
    root_path: &camino::Utf8Path,
    name: Option<&str>,
    dry_run: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let declared = declared_build_cache_names(root_path)?;
    let outcomes = match name {
        Some(name) => vec![ctx_traits_io::cache::prune_named_build_cache(
            root_path, name, &declared, dry_run,
        )?],
        None => ctx_traits_io::cache::prune_declared_build_caches(root_path, &declared, dry_run)?,
    };
    emit_build_cache_prune_report("build-cache", dry_run, &outcomes, json)
}

fn emit_build_cache_prune_report(
    prune_mode: &'static str,
    dry_run: bool,
    outcomes: &[ctx_traits_io::cache::BuildCachePruneOutcome],
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = BuildCachePruneReport {
                action: "prune",
                dry_run,
                prune_mode,
                caches: outcomes
                    .iter()
                    .map(|outcome| BuildCachePruneEntry {
                        name: outcome.name.as_str(),
                        path: outcome.path.as_str(),
                        existed: outcome.existed,
                        byte_size: outcome.byte_size,
                        removed: outcome.removed,
                    })
                    .collect(),
                note: "removes only this repository's declared build-cache directories",
            };
            print_json_report(&Envelope::ok(output), "cache prune build-cache")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new(
                "ctx",
                "cache prune",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned(
                "dry-run",
                dry_run.to_string(),
                RowTone::Default,
            ))
            .row(PanelRow::toned("prune-mode", prune_mode, RowTone::Default))
            .row(PanelRow::toned(
                "note",
                "removes only the reported repository-owned build-cache directory",
                RowTone::Default,
            ));
            let rows = outcomes
                .iter()
                .map(|outcome| {
                    PanelRow::toned(
                        outcome.name.as_str(),
                        format!(
                            "path={} bytes={} existed={} removed={}",
                            outcome.path, outcome.byte_size, outcome.existed, outcome.removed
                        ),
                        if outcome.removed {
                            RowTone::Pass
                        } else {
                            RowTone::Default
                        },
                    )
                })
                .collect();
            panel = panel.section(PanelSection::new("caches", rows));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Declared `[run.build-cache.<name>]` names from effective configuration
/// (project + machine layers) rooted at `root_path` — the selected
/// repository (honoring an explicit `--repo-root`), not the invocation CWD —
/// the same resolution doctor's `--config` report uses. Never trusts the
/// gitignored `.ctx/config.toml` as shipped source; only the isolated
/// configuration a caller's environment resolves.
fn declared_build_cache_names(
    root_path: &camino::Utf8Path,
) -> crate::Result<std::collections::BTreeSet<String>> {
    let report = ctx_traits_io::harness_config::resolve_config_report(root_path)?;
    Ok(report
        .runtime
        .run
        .map(|run| run.build_cache.into_keys().collect())
        .unwrap_or_default())
}

fn emit_cache_plan(
    action: &str,
    plan: &ctx_traits_core::cache::CachePlan,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let output = CachePlanReport {
                action,
                cache_artifact_mode: "metadata-only",
                plan,
                note: "cache commands never mutate canonical trait state",
            };
            print_json_report(&Envelope::ok(output), "cache plan")?;
        }
        OutputMode::Human(mode) => {
            let panel = cache_plan_panel(action, plan);
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

fn cache_plan_panel(action: &str, plan: &ctx_traits_core::cache::CachePlan) -> Panel {
    let mut panel = Panel::new(
        "ctx",
        format!("cache {action}"),
        PanelStatus::Passed("passed".to_string()),
    )
    .row(PanelRow::toned(
        "cache-artifact-mode",
        "metadata-only",
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "note",
        "cache commands never mutate canonical trait state",
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "counts",
        format!(
            "total: {} · fresh: {} · stale: {} · missing: {} · prune: {}",
            plan.total_count,
            plan.fresh_count,
            plan.stale_count,
            plan.missing_count,
            plan.prune_count
        ),
        RowTone::Default,
    ));
    if !plan.entries.is_empty() {
        let rows = plan
            .entries
            .iter()
            .map(|entry| {
                let status = if entry.fresh {
                    "fresh".to_string()
                } else if let Some(reason) = &entry.stale_reason {
                    reason.clone()
                } else {
                    "stale".to_string()
                };
                PanelRow::toned(
                    format!("{} ({})", entry.key.trait_id, entry.key.render_profile),
                    status,
                    if entry.fresh {
                        RowTone::Pass
                    } else {
                        RowTone::Default
                    },
                )
            })
            .collect();
        panel = panel.section(PanelSection::new("entries", rows));
    }
    panel
}
