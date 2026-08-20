//! Check report construction and presentation.

use ctx_traits_core::check::Section;
use ctx_traits_core::response::CommandOutput;

use crate::app::entry::{build_file_evidence_from_io, print_synth_provenance};
use crate::app::{report_drift, report_eval, report_import, report_resources, report_runtime};

pub(crate) struct CheckInputs<'a> {
    pub(crate) file: &'a str,
    pub(crate) locked: bool,
    pub(crate) skip_cdk_drift: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
    pub(crate) no_animate: bool,
    pub(crate) verbose: bool,
    pub(crate) run_ledger: Option<&'a str>,
    pub(crate) eval_reports: &'a [String],
}

pub(crate) struct DependencyEvidence {
    section_summary: String,
    section_ok: bool,
    trust_summary: String,
    trust_ok: bool,
    pub(crate) body_evidence: Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    pub(crate) resource_decls: Vec<ctx_traits_core::resource_plan::DependencyResourceDecl>,
    audit_findings: Vec<ctx_traits_core::audit::Finding>,
    warnings: Vec<ctx_traits_core::check::CheckWarning>,
    drift: Vec<ctx_traits_core::check::DriftSummary>,
}

struct SequenceReceipt {
    summary: String,
    ok: bool,
}

#[derive(Clone)]
struct QualifiedRefUse {
    field: String,
    ref_text: String,
    kind: ctx_traits_core::reference::Kind,
    alias: String,
    id: String,
}

pub(crate) fn handle_check(input: CheckInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let report = build_check_report(&input)?;
    emit_check_report(
        &report,
        input.locked,
        input.json,
        input.file,
        input.plain || input.no_animate,
        input.verbose,
    )
}

pub(crate) fn build_check_report(
    input: &CheckInputs<'_>,
) -> crate::Result<ctx_traits_core::check::CheckReport> {
    let path = camino::Utf8Path::new(input.file);
    let text = ctx_traits_io::read::read_text(path)?;

    match ctx_traits_io::run::load_trait(input.file) {
        Ok((trait_ref, trait_root, source_digest, canonical_digest)) => {
            let trait_id = trait_ref.id.as_str().to_string();
            let trait_variant = trait_ref.variant.as_deref();
            let trait_root = trait_root.as_path();
            let (package_status, trust_verdict) = ctx_traits_io::lifecycle::resolve_named(
                trait_root,
                trait_ref.id.as_str(),
                canonical_digest.as_str(),
            )?;
            let repo_root = ctx_traits_io::export::infer_repo_root_from_trait_file(path);
            let cdk_drift = if input.skip_cdk_drift {
                None
            } else {
                cdk_drift_check(path, &canonical_digest)
            };
            let lockfile = if input.locked {
                ctx_traits_io::lockfile::read_lockfile(trait_root)?
            } else {
                None
            };

            let roots =
                ctx_traits_io::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
            let manifest = ctx_traits_io::resource::digest_resources(
                &roots,
                trait_ref.id.as_str(),
                &trait_ref.resources,
            )?;
            let file_evidence = build_file_evidence_from_io(&manifest);
            let (mut resource_body_evidence, body_read_warnings) =
                report_resources::scan_resource_bodies(&roots, &trait_ref)?;
            let dependency_evidence = dependency_evidence(
                repo_root,
                trait_root,
                &trait_ref,
                lockfile.as_ref(),
                input.locked,
            )?;
            resource_body_evidence.extend(dependency_evidence.body_evidence.clone());
            let resource_plan = ctx_traits_core::resource_plan::plan_resource_inclusion_with_bodies(
                &trait_ref,
                &file_evidence,
                &resource_body_evidence,
                &dependency_evidence.resource_decls,
            );
            let resource_budget =
                ctx_traits_core::resource_plan::estimate_context_budget(&resource_plan);

            let mut resource_read_warnings =
                report_resources::resource_read_warning_strings(&manifest.warnings);
            resource_read_warnings.extend(report_resources::resource_read_warning_strings(
                &body_read_warnings,
            ));
            let render_plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
                &trait_ref,
                ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
                source_digest.as_str(),
                ctx_traits_core::render::ResourceEvidenceInputs {
                    file_evidence: &file_evidence,
                    body_evidence: &resource_body_evidence,
                    dependency_resources: &dependency_evidence.resource_decls,
                    manifest_digest: Some(manifest.manifest_digest.as_str()),
                    read_warnings: resource_read_warnings,
                },
            );
            let mut check_warnings = check_warnings_from_render_and_resources(
                &render_plan,
                &resource_plan,
                &body_read_warnings,
            );
            check_warnings.extend(ctx_traits_core::check::resource_root_advisories(
                &trait_ref.resources,
            ));
            check_warnings.extend(ctx_traits_core::check::family_variant_advisories(
                &trait_id,
                trait_ref.metadata.as_ref(),
            ));
            check_warnings.extend(report_resources::repo_root_audit_skip_warnings(&trait_ref));
            check_warnings.extend(sequence_control_check_warnings(&trait_ref));
            check_warnings.extend(subagent_role_check_warnings(&trait_ref));
            if let Some(cdk_drift) = cdk_drift.as_ref() {
                check_warnings.extend(cdk_drift.warnings.clone());
            }
            check_warnings.extend(dependency_evidence.warnings.clone());
            let active_run_config =
                ctx_traits_io::harness_config::describe_active_package_run_config(
                    Some(&trait_ref),
                    trait_root,
                )?;
            if let Some((tier, run_config_path)) = active_run_config.as_ref() {
                let message = match tier {
                    ctx_traits_io::harness_config::PackageRunConfigTier::Manifest => format!(
                        "package budget in {run_config_path} is active: its [budget] sets vendored execution caps below CLI flags but above built-in defaults; review it since a vendored trait's budget may raise execution caps, and CLI budget flags always override it"
                    ),
                };
                check_warnings.push(ctx_traits_core::check::CheckWarning {
                    section: Section::RunConfig,
                    code: "run-config-sidecar-active".to_string(),
                    field: None,
                    message,
                });
            }

            let resource_protection_issues =
                report_resources::resource_protection_issues(&roots, &trait_ref)?;
            let resource_audit =
                report_resources::audit_declared_text_resources(&roots, &trait_ref)?;
            let mut audit =
                ctx_traits_core::audit::scan_hidden_content(&text, &trait_id, Some(input.file));
            audit.extend(resource_audit.findings);
            audit.extend(dependency_evidence.audit_findings.clone());
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

            let audit_ok = audit.iter().all(|finding| {
                matches!(finding.severity, ctx_traits_core::audit::Severity::Advisory)
            });
            let template_resource_ids =
                report_resources::resource_template_candidate_ids(&trait_ref);
            let resource_ok = resource_plan.warnings.iter().all(|warning| {
                match warning {
                ctx_traits_core::resource_plan::InclusionWarning::SymlinkDetected { .. } => false,
                ctx_traits_core::resource_plan::InclusionWarning::DependencyResourceUnavailable {
                    ..
                } => false,
                ctx_traits_core::resource_plan::InclusionWarning::UndeclaredTemplateInput {
                    resource_id,
                    ..
                }
                | ctx_traits_core::resource_plan::InclusionWarning::TemplateDiagnostic {
                    resource_id,
                    ..
                } => !template_resource_ids.contains(resource_id),
                _ => true,
            }
            });
            let render_ok = render_plan.resource_warnings.is_empty()
                && !render_plan.model_view.has_blocking_post_audit_findings();
            let scenario_audit =
                ctx_traits_core::r#trait::scenario::audit_scenarios(&trait_ref.scenarios);
            let scenario_ids: std::collections::BTreeSet<&str> =
                trait_ref.scenarios.iter().map(|s| s.id.as_str()).collect();
            let eval_audit =
                ctx_traits_core::r#trait::eval::audit_evals(&trait_ref.evals, &scenario_ids);

            let current = report_drift::CurrentDigestEvidence {
                source_digest: source_digest.as_str().to_string(),
                canonical_digest: canonical_digest.as_str().to_string(),
                model_visible_digest: render_plan.model_view.content_digest.to_string(),
                resource_manifest_digest: manifest.manifest_digest.as_str().to_string(),
            };
            let projection_evidence =
                ctx_traits_io::lockfile::read_lockfile(trait_root)?.and_then(|document| {
                    document
                        .trait_entry(&trait_id, trait_variant)
                        .and_then(|entry| entry.projections.first().cloned())
                });
            let projection_model_view_digest = report_drift::projection_model_view_digest(
                report_drift::ProjectionModelViewDigestInput {
                    projection: projection_evidence.as_ref(),
                    trait_ref: &trait_ref,
                    source_digest: source_digest.as_str(),
                    file_evidence: &file_evidence,
                    resource_body_evidence: &resource_body_evidence,
                    dependency_resource_decls: &dependency_evidence.resource_decls,
                    resource_manifest_digest: Some(manifest.manifest_digest.as_str()),
                    manifest_warnings: &manifest.warnings,
                    body_read_warnings: &body_read_warnings,
                },
            )?;
            let projection_status = ctx_traits_core::lockfile::projection_drift_status(
                projection_evidence.as_ref(),
                source_digest.as_str(),
                canonical_digest.as_str(),
                &projection_model_view_digest,
            );
            let projection_summary = report_drift::projection_lock_summary(projection_status);
            let current_canonical_json = serde_json::to_value(&trait_ref).map_err(|e| {
                crate::Error::json("serialize current canonical trait for drift", e)
            })?;
            let run_ledger = report_runtime::read_run_ledger(input.run_ledger)?;
            let eval_reports = report_eval::read_eval_reports(input.eval_reports)?;
            let lock_entry = lockfile
                .as_ref()
                .and_then(|document| document.trait_entry(&trait_id, trait_variant));
            let mut drift = report_drift::locked_drift_summaries(
                input.locked,
                repo_root,
                &trait_id,
                trait_variant,
                &current,
                trait_root,
                &current_canonical_json,
            )?;
            drift.extend(dependency_evidence.drift.clone());
            let import_evidence = report_import::import_evidence_section(trait_root)?;
            let runtime_section = report_runtime::runtime_evidence_section(
                &trait_ref,
                &run_ledger,
                source_digest.as_str(),
                canonical_digest.as_str(),
            )?;
            let eval_section = report_eval::eval_evidence_section(
                &trait_ref,
                source_digest.as_str(),
                lock_entry,
                &eval_reports,
                input.locked || !eval_reports.is_empty(),
            );

            let lifecycle_gates =
                ctx_traits_core::r#trait::activation::lifecycle_status_gates_for_check(
                    &trait_id,
                    &package_status,
                );
            // Owner decision 2026-07-19: lifecycle status and machine trust
            // are RUN gates. `check` reports them as evidence — with the
            // exact remedy commands — but never fails on them; only artifact
            // findings (audit, structure, digests, drift) fail a check.
            let lifecycle_ok = lifecycle_gates.is_empty();
            let lifecycle_detail = format!(
                "status={}{}",
                package_status.display_name(),
                if lifecycle_ok {
                    ", gates=pass".to_string()
                } else {
                    format!(
                        " — reported only; run refuses until cleared: {}",
                        ctx_traits_core::r#trait::activation::format_gate_refusal(&lifecycle_gates)
                    )
                },
            );
            let derived_kinds = trait_ref.derived_kinds();
            let control_bounds = control_bounds_summary(&trait_ref);
            let io_contract = io_contract_summary(&trait_ref);
            let sequence_receipt = sequence_receipt_summary(&trait_ref, run_ledger.as_ref());

            let mut report = ctx_traits_core::check::CheckReport::new(&trait_id, true)
                .with_section(Section::Validation, "trait decoded successfully", true)
                .with_section(
                    Section::DerivedKind,
                    &format!("kinds={}", derived_kinds.join(",")),
                    true,
                )
                .with_section(Section::IoContract, &io_contract, true)
                .with_section(Section::ControlBounds, &control_bounds, true)
                .with_section(Section::Sequence, &sequence_receipt.summary, sequence_receipt.ok)
                .with_section(Section::LifecycleStatus, &lifecycle_detail, true)
                .with_section(
                    Section::MachineTrust,
                    &{
                        let trust_gates =
                            ctx_traits_core::r#trait::activation::trust_gates_for_check(
                                &trait_id,
                                &trust_verdict,
                            );
                        let trust_ok = trust_gates.is_empty();
                        let base = format!(
                            "trust={} (this machine's verdict for the current canonical digest, keyed in ~/.config/ctx/trust.toml)",
                            trust_verdict.display_name()
                        );
                        if trust_ok {
                            base
                        } else {
                            format!(
                                "{base}; reported only; run refuses until reviewed: {} (canonical-digest={}); review the trait's current behavior before approving",
                                ctx_traits_core::r#trait::activation::format_gate_refusal(
                                    &trust_gates
                                ),
                                canonical_digest.as_str()
                            )
                        }
                    },
                    true,
                )
                .with_section(
                    Section::Activation,
                    "not evaluated (check has no activation request facts)",
                    true,
                )
                .with_section(
                    Section::Dependencies,
                    &dependency_evidence.section_summary,
                    dependency_evidence.section_ok,
                )
                .with_section(
                    Section::DependencyTrust,
                    &dependency_evidence.trust_summary,
                    dependency_evidence.trust_ok,
                )
                .with_section(
                    Section::Resources,
                    &format!(
                        "manifest={}, read-warnings={}, inclusion-warnings={}, budget={} bytes/~{} tokens, text-audit-skipped={}",
                        manifest.manifest_digest.as_str(),
                        manifest.warnings.len() + resource_audit.warnings.len(),
                        resource_plan.warnings.len(),
                        resource_budget.total_bytes,
                        resource_budget.estimated_tokens,
                        resource_audit.skipped.len(),
                    ),
                    resource_ok,
                )
                .with_section(
                    Section::ResourceProtection,
                    &if resource_protection_issues.issues.is_empty() {
                        // A trait with resources nobody has digested yet is
                        // named here rather than silently passing — but it
                        // does not fail the check, because that is what every
                        // trait built before resource digests looks like.
                        if resource_protection_issues.unrecorded.is_empty() {
                            "no protected resource drift; every command/check argv resource is \
                             verified"
                                .to_string()
                        } else {
                            format!(
                                "no drift; {} resource(s) carry no digest yet (rebuild to record \
                                 them): {}",
                                resource_protection_issues.unrecorded.len(),
                                resource_protection_issues.unrecorded.join(", ")
                            )
                        }
                    } else {
                        format!(
                            "{} issue(s): {}",
                            resource_protection_issues.issues.len(),
                            resource_protection_issues
                                .issues
                                .iter()
                                .map(|issue| format!("{}: {}", issue.field, issue.message))
                                .collect::<Vec<_>>()
                                .join("; ")
                        )
                    },
                    resource_protection_issues.issues.is_empty(),
                )
                .with_section(
                    Section::RenderReadiness,
                    &format!(
                        "capability-warnings={}, resource-warnings={}, model-view-warnings={}",
                        render_plan.capability_warnings.len(),
                        render_plan.resource_warnings.len(),
                        render_plan.model_view.warnings.len(),
                    ),
                    render_ok,
                )
                .with_section(
                    Section::HiddenContentAudit,
                    &format!("{} finding(s)", audit.len()),
                    audit_ok,
                )
                .with_section(
                    Section::ScenarioEvalAudit,
                    &format!(
                        "scenario-warnings={}, eval-warnings={}",
                        scenario_audit.len(),
                        eval_audit.len(),
                    ),
                    true,
                )
                .with_section(
                    Section::ImportEvidence,
                    &import_evidence.summary,
                    import_evidence.ok,
                )
                .with_section(Section::RuntimeEvidence, &runtime_section.summary, runtime_section.ok)
                .with_section(Section::EvalEvidence, &eval_section.summary, eval_section.ok)
                .with_section(
                    Section::ModelView,
                    &format!(
                        "digest={}, sections={}, normalizations={}",
                        render_plan.model_view.content_digest,
                        render_plan.model_view.sections.len(),
                        render_plan.model_view.normalizations.len(),
                    ),
                    !render_plan.model_view.has_blocking_post_audit_findings(),
                )
                .with_section(
                    Section::ProjectionLock,
                    &projection_summary,
                    !matches!(
                        projection_status,
                        ctx_traits_core::lockfile::ProjectionDriftStatus::SourceDrifted
                    ),
                )
                .with_section(
                    Section::RunConfig,
                    if active_run_config.is_some() {
                        "package run config active"
                    } else {
                        "no package run config"
                    },
                    true,
                )
                .with_section(
                    Section::Extensions,
                    &extensions_section_summary(&trait_ref),
                    true,
                )
                .with_section(
                    Section::Metadata,
                    &metadata_section_summary(trait_ref.metadata.as_ref()),
                    true,
                )
                .with_unsupported_capabilities(vec![
                    "activation.request-facts-missing".to_string(),
                    "policy-manifest-drift-not-wired".to_string(),
                ])
                .with_audit(audit)
                .with_warnings(check_warnings)
                .with_drift(drift);
            if let Some(cdk_drift) = cdk_drift {
                let authoring_count = cdk_drift
                    .warnings
                    .iter()
                    .filter(|warning| warning.section == Section::CdkAuthoring)
                    .count();
                report = report
                    .with_section(Section::CdkDrift, &cdk_drift.summary, cdk_drift.ok)
                    .with_section(
                        Section::CdkAuthoring,
                        &if authoring_count == 0 {
                            "no authoring diagnostics".to_string()
                        } else {
                            format!("{authoring_count} authoring diagnostic(s)")
                        },
                        true,
                    );
                if let Some(provenance) = cdk_drift.provenance {
                    report = report.with_synth_provenance(vec![provenance]);
                }
            }
            if let Some(forked_from) = forked_from_summary(trait_root)? {
                report = report.with_section(Section::ForkedFrom, &forked_from, true);
            }
            Ok(report)
        }
        Err(e) => {
            let report = ctx_traits_core::check::CheckReport {
                validation_error: Some(e.to_string()),
                ..ctx_traits_core::check::CheckReport::new("unknown", false).with_section(
                    Section::Validation,
                    "trait failed to decode",
                    false,
                )
            };
            Ok(report)
        }
    }
}

