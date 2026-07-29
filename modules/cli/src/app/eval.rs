//! Evaluation command handler.

use crate::app::entry::{build_file_evidence_from_io, print_lock_update};
use ctx_traits_core::response::CommandOutput;

pub(crate) struct EvalInputs<'a> {
    pub(crate) file: &'a str,
    pub(crate) eval_ids: &'a [String],
    pub(crate) variant: Option<&'a str>,
    pub(crate) out: Option<&'a str>,
    pub(crate) update_lock: bool,
    pub(crate) json: bool,
}

pub(crate) fn handle_eval(input: EvalInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let path = camino::Utf8Path::new(input.file);
    let text = ctx_traits_io::read::read_text(path)?;
    let (trait_ref, trait_root, source_digest, canonical_digest) =
        ctx_traits_io::run::load_trait(input.file)?;
    let trait_root = trait_root.as_path();
    let roots = ctx_traits_io::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    let manifest = ctx_traits_io::resource::digest_resources(
        &roots,
        trait_ref.id.as_str(),
        &trait_ref.resources,
    )?;
    let file_evidence = build_file_evidence_from_io(&manifest);
    let selected_variant = match input.variant {
        Some(variant) => Some(ctx_traits_core::r#trait::EvalVariant::parse(variant).ok_or_else(|| {
            crate::Error::Command {
                message: format!(
                    "unsupported eval variant {variant:?}; expected documentation, lint, golden-render, behavioral, or runtime"
                ),
            }
        })?),
        None => None,
    };
    let golden_render = build_golden_render_evidence(
        path,
        trait_root,
        &trait_ref,
        source_digest.as_str(),
        &file_evidence,
        Some(manifest.manifest_digest.as_str()),
    )?;
    let report = ctx_traits_core::eval_run::run_evals(
        &trait_ref,
        ctx_traits_core::eval_run::Request {
            source_text: text,
            source_digest: source_digest.clone(),
            canonical_digest: canonical_digest.clone(),
            resource_manifest_digest: Some(manifest.manifest_digest.clone()),
            evaluated_at: "1970-01-01T00:00:00Z".to_string(),
            selected_eval_ids: input.eval_ids.to_vec(),
            selected_variant,
            lint_warnings: crate::app::report_resources::resource_read_warning_strings(
                &manifest.warnings,
            ),
            golden_render,
        },
    )?;

    if let Some(out_path) = input.out {
        let report_json = serde_json::to_string_pretty(&report)
            .map_err(|e| crate::Error::json("serialize eval report", e))?;
        ctx_traits_io::write::write_text(
            camino::Utf8Path::new(out_path),
            &format!("{report_json}\n"),
        )?;
    }

    if input.update_lock {
        let variant = trait_ref.variant.as_deref();
        for result in report.passing_results() {
            let update = ctx_traits_io::lockfile::update_eval_result_evidence(
                trait_root,
                trait_ref.id.as_str(),
                variant,
                result,
            )?;
            print_lock_update(&update);
        }
    }

    if input.json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| crate::Error::json("serialize eval report", e))?;
        println!("{json}");
    } else {
        print_eval_report(&report, input.out);
    }

    if !report.passed {
        return Err(crate::Error::Command {
            message: format!("eval failed for trait {}", report.trait_id),
        });
    }
    Ok(CommandOutput::new(()))
}

