//! Compose command handler.

use crate::app::entry::{
    apply_assist_check_drift, attach_assist_check_report, print_assist_candidate,
    safe_assist_context_text, to_json_value, trait_package_output_paths,
};
use ctx_traits_core::response::CommandOutput;

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ComposeConflictParticipant {
    source_index: usize,
    trait_id: String,
    field_path: String,
    value: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ComposeConflictRecord {
    kind: String,
    field_path: String,
    description: String,
    participants: Vec<ComposeConflictParticipant>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ComposeWarningRecord {
    code: String,
    kind: String,
    trait_ids: Vec<String>,
    message: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CompositionEvidenceReport {
    plan_result: String,
    source_trait_ids: Vec<String>,
    source_paths: Vec<String>,
    source_digests: Vec<String>,
    conflict_count: usize,
    conflict_fields: Vec<String>,
    conflicts: Vec<ComposeConflictRecord>,
    warning_count: usize,
    warning_codes: Vec<String>,
    warnings: Vec<ComposeWarningRecord>,
    binding_proposal_count: usize,
    port_compatibility_count: usize,
    context_digest: String,
    statement: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct CompositionAssistContext {
    composition_plan: serde_json::Value,
    context_digest: String,
    profile: String,
    target_schema: &'static str,
}

pub(crate) fn handle_compose(
    ids: &[String],
    profile: Option<&str>,
    out: Option<&str>,
    model: Option<&str>,
    candidate_path: Option<&str>,
    check: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let mut traits = Vec::new();
    let mut source_digests = Vec::new();
    let mut source_paths = Vec::new();
    for path_str in ids {
        let path = if camino::Utf8Path::new(path_str).extension().is_none() {
            ctx_traits_io::run::resolve_trait_path(None, Some(path_str), "compose")?.0
        } else {
            camino::Utf8PathBuf::from(path_str)
        };
        let encoding = ctx_traits_core::encoding::Encoding::from_path(&path)?;
        let text = ctx_traits_io::read::read_text(&path)?;
        source_digests.push(ctx_traits_core::digest::Digest::source(&text));
        let (decoded, decode_warnings) =
            ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &text)?;
        ctx_traits_io::decode_diagnostics::print_decode_warnings(path.as_str(), &decode_warnings);
        traits.push(decoded);
        source_paths.push(path.to_string());
    }

    let plan = ctx_traits_core::r#trait::composition::plan(&traits);
    let context = ctx_traits_core::r#trait::composition::Context::from_plan(&plan, &source_digests);
    let context_json = serde_json::to_string(&context)
        .map_err(|e| crate::Error::json("serialize composition context", e))?;
    let context_digest = ctx_traits_core::digest::Digest::canonical(&context_json);

    let trait_ids: Vec<String> = traits.iter().map(|t| t.id.as_str().to_string()).collect();
    let digest_strs: Vec<String> = source_digests
        .iter()
        .map(|d| d.as_str().to_string())
        .collect();

    let plan_result_str = match plan.result {
        ctx_traits_core::r#trait::composition::PlanResult::NoConflicts => "no-conflicts",
        ctx_traits_core::r#trait::composition::PlanResult::NeedsDecision => "needs-decision",
        ctx_traits_core::r#trait::composition::PlanResult::WarnAndIsolate => "warn-and-isolate",
        ctx_traits_core::r#trait::composition::PlanResult::Failed => "failed",
    };
    let plan_blocks_write = matches!(
        plan.result,
        ctx_traits_core::r#trait::composition::PlanResult::NeedsDecision
            | ctx_traits_core::r#trait::composition::PlanResult::Failed
    );

    let conflict_records = plan
        .conflicts
        .iter()
        .map(|conflict| ComposeConflictRecord {
            kind: conflict.kind.as_str().to_string(),
            field_path: conflict.field_path.clone(),
            description: safe_assist_context_text(&conflict.description),
            participants: conflict
                .participants
                .iter()
                .map(|participant| ComposeConflictParticipant {
                    source_index: participant.source_index,
                    trait_id: participant.trait_id.clone(),
                    field_path: participant.field_path.clone(),
                    value: participant.value.as_deref().map(safe_assist_context_text),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let warning_records = plan
        .warnings
        .iter()
        .map(|warning| ComposeWarningRecord {
            code: warning.code.clone(),
            kind: warning.code.clone(),
            trait_ids: warning.trait_ids.clone(),
            message: safe_assist_context_text(&warning.message),
        })
        .collect::<Vec<_>>();
    let warning_codes = plan
        .warnings
        .iter()
        .map(|warning| warning.code.clone())
        .collect::<Vec<_>>();

    let composition_evidence = CompositionEvidenceReport {
        plan_result: plan_result_str.to_string(),
        source_trait_ids: trait_ids.clone(),
        source_paths: source_paths.clone(),
        source_digests: digest_strs,
        conflict_count: plan.conflicts.len(),
        conflict_fields: plan
            .conflicts
            .iter()
            .map(|c| c.field_path.clone())
            .collect(),
        conflicts: conflict_records,
        warning_count: plan.warnings.len(),
        warning_codes,
        warnings: warning_records,
        binding_proposal_count: plan.binding_proposals.len(),
        port_compatibility_count: plan.port_compatibility.len(),
        context_digest: context_digest.as_str().to_string(),
        statement: "source attribution, conflicts, and binding evidence cannot be erased by model candidate",
    };

    let target_path = out.map_or_else(
        || trait_package_output_paths("compose-candidate", None).1,
        |out_path| trait_package_output_paths("compose-candidate", Some(out_path)).1,
    );

    let candidate =
        ctx_traits_core::assist::plan_assist_boundary(ctx_traits_core::assist::BoundaryRequest {
            operation: ctx_traits_core::assist::Operation::Compose,
            source_trait_ids: trait_ids.clone(),
            source_paths: source_paths.clone(),
            source_digests: source_digests.clone(),
            user_request: format!("compose {} traits", ids.len()),
            model: model.map(str::to_string),
            target_path: target_path.clone(),
            provider_available: false,
            context: to_json_value(
                &CompositionAssistContext {
                    composition_plan: serde_json::from_str::<serde_json::Value>(&context_json)
                        .unwrap_or_else(|_| serde_json::Value::Object(Default::default())),
                    context_digest: context_digest.as_str().to_string(),
                    profile: profile.unwrap_or("agent-skills").to_string(),
                    target_schema: "agent-traits/canonical-trait",
                },
                "serialize composition assist context",
            )?,
        })?;

    let candidate = ctx_traits_core::assist::with_context_evidence(
        candidate,
        to_json_value(&composition_evidence, "serialize composition evidence")?,
    );

    let Some(cand_path) = candidate_path else {
        print_assist_candidate(&candidate, json)?;
        return Err(crate::Error::Command {
            message: "compose failed: model provider adapter is unavailable; no candidate was produced or written".to_string(),
        });
    };
    let cand_utf8 = camino::Utf8Path::new(cand_path);
    let cand_encoding = ctx_traits_core::encoding::Encoding::from_path(cand_utf8)?;
    let raw = ctx_traits_io::read::read_text(cand_utf8)?;
    let evaluation =
        ctx_traits_core::assist::evaluate_supplied_candidate(candidate, &raw, cand_encoding);

    let actual_trait_id = evaluation
        .candidate
        .candidate_trait_id
        .clone()
        .unwrap_or_else(|| "compose-candidate".to_string());
    let (package, target_path) = trait_package_output_paths(&actual_trait_id, out);
    let trait_root = camino::Utf8Path::new(&package);
    let mut candidate = attach_assist_check_report(
        evaluation.candidate,
        evaluation.normalized_trait.as_ref(),
        evaluation.normalized_output_text.as_deref(),
        trait_root,
    )?;

    if plan_blocks_write {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate.warnings.push(format!(
            "composition plan result is {plan_result_str}; candidate cannot be written until conflicts are resolved"
        ));
    }
    if check {
        candidate = apply_assist_check_drift(
            candidate,
            &target_path,
            evaluation.normalized_output_text.as_deref(),
        );
    }

    let final_candidate = if candidate.gate_summary.all_passed()
        && candidate.status != ctx_traits_core::assist::CandidateStatus::Blocked
        && !check
        && !plan_blocks_write
    {
        match evaluation.normalized_output_text.as_deref() {
            Some(text) => {
                match ctx_traits_io::write::write_candidate(
                    ctx_traits_io::write::CandidateWriteRequest {
                        target_path: camino::Utf8Path::new(&target_path),
                        trait_id: &actual_trait_id,
                        content: text,
                        mode: ctx_traits_io::write::CandidateWriteMode::NewCandidate,
                    },
                ) {
                    Ok(result) => ctx_traits_core::assist::mark_written(candidate, &result.path),
                    Err(_) => {
                        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                        candidate.warnings.push(
                            "write gate failed: safe writer rejected target path".to_string(),
                        );
                        candidate
                    }
                }
            }
            None => candidate,
        }
    } else {
        candidate
    };

    print_assist_candidate(&final_candidate, json)?;

    if plan_blocks_write
        || final_candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked
    {
        return Err(crate::Error::Command {
            message: if check {
                "compose --check failed: candidate blocked by gates or composition plan".to_string()
            } else {
                "compose failed: candidate blocked by gates or composition plan; no file was written".to_string()
            },
        });
    }

    Ok(CommandOutput::new(()))
}

pub(crate) use handle_compose as handle;