pub(crate) fn dependency_evidence(
    repo_root: &camino::Utf8Path,
    trait_root: &camino::Utf8Path,
    trait_ref: &ctx_traits_core::Trait,
    lockfile: Option<&ctx_traits_io::lockfile::Document>,
    locked: bool,
) -> crate::Result<DependencyEvidence> {
    let declarations =
        ctx_traits_io::dependency::declarations_for_trait(repo_root, trait_root, trait_ref)?;
    let qualified_refs = collect_qualified_refs(trait_ref)?;
    if declarations.is_empty() && qualified_refs.is_empty() {
        return Ok(DependencyEvidence {
            section_summary: "no dependency declarations or qualified refs".to_string(),
            section_ok: true,
            trust_summary: "no dependency trust required".to_string(),
            trust_ok: true,
            body_evidence: Vec::new(),
            resource_decls: Vec::new(),
            audit_findings: Vec::new(),
            warnings: Vec::new(),
            drift: Vec::new(),
        });
    }

    let declarations_by_alias: std::collections::BTreeMap<
        String,
        ctx_traits_io::dependency::DependencyDeclaration,
    > = declarations
        .into_iter()
        .map(|declaration| (declaration.dependency.alias.clone(), declaration))
        .collect();
    let mut loaded_by_alias = std::collections::BTreeMap::new();
    let mut pending_aliases = Vec::new();
    let mut failed = Vec::new();
    let mut warnings = Vec::new();
    let mut audit_findings = Vec::new();
    let mut drift = Vec::new();

    for (alias, declaration) in &declarations_by_alias {
        match ctx_traits_io::dependency::load_vendored_dependency(repo_root, alias)? {
            Some(loaded) => {
                if loaded.id != declaration.dependency.id {
                    failed.push(format!(
                        "dependency:{alias} id mismatch expected {} found {}",
                        declaration.dependency.id, loaded.id
                    ));
                }
                if loaded.version != declaration.dependency.version {
                    failed.push(format!(
                        "dependency:{alias} version mismatch expected {} found {}",
                        declaration.dependency.version, loaded.version
                    ));
                }
                if !loaded.trait_ref.dependencies.is_empty() {
                    warnings.push(ctx_traits_core::check::CheckWarning {
                        section: Section::Dependencies,
                        code: "dependency-nested-resolution-deferred".to_string(),
                        field: Some(format!("dependency:{alias}")),
                        message: ctx_traits_io::dependency::nested_dependency_warning(
                            alias,
                            loaded.trait_ref.dependencies.len(),
                        ),
                    });
                }
                audit_findings.extend(audit_loaded_dependency(&loaded));
                if locked {
                    drift.extend(locked_dependency_drift(alias, lockfile, &loaded));
                }
                loaded_by_alias.insert(alias.clone(), loaded);
            }
            None => pending_aliases.push(alias.clone()),
        }
    }

    let mut resolved_refs = 0usize;
    let mut pending_refs = 0usize;
    let mut failed_refs = 0usize;
    let mut body_evidence = Vec::new();
    let mut resource_decls = Vec::new();
    let mut materialized_resources = std::collections::BTreeSet::new();

    for qualified in &qualified_refs {
        let Some(_declaration) = declarations_by_alias.get(&qualified.alias) else {
            failed_refs += 1;
            failed.push(format!(
                "{} uses undeclared dependency alias {:?} in {}",
                qualified.ref_text, qualified.alias, qualified.field
            ));
            continue;
        };
        let Some(loaded) = loaded_by_alias.get(&qualified.alias) else {
            pending_refs += 1;
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::Dependencies,
                code: "dependency-vendor-pending".to_string(),
                field: Some(qualified.field.clone()),
                message: format!(
                    "{} awaits vendored dependency alias {}",
                    qualified.ref_text, qualified.alias
                ),
            });
            continue;
        };
        match dependency_symbol_status(&loaded.trait_ref, qualified.kind, &qualified.id) {
            SymbolStatus::Resolved => {
                resolved_refs += 1;
                if qualified.kind == ctx_traits_core::reference::Kind::Resource {
                    materialize_dependency_resource(
                        loaded,
                        qualified,
                        &mut body_evidence,
                        &mut resource_decls,
                        &mut materialized_resources,
                        &mut warnings,
                    )?;
                }
            }
            SymbolStatus::Missing => {
                failed_refs += 1;
                failed.push(format!(
                    "{} missing in dependency:{} version {}",
                    qualified.ref_text, qualified.alias, loaded.version
                ));
            }
            SymbolStatus::KindMismatch { actual } => {
                failed_refs += 1;
                failed.push(format!(
                    "{} kind mismatch in dependency:{} version {} (found {})",
                    qualified.ref_text,
                    qualified.alias,
                    loaded.version,
                    actual.as_str()
                ));
            }
        }
    }

    failed.sort();
    failed.dedup();
    pending_aliases.sort();
    pending_aliases.dedup();
    let trust = dependency_trust_summary(loaded_by_alias.values(), locked)?;
    warnings.extend(trust.warnings);
    warnings.sort_by(|a, b| {
        a.section
            .cmp(&b.section)
            .then(a.code.cmp(&b.code))
            .then(a.field.cmp(&b.field))
            .then(a.message.cmp(&b.message))
    });
    warnings.dedup();
    audit_findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.code.cmp(&b.code))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.message.cmp(&b.message))
    });

    let section_ok = failed.is_empty();
    let failed_summary = if failed.is_empty() {
        "none".to_string()
    } else {
        failed.join("; ")
    };
    Ok(DependencyEvidence {
        section_summary: format!(
            "declared={}, vendored={}, refs: resolved={}, pending={}, failed={}, materialized-resources={}, pending-aliases={}, failures={}",
            declarations_by_alias.len(),
            loaded_by_alias.len(),
            resolved_refs,
            pending_refs,
            failed_refs,
            materialized_resources.len(),
            if pending_aliases.is_empty() {
                "none".to_string()
            } else {
                pending_aliases.join(",")
            },
            failed_summary,
        ),
        section_ok,
        trust_summary: trust.summary,
        trust_ok: trust.ok,
        body_evidence,
        resource_decls,
        audit_findings,
        warnings,
        drift,
    })
}

enum SymbolStatus {
    Resolved,
    Missing,
    KindMismatch {
        actual: ctx_traits_core::reference::Kind,
    },
}

struct TrustSummary {
    summary: String,
    ok: bool,
    warnings: Vec<ctx_traits_core::check::CheckWarning>,
}

