//! Lifecycle-adjacent discovery, synchronization, trust, and list reporting.

use ctx_traits_core::response::CommandOutput;

use crate::app::command_handlers::print_json_report;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human, emit_one_row,
};

pub(crate) fn format_manifest_discovery(
    repo_root: &camino::Utf8Path,
    result: &ctx_traits_io::discovery::ManifestDiscovery,
) -> String {
    match result {
        ctx_traits_io::discovery::ManifestDiscovery::Found(manifest) => {
            format!(
                "manifest: {} ({})",
                manifest.path.as_str(),
                manifest.encoding
            )
        }
        ctx_traits_io::discovery::ManifestDiscovery::NotFound => {
            format!("no manifest found in {}", repo_root)
        }
        ctx_traits_io::discovery::ManifestDiscovery::Conflict { found } => {
            let paths: Vec<String> = found
                .iter()
                .map(|m| format!("  {} ({})", m.path.as_str(), m.encoding))
                .collect();
            format!(
                "multiple manifests found in {}:\n{}\nuse --manifest <path> to select one",
                repo_root,
                paths.join("\n")
            )
        }
    }
}

pub(crate) fn current_utf8_dir() -> crate::Result<camino::Utf8PathBuf> {
    let cwd_raw = std::env::current_dir().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: ".".to_string(),
            source,
        })
    })?;
    camino::Utf8PathBuf::from_path_buf(cwd_raw).map_err(|e| crate::Error::Command {
        message: format!("current directory is not UTF-8: {}", e.display()),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct InitEntryJson<'a> {
    path: &'a str,
    created: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct InitReportJson<'a> {
    entries: Vec<InitEntryJson<'a>>,
}

/// Scaffold the v2 layout: `.ctx/traits/`, `.ctx/traits/config.toml`, and,
/// optionally, a starter package. Never overwrites an existing authored
/// file and never touches the retired `.agents/traits/` layout.
pub(crate) fn handle_init(
    name: Option<&str>,
    install: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let cwd = current_utf8_dir()?;
    let report = ctx_traits_io::init::init(&cwd, name)?;

    // Scaffolding a manifest is not the same as satisfying it. Whether or not
    // `--install` was asked for, say plainly what is still missing — the old
    // behaviour wrote the manifest, said "passed", and left the first build to
    // discover that nothing had been installed.
    let mut moved_range = None;
    let installed = if install {
        // Move the declared range BEFORE installing, so the install resolves
        // what this binary supports rather than what a previous one did.
        moved_range = ctx_traits_io::init::refresh_authoring_range(&cwd)?;
        Some(ctx_traits_io::authoring_env::install_authoring_packages(
            &cwd,
        )?)
    } else {
        None
    };
    let missing = ctx_traits_io::authoring_env::missing_for_authoring(
        &cwd,
        &ctx_traits_io::init::authoring_range_spec(),
    );

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let entries = report
                .entries
                .iter()
                .map(|entry| InitEntryJson {
                    path: entry.path(),
                    created: matches!(entry, ctx_traits_io::init::InitEntry::Created(_)),
                })
                .collect();
            print_json_report(&InitReportJson { entries }, "init output")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new("ctx", "init", PanelStatus::Passed("passed".to_string()));
            for entry in &report.entries {
                let (label, path, tone) = match entry {
                    ctx_traits_io::init::InitEntry::Created(path) => {
                        ("created", path.as_str(), RowTone::Pass)
                    }
                    ctx_traits_io::init::InitEntry::Preserved(path) => {
                        ("preserved", path.as_str(), RowTone::Default)
                    }
                };
                panel = panel.row(PanelRow::toned(label, path, tone));
            }
            if let Some(previous) = &moved_range {
                panel = panel.row(PanelRow::toned(
                    "range",
                    format!(
                        "{previous} -> {}; traits built against the old range may need rebuilding",
                        ctx_traits_io::init::authoring_range_spec()
                    ),
                    RowTone::Warn,
                ));
            }
            if let Some(manager) = installed {
                panel = panel.row(PanelRow::toned(
                    "installed",
                    manager.binary(),
                    RowTone::Pass,
                ));
            }
            for item in &missing {
                panel = panel.row(PanelRow::toned("next", item.remedy(), RowTone::Warn));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Sync every discovered package under the repo-local trait source root.
pub(crate) fn handle_sync_all(
    manifest: Option<&str>,
    locked: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let cwd = current_utf8_dir()?;

    // Project (`[dependencies]`) npm packages reconcile before package-local
    // dependency sync (P438): an existing compatible project lock is
    // authoritative so a fresh clone reproduces the same bytes, and this runs
    // even when there are no authored packages under `.ctx/traits`.
    let project_warnings = ctx_traits_io::distribution::reconcile_project_dependencies(
        &cwd,
        locked,
        ctx_traits_io::distribution::resolve_registry_options(&cwd),
    )?;
    if !json {
        for warning in &project_warnings {
            emit_one_row(
                false,
                "ctx",
                "vendor",
                PanelStatus::Blocked(warning.clone()),
            )?;
        }
    }
    if locked && !project_warnings.is_empty() {
        return Err(crate::Error::Command {
            message: "sync --locked detected project dependency drift".to_string(),
        });
    }

    let packages = ctx_traits_io::discovery::trait_package_variants(&cwd)?;
    if packages.is_empty() {
        if json {
            print_json_report(&project_warnings, "project dependency sync warnings")?;
        }
        return Ok(CommandOutput::new(()));
    }
    if json {
        let mut reports = Vec::new();
        for package in &packages {
            let mode = if locked {
                ctx_traits_io::dependency::SyncMode::VerifyLocked
            } else {
                ctx_traits_io::dependency::SyncMode::Write
            };
            reports.push(ctx_traits_io::dependency::sync(
                ctx_traits_io::dependency::SyncRequest {
                    repo_root: &cwd,
                    manifest_path: manifest.map(camino::Utf8Path::new),
                    trait_file: Some(package.trait_path.as_path()),
                    mode,
                    vendor_root_override: None,
                },
            )?);
        }
        print_json_report(&reports, "dependency sync reports")?;
        return Ok(CommandOutput::new(()));
    }
    for package in &packages {
        handle_sync(manifest, Some(package.trait_path.as_str()), locked, json)?;
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_sync(
    manifest: Option<&str>,
    file: Option<&str>,
    locked: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let cwd = current_utf8_dir()?;
    let manifest_path = manifest.map(camino::Utf8Path::new);
    let trait_file = file.map(camino::Utf8Path::new);
    let mode = if locked {
        ctx_traits_io::dependency::SyncMode::VerifyLocked
    } else {
        ctx_traits_io::dependency::SyncMode::Write
    };
    let report = ctx_traits_io::dependency::sync(ctx_traits_io::dependency::SyncRequest {
        repo_root: &cwd,
        manifest_path,
        trait_file,
        mode,
        vendor_root_override: None,
    })?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&report, "dependency sync report")?;
        }
        OutputMode::Human(mode) => {
            let status = if report.passed {
                PanelStatus::Passed("passed".to_string())
            } else {
                PanelStatus::Blocked("blocked".to_string())
            };
            let mut panel = Panel::new("ctx", "dependency install", status)
                .row(PanelRow::toned(
                    "repo-root",
                    report.repo_root.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "project-manifest",
                    report.project_manifest.as_deref().unwrap_or("not found"),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "lockfile",
                    report.lockfile.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "locked",
                    report.locked.to_string(),
                    RowTone::Default,
                ));
            for warning in &report.warnings {
                panel = panel.row(PanelRow::toned("warning", warning, RowTone::Fail));
            }
            if report.dependencies.is_empty() {
                panel = panel.row(PanelRow::toned("dependencies", "none", RowTone::Default));
            } else {
                for dependency in &report.dependencies {
                    let mut value = format!(
                        "id={} version={} status={} vendor={} source={} {} source-digest={} \
                         canonical-digest={} model-visible-digest={} \
                         resource-manifest-digest={}",
                        dependency.id,
                        dependency.resolved_version,
                        dependency.status,
                        dependency.vendored_path,
                        dependency.source_kind,
                        dependency.source_path,
                        dependency.source_digest,
                        dependency.canonical_digest,
                        dependency.model_visible_digest,
                        dependency.resource_manifest_digest,
                    );
                    for warning in &dependency.warnings {
                        value.push_str(&format!(" warning={warning}"));
                    }
                    panel = panel.row(PanelRow::toned(
                        dependency.alias.as_str(),
                        value,
                        if dependency.warnings.is_empty() {
                            RowTone::Default
                        } else {
                            RowTone::Fail
                        },
                    ));
                }
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    if locked && !report.passed {
        return Err(crate::Error::Command {
            message: "vendor --locked detected dependency drift".to_string(),
        });
    }
    Ok(CommandOutput::new(()))
}

/// `ctx traits trust --approved`: named-trait resolution wins; when `operand`
/// does not resolve as a trait, falls back to installed-package bulk
/// approval (P419's preserved `trust --approved <package>` behavior). `--digest`
/// bypasses both resolvers for scripts.
pub(crate) fn handle_trust_approve(
    operand: Option<String>,
    digest: Option<String>,
    all_current: bool,
    reason: Option<String>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    if all_current {
        let targets = current_trait_digests_with_roots()?;
        let guards: Vec<_> = targets
            .iter()
            .map(|(trait_id, variant, digest, trait_root)| {
                ctx_traits_io::trust::evaluate_approval_guard(
                    trait_id,
                    variant.as_deref(),
                    digest,
                    camino::Utf8Path::new(trait_root),
                )
            })
            .collect();
        // A lock-drifted member refuses the whole batch, naming every
        // offender in one report — partial batch approval is exactly what
        // the shared lock exists to prevent (P439/P534 review blocker 1).
        let refused: Vec<&ctx_traits_io::trust::ApprovalGuard> =
            guards.iter().filter(|guard| guard.refused()).collect();
        if !refused.is_empty() {
            let detail = refused
                .iter()
                .map(|guard| {
                    format!(
                        "{}: {}",
                        guard.trait_id,
                        guard.refusal.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(crate::Error::Command {
                message: format!("trust --approved --all-current refused: {detail}"),
            });
        }
        let updates: Vec<_> = targets
            .iter()
            .map(|(trait_id, _variant, digest, _root)| {
                ctx_traits_io::trust::DigestTrustUpdate::named(
                    trait_id.clone(),
                    digest.clone(),
                    ctx_traits_io::trust::TrustState::Verified,
                    reason.clone(),
                )
            })
            .collect();
        let written = ctx_traits_io::trust::update_digests_locked(&updates)?;
        match OutputMode::select(json, false) {
            OutputMode::Json => print_json_report(&written, "trust --approved --all-current")?,
            OutputMode::Human(mode) => {
                let mut panel = Panel::new(
                    "ctx",
                    "trust --approved --all-current",
                    PanelStatus::Passed("passed".to_string()),
                )
                .row(PanelRow::toned(
                    "approved",
                    written.len().to_string(),
                    RowTone::Default,
                ));
                let warnings: Vec<PanelRow> = guards
                    .iter()
                    .filter_map(|guard| {
                        guard.warning.as_deref().map(|warning| {
                            PanelRow::toned(guard.trait_id.as_str(), warning, RowTone::Warn)
                        })
                    })
                    .collect();
                if !warnings.is_empty() {
                    panel = panel.section(PanelSection::new("warnings", warnings));
                }
                emit_human(false, &panel, mode, || Ok(()))?;
            }
        }
        return Ok(CommandOutput::new(()));
    }
    if let Some(digest) = digest {
        return handle_trust_digest_update(
            digest,
            ctx_traits_io::trust::TrustState::Verified,
            reason,
            json,
        );
    }
    let operand = operand.expect("clap enforces operand or --digest");
    // A native family is approved WHOLE. Each variant carries its own
    // canonical digest, so a per-variant trust model would make `trust
    // approve implement` silently cover one variant and leave
    // `implement:smart` unreviewed until a run refused. Nothing a reviewer
    // inspects is per-variant — the variants are one authored package, built
    // together and moving together — so the family root approves every
    // variant in one journaled act.
    if !operand.contains(':')
        && let Some(variants) = family_variant_files(&operand)?
    {
        // ONE act, not one per variant: `update_digests_locked` gives every
        // update in a call the same sequence, and currency is per act, so all
        // five variants stay current together. Five separate calls would
        // make each variant supersede the previous and leave only the last
        // approved.
        let mut updates = Vec::with_capacity(variants.len());
        for variant in &variants {
            let (trait_ref, trait_root, _source, canonical) =
                ctx_traits_io::run::load_trait(variant)?;
            let guard = ctx_traits_io::trust::evaluate_approval_guard(
                trait_ref.id.as_str(),
                trait_ref.variant.as_deref(),
                canonical.as_str(),
                &trait_root,
            );
            if guard.refused() {
                return Err(crate::Error::Command {
                    message: format!(
                        "trust --approved {operand} refused: {}",
                        guard.refusal.as_deref().unwrap_or("")
                    ),
                });
            }
            updates.push(ctx_traits_io::trust::DigestTrustUpdate::named(
                trait_ref.id.as_str().to_string(),
                canonical.as_str().to_string(),
                ctx_traits_io::trust::TrustState::Verified,
                reason.clone(),
            ));
        }
        let approved = ctx_traits_io::trust::update_digests_locked(&updates)?.len();
        if let OutputMode::Human(mode) = OutputMode::select(json, false) {
            let panel = Panel::new(
                "ctx",
                "trust --approved",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned("family", operand.clone(), RowTone::Default))
            .row(PanelRow::toned(
                "variants approved",
                approved.to_string(),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
        return Ok(CommandOutput::new(()));
    }
    // A *vendored* native family package (P535, e.g. a folded `implement`
    // package installed via `path:`) must also be approved whole, exactly
    // like the repo-authored case just above: ordinary named-trait
    // resolution below only ever resolves a bare id to the family's default
    // variant, which would silently leave every other variant unreviewed.
    // This must be checked before trait resolution — never after — so it is
    // never shadowed by that single-variant resolution succeeding first.
    if !operand.contains(':') {
        let invocation = ctx_traits_io::state::discover_invocation_root()?;
        let repo_root =
            ctx_traits_io::state::project_tier_root(&invocation).map(|root| root.to_path_buf());
        if let Some(resolved) =
            ctx_traits_io::distribution::resolve_family_package(repo_root.as_deref(), &operand)?
        {
            let leaves = resolved.entry.traits.len();
            let report = ctx_traits_io::distribution::approve_resolved_package(resolved, reason)?;
            match OutputMode::select(json, false) {
                OutputMode::Json => print_json_report(&report, "trust report")?,
                OutputMode::Human(mode) => {
                    let panel = Panel::new(
                        "ctx",
                        format!("trust --approved {}", report.package),
                        PanelStatus::Passed("passed".to_string()),
                    )
                    .row(PanelRow::toned(
                        "alias",
                        report.alias.as_str(),
                        RowTone::Default,
                    ))
                    .row(PanelRow::toned(
                        "scope",
                        report.scope.as_str(),
                        RowTone::Default,
                    ))
                    .row(PanelRow::toned(
                        "leaves approved",
                        leaves.to_string(),
                        RowTone::Default,
                    ));
                    emit_human(false, &panel, mode, || Ok(()))?;
                }
            }
            return Ok(CommandOutput::new(()));
        }
    }
    match resolve_trust_approve_target(&operand)? {
        Some(file) => handle_trust_named_update(
            &file,
            ctx_traits_io::trust::TrustState::Verified,
            reason,
            json,
            "trust --approved",
        ),
        None => crate::app::distribution::handle_approve(&operand, reason, json),
    }
}

/// Every variant manifest of the local native family named by `operand`, or
/// `None` when the operand is not a local family root. Ordered by name so a
/// batch approval is deterministic.
fn family_variant_files(operand: &str) -> crate::Result<Option<Vec<String>>> {
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    let root = ctx_traits_io::layout::trait_authoring_root_path(context.repo_root_for_paths())
        .join(operand);
    let Some(table) = ctx_traits_io::family_manifest::read_family_table(
        &ctx_traits_io::layout::package_manifest_path(root.as_ref()),
    )?
    else {
        return Ok(None);
    };
    let files = table
        .variants
        .values()
        .map(|variant| root.join(&variant.relative_path).to_string())
        .collect::<Vec<_>>();
    Ok((!files.is_empty()).then_some(files))
}

/// `trust --approved <trait|package>`'s operand resolution seam: trait
/// resolution wins whenever `operand` actually resolves — as an existing
/// literal file path, or as a trait id in any tier via
/// [`ctx_traits_io::run::try_resolve_trait_id`] — and falls through to
/// `None` (package resolution) only for that function's own two senses of
/// "not a trait": `operand` is not shaped like a trait reference at all
/// (e.g. a dotted installed-package name/alias, `lodash.debounce`), or it
/// resolves to no candidate in any tier. Any OTHER error propagates
/// unchanged — a malformed/unreadable local package must surface its own
/// trait error, never silently fall back to approving a same-named package
/// instead. No local-shadow/tier-precedence logic is reimplemented here;
/// this seam delegates entirely to the one authoritative IO resolver.
///
/// Deliberately NOT the shared `resolve_optional_trait_target`: that
/// helper treats any operand containing a `.` as a literal file path
/// regardless of whether it exists (fine for `--file`-shaped callers, wrong
/// here — it would commit a dotted package alias to a nonexistent path and
/// never reach package fallback). This seam instead only ever treats a
/// `.`-bearing operand as a literal path when that exact path exists.
fn resolve_trust_approve_target(operand: &str) -> crate::Result<Option<String>> {
    let candidate = camino::Utf8Path::new(operand);
    if candidate.extension().is_some() && candidate.exists() {
        return Ok(Some(operand.to_string()));
    }
    if let Some((path, _source)) = ctx_traits_io::run::try_resolve_trait_id(operand)? {
        return Ok(Some(path.to_string()));
    }
    resolve_shared_builtin_target(operand)
}

/// A built-in package that is embedded and published but NOT runnable
/// (`spec`), resolved for trust purposes only.
///
/// Every resolver `try_resolve_trait_id` consults filters built-in
/// candidates through `runnable_package`, deliberately: a package with no
/// procedure must never be offered by `list`, selected by a query, or
/// reached by `run`. But trust is not selection. A dependent materializes a
/// shared package's resources only once that package's own canonical digest
/// is verified, and the runtime says so in as many words — "run `ctx traits
/// trust --approved spec`" — which the runnable filter turned into a dead end:
/// the instruction resolved to no trait, fell through to installed-package
/// approval, and failed with "no installed package matches alias or npm
/// package".
///
/// So this looks past `runnable` where the runnable filter is the wrong
/// question, and only there: an id that names an embedded package resolves
/// to its published manifest in the built-in store. Selection surfaces are
/// untouched.
fn resolve_shared_builtin_target(operand: &str) -> crate::Result<Option<String>> {
    if ctx_traits_core::builtin_trait_packages::package(operand)
        .is_none_or(|package| package.runnable)
    {
        return Ok(None);
    }
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    Ok(ctx_traits_io::builtin_store::resolve_builtin_manifest_path(
        context.repo_root_for_paths(),
        operand,
    )?
    .map(|path| path.to_string()))
}

/// `ctx traits trust --blocked`: named-trait resolution only (no package
/// fallback — blocking a whole installed package is not part of P419's
/// preserved surface). `--digest` bypasses resolution for scripts.
pub(crate) fn handle_trust_block(
    operand: Option<String>,
    digest: Option<String>,
    reason: Option<String>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    if let Some(digest) = digest {
        return handle_trust_digest_update(
            digest,
            ctx_traits_io::trust::TrustState::Blocked,
            reason,
            json,
        );
    }
    let operand = operand.expect("clap enforces operand or --digest");
    let file = crate::app::command_handlers::resolve_trait_target(
        Some(&operand),
        None,
        "trust --blocked",
    )?;
    handle_trust_named_update(
        &file,
        ctx_traits_io::trust::TrustState::Blocked,
        reason,
        json,
        "trust --blocked",
    )
}

fn handle_trust_digest_update(
    digest: String,
    state: ctx_traits_io::trust::TrustState,
    reason: Option<String>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    ctx_traits_core::digest::Digest::parse(&digest).map_err(|_| crate::Error::Command {
        message: format!("--digest {digest:?} is not a valid sha256:<hex> digest"),
    })?;
    let update = ctx_traits_io::trust::update_digest(&digest, state, reason)?;
    print_trust_update(&update, json)
}

/// Shared named-trait trust write for `trust --approved`/`trust --blocked` and the
/// hidden `review --approve/--deny` compatibility alias (P419): the trait's
/// current canonical digest is resolved fresh, the write is appended as a
/// new event under one lock (history is append-only — never a replacement of
/// prior records), and the same rendering/next-action logic applies
/// regardless of which command name reached here. An approve (never a
/// block) first passes guards (a)/(c) — [`ctx_traits_io::trust::evaluate_approval_guard`]
/// — outside the lock; a lock/canonical mismatch refuses before any write
/// (P534 review blocker 1).
pub(crate) fn handle_trust_named_update(
    file: &str,
    state: ctx_traits_io::trust::TrustState,
    reason: Option<String>,
    json: bool,
    action: &str,
) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, trait_root, _source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;
    let package_status = ctx_traits_io::lifecycle::resolve_package_status(&trait_root)?;
    let guard = matches!(state, ctx_traits_io::trust::TrustState::Verified).then(|| {
        ctx_traits_io::trust::evaluate_approval_guard(
            trait_ref.id.as_str(),
            trait_ref.variant.as_deref(),
            canonical_digest.as_str(),
            &trait_root,
        )
    });
    if let Some(refusal) = guard.as_ref().and_then(|guard| guard.refusal.as_deref()) {
        return Err(crate::Error::Command {
            message: format!("{action} refused: {refusal}"),
        });
    }
    let update = ctx_traits_io::trust::update_named_digest(
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
        state,
        reason,
    )?;
    let live_sessions = if matches!(state, ctx_traits_io::trust::TrustState::Blocked) {
        ctx_traits_io::run_liveness::live_sessions_pinned_to_digest(&update.digest)
    } else {
        Vec::new()
    };
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            #[derive(serde::Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct TrustUpdateReport<'a> {
                #[serde(flatten)]
                update: &'a ctx_traits_io::trust::TrustUpdate,
                live_sessions: Vec<TrustLiveSessionJson<'a>>,
            }
            #[derive(serde::Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct TrustLiveSessionJson<'a> {
                session_id: &'a str,
                run_id: &'a str,
            }
            print_json_report(
                &TrustUpdateReport {
                    update: &update,
                    live_sessions: live_sessions
                        .iter()
                        .map(|sighting| TrustLiveSessionJson {
                            session_id: sighting.session_id.as_str(),
                            run_id: sighting.run_id.as_str(),
                        })
                        .collect(),
                },
                "trust update",
            )?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new("ctx", action, PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned(
                    "trait",
                    trait_ref.id.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "state",
                    update.state.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "digest",
                    update.digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "sequence",
                    update.seq.to_string(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "store",
                    update.path.as_str(),
                    RowTone::Default,
                ));
            if let Some(reason) = update.reason.as_deref() {
                panel = panel.row(PanelRow::toned("reason", reason, RowTone::Default));
            }
            if let Some(warning) = guard.as_ref().and_then(|guard| guard.warning.as_deref()) {
                panel = panel.row(PanelRow::toned("warning", warning, RowTone::Warn));
            }
            if !live_sessions.is_empty() {
                panel = panel.row(PanelRow::toned(
                    "live-sessions",
                    format!(
                        "{} session(s) appear live and pinned to this digest ({}) — kill \
                         explicitly if intended, this block does not stop them",
                        live_sessions.len(),
                        live_sessions
                            .iter()
                            .map(|sighting| sighting.run_id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    RowTone::Warn,
                ));
            }
            if let Some(supersedes) = update.supersedes.as_ref() {
                panel = panel.row(PanelRow::toned(
                    "supersedes",
                    format!(
                        "{}, approved {}",
                        supersedes.digest,
                        supersedes
                            .approved_at
                            .as_deref()
                            .and_then(|epoch| epoch.parse::<u64>().ok())
                            .map(|epoch| format!(
                                "{} ago",
                                super::tui::human_elapsed_text(epoch_elapsed(epoch))
                            ))
                            .unwrap_or_else(|| "at an unknown time".to_string())
                    ),
                    RowTone::Default,
                ));
            }
            panel = panel.row(PanelRow::toned(
                "note",
                "trust does not prove future model behavior; a later canonical edit changes the \
                 digest and reverts the trait to unreviewed",
                RowTone::Default,
            ));
            if matches!(state, ctx_traits_io::trust::TrustState::Verified)
                && package_status == ctx_traits_core::manifest::PackageStatus::Draft
            {
                panel = panel.next(PanelRow::toned(
                    "next",
                    format!(
                        "this machine trusts the current digest, but package status is still \
                         draft; run `ctx traits state --active {}` before it can run",
                        trait_ref.id.as_str()
                    ),
                    RowTone::Default,
                ));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Elapsed wall-clock time since a stored Unix-epoch-seconds timestamp, for
/// [`super::tui::human_elapsed_text`] — mirrors the dashboard's own
/// `format_epoch_ago` derivation so the "supersedes ... ago" wording reads
/// identically wherever a stored trust timestamp is rendered.
fn epoch_elapsed(epoch_secs: u64) -> std::time::Duration {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(epoch_secs);
    std::time::Duration::from_secs(now.saturating_sub(epoch_secs))
}

fn print_trust_update(
    update: &ctx_traits_io::trust::TrustUpdate,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(update, "trust update")?;
        }
        OutputMode::Human(mode) => {
            let mut panel = Panel::new("ctx", "trust", PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned(
                    "state",
                    update.state.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "digest",
                    update.digest.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "sequence",
                    update.seq.to_string(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "store",
                    update.path.as_str(),
                    RowTone::Default,
                ));
            if let Some(reason) = update.reason.as_deref() {
                panel = panel.row(PanelRow::toned("reason", reason, RowTone::Default));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Every currently visible trait's `(id, current canonical digest)`, from
/// the same [`resolve_trait_inventory`] scan `list`/doctor use — the single
/// join key `ctx_traits_io::trust::classify_records` needs, so `trust`
/// reporting never re-derives digests through a separate resolution path.
pub(crate) fn current_trait_digests() -> crate::Result<Vec<(String, String)>> {
    Ok(current_trait_digests_with_roots()?
        .into_iter()
        .map(|(id, _variant, digest, _root)| (id, digest))
        .collect())
}

type CurrentTraitDigestWithRoot = (String, Option<String>, String, String);

/// [`current_trait_digests`], additionally carrying each trait's package
/// root — `trust --approved --all-current` needs it to evaluate guards (a)/(c)
/// per target before writing, the same as a single named `trust --approved`.
pub(crate) fn current_trait_digests_with_roots() -> crate::Result<Vec<CurrentTraitDigestWithRoot>> {
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    let mut targets = std::collections::BTreeMap::new();
    // `resolve_trait_inventory` already expands a native family into one row
    // per declared variant (0150), so it alone now supplies every
    // (id, variant) target this needs — a second repo-only walk over
    // `trait_package_variants` re-deriving the same rows would just be
    // redundant load/lifecycle-resolution work.
    for row in resolve_trait_inventory(&context)? {
        if let TraitInventoryRow::Resolved(entry) = row {
            targets.insert(
                (entry.id.clone(), entry.variant.clone()),
                (
                    entry.id,
                    entry.variant,
                    entry.canonical_digest,
                    entry.trait_root,
                ),
            );
        }
    }
    Ok(targets.into_values().collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TrustStatusJson<'a> {
    trait_id: &'a str,
    /// Additive (0150): the trait's variant name, when it is one member of a
    /// native family — `None` for a family-less trait.
    #[serde(skip_serializing_if = "Option::is_none")]
    variant: Option<&'a str>,
    package_status: &'a str,
    current_digest: &'a str,
    verdict: &'a str,
    recorded_digest: Option<&'a str>,
    recorded_state: Option<&'a str>,
    freshness: Option<ctx_traits_io::trust::TrustFreshness>,
}

/// `ctx traits trust <trait|family|family:variant>`: dispatches on the raw
/// operand spelling, before any single-file resolution collapses a bare
/// family id to its default variant — mirroring `handle_trust_approve`'s
/// operand resolution order exactly, so what this reports for a family
/// covers exactly what `trust --approved <family>` would approve in one act
/// (0150). A bare family operand reports every declared variant; anything
/// else (a single trait id, `family:variant`, or `--file`) resolves and
/// reports that one trait, echoing its variant when it has one.
pub(crate) fn handle_trust_status(
    trait_arg: Option<&str>,
    file: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    if let Some(operand) = trait_arg
        && file.is_none()
        && !operand.contains(':')
        && let Some(variants) = family_variant_files(operand)?
    {
        return handle_trust_status_family(operand, &variants, json);
    }
    let resolved = crate::app::command_handlers::resolve_trait_target(trait_arg, file, "trust")?;
    handle_trust_status_single(&resolved, json)
}

/// `ctx traits trust <family>`: one row per declared variant plus the
/// conservative cross-variant aggregate — never a single verdict standing in
/// for the whole family (0150's core invariant).
fn handle_trust_status_family(
    family: &str,
    variant_files: &[String],
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    struct VariantStatus {
        variant: String,
        current_digest: String,
        verdict: ctx_traits_core::r#trait::TrustVerdict,
    }

    let mut statuses = Vec::with_capacity(variant_files.len());
    for path in variant_files {
        let (trait_ref, _trait_root, _source_digest, canonical_digest) =
            ctx_traits_io::run::load_trait(path)?;
        let verdict = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
            trait_ref.id.as_str(),
            canonical_digest.as_str(),
        )?;
        statuses.push(VariantStatus {
            variant: trait_ref.variant.unwrap_or_else(|| "default".to_string()),
            current_digest: canonical_digest.as_str().to_string(),
            verdict,
        });
    }
    statuses.sort_by(|a, b| a.variant.cmp(&b.variant));
    let verdict_labels: Vec<&str> = statuses
        .iter()
        .map(|status| status.verdict.display_name())
        .collect();
    let aggregate = super::trust_story::aggregate_trust_label(&verdict_labels);

    if json {
        print_json_report(
            &TrustFamilyStatusJson {
                family,
                aggregate: &aggregate,
                variants: statuses
                    .iter()
                    .map(|status| TrustFamilyVariantJson {
                        variant: &status.variant,
                        current_digest: &status.current_digest,
                        verdict: status.verdict.display_name(),
                    })
                    .collect(),
            },
            "trust status (family)",
        )?;
    } else {
        println!("ctx traits trust {family}");
        println!("  aggregate: {aggregate}");
        for status in &statuses {
            println!(
                "  variant={} current-digest={} verdict={}",
                status.variant,
                status.current_digest,
                status.verdict.display_name()
            );
        }
        // Pin the store semantics the aggregate above reports against: a
        // family-wide `trust --approved` covers every variant in one journaled
        // act (`handle_trust_approve`), so this footer's remedy is never
        // narrower than what actually clears the family's gates.
        println!(
            "  next: `ctx traits trust --approved {family}` re-approves all {} variants in one act",
            statuses.len()
        );
    }
    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TrustFamilyVariantJson<'a> {
    variant: &'a str,
    current_digest: &'a str,
    verdict: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TrustFamilyStatusJson<'a> {
    family: &'a str,
    aggregate: &'a str,
    variants: Vec<TrustFamilyVariantJson<'a>>,
}

/// `ctx traits trust <trait>`: reports resolved trait ID, package status,
/// current canonical digest, current verdict, and digest drift, joined from
/// the same [`ctx_traits_io::trust::classify_records`] shared by `trust
/// list` and `doctor`.
fn handle_trust_status_single(file: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, trait_root, _source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;
    let package_status = ctx_traits_io::lifecycle::resolve_package_status(&trait_root)?;
    let verdict = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;

    let document = ctx_traits_io::trust::read_store()?;
    let current = vec![(
        trait_ref.id.as_str().to_string(),
        canonical_digest.as_str().to_string(),
    )];
    let rows = ctx_traits_io::trust::classify_records(&document, &current);
    // Selects by the store's own append-only sequence, never wall-clock
    // `updated_at` (P534 review blocker 2) — `seq` is the only authority
    // for which identity-bound record is current.
    let recorded = document
        .record_for_current(trait_ref.id.as_str(), canonical_digest.as_str())
        .and_then(|record| {
            // `seq` is absent in older stores, so `(seq, digest)` cannot
            // identify a record. Keep the selector's exact append-only record
            // identity when joining to its classified row.
            document
                .digests
                .iter()
                .position(|candidate| std::ptr::eq(candidate, record))
                .and_then(|index| rows.get(index))
        });

    // Echo the variant-qualified id whenever this trait is one member of a
    // native family — a bare `ctx traits trust <id>` header for
    // `<family>:<variant>` silently dropped the qualifier the caller asked
    // about (0150).
    let display_ref = match &trait_ref.variant {
        Some(variant) => format!("{}:{variant}", trait_ref.id.as_str()),
        None => trait_ref.id.as_str().to_string(),
    };

    if json {
        print_json_report(
            &TrustStatusJson {
                trait_id: trait_ref.id.as_str(),
                variant: trait_ref.variant.as_deref(),
                package_status: package_status.display_name(),
                current_digest: canonical_digest.as_str(),
                verdict: verdict.display_name(),
                recorded_digest: recorded.map(|row| row.digest.as_str()),
                recorded_state: recorded.map(|row| row.state.as_str()),
                freshness: recorded.map(|row| row.freshness),
            },
            "trust status",
        )?;
    } else {
        println!("ctx traits trust {display_ref}");
        println!("  package-status: {}", package_status.display_name());
        println!("  current-digest: {}", canonical_digest.as_str());
        println!("  verdict: {}", verdict.display_name());
        // Shares P419's state-aware stale-approval rule
        // (`TrustReportRow::is_stale_approval`) with `trust --list --stale`
        // and doctor via `trust_story::classify_trust`, which delegates the
        // same split rather than re-deriving it: a moved VERIFIED record
        // recommends re-approval, but a moved BLOCKED record must never
        // suggest overturning a deliberate block — its current digest
        // already reads unreviewed above. Wired to the shared
        // `trust_story` sentences (P473 §4.9) so this CLI panel and the
        // TRUST screen never carry two voices.
        let class = super::trust_story::classify_trust(recorded);
        println!("  drift: {}", super::trust_story::state_sentence(class));
        let surface = super::trust_story::Surface::Cli {
            trait_id: display_ref.clone(),
        };
        println!(
            "  next: {}",
            super::trust_story::next_action(class, None, &surface)
        );
    }
    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TrustListRowJson<'a> {
    trait_id: Option<&'a str>,
    digest: &'a str,
    current_digest: Option<&'a str>,
    state: &'a str,
    freshness: ctx_traits_io::trust::TrustFreshness,
    updated_at: Option<&'a str>,
    reason: Option<&'a str>,
    seq: Option<u64>,
    /// `true` when a later event exists in this record's identity lineage
    /// (P534 review blocker 2) — the append-only-history-aware currency
    /// `handle_trust_status` and the TRUST screen already select by; `trust
    /// list` renders it so history reads distinctly from a single current
    /// decision.
    superseded: bool,
}

/// `ctx traits trust --list [--stale]`: every named trust decision, joined
/// against current trait resolution via
/// [`ctx_traits_io::trust::classify_records`].
pub(crate) fn handle_trust_list(stale: bool, json: bool) -> crate::Result<CommandOutput<()>> {
    let document = ctx_traits_io::trust::read_store()?;
    let current = current_trait_digests()?;
    let mut rows = ctx_traits_io::trust::classify_records(&document, &current);
    if stale {
        rows.retain(ctx_traits_io::trust::TrustReportRow::is_stale_approval);
    }
    rows.sort_by(|a, b| {
        a.trait_id
            .as_deref()
            .unwrap_or("")
            .cmp(b.trait_id.as_deref().unwrap_or(""))
            .then_with(|| a.digest.cmp(&b.digest))
    });

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            let json_rows: Vec<TrustListRowJson> = rows
                .iter()
                .map(|row| TrustListRowJson {
                    trait_id: row.trait_id.as_deref(),
                    digest: &row.digest,
                    current_digest: row.current_digest.as_deref(),
                    state: row.state.as_str(),
                    freshness: row.freshness,
                    updated_at: row.updated_at.as_deref(),
                    reason: row.reason.as_deref(),
                    seq: row.seq,
                    superseded: row.superseded,
                })
                .collect();
            print_json_report(&json_rows, "trust --list")?;
        }
        OutputMode::Human(mode) => {
            let headline = if stale {
                "trust --list --stale"
            } else {
                "trust --list"
            };
            let mut panel = Panel::new("ctx", headline, PanelStatus::Passed("passed".to_string()));
            let mut entry_rows = Vec::new();
            for row in &rows {
                // `Stale` shares P473's `trust_story` label so a moved
                // VERIFIED record ("moved (approval stale)") and a moved
                // BLOCKED record ("moved (block stale)") read distinctly
                // here too, rather than collapsing both to the bare word
                // "stale" — the same voice as the TRUST screen and `trust
                // status` above.
                let freshness = match row.freshness {
                    ctx_traits_io::trust::TrustFreshness::Current => "current",
                    ctx_traits_io::trust::TrustFreshness::Stale => {
                        super::trust_story::classify_trust(Some(row)).label()
                    }
                    ctx_traits_io::trust::TrustFreshness::Orphaned => "orphaned",
                };
                let mut value = format!(
                    "state={} digest={} freshness={}",
                    row.state.as_str(),
                    row.digest,
                    freshness
                );
                if let Some(current_digest) = &row.current_digest
                    && current_digest != &row.digest
                {
                    value.push_str(&format!(" current-digest={current_digest}"));
                }
                if let Some(reason) = row.reason.as_deref() {
                    value.push_str(&format!(" reason={reason}"));
                }
                if row.superseded {
                    value.push_str(" superseded=true");
                }
                let tone = match row.freshness {
                    ctx_traits_io::trust::TrustFreshness::Current if row.superseded => {
                        RowTone::Warn
                    }
                    ctx_traits_io::trust::TrustFreshness::Current => RowTone::Pass,
                    ctx_traits_io::trust::TrustFreshness::Stale
                    | ctx_traits_io::trust::TrustFreshness::Orphaned => RowTone::Fail,
                };
                entry_rows.push(PanelRow::toned(
                    row.trait_id.as_deref().unwrap_or("(digest-only)"),
                    value,
                    tone,
                ));
            }
            if entry_rows.is_empty() {
                panel = panel.row(PanelRow::toned("entries", "(none)", RowTone::Default));
            } else {
                panel = panel.section(PanelSection::new("entries", entry_rows));
            }
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Decode a built-in package's embedded `generated/index.toml` to report its
/// trait version. Reads only compiled-in bytes — never touches disk, so
/// `ctx traits list` never requires the runtime store to be materialized.
struct BuiltinLifecycle {
    version: String,
    schema_version: String,
    status: String,
    trust: String,
}

/// Resolve version, package status, and machine trust verdict for one
/// embedded built-in trait package, using the same lifecycle helper
/// (`ctx_traits_io::lifecycle::resolve_builtin`) and canonical-digest
/// computation as local packages, so built-in and local list rows never
/// drift onto separate resolution logic.
fn builtin_lifecycle(
    package: &ctx_traits_core::builtin_trait_packages::BuiltinTraitPackage,
) -> Option<BuiltinLifecycle> {
    let index_file = package.file("generated/index.toml")?;
    let index_text = std::str::from_utf8(index_file.bytes).ok()?;
    let (trait_ref, decode_warnings) = ctx_traits_core::encoding::decode_trait_with_warnings(
        ctx_traits_core::encoding::Encoding::Toml,
        index_text,
    )
    .ok()?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(
        &format!("built-in {}", package.id),
        &decode_warnings,
    );
    let canonical_digest = ctx_traits_core::digest::canonical_digest(&trait_ref).ok()?;
    let manifest_text = package
        .file("trait.toml")
        .and_then(|file| std::str::from_utf8(file.bytes).ok());
    let (status, trust) =
        ctx_traits_io::lifecycle::resolve_builtin(manifest_text, canonical_digest.as_str()).ok()?;
    Some(BuiltinLifecycle {
        version: trait_ref.version.as_str().to_string(),
        schema_version: trait_ref.schema_version.as_str().to_string(),
        status: status.display_name().to_string(),
        trust: trust.display_name().to_string(),
    })
}

#[derive(serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
enum ListRow {
    Trait {
        id: String,
        version: String,
        schema_version: String,
        status: String,
        trust: String,
        path: String,
        origin: Option<String>,
        family: Option<String>,
        variant: Option<String>,
        /// Origin label of a further-tier candidate this row's winner
        /// shadowed (P439), e.g. `"npm (global):pkg@1.0.0"`. `None` when no
        /// other tier also offers this id.
        shadow: Option<String>,
    },
    Unreadable {
        id: String,
        path: String,
        error: String,
    },
    SourceOnly {
        id: String,
        source: String,
    },
}

impl ListRow {
    /// Key used for ungrouped ordering: the trait/package ID.
    fn sort_key(&self) -> &str {
        match self {
            ListRow::Trait { id, .. }
            | ListRow::Unreadable { id, .. }
            | ListRow::SourceOnly { id, .. } => id,
        }
    }
}

/// One entry in the deterministic, ordered `ListReport` sequence: either a
/// standalone row or a family group with its members pre-sorted (declared
/// `variant=default` first, then by trait ID). Preparing this once keeps the
/// styled and plain renderers from being able to drift.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum ListEntry {
    Row(ListRow),
    Family {
        family: String,
        /// The conservative cross-variant summary
        /// ([`super::trust_story::aggregate_trust_label`]) — never plain
        /// `verified` unless every member's trust is `verified` (0150).
        trust: String,
        members: Vec<ListRow>,
    },
}

impl ListEntry {
    fn sort_key(&self) -> &str {
        match self {
            ListEntry::Row(row) => row.sort_key(),
            ListEntry::Family { family, .. } => family,
        }
    }
}

struct ListReport {
    repo_root_hint: String,
    builtins: Vec<(String, BuiltinLifecycle)>,
    entries: Vec<ListEntry>,
    is_empty: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListBuiltinJson<'a> {
    id: &'a str,
    version: &'a str,
    schema_version: &'a str,
    status: &'a str,
    trust: &'a str,
    source: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListReportJson<'a> {
    repo_root_hint: &'a str,
    builtins: Vec<ListBuiltinJson<'a>>,
    entries: &'a [ListEntry],
}

impl ListReport {
    fn to_json(&self) -> ListReportJson<'_> {
        ListReportJson {
            repo_root_hint: &self.repo_root_hint,
            builtins: self
                .builtins
                .iter()
                .map(|(id, lifecycle)| ListBuiltinJson {
                    id,
                    version: &lifecycle.version,
                    schema_version: &lifecycle.schema_version,
                    status: &lifecycle.status,
                    trust: &lifecycle.trust,
                    source: "binary",
                })
                .collect(),
            entries: &self.entries,
        }
    }
}

/// One resolved local trait package: the exact per-package resolution both
/// `ctx traits list` and the P423 TRAITS/TRUST dashboard screens need
/// (name/version/schema-version/status/trust/canonical-digest/source-path
/// plus family/variant for `list`'s grouping) — computed exactly once by
/// [`resolve_trait_inventory`] so the two presentations can never drift onto
/// separate discovery/load/lifecycle loops.
pub(crate) struct ResolvedTraitEntry {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) schema_version: String,
    pub(crate) status: String,
    pub(crate) trust: String,
    pub(crate) canonical_digest: String,
    pub(crate) source_path: String,
    /// The trait's package root, for approval guard evaluation
    /// ([`ctx_traits_io::trust::evaluate_approval_guard`]) — never re-derived
    /// from `source_path`, which is the manifest file, not its containing
    /// package.
    pub(crate) trait_root: String,
    /// npm origin label (`npm:<package>@<version>`) for project-vendored
    /// rows, `None` for authored packages. Kept separate from
    /// `source_path`, which always stays the real, resolvable filesystem
    /// path so drift checks and editor-path resolution never have to parse
    /// display text back out of it.
    pub(crate) origin: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) variant: Option<String>,
    /// Origin label of a further-tier candidate this entry's winner
    /// shadowed (P439). `None` when no other tier also offers this id.
    pub(crate) shadow: Option<String>,
}

/// One row of [`resolve_trait_inventory`]'s result: either a fully resolved
/// package, or the reason it could not be read — kept as a typed alternative
/// (not a `Result`) so one corrupt package never aborts the whole scan.
pub(crate) enum TraitInventoryRow {
    Resolved(Box<ResolvedTraitEntry>),
    Unreadable {
        id: String,
        path: String,
        error: String,
    },
}

/// The single discovery/load/lifecycle-resolution loop over every tier
/// visible from `context` — repo-authored, repo-vendored, user-global,
/// built-in (P439). `handle_list`, the TRAITS screen, and the TRUST screen
/// all project from this one typed result, resolved through
/// [`ctx_traits_io::inventory::InventoryContext`], instead of each
/// re-walking discovery/lock/lifecycle-resolution themselves — the same
/// shared scan explicit-id run resolution and query selection use, so `list`
/// can never disagree with them about which tier wins or what it shadowed.
pub(crate) fn resolve_trait_inventory(
    context: &ctx_traits_io::inventory::InventoryContext,
) -> crate::Result<Vec<TraitInventoryRow>> {
    let ids = context.candidate_ids()?;
    let mut rows = Vec::new();
    for id in ids {
        let Some(resolution) = context.resolve_tiers(&id)? else {
            continue;
        };
        let path = resolution.winner.path.as_str().to_string();
        let origin = match resolution.winner.tier {
            ctx_traits_io::inventory::Tier::RepoAuthored => None,
            _ => Some(resolution.winner.origin.clone()),
        };
        let shadow = resolution
            .shadowed
            .first()
            .map(|candidate| candidate.origin.clone());

        // A native family shares one candidate id across every variant, each
        // with its own canonical digest and trust verdict. Reporting only the
        // bare id's winner (the default variant) let one variant's verdict
        // stand in for the whole family — the 0150 collapse. Reading the
        // family table from the winner's package root, when present, expands
        // the id into one row per declared variant instead of one row total;
        // every reporting surface (`list`, TRAITS, TRUST) derives from this
        // one scan, so the fix lands everywhere at once.
        let family_table =
            ctx_traits_io::layout::package_root_for_manifest(camino::Utf8Path::new(&path))
                .and_then(|root| {
                    ctx_traits_io::family_manifest::read_family_table(
                        &ctx_traits_io::layout::package_manifest_path(root),
                    )
                    .ok()
                    .flatten()
                    .map(|table| (root.to_path_buf(), table))
                });

        if let Some((root, table)) = family_table {
            for variant in table.variants.values() {
                let variant_path = root.join(&variant.relative_path);
                push_resolved_trait_row(
                    &mut rows,
                    &id,
                    variant_path.as_str(),
                    origin.clone(),
                    shadow.clone(),
                    // The `[family]` package table is the ground truth this
                    // expansion is reading from — a native build stamps no
                    // `metadata.family` slug on the canonical document
                    // itself, so grouping must key off the family id this
                    // scan already resolved from, not off a field the
                    // variant's own bytes never carry.
                    Some(id.clone()),
                )?;
            }
            continue;
        }

        push_resolved_trait_row(&mut rows, &id, &path, origin, shadow, None)?;
    }
    Ok(rows)
}

/// Load one trait file and push its resolved (or unreadable) inventory row.
/// The one place [`resolve_trait_inventory`] turns a candidate manifest path
/// into a [`TraitInventoryRow`], shared by both the single-id path and the
/// per-variant family expansion so the two can never diverge on how a row is
/// built. `family_override` is `Some(family_id)` for a variant reached via
/// the `[family]` package-table expansion; `None` falls back to the
/// document's own `metadata.family` slug, for a family-less trait or one
/// declared that way outside the native-family mechanism.
fn push_resolved_trait_row(
    rows: &mut Vec<TraitInventoryRow>,
    id: &str,
    path: &str,
    origin: Option<String>,
    shadow: Option<String>,
    family_override: Option<String>,
) -> crate::Result<()> {
    match ctx_traits_io::run::load_trait(path) {
        Ok((trait_ref, trait_root, _, canonical_digest)) => {
            let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
                &trait_root,
                trait_ref.id.as_str(),
                canonical_digest.as_str(),
            )?;
            let family = family_override.or_else(|| {
                trait_ref
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.family.as_ref())
                    .map(|slug| slug.as_str().to_string())
            });
            let variant = trait_ref.variant.clone();
            rows.push(TraitInventoryRow::Resolved(Box::new(ResolvedTraitEntry {
                id: trait_ref.id.as_str().to_string(),
                version: trait_ref.version.as_str().to_string(),
                schema_version: trait_ref.schema_version.as_str().to_string(),
                status: status.display_name().to_string(),
                trust: trust.display_name().to_string(),
                canonical_digest: canonical_digest.as_str().to_string(),
                source_path: path.to_string(),
                trait_root: trait_root.to_string(),
                origin,
                family,
                variant,
                shadow,
            })));
        }
        Err(error) => rows.push(TraitInventoryRow::Unreadable {
            id: id.to_string(),
            path: path.to_string(),
            error: error.to_string(),
        }),
    }
    Ok(())
}

/// One TRAITS-screen row: [`ResolvedTraitEntry`], or the read error for an
/// unreadable package — never synthesizing digest/status/trust values for the
/// latter. Drift is deliberately not list inventory: the dashboard computes it
/// only for the selected preview.
pub(crate) struct DashboardTraitRow {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) status: String,
    pub(crate) trust: String,
    pub(crate) canonical_digest: String,
    pub(crate) source_path: String,
    pub(crate) error: Option<String>,
    /// `Some("built-in")`/`Some("npm:...")` for a vendored/global/built-in
    /// tier winner, `None` for a repo-authored package (mirrors
    /// [`ResolvedTraitEntry::origin`]) — TRUST (P473) needs every tier;
    /// TRAITS still excludes `built-in` by filtering on this field.
    pub(crate) origin: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) variant: Option<String>,
}

/// Build the dashboard's full trait inventory, projected from the single
/// [`resolve_trait_inventory`] scan — every tier `InventoryContext` sees
/// (repo-authored, vendored, user-global, built-in), matching `trust
/// list`/`doctor` (P473 §1 note 2). TRAITS filters this down to
/// `origin != Some("built-in")` itself (byte-identical to pre-P473 rows);
/// TRUST uses the full set. The list does not compute drift.
pub(crate) fn dashboard_trait_inventory() -> crate::Result<Vec<DashboardTraitRow>> {
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    let mut rows: Vec<DashboardTraitRow> = resolve_trait_inventory(&context)?
        .into_iter()
        .map(|row| match row {
            TraitInventoryRow::Resolved(entry) => DashboardTraitRow {
                id: entry.id,
                version: entry.version,
                status: entry.status,
                trust: entry.trust,
                canonical_digest: entry.canonical_digest,
                source_path: entry.source_path,
                error: None,
                origin: entry.origin,
                family: entry.family,
                variant: entry.variant,
            },
            TraitInventoryRow::Unreadable { id, path, error } => DashboardTraitRow {
                id,
                version: String::new(),
                status: String::new(),
                trust: String::new(),
                canonical_digest: String::new(),
                source_path: path,
                error: Some(error),
                origin: None,
                family: None,
                variant: None,
            },
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(rows)
}

/// One-line drift summary for a TRAITS row, reusing the existing
/// `ctx traits check --locked` drift computation
/// ([`crate::app::report_check::build_check_report`]) rather than
/// re-implementing digest comparison. Best-effort: any error (no lockfile,
/// unreadable trait) reports as `unverified` rather than failing the whole
/// screen load.
pub(crate) fn dashboard_trait_drift(source_path: &str) -> String {
    let report =
        crate::app::report_check::build_check_report(&crate::app::report_check::CheckInputs {
            file: source_path,
            locked: true,
            skip_cdk_drift: true,
            json: false,
            plain: true,
            no_animate: true,
            verbose: false,
            run_ledger: None,
            eval_reports: &[],
        });
    match report {
        Ok(report) if report.drift.is_empty() => "clean".to_string(),
        Ok(report) => {
            let drifted = report.drift.iter().filter(|d| !d.unsupported).count();
            if drifted > 0 {
                format!("{drifted} layer(s) drifted")
            } else {
                // Every reported entry is `unsupported` (comparison not
                // implemented for that layer) rather than a confirmed
                // difference — distinct from a clean, fully-verified match.
                "unverified".to_string()
            }
        }
        Err(_) => "unverified".to_string(),
    }
}

/// Resolve the authored source document to open for `e` on a TRAITS row: an
/// `index.ts`/`index.mjs` sibling when present (authored TypeScript source),
/// falling back to the canonical trait file itself. `None` when neither
/// exists (a built-in/binary package without a discoverable authored source),
/// which the dashboard reports as an unsupported edit rather than opening
/// nothing.
pub(crate) fn dashboard_trait_editable_source(trait_path: &str) -> Option<camino::Utf8PathBuf> {
    let path = camino::Utf8Path::new(trait_path);
    // Generated manifests live in `<package>/generated/`; resolve from the
    // package root so the preview and editor find the authored entrypoint.
    let dir = ctx_traits_io::layout::package_root_for_manifest(path)?;
    for candidate in [
        "source/index.ts",
        "source/index.mjs",
        "index.ts",
        "index.mjs",
    ] {
        let candidate_path = dir.join(candidate);
        if candidate_path.is_file() {
            return Some(candidate_path);
        }
    }
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    None
}

pub(crate) fn handle_list(json: bool, verbose: bool) -> crate::Result<CommandOutput<()>> {
    let cwd = current_utf8_dir()?;
    let context = ctx_traits_io::inventory::InventoryContext::discover()?;
    // Built-in packages are reported in the separate `builtins` section
    // below (always the full compiled-in set, regardless of whether one is
    // currently shadowed by a project/global tier), so they are excluded
    // from `entries` here to avoid a duplicate row.
    let inventory: Vec<TraitInventoryRow> = resolve_trait_inventory(&context)?
        .into_iter()
        .filter(|row| {
            !matches!(row, TraitInventoryRow::Resolved(entry) if entry.origin.as_deref() == Some("built-in"))
        })
        .collect();
    let authoring_packages = ctx_traits_io::discovery::trait_authoring_packages(&cwd)?;

    // Runnable only: a shared package like `spec` ships in the binary so a
    // dependent's `../spec` resolves, but listing it would offer a trait that
    // declares no procedure and cannot run.
    let mut builtin_packages: Vec<_> =
        ctx_traits_core::builtin_trait_packages::runnable_packages().collect();
    builtin_packages.sort_by_key(|package| package.id);
    let builtins = builtin_packages
        .into_iter()
        .map(|package| {
            let lifecycle = builtin_lifecycle(package).unwrap_or_else(|| BuiltinLifecycle {
                version: "unknown".to_string(),
                schema_version: "unknown".to_string(),
                status: "unknown".to_string(),
                trust: "unknown".to_string(),
            });
            (package.id.to_string(), lifecycle)
        })
        .collect();

    let repo_root_hint = format!(
        "{} or {}",
        ctx_traits_io::layout::trait_protocol_root_path(&cwd),
        ctx_traits_io::layout::trait_authoring_root_path(&cwd)
    );

    let mut rows = Vec::new();
    let protocol_ids: std::collections::BTreeSet<String> = inventory
        .iter()
        .map(|row| match row {
            TraitInventoryRow::Resolved(entry) => entry.id.clone(),
            TraitInventoryRow::Unreadable { id, .. } => id.clone(),
        })
        .collect();
    for row in inventory {
        match row {
            TraitInventoryRow::Resolved(entry) => rows.push(ListRow::Trait {
                id: entry.id,
                version: entry.version,
                schema_version: entry.schema_version,
                status: entry.status,
                trust: entry.trust,
                path: entry.source_path,
                origin: entry.origin,
                family: entry.family,
                variant: entry.variant,
                shadow: entry.shadow,
            }),
            TraitInventoryRow::Unreadable { id, path, error } => {
                rows.push(ListRow::Unreadable { id, path, error })
            }
        }
    }
    for package in authoring_packages {
        if !protocol_ids.contains(&package.trait_id) {
            rows.push(ListRow::SourceOnly {
                id: package.trait_id,
                source: package.source_path.to_string(),
            });
        }
    }

    let is_empty = rows.is_empty();
    let entries = group_list_rows(rows);

    let report = ListReport {
        repo_root_hint,
        builtins,
        entries,
        is_empty,
    };

    match OutputMode::select(json, verbose) {
        OutputMode::Json => {
            print_json_report(&report.to_json(), "list output")?;
        }
        OutputMode::Human(mode) => {
            let panel = compact_list_panel(&report);
            emit_human(false, &panel, mode, || {
                crate::app::tui::emit_report(
                    false,
                    || styled_list_lines(&report),
                    || emit_plain_list(&report),
                )
            })?;
        }
    }
    Ok(CommandOutput::new(()))
}

/// Tallies every [`ListRow`] across [`ListEntry::Row`]s and
/// [`ListEntry::Family`] members alike, by kind — the counts the compact
/// panel leads with, computed in one pass over the same `entries` the
/// `--verbose` detail body renders so the two can never disagree.
fn list_row_counts(entries: &[ListEntry]) -> (usize, usize, usize) {
    let (mut resolved, mut unreadable, mut source_only) = (0usize, 0usize, 0usize);
    let mut tally = |row: &ListRow| match row {
        ListRow::Trait { .. } => resolved += 1,
        ListRow::Unreadable { .. } => unreadable += 1,
        ListRow::SourceOnly { .. } => source_only += 1,
    };
    for entry in entries {
        match entry {
            ListEntry::Row(row) => tally(row),
            ListEntry::Family { members, .. } => members.iter().for_each(&mut tally),
        }
    }
    (resolved, unreadable, source_only)
}

fn compact_list_panel(report: &ListReport) -> Panel {
    let (resolved, unreadable, source_only) = list_row_counts(&report.entries);
    let total = resolved + unreadable + source_only;
    let status = if unreadable > 0 {
        PanelStatus::Blocked("blocked".to_string())
    } else {
        PanelStatus::Passed("passed".to_string())
    };
    let mut panel = Panel::new("ctx", "list", status)
        .row(PanelRow::toned(
            "traits",
            format!(
                "{total} · resolved: {resolved} · unreadable: {unreadable} · source-only: {source_only}"
            ),
            if unreadable > 0 {
                RowTone::Fail
            } else {
                RowTone::Pass
            },
        ))
        .row(PanelRow::toned(
            "builtins",
            report.builtins.len().to_string(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "root",
            report.repo_root_hint.as_str(),
            RowTone::Default,
        ));
    if unreadable > 0 {
        let mut rows = Vec::new();
        for entry in &report.entries {
            let members: &[ListRow] = match entry {
                ListEntry::Row(row) => std::slice::from_ref(row),
                ListEntry::Family { members, .. } => members,
            };
            for row in members {
                if let ListRow::Unreadable { id, path, error } = row {
                    rows.push(PanelRow::toned(
                        id,
                        format!("{path} — {error}"),
                        RowTone::Fail,
                    ));
                }
            }
        }
        panel = panel.section(PanelSection::new("unreadable", rows));
    }
    panel.next(PanelRow::toned(
        "next",
        "run `ctx traits list --verbose` for the full per-package narrative",
        RowTone::Default,
    ))
}

/// Partition readable trait rows with declared `metadata.family` into
/// alphabetically-keyed family groups, sorted with a declared
/// `variant=default` row first then by trait ID; everything else
/// (family-less rows, unreadable rows, source-only rows) stays ungrouped.
/// Groups and ungrouped rows are merged into one deterministic alphabetical
/// sequence (family name vs. trait ID) so styled and plain output cannot
/// drift from each other.
fn group_list_rows(rows: Vec<ListRow>) -> Vec<ListEntry> {
    let mut families: std::collections::BTreeMap<String, Vec<ListRow>> =
        std::collections::BTreeMap::new();
    let mut ungrouped = Vec::new();

    for row in rows {
        match &row {
            ListRow::Trait {
                family: Some(family),
                ..
            } => {
                families.entry(family.clone()).or_default().push(row);
            }
            _ => ungrouped.push(row),
        }
    }

    fn is_default_variant(row: &ListRow) -> bool {
        matches!(
            row,
            ListRow::Trait {
                variant: Some(variant),
                ..
            } if variant == "default"
        )
    }

    let mut entries: Vec<ListEntry> = ungrouped.into_iter().map(ListEntry::Row).collect();
    for (family, mut members) in families {
        members.sort_by(|a, b| {
            is_default_variant(b)
                .cmp(&is_default_variant(a))
                .then_with(|| a.sort_key().cmp(b.sort_key()))
        });
        let member_trust: Vec<&str> = members
            .iter()
            .filter_map(|member| match member {
                ListRow::Trait { trust, .. } => Some(trust.as_str()),
                ListRow::Unreadable { .. } | ListRow::SourceOnly { .. } => None,
            })
            .collect();
        let trust = super::trust_story::aggregate_trust_label(&member_trust);
        entries.push(ListEntry::Family {
            family,
            trust,
            members,
        });
    }

    entries.sort_by(|a, b| a.sort_key().cmp(b.sort_key()));
    entries
}

fn emit_plain_list(report: &ListReport) -> crate::Result<()> {
    use crate::app::tui::write_plain_line as w;
    w("ctx traits list")?;

    w("  built-in:")?;
    for (id, lifecycle) in &report.builtins {
        w(format!(
            "    {id} version={} schema-version={} status={} trust={} source=binary",
            lifecycle.version, lifecycle.schema_version, lifecycle.status, lifecycle.trust
        ))?;
    }

    if report.is_empty {
        w(format!(
            "  no repo-local trait packages found under {}",
            report.repo_root_hint
        ))?;
        return Ok(());
    }

    for entry in &report.entries {
        match entry {
            ListEntry::Row(row) => w(plain_row_line(row, "  ", false))?,
            ListEntry::Family {
                family,
                trust,
                members,
            } => {
                w(format!("  family: {family} trust={trust}"))?;
                for member in members {
                    w(plain_row_line(member, "    ", true))?;
                }
            }
        }
    }
    Ok(())
}

/// Render one row's plain-text line. `indent` and `show_variant` are the
/// only knobs between the ungrouped path (byte-identical to pre-grouping
/// output) and family-member rows (extra indent, `variant=` after `trust=`).
fn plain_row_line(row: &ListRow, indent: &str, show_variant: bool) -> String {
    match row {
        ListRow::Trait {
            id,
            version,
            schema_version,
            status,
            trust,
            path,
            origin,
            variant,
            shadow,
            ..
        } => {
            let variant_suffix = if show_variant {
                match variant {
                    Some(variant) => format!(" variant={variant}"),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            let origin_suffix = match origin {
                Some(origin) => format!(" origin={origin}"),
                None => String::new(),
            };
            let shadow_suffix = match shadow {
                Some(shadow) => format!(" shadows={shadow}"),
                None => String::new(),
            };
            format!(
                "{indent}{id} version={version} schema-version={schema_version} status={status} trust={trust}{variant_suffix} path={path}{origin_suffix}{shadow_suffix}",
            )
        }
        ListRow::Unreadable { id, path, error } => {
            format!("{indent}{id} status=unreadable path={path} error={error}")
        }
        ListRow::SourceOnly { id, source } => {
            format!("{indent}{id} status=source-only source={source}")
        }
    }
}

fn styled_list_lines(report: &ListReport) -> Vec<crate::app::tui::Line> {
    use crate::app::tui::{Line, Tone};

    let mut lines = Vec::new();
    lines.push(crate::app::tui::command_line("ctx traits list"));
    lines.push(Line::blank());

    let mut builtin_header = Line::blank();
    builtin_header.push("built-in:", Tone::Muted);
    lines.push(builtin_header);
    for (id, lifecycle) in &report.builtins {
        let mut line = Line::blank();
        line.push("  ", Tone::Default);
        line.push(id.clone(), Tone::Default);
        line.push(" version=", Tone::Muted);
        line.push(lifecycle.version.clone(), Tone::Default);
        line.push(" schema-version=", Tone::Muted);
        line.push(lifecycle.schema_version.clone(), Tone::Default);
        line.push(" status=", Tone::Muted);
        line.push(lifecycle.status.clone(), Tone::Default);
        line.push(" trust=", Tone::Muted);
        line.push(lifecycle.trust.clone(), Tone::Default);
        line.push(" source=binary", Tone::Muted);
        lines.push(line);
    }

    if report.is_empty {
        lines.push(Line::blank());
        let mut line = Line::blank();
        line.push(
            format!(
                "no repo-local trait packages found under {}",
                report.repo_root_hint
            ),
            Tone::Muted,
        );
        lines.push(line);
        return lines;
    }

    lines.push(Line::blank());
    for entry in &report.entries {
        match entry {
            ListEntry::Row(row) => lines.push(styled_list_row_line(row, "", false)),
            ListEntry::Family {
                family,
                trust,
                members,
            } => {
                let mut header = Line::blank();
                header.push("family: ", Tone::Muted);
                header.push(family.clone(), Tone::Default);
                header.push(" trust=", Tone::Muted);
                header.push(trust.clone(), Tone::Default);
                lines.push(header);
                for member in members {
                    lines.push(styled_list_row_line(member, "  ", true));
                }
            }
        }
    }
    lines
}

/// Render one row's styled line. `indent` and `show_variant` are the only
/// knobs between the ungrouped path (identical to pre-grouping output) and
/// family-member rows (extra indent, `variant=` after `trust=`).
fn styled_list_row_line(row: &ListRow, indent: &str, show_variant: bool) -> crate::app::tui::Line {
    use crate::app::tui::{Line, Tone};

    let mut line = Line::blank();
    if !indent.is_empty() {
        line.push(indent, Tone::Default);
    }
    match row {
        ListRow::Trait {
            id,
            version,
            schema_version,
            status,
            trust,
            path,
            origin,
            variant,
            shadow,
            ..
        } => {
            line.push(id.clone(), Tone::Default);
            line.push(" version=", Tone::Muted);
            line.push(version.clone(), Tone::Default);
            line.push(" schema-version=", Tone::Muted);
            line.push(schema_version.clone(), Tone::Default);
            line.push(" status=", Tone::Muted);
            line.push(status.clone(), Tone::Default);
            line.push(" trust=", Tone::Muted);
            line.push(trust.clone(), Tone::Default);
            if show_variant && let Some(variant) = variant {
                line.push(" variant=", Tone::Muted);
                line.push(variant.clone(), Tone::Default);
            }
            line.push(" path=", Tone::Muted);
            line.push(path.clone(), Tone::Default);
            if let Some(origin) = origin {
                line.push(" origin=", Tone::Muted);
                line.push(origin.clone(), Tone::Default);
            }
            if let Some(shadow) = shadow {
                line.push(" shadows=", Tone::Muted);
                line.push(shadow.clone(), Tone::Default);
            }
        }
        ListRow::Unreadable { id, path, error } => {
            line.push(id.clone(), Tone::Fail);
            line.push(" status=", Tone::Muted);
            line.push("unreadable", Tone::Fail);
            line.push(" path=", Tone::Muted);
            line.push(path.clone(), Tone::Default);
            line.push(" error=", Tone::Muted);
            line.push(error.clone(), Tone::Default);
        }
        ListRow::SourceOnly { id, source } => {
            line.push(id.clone(), Tone::Warn);
            line.push(" status=", Tone::Muted);
            line.push("source-only", Tone::Warn);
            line.push(" source=", Tone::Muted);
            line.push(source.clone(), Tone::Default);
        }
    }
    line
}
