//! `ctx traits doctor`: read-only, deterministic x-ray of a folder of
//! Agent-Skills-style source files.
//!
//! Doctor never writes, never calls a model/provider, and never touches the
//! network. It discovers candidate files (`SKILL.md`/`AGENTS.md`/`CLAUDE.md`)
//! under a path, runs the same deterministic import analysis `ctx traits
//! import` uses ([`crate::app::import_analysis::analyze_import_source`]) on
//! each one, and aggregates the per-file evidence into one report. `doctor`
//! suggests next steps; it never verifies — that remains `ctx traits
//! check`'s job after an actual import.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::import::plan::doctor::{DoctorFileInput, DoctorFileOutcome, DoctorReport};
use ctx_traits_core::response::{CommandOutput, Envelope};

use crate::app::command_handlers::print_json_report;
use crate::app::import_analysis::analyze_import_source;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};
use crate::app::tui::{
    Line, Tone, clean_live_text, command_line, emit_report, labeled_line, write_plain_line,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigDoctorReport {
    knobs: std::collections::BTreeMap<String, ConfigDoctorValue>,
    tier_warnings: Vec<String>,
    requirement_conflicts: Vec<ctx_traits_io::harness_config::ConfigRequirementConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    foreign_config: Option<String>,
    /// P427 zero-config fallback: one row per compiled-in built-in harness,
    /// in fixed candidate order, reporting exactly what automatic selection
    /// would see this invocation.
    builtin_harnesses: Vec<BuiltinHarnessDoctorRow>,
    /// P457: one line per config layer whose `config.toml` is a generated
    /// artifact (a sibling `config.ts` present, header parsed). Empty for
    /// the ordinary hand-authored, TOML-first case.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    generated: Vec<String>,
    /// P492: every environment variable product code reads
    /// (`ctx_traits_io::env_reference::env_reference()`), its contract, and
    /// whether it is currently set in this invocation's environment. The
    /// shipped, self-updating surface a README pointer names instead of a
    /// second hand-maintained list.
    environment: Vec<EnvVarDoctorRow>,
    /// 0084: one row per installed command/check step that authors
    /// `timeout-ms` and/or `idle-timeout-ms`, with the effective wall/idle
    /// bound and which side (the step or the repository config) won. Every
    /// step this list omits is silently governed by the two `run.command-*`
    /// rows above. Empty when trait inventory cannot be resolved (e.g. an
    /// ad-hoc, non-repository invocation) rather than failing doctor.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_step_bounds: Vec<CommandStepBoundDoctorRow>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CommandStepBoundDoctorRow {
    trait_id: String,
    step_id: String,
    wall_ms: u64,
    wall_source: &'static str,
    idle_ms: u64,
    idle_source: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct EnvVarDoctorRow {
    name: String,
    contract: String,
    kind: ctx_traits_io::env_reference::EnvVarKind,
    set: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct BuiltinHarnessDoctorRow {
    id: String,
    bin: String,
    order: usize,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// `true` when a same-id `[harness.<id>]` table in effective
    /// configuration replaces this built-in's compiled-in definition.
    overridden: bool,
}

impl From<ctx_traits_io::harness_config::BuiltinHarnessDetection> for BuiltinHarnessDoctorRow {
    fn from(row: ctx_traits_io::harness_config::BuiltinHarnessDetection) -> Self {
        Self {
            id: row.id,
            bin: row.bin,
            order: row.order,
            available: row.available,
            version: row.version,
            error: row.error,
            overridden: row.overridden,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigDoctorValue {
    value: String,
    winner: ctx_traits_io::harness_config::ConfigWinner,
    reason: String,
}

pub(crate) fn handle_doctor_config(json: bool) -> crate::Result<CommandOutput<()>> {
    let mut report = ctx_traits_io::harness_config::resolve_config_report(Utf8Path::new("."))?;
    let mut knobs = std::collections::BTreeMap::new();
    fn add_as(
        knobs: &mut std::collections::BTreeMap<String, ConfigDoctorValue>,
        winners: &std::collections::BTreeMap<String, ctx_traits_io::harness_config::ConfigWinner>,
        name: String,
        value: String,
        winner_key: &str,
    ) {
        let winner = winners.get(winner_key).cloned().unwrap_or(
            ctx_traits_io::harness_config::ConfigWinner {
                layer: ctx_traits_io::harness_config::ConfigLayer::BuiltIn,
                source: None,
                reason: ctx_traits_io::harness_config::ConfigReason::Default,
                contributors: Vec::new(),
            },
        );
        let reason = winner.reason.label().to_string();
        knobs.insert(
            name,
            ConfigDoctorValue {
                value,
                winner,
                reason,
            },
        );
    }
    fn add(
        knobs: &mut std::collections::BTreeMap<String, ConfigDoctorValue>,
        winners: &std::collections::BTreeMap<String, ctx_traits_io::harness_config::ConfigWinner>,
        name: String,
        value: String,
    ) {
        let winner_key = name.clone();
        add_as(knobs, winners, name, value, &winner_key);
    }
    let policy = report.runtime.effective_run_policy();
    add(
        &mut knobs,
        &report.winners,
        "worktree.enabled".into(),
        policy.worktree.to_string(),
    );
    let total_seconds = policy.total_seconds.unwrap_or(1800);
    for (name, value) in [
        (
            "budget.max-frames",
            policy.max_frames.unwrap_or(100).to_string(),
        ),
        (
            "budget.frame-seconds",
            policy.frame_seconds.unwrap_or(300).to_string(),
        ),
        ("budget.total-seconds", total_seconds.to_string()),
        (
            "budget.max-retries",
            policy.max_retries.unwrap_or(1).to_string(),
        ),
        (
            "budget.attach-wait-seconds",
            policy
                .attach_wait_seconds
                .unwrap_or(total_seconds)
                .to_string(),
        ),
        (
            "budget.idle-seconds",
            policy
                .idle_seconds
                .map_or_else(|| "absent".into(), |v| v.to_string()),
        ),
        ("drive.max-in-flight", policy.max_in_flight.to_string()),
        ("drive.wait", policy.wait.to_string()),
        ("drive.strict-loops", policy.strict_loops.to_string()),
        (
            "drive.inline-prompt-bytes",
            policy
                .inline_prompt_bytes
                .unwrap_or(crate::app::frame_prompt::DEFAULT_MAX_INLINE_PROMPT_BYTES)
                .to_string(),
        ),
        (
            "budget.command-seconds",
            format!(
                "{} (authored step timeout-ms wins over this)",
                policy.command_seconds.unwrap_or(14_400)
            ),
        ),
        (
            "budget.command-idle-seconds",
            format!(
                "{} (authored step idle-timeout-ms wins over this)",
                policy.command_idle_seconds.unwrap_or(600)
            ),
        ),
    ] {
        add(&mut knobs, &report.winners, name.into(), value);
    }
    add(
        &mut knobs,
        &report.winners,
        "drive.story".into(),
        match policy.story {
            Some(level) if level.spends_model_call() => {
                format!("{level} (spends narrator model calls)")
            }
            Some(level) => level.to_string(),
            None => "absent (off)".to_string(),
        },
    );
    add(
        &mut knobs,
        &report.winners,
        "drive.usage-warning-threshold".into(),
        match policy.usage_warning_threshold {
            Some(threshold) => threshold.to_string(),
            None => "absent (off)".to_string(),
        },
    );
    let merge = report.runtime.effective_merge_policy();
    add(
        &mut knobs,
        &report.winners,
        "merge.wait".into(),
        merge.wait.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.overlap".into(),
        crate::app::presentation::wire_name(&merge.overlap).to_lowercase(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.auto".into(),
        merge.auto.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.deep".into(),
        merge.deep.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.branch".into(),
        match merge.branch.as_deref() {
            Some(branch) => branch.to_string(),
            None => match ctx_traits_io::repository::discover_repo_root() {
                Ok(repo_root) => {
                    let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
                    match ctx_traits_io::worktree::resolve_default_branch(
                        &repo_root,
                        None,
                        &mut warnings,
                    ) {
                        Ok((branch, ctx_traits_io::worktree::DefaultBranchSource::Fallback)) => {
                            format!("{branch} (assumed: fallback)")
                        }
                        Ok((branch, source)) => {
                            format!("{branch} (discovered: {})", source.label())
                        }
                        Err(_) => "unresolved (discovery failed)".to_string(),
                    }
                }
                Err(_) => "unresolved (not a git repository)".to_string(),
            },
        },
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.gate".into(),
        if merge.gate.is_empty() {
            "absent (empty)".to_string()
        } else {
            crate::app::presentation::wire_name(&merge.gate)
        },
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.gate-seconds".into(),
        merge.gate_seconds.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.retry-attempts".into(),
        merge.retry_attempts.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.retry-backoff-ms".into(),
        merge.retry_backoff_ms.to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "merge.generated".into(),
        if merge.generated.is_empty() {
            "absent (empty)".to_string()
        } else {
            format!("{:?}", merge.generated)
        },
    );
    add(
        &mut knobs,
        &report.winners,
        "worktree.seed".into(),
        report.runtime.worktree.seed.len().to_string(),
    );
    add(
        &mut knobs,
        &report.winners,
        "worktree.warm".into(),
        if report.runtime.worktree.warm.is_empty() {
            "none (worktrees build cold)".to_string()
        } else {
            report.runtime.worktree.warm.join(", ")
        },
    );
    add(
        &mut knobs,
        &report.winners,
        "worktree.setup".into(),
        report.runtime.worktree.setup.len().to_string(),
    );
    for (key, value) in &report.runtime.worktree.env {
        add(
            &mut knobs,
            &report.winners,
            format!("worktree.env.{key}"),
            value.clone(),
        );
    }
    add(
        &mut knobs,
        &report.winners,
        "worktree.setup-seconds".into(),
        report.runtime.worktree.setup_seconds.map_or_else(
            || {
                format!(
                    "absent ({}s default)",
                    ctx_traits_io::worktree::DEFAULT_SETUP_TIMEOUT_MS / 1000
                )
            },
            |seconds| seconds.to_string(),
        ),
    );
    add(
        &mut knobs,
        &report.winners,
        "worktree.setup-capture-bytes".into(),
        report.runtime.worktree.setup_capture_bytes.map_or_else(
            || {
                format!(
                    "absent ({} bytes default)",
                    ctx_traits_io::worktree::DEFAULT_SETUP_CAPTURE_BYTES
                )
            },
            |bytes| bytes.to_string(),
        ),
    );
    add_as(
        &mut knobs,
        &report.winners,
        "worktree.tripwire.policy".into(),
        report.runtime.worktree.tripwire.policy.as_str().to_string(),
        "worktree.tripwire.policy",
    );
    add_as(
        &mut knobs,
        &report.winners,
        "worktree.tripwire.sentinel".into(),
        if report.runtime.worktree.tripwire.sentinel.is_empty() {
            "none (config-layer files only)".to_string()
        } else {
            crate::app::presentation::wire_name(&report.runtime.worktree.tripwire.sentinel)
        },
        "worktree.tripwire.sentinel",
    );
    add_as(
        &mut knobs,
        &report.winners,
        "worktree.confinement.enabled".into(),
        report.runtime.worktree.confinement.enabled.to_string(),
        "worktree.confinement.enabled",
    );
    add_as(
        &mut knobs,
        &report.winners,
        "worktree.confinement.sandbox".into(),
        report.runtime.worktree.confinement.sandbox.to_string(),
        "worktree.confinement.sandbox",
    );
    add_as(
        &mut knobs,
        &report.winners,
        "worktree.confinement.allow".into(),
        if report.runtime.worktree.confinement.allow.is_empty() {
            "none (no additional directories)".to_string()
        } else {
            crate::app::presentation::wire_name(&report.runtime.worktree.confinement.allow)
        },
        "worktree.confinement.allow",
    );
    if !report.runtime.worktree.build_cache.is_empty() {
        // Use the owning checkout for linked worktrees, matching build-cache
        // exports and `cache prune --build` exactly.
        let build_cache_repo_root = match ctx_traits_io::repository::discover_repo_root() {
            Ok(root) => ctx_traits_io::repository::discover_main_repo_root(&root),
            Err(error) => Err(error),
        };
        for (name, cache) in &report.runtime.worktree.build_cache {
            add_as(
                &mut knobs,
                &report.winners,
                format!("worktree.build-cache.{name}.env"),
                cache.env.clone(),
                &format!("worktree.build-cache.{name}"),
            );
            add_as(
                &mut knobs,
                &report.winners,
                format!("worktree.build-cache.{name}.dir"),
                match &build_cache_repo_root {
                    Ok(repo_root) => {
                        ctx_traits_io::layout::build_cache_root_path(repo_root, name)?.to_string()
                    }
                    Err(_) => "unresolved (not a git repository)".to_string(),
                },
                &format!("worktree.build-cache.{name}"),
            );
        }
    }
    add(
        &mut knobs,
        &report.winners,
        "git.long-seconds".into(),
        report
            .runtime
            .git
            .as_ref()
            .and_then(|git| git.long_seconds)
            .map_or_else(
                || {
                    format!(
                        "absent ({}s default)",
                        ctx_traits_io::git_process::LONG_TIMEOUT_MS / 1000
                    )
                },
                |seconds| seconds.to_string(),
            ),
    );
    add(
        &mut knobs,
        &report.winners,
        "publish.exclude".into(),
        report
            .runtime
            .publish
            .as_ref()
            .and_then(|publish| publish.exclude.as_ref())
            .map_or_else(
                || {
                    format!(
                        "absent (default: {})",
                        crate::app::presentation::wire_name(
                            &ctx_traits_io::publish::PACK_DEFAULT_EXCLUDES
                        )
                    )
                },
                |exclude| crate::app::presentation::wire_name(&exclude),
            ),
    );
    // P492: the env override is not itself a config-file layer, so the
    // winner map alone cannot express it — name it explicitly whenever
    // active, or this row would lie about what the next install actually
    // hits. Reads through the same `resolve_registry_base_with_source`
    // precedence/emptiness rule `resolve_registry_options` uses, rather than
    // re-deriving it from a second `CTX_TRAITS_REGISTRY_BASE` read here.
    {
        use ctx_traits_io::distribution::RegistryBaseSource;
        let resolved =
            ctx_traits_io::distribution::resolve_registry_base_with_source(Utf8Path::new("."));
        match resolved.source {
            // The winners map has no entry for an env override — it tracks
            // config-file layers only — so inserting directly with an
            // `Environment` winner keeps the row's `[layer]` suffix honest
            // instead of falling back through `add`'s lookup to whatever
            // config layer happens to be recorded for `registry.base`.
            RegistryBaseSource::EnvOverride => {
                knobs.insert(
                    "registry.base".into(),
                    ConfigDoctorValue {
                        value: format!(
                            "{} (env override: CTX_TRAITS_REGISTRY_BASE)",
                            resolved.base
                        ),
                        winner: ctx_traits_io::harness_config::ConfigWinner {
                            layer: ctx_traits_io::harness_config::ConfigLayer::Environment,
                            source: Some("CTX_TRAITS_REGISTRY_BASE".into()),
                            reason:
                                ctx_traits_io::harness_config::ConfigReason::EnvironmentOverride,
                            contributors: Vec::new(),
                        },
                        reason: "environment override".into(),
                    },
                );
            }
            RegistryBaseSource::Config => {
                add(
                    &mut knobs,
                    &report.winners,
                    "registry.base".into(),
                    resolved.base,
                );
            }
            RegistryBaseSource::Default => {
                add(
                    &mut knobs,
                    &report.winners,
                    "registry.base".into(),
                    format!("absent (default: {})", resolved.base),
                );
            }
        }
    }
    // P569: render every harness that RESOLVES, not only every harness the
    // config happens to name. Once conventions live in the compiled-in
    // definitions a working repo may configure none at all, and iterating the
    // config alone would then show an empty registry while runs dispatch
    // happily — the operator surface would go blind exactly as the config got
    // simpler. Configured entries are already merged (P568), so an entry that
    // is present is used as-is and an absent built-in resolves to its
    // compiled-in definition.
    let configured_registry = ctx_traits_io::harness_config::HarnessRegistry {
        schema_version: None,
        harness: report.runtime.harness.clone(),
    };
    let mut harness_names: Vec<String> = ctx_traits_io::harness_config::built_in_harness_ids()
        .into_iter()
        .map(str::to_string)
        .collect();
    for name in report.runtime.harness.keys() {
        if !harness_names.contains(name) {
            harness_names.push(name.clone());
        }
    }
    harness_names.sort();
    for name in &harness_names {
        let resolved =
            ctx_traits_io::harness_config::built_in_harness_definition(name, &configured_registry);
        let harness = &resolved;
        add(
            &mut knobs,
            &report.winners,
            format!("harness.{name}.kind"),
            harness.kind().to_string(),
        );
        add(
            &mut knobs,
            &report.winners,
            format!("harness.{name}.bin"),
            harness.bin().to_string(),
        );
        add(
            &mut knobs,
            &report.winners,
            format!("harness.{name}.transports"),
            crate::app::presentation::wire_name(&harness.transports),
        );
        add(
            &mut knobs,
            &report.winners,
            format!("harness.{name}.version-probe"),
            crate::app::presentation::wire_name(&harness.version_probe),
        );
        if let Some(cli) = &harness.cli {
            for (field, value) in [
                ("argv", crate::app::presentation::wire_name(&cli.argv)),
                (
                    "narrator-argv",
                    crate::app::presentation::wire_name(&cli.narrator_argv),
                ),
                (
                    "warm-argv",
                    crate::app::presentation::wire_name(&cli.warm_argv),
                ),
                (
                    "json-schema-flag",
                    crate::app::presentation::wire_name(&cli.json_schema_flag),
                ),
                (
                    "model-flag",
                    crate::app::presentation::wire_name(&cli.model_flag),
                ),
                (
                    "reasoning-effort-flag",
                    crate::app::presentation::wire_name(&cli.reasoning_effort_flag),
                ),
                (
                    "system-prompt-flag",
                    crate::app::presentation::wire_name(&cli.system_prompt_flag),
                ),
                (
                    "resume-flag",
                    crate::app::presentation::wire_name(&cli.resume_flag),
                ),
                (
                    "session-flag",
                    crate::app::presentation::wire_name(&cli.session_flag),
                ),
                (
                    "dir-flag",
                    crate::app::presentation::wire_name(&cli.dir_flag),
                ),
                (
                    "prompt-via",
                    crate::app::presentation::wire_name(&cli.prompt_via),
                ),
                ("stream", cli.stream().to_string()),
                ("output", crate::app::presentation::wire_name(&cli.output)),
            ] {
                add(
                    &mut knobs,
                    &report.winners,
                    format!("harness.{name}.cli.{field}"),
                    value,
                );
            }
        }
        if let Some(mcp) = &harness.mcp {
            for (field, value) in [
                (
                    "mcp-config-flag",
                    crate::app::presentation::wire_name(&mcp.mcp_config_flag),
                ),
                (
                    "allowed-tools-flag",
                    crate::app::presentation::wire_name(&mcp.allowed_tools_flag),
                ),
                (
                    "allowed-tools",
                    crate::app::presentation::wire_name(&mcp.allowed_tools),
                ),
                (
                    "system-prompt-flag",
                    crate::app::presentation::wire_name(&mcp.system_prompt_flag),
                ),
                (
                    "reasoning-effort-flag",
                    crate::app::presentation::wire_name(&mcp.reasoning_effort_flag),
                ),
                (
                    "config-via",
                    crate::app::presentation::wire_name(&mcp.config_via),
                ),
            ] {
                add(
                    &mut knobs,
                    &report.winners,
                    format!("harness.{name}.mcp.{field}"),
                    value,
                );
            }
        }
    }
    // 0025: the resolved view keeps showing the expanded seats — a
    // `count`/list-form role's seat aliases (`<role>-1` … `<role>-N`) are
    // what an actual run resolves, and this is the same expansion drive
    // dispatch uses, not a second reimplementation of the rule.
    let mut expanded_agent = report.runtime.agent.clone();
    ctx_traits_io::harness_config::expand_role_seats(&mut expanded_agent);
    for (name, assignment) in &report.runtime.agent.role {
        add_assignment_rows(
            &mut knobs,
            &report.winners,
            &format!("agent.role.{name}"),
            assignment,
        );
    }
    for (name, assignment) in &expanded_agent.role {
        if report.runtime.agent.role.contains_key(name) {
            continue;
        }
        add_assignment_rows(
            &mut knobs,
            &report.winners,
            &format!("agent.role.{name}"),
            assignment,
        );
    }
    // P451: every declared variant/repo qualifier table, plus the active
    // repo-qualifier key for this invocation — doctor has no trait, hence no
    // variant, so this only ever shows what is DECLARED, not which qualifier
    // a real run would resolve (that proof lives in run provenance).
    for (variant, value) in &report.runtime.agent.variant {
        for role in value.role.keys() {
            add_assignment_rows(
                &mut knobs,
                &report.winners,
                &format!("agent.variant.{variant}.role.{role}"),
                &value.role[role],
            );
        }
    }
    for (repo_key, repo_override) in &report.runtime.repo {
        for role in repo_override.agent.role.keys() {
            add_assignment_rows(
                &mut knobs,
                &report.winners,
                &format!("repo.{repo_key}.agent.role.{role}"),
                &repo_override.agent.role[role],
            );
        }
        for (variant, value) in &repo_override.agent.variant {
            for role in value.role.keys() {
                add_assignment_rows(
                    &mut knobs,
                    &report.winners,
                    &format!("repo.{repo_key}.agent.variant.{variant}.role.{role}"),
                    &value.role[role],
                );
            }
        }
    }
    add(
        &mut knobs,
        &report.winners,
        "repo.active-key".into(),
        ctx_traits_io::harness_config::active_repo_qualifier_key()
            .unwrap_or_else(|| "none (ad-hoc invocation)".to_string()),
    );
    // 0034: every declared `[trait.<id>]` seat, plus a warning for any
    // declared id that names no installed trait — a config may legitimately
    // outlive a trait (0034's Watch), so this is a warning, never a
    // decode/validation failure.
    for (trait_id, trait_defaults) in &report.runtime.trait_defaults {
        for role in trait_defaults.agent.role.keys() {
            add_assignment_rows(
                &mut knobs,
                &report.winners,
                &format!("trait.{trait_id}.agent.role.{role}"),
                &trait_defaults.agent.role[role],
            );
        }
        for (variant, value) in &trait_defaults.variant {
            for role in value.agent.role.keys() {
                add_assignment_rows(
                    &mut knobs,
                    &report.winners,
                    &format!("trait.{trait_id}.variant.{variant}.agent.role.{role}"),
                    &value.agent.role[role],
                );
            }
        }
    }
    if !report.runtime.trait_defaults.is_empty()
        && let Ok(repo_root) = ctx_traits_io::repository::discover_repo_root()
        && let Ok(installed) = ctx_traits_io::discovery::trait_inventory_ids(&repo_root)
    {
        let installed: std::collections::BTreeSet<_> = installed.into_iter().collect();
        for trait_id in report.runtime.trait_defaults.keys() {
            if !installed.contains(trait_id) {
                report.tier_warnings.push(format!(
                    "[trait.{trait_id}] names no installed trait in this repository"
                ));
            }
        }
    }
    // P475: seat budgets, for the union of every configured role and the
    // four standing seats, resolve through the same
    // `ResolvedRuntimeAssignments::budget_for_seat` path drive/merge/narrator
    // dispatch use. Their rows retain the assignment leaf winner, so inherited
    // budget fields and replacement seats report their actual source.
    {
        let profile = ctx_traits_io::harness_config::resolve_runtime_assignments(&[])?;
        // 0025: iterate the expanded map so a `count`/list-form role's seat
        // aliases each get their own budget row, matching what
        // `budget_for_seat` actually resolves for them at dispatch time.
        let mut roles: std::collections::BTreeSet<String> =
            expanded_agent.role.keys().cloned().collect();
        for seat in ctx_traits_io::harness_config::standing_seat_names() {
            roles.insert(seat.to_string());
        }
        for role in roles {
            let list_length = match report.runtime.agent.role.get(&role) {
                Some(ctx_traits_io::harness_config::RoleAssignmentValue::List(entries)) => {
                    Some(entries.len())
                }
                _ => None,
            };
            // `seat_index` is the 1-BASED label `format_role`/the row key
            // already use elsewhere (`agent.role.<name>.<seat_index>`);
            // `budget_for_seat` takes the 0-BASED structural ordinal
            // `plan_from_seats`/drive's own seat selection use
            // (`frame.assigned_agent.structural_seat`) — the two bases
            // differ, so every seat in this loop carries both, and the
            // 0-based value is what's actually passed to `budget_for_seat`
            // (never the label) to keep the row's displayed seat and its
            // resolved budget paired from the same index.
            let seats: Vec<Option<u32>> = match list_length {
                Some(len) => (1..=len as u32).map(Some).collect(),
                None => vec![None],
            };
            // P475 D3/D4: `narrator`/`merger`/`merger-deep` are one-shot
            // calls outside the drive frame loop, resolved against their OWN
            // seat default (never the `[budget]`/CLI-flag chain frame seats
            // use), and never take an idle timeout or a retry loop at all —
            // `validate_role_budget` rejects declaring either. Each row
            // below shows the value a frame/call would actually resolve,
            // not a generic placeholder, so this listing is byte-for-byte
            // what dispatch uses. Classification reuses
            // `standing_seat_is_one_shot` — the one place seat dispatch
            // shape is declared — rather than re-deriving it from role-name
            // literals here.
            let one_shot = ctx_traits_io::harness_config::standing_seat_is_one_shot(&role);
            let one_shot_default_ms = match role.as_str() {
                "narrator" => Some(crate::app::drive::DEFAULT_NARRATOR_TIMEOUT_MS),
                "merger" | "merger-deep" => Some(crate::app::merge::DEFAULT_MERGER_TIMEOUT_MS),
                _ => None,
            };
            for seat_label in seats {
                let structural_seat = seat_label.map(|label| label - 1);
                let budget = profile.budget_for_seat(&role, structural_seat);
                let key_role = match seat_label {
                    Some(seat_index) => format!("{role}.{seat_index}"),
                    None => role.clone(),
                };
                let frame_seconds_default = one_shot_default_ms
                    .map(|ms| ms / 1_000)
                    .unwrap_or(crate::app::drive::DEFAULT_FRAME_SECONDS);
                for (field, value, declared) in [
                    (
                        "frame-seconds",
                        budget
                            .frame_seconds
                            .unwrap_or(frame_seconds_default)
                            .to_string(),
                        budget.frame_seconds.is_some(),
                    ),
                    (
                        "idle-seconds",
                        if one_shot {
                            "not applicable (one-shot call)".to_string()
                        } else {
                            budget
                                .idle_seconds
                                .map_or_else(|| "absent".to_string(), |v| v.to_string())
                        },
                        !one_shot && budget.idle_seconds.is_some(),
                    ),
                    (
                        "max-retries",
                        if one_shot {
                            "not applicable (one-shot call)".to_string()
                        } else {
                            budget
                                .max_retries
                                .unwrap_or(crate::app::drive::DEFAULT_MAX_RETRIES)
                                .to_string()
                        },
                        !one_shot && budget.max_retries.is_some(),
                    ),
                ] {
                    let winner_key = if declared {
                        match seat_label {
                            Some(label) => format!("agent.role.{role}.{label}.budget.{field}"),
                            None => format!("agent.role.{role}.budget.{field}"),
                        }
                    } else {
                        String::new()
                    };
                    add_as(
                        &mut knobs,
                        &report.winners,
                        format!("agent.role.{key_role}.budget.{field}"),
                        value,
                        &winner_key,
                    );
                }
            }
        }
    }
    // P475 D5: the lock-wait ceiling each merge rung's merger budget derives
    // — surfaced BEFORE an operator raises a merger budget, not after a
    // batch wedges on the resulting 25x-amplified worst-case wait. Reuses
    // `ctx traits merge`'s OWN derivation (`merge::merge_lock_wait_timeout_ms`,
    // `merge::resolved_merger_role_budget`) rather than a second,
    // independently maintained formula that could silently drift from the
    // one actually enforced. Renders BOTH rungs, standard and `--deep`, each
    // rung-parameterised the same way `merge()` itself selects a rung (the
    // `--deep` FLAG, not table presence) — doctor has no flag to read, so
    // showing only one rung risks showing the wrong one to whichever
    // invocation the operator is about to run; when `merger-deep` has no
    // table of its own, `resolved_merger_role_budget`'s own fallback to
    // `merger` (mirroring `resolve_merger`) makes the two rows identical,
    // exactly like a real `--deep` merge would fall back.
    {
        let gate_policy = report.runtime.effective_merge_policy();
        let gate_total_seconds =
            (gate_policy.gate.len() as u64).saturating_mul(gate_policy.gate_seconds);
        for (row, merger_role, deep) in
            [("standard", "merger", false), ("deep", "merger-deep", true)]
        {
            let merger_budget =
                crate::app::merge::resolved_merger_role_budget(&report.runtime.agent, deep);
            let merger_seconds = merger_budget
                .frame_seconds
                .unwrap_or(crate::app::merge::DEFAULT_MERGER_TIMEOUT_MS / 1000);
            let derived_ms = crate::app::merge::merge_lock_wait_timeout_ms(
                &gate_policy,
                merger_seconds.saturating_mul(1_000),
            );
            add(
                &mut knobs,
                &report.winners,
                format!("merge.lock-wait-derived-seconds.{row}"),
                format!(
                    "{} (= {} * {merger_role}-budget[{merger_seconds}s] + gate[{gate_total_seconds}s] + overhead[{}s])",
                    derived_ms / 1_000,
                    crate::app::merge::MAX_RECONCILIATION_ITERATIONS,
                    crate::app::merge::MERGE_LOCK_WAIT_OVERHEAD_MS / 1_000,
                ),
            );
        }
    }
    for name in report.runtime.host.keys() {
        let host = &report.runtime.host[name];
        for (field, value) in [
            ("profile", host.profile.clone()),
            ("format", host.format.clone()),
            ("project-path", host.project_path.clone()),
            ("global-path", host.global_path.clone()),
        ] {
            add(
                &mut knobs,
                &report.winners,
                format!("host.{name}.{field}"),
                value.unwrap_or_else(|| "absent".into()),
            );
        }
    }
    let configured_registry = ctx_traits_io::harness_config::HarnessRegistry {
        schema_version: None,
        harness: report.runtime.harness.clone(),
    };
    let builtin_harnesses: Vec<BuiltinHarnessDoctorRow> =
        ctx_traits_io::harness_config::detect_builtin_harnesses(&configured_registry)
            .into_iter()
            .map(Into::into)
            .collect();
    // 0177: the generated pathway is now the config *document*
    // (`.ctx/traits/config.ts` -> `.ctx/traits/generated/config.toml`), not
    // `RuntimeConfig` — the P457 pathway this loop used to inspect retired.
    let mut generated = Vec::new();
    if let Ok(repo_root) = ctx_traits_io::repository::discover_repo_root() {
        let generated_config_path =
            ctx_traits_io::layout::trait_generated_root_path(&repo_root).join("config.toml");
        if generated_config_path.exists()
            && let Ok(text) = ctx_traits_io::read::read_text(&generated_config_path)
        {
            let header = ctx_traits_io::config_source::parse_header(&text);
            generated.push(format!(
                "repo: generated from config.ts, {} sources",
                header.entries.len()
            ));
        }
    }
    let environment = ctx_traits_io::env_reference::env_reference()
        .into_iter()
        .map(|doc| EnvVarDoctorRow {
            name: doc.name.to_string(),
            contract: doc.contract.to_string(),
            kind: doc.kind,
            set: std::env::var_os(doc.name).is_some(),
        })
        .collect();
    let command_step_bounds = command_step_bound_doctor_rows(&policy);
    let output = ConfigDoctorReport {
        knobs,
        tier_warnings: report.tier_warnings,
        requirement_conflicts: report.requirement_conflicts,
        foreign_config: report.foreign_config,
        builtin_harnesses,
        generated,
        environment,
        command_step_bounds,
    };
    if json {
        print_json_report(&output, "doctor config report")?;
    } else {
        println!("ctx traits doctor --config");
        for (name, value) in &output.knobs {
            println!(
                "  {name}: {} [{}] [{}]",
                value.value,
                format_winner(&value.winner),
                value.reason,
            );
            if !value.winner.contributors.is_empty() {
                let contributors = value
                    .winner
                    .contributors
                    .iter()
                    .map(|contributor| match contributor.source.as_deref() {
                        Some(source) => {
                            format!("{}: {source}", config_layer_label(contributor.layer))
                        }
                        None => config_layer_label(contributor.layer).to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("    contributors: {contributors}");
            }
        }
        for warning in &output.tier_warnings {
            println!("  warning: {warning}");
        }
        for conflict in &output.requirement_conflicts {
            println!(
                "  warning: {} rejected from {}; repository requirement from {} remains effective",
                conflict.field, conflict.rejected_source, conflict.repo_source
            );
        }
        if let Some(path) = &output.foreign_config {
            println!("  hint: foreign config exists but was not loaded: {path}");
        }
        for line in &output.generated {
            println!("  {line}");
        }
        for row in &output.builtin_harnesses {
            println!(
                "  builtin-harness: {} bin={} order={} available={} version={} overridden={}{}",
                row.id,
                row.bin,
                row.order,
                row.available,
                row.version.as_deref().unwrap_or("-"),
                row.overridden,
                row.error
                    .as_deref()
                    .map(|error| format!(" error={error:?}"))
                    .unwrap_or_default(),
            );
        }
        println!("  environment:");
        for row in &output.environment {
            println!(
                "    {} [{}] set={}: {}",
                row.name,
                format_env_var_kind(row.kind),
                row.set,
                row.contract
            );
        }
        if !output.command_step_bounds.is_empty() {
            println!("  command-step-bounds:");
            for row in &output.command_step_bounds {
                println!(
                    "    {}: {} — wall={}ms [{}] idle={}ms [{}]",
                    safe(&row.trait_id),
                    safe(&row.step_id),
                    row.wall_ms,
                    row.wall_source,
                    row.idle_ms,
                    row.idle_source,
                );
            }
        }
    }
    Ok(CommandOutput::new(()))
}

/// 0084: one row per installed command/check step that authors
/// `timeout-ms` and/or `idle-timeout-ms`, resolved against `policy` with the
/// same step-over-config precedence the IO runner applies
/// (`ctx_traits_io::run`'s `resolve_command_bounds`). Listing only authoring
/// steps keeps the output bounded — the `run.command-seconds`/
/// `run.command-idle-seconds` rows already describe every silent step.
/// Degrades to an empty list rather than failing doctor when inventory
/// discovery is unavailable (e.g. an ad-hoc, non-repository invocation).
fn command_step_bound_doctor_rows(
    policy: &ctx_traits_io::harness_config::EffectiveRunPolicy,
) -> Vec<CommandStepBoundDoctorRow> {
    let Ok(context) = ctx_traits_io::inventory::InventoryContext::discover() else {
        return Vec::new();
    };
    let Ok(ids) = context.candidate_ids() else {
        return Vec::new();
    };
    let default_wall_ms = policy
        .command_seconds
        .unwrap_or(14_400)
        .saturating_mul(1_000);
    let default_idle_ms = policy
        .command_idle_seconds
        .unwrap_or(600)
        .saturating_mul(1_000);
    let mut rows = Vec::new();
    for id in ids {
        let Ok(Some(resolution)) = context.resolve_tiers(&id) else {
            continue;
        };
        let Ok((trait_ref, ..)) = ctx_traits_io::run::load_trait(resolution.winner.path.as_str())
        else {
            continue;
        };
        let mut items: Vec<&ctx_traits_core::r#trait::procedure::SequenceItem> = Vec::new();
        if let Some(procedure) = &trait_ref.procedure {
            items.extend(procedure.sequence.iter());
        }
        for (_, named) in trait_ref.sequences.iter() {
            items.extend(named.sequence.iter());
        }
        for item in items {
            if !matches!(
                item.effective_kind(),
                ctx_traits_core::r#trait::procedure::SequenceKind::Command
                    | ctx_traits_core::r#trait::procedure::SequenceKind::Check
            ) {
                continue;
            }
            let declared_timeout_ms = item
                .timeout_ms
                .or_else(|| item.command.as_ref().and_then(|command| command.timeout_ms));
            let declared_idle_timeout_ms = item.idle_timeout_ms.or_else(|| {
                item.command
                    .as_ref()
                    .and_then(|command| command.idle_timeout_ms)
            });
            if declared_timeout_ms.is_none() && declared_idle_timeout_ms.is_none() {
                continue;
            }
            let step_id = item
                .id
                .clone()
                .unwrap_or_else(|| "(unnamed step)".to_string());
            rows.push(CommandStepBoundDoctorRow {
                trait_id: trait_ref.id.as_str().to_string(),
                step_id,
                wall_ms: declared_timeout_ms.unwrap_or(default_wall_ms),
                wall_source: if declared_timeout_ms.is_some() {
                    "step"
                } else {
                    "config"
                },
                idle_ms: declared_idle_timeout_ms.unwrap_or(default_idle_ms),
                idle_source: if declared_idle_timeout_ms.is_some() {
                    "step"
                } else {
                    "config"
                },
            });
        }
    }
    rows
}

/// 0079: whether an `api-key-env` reference resolves — the variable's NAME
/// only ever appears elsewhere in this render; its VALUE is never read into
/// any rendered string, here or anywhere else.
fn api_key_status(api_key_env: Option<&str>) -> String {
    match api_key_env {
        None => "absent".to_string(),
        Some(name) => {
            if ctx_traits_io::env_reference::resolve_env_var_reference(name).is_some() {
                "resolves".to_string()
            } else {
                "missing".to_string()
            }
        }
    }
}

fn format_env_var_kind(kind: ctx_traits_io::env_reference::EnvVarKind) -> &'static str {
    match kind {
        ctx_traits_io::env_reference::EnvVarKind::UserFacing => "user-facing",
        ctx_traits_io::env_reference::EnvVarKind::Internal => "internal",
        ctx_traits_io::env_reference::EnvVarKind::DebugOnlyTestHook => "debug-only-test-hook",
    }
}

fn add_assignment_rows(
    knobs: &mut std::collections::BTreeMap<String, ConfigDoctorValue>,
    winners: &std::collections::BTreeMap<String, ctx_traits_io::harness_config::ConfigWinner>,
    prefix: &str,
    value: &ctx_traits_io::harness_config::RoleAssignmentValue,
) {
    for (index, assignment) in value.entries().iter().enumerate() {
        let prefix = if value.is_list() {
            format!("{prefix}.{}", index + 1)
        } else {
            prefix.to_string()
        };
        let rows = [
            (
                "harness",
                crate::app::presentation::wire_name(&assignment.harness),
            ),
            (
                "transport",
                crate::app::presentation::wire_name(&assignment.transport),
            ),
            (
                "session-mode",
                crate::app::presentation::wire_name(&assignment.session_mode),
            ),
            (
                "model",
                crate::app::presentation::wire_name(&assignment.model),
            ),
            (
                "reasoning-effort",
                crate::app::presentation::wire_name(&assignment.reasoning_effort),
            ),
            (
                "extra-args",
                crate::app::presentation::wire_name(&assignment.extra_args),
            ),
            (
                "system-prompt",
                if assignment.system_prompt.is_some() {
                    "present".into()
                } else {
                    "absent".into()
                },
            ),
        ];
        // 0079: `transport = "api"` endpoint visibility is the designed
        // defense against a `base-url` silently redirecting every prompt to
        // an arbitrary host — always rendered when declared, never only when
        // the transport happens to resolve to `api`, so a stale endpoint
        // left behind by a transport switch is still visible.
        let api_rows: Vec<(&str, String)> = if assignment.transport
            == Some(ctx_traits_io::harness_config::RunTransport::Api)
            || assignment.api.base_url.is_some()
        {
            vec![
                (
                    "base-url",
                    crate::app::presentation::wire_name(&assignment.api.base_url),
                ),
                (
                    "wire",
                    crate::app::presentation::wire_name(&assignment.api.wire),
                ),
                (
                    "api-key-env",
                    crate::app::presentation::wire_name(&assignment.api.api_key_env),
                ),
                (
                    "api-key-status",
                    api_key_status(assignment.api.api_key_env.as_deref()),
                ),
            ]
        } else {
            Vec::new()
        };
        for (field, rendered) in rows.into_iter().chain(api_rows) {
            let name = format!("{prefix}.{field}");
            let winner = winners.get(&name).cloned().unwrap_or(
                ctx_traits_io::harness_config::ConfigWinner {
                    layer: ctx_traits_io::harness_config::ConfigLayer::BuiltIn,
                    source: None,
                    reason: ctx_traits_io::harness_config::ConfigReason::Default,
                    contributors: Vec::new(),
                },
            );
            knobs.insert(
                name,
                ConfigDoctorValue {
                    value: rendered,
                    reason: winner.reason.label().to_string(),
                    winner,
                },
            );
        }
    }
}

/// The single owner of the `ConfigLayer` → display-label mapping — every
/// renderer of a config layer (the P418 `--config` winner provenance, the
/// P514 `--migrate-config` layer listing) calls this, never a second
/// hand-copied match.
fn config_layer_label(layer: ctx_traits_io::harness_config::ConfigLayer) -> &'static str {
    match layer {
        ctx_traits_io::harness_config::ConfigLayer::BuiltIn => "built-in",
        ctx_traits_io::harness_config::ConfigLayer::UserGlobal => "user-global",
        ctx_traits_io::harness_config::ConfigLayer::Repo => "repo",
        ctx_traits_io::harness_config::ConfigLayer::Environment => "environment",
        ctx_traits_io::harness_config::ConfigLayer::Flag => "flag",
    }
}

/// 0037 refinement of [`config_layer_label`] for winner provenance: the two
/// `.ctx/traits/` documents share [`ConfigLayer::Repo`] in the resolver (the
/// merge order alone encodes their precedence), but the tier is exactly what
/// a `--config` reader needs — so the label splits them by the source path,
/// which JSON consumers also receive and can derive identically.
fn config_tier_label(
    layer: ctx_traits_io::harness_config::ConfigLayer,
    source: Option<&str>,
) -> &'static str {
    if layer == ctx_traits_io::harness_config::ConfigLayer::Repo
        && let Some(source) = source
    {
        if source.ends_with(ctx_traits_io::layout::RUNTIME_CONFIG) {
            return "local (this machine)";
        }
        if source.ends_with(ctx_traits_io::layout::PROJECT_CONFIG) {
            return "project";
        }
    }
    config_layer_label(layer)
}

fn format_winner(winner: &ctx_traits_io::harness_config::ConfigWinner) -> String {
    let layer = config_tier_label(winner.layer, winner.source.as_deref());
    match winner.source.as_deref() {
        Some(source) => format!("{layer}: {source}"),
        None => layer.to_string(),
    }
}

// ---------------------------------------------------------------------------
// `ctx traits doctor --migrate-state [--apply]` (P426)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct StateFamilyReport {
    global: String,
    legacy: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlannedMoveReport {
    family: &'static str,
    source: String,
    dest: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct OrphanReport {
    key: String,
    indexed_path: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppliedReport {
    moved: Vec<PlannedMoveReport>,
    failed: Vec<PlannedMoveFailureReport>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlannedMoveFailureReport {
    #[serde(flatten)]
    planned: PlannedMoveReport,
    error: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct MigrateStateReport {
    action: &'static str,
    repo_key: String,
    canonical_repo_root: String,
    runs: StateFamilyReport,
    debug: StateFamilyReport,
    cache: StateFamilyReport,
    moves: Vec<PlannedMoveReport>,
    conflicts: Vec<PlannedMoveReport>,
    orphans: Vec<OrphanReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    applied: Option<AppliedReport>,
    note: &'static str,
}

fn move_report(
    family: ctx_traits_io::state::StateFamily,
    source: &Utf8Path,
    dest: &Utf8Path,
) -> PlannedMoveReport {
    PlannedMoveReport {
        family: family.label(),
        source: source.to_string(),
        dest: dest.to_string(),
    }
}

pub(crate) fn handle_doctor_migrate_state(
    apply: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let plan = ctx_traits_io::state::plan_migration()?;

    let runs = StateFamilyReport {
        global: ctx_traits_io::state::global_runs_root(&plan.repo_key)?.to_string(),
        legacy: ctx_traits_io::state::legacy_runs_root(&plan.canonical_repo_root).to_string(),
    };
    let debug = StateFamilyReport {
        global: ctx_traits_io::state::global_debug_root(&plan.repo_key)?.to_string(),
        legacy: ctx_traits_io::state::legacy_debug_root(&plan.canonical_repo_root).to_string(),
    };
    let cache = StateFamilyReport {
        global: ctx_traits_io::state::global_cache_root(&plan.repo_key)?.to_string(),
        legacy: ctx_traits_io::state::legacy_cache_root(&plan.canonical_repo_root).to_string(),
    };

    let moves: Vec<PlannedMoveReport> = plan
        .moves
        .iter()
        .map(|m| move_report(m.family, &m.source, &m.dest))
        .collect();
    let conflicts: Vec<PlannedMoveReport> = plan
        .conflicts
        .iter()
        .map(|c| move_report(c.family, &c.source, &c.dest))
        .collect();
    let orphans: Vec<OrphanReport> = plan
        .orphans
        .iter()
        .map(|o| OrphanReport {
            key: o.key.clone(),
            indexed_path: o.indexed_path.clone(),
        })
        .collect();

    let applied = if apply {
        let result = ctx_traits_io::state::apply_migration(&plan)?;
        Some(AppliedReport {
            moved: result
                .moved
                .iter()
                .map(|m| move_report(m.family, &m.source, &m.dest))
                .collect(),
            failed: result
                .failed
                .iter()
                .map(|(m, error)| PlannedMoveFailureReport {
                    planned: move_report(m.family, &m.source, &m.dest),
                    error: error.clone(),
                })
                .collect(),
        })
    } else {
        None
    };

    let report = MigrateStateReport {
        action: if apply { "apply" } else { "plan" },
        repo_key: plan.repo_key.clone(),
        canonical_repo_root: plan.canonical_repo_root.to_string(),
        runs,
        debug,
        cache,
        moves,
        conflicts,
        orphans,
        applied,
        note: "conflicts are never overwritten; re-run with --apply after resolving them by hand",
    };

    if json {
        print_json_report(&Envelope::ok(report), "doctor migrate-state report")?;
    } else {
        println!(
            "ctx traits doctor --migrate-state{}",
            if apply { " --apply" } else { "" }
        );
        println!("  repo-key: {}", report.repo_key);
        println!("  canonical-repo-root: {}", report.canonical_repo_root);
        println!(
            "  runs: global={} legacy={}",
            report.runs.global, report.runs.legacy
        );
        println!(
            "  debug: global={} legacy={}",
            report.debug.global, report.debug.legacy
        );
        println!(
            "  cache: global={} legacy={}",
            report.cache.global, report.cache.legacy
        );
        println!("  moves: {}", report.moves.len());
        for m in &report.moves {
            println!("    [{}] {} -> {}", m.family, m.source, m.dest);
        }
        println!("  conflicts: {}", report.conflicts.len());
        for c in &report.conflicts {
            println!(
                "    [{}] {} (dest already exists: {})",
                c.family, c.source, c.dest
            );
        }
        println!("  orphans: {}", report.orphans.len());
        for o in &report.orphans {
            println!("    {} -> {}", o.key, o.indexed_path);
        }
        if let Some(applied) = &report.applied {
            println!(
                "  applied: moved={} failed={}",
                applied.moved.len(),
                applied.failed.len()
            );
            for failure in &applied.failed {
                println!(
                    "    failed: [{}] {} -> {}: {}",
                    failure.planned.family,
                    failure.planned.source,
                    failure.planned.dest,
                    failure.error
                );
            }
        }
        println!("  note: {}", report.note);
    }

    Ok(CommandOutput::new(()))
}

// ---------------------------------------------------------------------------
// `ctx traits doctor --migrate-config [--apply]` (P514)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct AgentConfigLayerReport {
    layer: &'static str,
    path: String,
    rewrites: Vec<ctx_traits_io::harness_config::AgentConfigRewrite>,
    conflicts: Vec<ctx_traits_io::harness_config::AgentConfigConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppliedAgentConfigReport {
    rewritten: Vec<ctx_traits_io::harness_config::AppliedAgentConfigLayer>,
    failed: Vec<ctx_traits_io::harness_config::AppliedAgentConfigFailure>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct MigrateConfigReport {
    action: &'static str,
    layers: Vec<AgentConfigLayerReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    applied: Option<AppliedAgentConfigReport>,
    note: &'static str,
}

pub(crate) fn handle_doctor_migrate_config(
    apply: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let plan = ctx_traits_io::harness_config::plan_agent_config_migration(Utf8Path::new("."))?;

    let applied = if apply {
        let result = ctx_traits_io::harness_config::apply_agent_config_migration(&plan);
        Some(result)
    } else {
        None
    };

    let layers: Vec<AgentConfigLayerReport> = plan
        .iter()
        .map(|entry| AgentConfigLayerReport {
            layer: config_layer_label(entry.layer),
            path: entry.path.clone(),
            rewrites: entry.rewrites.clone(),
            conflicts: entry.conflicts.clone(),
            refusal: entry.refusal.clone(),
        })
        .collect();

    let report = MigrateConfigReport {
        action: if apply { "apply" } else { "plan" },
        layers,
        applied: applied.map(|result| AppliedAgentConfigReport {
            rewritten: result.rewritten,
            failed: result.failed,
        }),
        note: "conflicts and refused layers (generated config.toml, or a rewrite that failed round-trip verification) are never rewritten; resolve them by hand and re-run",
    };

    if json {
        print_json_report(&Envelope::ok(report), "doctor migrate-config report")?;
    } else {
        println!(
            "ctx traits doctor --migrate-config{}",
            if apply { " --apply" } else { "" }
        );
        if report.layers.is_empty() {
            println!("  no legacy [agent] keys found");
        }
        for layer in &report.layers {
            println!("  [{}] {}", layer.layer, layer.path);
            if let Some(refusal) = &layer.refusal {
                println!("    refused: {refusal}");
                continue;
            }
            for rewrite in &layer.rewrites {
                println!("    {} -> {}", rewrite.from, rewrite.to);
            }
            for conflict in &layer.conflicts {
                println!(
                    "    conflict: {} -> {} ({})",
                    conflict.from, conflict.to, conflict.reason
                );
            }
        }
        if let Some(applied) = &report.applied {
            println!(
                "  applied: rewritten={} failed={}",
                applied.rewritten.len(),
                applied.failed.len()
            );
            for failure in &applied.failed {
                println!("    failed: {}: {}", failure.path, failure.error);
            }
        }
        println!("  note: {}", report.note);
    }

    Ok(CommandOutput::new(()))
}

/// One analyzed candidate's cross-tier resolution, from the same shared
/// [`ctx_traits_io::inventory::InventoryContext`] `list` and explicit-id/query
/// run resolution use (P439): whether the trait ID this candidate would
/// import as already resolves to an existing package, and if so at which
/// origin, plus the origin of any further tier it would shadow. `None`
/// resolution means importing this candidate would introduce a brand new id.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorTraitShadow {
    trait_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    existing_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shadows: Option<String>,
}

/// One P419 trust-store problem: a stale approval — a VERIFIED record whose
/// recorded digest no longer matches the trait's current canonical digest —
/// or a record (named or legacy digest-only) that matches no currently
/// visible trait by identity or exact digest (orphaned). A moved BLOCKED
/// record is neither: it is not a stale approval (see
/// [`ctx_traits_io::trust::TrustReportRow::is_stale_approval`]), so it never
/// appears here. Never reports a `Current` row — doctor surfaces trust
/// *problems*, not the whole store; use `ctx traits trust list` for the
/// full inventory.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorTrustFinding {
    trait_id: Option<String>,
    digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_digest: Option<String>,
    state: String,
    freshness: ctx_traits_io::trust::TrustFreshness,
    remedy: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorOutput {
    #[serde(flatten)]
    report: DoctorReport,
    /// Cross-tier shadow diagnostics for every candidate that produced a
    /// trait ID, in path order. Empty when no candidate analyzed
    /// successfully.
    trait_shadow: Vec<DoctorTraitShadow>,
    /// Stale/orphaned machine trust records (P419), joined from the same
    /// [`ctx_traits_io::trust::classify_records`] `ctx traits trust list`
    /// uses. Never writes `trust.toml`. Empty when every record is current.
    trust: Vec<DoctorTrustFinding>,
    /// P446 repository-state housekeeping: nested `.ctx/.gitignore`
    /// completeness, tracked runtime paths, and a possible global-store
    /// warning.
    repo_state: RepoStateReport,
    /// P462: git worktree registrations, merged run branches, and stale
    /// gate-capture temp files.
    debris: DebrisReport,
}

/// One missing-entry finding for the invocation repository's nested
/// `.ctx/.gitignore` (P446). Absent when the file already carries every
/// canonical entry.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorGitignoreFinding {
    path: String,
    missing_entries: Vec<String>,
}

/// One tracked path that should instead be ignored, with a text-only
/// `git rm --cached` remedy (P446). Doctor never executes the remedy.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorTrackedRuntimeFinding {
    path: String,
    remedy: String,
}

/// The global `ctx` config-home store physically resolved inside a Git
/// repository (P446), most commonly a dotfiles checkout.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorGlobalStoreFinding {
    global_root: String,
    git_root: String,
    remedy: String,
}

/// P446 repository-state housekeeping report: nested-ignore completeness,
/// tracked runtime paths, and the global-store warning, gathered before
/// doctor's no-source early exit so a fresh repository with no importable
/// file still sees these findings. `applied_entries` is non-empty only when
/// `ctx traits doctor --apply` (without `--migrate-state`) actually appended
/// missing entries this call.
#[derive(serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
struct RepoStateReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    gitignore: Option<DoctorGitignoreFinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tracked: Vec<DoctorTrackedRuntimeFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    global_store: Option<DoctorGlobalStoreFinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied_entries: Vec<String>,
}

impl RepoStateReport {
    /// Empty when this report has nothing to show at all: no missing
    /// entries, no tracked runtime paths, no global-store warning, and
    /// `--apply` appended nothing either. Every panel/JSON call site checks
    /// this one method so an apply-only report (nothing wrong, something
    /// just applied) is never treated as if there were nothing to say.
    fn is_empty(&self) -> bool {
        self.gitignore.is_none()
            && self.tracked.is_empty()
            && self.global_store.is_none()
            && self.applied_entries.is_empty()
    }
}

/// One P462 debris finding: a git worktree registration whose directory is
/// gone, a `ctx/run/*` branch already merged into the default branch, or a
/// stale ctx gate failure-capture temp file. `remedy` is shown when this
/// finding is read-only; `--apply` clears exactly these three classes and
/// nothing else (never an unmerged branch, a present worktree, or any other
/// temp file).
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorDebrisFinding {
    kind: String,
    detail: String,
    remedy: String,
}

/// P462 doctor debris sweep: git worktree registrations, merged run
/// branches, and stale gate-capture temp files. `applied` is non-empty only
/// when `ctx traits doctor --apply` actually removed something this call.
#[derive(serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
struct DebrisReport {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    findings: Vec<DoctorDebrisFinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied: Vec<String>,
}

impl DebrisReport {
    fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.applied.is_empty()
    }
}

/// Gather P462 debris findings for the invocation Git repository, optionally
/// clearing (applying) each one first. Read-only outside a Git repository —
/// every finding class here is git/worktree-scoped, so there is simply
/// nothing to report there, never an error. Doctor never touches an
/// unmerged branch, a worktree whose directory still exists, or any temp
/// file outside its own deterministic gate-capture name prefix.
fn debris_doctor_diagnostics(apply: bool) -> crate::Result<DebrisReport> {
    let mut report = DebrisReport::default();
    let ctx_traits_io::state::InvocationRoot::Repo(repo_root) =
        ctx_traits_io::state::discover_invocation_root()?
    else {
        return Ok(report);
    };

    let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();

    let registrations = ctx_traits_io::worktree::list_worktree_registrations(&repo_root)?;
    let missing_registrations: Vec<_> = registrations
        .into_iter()
        .filter(|registration| !registration.path.is_dir())
        .collect();
    if !missing_registrations.is_empty() {
        if apply {
            ctx_traits_io::worktree::prune_worktree_registrations(&repo_root)?;
            report.applied.push(format!(
                "pruned {} stale git worktree registration(s)",
                missing_registrations.len()
            ));
        } else {
            for registration in &missing_registrations {
                report.findings.push(DoctorDebrisFinding {
                    kind: "worktree-registration".to_string(),
                    detail: registration.path.to_string(),
                    remedy: "directory is gone — fix: run `ctx traits doctor --apply` to prune it"
                        .to_string(),
                });
            }
        }
    }

    let runtime_config = ctx_traits_io::harness_config::resolve_runtime_config(&repo_root)?;
    let merge_policy = runtime_config.effective_merge_policy();
    let (default_branch, _source) = ctx_traits_io::worktree::resolve_default_branch(
        &repo_root,
        merge_policy.branch.as_deref(),
        &mut warnings,
    )?;
    let merged_branches =
        ctx_traits_io::worktree::run_branches_merged_into(&repo_root, &default_branch)?;
    for branch in &merged_branches {
        if apply {
            ctx_traits_io::worktree::delete_branch(&repo_root, branch, &mut warnings)?;
            report
                .applied
                .push(format!("deleted merged run branch {branch}"));
        } else {
            report.findings.push(DoctorDebrisFinding {
                kind: "merged-run-branch".to_string(),
                detail: branch.clone(),
                remedy: format!(
                    "already merged into {default_branch} — fix: run `ctx traits doctor --apply` to delete it"
                ),
            });
        }
    }

    let stale_captures = ctx_traits_io::command::stale_gate_capture_files(
        ctx_traits_io::retention::DEFAULT_DEBUG_MAX_AGE,
    )?;
    for capture in &stale_captures {
        if apply {
            ctx_traits_io::command::remove_stale_capture_file(&capture.path)?;
            report
                .applied
                .push(format!("removed stale gate-capture file {}", capture.path));
        } else {
            report.findings.push(DoctorDebrisFinding {
                kind: "stale-capture-file".to_string(),
                detail: capture.path.to_string(),
                remedy: "stale gate failure-capture temp file — fix: run `ctx traits doctor --apply` to remove it"
                    .to_string(),
            });
        }
    }

    Ok(report)
}

/// Gather P446 repository-state diagnostics for the invocation Git
/// repository, optionally applying (appending) missing nested-ignore
/// entries first. Read-only outside a Git repository — the gitignore and
/// tracked-path diagnostics are simply absent there, never an error — but
/// the global-store check always runs (it is machine-wide, not tied to the
/// invocation repository). Reuses [`ctx_traits_io::state::discover_invocation_root`]
/// (itself [`ctx_traits_io::repository::discover_repo_root_at`]) so only its
/// explicit "not a Git repository" outcome is read as absence: a genuine
/// discovery failure (unsafe/dubious ownership, corrupted `.git`, missing
/// `git`) propagates as `Err` instead of silently producing a report with
/// these findings missing.
fn repo_state_doctor_diagnostics(apply: bool) -> crate::Result<RepoStateReport> {
    let mut report = RepoStateReport::default();
    if let ctx_traits_io::state::InvocationRoot::Repo(repo_root) =
        ctx_traits_io::state::discover_invocation_root()?
    {
        if apply {
            let ensure = ctx_traits_io::gitignore::ensure_nested_gitignore(&repo_root)?;
            report.applied_entries = ensure.appended;
        }
        let plan = ctx_traits_io::gitignore::plan_nested_gitignore(&repo_root)?;
        if !plan.missing.is_empty() {
            report.gitignore = Some(DoctorGitignoreFinding {
                path: plan.path.to_string(),
                missing_entries: plan.missing,
            });
        }
        report.tracked = ctx_traits_io::gitignore::tracked_runtime_paths(&repo_root)?
            .into_iter()
            .map(|finding| DoctorTrackedRuntimeFinding {
                path: finding.path,
                remedy: finding.remedy,
            })
            .collect();
    }
    if let Some(finding) = ctx_traits_io::gitignore::global_store_inside_git_repo()? {
        report.global_store = Some(DoctorGlobalStoreFinding {
            global_root: finding.global_root.to_string(),
            git_root: finding.git_root.to_string(),
            remedy: finding.remedy,
        });
    }
    Ok(report)
}

/// Resolve stale and orphaned trust-store rows, read-only, from the same
/// shared classification `ctx traits trust list` uses (P419). Never touches
/// `trust.toml`.
fn trust_doctor_diagnostics() -> crate::Result<Vec<DoctorTrustFinding>> {
    let document = ctx_traits_io::trust::read_store()?;
    let current = crate::app::lifecycle_reporting::current_trait_digests()?;
    let rows = ctx_traits_io::trust::classify_records(&document, &current);
    Ok(rows
        .into_iter()
        .filter(|row| {
            row.is_stale_approval()
                || matches!(
                    row.freshness,
                    ctx_traits_io::trust::TrustFreshness::Orphaned
                )
        })
        .map(|row| {
            // Only a stale approval (VERIFIED, digest moved) or an orphaned
            // record ever reaches this map (the filter above excludes
            // everything else, including a moved BLOCKED record — never a
            // stale approval). An orphaned row has no current trait to
            // re-approve and `--stale` deliberately excludes it (it is not
            // an approval at all), so its remedy must name a command that
            // actually shows it (`trust list`, unfiltered) and the true
            // manual cleanup this surface has no command for.
            let remedy = if row.is_stale_approval() {
                let id = row
                    .trait_id
                    .as_deref()
                    .expect("a stale approval is always identity-bound");
                format!("ctx traits trust approve {id}")
            } else {
                "orphaned: no current trait resolves to this record; run `ctx traits trust list` \
                 to see it (orphaned rows are excluded from --stale), then remove it manually \
                 from trust.toml if it is no longer needed"
                    .to_string()
            };
            DoctorTrustFinding {
                trait_id: row.trait_id,
                digest: row.digest,
                current_digest: row.current_digest,
                state: row.state.as_str().to_string(),
                freshness: row.freshness,
                remedy,
            }
        })
        .collect())
}

/// The exact JSON serialization boundary the production `--json` branch
/// emits: pretty-printed [`DoctorOutput`], unchanged from the pre-kit
/// output. Factored out so an in-process test can byte-compare this
/// function's output against an independently evaluated pre-kit
/// `serde_json::to_string_pretty` call for the same value, catching drift
/// that a same-call-twice comparison never would.
fn doctor_json_bytes(output: &DoctorOutput) -> crate::Result<String> {
    serde_json::to_string_pretty(output)
        .map_err(|e| crate::Error::json("doctor report".to_string(), e))
}

/// Resolve each analyzed candidate's trait ID against the shared cross-tier
/// inventory, so doctor can tell the caller, before they run `ctx traits
/// import`, whether the id they are about to create already exists
/// somewhere else and would win or lose against it (P439). A candidate whose
/// id resolves to nothing yet is omitted rather than reported with an empty
/// origin.
fn trait_shadow_diagnostics(report: &DoctorReport) -> crate::Result<Vec<DoctorTraitShadow>> {
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    let mut diagnostics = Vec::new();
    for entry in &report.entries {
        let Some(trait_id) = &entry.trait_id else {
            continue;
        };
        let Some(resolution) = context.resolve_tiers(trait_id)? else {
            continue;
        };
        diagnostics.push(DoctorTraitShadow {
            trait_id: trait_id.clone(),
            existing_origin: Some(resolution.winner.origin),
            shadows: resolution
                .shadowed
                .first()
                .map(|candidate| candidate.origin.clone()),
        });
    }
    Ok(diagnostics)
}

pub(crate) fn handle_doctor(
    path: Option<&str>,
    json: bool,
    verbose: bool,
    apply: bool,
) -> crate::Result<CommandOutput<()>> {
    let root_display = path.unwrap_or(".").to_string();
    let root = Utf8Path::new(&root_display);

    // Every fallible call in this function that can carry caller-controlled
    // text (the root path, discovered relative paths, io error messages)
    // is handled explicitly here rather than via a bare `?`, and the text
    // is run through `safe()` before it enters a `crate::Error::Command`.
    // That is the single invariant for doctor's error boundary: a
    // `crate::Error` returned by `handle_doctor` is printed verbatim (full
    // `Display`, multi-line) by the generic CLI entry point
    // (`command_handlers::run`'s error renderer), which has no sanitization
    // of its own and is shared by every command — so doctor must never let
    // a raw `ctx_traits_io`/`ctx_traits_core` error (whose `Display` impls
    // freely interpolate untrusted path/content text) escape via `?`.
    // Every discovery error is caught here and rebuilt as a sanitized
    // `Command` message instead of being propagated raw.
    let discovery = match ctx_traits_io::import::discover_doctor_sources(root) {
        Ok(discovery) => discovery,
        Err(error) => {
            return Err(crate::Error::Command {
                message: format!(
                    "doctor could not read {}: {}",
                    safe(&root_display),
                    safe(&error.to_string())
                ),
            });
        }
    };

    // Resolved before the no-source-candidates check below (P419 risk: a
    // repository with no importable file still deserves its trust
    // diagnostics, so an actionable stale/orphaned finding must never be
    // hidden behind an early "nothing to import" refusal).
    let trust = trust_doctor_diagnostics()?;
    // Same reasoning applies to P446 repository-state housekeeping: a fresh
    // repository with no importable file still deserves to see a missing
    // nested `.ctx/.gitignore` or a tracked runtime path. Handled with the
    // same explicit match/`safe()` boundary as `discovery` above rather than
    // a bare `?`: a genuine Git discovery failure can carry raw stderr/path
    // text (including control sequences) that must never reach the shared
    // CLI error printer unsanitized.
    let repo_state = match repo_state_doctor_diagnostics(apply) {
        Ok(repo_state) => repo_state,
        Err(error) => {
            return Err(crate::Error::Command {
                message: format!(
                    "doctor could not resolve repository-state diagnostics: {}",
                    safe(&error.to_string())
                ),
            });
        }
    };
    // Same reasoning applies to P462 debris: a fresh repository with no
    // importable file still deserves to see stale worktree registrations,
    // merged run branches, or leftover gate-capture files.
    let debris = match debris_doctor_diagnostics(apply) {
        Ok(debris) => debris,
        Err(error) => {
            return Err(crate::Error::Command {
                message: format!(
                    "doctor could not resolve debris diagnostics: {}",
                    safe(&error.to_string())
                ),
            });
        }
    };

    // `--apply` is a self-contained repository-housekeeping mode: it must
    // succeed and print its deterministic report even when there are no
    // source candidates and a prior apply already left nothing to append
    // (the byte-idempotent second run), never fall through to the
    // no-source-candidates refusal below, which is for plain source
    // inspection only.
    if !apply
        && discovery.files.is_empty()
        && trust.is_empty()
        && repo_state.is_empty()
        && debris.is_empty()
    {
        // This error message is built from source-derived text (the
        // caller-supplied root path, and discovery error paths/messages
        // that may echo untrusted file/directory names) and is returned to
        // the caller for direct stderr display, not routed through
        // `emit_report`'s line rendering — so it must be sanitized here
        // explicitly, the same as every other human-rendered value below.
        let mut message = format!(
            "doctor found no supported SKILL.md/AGENTS.md/CLAUDE.md files under {}",
            safe(&root_display)
        );
        if !discovery.errors.is_empty() {
            let unreadable = discovery
                .errors
                .iter()
                .map(|error| format!("{}: {}", safe(&error.relative_path), safe(&error.message)))
                .collect::<Vec<_>>()
                .join("; ");
            message.push_str(&format!(" (unreadable candidates: {unreadable})"));
        }
        return Err(crate::Error::Command { message });
    }

    let mut inputs: Vec<DoctorFileInput> = discovery
        .errors
        .iter()
        .map(|error| DoctorFileInput {
            path: error.relative_path.clone(),
            outcome: DoctorFileOutcome::ReadError {
                message: error.message.clone(),
            },
        })
        .collect();
    inputs.extend(
        discovery
            .files
            .iter()
            .map(|file| analyze_file(&discovery.effective_root, file)),
    );

    let report_root = discovery.effective_root.to_string();
    let report = ctx_traits_core::import::plan::doctor::build_doctor_report(&report_root, inputs);
    let trait_shadow = trait_shadow_diagnostics(&report)?;
    let critical = report.summary.files_errored + report.summary.critical_findings;

    match OutputMode::select(json, verbose) {
        OutputMode::Json => {
            let output = DoctorOutput {
                report,
                trait_shadow,
                trust,
                repo_state,
                debris,
            };
            crate::app::tui::write_plain_line(doctor_json_bytes(&output)?)?;
        }
        OutputMode::Human(mode) => {
            let panel = compact_doctor_panel(&report, &trust, &repo_state, &debris);
            emit_human(false, &panel, mode, || {
                emit_report(
                    false,
                    || {
                        styled_doctor_lines(
                            &root_display,
                            &report,
                            &trait_shadow,
                            &trust,
                            &repo_state,
                            &debris,
                        )
                    },
                    || {
                        emit_plain_doctor_report(
                            &root_display,
                            &report,
                            &trait_shadow,
                            &trust,
                            &repo_state,
                            &debris,
                        )
                    },
                )
            })?;
        }
    }

    if critical > 0 {
        return Err(crate::Error::AlreadyReported {
            message: format!("doctor found {critical} critical finding(s)"),
            exit_code: crate::app::error::EXIT_FINDINGS,
        });
    }
    Ok(CommandOutput::new(()))
}

/// Project a [`DoctorReport`] into the compact P465 panel: one `checks` row
/// carrying `checks`/`passed`/`warnings`/`critical` in that order, one
/// section per candidate that has an actionable warning, critical finding,
/// or read/plan failure (each row the existing remediation text alone, not
/// the verbose finding message), and healthy candidates omitted entirely.
/// Built in one traversal of `report.entries` so the displayed rows and the
/// counts above them can never diverge.
fn compact_doctor_panel(
    report: &DoctorReport,
    trust: &[DoctorTrustFinding],
    repo_state: &RepoStateReport,
    debris: &DebrisReport,
) -> Panel {
    let mut passed = 0usize;
    let mut warnings = 0usize;
    let mut critical = 0usize;
    let mut sections = Vec::new();

    for entry in &report.entries {
        match entry.status {
            ctx_traits_core::import::plan::doctor::DoctorFileStatus::ReadError
            | ctx_traits_core::import::plan::doctor::DoctorFileStatus::PlanError => {
                critical += 1;
                let message = entry.error.as_deref().unwrap_or("unknown error");
                sections.push(PanelSection::new(
                    entry.path.clone(),
                    vec![PanelRow::toned(
                        "critical",
                        format!(
                            "{message} — fix: repair {} and rerun `ctx traits doctor`",
                            entry.path
                        ),
                        RowTone::Fail,
                    )],
                ));
            }
            ctx_traits_core::import::plan::doctor::DoctorFileStatus::Analyzed => {
                let mut rows = Vec::new();
                for finding in &entry.hidden_content_findings {
                    match finding.severity {
                        ctx_traits_core::audit::Severity::Critical => {
                            critical += 1;
                            rows.push(PanelRow::toned(
                                "critical",
                                finding.remediation.clone(),
                                RowTone::Fail,
                            ));
                        }
                        ctx_traits_core::audit::Severity::Warning => {
                            warnings += 1;
                            rows.push(PanelRow::toned(
                                "warning",
                                finding.remediation.clone(),
                                RowTone::Default,
                            ));
                        }
                        ctx_traits_core::audit::Severity::Advisory => {}
                    }
                }
                if rows.is_empty() {
                    passed += 1;
                } else {
                    sections.push(PanelSection::new(entry.path.clone(), rows));
                }
            }
        }
    }

    if !trust.is_empty() {
        let mut rows = Vec::new();
        for finding in trust {
            warnings += 1;
            let label = finding.trait_id.as_deref().unwrap_or("(digest-only)");
            let kind = match finding.freshness {
                ctx_traits_io::trust::TrustFreshness::Stale => "stale",
                ctx_traits_io::trust::TrustFreshness::Orphaned => "orphaned",
                ctx_traits_io::trust::TrustFreshness::Current => "current",
            };
            rows.push(PanelRow::toned(
                "warning",
                format!("{label} trust record is {kind} — fix: {}", finding.remedy),
                RowTone::Default,
            ));
        }
        sections.push(PanelSection::new("trust", rows));
    }

    if !repo_state.is_empty() {
        let mut rows = Vec::new();
        if let Some(finding) = &repo_state.gitignore {
            warnings += 1;
            rows.push(PanelRow::toned(
                "warning",
                format!(
                    "{} missing {} — fix: run `ctx traits doctor --apply`",
                    finding.path,
                    finding.missing_entries.join(", ")
                ),
                RowTone::Default,
            ));
        }
        for finding in &repo_state.tracked {
            warnings += 1;
            rows.push(PanelRow::toned(
                "warning",
                format!(
                    "{} is tracked but should be ignored — fix: {}",
                    finding.path, finding.remedy
                ),
                RowTone::Default,
            ));
        }
        if let Some(finding) = &repo_state.global_store {
            warnings += 1;
            rows.push(PanelRow::toned(
                "warning",
                format!(
                    "global ctx store at {} resolves inside {} — fix: {}",
                    finding.global_root, finding.git_root, finding.remedy
                ),
                RowTone::Default,
            ));
        }
        if !repo_state.applied_entries.is_empty() {
            passed += 1;
            rows.push(PanelRow::toned(
                "applied",
                format!(
                    "appended {} to nested .ctx/.gitignore",
                    repo_state.applied_entries.join(", ")
                ),
                RowTone::Pass,
            ));
        }
        sections.push(PanelSection::new("repo-state", rows));
    }

    if !debris.is_empty() {
        let mut rows = Vec::new();
        for finding in &debris.findings {
            warnings += 1;
            rows.push(PanelRow::toned(
                "warning",
                format!("{} {} — {}", finding.kind, finding.detail, finding.remedy),
                RowTone::Default,
            ));
        }
        for applied in &debris.applied {
            passed += 1;
            rows.push(PanelRow::toned("applied", applied.clone(), RowTone::Pass));
        }
        sections.push(PanelSection::new("debris", rows));
    }

    let checks = passed + warnings + critical;
    let status = if critical > 0 {
        PanelStatus::Critical("critical".to_string())
    } else if warnings > 0 {
        PanelStatus::Blocked("blocked".to_string())
    } else {
        PanelStatus::Passed("passed".to_string())
    };

    let mut panel = Panel::new("ctx", "doctor", status).row(PanelRow::toned(
        "checks",
        format!("{checks} · passed: {passed} · warnings: {warnings} · critical: {critical}"),
        if critical > 0 {
            RowTone::Fail
        } else if warnings > 0 {
            RowTone::Default
        } else {
            RowTone::Pass
        },
    ));
    for section in sections {
        panel = panel.section(section);
    }
    panel
}

/// Analyze one discovered doctor candidate through the exact same
/// deterministic pipeline `ctx traits import` uses
/// ([`analyze_import_source`]), so doctor's reported trait ID, digest,
/// declarations, and warnings never diverge from what running the suggested
/// import command would actually produce.
///
/// `SKILL.md`-named candidates go through [`analyze_import_source`]
/// directly: when the file's containing directory has discoverable linked
/// resources, that directory is analyzed (matching what `ctx traits import
/// --source <dir>` would do); otherwise the bare file is analyzed.
/// `AGENTS.md`/`CLAUDE.md` candidates cannot go through
/// `ctx_traits_io::import::read_agent_skill_source`'s strict `SKILL.md`-named
/// file gate — the same gate `ctx traits import` itself would hit — so they
/// run the shared pipeline's tail directly
/// ([`crate::app::import_analysis::analyze_loaded_source`]) from doctor's
/// already-read content, without re-reading the file from disk.
fn analyze_file(
    effective_root: &Utf8Path,
    file: &ctx_traits_io::import::DoctorSourceFile,
) -> DoctorFileInput {
    let outcome = plan_file(effective_root, file);
    DoctorFileInput {
        path: file.relative_path.clone(),
        outcome,
    }
}

fn plan_file(
    effective_root: &Utf8Path,
    file: &ctx_traits_io::import::DoctorSourceFile,
) -> DoctorFileOutcome {
    let relative = Utf8Path::new(&file.relative_path);
    let is_skill_md = relative.file_name() == Some("SKILL.md");

    let analysis = if is_skill_md {
        let directory_source = directory_source_for(effective_root, relative, &file.content);
        let analyze_path = directory_source
            .clone()
            .unwrap_or_else(|| effective_root.join(relative));
        analyze_import_source(
            &analyze_path,
            ctx_traits_core::import::plan::ImportProfile::AgentSkills,
            "ctx-traits-doctor",
            "agent-skills-doctor",
            &std::collections::BTreeMap::new(),
        )
        .map(|analysis| (analysis, directory_source.is_some()))
    } else {
        let source_name = source_name_for(&file.relative_path);
        let skill_path = effective_root.join(relative);
        let raw_source_digest = ctx_traits_core::digest::Digest::source(&file.content);
        let loaded_source = ctx_traits_io::import::LoadedAgentSkillSource {
            source_root: skill_path
                .parent()
                .map_or_else(|| Utf8PathBuf::from("."), Utf8Path::to_path_buf),
            skill_path: skill_path.clone(),
            source_name,
            skill_markdown: file.content.clone(),
        };
        crate::app::import_analysis::analyze_loaded_source(
            crate::app::import_analysis::LoadedSourceAnalysisRequest {
                loaded_source,
                source_display_path: file.relative_path.clone(),
                raw_source_digest,
                multi_file_source: None,
                source_profile: ctx_traits_core::import::plan::ImportProfile::AgentSkills,
                generator_package: "ctx-traits-doctor",
                scaffold_source_kind: "agent-skills-doctor",
                prior_checklists: &std::collections::BTreeMap::new(),
            },
        )
        .map(|analysis| (analysis, false))
    };

    match analysis {
        Ok((analysis, source_is_directory)) => DoctorFileOutcome::Analyzed {
            trait_id: analysis.trait_id,
            trait_name: analysis.trait_name,
            summary: analysis.summary,
            report: Box::new(analysis.report),
            scaffold: Box::new(analysis.scaffold),
            source_is_directory,
        },
        Err(error) => DoctorFileOutcome::PlanError {
            message: error.to_string(),
        },
    }
}

/// When `skill_relative`'s markdown links to at least one local file that
/// exists under its containing directory, return that directory: doctor
/// should analyze it as a multi-file source, matching what `ctx traits
/// import --source <dir>` would discover. Otherwise `None`: the bare file is
/// the analyzable unit.
fn directory_source_for(
    effective_root: &Utf8Path,
    skill_relative: &Utf8Path,
    skill_markdown: &str,
) -> Option<Utf8PathBuf> {
    let parent_relative = skill_relative.parent().unwrap_or_else(|| Utf8Path::new(""));
    let directory = effective_root.join(parent_relative);

    let links = ctx_traits_core::import::plan::discover_markdown_links(skill_markdown, "SKILL.md");
    let has_local_resource = links.iter().any(|link| {
        let target = link.resolved_path.as_str();
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with('/')
            || target.contains("..")
        {
            return false;
        }
        match std::fs::symlink_metadata(directory.join(target)) {
            Ok(metadata) => metadata.file_type().is_file(),
            Err(_) => false,
        }
    });

    has_local_resource.then_some(directory)
}

fn source_name_for(relative_path: &str) -> String {
    Utf8Path::new(relative_path)
        .parent()
        .and_then(|parent| parent.file_name())
        .or_else(|| Utf8Path::new(relative_path).file_stem())
        .unwrap_or("imported-skill")
        .to_string()
}

fn doctor_command_line(root_display: &str) -> Line {
    command_line(format!("ctx traits doctor {}", safe(root_display)))
}

/// Sanitize one piece of source-derived text before it reaches a styled or
/// plain human-rendered line: strip ANSI/CSI/OSC control sequences and
/// collapse remaining control characters to spaces. JSON output is never
/// routed through this — serde's JSON string escaping already makes control
/// characters safe there.
fn safe(text: &str) -> String {
    clean_live_text(text)
}

/// Format one scaffold declaration's ref, confidence, rationale, and source
/// anchor (file + one-based start/end line span, when present) as a single
/// sanitized human-readable line. The single formatter both `styled_doctor_lines`
/// and `emit_plain_doctor_report` call, so the two renderers can never drift
/// on which declaration fields are shown.
fn format_scaffold_declaration_line(
    declaration: &ctx_traits_core::scaffold::ScaffoldDeclaration,
) -> String {
    let anchor = match &declaration.anchor {
        Some(anchor) => format!("{}:{}-{}", anchor.file, anchor.start, anchor.end),
        None => "none".to_string(),
    };
    safe(&format!(
        "{} confidence={} anchor={} {}",
        declaration.ref_text, declaration.confidence, anchor, declaration.rationale
    ))
}

fn what_import_gives_you_summary(report: &DoctorReport) -> String {
    format!(
        "what import would give you: a typed scaffold ({} declaration(s) total across {} \
         analyzed candidate(s)) with digest-anchored provenance, written as package \
         status=draft, trust=unreviewed — quarantined until you run `ctx traits check`",
        report.summary.total_scaffold_declarations, report.summary.files_analyzed
    )
}

fn styled_doctor_lines(
    root_display: &str,
    report: &DoctorReport,
    trait_shadow: &[DoctorTraitShadow],
    trust: &[DoctorTrustFinding],
    repo_state: &RepoStateReport,
    debris: &DebrisReport,
) -> Vec<Line> {
    let mut lines = vec![doctor_command_line(root_display), Line::blank()];
    lines.push(labeled_line("root: ", &safe(&report.root)));
    lines.push(labeled_line(
        "files: ",
        &format!(
            "total={} analyzed={} errored={}",
            report.summary.files_total, report.summary.files_analyzed, report.summary.files_errored
        ),
    ));

    let mut findings_line = Line::blank();
    findings_line.push("findings: ", Tone::Muted);
    findings_line.push(
        format!("{} critical", report.summary.critical_findings),
        if report.summary.critical_findings > 0 {
            Tone::Fail
        } else {
            Tone::Default
        },
    );
    findings_line.push(", ", Tone::Muted);
    findings_line.push(
        format!("{} warning", report.summary.warning_findings),
        if report.summary.warning_findings > 0 {
            Tone::Warn
        } else {
            Tone::Default
        },
    );
    findings_line.push(", ", Tone::Muted);
    findings_line.push(
        format!("{} advisory", report.summary.advisory_findings),
        Tone::Default,
    );
    lines.push(findings_line);
    lines.push(Line::blank());

    for entry in &report.entries {
        let mut header = Line::blank();
        header.push(safe(&entry.path), Tone::Bold);
        header.push(
            format!(" [{}]", crate::app::presentation::wire_name(&entry.status)),
            match entry.status {
                ctx_traits_core::import::plan::doctor::DoctorFileStatus::Analyzed => Tone::Pass,
                _ => Tone::Fail,
            },
        );
        lines.push(header);
        if let Some(error) = &entry.error {
            lines.push(labeled_line("  error: ", &safe(error)));
            continue;
        }
        if let Some(trait_id) = &entry.trait_id {
            lines.push(labeled_line("  trait-id: ", &safe(trait_id)));
        }
        if let Some(summary) = &entry.summary {
            lines.push(labeled_line("  summary: ", &safe(summary)));
        }
        if let Some(digest) = &entry.raw_source_digest {
            lines.push(labeled_line("  raw-source-digest: ", &safe(digest)));
        }
        if let Some(evidence) = &entry.multi_file_evidence {
            lines.push(labeled_line(
                "  multi-file-evidence: ",
                &safe(&format!(
                    "included={} resource-mappings={}",
                    evidence.included_files.len(),
                    evidence.resource_mappings.len()
                )),
            ));
            for mapping in &evidence.resource_mappings {
                lines.push(labeled_line(
                    "    resource: ",
                    &safe(&format!(
                        "{} -> {}",
                        mapping.source_path, mapping.resource_id
                    )),
                ));
            }
        }
        if !entry.unsupported_fields.is_empty() {
            for unsupported in &entry.unsupported_fields {
                lines.push(labeled_line(
                    "  unsupported-field: ",
                    &safe(&format!(
                        "{}: {} ({})",
                        unsupported.source_field, unsupported.value, unsupported.reason
                    )),
                ));
            }
        }
        if !entry.review_actions.is_empty() {
            for action in &entry.review_actions {
                lines.push(labeled_line(
                    "  review-action: ",
                    &safe(&format!(
                        "{} {}: {}",
                        crate::app::presentation::wire_name(&action.action),
                        action.target,
                        action.detail
                    )),
                ));
            }
        }
        if !entry.scaffold_declarations.is_empty() {
            lines.push(labeled_line(
                "  scaffold-declarations: ",
                &format!(
                    "{} (check-required={})",
                    entry.scaffold_declarations.len(),
                    entry.scaffold_check_required
                ),
            ));
            for declaration in &entry.scaffold_declarations {
                lines.push(labeled_line(
                    "    ",
                    &format_scaffold_declaration_line(declaration),
                ));
            }
        }
        if !entry.scaffold_review_warnings.is_empty() {
            for warning in &entry.scaffold_review_warnings {
                lines.push(labeled_line("  scaffold-warning: ", &safe(warning)));
            }
        }
        for finding in &entry.hidden_content_findings {
            let mut line = Line::blank();
            line.push("  hidden-content: ", Tone::Muted);
            line.push(
                format!(
                    "[{}] ",
                    crate::app::presentation::wire_name(&finding.severity)
                ),
                match finding.severity {
                    ctx_traits_core::audit::Severity::Critical => Tone::Fail,
                    ctx_traits_core::audit::Severity::Warning => Tone::Warn,
                    ctx_traits_core::audit::Severity::Advisory => Tone::Default,
                },
            );
            line.push(safe(&finding.message), Tone::Default);
            lines.push(line);
        }
        for advisory in &entry.advisories {
            let mut line = Line::blank();
            line.push("  advisory: ", Tone::Muted);
            line.push(safe(&advisory.message), Tone::Default);
            lines.push(line);
        }
    }

    if !report.collisions.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("collisions:", ""));
        for collision in &report.collisions {
            lines.push(labeled_line(
                &format!(
                    "  {} {}: ",
                    crate::app::presentation::wire_name(&collision.kind),
                    safe(&collision.key)
                ),
                &safe(&collision.paths.join(", ")),
            ));
        }
    }

    if !trait_shadow.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("trait-shadow:", ""));
        for entry in trait_shadow {
            let mut line = Line::blank();
            line.push(format!("  {}: ", safe(&entry.trait_id)), Tone::Muted);
            match &entry.existing_origin {
                Some(origin) => {
                    line.push(format!("already exists at {}", safe(origin)), Tone::Warn)
                }
                None => line.push("no existing package".to_string(), Tone::Default),
            }
            if let Some(shadow) = &entry.shadows {
                line.push(format!(" (shadows {})", safe(shadow)), Tone::Muted);
            }
            lines.push(line);
        }
    }

    if !trust.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("trust:", ""));
        for finding in trust {
            let mut line = Line::blank();
            let label = finding.trait_id.as_deref().unwrap_or("(digest-only)");
            let kind = match finding.freshness {
                ctx_traits_io::trust::TrustFreshness::Stale => "stale",
                ctx_traits_io::trust::TrustFreshness::Orphaned => "orphaned",
                ctx_traits_io::trust::TrustFreshness::Current => "current",
            };
            line.push(format!("  {}: ", safe(label)), Tone::Muted);
            line.push(kind.to_string(), Tone::Warn);
            line.push(format!(" — fix: {}", safe(&finding.remedy)), Tone::Muted);
            lines.push(line);
        }
    }

    if !repo_state.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("repo-state:", ""));
        if let Some(finding) = &repo_state.gitignore {
            lines.push(labeled_line(
                "  gitignore: ",
                &safe(&format!(
                    "{} missing {}",
                    finding.path,
                    finding.missing_entries.join(", ")
                )),
            ));
        }
        for finding in &repo_state.tracked {
            lines.push(labeled_line(
                "  tracked: ",
                &safe(&format!("{} — fix: {}", finding.path, finding.remedy)),
            ));
        }
        if let Some(finding) = &repo_state.global_store {
            lines.push(labeled_line(
                "  global-store: ",
                &safe(&format!(
                    "{} resolves inside {} — fix: {}",
                    finding.global_root, finding.git_root, finding.remedy
                )),
            ));
        }
        if !repo_state.applied_entries.is_empty() {
            lines.push(labeled_line(
                "  applied: ",
                &safe(&repo_state.applied_entries.join(", ")),
            ));
        }
    }

    if !debris.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("debris:", ""));
        for finding in &debris.findings {
            lines.push(labeled_line(
                &format!("  {}: ", safe(&finding.kind)),
                &safe(&format!("{} — {}", finding.detail, finding.remedy)),
            ));
        }
        for applied in &debris.applied {
            lines.push(labeled_line("  applied: ", &safe(applied)));
        }
    }

    lines.push(Line::blank());
    lines.push(labeled_line("", &what_import_gives_you_summary(report)));

    if !report.next_actions.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("next-steps:", ""));
        for action in &report.next_actions {
            if let Some(command) = &action.command {
                // Sanitizing here means an adversarial path/name containing
                // control bytes will print differently than the on-disk
                // name it was built from; that's the right tradeoff (never
                // echo raw control bytes to a terminal) but it means this
                // copy-pasteable command is not guaranteed byte-identical
                // to a shell-safe invocation for such names.
                lines.push(labeled_line("  $ ", &safe(command)));
            }
            if let Some(caveat) = &action.caveat {
                lines.push(labeled_line("    caveat: ", &safe(caveat)));
            }
        }
    }

    lines
}