fn collect_qualified_refs(
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<QualifiedRefUse>> {
    let value = serde_json::to_value(trait_ref)
        .map_err(|e| crate::Error::json("serialize trait for dependency ref scan", e))?;
    let mut refs = Vec::new();
    collect_qualified_refs_from_value(&value, "root", &mut refs);
    refs.sort_by(|a, b| {
        a.alias
            .cmp(&b.alias)
            .then(a.kind.cmp(&b.kind))
            .then(a.id.cmp(&b.id))
            .then(a.field.cmp(&b.field))
            .then(a.ref_text.cmp(&b.ref_text))
    });
    refs.dedup_by(|a, b| a.field == b.field && a.ref_text == b.ref_text);
    Ok(refs)
}

fn collect_qualified_refs_from_value(
    value: &serde_json::Value,
    field: &str,
    refs: &mut Vec<QualifiedRefUse>,
) {
    match value {
        serde_json::Value::String(text) => {
            let ref_text = text
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(text);
            if let Ok(parsed) = ctx_traits_core::reference::Reference::parse(ref_text)
                && let ctx_traits_core::reference::Path::Qualified { namespace, id, .. } =
                    parsed.ref_path()
            {
                refs.push(QualifiedRefUse {
                    field: field.to_string(),
                    ref_text: ref_text.to_string(),
                    kind: parsed.kind(),
                    alias: namespace.to_string(),
                    id: id.to_string(),
                });
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_qualified_refs_from_value(item, &format!("{field}[{index}]"), refs);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if is_prose_ref_scan_field(key) {
                    continue;
                }
                collect_qualified_refs_from_value(item, &format!("{field}.{key}"), refs);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn is_prose_ref_scan_field(key: &str) -> bool {
    matches!(
        key,
        "description" | "summary" | "hint" | "title" | "text" | "reason" | "name"
    )
}

fn dependency_symbol_status(
    trait_ref: &ctx_traits_core::Trait,
    kind: ctx_traits_core::reference::Kind,
    id: &str,
) -> SymbolStatus {
    if dependency_has_kind(trait_ref, kind, id) {
        return SymbolStatus::Resolved;
    }
    match dependency_actual_kind(trait_ref, id) {
        Some(actual) => SymbolStatus::KindMismatch { actual },
        None => SymbolStatus::Missing,
    }
}

fn dependency_has_kind(
    trait_ref: &ctx_traits_core::Trait,
    kind: ctx_traits_core::reference::Kind,
    id: &str,
) -> bool {
    match kind {
        ctx_traits_core::reference::Kind::Prompt => trait_ref.prompts.get(id).is_some(),
        ctx_traits_core::reference::Kind::Resource => {
            trait_ref.resources.iter().any(|resource| resource.id == id)
        }
        ctx_traits_core::reference::Kind::Schema => {
            trait_ref.schemas.iter().any(|schema| schema.id == id)
        }
        ctx_traits_core::reference::Kind::Slot => trait_ref.slots.iter().any(|slot| slot.id == id),
        ctx_traits_core::reference::Kind::Port => trait_ref.ports.iter().any(|port| port.id == id),
        ctx_traits_core::reference::Kind::Agent => {
            trait_ref.agents.iter().any(|agent| agent.id == id)
        }
        ctx_traits_core::reference::Kind::Session => {
            trait_ref.sessions.iter().any(|session| session.id == id)
        }
        ctx_traits_core::reference::Kind::Signal => {
            trait_ref.signals.iter().any(|signal| signal.id == id)
        }
        ctx_traits_core::reference::Kind::Sequence => trait_ref.sequences.get(id).is_some(),
        ctx_traits_core::reference::Kind::Condition => trait_ref.conditions.get(id).is_some(),
        _ => false,
    }
}

fn dependency_actual_kind(
    trait_ref: &ctx_traits_core::Trait,
    id: &str,
) -> Option<ctx_traits_core::reference::Kind> {
    use ctx_traits_core::reference::Kind;
    [
        Kind::Prompt,
        Kind::Resource,
        Kind::Schema,
        Kind::Slot,
        Kind::Port,
        Kind::Agent,
        Kind::Session,
        Kind::Signal,
        Kind::Sequence,
        Kind::Condition,
    ]
    .into_iter()
    .find(|kind| dependency_has_kind(trait_ref, *kind, id))
}

fn materialize_dependency_resource(
    loaded: &ctx_traits_io::dependency::LoadedDependency,
    qualified: &QualifiedRefUse,
    body_evidence: &mut Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    resource_decls: &mut Vec<ctx_traits_core::resource_plan::DependencyResourceDecl>,
    materialized: &mut std::collections::BTreeSet<String>,
    warnings: &mut Vec<ctx_traits_core::check::CheckWarning>,
) -> crate::Result<()> {
    let qualified_id = format!("{}/{}", qualified.alias, qualified.id);
    if !materialized.insert(qualified_id.clone()) {
        return Ok(());
    }
    // Declaration metadata (render mode, presentation path) is carried
    // independently of body text: a reference-mode dependency resource
    // never needs its body scanned, and a resource whose body scan simply
    // failed must still retain a plan entry and a usable pointer.
    let decl = ctx_traits_io::dependency::dependency_resource_decl(loaded, &qualified.id)?;
    let needs_body = decl
        .as_ref()
        .is_none_or(|decl| decl.render == ctx_traits_core::r#trait::ResourceRender::Inline);
    if let Some(decl) = decl {
        resource_decls.push(decl);
    }
    match loaded
        .body_evidence
        .iter()
        .find(|body| body.resource_id == qualified.id)
    {
        Some(body) => {
            let mut body = body.clone();
            body.resource_id = qualified_id;
            body_evidence.push(body);
        }
        None if needs_body => warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Dependencies,
            code: "dependency-resource-body-unavailable".to_string(),
            field: Some(qualified.field.clone()),
            message: format!(
                "{} resolved but no readable text body was materialized",
                qualified.ref_text
            ),
        }),
        None => {}
    }
    Ok(())
}

fn audit_loaded_dependency(
    loaded: &ctx_traits_io::dependency::LoadedDependency,
) -> Vec<ctx_traits_core::audit::Finding> {
    let trait_id = format!("dependency:{}", loaded.alias);
    let mut findings = ctx_traits_core::audit::scan_hidden_content(
        &loaded.source_text,
        &trait_id,
        Some(loaded.manifest_path.as_str()),
    );
    findings.extend(ctx_traits_core::audit::scan_hidden_content(
        &loaded.model_visible_text,
        &trait_id,
        Some("model-view"),
    ));
    for body in &loaded.body_evidence {
        if let Some(text) = body.text.as_deref() {
            findings.extend(ctx_traits_core::audit::scan_hidden_content(
                text,
                &trait_id,
                Some(&format!("resource:{}", body.resource_id)),
            ));
        }
    }
    findings
}

fn locked_dependency_drift(
    alias: &str,
    lockfile: Option<&ctx_traits_io::lockfile::Document>,
    loaded: &ctx_traits_io::dependency::LoadedDependency,
) -> Vec<ctx_traits_core::check::DriftSummary> {
    let Some(lockfile) = lockfile else {
        return Vec::new();
    };
    let Some(entry) = lockfile.dependency_entry(alias) else {
        return vec![ctx_traits_core::check::DriftSummary {
            layer: ctx_traits_core::check::DriftLayer::Dependency,
            expected: format!("dependency lock entry {alias}"),
            actual: None,
            summary: format!("missing dependency lock entry for {alias}"),
            unsupported: false,
        }];
    };
    let comparisons = [
        (
            "source",
            entry.source_digest(),
            loaded.source_digest.as_str(),
        ),
        (
            "canonical",
            entry.canonical_digest(),
            loaded.canonical_digest.as_str(),
        ),
        (
            "model-visible",
            entry.model_visible_digest(),
            loaded.model_visible_digest.as_str(),
        ),
        (
            "resource-manifest",
            entry.resource_manifest_digest(),
            loaded.resource_manifest_digest.as_str(),
        ),
    ];
    let mut mismatches = Vec::new();
    for (label, expected, actual) in comparisons {
        match expected {
            Some(expected) if expected == actual => {}
            Some(expected) => {
                mismatches.push(format!("{label} expected {expected} actual {actual}"))
            }
            None => mismatches.push(format!("{label} missing from lock actual {actual}")),
        }
    }
    let ok_text = format!("dependency {alias} digests match lock evidence");
    vec![ctx_traits_core::check::DriftSummary {
        layer: ctx_traits_core::check::DriftLayer::Dependency,
        expected: ok_text.clone(),
        actual: Some(if mismatches.is_empty() {
            ok_text.clone()
        } else {
            mismatches.join("; ")
        }),
        summary: if mismatches.is_empty() {
            ok_text
        } else {
            format!("dependency {alias} digest drift")
        },
        unsupported: false,
    }]
}

fn dependency_trust_summary<'a>(
    dependencies: impl Iterator<Item = &'a ctx_traits_io::dependency::LoadedDependency>,
    locked: bool,
) -> crate::Result<TrustSummary> {
    let store = ctx_traits_io::trust::read_store()?;
    let mut verified = 0usize;
    let mut blocked = Vec::new();
    let mut unknown = Vec::new();
    for dependency in dependencies {
        let canonical = dependency.canonical_digest.as_str();
        // Trust is bound to the canonical digest only. A model-visible-only
        // digest record must never verify a dependency: the model-visible
        // text is a derived projection, not the reviewed artifact, so a
        // trust decision keyed on it cannot stand in for review of the
        // canonical document actually resolved and run.
        let canonical_record = store.record(canonical);
        if canonical_record
            .is_some_and(|record| record.state == ctx_traits_io::trust::TrustState::Blocked)
        {
            blocked.push(dependency.alias.clone());
        } else if canonical_record
            .is_some_and(|record| record.state == ctx_traits_io::trust::TrustState::Verified)
        {
            verified += 1;
        } else {
            unknown.push(format!(
                "{}:{}",
                dependency.alias,
                dependency.canonical_digest.as_str()
            ));
        }
    }
    let mut warnings = Vec::new();
    for item in &unknown {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::DependencyTrust,
            code: "dependency-trust-unknown".to_string(),
            field: Some(item.clone()),
            message: "dependency digest has no local trust decision".to_string(),
        });
    }
    for alias in &blocked {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::DependencyTrust,
            code: "dependency-trust-blocked".to_string(),
            field: Some(alias.clone()),
            message: "dependency digest is locally blocked".to_string(),
        });
    }
    // Owner decision 2026-07-19: dependency trust is machine-local run-gate
    // state, like the trait's own trust — `check` reports the counts and
    // warns, `run` enforces. `--locked` continues to police byte-lock
    // reproducibility elsewhere; it does not turn trust into a check failure.
    let _ = locked;
    Ok(TrustSummary {
        summary: format!(
            "verified={}, unknown={}, blocked={} (machine-local; reported by check, enforced by run)",
            verified,
            unknown.len(),
            blocked.len(),
        ),
        ok: true,
        warnings,
    })
}

fn extensions_section_summary(trait_ref: &ctx_traits_core::Trait) -> String {
    match trait_ref
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.ctx.as_ref())
    {
        Some(_) => "extensions.ctx metadata declared".to_string(),
        None => "none declared".to_string(),
    }
}

fn metadata_section_summary(metadata: Option<&ctx_traits_core::r#trait::Metadata>) -> String {
    let Some(metadata) = metadata else {
        return "no family/variant identity declared".to_string();
    };
    match (metadata.family.as_ref(), metadata.variant.as_ref()) {
        (None, None) => "no family/variant identity declared".to_string(),
        (family, variant) => format!(
            "family={} variant={}",
            family.map(|slug| slug.as_str()).unwrap_or("none"),
            variant.map(|slug| slug.as_str()).unwrap_or("none"),
        ),
    }
}

fn subagent_role_check_warnings(
    trait_ref: &ctx_traits_core::Trait,
) -> Vec<ctx_traits_core::check::CheckWarning> {
    let Some(role) = trait_ref
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.ctx.as_ref())
        .and_then(|ctx| ctx.subagent.as_ref())
        .and_then(|subagent| subagent.role.as_deref())
    else {
        return Vec::new();
    };
    if trait_ref.agents.is_empty() || trait_ref.agents.iter().any(|agent| agent.id == role) {
        return Vec::new();
    }
    vec![ctx_traits_core::check::CheckWarning {
        section: Section::Extensions,
        code: "subagent.role-agent-mismatch".to_string(),
        field: Some("extensions.ctx.subagent.role".to_string()),
        message: format!(
            "subagent role {role:?} is advisory metadata and does not match any declared [[agent]] role"
        ),
    }]
}

pub(crate) fn check_warnings_from_render_and_resources(
    render_plan: &ctx_traits_core::render::RenderPlan,
    resource_plan: &ctx_traits_core::resource_plan::Plan,
    body_read_warnings: &[ctx_traits_io::resource::ResourceReadWarning],
) -> Vec<ctx_traits_core::check::CheckWarning> {
    let mut warnings = Vec::new();
    for warning in &render_plan.capability_warnings {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::RenderReadiness,
            code: "render-capability".to_string(),
            field: Some(warning.field.clone()),
            message: warning.reason.clone(),
        });
    }
    for warning in &render_plan.resource_warnings {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::RenderReadiness,
            code: "render-resource".to_string(),
            field: Some(format!("resource.{}", warning.resource_id)),
            message: warning.reason.clone(),
        });
    }
    for warning in &render_plan.resource_read_warnings {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Resources,
            code: "resource-read".to_string(),
            field: None,
            message: warning.clone(),
        });
    }
    for warning in body_read_warnings {
        let (code, field, message) = check_resource_read_warning(warning);
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Resources,
            code,
            field,
            message,
        });
    }
    for warning in &resource_plan.warnings {
        let (code, field, message) = check_resource_inclusion_warning(warning);
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Resources,
            code,
            field,
            message,
        });
    }
    for warning in &render_plan.model_view.warnings {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::ModelView,
            code: model_view_warning_code(warning),
            field: model_view_warning_field(warning),
            message: warning.clone(),
        });
    }
    if let Some(frontmatter) = &render_plan.frontmatter {
        for warning in &frontmatter.sanitization_warnings {
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::RenderReadiness,
                code: "frontmatter-sanitization".to_string(),
                field: model_view_warning_field(warning),
                message: warning.clone(),
            });
        }
    }
    warnings.sort_by(|a, b| {
        a.section
            .cmp(&b.section)
            .then(a.code.cmp(&b.code))
            .then(a.field.cmp(&b.field))
            .then(a.message.cmp(&b.message))
    });
    warnings.dedup();
    warnings
}

pub(crate) fn sequence_control_check_warnings(
    trait_ref: &ctx_traits_core::Trait,
) -> Vec<ctx_traits_core::check::CheckWarning> {
    let mut warnings = Vec::new();
    if !trait_ref.sequences.is_empty() {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Sequence,
            code: "sequence-control.runtime-only".to_string(),
            field: Some("sequence".to_string()),
            message: "named sequences execute only through the controlled ctx runtime; static renders are advisory".to_string(),
        });
    }
    if !trait_ref.conditions.is_empty() {
        warnings.push(ctx_traits_core::check::CheckWarning {
            section: Section::Sequence,
            code: "condition.runtime-evidence-only".to_string(),
            field: Some("condition".to_string()),
            message: "conditions match accepted typed slot/signal evidence, not raw model prose"
                .to_string(),
        });
    }
    if let Some(procedure) = trait_ref.procedure.as_ref() {
        collect_sequence_control_warnings(
            trait_ref,
            &procedure.sequence,
            "procedure.sequence",
            &mut warnings,
        );
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        collect_sequence_control_warnings(
            trait_ref,
            &sequence.sequence,
            &format!("sequence.{sequence_id}.sequence"),
            &mut warnings,
        );
    }
    warnings
}

