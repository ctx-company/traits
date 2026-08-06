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

/// Path within `trait_id`'s scratch package where every refine/import round
/// persists the raw candidate text before it can fail any rung — the file a
/// non-convergence error names as the preserved candidate (task 0066.3
/// "Done when": a non-converging run "preserves its last candidate at a
/// stated path"). Overwritten every round, so it always holds the most
/// recent candidate regardless of which rung it fails.
pub(crate) fn candidate_path(trait_id: &str) -> Utf8PathBuf {
    scratch_package_root(trait_id).join("candidate.json")
}

/// Persist `candidate_text` to [`candidate_path`] for `trait_id`, so the
/// scratch file backing a non-convergence error's stated path is written
/// before evaluation can fail — restoring parity with generate's
/// preserve-on-failure contract for candidates that never reach a build
/// rung (refine/import candidates are already canonical JSON).
fn persist_candidate(trait_id: &str, candidate_text: &str) -> crate::Result<()> {
    ctx_traits_io::write::write_build_output(&candidate_path(trait_id), candidate_text)?;
    Ok(())
}

/// Shared ladder core for `refine-round`/`import-round`: the candidate is
/// already canonical JSON (refine's `proposed-trait`, import's decoded trait
/// draft), so unlike [`evaluate_round`] there is no build rung — the ladder
/// starts at synth/normalize. `identity_extra` runs only once the plain
/// trait-id check passes, so a command-specific identity/baseline predicate
/// (import's scaffold-retention check) can add a further `Identity`-rung
/// failure without duplicating the id comparison.
fn evaluate_canonical_ladder(
    candidate_json: &str,
    expected_trait_id: &str,
    package_root: &Utf8Path,
    identity_extra: impl FnOnce(&ctx_traits_core::Trait) -> Option<Diagnostic>,
) -> crate::Result<RoundReport> {
    persist_candidate(expected_trait_id, candidate_json)?;
    let boundary = plan_deterministic_boundary(ctx_traits_core::assist::BoundaryRequest {
        operation: ctx_traits_core::assist::Operation::Generate,
        source_trait_ids: vec![expected_trait_id.to_string()],
        source_paths: Vec::new(),
        source_digests: Vec::new(),
        user_request: format!("round evaluation of {expected_trait_id}"),
        model: None,
        target_path: package_root.to_string(),
        provider_available: false,
        context: serde_json::Value::Null,
    })?;
    let evaluation = evaluate_supplied_candidate(
        boundary,
        candidate_json,
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

    if normalized_trait.id.as_str() != expected_trait_id {
        return Ok(failed(
            Rung::Identity,
            vec![Diagnostic {
                gate: Gate::Normalize,
                code: DiagnosticCode::IdentityChanged,
                field: Some("id".to_string()),
                message: format!(
                    "candidate trait id {:?} does not match expected id {expected_trait_id:?}",
                    normalized_trait.id.as_str()
                ),
            }],
        ));
    }
    if let Some(diagnostic) = identity_extra(normalized_trait) {
        return Ok(failed(Rung::Identity, vec![diagnostic]));
    }

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

    let candidate = crate::app::generate::attach_assist_check_report(
        evaluation.candidate,
        Some(normalized_trait),
        Some(normalized_text),
        package_root,
    )?;
    if !candidate.gate_summary.check.ok {
        return Ok(failed(Rung::Check, candidate.diagnostics));
    }
    if !candidate.gate_summary.audit.ok {
        return Ok(failed(Rung::Audit, candidate.diagnostics));
    }

    Ok(RoundReport {
        rung: Rung::Audit,
        converged: true,
        diagnostics: Vec::new(),
    })
}

/// Evaluate one `refine --apply` round: `candidate` is the refine scaffold
/// JSON text (`{"source-trait-id", "source-digest", "proposed-trait",
/// "patches"}`). Re-reads `source_path` for the digest/line-count scaffold
/// anchors are validated against, so a scaffold whose patches drifted from
/// the live source fails here, at `SynthNormalize`, as a typed diagnostic —
/// never a hard error the loop cannot recover from.
pub(crate) fn evaluate_refine_round(
    source_path: &str,
    candidate_scaffold: &str,
) -> crate::Result<(RoundReport, String)> {
    let source_text = ctx_traits_io::read::read_text(Utf8Path::new(source_path))?;
    let source_digest = ctx_traits_core::digest::Digest::source(&source_text);
    let (existing_trait, _) = ctx_traits_core::encoding::decode_trait_with_warnings(
        ctx_traits_core::encoding::Encoding::from_path(Utf8Path::new(source_path))?,
        &source_text,
    )?;
    let trait_id = existing_trait.id.as_str().to_string();
    persist_candidate(&trait_id, candidate_scaffold)?;
    let (candidate_text, _scaffold) = match crate::app::refine::decode_refine_candidate(
        candidate_scaffold,
        source_digest.as_str(),
        source_path,
        source_text.lines().count(),
    ) {
        Ok(decoded) => decoded,
        Err(error) => {
            return Ok((
                failed(
                    Rung::SynthNormalize,
                    vec![Diagnostic {
                        gate: Gate::Parse,
                        code: DiagnosticCode::ParseFailed,
                        field: None,
                        message: error.to_string(),
                    }],
                ),
                trait_id,
            ));
        }
    };
    let package_root = scratch_package_root(&trait_id);
    let report = evaluate_canonical_ladder(&candidate_text, &trait_id, &package_root, |_| None)?;
    Ok((report, trait_id))
}

/// Path within `trait_id`'s scratch package where `handle_import` persists
/// the deterministic scaffold baseline text before driving the guarded
/// import loop, and `import-round` reads it back to check baseline
/// retention (task 0066.3 §3).
pub(crate) fn baseline_path(trait_id: &str) -> Utf8PathBuf {
    scratch_package_root(trait_id).join("baseline.toml")
}

/// Persist the deterministic scaffold baseline `handle_import` computed for
/// `trait_id` before driving the guarded import loop, so `import-round`
/// (which never sees the CLI's in-memory analysis) has the same baseline to
/// converge against for every round of the same run.
pub(crate) fn persist_baseline(trait_id: &str, baseline_text: &str) -> crate::Result<Utf8PathBuf> {
    let path = baseline_path(trait_id);
    ctx_traits_io::write::write_build_output(&path, baseline_text)?;
    Ok(path)
}

/// Evaluate one `import --llm-assisted` round: `candidate` is the raw model
/// output (either a `{"trait": ...}` wrapper or a bare canonical draft).
/// Re-reads the deterministic scaffold baseline text `handle_import`
/// persisted to `baseline_path(trait_id)` before driving the loop — the
/// convergence target the candidate must remain recognizable against
/// (declaration set retained; enrichment adds, never deletes).
pub(crate) fn evaluate_import_round(trait_id: &str, candidate: &str) -> crate::Result<RoundReport> {
    persist_candidate(trait_id, candidate)?;
    let (candidate_text, _wrapped) = crate::app::generate::decode_generate_candidate(candidate)?;
    let package_root = scratch_package_root(trait_id);
    let baseline_text = ctx_traits_io::read::read_text(&baseline_path(trait_id))?;
    let (baseline_trait, _) = ctx_traits_core::encoding::decode_trait_with_warnings(
        ctx_traits_core::encoding::Encoding::Toml,
        &baseline_text,
    )?;
    evaluate_canonical_ladder(
        &candidate_text,
        trait_id,
        &package_root,
        |candidate_trait| {
            let missing: Vec<String> = declaration_ids(&baseline_trait)
                .difference(&declaration_ids(candidate_trait))
                .cloned()
                .collect();
            if missing.is_empty() {
                return None;
            }
            Some(Diagnostic {
                gate: Gate::Normalize,
                code: DiagnosticCode::BaselineDiscarded,
                field: None,
                message: format!(
                    "candidate drops declaration(s) present in the imported scaffold baseline: {}",
                    missing.join(", ")
                ),
            })
        },
    )
}

/// A trait's declaration set as a deterministic proxy for "recognizably the
/// source": dependency aliases, resource/schema ids, and metadata tags.
/// Import's baseline rung checks this set is retained (a subset relation),
/// not equal — enrichment is expected to add declarations, never drop them.
fn declaration_ids(trait_ref: &ctx_traits_core::Trait) -> std::collections::BTreeSet<String> {
    let mut ids = std::collections::BTreeSet::new();
    for dependency in &trait_ref.dependencies {
        ids.insert(format!("dependency:{}", dependency.alias));
    }
    for resource in &trait_ref.resources {
        ids.insert(format!("resource:{}", resource.id));
    }
    for schema in &trait_ref.schemas {
        ids.insert(format!("schema:{}", schema.id));
    }
    if let Some(metadata) = trait_ref.metadata.as_ref() {
        for tag in metadata.tag.iter() {
            ids.insert(format!("tag:{}", tag.as_str()));
        }
    }
    ids
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
