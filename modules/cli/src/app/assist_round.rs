//! Single-round candidate evaluator shared by `generate`'s in-loop command
//! rung and its `--candidate` path (task 0066.1).
//!
//! Climbs the rung ladder — build, synth/normalize, non-degenerate, check,
//! audit — stopping at the first rung a candidate fails. Never calls a
//! provider and never loops; the meta-trait loop (or a single `--candidate`
//! caller) is responsible for iteration.

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::assist::{
    Diagnostic, DiagnosticCode, Gate, RoundReport, Rung, evaluate_supplied_candidate,
    plan_deterministic_boundary,
};

/// Scratch package root a round evaluates a candidate against:
/// `<trait-protocol-root>/<trait_id>-candidate` — the `-candidate` suffix is
/// what the safe writer's `NewCandidateSource` mode accepts alongside the
/// exact trait ID (`write_candidate`'s `package_is_candidate` check).
/// Rounds never touch `<trait-protocol-root>/<trait_id>` itself, so an
/// in-progress or non-converging run can never collide with (or overwrite)
/// an already-published package of the same name.
pub(crate) fn scratch_package_root(trait_id: &str) -> Utf8PathBuf {
    Utf8Path::new(ctx_traits_io::layout::trait_protocol_root())
        .join(format!("{trait_id}-candidate"))
}

/// Evaluate one candidate authoring source through the rung ladder against a
/// package rooted at `package_root`. Writes the candidate to
/// `<package_root>/source/index.ts` through the safe writer before building
/// it, so every rung after `build` observes exactly what a real `ctx traits
/// build` of that source would produce.
pub(crate) fn evaluate_round(
    candidate_source: &str,
    package_root: &Utf8Path,
    trait_id: &str,
) -> crate::Result<RoundReport> {
    let source_path = ctx_traits_io::layout::package_source_write_path(package_root);
    // Each round overwrites the same scratch candidate: the first round
    // creates it (`NewCandidateSource`, which refuses to overwrite), every
    // round after that lands on an already-existing scratch file and must
    // use the overwrite-permitting mode instead, or the second round of any
    // loop would fail with "target already exists" before it even reached
    // the build rung.
    if source_path.exists() {
        // `RefineApplySource`'s path validator requires the package ID to
        // match `trait_id` exactly (it has no candidate-suffix allowance —
        // that belongs to `NewCandidateSource`, whose first-write-only
        // package IS the `<trait_id>-candidate` scratch package), so the ID
        // passed here must be the scratch package's own ID.
        let scratch_trait_id = format!("{trait_id}-candidate");
        ctx_traits_io::write::write_candidate(ctx_traits_io::write::CandidateWriteRequest {
            target_path: &source_path,
            trait_id: &scratch_trait_id,
            content: candidate_source,
            mode: ctx_traits_io::write::CandidateWriteMode::RefineApplySource,
        })?;
    } else {
        ctx_traits_io::write::write_candidate(ctx_traits_io::write::CandidateWriteRequest {
            target_path: &source_path,
            trait_id,
            content: candidate_source,
            mode: ctx_traits_io::write::CandidateWriteMode::NewCandidateSource,
        })?;
    }

    // Rung 1: build.
    let synth = match crate::app::cdk_build::synthesize_cdk_source(
        &source_path,
        ctx_traits_core::synth::OutputFormat::Json,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            return Ok(failed(Rung::Build, vec![build_error_diagnostic(&error)]));
        }
    };
    let build_errors: Vec<Diagnostic> = synth
        .authoring_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == "error")
        .map(|diagnostic| Diagnostic {
            gate: Gate::Build,
            code: DiagnosticCode::CdkBuildFailed,
            field: Some(diagnostic.field_path.clone()),
            message: diagnostic.message.clone(),
        })
        .collect();
    if !build_errors.is_empty() {
        return Ok(failed(Rung::Build, build_errors));
    }

    // Rung 2: synth/normalize — evaluate the built canonical draft through
    // the existing candidate gate pipeline.
    let boundary = plan_deterministic_boundary(ctx_traits_core::assist::BoundaryRequest {
        operation: ctx_traits_core::assist::Operation::Generate,
        source_trait_ids: vec![trait_id.to_string()],
        source_paths: Vec::new(),
        source_digests: Vec::new(),
        user_request: format!("round evaluation of {trait_id}"),
        model: None,
        target_path: package_root.to_string(),
        provider_available: false,
        context: serde_json::Value::Null,
    })?;
    let evaluation = evaluate_supplied_candidate(
        boundary,
        &synth.response.canonical_json,
        ctx_traits_core::encoding::Encoding::Json,
    );
    if !evaluation.candidate.gate_summary.parse.ok
        || !evaluation.candidate.gate_summary.normalize.ok
    {
        return Ok(failed(
            Rung::SynthNormalize,
            evaluation.candidate.diagnostics,
        ));
    }
    let Some(normalized_trait) = evaluation.normalized_trait.as_ref() else {
        return Ok(failed(
            Rung::SynthNormalize,
            evaluation.candidate.diagnostics,
        ));
    };
    let Some(normalized_text) = evaluation.normalized_output_text.as_deref() else {
        return Ok(failed(
            Rung::SynthNormalize,
            evaluation.candidate.diagnostics,
        ));
    };

    // Rung 3: non-degenerate — a candidate that compiles but declares no
    // procedure sequence converges by deletion, which this rung refuses.
    let has_sequence = normalized_trait
        .procedure
        .as_ref()
        .is_some_and(|procedure| !procedure.sequence.is_empty());
    if !has_sequence {
        return Ok(failed(
            Rung::NonDegenerate,
            vec![Diagnostic {
                gate: Gate::Build,
                code: DiagnosticCode::DegenerateCandidate,
                field: Some("procedure.sequence".to_string()),
                message: "candidate declares no procedure sequence".to_string(),
            }],
        ));
    }

    // Rung 4: check.
    let candidate = crate::app::generate::attach_assist_check_report(
        evaluation.candidate,
        Some(normalized_trait),
        Some(normalized_text),
        package_root,
    )?;
    if !candidate.gate_summary.check.ok {
        return Ok(failed(Rung::Check, candidate.diagnostics));
    }

    // Rung 5: audit — hidden-content scan already folded into both the
    // synth/normalize gate (raw + normalized text) and the check gate's
    // `HiddenContentAudit` section (normalized text + resources); nothing
    // further to run here, only to gate on.
    if !candidate.gate_summary.audit.ok {
        return Ok(failed(Rung::Audit, candidate.diagnostics));
    }

    Ok(RoundReport {
        rung: Rung::Audit,
        converged: true,
        diagnostics: Vec::new(),
    })
}