fn control_bounds_summary(trait_ref: &ctx_traits_core::Trait) -> String {
    let mut bounds = Vec::new();
    if let Some(procedure) = trait_ref.procedure.as_ref() {
        collect_control_bounds(
            trait_ref,
            &procedure.sequence,
            "procedure.sequence",
            &mut bounds,
            &mut std::collections::BTreeSet::new(),
        );
    }
    if bounds.is_empty() {
        "no sequence-control loops declared".to_string()
    } else {
        bounds.join("; ")
    }
}

fn io_contract_summary(trait_ref: &ctx_traits_core::Trait) -> String {
    if trait_ref.ports.is_empty() {
        return "ports=none".to_string();
    }

    let mut entries: Vec<String> = trait_ref
        .ports
        .iter()
        .map(|port| {
            let direction = match port.direction {
                ctx_traits_core::r#trait::PortDirection::Input => "input",
                ctx_traits_core::r#trait::PortDirection::Output => "output",
            };
            let value = port
                .value
                .as_deref()
                .map(|value| format!(", value={value}"))
                .unwrap_or_default();
            let default = if port.default.is_some() {
                ", default=command"
            } else {
                ""
            };
            format!(
                "{direction}:{} schema={} optional={}{}{default}",
                port.id, port.schema, port.optional, value
            )
        })
        .collect();
    entries.sort();
    format!("ports={}", entries.join("; "))
}

fn sequence_receipt_summary(
    trait_ref: &ctx_traits_core::Trait,
    run_ledger: Option<&ctx_traits_core::procedure::runtime::State>,
) -> SequenceReceipt {
    let Some(procedure) = trait_ref.procedure.as_ref() else {
        return SequenceReceipt {
            summary: "no procedure sequence declared".to_string(),
            ok: true,
        };
    };

    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let input = format_string_iter(procedure.input.iter().map(String::as_str));
    let output = format_string_iter(procedure.output.iter().map(String::as_str));
    entries.push(format!(
        "procedure-contract input={} output={}",
        input, output
    ));
    let ordered = ordered_procedure_items(procedure);
    collect_sequence_receipt_items(
        trait_ref,
        &ordered,
        "sequence:procedure",
        &mut entries,
        &mut seen,
    );
    if let Some(ledger) = run_ledger {
        entries.extend(ledger.branch_decisions.iter().map(|decision| {
            format!(
                "branch-decision id={} parent-run={} selected={} sequence={} path={}",
                decision.branch_id,
                decision.parent_run_index,
                decision.selected_arm,
                decision.sequence_id.as_deref().unwrap_or("none"),
                serde_json::to_string(&decision.position_path).unwrap_or_else(|_| "[]".to_string())
            )
        }));
    }
    SequenceReceipt {
        summary: entries.join("; "),
        ok: true,
    }
}

fn ordered_procedure_items(
    procedure: &ctx_traits_core::r#trait::procedure::Model,
) -> Vec<&ctx_traits_core::r#trait::procedure::SequenceItem> {
    procedure.sequence.iter().collect()
}

fn collect_sequence_receipt_items(
    trait_ref: &ctx_traits_core::Trait,
    items: &[&ctx_traits_core::r#trait::procedure::SequenceItem],
    base_ref: &str,
    entries: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    for (index, item) in items.iter().enumerate() {
        let step_id = item
            .id
            .clone()
            .unwrap_or_else(|| format!("step-{}", index + 1));
        let construct_ref = format!("{base_ref}/{step_id}");
        entries.push(sequence_item_receipt(&construct_ref, item));

        collect_sequence_receipt_arm(
            trait_ref,
            item.sequence.as_deref(),
            "then",
            &construct_ref,
            entries,
            seen,
        );
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::Branch {
            collect_sequence_receipt_arm(
                trait_ref,
                item.otherwise.as_deref(),
                "otherwise",
                &construct_ref,
                entries,
                seen,
            );
        }
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::Parallel {
            for (branch_index, branch_ref) in item.branches.iter().enumerate() {
                collect_sequence_receipt_arm(
                    trait_ref,
                    Some(branch_ref),
                    &format!("branch{branch_index}"),
                    &construct_ref,
                    entries,
                    seen,
                );
            }
        }
    }
}

fn collect_sequence_receipt_arm(
    trait_ref: &ctx_traits_core::Trait,
    sequence_ref: Option<&str>,
    arm: &str,
    construct_ref: &str,
    entries: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    let Some(sequence_id) = local_sequence_id_for_check(sequence_ref) else {
        return;
    };
    if !seen.insert(sequence_id.clone()) {
        entries.push(format!(
            "sequence:{sequence_id} recursion=skipped(already-seen)"
        ));
        return;
    }
    if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
        let nested: Vec<_> = sequence.sequence.iter().collect();
        collect_sequence_receipt_items(
            trait_ref,
            &nested,
            &format!("{construct_ref}/{arm}:sequence:{sequence_id}"),
            entries,
            seen,
        );
    }
    seen.remove(&sequence_id);
}

fn sequence_item_receipt(
    construct_ref: &str,
    item: &ctx_traits_core::r#trait::procedure::SequenceItem,
) -> String {
    let kind = item.effective_kind();
    let bound = sequence_bound_receipt(item, kind);
    let agent = item
        .agent
        .as_deref()
        .map(|agent| format!(", agent={agent}"))
        .unwrap_or_default();
    let input = format_string_iter(item.input.ref_texts());
    let output = format_string_iter(item.output.ref_texts());
    let on_complete = format_string_iter(item.on_complete.iter().map(|emit| emit.signal_ref()));
    format!(
        "{construct_ref} kind={} {bound}{agent}, input={}, output={}, on-complete={}",
        sequence_kind_label(kind),
        input,
        output,
        on_complete,
    )
}

fn sequence_bound_receipt(
    item: &ctx_traits_core::r#trait::procedure::SequenceItem,
    kind: ctx_traits_core::r#trait::procedure::SequenceKind,
) -> String {
    match kind {
        ctx_traits_core::r#trait::procedure::SequenceKind::Prompt
        | ctx_traits_core::r#trait::procedure::SequenceKind::Ask
        | ctx_traits_core::r#trait::procedure::SequenceKind::Command
        | ctx_traits_core::r#trait::procedure::SequenceKind::Check
        | ctx_traits_core::r#trait::procedure::SequenceKind::Project => {
            "provable=single-step".to_string()
        }
        ctx_traits_core::r#trait::procedure::SequenceKind::Sequence => {
            let target = item.sequence.as_deref().unwrap_or("missing");
            format!("provable=delegates-to-named-sequence target={target}")
        }
        ctx_traits_core::r#trait::procedure::SequenceKind::Branch => format!(
            "provable=conditional guard={} then={} otherwise={}",
            item.when.is_some(),
            item.sequence.as_deref().unwrap_or("missing"),
            item.otherwise.as_deref().unwrap_or("none")
        ),
        ctx_traits_core::r#trait::procedure::SequenceKind::Loop => {
            let on_abort = loop_on_abort_report(item);
            match item.max_iterations {
                Some(iterations) => format!(
                    "provable=bounded max-iterations={} abort-if={} on-abort={}",
                    iterations,
                    item.abort_if.is_some(),
                    on_abort
                ),
                None => item.max_iterations_from.as_ref().map_or_else(
                    || {
                        format!(
                            "unbounded (will not run) max-iterations=missing abort-if={} on-abort={}",
                            item.abort_if.is_some(),
                            on_abort
                        )
                    },
                    |source| {
                        format!(
                            "provable=runtime-bounded max-iterations-from={} abort-if={} on-abort={}",
                            source,
                            item.abort_if.is_some(),
                            on_abort
                        )
                    },
                ),
            }
        }
        ctx_traits_core::r#trait::procedure::SequenceKind::ForEach => {
            let base = match item.max_items {
                Some(limit) => format!("provable=bounded max-items={limit}"),
                None => "provable=finite-over-slot limit=unset".to_string(),
            };
            if item.concurrent {
                // The core runtime always advances `for-each` items
                // sequentially, in authored order, on the single ledger
                // cursor (P402) — `concurrent` is only ever an opt-in hint a
                // CLI/IO-layer driver may use to speculatively dispatch
                // items ahead of that cursor (`ctx traits drive
                // --max-in-flight`), never a change to core disposition.
                format!("{base} concurrent=serial-core+optional-driver-dispatch(P402)")
            } else {
                base
            }
        }
        ctx_traits_core::r#trait::procedure::SequenceKind::Parallel => format!(
            "provable=parallel-static max-branches={} branches=[{}] join={}",
            item.max_branches
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            item.branches.iter().cloned().collect::<Vec<_>>().join(","),
            item.join.as_ref().map_or(
                "collect-in-order",
                ctx_traits_core::r#trait::procedure::JoinPolicy::label
            )
        ),
        ctx_traits_core::r#trait::procedure::SequenceKind::Terminal => format!(
            "provable=terminal-static outcome={}",
            item.outcome
                .map(|outcome| format!("{outcome:?}"))
                .unwrap_or_else(|| "missing".to_string())
        ),
    }
}

pub(crate) fn sequence_kind_label(
    kind: ctx_traits_core::r#trait::procedure::SequenceKind,
) -> &'static str {
    match kind {
        ctx_traits_core::r#trait::procedure::SequenceKind::Prompt => "prompt",
        ctx_traits_core::r#trait::procedure::SequenceKind::Ask => "ask",
        ctx_traits_core::r#trait::procedure::SequenceKind::Command => "command",
        ctx_traits_core::r#trait::procedure::SequenceKind::Check => "check",
        ctx_traits_core::r#trait::procedure::SequenceKind::Project => "project",
        ctx_traits_core::r#trait::procedure::SequenceKind::Sequence => "sequence",
        ctx_traits_core::r#trait::procedure::SequenceKind::Branch => "branch",
        ctx_traits_core::r#trait::procedure::SequenceKind::Loop => "loop",
        ctx_traits_core::r#trait::procedure::SequenceKind::ForEach => "for-each",
        ctx_traits_core::r#trait::procedure::SequenceKind::Parallel => "parallel",
        ctx_traits_core::r#trait::procedure::SequenceKind::Terminal => "terminal",
    }
}

/// The loop bounds line's `on-abort=` value: `none` when undeclared, else the
/// authored terminal signal(s) a `abort-if` match emits in place of the
/// canonical `signal:abort-if-matched`.
fn loop_on_abort_report(item: &ctx_traits_core::r#trait::procedure::SequenceItem) -> String {
    let signals = item
        .on_abort
        .as_ref()
        .map(ctx_traits_core::r#trait::procedure::ExhaustionTarget::signals)
        .unwrap_or_default();
    if signals.is_empty() {
        "none".to_string()
    } else {
        signals.join(",")
    }
}

fn format_string_iter<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut values: Vec<&str> = values.collect();
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(",")
    }
}

fn collect_control_bounds(
    trait_ref: &ctx_traits_core::Trait,
    items: &[ctx_traits_core::r#trait::procedure::SequenceItem],
    base: &str,
    bounds: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    for (index, item) in items.iter().enumerate() {
        let field = format!("{base}[{index}]");
        match item.effective_kind() {
            ctx_traits_core::r#trait::procedure::SequenceKind::Loop => {
                let iterations = item
                    .max_iterations
                    .map(|value| value.to_string())
                    .or_else(|| item.max_iterations_from.clone())
                    .unwrap_or_else(|| "missing".to_string());
                let abort_if = if item.abort_if.is_some() {
                    ", abort-if"
                } else {
                    ""
                };
                bounds.push(format!("{field}: loop iterations={iterations}{abort_if}"));
            }
            ctx_traits_core::r#trait::procedure::SequenceKind::ForEach => {
                let limit = item
                    .max_items
                    .map(|value| format!("limit={value}"))
                    .unwrap_or_else(|| "limit=finite-over-slot".to_string());
                bounds.push(format!("{field}: for-each {limit}"));
            }
            ctx_traits_core::r#trait::procedure::SequenceKind::Branch => {
                bounds.push(format!("{field}: branch conditional"));
            }
            ctx_traits_core::r#trait::procedure::SequenceKind::Parallel => {
                bounds.push(format!(
                    "{field}: parallel branches={} max-branches={} join={}{}",
                    item.branches.as_slice().len(),
                    item.max_branches
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "missing".to_string()),
                    item.join.as_ref().map_or(
                        "collect-in-order",
                        ctx_traits_core::r#trait::procedure::JoinPolicy::label
                    ),
                    if item.branch_failure.is_empty() {
                        String::new()
                    } else {
                        format!(" branch-failure={}", item.branch_failure.len())
                    }
                ));
            }
            ctx_traits_core::r#trait::procedure::SequenceKind::Sequence
            | ctx_traits_core::r#trait::procedure::SequenceKind::Prompt
            | ctx_traits_core::r#trait::procedure::SequenceKind::Ask
            | ctx_traits_core::r#trait::procedure::SequenceKind::Command
            | ctx_traits_core::r#trait::procedure::SequenceKind::Check
            | ctx_traits_core::r#trait::procedure::SequenceKind::Project
            | ctx_traits_core::r#trait::procedure::SequenceKind::Terminal => {}
        }
        collect_control_bounds_arm(trait_ref, item.sequence.as_deref(), bounds, seen);
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::Branch {
            collect_control_bounds_arm(trait_ref, item.otherwise.as_deref(), bounds, seen);
        }
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::Parallel {
            for branch_ref in item.branches.iter() {
                collect_control_bounds_arm(trait_ref, Some(branch_ref), bounds, seen);
            }
        }
    }
}

