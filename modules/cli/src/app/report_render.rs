//! Render and export report commands.
//!
//! P497: every consumer of [`build_render_context`] — `prompt`, `export`
//! (and its hidden `render` alias), and host placement (`install`/`update`,
//! via [`build_export_artifact`]) — is gated on the same lifecycle/trust
//! classification the run family uses, applied here immediately after
//! [`ctx_traits_io::run::load_trait`] and before any resource resolution or
//! body scanning. The future MCP serving path inherits this gate by
//! construction, since it must also go through `build_render_context`.

use crate::app::entry::{build_file_evidence_from_io, print_json_report};
use crate::app::{report_check, report_resources};
use ctx_traits_core::response::CommandOutput;
use ctx_traits_core::r#trait::activation::Gate;

/// Per-verb posture for the P497 render-trust gate: which gates are hard
/// refusals, which are escapable by a flag, and whether refusals are
/// reported as JSON. `verb` is the only per-command string threaded through
/// refusal text, so the message template itself never forks per caller.
pub(crate) struct RenderTrustPosture {
    verb: &'static str,
    allow_unreviewed: bool,
    draft: DraftPosture,
    json: bool,
}

enum DraftPosture {
    /// `prompt`/`export`: a draft trait always renders, with an advisory.
    Advisory,
    /// `host install`/`host update`: a draft trait refuses unless escaped.
    Escapable(bool),
}

impl RenderTrustPosture {
    pub(crate) fn prompt(allow_unreviewed: bool, json: bool) -> Self {
        Self {
            verb: "prompt",
            allow_unreviewed,
            draft: DraftPosture::Advisory,
            json,
        }
    }

    pub(crate) fn export(allow_unreviewed: bool, json: bool) -> Self {
        Self {
            verb: "export",
            allow_unreviewed,
            draft: DraftPosture::Advisory,
            json,
        }
    }

    pub(crate) fn host_install(allow_unreviewed: bool, allow_draft: bool, json: bool) -> Self {
        Self {
            verb: "host install",
            allow_unreviewed,
            draft: DraftPosture::Escapable(allow_draft),
            json,
        }
    }