fn failed(rung: Rung, diagnostics: Vec<Diagnostic>) -> RoundReport {
    RoundReport {
        rung,
        converged: false,
        diagnostics,
    }
}

fn build_error_diagnostic(error: &crate::Error) -> Diagnostic {
    let message = error.to_string();
    let truncated = if message.len() > 480 {
        format!("{}…", &message[..480])
    } else {
        message
    };
    Diagnostic {
        gate: Gate::Build,
        code: DiagnosticCode::CdkBuildFailed,
        field: None,
        message: truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Building a candidate resolves `@ctx-traits/cdk` via the repo's own
    // `node_modules`, so the scratch package must sit inside this checkout's
    // real trait-protocol root, not an isolated tmp dir.
    fn repo_root() -> Utf8PathBuf {
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize_utf8()
            .expect("resolve repo root from crate manifest dir")
    }

    fn scratch_package(label: &str) -> Utf8PathBuf {
        let trait_id = format!("assist-round-test-{label}-{}", std::process::id());
        let root = repo_root().join(scratch_package_root(&trait_id));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("source")).expect("create scratch package root");
        root
    }

    #[test]
    fn broken_typescript_fails_at_build_rung() {
        let root = scratch_package("broken-ts");
        let trait_id = root
            .file_name()
            .and_then(|name| name.strip_suffix("-candidate"))
            .expect("scratch package name ends in -candidate")
            .to_string();
        let report = evaluate_round("this is not valid TypeScript {{{", &root, &trait_id)
            .expect("evaluator itself must not error on a bad candidate");
        assert_eq!(report.rung, Rung::Build);
        assert!(!report.converged);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn degenerate_candidate_fails_at_non_degenerate_rung() {
        let root = scratch_package("degenerate");
        let trait_id = root
            .file_name()
            .and_then(|name| name.strip_suffix("-candidate"))
            .expect("scratch package name ends in -candidate")
            .to_string();
        let source = format!(
            "import {{ trait }} from \"@ctx-traits/cdk\";\n\
             export default trait(\"{trait_id}\", {{\n\
             \x20\x20version: \"0.1.0\",\n\
             \x20\x20name: \"Degenerate\",\n\
             \x20\x20summary: \"Compiles but declares no procedure.\",\n\
             }});\n"
        );
        let report = evaluate_round(&source, &root, &trait_id)
            .expect("evaluator itself must not error on a degenerate candidate");
        assert_eq!(report.rung, Rung::NonDegenerate);
        assert!(!report.converged);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::DegenerateCandidate)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