fn collect_control_bounds_arm(
    trait_ref: &ctx_traits_core::Trait,
    sequence_ref: Option<&str>,
    bounds: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    let Some(sequence_id) = local_sequence_id_for_check(sequence_ref) else {
        return;
    };
    if !seen.insert(sequence_id.clone()) {
        return;
    }
    if let Some(sequence) = trait_ref.sequences.get(&sequence_id) {
        collect_control_bounds(
            trait_ref,
            &sequence.sequence,
            &format!("sequence.{sequence_id}.sequence"),
            bounds,
            seen,
        );
    }
    // Bounds are path-scoped: a shared named sequence is reported once for
    // each referencing path, while `seen` only guards recursion cycles.
    seen.remove(&sequence_id);
}

fn local_sequence_id_for_check(ref_text: Option<&str>) -> Option<String> {
    let parsed = ctx_traits_core::reference::Reference::parse(ref_text?).ok()?;
    (parsed.kind() == ctx_traits_core::reference::Kind::Sequence && !parsed.is_qualified())
        .then(|| parsed.id().to_string())
}

fn collect_sequence_control_warnings(
    trait_ref: &ctx_traits_core::Trait,
    items: &[ctx_traits_core::r#trait::procedure::SequenceItem],
    base: &str,
    warnings: &mut Vec<ctx_traits_core::check::CheckWarning>,
) {
    for (index, item) in items.iter().enumerate() {
        let field = format!("{base}[{index}]");
        let warning = match item.effective_kind() {
            ctx_traits_core::r#trait::procedure::SequenceKind::Sequence => Some((
                "sequence-control.nested-sequence",
                "nested sequence item is runtime-controlled and must not be represented as an enforceable static Markdown step",
            )),
            ctx_traits_core::r#trait::procedure::SequenceKind::Branch => Some((
                "sequence-control.branch",
                "branch selects exactly one named sequence arm in the controlled runtime",
            )),
            ctx_traits_core::r#trait::procedure::SequenceKind::Loop => Some((
                "sequence-control.bounded-loop",
                "bounded loop uses typed guards and max-iterations in the controlled runtime",
            )),
            ctx_traits_core::r#trait::procedure::SequenceKind::ForEach => Some((
                "sequence-control.typed-for-each",
                "for-each iterates a typed list slot in the controlled runtime; no Markdown checklist parsing or implicit aggregation is performed",
            )),
            ctx_traits_core::r#trait::procedure::SequenceKind::Parallel => Some((
                "sequence-control.parallel",
                "parallel declares independent named-sequence branches; execution is deterministic sequential barrier execution on the single cursor, true concurrency remains deferred",
            )),
            ctx_traits_core::r#trait::procedure::SequenceKind::Prompt
            | ctx_traits_core::r#trait::procedure::SequenceKind::Ask
            | ctx_traits_core::r#trait::procedure::SequenceKind::Command
            | ctx_traits_core::r#trait::procedure::SequenceKind::Check
            | ctx_traits_core::r#trait::procedure::SequenceKind::Project
            | ctx_traits_core::r#trait::procedure::SequenceKind::Terminal => None,
        };
        if let Some((code, message)) = warning {
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::Sequence,
                code: code.to_string(),
                field: Some(field.clone()),
                message: message.to_string(),
            });
        }
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::Loop
            && item.max_iterations.is_none()
            && item.max_iterations_from.is_none()
        {
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::Sequence,
                code: "sequence-control.loop-unbounded".to_string(),
                field: Some(field.clone()),
                message: "loop has no max-iterations; check receipt marks it unbounded".to_string(),
            });
        }
        if item.effective_kind() == ctx_traits_core::r#trait::procedure::SequenceKind::ForEach
            && item.max_items.is_none()
        {
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::Sequence,
                code: "sequence-control.for-each-limit-omitted".to_string(),
                field: Some(field.clone()),
                message: "for-each has no explicit limit; runtime bound is the finite over-slot item count".to_string(),
            });
        }
        if item.until.is_some() && item.until == item.abort_if {
            warnings.push(ctx_traits_core::check::CheckWarning {
                section: Section::Sequence,
                code: "sequence-control.guard-conflict-static".to_string(),
                field: Some(field.clone()),
                message: "until and abort-if use the same guard; runtime will stop with guard-conflict if it matches".to_string(),
            });
        }
        if let (Some(max_iterations), Some(until)) = (item.max_iterations, item.until.as_ref())
            && guard_iteration_unsatisfiable(
                trait_ref,
                until,
                max_iterations,
                &mut std::collections::BTreeSet::new(),
            )
        {
            warnings.push(ctx_traits_core::check::CheckWarning {
                    section: Section::Sequence,
                    code: "sequence-control.guard-unsatisfiable".to_string(),
                    field: Some(format!("{field}.until")),
                    message: "iteration guards are zero-based; this guard cannot match before max-iterations is exhausted".to_string(),
                });
        }
        if let (Some(max_iterations), Some(abort_if)) =
            (item.max_iterations, item.abort_if.as_ref())
            && guard_iteration_unsatisfiable(
                trait_ref,
                abort_if,
                max_iterations,
                &mut std::collections::BTreeSet::new(),
            )
        {
            warnings.push(ctx_traits_core::check::CheckWarning {
                    section: Section::Sequence,
                    code: "sequence-control.guard-unsatisfiable".to_string(),
                    field: Some(format!("{field}.abort-if")),
                    message: "iteration guards are zero-based; this guard cannot match before max-iterations is exhausted".to_string(),
                });
        }
    }
}

fn guard_iteration_unsatisfiable(
    trait_ref: &ctx_traits_core::Trait,
    guard: &ctx_traits_core::r#trait::GuardExpr,
    max_iterations: usize,
    seen_conditions: &mut std::collections::BTreeSet<String>,
) -> bool {
    match guard {
        ctx_traits_core::r#trait::GuardExpr::Ref(ref_text) => {
            condition_guard_iteration_unsatisfiable(
                trait_ref,
                ref_text,
                max_iterations,
                seen_conditions,
            )
        }
        ctx_traits_core::r#trait::GuardExpr::Any(items) => items.iter().all(|item| {
            guard_iteration_unsatisfiable(trait_ref, item, max_iterations, seen_conditions)
        }),
        ctx_traits_core::r#trait::GuardExpr::Predicate(predicate) => {
            if predicate.not.is_some() {
                return false;
            }
            predicate
                .iteration
                .is_some_and(|iteration| iteration as usize >= max_iterations)
                || predicate
                    .iteration_at_least
                    .is_some_and(|iteration| iteration as usize >= max_iterations)
                || predicate.condition.as_deref().is_some_and(|condition| {
                    condition_guard_iteration_unsatisfiable(
                        trait_ref,
                        condition,
                        max_iterations,
                        seen_conditions,
                    )
                })
                || (!predicate.all.is_empty()
                    && predicate.all.iter().any(|item| {
                        guard_iteration_unsatisfiable(
                            trait_ref,
                            item,
                            max_iterations,
                            seen_conditions,
                        )
                    }))
                || (!predicate.any.is_empty()
                    && predicate.any.iter().all(|item| {
                        guard_iteration_unsatisfiable(
                            trait_ref,
                            item,
                            max_iterations,
                            seen_conditions,
                        )
                    }))
        }
    }
}

fn condition_guard_iteration_unsatisfiable(
    trait_ref: &ctx_traits_core::Trait,
    ref_text: &str,
    max_iterations: usize,
    seen_conditions: &mut std::collections::BTreeSet<String>,
) -> bool {
    let Ok(parsed) = ctx_traits_core::reference::Reference::parse(ref_text) else {
        return false;
    };
    if parsed.kind() != ctx_traits_core::reference::Kind::Condition || parsed.is_qualified() {
        return false;
    }
    let condition_id = parsed.id().to_string();
    if !seen_conditions.insert(condition_id.clone()) {
        return false;
    }
    let result = trait_ref
        .conditions
        .get(parsed.id())
        .is_some_and(|condition| {
            guard_iteration_unsatisfiable(
                trait_ref,
                &condition.as_guard(),
                max_iterations,
                seen_conditions,
            )
        });
    seen_conditions.remove(&condition_id);
    result
}

fn check_resource_read_warning(
    warning: &ctx_traits_io::resource::ResourceReadWarning,
) -> (String, Option<String>, String) {
    use ctx_traits_io::resource::ResourceReadWarning;
    match warning {
        ResourceReadWarning::MissingFile { resource_id, path } => (
            "resource-missing-file".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} file is missing at {path}"),
        ),
        ResourceReadWarning::SymlinkDetected { resource_id, path } => (
            "resource-symlink-detected".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} path is a symlink at {path}"),
        ),
        ResourceReadWarning::SpecialFile { resource_id, path } => (
            "resource-special-file".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} path is not a regular file at {path}"),
        ),
        ResourceReadWarning::Directory { resource_id, path } => (
            "resource-directory".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!(
                "resource {resource_id} path is a directory at {path}; agents read its files on demand"
            ),
        ),
        ResourceReadWarning::BinaryContent {
            resource_id,
            path,
            byte_size,
        } => (
            "resource-binary-content".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} at {path} is binary-like ({byte_size} bytes)"),
        ),
    }
}

fn check_resource_inclusion_warning(
    warning: &ctx_traits_core::resource_plan::InclusionWarning,
) -> (String, Option<String>, String) {
    use ctx_traits_core::resource_plan::InclusionWarning;
    match warning {
        InclusionWarning::BinaryContent {
            resource_id,
            byte_size,
        } => (
            "resource-binary-content".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} is binary-like ({byte_size} bytes)"),
        ),
        InclusionWarning::SymlinkDetected { resource_id } => (
            "resource-symlink-detected".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!("resource {resource_id} path is a symlink"),
        ),
        InclusionWarning::DependencyResourceUnavailable {
            resource_id,
            status,
        } => (
            "dependency-resource-unavailable".to_string(),
            Some(format!("resource.{resource_id}.path")),
            format!(
                "dependency resource {resource_id} path is unavailable: {}",
                ctx_traits_core::resource_plan::dependency_unavailable_reason(*status)
            ),
        ),
        InclusionWarning::UndeclaredTemplateInput {
            resource_id,
            ref_text,
        } => (
            "undeclared-template-input".to_string(),
            Some(format!("resource.{resource_id}.body:{ref_text}")),
            format!(
                "resource {resource_id} body uses {ref_text} but resource.input does not declare it"
            ),
        ),
        InclusionWarning::UnusedTemplateInput {
            resource_id,
            ref_text,
        } => (
            "unused-template-input".to_string(),
            Some(format!("resource.{resource_id}.input:{ref_text}")),
            format!("resource {resource_id} declares {ref_text} but body evidence does not use it"),
        ),
        InclusionWarning::TemplateDiagnostic {
            resource_id,
            message,
        } => (
            "template-diagnostic".to_string(),
            Some(format!("resource.{resource_id}.body")),
            format!("resource {resource_id} template diagnostic: {message}"),
        ),
    }
}

fn model_view_warning_code(warning: &str) -> String {
    if warning.contains("no built-in or local summary/description") {
        "thin-static-guidance".to_string()
    } else if warning.contains("frontmatter") {
        "frontmatter".to_string()
    } else {
        "model-view-warning".to_string()
    }
}

fn model_view_warning_field(warning: &str) -> Option<String> {
    warning.split_once(' ').and_then(|(field, _)| {
        field
            .contains('.')
            .then(|| field.trim_end_matches(':').to_string())
    })
}

struct CdkDriftCheck {
    summary: String,
    ok: bool,
    provenance: Option<ctx_traits_core::synth::Provenance>,
    warnings: Vec<ctx_traits_core::check::CheckWarning>,
}

/// Compares the rebuilt source-map sidecar against the committed one for
/// packages whose canonical is the `generated/index.toml` v2 shape. Returns
/// `false` (not drifted) when the package has no committed sidecar to
/// compare against, since the map is the CDK build's own artifact rather
/// than a hand-authored file some packages are exempt from producing.
fn committed_source_map_drifted(
    trait_path: &camino::Utf8Path,
    rebuilt_map: &ctx_traits_core::source_map::SourceMap,
) -> crate::Result<bool> {
    let map_path = crate::app::cdk_build::package_source_map(trait_path)?;
    if !map_path.exists() {
        return Ok(false);
    }
    let committed_text = ctx_traits_io::read::read_text(&map_path)?;
    let rebuilt_text = serialized_source_map(rebuilt_map)?;
    Ok(rebuilt_text != committed_text)
}

fn serialized_source_map(
    source_map: &ctx_traits_core::source_map::SourceMap,
) -> crate::Result<String> {
    serde_json::to_string_pretty(source_map)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| crate::Error::json("serialize CDK source map", error))
}

