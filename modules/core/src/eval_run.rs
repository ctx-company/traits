//! Deterministic eval runner result model.
//!
//! Core evaluates only supplied data. It does not read fixtures, render files,
//! run test harnesses, call models, call agents, or update locks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::Severity;
use crate::digest::Digest;
use crate::r#trait::{Eval, EvalResult, EvalVariant, Trait};

/// Fixture/render evidence supplied by the CLI for one golden-render eval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct GoldenRenderEvidence {
    pub eval_id: String,
    pub profile: String,
    pub actual_text: String,
    pub actual_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_ref: Option<String>,
    pub fixture_available: bool,
}

/// Input data for a deterministic eval run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Request {
    pub source_text: String,
    pub source_digest: Digest,
    pub canonical_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_manifest_digest: Option<Digest>,
    pub evaluated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_eval_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_variant: Option<EvalVariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lint_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub golden_render: Vec<GoldenRenderEvidence>,
}

/// Eval outcome status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Status {
    Passed,
    Failed,
    Unsupported,
}

/// One eval execution record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CaseOutcome {
    pub eval_id: String,
    pub variant: EvalVariant,
    pub status: Status,
    pub input_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<EvalResult>,
}

/// Complete eval report for selected declarations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Report {
    pub trait_id: String,
    pub source_digest: Digest,
    pub canonical_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_manifest_digest: Option<Digest>,
    pub evaluated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<CaseOutcome>,
    pub passed: bool,
}

impl Report {
    /// Passing generated evidence sorted for lockfile insertion.
    pub fn passing_results(&self) -> Vec<EvalResult> {
        let mut results: Vec<EvalResult> = self
            .records
            .iter()
            .filter_map(|record| record.result.clone())
            .collect();
        results.sort();
        results
    }
}

/// Run selected deterministic eval declarations.
pub fn run_evals(trait_ref: &Trait, request: Request) -> crate::Result<Report> {
    let mut records = Vec::new();
    for eval in &trait_ref.evals {
        if !selected(eval, &request) {
            continue;
        }
        let record = match eval.variant {
            EvalVariant::Documentation => documentation_eval(trait_ref, eval, &request)?,
            EvalVariant::Lint => lint_eval(trait_ref, eval, &request)?,
            EvalVariant::GoldenRender => golden_render_eval(eval, &request)?,
            EvalVariant::Behavioral | EvalVariant::Runtime => unsupported_eval(eval, &request),
        };
        records.push(record);
    }

    if !request.selected_eval_ids.is_empty() {
        let selected: std::collections::BTreeSet<&str> = request
            .selected_eval_ids
            .iter()
            .map(String::as_str)
            .collect();
        let present: std::collections::BTreeSet<&str> = trait_ref
            .evals
            .iter()
            .map(|eval| eval.id.as_str())
            .collect();
        for id in selected.difference(&present) {
            records.push(CaseOutcome {
                eval_id: (*id).to_string(),
                variant: request
                    .selected_variant
                    .clone()
                    .unwrap_or(EvalVariant::Lint),
                status: Status::Failed,
                input_digest: request.source_digest.clone(),
                output_digest: None,
                profile: None,
                findings: vec![format!("selected eval id {id:?} is not declared")],
                warnings: Vec::new(),
                result: None,
            });
        }
    }

    records.sort_by(|a, b| {
        a.eval_id
            .cmp(&b.eval_id)
            .then(a.variant.as_str().cmp(b.variant.as_str()))
    });
    let passed =
        !records.is_empty() && records.iter().all(|record| record.status == Status::Passed);

    Ok(Report {
        trait_id: trait_ref.id.as_str().to_string(),
        source_digest: request.source_digest,
        canonical_digest: request.canonical_digest,
        resource_manifest_digest: request.resource_manifest_digest,
        evaluated_at: request.evaluated_at,
        records,
        passed,
    })
}

fn selected(eval: &Eval, request: &Request) -> bool {
    let id_matches = request.selected_eval_ids.is_empty()
        || request.selected_eval_ids.iter().any(|id| id == &eval.id);
    let variant_matches = request
        .selected_variant
        .as_ref()
        .is_none_or(|variant| variant == &eval.variant);
    id_matches && variant_matches
}

fn documentation_eval(
    trait_ref: &Trait,
    eval: &Eval,
    request: &Request,
) -> crate::Result<CaseOutcome> {
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    if trait_ref.scenarios.is_empty() {
        warnings.push("no declared scenarios to review".to_string());
    }
    if trait_ref.evals.is_empty() {
        warnings.push("no declared evals to review".to_string());
    }
    if trait_ref.prompts.is_empty() {
        warnings.push("no prompt declarations to review".to_string());
    }
    if let Some(proc) = trait_ref.procedure.as_ref() {
        if proc.description.trim().len() < 12 {
            findings.push("procedure description is too short for review".to_string());
        }
    } else {
        warnings.push("no procedure declaration to review".to_string());
    }
    for port in trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, crate::r#trait::PortDirection::Output))
    {
        if port.description.trim().len() < 12 {
            findings.push(format!(
                "output port {:?} description is too short",
                port.id
            ));
        }
    }
    for scenario in &trait_ref.scenarios {
        if scenario.input.as_deref().is_none_or(str::is_empty) {
            warnings.push(format!("scenario {:?} has no input text", scenario.id));
        }
        if scenario.output.as_deref().is_none_or(str::is_empty) {
            warnings.push(format!("scenario {:?} has no output text", scenario.id));
        }
    }
    let output_digest = digest_report_parts(&findings, &warnings)?;
    let status = if findings.is_empty() {
        Status::Passed
    } else {
        Status::Failed
    };
    Ok(record(
        eval,
        status,
        request,
        Some(output_digest),
        None,
        findings,
        warnings,
    ))
}

