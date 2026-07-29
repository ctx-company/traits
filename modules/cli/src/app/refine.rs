//! Refine command handler.

use crate::app::entry::to_json_value;
use crate::app::generate::{
    RefineAssistContext, RefineEvidenceReport, apply_assist_check_drift,
    attach_assist_check_report, blocked_assist_candidate, blocked_wrapper_candidate,
    canonical_json_value, print_assist_candidate, run_builtin_trait, runtime_input,
    trait_package_output_paths,
};
use ctx_traits_core::response::CommandOutput;

pub(crate) struct RefineInputs<'a> {
    pub(crate) id_or_path: &'a str,
    pub(crate) change_request: &'a str,
    pub(crate) model: Option<&'a str>,
    pub(crate) assignments: &'a [String],
    pub(crate) out: Option<&'a str>,
    pub(crate) apply: bool,
    pub(crate) candidate_path: Option<&'a str>,
    pub(crate) check: bool,
    pub(crate) json: bool,
}

pub(crate) fn handle_refine(input: RefineInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let RefineInputs {
        id_or_path,
        change_request,
        model,
        assignments,
        out,
        apply,
        candidate_path,
        check,
        json,
    } = input;
    let path = if camino::Utf8Path::new(id_or_path).extension().is_none() {
        ctx_traits_io::run::resolve_trait_path(None, Some(id_or_path), "refine")?.0
    } else {
        camino::Utf8PathBuf::from(id_or_path)
    };
    let source_path = path.as_str();
    let encoding = ctx_traits_core::encoding::Encoding::from_path(&path)?;
    let source_text = ctx_traits_io::read::read_text(&path)?;
    let source_digest = ctx_traits_core::digest::Digest::source(&source_text);
    let (existing_trait, decode_warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &source_text)?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(source_path, &decode_warnings);
    let source_trait_id = existing_trait.id.as_str().to_string();

    let target_path = if apply {
        Some(source_path.to_string())
    } else {
        out.map(|out_path| trait_package_output_paths(&source_trait_id, Some(out_path)).1)
    };
    let plan_target_path = target_path
        .clone()
        .unwrap_or_else(|| "preview-only-no-write-target".to_string());

    let candidate =
        ctx_traits_core::assist::plan_assist_boundary(ctx_traits_core::assist::BoundaryRequest {
            operation: ctx_traits_core::assist::Operation::Refine,
            source_trait_ids: vec![source_trait_id.clone()],
            source_paths: vec![source_path.to_string()],
            source_digests: vec![source_digest.clone()],
            user_request: change_request.to_string(),
            model: model.map(str::to_string),
            target_path: plan_target_path.clone(),
            provider_available: candidate_path.is_none(),
            context: to_json_value(
                &RefineAssistContext {
                    change_request,
                    trait_id: source_trait_id.clone(),
                    source_digest: source_digest.as_str().to_string(),
                    target_schema: "agent-traits/canonical-trait",
                },
                "serialize refine assist context",
            )?,
        })?;

    let (raw, candidate_encoding) = match candidate_path {
        Some(cand_path) => {
            let cand_utf8 = camino::Utf8Path::new(cand_path);
            (
                ctx_traits_io::read::read_text(cand_utf8)?,
                ctx_traits_core::encoding::Encoding::from_path(cand_utf8)?,
            )
        }
        None => (
            match run_builtin_trait(
                "refine-trait",
                vec![
                    runtime_input("source", source_text.as_str()),
                    runtime_input("source-digest", source_digest.as_str()),
                    runtime_input("source-path", source_path),
                    runtime_input("change-request", change_request),
                ],
                assignments,
                model,
                None,
            ) {
                Ok(raw) => raw,
                Err(error) => return blocked_assist_candidate(candidate, json, error.to_string()),
            },
            ctx_traits_core::encoding::Encoding::Json,
        ),
    };
    let (candidate_text, scaffold) = match decode_refine_candidate(
        &raw,
        source_digest.as_str(),
        source_path,
        source_text.lines().count(),
    ) {
        Ok(decoded) => decoded,
        Err(error) => {
            return blocked_wrapper_candidate(
                candidate,
                &raw,
                candidate_encoding,
                &source_trait_id,
                json,
                error.to_string(),
            );
        }
    };
    let candidate_encoding = if scaffold.is_some() {
        ctx_traits_core::encoding::Encoding::Json
    } else {
        candidate_encoding
    };
    let mut evaluation = ctx_traits_core::assist::evaluate_supplied_candidate(
        candidate,
        &candidate_text,
        candidate_encoding,
    );
    if scaffold.is_some() {
        // Preserve the complete scaffold as the audited model output identity;
        // only its validated proposed trait is decoded as a canonical trait.
        evaluation.candidate.raw_output_digest =
            Some(ctx_traits_core::digest::Digest::source(&raw));
        evaluation.candidate.parsed_candidate_digest = Some(
            ctx_traits_core::digest::Digest::canonical(&canonical_json_value(&raw)?),
        );
        evaluation.candidate = ctx_traits_core::assist::audit_wrapper_output(
            evaluation.candidate,
            &raw,
            &source_trait_id,
        );
    }

    let check_root_path = target_path.as_deref().unwrap_or(source_path);
    let check_root =
        ctx_traits_io::layout::package_root_for_manifest(camino::Utf8Path::new(check_root_path))
            .unwrap_or_else(|| camino::Utf8Path::new("."));
    let mut candidate = attach_assist_check_report(
        evaluation.candidate,
        evaluation.normalized_trait.as_ref(),
        evaluation.normalized_output_text.as_deref(),
        check_root,
    )?;
    let refine_evidence = RefineEvidenceReport {
        source_trait_id: source_trait_id.clone(),
        source_path: source_path.to_string(),
        source_digest: source_digest.clone(),
        candidate_trait_id: candidate.candidate_trait_id.clone(),
        candidate_canonical_digest: candidate.canonical_digest.clone(),
        canonical_text_differs: evaluation
            .normalized_output_text
            .as_ref()
            .is_some_and(|text| text != &source_text),
        target_path: plan_target_path.clone(),
        apply,
    };
    let evidence = match scaffold.filter(|_| candidate.gate_summary.audit.ok) {
        Some(scaffold) => serde_json::json!({
            "refinement": to_json_value(&refine_evidence, "serialize refine evidence")?,
            "scaffold": scaffold,
        }),
        None => to_json_value(&refine_evidence, "serialize refine evidence")?,
    };
    candidate = ctx_traits_core::assist::with_context_evidence(candidate, evidence);

    if let Some(ref cand_id) = candidate.candidate_trait_id
        && cand_id != &source_trait_id
    {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate.warnings.push(format!(
            "candidate trait ID {cand_id} does not match source trait ID {source_trait_id}"
        ));
        print_assist_candidate(&candidate, json)?;
        return Err(crate::Error::Command {
            message: format!(
                "refine failed: candidate trait ID {cand_id} does not match source trait ID {source_trait_id}"
            ),
        });
    }

    if check && let Some(target_path) = target_path.as_deref() {
        candidate = apply_assist_check_drift(
            candidate,
            target_path,
            evaluation.normalized_output_text.as_deref(),
        );
    }

    let final_candidate = if candidate.gate_summary.all_passed()
        && candidate.status != ctx_traits_core::assist::CandidateStatus::Blocked
        && !check
    {
        match evaluation.normalized_output_text.as_deref() {
            Some(text) if target_path.is_some() => {
                let target_path = target_path.as_deref().unwrap_or("");
                if apply {
                    match ctx_traits_io::read::read_text(camino::Utf8Path::new(target_path)) {
                        Ok(current)
                            if ctx_traits_core::digest::Digest::source(&current)
                                == source_digest => {}
                        Ok(_) => {
                            candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                            candidate.warnings.push(
                                "write gate failed: source trait changed while refine candidate was being prepared"
                                    .to_string(),
                            );
                            return finish_refine_candidate(candidate, json, check);
                        }
                        Err(error) => {
                            candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                            candidate.warnings.push(format!(
                                "write gate failed: could not re-read refine source before apply: {error}"
                            ));
                            return finish_refine_candidate(candidate, json, check);
                        }
                    }
                }
                let write_mode = if apply {
                    ctx_traits_io::write::CandidateWriteMode::RefineApply
                } else {
                    ctx_traits_io::write::CandidateWriteMode::NewCandidate
                };
                match ctx_traits_io::write::write_candidate(
                    ctx_traits_io::write::CandidateWriteRequest {
                        target_path: camino::Utf8Path::new(&target_path),
                        trait_id: &source_trait_id,
                        content: text,
                        mode: write_mode,
                    },
                ) {
                    Ok(result) => {
                        if apply {
                            ctx_traits_core::assist::mark_applied(candidate, &result.path)
                        } else {
                            ctx_traits_core::assist::mark_written(candidate, &result.path)
                        }
                    }
                    Err(_) => {
                        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                        candidate.warnings.push(
                            "write gate failed: safe writer rejected target path".to_string(),
                        );
                        candidate
                    }
                }
            }
            Some(_) => candidate,
            None => candidate,
        }
    } else {
        candidate
    };

    print_assist_candidate(&final_candidate, json)?;

    if final_candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked {
        return Err(crate::Error::Command {
            message: if check {
                "refine --check failed: candidate was blocked by validation gates".to_string()
            } else {
                "refine failed: candidate was blocked by validation gates; no file was written"
                    .to_string()
            },
        });
    }

    Ok(CommandOutput::new(()))
}