/// Divergence check for the build-owned `package.json`: `None` when the
/// package has no manifest to generate from (nothing to check) or the file
/// on disk byte-matches what `ctx traits build` would emit; `Some(reason)`
/// otherwise. Same policy class as projection-lock drift — a hand edit
/// never passes `check` silently.
fn package_json_drift_failure(package_root: &camino::Utf8Path) -> Option<String> {
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    let manifest_text = ctx_traits_io::read::read_optional_text(&manifest_path).ok()??;
    let manifest =
        ctx_traits_core::manifest::decode_package_manifest(&manifest_text, manifest_path.as_str())
            .ok()??;
    let expected = ctx_traits_io::cdk_build::generated_package_json_text(&manifest);
    let package_json_path = ctx_traits_io::cdk_build::generated_package_json_path(package_root);
    match ctx_traits_io::read::read_optional_text(&package_json_path) {
        Ok(Some(actual)) if actual == expected => None,
        Ok(Some(_)) => Some(format!(
            "{package_json_path} is stale vs {manifest_path} (re-run ctx traits build)"
        )),
        Ok(None) => Some(format!(
            "{package_json_path} is missing (re-run ctx traits build)"
        )),
        Err(error) => Some(format!("{package_json_path} is unreadable: {error}")),
    }
}

/// `[forked-from]` provenance row (0213): present only for a package `ctx
/// traits fork` produced. No drift machinery needed — the fork detaches the
/// vendor lock entry it forked from, so there is nothing left to drift
/// against.
fn forked_from_summary(package_root: &camino::Utf8Path) -> crate::Result<Option<String>> {
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    let Some(manifest_text) = ctx_traits_io::read::read_optional_text(&manifest_path)? else {
        return Ok(None);
    };
    let Some(manifest) =
        ctx_traits_core::manifest::decode_package_manifest(&manifest_text, manifest_path.as_str())?
    else {
        return Ok(None);
    };
    Ok(manifest.forked_from.map(|forked_from| {
        format!(
            "{}@{}, canonical {}",
            forked_from.id, forked_from.version, forked_from.canonical_digest
        )
    }))
}

fn native_family_drift_check(
    trait_path: &camino::Utf8Path,
    source_path: &camino::Utf8Path,
    family: crate::app::cdk_build::CdkFamilySynthOutcome,
) -> CdkDriftCheck {
    let package_root = match ctx_traits_io::layout::package_root_for_manifest(trait_path) {
        Some(root) => root,
        None => {
            return CdkDriftCheck {
                summary: format!(
                    "CDK family drift check could not locate package root for {trait_path}"
                ),
                ok: false,
                provenance: None,
                warnings: Vec::new(),
            };
        }
    };
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    let committed = match ctx_traits_io::family_manifest::read_family_table(&manifest_path) {
        Ok(Some(table)) => table,
        Ok(None) => {
            return CdkDriftCheck {
                summary: format!("{manifest_path} is missing the synthesized [family] topology"),
                ok: false,
                provenance: None,
                warnings: Vec::new(),
            };
        }
        Err(error) => {
            return CdkDriftCheck {
                summary: format!("CDK family drift check could not read {manifest_path}: {error}"),
                ok: false,
                provenance: None,
                warnings: Vec::new(),
            };
        }
    };
    let synthesized_default = family
        .topology
        .as_object()
        .and_then(|topology| topology.get("default"))
        .and_then(serde_json::Value::as_str);
    let synthesized_names = family
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let committed_names = committed
        .variants
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut failures = Vec::new();
    if synthesized_default != Some(committed.default.as_str()) {
        failures.push(format!(
            "default name differs (synthesized={synthesized_default:?}, committed={:?})",
            committed.default
        ));
    }
    if synthesized_names != committed_names {
        failures.push(format!(
            "variant set differs (synthesized={synthesized_names:?}, committed={committed_names:?})"
        ));
    }

    for variant in &family.variants {
        let expected_relative = format!("generated/{}/index.toml", variant.name);
        let expected_aliases =
            crate::app::cdk_build::family_variant_aliases(&family.family_id, &variant.name);
        match committed.variants.get(&variant.name) {
            Some(entry) => {
                if entry.relative_path != expected_relative {
                    failures.push(format!(
                        "variant {} path differs (expected {expected_relative}, committed {})",
                        variant.name, entry.relative_path
                    ));
                }
                if entry.aliases != expected_aliases {
                    failures.push(format!(
                        "variant {} aliases differ (expected {expected_aliases:?}, committed {:?})",
                        variant.name, entry.aliases
                    ));
                }
            }
            None => continue,
        }
        let canonical_path = package_root.join(&expected_relative);
        match ctx_traits_io::read::read_optional_text(&canonical_path) {
            Ok(Some(text)) if text == variant.response.output_text => {}
            Ok(Some(_)) => failures.push(format!(
                "variant {} canonical bytes are stale",
                variant.name
            )),
            Ok(None) => failures.push(format!(
                "variant {} canonical output is missing",
                variant.name
            )),
            Err(error) => failures.push(format!(
                "variant {} canonical output is unreadable: {error}",
                variant.name
            )),
        }
        let map_path = package_root
            .join("generated")
            .join(&variant.name)
            .join(ctx_traits_io::layout::CANONICAL_SOURCE_MAP);
        let rebuilt_map = serialized_source_map(&variant.source_map);
        match (
            ctx_traits_io::read::read_optional_text(&map_path),
            rebuilt_map,
        ) {
            (Ok(Some(committed)), Ok(rebuilt)) if committed == rebuilt => {}
            (Ok(Some(_)), Ok(_)) => {
                failures.push(format!("variant {} source map is stale", variant.name))
            }
            (Ok(None), _) => {
                failures.push(format!("variant {} source map is missing", variant.name))
            }
            (Err(error), _) => failures.push(format!(
                "variant {} source map is unreadable: {error}",
                variant.name
            )),
            (_, Err(error)) => failures.push(format!(
                "variant {} source map could not be serialized: {error}",
                variant.name
            )),
        }
    }

    failures.extend(package_json_drift_failure(package_root));

    let provenance = family
        .variants
        .first()
        .map(|variant| variant.response.provenance.clone());
    CdkDriftCheck {
        summary: if failures.is_empty() {
            format!(
                "{} family variants and source maps byte-match rebuilt {source_path}",
                family.variants.len()
            )
        } else {
            format!(
                "native family is stale vs {source_path} (re-run ctx traits build): {}",
                failures.join("; ")
            )
        },
        ok: failures.is_empty(),
        provenance,
        warnings: Vec::new(),
    }
}

fn cdk_drift_check(
    trait_path: &camino::Utf8Path,
    current_canonical_digest: &ctx_traits_core::digest::Digest,
) -> Option<CdkDriftCheck> {
    let source_path = match crate::app::cdk_build::package_cdk_source(trait_path) {
        Ok(Some(source_path)) => source_path,
        Ok(None) => return None,
        Err(error) => {
            return Some(CdkDriftCheck {
                summary: format!("CDK drift check could not select a source: {error}"),
                ok: false,
                provenance: None,
                warnings: Vec::new(),
            });
        }
    };

    let outcome = match crate::app::cdk_build::synthesize_cdk_source_any(
        &source_path,
        ctx_traits_core::synth::OutputFormat::Toml,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            let summary = cdk_drift_unverified_summary(&source_path, &error);
            let blocking = crate::app::cdk_build::is_import_boundary_error(&error);
            return Some(CdkDriftCheck {
                summary: summary.clone(),
                ok: !blocking,
                provenance: None,
                warnings: vec![ctx_traits_core::check::CheckWarning {
                    section: Section::CdkDrift,
                    code: if blocking {
                        "cdk-import-boundary".to_string()
                    } else {
                        "cdk-drift-unverified".to_string()
                    },
                    field: Some(source_path.to_string()),
                    message: summary,
                }],
            });
        }
    };

    let (response, source_map, diagnostics) = match outcome {
        crate::app::cdk_build::CdkSynthResult::Single(outcome) => (
            outcome.response,
            outcome.source_map,
            outcome.authoring_diagnostics,
        ),
        crate::app::cdk_build::CdkSynthResult::Family(family) => {
            return Some(native_family_drift_check(trait_path, &source_path, *family));
        }
    };

    let mut authoring_warnings = diagnostics
        .iter()
        .map(|diagnostic| ctx_traits_core::check::CheckWarning {
            section: Section::CdkAuthoring,
            code: diagnostic.code.clone(),
            field: Some(diagnostic.field_path.clone()),
            message: diagnostic.message.clone(),
        })
        .collect::<Vec<_>>();

    let expected = response.provenance.canonical_digest.as_str();
    let actual = current_canonical_digest.as_str();
    let committed_text = match ctx_traits_io::read::read_text(trait_path) {
        Ok(text) => text,
        Err(error) => {
            return Some(CdkDriftCheck {
                summary: format!("CDK drift check could not read {trait_path}: {error}"),
                ok: false,
                provenance: None,
                warnings: Vec::new(),
            });
        }
    };
    let byte_stable = matches!(trait_path.file_name(), Some("trait.toml" | "index.toml"));
    let map_drifted = byte_stable
        && match committed_source_map_drifted(trait_path, &source_map) {
            Ok(drifted) => drifted,
            Err(error) => {
                return Some(CdkDriftCheck {
                    summary: format!(
                        "CDK drift check could not compare source map for {trait_path}: {error}"
                    ),
                    ok: false,
                    provenance: None,
                    warnings: Vec::new(),
                });
            }
        };
    let package_json_drift = ctx_traits_io::layout::package_root_for_manifest(trait_path)
        .and_then(package_json_drift_failure);
    let ok = expected == actual
        && (!byte_stable || response.output_text == committed_text)
        && !map_drifted
        && package_json_drift.is_none();
    authoring_warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then(left.field.cmp(&right.field))
            .then(left.message.cmp(&right.message))
    });
    Some(CdkDriftCheck {
        summary: if ok {
            if byte_stable {
                format!(
                    "{trait_path} byte-matches rebuilt {source_path}; canonical digest {actual}"
                )
            } else {
                format!("{trait_path} matches rebuilt {source_path} canonical digest {actual}")
            }
        } else if map_drifted && expected == actual && response.output_text == committed_text {
            format!(
                "source map for {trait_path} is stale vs rebuilt {source_path} (re-run ctx traits build)"
            )
        } else if let Some(reason) = package_json_drift.filter(|_| {
            expected == actual
                && (!byte_stable || response.output_text == committed_text)
                && !map_drifted
        }) {
            reason
        } else {
            format!(
                "trait.toml is stale vs {} (re-run ctx traits build): rebuilt bytes or canonical digest differ (expected {}, actual {})",
                source_path.file_name().unwrap_or(source_path.as_str()),
                expected,
                actual,
            )
        },
        ok,
        provenance: Some(response.provenance),
        warnings: authoring_warnings,
    })
}

fn cdk_drift_unverified_summary(source_path: &camino::Utf8Path, error: &crate::Error) -> String {
    if cdk_runtime_unavailable(error) {
        format!(
            "drift unverified: runtime unavailable for {source_path}; required CDK runtime executable was not found or is not executable on PATH"
        )
    } else {
        format!("drift unverified: CDK rebuild failed for {source_path}: {error}")
    }
}

fn cdk_runtime_unavailable(error: &crate::Error) -> bool {
    let crate::Error::Io(error) = error else {
        return false;
    };
    let ctx_traits_io::Error::Environment(ctx_traits_io::environment::Error::Filesystem {
        path,
        source,
    }) = error.as_ref()
    else {
        return false;
    };
    matches!(
        source.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) && cdk_runtime_error_path(path)
}

fn cdk_runtime_error_path(path: &str) -> bool {
    path == "node" || path.starts_with("node ")
}