fn lint_eval(trait_ref: &Trait, eval: &Eval, request: &Request) -> crate::Result<CaseOutcome> {
    let mut findings = Vec::new();
    let mut warnings = request.lint_warnings.clone();
    let audit =
        crate::audit::scan_hidden_content(&request.source_text, trait_ref.id.as_str(), None);
    for finding in audit {
        let text = format!("{:?}: {}", finding.code, finding.message);
        if matches!(finding.severity, Severity::Advisory) {
            warnings.push(text);
        } else {
            findings.push(text);
        }
    }
    let scenario_ids: std::collections::BTreeSet<&str> =
        trait_ref.scenarios.iter().map(|s| s.id.as_str()).collect();
    for warning in crate::r#trait::eval::audit_evals(&trait_ref.evals, &scenario_ids) {
        warnings.push(format!("eval {:?}: {}", warning.eval_id, warning.message));
    }
    for warning in crate::r#trait::scenario::audit_scenarios(&trait_ref.scenarios) {
        warnings.push(format!(
            "scenario {:?}: {}",
            warning.scenario_id, warning.message
        ));
    }
    warnings.sort();
    warnings.dedup();
    let output_digest = digest_report_parts(&findings, &warnings)?;
    let status = if findings.is_empty() {
        Status::Passed
    } else {
        Status::Failed
    };
    Ok(record(
        eval,
        status,
        request,
        Some(output_digest),
        None,
        findings,
        warnings,
    ))
}

fn golden_render_eval(eval: &Eval, request: &Request) -> crate::Result<CaseOutcome> {
    let evidence = request
        .golden_render
        .iter()
        .find(|evidence| evidence.eval_id == eval.id);
    let Some(evidence) = evidence else {
        return Ok(record(
            eval,
            Status::Failed,
            request,
            None,
            None,
            vec!["missing render/fixture evidence for golden-render eval".to_string()],
            Vec::new(),
        ));
    };

    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    if !evidence.fixture_available {
        findings.push(format!(
            "expected fixture is unavailable: {}",
            evidence
                .fixture_ref
                .as_deref()
                .map_or("none", |value| value)
        ));
    } else if let Some(expected_text) = evidence.expected_text.as_ref() {
        if evidence.actual_text != *expected_text {
            findings.push("rendered text differs from expected fixture text".to_string());
        }
    } else if let Some(expected_digest) = evidence.expected_digest.as_ref() {
        if &evidence.actual_digest != expected_digest {
            findings.push("rendered text digest differs from expected digest".to_string());
        }
    } else {
        findings.push("golden-render eval has no expected text or digest evidence".to_string());
    }
    if let Some(ref output) = eval.output {
        warnings.push(format!("fixture: {output}"));
    }
    let status = if findings.is_empty() {
        Status::Passed
    } else {
        Status::Failed
    };
    Ok(record(
        eval,
        status,
        request,
        Some(evidence.actual_digest.clone()),
        Some(evidence.profile.clone()),
        findings,
        warnings,
    ))
}

fn unsupported_eval(eval: &Eval, request: &Request) -> CaseOutcome {
    CaseOutcome {
        eval_id: eval.id.clone(),
        variant: eval.variant.clone(),
        status: Status::Unsupported,
        input_digest: request.source_digest.clone(),
        output_digest: None,
        profile: None,
        findings: vec![format!(
            "eval variant {} requires explicit runtime ledger support and does not call a model/provider",
            eval.variant.as_str()
        )],
        warnings: Vec::new(),
        result: None,
    }
}

fn record(
    eval: &Eval,
    status: Status,
    request: &Request,
    output_digest: Option<Digest>,
    profile: Option<String>,
    findings: Vec<String>,
    warnings: Vec<String>,
) -> CaseOutcome {
    let result = if status == Status::Passed {
        Some(EvalResult {
            eval_id: eval.id.clone(),
            evaluated_at: request.evaluated_at.clone(),
            input_digest: request.source_digest.clone(),
            output_digest: match output_digest.clone() {
                Some(digest) => digest,
                None => request.source_digest.clone(),
            },
            profile: profile.clone(),
            warnings: warnings.clone(),
        })
    } else {
        None
    };
    CaseOutcome {
        eval_id: eval.id.clone(),
        variant: eval.variant.clone(),
        status,
        input_digest: request.source_digest.clone(),
        output_digest,
        profile,
        findings,
        warnings,
        result,
    }
}

fn digest_report_parts(findings: &[String], warnings: &[String]) -> crate::Result<Digest> {
    let text = serde_json::to_string(&(findings, warnings))
        .map_err(|e| crate::digest::serialization("eval.report", "eval report parts", e))?;
    Ok(Digest::source(&text))
}