fn emit_plain_doctor_report(
    root_display: &str,
    report: &DoctorReport,
    trait_shadow: &[DoctorTraitShadow],
    trust: &[DoctorTrustFinding],
    repo_state: &RepoStateReport,
    debris: &DebrisReport,
) -> crate::Result<()> {
    write_plain_line(format!("ctx traits doctor {}", safe(root_display)))?;
    write_plain_line(format!("  root: {}", safe(&report.root)))?;
    write_plain_line(format!(
        "  files: total={} analyzed={} errored={}",
        report.summary.files_total, report.summary.files_analyzed, report.summary.files_errored
    ))?;
    write_plain_line(format!(
        "  findings: critical={} warning={} advisory={}",
        report.summary.critical_findings,
        report.summary.warning_findings,
        report.summary.advisory_findings
    ))?;
    for entry in &report.entries {
        write_plain_line(format!(
            "  {} [{}]",
            safe(&entry.path),
            crate::app::presentation::wire_name(&entry.status)
        ))?;
        if let Some(error) = &entry.error {
            write_plain_line(format!("    error: {}", safe(error)))?;
            continue;
        }
        if let Some(trait_id) = &entry.trait_id {
            write_plain_line(format!("    trait-id: {}", safe(trait_id)))?;
        }
        if let Some(summary) = &entry.summary {
            write_plain_line(format!("    summary: {}", safe(summary)))?;
        }
        if let Some(digest) = &entry.raw_source_digest {
            write_plain_line(format!("    raw-source-digest: {}", safe(digest)))?;
        }
        if let Some(evidence) = &entry.multi_file_evidence {
            write_plain_line(format!(
                "    multi-file-evidence: included={} resource-mappings={}",
                evidence.included_files.len(),
                evidence.resource_mappings.len()
            ))?;
            for mapping in &evidence.resource_mappings {
                write_plain_line(format!(
                    "      resource: {} -> {}",
                    safe(&mapping.source_path),
                    safe(&mapping.resource_id)
                ))?;
            }
        }
        for unsupported in &entry.unsupported_fields {
            write_plain_line(format!(
                "    unsupported-field: {}: {} ({})",
                safe(&unsupported.source_field),
                safe(&unsupported.value),
                safe(&unsupported.reason)
            ))?;
        }
        for action in &entry.review_actions {
            write_plain_line(format!(
                "    review-action: {} {}: {}",
                crate::app::presentation::wire_name(&action.action),
                safe(&action.target),
                safe(&action.detail)
            ))?;
        }
        if !entry.scaffold_declarations.is_empty() {
            write_plain_line(format!(
                "    scaffold-declarations: {} (check-required={})",
                entry.scaffold_declarations.len(),
                entry.scaffold_check_required
            ))?;
            for declaration in &entry.scaffold_declarations {
                write_plain_line(format!(
                    "      {}",
                    format_scaffold_declaration_line(declaration)
                ))?;
            }
        }
        for warning in &entry.scaffold_review_warnings {
            write_plain_line(format!("    scaffold-warning: {}", safe(warning)))?;
        }
        for finding in &entry.hidden_content_findings {
            write_plain_line(format!(
                "    hidden-content: [{}] {}: {}",
                crate::app::presentation::wire_name(&finding.severity),
                crate::app::presentation::wire_name(&finding.code),
                safe(&finding.message)
            ))?;
        }
        for advisory in &entry.advisories {
            write_plain_line(format!(
                "    advisory: {}: {}",
                advisory.code,
                safe(&advisory.message)
            ))?;
        }
    }
    if !report.collisions.is_empty() {
        write_plain_line("  collisions:")?;
        for collision in &report.collisions {
            write_plain_line(format!(
                "    {} {}: {}",
                crate::app::presentation::wire_name(&collision.kind),
                safe(&collision.key),
                safe(&collision.paths.join(", "))
            ))?;
        }
    }
    if !trait_shadow.is_empty() {
        write_plain_line("  trait-shadow:")?;
        for entry in trait_shadow {
            let existing = entry
                .existing_origin
                .as_deref()
                .map(|origin| format!("already exists at {}", safe(origin)))
                .unwrap_or_else(|| "no existing package".to_string());
            let shadows = entry
                .shadows
                .as_deref()
                .map(|shadow| format!(" (shadows {})", safe(shadow)))
                .unwrap_or_default();
            write_plain_line(format!(
                "    {}: {}{}",
                safe(&entry.trait_id),
                existing,
                shadows
            ))?;
        }
    }
    if !trust.is_empty() {
        write_plain_line("  trust:")?;
        for finding in trust {
            let label = finding.trait_id.as_deref().unwrap_or("(digest-only)");
            let kind = match finding.freshness {
                ctx_traits_io::trust::TrustFreshness::Stale => "stale",
                ctx_traits_io::trust::TrustFreshness::Orphaned => "orphaned",
                ctx_traits_io::trust::TrustFreshness::Current => "current",
            };
            write_plain_line(format!(
                "    {}: {} — fix: {}",
                safe(label),
                kind,
                safe(&finding.remedy)
            ))?;
        }
    }
    if !repo_state.is_empty() {
        write_plain_line("  repo-state:")?;
        if let Some(finding) = &repo_state.gitignore {
            write_plain_line(format!(
                "    gitignore: {} missing {}",
                safe(&finding.path),
                safe(&finding.missing_entries.join(", "))
            ))?;
        }
        for finding in &repo_state.tracked {
            write_plain_line(format!(
                "    tracked: {} — fix: {}",
                safe(&finding.path),
                safe(&finding.remedy)
            ))?;
        }
        if let Some(finding) = &repo_state.global_store {
            write_plain_line(format!(
                "    global-store: {} resolves inside {} — fix: {}",
                safe(&finding.global_root),
                safe(&finding.git_root),
                safe(&finding.remedy)
            ))?;
        }
        if !repo_state.applied_entries.is_empty() {
            write_plain_line(format!(
                "    applied: {}",
                safe(&repo_state.applied_entries.join(", "))
            ))?;
        }
    }
    if !debris.is_empty() {
        write_plain_line("  debris:")?;
        for finding in &debris.findings {
            write_plain_line(format!(
                "    {}: {} — {}",
                safe(&finding.kind),
                safe(&finding.detail),
                safe(&finding.remedy)
            ))?;
        }
        for applied in &debris.applied {
            write_plain_line(format!("    applied: {}", safe(applied)))?;
        }
    }
    write_plain_line(format!("  {}", what_import_gives_you_summary(report)))?;
    if !report.next_actions.is_empty() {
        write_plain_line("  next-steps:")?;
        for action in &report.next_actions {
            if let Some(command) = &action.command {
                // See the sanitization tradeoff note in `styled_doctor_lines`.
                write_plain_line(format!("    $ {}", safe(command)))?;
            }
            if let Some(caveat) = &action.caveat {
                write_plain_line(format!("    caveat: {}", safe(caveat)))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::audit::{Code, Finding, Severity};
    use ctx_traits_core::import::plan::doctor::{
        DoctorEntry, DoctorFileStatus, DoctorFreshness, DoctorSummary,
    };

    fn analyzed_entry(path: &str, findings: Vec<Finding>) -> DoctorEntry {
        DoctorEntry {
            path: path.to_string(),
            status: DoctorFileStatus::Analyzed,
            trait_id: Some(format!("{path}-id")),
            trait_name: Some("Demo".to_string()),
            summary: Some("demo summary".to_string()),
            raw_source_digest: Some("digest".to_string()),
            multi_file_evidence: None,
            inferred_fields: Vec::new(),
            unsupported_fields: Vec::new(),
            review_actions: Vec::new(),
            conversion_warnings: Vec::new(),
            import_warnings: Vec::new(),
            hidden_content_findings: findings,
            advisories: Vec::new(),
            scaffold_declarations: Vec::new(),
            scaffold_dependency_edges: Vec::new(),
            scaffold_review_warnings: Vec::new(),
            scaffold_check_required: true,
            source_is_directory: false,
            freshness: DoctorFreshness::Unknown,
            error: None,
        }
    }

    fn errored_entry(path: &str, status: DoctorFileStatus, message: &str) -> DoctorEntry {
        DoctorEntry {
            path: path.to_string(),
            status,
            trait_id: None,
            trait_name: None,
            summary: None,
            raw_source_digest: None,
            multi_file_evidence: None,
            inferred_fields: Vec::new(),
            unsupported_fields: Vec::new(),
            review_actions: Vec::new(),
            conversion_warnings: Vec::new(),
            import_warnings: Vec::new(),
            hidden_content_findings: Vec::new(),
            advisories: Vec::new(),
            scaffold_declarations: Vec::new(),
            scaffold_dependency_edges: Vec::new(),
            scaffold_review_warnings: Vec::new(),
            scaffold_check_required: false,
            source_is_directory: false,
            freshness: DoctorFreshness::Unknown,
            error: Some(message.to_string()),
        }
    }

    fn report_from(entries: Vec<DoctorEntry>) -> DoctorReport {
        DoctorReport {
            schema_version: "doctor/v1".to_string(),
            root: ".".to_string(),
            entries,
            summary: DoctorSummary {
                files_total: 0,
                files_analyzed: 0,
                files_errored: 0,
                critical_findings: 0,
                warning_findings: 0,
                advisory_findings: 0,
                total_scaffold_declarations: 0,
            },
            collisions: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    fn finding(severity: Severity, message: &str, remediation: &str) -> Finding {
        let code = match severity {
            Severity::Critical => Code::HtmlComment,
            Severity::Warning => Code::ColorOnColor,
            Severity::Advisory => Code::Base64Blob,
        };
        Finding {
            severity,
            code,
            message: message.to_string(),
            trait_id: "demo".to_string(),
            path: None,
            byte_offset: None,
            line: None,
            remediation: remediation.to_string(),
        }
    }

    #[test]
    fn healthy_report_is_only_a_short_counts_panel() {
        let report = report_from(vec![analyzed_entry("healthy/SKILL.md", Vec::new())]);
        let panel = compact_doctor_panel(
            &report,
            &[],
            &RepoStateReport::default(),
            &DebrisReport::default(),
        );
        let plain = panel.plain_lines();
        let joined = plain.join("\n");

        assert!(joined.contains("checks: 1"), "{joined}");
        assert!(joined.contains("passed: 1"), "{joined}");
        assert!(joined.contains("warnings: 0"), "{joined}");
        assert!(joined.contains("critical: 0"), "{joined}");
        assert!(
            !joined.contains("healthy/SKILL.md"),
            "healthy candidates must be omitted from compact output: {joined}"
        );
        assert_eq!(plain.last().unwrap(), "passed");
    }

    #[test]
    fn mixed_report_includes_every_actionable_item_with_remediation() {
        let critical = finding(Severity::Critical, "hidden text", "remove it");
        let warning = finding(Severity::Warning, "same color", "fix colors");
        let report = report_from(vec![
            analyzed_entry("healthy/SKILL.md", Vec::new()),
            analyzed_entry("Broken/SKILL.md", vec![critical, warning]),
            errored_entry(
                "unread/AGENTS.md",
                DoctorFileStatus::ReadError,
                "cannot read: permission denied",
            ),
        ]);
        let panel = compact_doctor_panel(
            &report,
            &[],
            &RepoStateReport::default(),
            &DebrisReport::default(),
        );
        let plain = panel.plain_lines();
        let joined = plain.join("\n");

        assert!(!joined.contains("healthy/SKILL.md"), "{joined}");
        assert!(joined.contains("Broken/SKILL.md"), "{joined}");
        // Compact rows carry the remediation alone; the verbose finding
        // message is not part of the compact contract.
        assert!(!joined.contains("hidden text"), "{joined}");
        assert!(joined.contains("remove it"), "{joined}");
        assert!(!joined.contains("same color"), "{joined}");
        assert!(joined.contains("fix colors"), "{joined}");
        assert!(joined.contains("unread/AGENTS.md"), "{joined}");
        assert!(
            joined.contains("cannot read: permission denied"),
            "{joined}"
        );

        // checks = 1 passed + 1 warning finding + (1 critical finding + 1
        // read-error) critical, matching the counted invariant exactly.
        assert!(joined.contains("checks: 4"), "{joined}");
        assert!(joined.contains("passed: 1"), "{joined}");
        assert!(joined.contains("warnings: 1"), "{joined}");
        assert!(joined.contains("critical: 2"), "{joined}");
        assert_eq!(plain.last().unwrap(), "critical");
    }

    #[test]
    fn warning_only_report_is_blocked_not_critical() {
        let warning = finding(Severity::Warning, "same color", "fix colors");
        let report = report_from(vec![analyzed_entry("Broken/SKILL.md", vec![warning])]);
        let plain = compact_doctor_panel(
            &report,
            &[],
            &RepoStateReport::default(),
            &DebrisReport::default(),
        )
        .plain_lines();
        assert_eq!(plain.last().unwrap(), "blocked");
    }

    #[test]
    fn mixed_case_paths_and_sanitized_source_text_survive() {
        let critical = finding(
            Severity::Critical,
            "hidden \u{1b}[31mtext\u{1b}[0m here",
            "remove \u{1b}[31mhidden\u{1b}[0m \u{7}text",
        );
        let report = report_from(vec![analyzed_entry("Mixed/CaSe/SKILL.md", vec![critical])]);
        let joined = compact_doctor_panel(
            &report,
            &[],
            &RepoStateReport::default(),
            &DebrisReport::default(),
        )
        .plain_lines()
        .join("\n");

        assert!(joined.contains("Mixed/CaSe/SKILL.md"), "{joined}");
        assert!(!joined.contains('\u{1b}'), "{joined:?}");
        assert!(!joined.contains('\u{7}'), "{joined:?}");
        assert!(
            joined.contains("hidden") && joined.contains("text"),
            "{joined}"
        );
    }

    #[test]
    fn json_output_is_stable_across_independent_serializations() {
        let entries = vec![
            analyzed_entry("healthy/SKILL.md", Vec::new()),
            errored_entry("unread/AGENTS.md", DoctorFileStatus::ReadError, "boom"),
        ];
        let report_a = report_from(entries.clone());
        let report_b = report_from(entries);
        let output_a = DoctorOutput {
            report: report_a,
            trait_shadow: Vec::new(),
            trust: Vec::new(),
            repo_state: RepoStateReport::default(),
            debris: DebrisReport::default(),
        };
        let output_b = DoctorOutput {
            report: report_b,
            trait_shadow: Vec::new(),
            trust: Vec::new(),
            repo_state: RepoStateReport::default(),
            debris: DebrisReport::default(),
        };

        // `production` goes through the exact function the `--json` branch
        // calls; `independent` re-derives the pre-kit expression directly,
        // against a separately built (but equal) `DoctorOutput`. If
        // `doctor_json_bytes` ever diverges from plain
        // `serde_json::to_string_pretty` — a stray field, a different
        // serializer, panel bytes leaking in — this stops passing.
        let production = doctor_json_bytes(&output_a).unwrap();
        let independent = serde_json::to_string_pretty(&output_b).unwrap();
        assert_eq!(production, independent);

        assert_eq!(OutputMode::select(true, false), OutputMode::Json);
        assert_eq!(OutputMode::select(true, true), OutputMode::Json);
    }
}