fn build_golden_render_evidence(
    _trait_path: &camino::Utf8Path,
    trait_root: &camino::Utf8Path,
    trait_ref: &ctx_traits_core::Trait,
    source_digest: &str,
    file_evidence: &[ctx_traits_core::resource_plan::FileEvidence],
    resource_manifest_digest: Option<&str>,
) -> crate::Result<Vec<ctx_traits_core::eval_run::GoldenRenderEvidence>> {
    let mut evidence = Vec::new();
    for eval in trait_ref
        .evals
        .iter()
        .filter(|eval| eval.variant == ctx_traits_core::r#trait::EvalVariant::GoldenRender)
    {
        let Some(input_ref) = eval.input.as_deref() else {
            continue;
        };
        let Some(profile) = input_ref.strip_prefix("render:") else {
            continue;
        };
        let Some(render_profile) = ctx_traits_core::render::ExtendedRenderProfile::parse(profile)
        else {
            continue;
        };
        let plan = ctx_traits_core::render::plan_render_with_evidence(
            trait_ref,
            render_profile,
            source_digest,
            file_evidence,
            resource_manifest_digest,
            Vec::new(),
        );
        let actual_text = format!(
            "{}\n\n# {}\n\n{}\n",
            plan.generated_file_marker, plan.trait_id, plan.model_view.full_text
        );
        let actual_digest = ctx_traits_core::digest::Digest::source(&actual_text);
        let (expected_text, expected_digest, fixture_available) = match eval.output.as_deref() {
            Some(output_ref) => {
                let fixture_path = fixture_path_from_ref(trait_root, output_ref)?;
                match ctx_traits_io::read::read_optional_text(&fixture_path)? {
                    Some(text) => {
                        let digest = ctx_traits_core::digest::Digest::source(&text);
                        (Some(text), Some(digest), true)
                    }
                    None => (None, None, false),
                }
            }
            None => (None, None, false),
        };
        evidence.push(ctx_traits_core::eval_run::GoldenRenderEvidence {
            eval_id: eval.id.clone(),
            profile: render_profile.as_str().to_string(),
            actual_text,
            actual_digest,
            expected_text,
            expected_digest,
            fixture_ref: eval.output.clone(),
            fixture_available,
        });
    }
    Ok(evidence)
}

fn fixture_path_from_ref(
    trait_root: &camino::Utf8Path,
    output_ref: &str,
) -> crate::Result<camino::Utf8PathBuf> {
    let parsed = ctx_traits_core::reference::Reference::parse(output_ref).map_err(|_| {
        crate::Error::Command {
            message: format!("golden-render output must be a fixture:* ref, got {output_ref:?}"),
        }
    })?;
    if parsed.kind() != ctx_traits_core::reference::Kind::Fixture {
        return Err(crate::Error::Command {
            message: format!("golden-render output must be a fixture:* ref, got {output_ref:?}"),
        });
    }
    let relative = camino::Utf8Path::new(parsed.path());
    if relative.is_absolute() || parsed.path().contains('\\') {
        return Err(crate::Error::Command {
            message: format!("fixture path {output_ref:?} is not a safe relative path"),
        });
    }
    for component in relative.components() {
        if matches!(component, camino::Utf8Component::ParentDir) {
            return Err(crate::Error::Command {
                message: format!("fixture path {output_ref:?} must not contain '..' traversal"),
            });
        }
    }
    Ok(trait_root.join(relative))
}

fn print_eval_report(report: &ctx_traits_core::eval_run::Report, out: Option<&str>) {
    println!("ctx traits eval");
    println!("  trait: {}", report.trait_id);
    println!("  source-digest: {}", report.source_digest);
    println!("  report: {}", out.unwrap_or("not written"));
    println!("  records: {}", report.records.len());
    for record in &report.records {
        println!(
            "    {} ({}) {}",
            record.eval_id,
            record.variant.as_str(),
            match record.status {
                ctx_traits_core::eval_run::Status::Passed => "passed",
                ctx_traits_core::eval_run::Status::Failed => "failed",
                ctx_traits_core::eval_run::Status::Unsupported => "unsupported",
            }
        );
        println!("      input-digest: {}", record.input_digest);
        println!(
            "      output-digest: {}",
            record.output_digest.as_deref().unwrap_or("none")
        );
        println!(
            "      profile: {}",
            record.profile.as_deref().unwrap_or("none")
        );
        for finding in &record.findings {
            println!("      finding: {finding}");
        }
        for warning in &record.warnings {
            println!("      warning: {warning}");
        }
    }
    println!("  passed: {}", report.passed);
}

pub(crate) use handle_eval as handle;