fn emit_check_report(
    report: &ctx_traits_core::check::CheckReport,
    locked: bool,
    json: bool,
    file: &str,
    disable_presentation: bool,
    verbose: bool,
) -> crate::Result<CommandOutput<()>> {
    use crate::app::presentation::{
        HumanOutputMode, OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
    };

    match OutputMode::select(json, verbose) {
        OutputMode::Json => {
            crate::app::command_handlers::print_json_report(report, "check report")?;
        }
        OutputMode::Human(mode) => {
            let status = if report.passed {
                PanelStatus::Passed("passed".to_string())
            } else {
                PanelStatus::Blocked("blocked".to_string())
            };
            let mut panel = Panel::new("ctx", "check", status)
                .row(PanelRow::toned("trait", &report.trait_id, RowTone::Default))
                .row(PanelRow::toned(
                    "valid",
                    report.valid.to_string(),
                    if report.valid {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                ))
                .row(PanelRow::toned(
                    "audit",
                    if report.audit_findings.is_empty() {
                        "none".to_string()
                    } else {
                        format!("{} finding(s)", report.audit_findings.len())
                    },
                    if report.audit_findings.is_empty() {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                ))
                .row(PanelRow::toned(
                    "warnings",
                    if report.warnings.is_empty() {
                        "none".to_string()
                    } else {
                        format!("{}", report.warnings.len())
                    },
                    RowTone::Default,
                ));
            if locked {
                panel = panel.row(PanelRow::toned(
                    "drift",
                    format!("{} comparison(s)", report.drift.len()),
                    RowTone::Default,
                ));
            }
            if mode == HumanOutputMode::Compact {
                panel = panel.next(PanelRow::toned(
                    "next",
                    "ctx traits check --verbose for the full report",
                    RowTone::Default,
                ));
            }
            emit_human(disable_presentation, &panel, mode, || {
                crate::app::tui::emit_report(
                    disable_presentation,
                    || styled_check_lines(report, locked, file, verbose),
                    || emit_plain_check_report(report, locked),
                )
            })?;
        }
    }
    if !report.passed {
        return Err(crate::Error::AlreadyReported {
            message: format!("check failed for trait {}", report.trait_id),
            exit_code: crate::app::error::EXIT_FINDINGS,
        });
    }
    Ok(CommandOutput::new(()))
}

fn emit_plain_check_report(
    report: &ctx_traits_core::check::CheckReport,
    locked: bool,
) -> crate::Result<()> {
    use crate::app::tui::write_plain_line as w;
    w("ctx traits check")?;
    w(format!("  trait: {}", report.trait_id))?;
    w(format!("  valid: {}", report.valid))?;
    if let Some(ref err) = report.validation_error {
        w(format!("  error: {err}"))?;
    }
    if report.audit_findings.is_empty() {
        w("  audit: none")?;
    } else {
        w(format!(
            "  audit ({} findings):",
            report.audit_findings.len()
        ))?;
        for f in &report.audit_findings {
            print_audit_finding("    ", f);
        }
    }
    if report.sections.is_empty() {
        w("  sections: none")?;
    } else {
        w("  sections:")?;
        for section in &report.sections {
            w(format!(
                "    {}: {} ({})",
                section.name,
                section.summary,
                if section.ok { "ok" } else { "blocked" }
            ))?;
        }
    }
    if report.warnings.is_empty() {
        w("  warnings: none")?;
    } else {
        w("  warnings:")?;
        for warning in &report.warnings {
            w(format!(
                "    [{}] {}{}: {}",
                warning.section,
                warning.code,
                warning
                    .field
                    .as_ref()
                    .map(|field| format!(" ({field})"))
                    .unwrap_or_default(),
                warning.message
            ))?;
        }
    }
    if locked {
        w(format!(
            "  locked-mode: {} drift comparison(s)",
            report.drift.len()
        ))?;
        for drift in &report.drift {
            w(format!(
                "    {}: {}",
                report_drift::layer_label(&drift.layer),
                drift.summary
            ))?;
            w(format!("      expected: {}", drift.expected))?;
            w(format!(
                "      actual: {}",
                drift.actual.as_deref().unwrap_or("none")
            ))?;
            w(format!("      unsupported: {}", drift.unsupported))?;
        }
    }
    if report.unsupported_capabilities.is_empty() {
        w("  unsupported-capabilities: none")?;
    } else {
        w("  unsupported-capabilities:")?;
        for capability in &report.unsupported_capabilities {
            w(format!("    {capability}"))?;
        }
    }
    if report.synth_provenance.is_empty() {
        w("  synth-provenance: none")?;
    } else {
        w("  synth-provenance:")?;
        for provenance in &report.synth_provenance {
            print_synth_provenance("    ", provenance);
        }
    }
    w(format!("  passed: {}", report.passed))?;
    Ok(())
}

use crate::app::tui::{Line, Tone};

/// `pub(crate)` (rather than the module-private default every other
/// `styled_*` helper here uses) so the trait editor's `c` action (P244 §4.1)
/// can render a `check` report into its detail pane through the same
/// `Line`/`Tone` → `render_line` bridge `run` already renders it through,
/// instead of re-deriving a second summary of the same [`CheckReport`].
pub(crate) fn styled_check_lines(
    report: &ctx_traits_core::check::CheckReport,
    locked: bool,
    file: &str,
    verbose: bool,
) -> Vec<Line> {
    let mut lines = vec![check_command_line(file, locked), Line::blank()];
    push_grouped_report(&mut lines, report, locked, verbose);
    lines
}

// ---- grouped report: Failed / Passed / Flagged ----------------------------

const BULLET: &str = "∙ ";

/// Global cap on how many flagged advisories list before the tail collapses to
/// a "... N more" line; `--verbose` lifts it. Model-view alone can carry a dozen
/// thin-guidance advisories.
const FLAG_LIMIT: usize = 5;

fn push_grouped_report(
    lines: &mut Vec<Line>,
    report: &ctx_traits_core::check::CheckReport,
    locked: bool,
    verbose: bool,
) {
    let gates = build_grouped_gates(report, locked);

    let passed_label_width = gates
        .iter()
        .filter(|gate| gate.outcome == Outcome::Passed)
        .map(|gate| gate.name.chars().count())
        .max()
        .unwrap_or(12);

    let failed: Vec<&Gate> = gates
        .iter()
        .filter(|g| g.outcome == Outcome::Failed)
        .collect();
    let passed: Vec<&Gate> = gates
        .iter()
        .filter(|g| g.outcome == Outcome::Passed)
        .collect();
    let flagged: Vec<&Gate> = gates
        .iter()
        .filter(|g| g.outcome == Outcome::Flagged)
        .collect();

    let tally = |group: &[&Gate], marker: &str| -> usize {
        group
            .iter()
            .flat_map(|gate| &gate.issues)
            .filter(|issue| issue.marker == marker)
            .count()
    };
    let critical = tally(&failed, "(critical)");
    let minor = failed.iter().map(|gate| gate.issues.len()).sum::<usize>() - critical;
    let advisory = tally(&flagged, "(advisory)");
    let at_risk = tally(&flagged, "(at risk)");

    push_issue_group(lines, &failed, "Failed", Tone::Fail, None);
    push_passed_group(lines, &passed, passed_label_width);
    push_issue_group(
        lines,
        &flagged,
        "Flagged",
        Tone::Warn,
        (!verbose).then_some(FLAG_LIMIT),
    );

    push_verdict(lines, report, critical, minor, advisory, at_risk);
}

/// Render the Failed or Flagged group. Each gate name heads its own dim line;
/// its issues bullet below, dimmed, split on the first ` · ` so the inner
/// separator and the trailing severity marker each form an aligned column.
/// Alignment is scoped to a single gate, so a long value under one gate never
/// pads the columns of another. A long value wraps at its `;` clause breaks: the
/// first clause stays inline (carrying the marker column) and the rest sit on
/// their own lines indented under the bullet text. When `cap` is set (Flagged,
/// non-verbose) only the first `cap` issues across the whole group show and the
/// remainder collapses into one "... N more" line.
fn push_issue_group(
    lines: &mut Vec<Line>,
    gates: &[&Gate],
    label: &str,
    header_tone: Tone,
    cap: Option<usize>,
) {
    if gates.is_empty() {
        return;
    }
    let mut header = Line::blank();
    header.push(format!("── {label} ──"), header_tone);
    lines.push(header);

    let total: usize = gates.iter().map(|gate| gate.issues.len()).sum();
    let mut shown = 0usize;
    for gate in gates {
        if cap.is_some_and(|limit| shown >= limit) {
            break;
        }
        let mut name = Line::blank();
        name.push(gate.name.clone(), Tone::Muted);
        lines.push(name);

        // Columns are measured over THIS gate's issues only, on the inline first
        // clause; wrapped clauses render full-width and do not count toward it.
        let parts: Vec<(&Issue, String, Vec<String>, Option<String>)> = gate
            .issues
            .iter()
            .map(|issue| {
                let (first, rest, tail) = issue_parts(&issue.text);
                (issue, first, rest, tail)
            })
            .collect();
        let head_width = parts
            .iter()
            .map(|(_, first, _, _)| first.chars().count())
            .max()
            .unwrap_or(0);
        let content_width = head_width
            + parts
                .iter()
                .map(|(_, _, _, tail)| tail.as_ref().map_or(0, |tail| 3 + tail.chars().count()))
                .max()
                .unwrap_or(0);

        for (issue, first, rest, tail) in &parts {
            if cap.is_some_and(|limit| shown >= limit) {
                break;
            }
            // A head with a tail is a field name (dim); a tail — or a lone head —
            // is the value, kept in main text colour like the Passed summaries.
            let head_tone = if tail.is_some() {
                Tone::Muted
            } else {
                Tone::Default
            };
            let mut line = Line::blank();
            let mut used = head_width;
            line.push(BULLET, Tone::Muted);
            line.push(
                format!(
                    "{first}{}",
                    " ".repeat(head_width.saturating_sub(first.chars().count()))
                ),
                head_tone,
            );
            if let Some(tail) = tail {
                line.push(" · ", Tone::Muted);
                used += 3 + tail.chars().count();
                line.push(tail.clone(), Tone::Default);
            }
            line.push(" ".repeat(content_width.saturating_sub(used)), Tone::Muted);
            line.push(" · ", Tone::Muted);
            line.push(issue.marker, issue.tone);
            lines.push(line);
            for clause in rest {
                let mut cont = Line::blank();
                cont.push(" ".repeat(BULLET.chars().count()), Tone::Muted);
                cont.push(clause.clone(), head_tone);
                lines.push(cont);
            }
            shown += 1;
        }
    }
    if total > shown {
        let mut more = Line::blank();
        more.push(
            format!("... {} more ∙ to see them, use ", total - shown),
            Tone::Muted,
        );
        more.push("--verbose", Tone::Bold);
        more.push(" flag", Tone::Muted);
        lines.push(more);
    }
    lines.push(Line::blank());
}

/// Split an issue's text into its aligned columns and wrapped clauses: the
/// value/field before the first ` · `, that value's `;`-separated clauses (the
/// first shown inline, the rest wrapped), and the optional detail after ` · `.
fn issue_parts(text: &str) -> (String, Vec<String>, Option<String>) {
    let (head, tail) = match text.split_once(" · ") {
        Some((head, tail)) => (head, Some(tail.to_string())),
        None => (text, None),
    };
    let mut clauses = head.split(';').map(|clause| clause.trim().to_string());
    let first = clauses.next().unwrap_or_default();
    let rest = clauses.filter(|clause| !clause.is_empty()).collect();
    (first, rest, tail)
}

fn push_passed_group(lines: &mut Vec<Line>, gates: &[&Gate], label_width: usize) {
    if gates.is_empty() {
        return;
    }
    let mut header = Line::blank();
    header.push("── Passed ──", Tone::Pass);
    lines.push(header);
    for gate in gates {
        let mut line = Line::blank();
        let pad = label_width.saturating_sub(gate.name.chars().count());
        line.push(format!("{}{}", gate.name, " ".repeat(pad)), Tone::Muted);
        line.push(" · ", Tone::Muted);
        line.push(gate.summary.clone(), Tone::Default);
        lines.push(line);
    }
    lines.push(Line::blank());
}

/// A row of the verdict summary. `aligned` rows carry severity counts whose `∙`
/// columns line up; a non-aligned row (passed status) holds one free-text cell.
struct VerdictRow {
    label: String,
    label_tone: Tone,
    cells: Vec<(String, Tone)>,
    aligned: bool,
}

fn push_verdict(
    lines: &mut Vec<Line>,
    report: &ctx_traits_core::check::CheckReport,
    critical: usize,
    minor: usize,
    advisory: usize,
    at_risk: usize,
) {
    // Clean single line when the trait passes with nothing flagged.
    if report.passed && advisory + at_risk == 0 {
        let mut line = Line::blank();
        line.push("∴ Passed", Tone::Pass);
        line.push(" ∙ ", Tone::Muted);
        line.push("trait is ready for use.", Tone::Default);
        lines.push(line);
        lines.push(Line::blank());
        return;
    }

    let mut rows: Vec<VerdictRow> = Vec::new();
    if report.passed {
        rows.push(VerdictRow {
            label: "∴ Passed".to_string(),
            label_tone: Tone::Pass,
            cells: vec![("ready for use".to_string(), Tone::Default)],
            aligned: false,
        });
    } else {
        let mut cells = Vec::new();
        if critical > 0 {
            cells.push((format!("{critical} critical"), Tone::Fail));
        }
        if minor > 0 {
            cells.push((format!("{minor} minor"), Tone::Fail));
        }
        if cells.is_empty() {
            cells.push(("check failed".to_string(), Tone::Fail));
        }
        rows.push(VerdictRow {
            label: "∴ Failed".to_string(),
            label_tone: Tone::Fail,
            cells,
            aligned: true,
        });
    }
    if advisory + at_risk > 0 {
        let mut cells = Vec::new();
        if advisory > 0 {
            cells.push((format!("{advisory} advisory"), Tone::Warn));
        }
        if at_risk > 0 {
            cells.push((format!("{at_risk} at risk"), Tone::Warn));
        }
        rows.push(VerdictRow {
            label: "  Flagged".to_string(),
            label_tone: Tone::Warn,
            cells,
            aligned: true,
        });
    }

    let label_width = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    let cell_count = rows
        .iter()
        .filter(|row| row.aligned)
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0);
    let mut widths = vec![0usize; cell_count];
    for row in rows.iter().filter(|row| row.aligned) {
        for (index, (text, _)) in row.cells.iter().enumerate() {
            widths[index] = widths[index].max("∙ ".chars().count() + text.chars().count());
        }
    }

    for row in &rows {
        let mut line = Line::blank();
        let pad = label_width.saturating_sub(row.label.chars().count());
        line.push(format!("{}{}", row.label, " ".repeat(pad)), row.label_tone);
        for (index, (text, tone)) in row.cells.iter().enumerate() {
            line.push(" ∙ ", Tone::Muted);
            line.push(text.clone(), *tone);
            if row.aligned && index + 1 < row.cells.len() {
                let used = "∙ ".chars().count() + text.chars().count();
                line.push(
                    " ".repeat(widths[index].saturating_sub(used)),
                    Tone::Default,
                );
            }
        }
        lines.push(line);
    }
    lines.push(Line::blank());
}

// ---- gate model + grouping ------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Failed,
    Passed,
    Flagged,
}