fn finish_refine_candidate(
    candidate: ctx_traits_core::assist::Candidate,
    json: bool,
    check: bool,
) -> crate::Result<CommandOutput<()>> {
    print_assist_candidate(&candidate, json)?;
    Err(crate::Error::Command {
        message: if check {
            "refine --check failed: candidate was blocked by validation gates".to_string()
        } else {
            "refine failed: candidate was blocked by validation gates; no file was written"
                .to_string()
        },
    })
}

fn decode_refine_candidate(
    raw: &str,
    expected_source_digest: &str,
    source_path: &str,
    source_line_count: usize,
) -> crate::Result<(String, Option<ctx_traits_core::scaffold::RefineScaffold>)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok((raw.to_string(), None));
    };
    if value.get("source-trait-id").is_none() {
        return Ok((raw.to_string(), None));
    }
    let scaffold: ctx_traits_core::scaffold::RefineScaffold = serde_json::from_value(value)
        .map_err(|e| crate::Error::json("decode refine scaffold", e))?;
    scaffold.validate()?;
    if scaffold.source_digest != expected_source_digest {
        return Err(crate::Error::Command {
            message: "refine scaffold source digest does not match the source trait".to_string(),
        });
    }
    for (index, patch) in scaffold.patches.iter().enumerate() {
        if patch.anchor.file != source_path || patch.anchor.end > source_line_count {
            return Err(crate::Error::Command {
                message: format!(
                    "refine scaffold patch {index} anchor must reference {source_path:?} within its {source_line_count} source lines"
                ),
            });
        }
    }
    let candidate = serde_json::to_string(&scaffold.proposed_trait)
        .map_err(|e| crate::Error::json("serialize refine scaffold proposed trait", e))?;
    Ok((candidate, Some(scaffold)))
}

pub(crate) use handle_refine as handle;