    /// `host update` re-renders and rewrites already-placed host bytes
    /// through the same [`build_export_artifact`] path as `host install`, so
    /// it is gated identically but with no escape flags of its own: a
    /// placement whose source has since gone blocked/unreviewed/draft is the
    /// per-entry caller's problem to report, never this function's to print
    /// (a stray JSON envelope mid-batch would corrupt the batch report), so
    /// `json` is always `false` here regardless of the outer `--json` flag.
    pub(crate) fn host_update() -> Self {
        Self {
            verb: "host update",
            allow_unreviewed: false,
            draft: DraftPosture::Escapable(false),
            json: false,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct RenderTrustRefusalJson<'a> {
    kind: &'a str,
    verb: &'a str,
    trait_id: &'a str,
    gates: &'a [Gate],
    remedy: &'a str,
}

/// Build one refusal error for `posture`/`gates`: a plain `Command` error in
/// human mode, or a typed `{ kind: "refused", ... }` envelope printed via
/// `print_json_report` followed by `AlreadyReported` in `--json` mode.
/// `message` is the human-readable refusal text; `remedy` is the short
/// escape/remediation hint carried in the JSON envelope's top-level field
/// (the per-gate `remedy` fields ride along verbatim from core regardless).
fn render_trust_refusal(
    posture: &RenderTrustPosture,
    trait_id: &str,
    gates: &[Gate],
    message: String,
    remedy: &str,
) -> crate::Error {
    if posture.json {
        let envelope = RenderTrustRefusalJson {
            kind: "refused",
            verb: posture.verb,
            trait_id,
            gates,
            remedy,
        };
        if let Err(error) = print_json_report(&envelope, "render-trust refusal") {
            return error;
        }
        crate::Error::AlreadyReported {
            message,
            exit_code: 1,
        }
    } else {
        crate::Error::Command { message }
    }
}

/// Classify `trait_id`'s lifecycle/trust gates against `posture`: hard
/// refuses on `blocked.trust.blocked` (naming `ctx traits trust list`, never
/// core's `trust approve` remedy — blocked is a decision, not a pending
/// review) and on any escapable gate that `posture` did not escape; returns
/// the gates that passed only because they were escaped or are
/// advisory-only, so the caller can surface a `lifecycle-advisory`.
fn classify_render_trust(
    trait_id: &str,
    status: &ctx_traits_core::manifest::PackageStatus,
    trust: &ctx_traits_core::r#trait::TrustVerdict,
    posture: &RenderTrustPosture,
) -> crate::Result<Vec<Gate>> {
    use ctx_traits_core::r#trait::activation::{
        format_gate_refusal, lifecycle_trust_gates_for_check,
    };
    use ctx_traits_core::r#trait::gate_code;

    let gates = lifecycle_trust_gates_for_check(trait_id, status, trust);
    if gates.is_empty() {
        return Ok(Vec::new());
    }

    let blocked: Vec<Gate> = gates
        .iter()
        .filter(|gate| gate.code == gate_code::TRUST_BLOCKED)
        .cloned()
        .collect();
    if !blocked.is_empty() {
        let message = format!(
            "{} refused: {} — trust decisions are not reviewable via activation; run `ctx traits trust list`",
            posture.verb,
            blocked
                .iter()
                .map(|gate| format!("{} ({})", gate.code, gate.message))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Err(render_trust_refusal(
            posture,
            trait_id,
            &blocked,
            message,
            "ctx traits trust list",
        ));
    }

    let unreviewed: Vec<Gate> = gates
        .iter()
        .filter(|gate| gate.code == gate_code::TRUST_UNREVIEWED)
        .cloned()
        .collect();
    if !unreviewed.is_empty() && !posture.allow_unreviewed {
        let message = format!(
            "{} refused: {}; pass --allow-unreviewed to {} anyway",
            posture.verb,
            format_gate_refusal(&unreviewed),
            posture.verb
        );
        return Err(render_trust_refusal(
            posture,
            trait_id,
            &unreviewed,
            message,
            "--allow-unreviewed",
        ));
    }

    let draft: Vec<Gate> = gates
        .iter()
        .filter(|gate| gate.code == gate_code::STATUS_DRAFT)
        .cloned()
        .collect();
    let mut advisory = unreviewed;
    if !draft.is_empty() {
        match &posture.draft {
            DraftPosture::Advisory => advisory.extend(draft),
            DraftPosture::Escapable(true) => advisory.extend(draft),
            DraftPosture::Escapable(false) => {
                let message = format!(
                    "{} refused: {}; pass --allow-draft to {} anyway",
                    posture.verb,
                    format_gate_refusal(&draft),
                    posture.verb
                );
                return Err(render_trust_refusal(
                    posture,
                    trait_id,
                    &draft,
                    message,
                    "--allow-draft",
                ));
            }
        }
    }

    Ok(advisory)
}

/// Shared load/resolve/plan sequence for render, export, and use: loads the
/// canonical trait, resolves and digests resources, scans resource bodies,
/// folds in dependency body evidence, and compiles the render plan. Reused by
/// every consumer of `plan_render_with_resource_body_evidence` so resource
/// inclusion, dependency evidence, and model-view bytes cannot drift between
/// them.
pub(crate) struct RenderContext {
    pub(crate) trait_ref: ctx_traits_core::Trait,
    pub(crate) trait_root: camino::Utf8PathBuf,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) canonical_digest: ctx_traits_core::digest::Digest,
    pub(crate) plan: ctx_traits_core::render::RenderPlan,
    /// Gates that passed only because they are advisory-only (draft on
    /// `prompt`/`export`) or were explicitly escaped by a flag. Empty when
    /// the trait cleared lifecycle/trust with no gates at all.
    pub(crate) lifecycle_advisory: Vec<Gate>,
    /// Resolved resource roots, kept so a `Format::Skill` export can read
    /// placeable resource bytes for its companion files without re-resolving
    /// roots or diverging from the same evidence the render plan used.
    pub(crate) roots: ctx_traits_io::resource::ResourceRoots,
}

pub(crate) fn build_render_context(
    file: &str,
    render_profile: ctx_traits_core::render::ExtendedRenderProfile,
    posture: &RenderTrustPosture,
) -> crate::Result<RenderContext> {
    let (trait_ref, trait_root, source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(file)?;

    let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
        &trait_root,
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;
    let lifecycle_advisory =
        classify_render_trust(trait_ref.id.as_str(), &status, &trust, posture)?;
    if !lifecycle_advisory.is_empty() {
        eprintln!(
            "warning: {} proceeding despite lifecycle/trust gate(s) for {}: {}",
            posture.verb,
            trait_ref.id.as_str(),
            ctx_traits_core::r#trait::activation::format_gate_refusal(&lifecycle_advisory)
        );
    }

    let roots = ctx_traits_io::resource::resolve_resource_roots(
        trait_root.as_path(),
        &trait_ref.resources,
    )?;
    let manifest = ctx_traits_io::resource::digest_resources(
        &roots,
        trait_ref.id.as_str(),
        &trait_ref.resources,
    )?;
    let file_evidence = build_file_evidence_from_io(&manifest);
    let (mut resource_body_evidence, body_read_warnings) =
        report_resources::scan_resource_bodies(&roots, &trait_ref)?;
    let repo_root =
        ctx_traits_io::export::infer_repo_root_from_trait_file(camino::Utf8Path::new(file));
    let dependency_evidence = report_check::dependency_evidence(
        repo_root,
        trait_root.as_path(),
        &trait_ref,
        None,
        false,
    )?;
    let dependency_resource_decls = dependency_evidence.resource_decls.clone();
    resource_body_evidence.extend(dependency_evidence.body_evidence);
    let mut resource_read_warnings =
        report_resources::resource_read_warning_strings(&manifest.warnings);
    resource_read_warnings.extend(report_resources::resource_read_warning_strings(
        &body_read_warnings,
    ));
    let plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
        &trait_ref,
        render_profile,
        source_digest.as_str(),
        ctx_traits_core::render::ResourceEvidenceInputs {
            file_evidence: &file_evidence,
            body_evidence: &resource_body_evidence,
            dependency_resources: &dependency_resource_decls,
            manifest_digest: Some(manifest.manifest_digest.as_str()),
            read_warnings: resource_read_warnings,
        },
    );

    Ok(RenderContext {
        trait_ref,
        trait_root,
        source_digest,
        canonical_digest,
        plan,
        lifecycle_advisory,
        roots,
    })
}

/// Read the exact bytes of every placeable resource for a `Format::Skill`
/// export, keyed by the same relative path
/// [`ctx_traits_core::render::skill_resource_placement`] computes — the one
/// seam the renderer (which prints the placed path) and this writer share.
/// Only resources [`ctx_traits_core::render::skill_resource_placement`]
/// selects are read; a resource it excludes (binary, missing, inline) is
/// left for the caller to report as export-partial.
pub(crate) fn build_skill_companions(
    roots: &ctx_traits_io::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
    plan: &ctx_traits_core::render::RenderPlan,
) -> crate::Result<Vec<(camino::Utf8PathBuf, Vec<u8>)>> {
    let placements =
        ctx_traits_core::render::skill_resource_placement(trait_ref, &plan.resource_plan);
    let mut companions = Vec::with_capacity(placements.len());
    for (resource_id, relative_path) in placements {
        let Some(resource) = trait_ref.resources.iter().find(|r| r.id == resource_id) else {
            continue;
        };
        let Some(declared_path) = resource.path.as_deref() else {
            continue;
        };
        let presented = ctx_traits_io::resource::presentation_path(roots, resource, declared_path)?;
        if presented.status != ctx_traits_io::resource::PresentationStatus::Available {
            continue;
        }
        let bytes =
            std::fs::read(presented.path.as_std_path()).map_err(|source| crate::Error::Command {
                message: format!(
                    "cannot read resource {resource_id:?} at {:?} for skill directory export: {source}",
                    presented.path
                ),
            })?;
        companions.push((relative_path, bytes));
    }
    Ok(companions)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
enum LockUpdateJson {
    Updated { path: String },
    SkippedMissingLock { path: String },
    SkippedMissingEntry { path: String, trait_id: String },
    Skipped,
}

impl From<&ctx_traits_io::lockfile::LockUpdateResult> for LockUpdateJson {
    fn from(update: &ctx_traits_io::lockfile::LockUpdateResult) -> Self {
        match update {
            ctx_traits_io::lockfile::LockUpdateResult::Updated { path } => {
                LockUpdateJson::Updated { path: path.clone() }
            }
            ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingLock { path } => {
                LockUpdateJson::SkippedMissingLock { path: path.clone() }
            }
            ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingEntry { path, trait_id } => {
                LockUpdateJson::SkippedMissingEntry {
                    path: path.clone(),
                    trait_id: trait_id.clone(),
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
enum GitignoreUpdateJson {
    Added { path: String, entry: String },
    AlreadyPresent { path: String, entry: String },
    Skipped,
}

impl From<&ctx_traits_io::write::GitignoreUpdateResult> for GitignoreUpdateJson {
    fn from(update: &ctx_traits_io::write::GitignoreUpdateResult) -> Self {
        match update {
            ctx_traits_io::write::GitignoreUpdateResult::Added { path, entry } => {
                GitignoreUpdateJson::Added {
                    path: path.clone(),
                    entry: entry.clone(),
                }
            }
            ctx_traits_io::write::GitignoreUpdateResult::AlreadyPresent { path, entry } => {
                GitignoreUpdateJson::AlreadyPresent {
                    path: path.clone(),
                    entry: entry.clone(),
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExportReportJson<'a> {
    trait_id: &'a str,
    profile: &'a str,
    format: &'a str,
    output_root: &'a str,
    path: &'a str,
    digest: &'a str,
    byte_size: u64,
    export_partial: bool,
    unexported_resources: usize,
    resource_manifest_digest: Option<&'a str>,
    lock_target: &'a str,
    lock_path: &'a str,
    lock_update: LockUpdateJson,
    projection_lock_update: LockUpdateJson,
    gitignore_update: GitignoreUpdateJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle_advisory: Option<&'a [Gate]>,
}

/// One rendered, audited export artifact: everything `export` and every
/// host-placement operation need to write a managed file, without either
/// re-running the load/resolve/plan/audit sequence or duplicating it.
/// Reused by [`handle_export`] and by `app::host_install`.
pub(crate) struct ExportArtifact {
    pub(crate) trait_ref: ctx_traits_core::Trait,
    pub(crate) trait_root: camino::Utf8PathBuf,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) canonical_digest: ctx_traits_core::digest::Digest,
    pub(crate) plan: ctx_traits_core::render::RenderPlan,
    pub(crate) content: String,
    pub(crate) resource_manifest_digest: Option<String>,
    pub(crate) lifecycle_advisory: Vec<Gate>,
    /// Companion resource files (relative path, exact bytes) for a
    /// `Format::Skill` export; always empty for every other format.
    pub(crate) companions: Vec<(camino::Utf8PathBuf, Vec<u8>)>,
    /// Declared resources that a `Format::Skill` export could not place
    /// alongside `content` (binary, missing, symlinked, or inline). Narrows
    /// `export-partial`'s meaning from "resources are never placed" to
    /// "these specific resources could not be placed" — `0` for an ordinary
    /// trait whose declared resources are all path-backed text references.
    pub(crate) unplaced_resource_count: usize,
}

/// Build one rendered export artifact: load, resolve, plan, run the
/// blocking model-view/generated-content audits, and render final bytes for
/// `export_format`. Returns `Err` if either audit finds a blocking finding —
/// callers never receive partially-audited content.
pub(crate) fn build_export_artifact(
    file: &str,
    render_profile: ctx_traits_core::render::ExtendedRenderProfile,
    export_format: ctx_traits_io::export::Format,
    posture: &RenderTrustPosture,
) -> crate::Result<ExportArtifact> {
    let RenderContext {
        trait_ref,
        trait_root,
        source_digest,
        canonical_digest,
        plan,
        lifecycle_advisory,
        roots,
    } = build_render_context(file, render_profile, posture)?;

    if plan.model_view.has_blocking_post_audit_findings() {
        return Err(crate::Error::Command {
            message: format!(
                "export refused: model-view post-audit found {} blocking finding(s)",
                plan.model_view.post_audit_findings.len()
            ),
        });
    }

    let content = match export_format {
        ctx_traits_io::export::Format::Compat | ctx_traits_io::export::Format::Agents => {
            ctx_traits_core::render::render_export_content(&plan)
        }
        ctx_traits_io::export::Format::Skill => {
            ctx_traits_core::render::render_skill_export_content(
                &plan,
                &trait_ref,
                &canonical_digest,
            )
        }
        ctx_traits_io::export::Format::Stub => ctx_traits_core::render::render_stub_export_content(
            &plan,
            &trait_ref,
            &canonical_digest,
        ),
    };
    let export_findings = ctx_traits_core::audit::scan_hidden_content(
        &content,
        plan.trait_id.as_str(),
        Some("export.generated-content"),
    );
    let export_blocking_findings = export_findings
        .iter()
        .filter(|finding| !matches!(finding.severity, ctx_traits_core::audit::Severity::Advisory))
        .count();
    if export_blocking_findings > 0 {
        return Err(crate::Error::Command {
            message: format!(
                "export refused: generated content audit found {export_blocking_findings} blocking finding(s)"
            ),
        });
    }

    let companions = if export_format == ctx_traits_io::export::Format::Skill {
        build_skill_companions(&roots, &trait_ref, &plan)?
    } else {
        Vec::new()
    };
    // Only `Format::Skill` ever places resources as companion files (§3.2 —
    // a stub points at `ctx` instead, and `compat`/`agents` were never
    // resource-bearing exports). Reporting an unplaced count for those
    // other formats would print a false `export-partial` for a resource
    // that format never intended to place.
    let unplaced_resource_count = if export_format == ctx_traits_io::export::Format::Skill {
        trait_ref.resources.len().saturating_sub(companions.len())
    } else {
        0
    };

    Ok(ExportArtifact {
        resource_manifest_digest: plan
            .resource_manifest_digest
            .as_ref()
            .map(|d| d.to_string()),
        companions,
        unplaced_resource_count,
        trait_ref,
        trait_root,
        source_digest,
        canonical_digest,
        plan,
        content,
        lifecycle_advisory,
    })
}

pub(crate) struct ExportInputs<'a> {
    pub(crate) file: &'a str,
    pub(crate) profile: &'a str,
    pub(crate) format: &'a str,
    pub(crate) out: Option<&'a str>,
    pub(crate) update_skill_lock: bool,
    pub(crate) update_gitignore: bool,
    pub(crate) allow_unreviewed: bool,
    pub(crate) json: bool,
}

pub(crate) fn handle_export(inputs: ExportInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let ExportInputs {
        file,
        profile,
        format,
        out,
        update_skill_lock,
        update_gitignore,
        allow_unreviewed,
        json,
    } = inputs;
    let path = camino::Utf8Path::new(file);

    let render_profile =
        ctx_traits_core::render::ExtendedRenderProfile::parse(profile).ok_or_else(|| {
            crate::Error::Command {
                message: format!(
                    "unsupported profile: {profile:?} (expected agent-skills, pi, opencode, claude-code, codex, copilot, or markdown-only)"
                ),
            }
        })?;
    let export_format =
        ctx_traits_io::export::Format::parse(format).ok_or_else(|| crate::Error::Command {
            message: format!(
                "unsupported export format: {format:?} (expected compat, skill, agents, or stub)"
            ),
        })?;
    if export_format == ctx_traits_io::export::Format::Skill && update_skill_lock {
        return Err(crate::Error::Command {
            message:
                "export --format skill does not update trait.skill.lock; omit --update-skill-lock"
                    .to_string(),
        });
    }

    let posture = RenderTrustPosture::export(allow_unreviewed, json);
    let ExportArtifact {
        trait_ref,
        trait_root,
        source_digest,
        canonical_digest,
        plan,
        content,
        lifecycle_advisory,
        companions,
        unplaced_resource_count,
        ..
    } = build_export_artifact(file, render_profile, export_format, &posture)?;
    let trait_root = trait_root.as_path();

    let repo_root = ctx_traits_io::export::infer_repo_root_from_trait_file(path);
    // Fail early on unsafe or invalid existing lockfiles so export does not
    // write a new artifact before discovering lock evidence cannot be updated.
    let _lock_preflight = ctx_traits_io::lockfile::read_lockfile(trait_root)?;
    let output_root = match out {
        Some(raw) => {
            let out_path = camino::Utf8Path::new(raw);
            if out_path.is_absolute() {
                out_path.to_owned()
            } else {
                repo_root.join(out_path)
            }
        }
        None => {
            let default_dir =
                render_profile
                    .default_export_dir()
                    .ok_or_else(|| crate::Error::Command {
                        message: format!(
                            "profile {:?} has no default export directory in P51; pass --out to write <out>/<trait-id>/{}",
                            render_profile.as_str(),
                            export_format.filename()
                        ),
                    })?;
            repo_root.join(default_dir)
        }
    };
    // Export under the variant-qualified name, not the bare trait id. Every
    // leaf of a native family shares one id — all five `implement` leaves are
    // `implement` — so a bare-id path made them overwrite each other at
    // `.agents/skills/implement/SKILL.md`, and the lock then held five
    // entries for one file with five different digests. At most one could
    // ever match, so the locked check failed permanently and the family could
    // not be published. `<id>-<variant>` is the same spelling the family
    // already declares as each leaf's alias (`implement-quick`, ...), so this
    // introduces no new naming convention.
    let export_id = trait_ref.export_id();
    let identity = ctx_traits_io::export::Identity::new(
        export_id.clone(),
        source_digest.clone(),
        export_format.ownership(render_profile),
    );
    // Companion resource files sit beside the leaf, inside the trait-id
    // directory (`skill_resource_placement`'s `resources/<id>.<ext>` is
    // relative to `SKILL.md`'s own directory, not the export `--out` root),
    // so each companion's path relative to `output_root` is prefixed with
    // the trait-id directory the leaf's own default relative target uses.
    // Written digest-keyed, not marker-keyed: a fresh `export` has nothing
    // previously recorded to compare against, so ownership is `Fresh` here
    // for every entry — an existing unmanaged file at a companion path
    // refuses rather than being silently overwritten (a reinstall's
    // recorded-digest overwrite check lives in host-install/-update).
    let companion_relative_paths: Vec<camino::Utf8PathBuf> = companions
        .iter()
        .map(|(relative_path, _)| camino::Utf8Path::new(export_id.as_str()).join(relative_path))
        .collect();
    let companion_requests: Vec<ctx_traits_io::export::control::Companion<'_>> =
        companion_relative_paths
            .iter()
            .zip(companions.iter())
            .map(
                |(relative_path, (_, bytes))| ctx_traits_io::export::control::Companion {
                    relative_target: relative_path,
                    bytes,
                    ownership: ctx_traits_io::export::control::CompanionOwnership::Fresh,
                },
            )
            .collect();
    use ctx_traits_io::export::control::Interface as _;
    let result = ctx_traits_io::export::fs::Service
        .write(
            ctx_traits_io::export::control::Request::new(
                &output_root,
                &identity,
                &content,
                export_format,
            )
            .with_companions(&companion_requests),
        )
        .map_err(ctx_traits_io::Error::from)?;
    let export_path = result.path.clone();
    let companion_paths: Vec<camino::Utf8PathBuf> = result
        .companions
        .iter()
        .map(|companion| companion.path.clone())
        .collect();
    let export_target = export_format.lock_target(result.ownership);
    let lock_path = lock_path_for_export(repo_root, &result.path);
    let variant = trait_ref.variant.as_deref();
    let lock_update = ctx_traits_io::lockfile::update_export_evidence(
        trait_root,
        trait_ref.id.as_str(),
        variant,
        ctx_traits_io::lockfile::LockExportEntry {
            target: export_target.clone(),
            path: lock_path.clone(),
            digest: result.content_digest.as_str().to_string(),
        },
    )?;
    let projection_update = if update_skill_lock {
        let projection = ctx_traits_core::lockfile::build_lock_projection(
            ctx_traits_core::lockfile::ProjectionBuild {
                source_digest: source_digest.as_str(),
                canonical_digest: canonical_digest.as_str(),
                plan: &plan,
                generated_markdown: Some(&content),
                output_path: Some(result.path.as_str()),
                command: "ctx traits export --update-skill-lock",
                options: vec![format!("--profile={}", render_profile.as_str())],
            },
        );
        Some(ctx_traits_io::lockfile::update_projection_evidence(
            trait_root,
            trait_ref.id.as_str(),
            variant,
            projection,
        )?)
    } else {
        None
    };
    let gitignore_update = if update_gitignore {
        // A skill directory export's placed resources sit beside the leaf
        // under the trait-id directory; ignore that whole directory rather
        // than only the leaf so a reinstall's companion files stay covered.
        let gitignore_target = if companions.is_empty() {
            export_path.clone()
        } else {
            export_path
                .parent()
                .map(camino::Utf8Path::to_path_buf)
                .unwrap_or_else(|| export_path.clone())
        };
        Some(ctx_traits_io::write::update_gitignore_for_generated_path(
            repo_root,
            &gitignore_target,
        )?)
    } else {
        None
    };

    if unplaced_resource_count > 0 {
        eprintln!(
            "warning: {unplaced_resource_count} declared resource(s) could not be placed \
             alongside {} (binary, missing, symlinked, or inline)",
            export_format.filename()
        );
    }

    if json {
        let report = ExportReportJson {
            trait_id: trait_ref.id.as_str(),
            profile: result.ownership.as_str(),
            format: export_format.as_str(),
            output_root: output_root.as_str(),
            path: result.path.as_str(),
            digest: result.content_digest.as_str(),
            byte_size: result.byte_size,
            export_partial: unplaced_resource_count > 0,
            unexported_resources: unplaced_resource_count,
            resource_manifest_digest: plan.resource_manifest_digest.as_deref(),
            lock_target: &export_target,
            lock_path: &lock_path,
            lock_update: LockUpdateJson::from(&lock_update),
            projection_lock_update: projection_update
                .as_ref()
                .map(LockUpdateJson::from)
                .unwrap_or(LockUpdateJson::Skipped),
            gitignore_update: gitignore_update
                .as_ref()
                .map(GitignoreUpdateJson::from)
                .unwrap_or(GitignoreUpdateJson::Skipped),
            lifecycle_advisory: (!lifecycle_advisory.is_empty()).then_some(&lifecycle_advisory[..]),
        };
        print_json_report(&report, "export output")?;
        return Ok(CommandOutput::new(()));
    }

    use crate::app::presentation::{Panel, PanelRow, PanelStatus, RowTone, emit_human};

    let mut panel = Panel::new("ctx", "export", PanelStatus::Passed("passed".to_string()))
        .row(PanelRow::toned(
            "trait",
            trait_ref.id.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "profile",
            result.ownership.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "format",
            export_format.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "output-root",
            output_root.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "path",
            result.path.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "digest",
            result.content_digest.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "byte-size",
            result.byte_size.to_string(),
            RowTone::Default,
        ));
    if !companion_paths.is_empty() {
        panel = panel.row(PanelRow::toned(
            "companions",
            companion_paths
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            RowTone::Default,
        ));
    }
    if unplaced_resource_count > 0 {
        panel = panel.row(PanelRow::toned(
            "export-partial",
            format!("true ({unplaced_resource_count} resource(s) not placed)"),
            RowTone::Fail,
        ));
    }
    if !lifecycle_advisory.is_empty() {
        panel = panel.row(PanelRow::toned(
            "lifecycle-advisory",
            ctx_traits_core::r#trait::activation::format_gate_refusal(&lifecycle_advisory),
            RowTone::Fail,
        ));
    }
    panel = panel
        .row(PanelRow::toned(
            "resource-manifest-digest",
            plan.resource_manifest_digest.as_deref().unwrap_or("none"),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "lock-evidence",
            format!(
                "target={export_target} path={lock_path} digest={}",
                result.content_digest.as_str()
            ),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "lock-update",
            lock_update_text(&lock_update),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "projection-lock-update",
            projection_update
                .as_ref()
                .map(projection_lock_update_text)
                .unwrap_or_else(|| "skipped (pass --update-skill-lock)".to_string()),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "gitignore-update",
            gitignore_update
                .as_ref()
                .map(gitignore_update_text)
                .unwrap_or_else(|| "skipped (pass --update-gitignore)".to_string()),
            RowTone::Default,
        ));

    fn lock_update_text(update: &ctx_traits_io::lockfile::LockUpdateResult) -> String {
        match update {
            ctx_traits_io::lockfile::LockUpdateResult::Updated { path } => {
                format!("updated {path}")
            }
            ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingLock { path } => {
                format!("skipped (missing lockfile {path})")
            }
            ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingEntry { path, trait_id } => {
                format!("skipped (missing trait {trait_id} in {path})")
            }
        }
    }

    fn projection_lock_update_text(update: &ctx_traits_io::lockfile::LockUpdateResult) -> String {
        match update {
            ctx_traits_io::lockfile::LockUpdateResult::Updated { path } => {
                format!("updated {path}")
            }
            ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingLock { path }
            | ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingEntry { path, .. } => {
                format!("skipped {path}")
            }
        }
    }

    fn gitignore_update_text(update: &ctx_traits_io::write::GitignoreUpdateResult) -> String {
        match update {
            ctx_traits_io::write::GitignoreUpdateResult::Added { path, entry } => {
                format!("added {entry} to {path}")
            }
            ctx_traits_io::write::GitignoreUpdateResult::AlreadyPresent { path, entry } => {
                format!("already present {entry} in {path}")
            }
        }
    }

    emit_human(
        false,
        &panel,
        crate::app::presentation::HumanOutputMode::Compact,
        || Ok(()),
    )?;

    Ok(CommandOutput::new(()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct PromptJsonDigests<'a> {
    source: &'a str,
    canonical: &'a str,
    resource_manifest: Option<&'a str>,
    model_view: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct PromptJsonOutput<'a> {
    text: &'a str,
    digests: PromptJsonDigests<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle_advisory: Option<&'a [Gate]>,
}

pub(crate) fn handle_prompt(
    file: &str,
    allow_unreviewed: bool,
    level: ctx_traits_core::resolve::LoadLevel,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let posture = RenderTrustPosture::prompt(allow_unreviewed, json);
    let RenderContext {
        source_digest,
        canonical_digest,
        plan,
        lifecycle_advisory,
        ..
    } = build_render_context(
        file,
        ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
        &posture,
    )?;

    let (text, model_view_digest) = plan
        .model_view
        .artifact_for_load_level(level)
        .expect("prompt only accepts content-bearing load levels");
    if json {
        let output = PromptJsonOutput {
            text,
            lifecycle_advisory: (!lifecycle_advisory.is_empty()).then_some(&lifecycle_advisory[..]),
            digests: PromptJsonDigests {
                source: source_digest.as_str(),
                canonical: canonical_digest.as_str(),
                resource_manifest: plan.resource_manifest_digest.as_deref(),
                model_view: model_view_digest.as_str(),
            },
        };
        print_json_report(&output, "prompt output")?;
    } else {
        print!("{text}");
    }

    Ok(CommandOutput::new(()))
}

fn lock_path_for_export(repo_root: &camino::Utf8Path, export_path: &camino::Utf8Path) -> String {
    match export_path.strip_prefix(repo_root) {
        Ok(relative) => relative.to_string(),
        Err(_) => export_path.to_string(),
    }
}