struct Issue {
    text: String,
    marker: &'static str,
    tone: Tone,
}

struct Gate {
    name: String,
    summary: String,
    outcome: Outcome,
    issues: Vec<Issue>,
}

fn build_grouped_gates(report: &ctx_traits_core::check::CheckReport, locked: bool) -> Vec<Gate> {
    let mut gates = Vec::new();
    for section in &report.sections {
        let mut blocking = Vec::new();
        let mut advisories = Vec::new();

        if section.name == "validation"
            && let Some(error) = report.validation_error.as_deref()
        {
            blocking.push(Issue {
                text: error.to_string(),
                marker: "(error)",
                tone: Tone::Fail,
            });
        }
        if section.name == Section::HiddenContentAudit.as_str() {
            for finding in &report.audit_findings {
                match finding.severity {
                    ctx_traits_core::audit::Severity::Critical => blocking.push(Issue {
                        text: finding.message.clone(),
                        marker: "(critical)",
                        tone: Tone::Fail,
                    }),
                    ctx_traits_core::audit::Severity::Warning => advisories.push(Issue {
                        text: shorten_advisory(&finding.message),
                        marker: "(at risk)",
                        tone: Tone::Warn,
                    }),
                    ctx_traits_core::audit::Severity::Advisory => advisories.push(Issue {
                        text: shorten_advisory(&finding.message),
                        marker: "(advisory)",
                        tone: Tone::Warn,
                    }),
                }
            }
        }
        for warning in report
            .warnings
            .iter()
            .filter(|w| w.section.as_str() == section.name)
        {
            advisories.push(Issue {
                text: shorten_advisory(&warning.message),
                marker: "(advisory)",
                tone: Tone::Warn,
            });
        }

        let name = gate_display_name(&section.name).to_string();
        let summary = humanize_summary(&section.name, &section.summary);

        let (outcome, issues) = if !section.ok {
            let issues = if blocking.is_empty() {
                vec![Issue {
                    text: summary.clone(),
                    marker: "(blocked)",
                    tone: Tone::Fail,
                }]
            } else {
                blocking
            };
            (Outcome::Failed, issues)
        } else if advisories.is_empty() {
            (Outcome::Passed, Vec::new())
        } else {
            (Outcome::Flagged, advisories)
        };
        gates.push(Gate {
            name,
            summary,
            outcome,
            issues,
        });
    }

    // Safety net: a vocabulary section can legitimately go unregistered in a
    // given report (e.g. `cdk-authoring` outside a CDK block). A warning
    // whose label matches no registered section still shows here, under its
    // own label, instead of being dropped.
    let registered: std::collections::BTreeSet<&str> = report
        .sections
        .iter()
        .map(|section| section.name.as_str())
        .collect();
    let mut unmatched: std::collections::BTreeMap<
        &str,
        Vec<&ctx_traits_core::check::CheckWarning>,
    > = std::collections::BTreeMap::new();
    for warning in &report.warnings {
        if !registered.contains(warning.section.as_str()) {
            unmatched
                .entry(warning.section.as_str())
                .or_default()
                .push(warning);
        }
    }
    for (label, warnings) in unmatched {
        let issues = warnings
            .iter()
            .map(|warning| Issue {
                text: shorten_advisory(&warning.message),
                marker: "(advisory)",
                tone: Tone::Warn,
            })
            .collect();
        gates.push(Gate {
            name: gate_display_name(label).to_string(),
            summary: String::new(),
            outcome: Outcome::Flagged,
            issues,
        });
    }

    if locked {
        let blocking = report
            .drift
            .iter()
            .filter(|drift| !drift.unsupported && drift.actual.as_deref() != Some(&drift.expected))
            .count();
        if blocking == 0 {
            gates.push(Gate {
                name: "locked-mode".to_string(),
                summary: "no drift".to_string(),
                outcome: Outcome::Passed,
                issues: Vec::new(),
            });
        } else {
            gates.push(Gate {
                name: "locked-mode".to_string(),
                summary: String::new(),
                outcome: Outcome::Failed,
                issues: vec![Issue {
                    text: plural(blocking as u64, "drift comparison"),
                    marker: "(blocked)",
                    tone: Tone::Fail,
                }],
            });
        }
    }
    gates
}

/// Presentation-only: shorten the verbose static-guidance advisory phrasing so
/// the Flagged list stays scannable (`… has no built-in or local summary/
/// description for static render` → `… · no static description`).
fn shorten_advisory(message: &str) -> String {
    const RULES: &[(&str, &str)] = &[
        (
            " has no built-in or local summary/description for static render",
            " · no static description",
        ),
        (
            " has no rules for static trigger guidance",
            " · no static trigger rules",
        ),
        (
            " output contract has no format guidance",
            " · no format guidance",
        ),
        (" output contract has no title", " · no title"),
    ];
    for (suffix, replacement) in RULES {
        if let Some(head) = message.strip_suffix(suffix) {
            return format!("{head}{replacement}");
        }
    }
    message.to_string()
}

fn gate_display_name(name: &str) -> &str {
    name.strip_suffix("-audit").unwrap_or(name)
}

/// Presentation-only: turn a machine gate summary into a short, human-readable
/// phrase (digests shortened, `key=value` noise dropped). Never changes report
/// content — only the styled TTY view uses it; `--json` is untouched.
fn humanize_summary(name: &str, summary: &str) -> String {
    match name {
        "validation" => summary
            .strip_prefix("trait ")
            .unwrap_or(summary)
            .to_string(),
        "lifecycle-status" | "declared-trust" | "lifecycle-trust" => kv_values(summary),
        "activation" => summary
            .split(" (")
            .next()
            .unwrap_or(summary)
            .trim()
            .to_string(),
        "dependencies" => "no dependency manifest".to_string(),
        "resources" => humanize_resources(summary),
        "render-readiness" | "scenario-eval-audit" => humanize_warnings(summary),
        "hidden-content-audit" => humanize_finding_count(summary),
        "import-evidence" => "no prior import".to_string(),
        "runtime-evidence" => summary
            .split(';')
            .next()
            .unwrap_or(summary)
            .trim()
            .to_string(),
        "eval-evidence" => humanize_evals(summary),
        "model-view" => humanize_model_view(summary),
        "projection-lock" => "no projection lock".to_string(),
        "cdk-drift" => {
            if summary.contains(" matches ") {
                "matches source".to_string()
            } else {
                shorten_digests(summary)
            }
        }
        "cdk-authoring" => humanize_authoring_count(summary),
        _ => shorten_digests(summary),
    }
}

fn kv_values(summary: &str) -> String {
    // `·` is reserved for row structure, so compound values join with `|`.
    summary
        .split(", ")
        .map(|pair| pair.split_once('=').map_or(pair, |(_, value)| value))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn humanize_warnings(summary: &str) -> String {
    plural(sum_warning_counts(summary), "warning")
}

fn humanize_finding_count(summary: &str) -> String {
    let count = summary
        .split_whitespace()
        .next()
        .and_then(|token| token.parse::<u64>().ok())
        .unwrap_or(0);
    plural(count, "finding")
}

fn humanize_authoring_count(summary: &str) -> String {
    let count = summary
        .split_whitespace()
        .next()
        .and_then(|token| token.parse::<u64>().ok())
        .unwrap_or(0);
    plural(count, "authoring diagnostic")
}

fn humanize_evals(summary: &str) -> String {
    let declared = kv_number(summary, "declared").unwrap_or(0);
    if declared == 0 {
        return "no evals declared".to_string();
    }
    let passing = kv_number(summary, "passing-results").unwrap_or(0);
    format!("{passing}/{declared} evals passing")
}

fn humanize_resources(summary: &str) -> String {
    let tokens = summary
        .split('~')
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or("?");
    format!("~{tokens} tokens")
}

fn humanize_model_view(summary: &str) -> String {
    let sections = plural(kv_number(summary, "sections").unwrap_or(0), "section");
    match kv_value(summary, "digest").map(shorten_hex) {
        Some(digest) if !digest.is_empty() => format!("{digest} ({sections})"),
        _ => sections,
    }
}

fn sum_warning_counts(summary: &str) -> u64 {
    summary
        .split(", ")
        .filter(|pair| pair.contains("warnings="))
        .filter_map(|pair| pair.split_once('='))
        .filter_map(|(_, value)| value.trim().parse::<u64>().ok())
        .sum()
}

fn kv_number(summary: &str, key: &str) -> Option<u64> {
    kv_value(summary, key).and_then(|value| value.parse().ok())
}

fn kv_value<'a>(summary: &'a str, key: &str) -> Option<&'a str> {
    summary.split(", ").find_map(|pair| {
        let (found, value) = pair.split_once('=')?;
        (found.trim() == key).then_some(value.trim())
    })
}

fn shorten_hex(value: &str) -> String {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    if hex.len() >= 8 && hex.as_bytes()[..8].iter().all(u8::is_ascii_hexdigit) {
        hex[..8].to_string()
    } else {
        value.to_string()
    }
}

fn shorten_digests(summary: &str) -> String {
    summary
        .split_whitespace()
        .map(|token| match token.split_once('=') {
            Some((key, value)) => format!("{key}={}", shorten_hex(value)),
            None => shorten_hex(token),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn plural(count: u64, noun: &str) -> String {
    match count {
        0 => format!("no {noun}s"),
        1 => format!("1 {noun}"),
        other => format!("{other} {noun}s"),
    }
}

fn check_command_line(file: &str, locked: bool) -> crate::app::tui::Line {
    let mut command = format!("ctx traits check --file {file}");
    if locked {
        command.push_str(" --locked");
    }
    crate::app::tui::command_line(command)
}

pub(crate) fn print_audit_finding(prefix: &str, finding: &ctx_traits_core::audit::Finding) {
    // Broken-pipe-safe: route through write_plain_line and drop the result
    // (matching println!'s fire-and-forget semantics without its panic).
    let _ = crate::app::tui::write_plain_line(format!(
        "{prefix}[{:?}] {:?}: {}",
        finding.severity, finding.code, finding.message
    ));
    let _ = crate::app::tui::write_plain_line(format!(
        "{prefix}  path: {}",
        finding.path.as_deref().unwrap_or("none")
    ));
    let _ = crate::app::tui::write_plain_line(format!(
        "{prefix}  line: {}",
        finding
            .line
            .map_or("none".to_string(), |line| line.to_string())
    ));
    let _ = crate::app::tui::write_plain_line(format!(
        "{prefix}  byte-offset: {}",
        finding
            .byte_offset
            .map_or("none".to_string(), |offset| offset.to_string())
    ));
    let _ = crate::app::tui::write_plain_line(format!(
        "{prefix}  remediation: {}",
        finding.remediation
    ));
}

#[cfg(test)]
mod grouped_gates_tests {
    use super::*;
    use ctx_traits_core::check::{CheckReport, CheckWarning};

    fn warning(section: Section, code: &str) -> CheckWarning {
        CheckWarning {
            section,
            code: code.to_string(),
            field: None,
            message: format!("{code} advisory message"),
        }
    }

    fn count_issue_lines(lines: &[Line]) -> usize {
        lines
            .iter()
            .filter(|line| {
                line.segments()
                    .next()
                    .is_some_and(|(text, _)| text == BULLET)
            })
            .count()
    }

    #[test]
    fn every_warning_in_a_registered_section_renders_under_verbose() {
        let warnings = vec![
            warning(Section::Resources, "resource-1"),
            warning(Section::Resources, "resource-2"),
            warning(Section::ModelView, "model-view-1"),
            warning(Section::RunConfig, "run-config-1"),
        ];
        let report = CheckReport::new("t", true)
            .with_section(Section::Resources, "resources ok", true)
            .with_section(Section::ModelView, "model-view ok", true)
            .with_section(Section::RunConfig, "run-config ok", true)
            .with_warnings(warnings.clone());

        let lines = styled_check_lines(&report, false, "t.toml", true);
        assert_eq!(count_issue_lines(&lines), warnings.len());
    }

    #[test]
    fn warning_under_unregistered_vocabulary_section_still_renders() {
        let warnings = vec![warning(Section::CdkAuthoring, "cdk-authoring-1")];
        let report = CheckReport::new("t", true)
            .with_section(Section::Validation, "trait decoded successfully", true)
            .with_warnings(warnings.clone());

        let lines = styled_check_lines(&report, false, "t.toml", true);
        assert_eq!(count_issue_lines(&lines), warnings.len());
    }

    #[test]
    fn non_verbose_shown_plus_tail_equals_total() {
        let warnings: Vec<_> = (0..8)
            .map(|index| warning(Section::Resources, &format!("resource-{index}")))
            .collect();
        let report = CheckReport::new("t", true)
            .with_section(Section::Resources, "resources ok", true)
            .with_warnings(warnings.clone());

        let lines = styled_check_lines(&report, false, "t.toml", false);
        let shown = count_issue_lines(&lines);
        let tail: usize = lines
            .iter()
            .find_map(|line| {
                let text: String = line.segments().map(|(text, _)| text).collect();
                text.strip_prefix("... ")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|count| count.parse::<usize>().ok())
            })
            .unwrap_or(0);
        assert_eq!(shown + tail, warnings.len());
    }
}
