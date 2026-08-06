//! Common LLM-assistance boundary for trait authoring.
//!
//! Defines one reusable candidate envelope shared by `generate`, `refine`,
//! `compose`, and `import --llm-assisted`. Core creates request plans and
//! evaluates candidates deterministically; provider/model calls live only in
//! CLI/IO/adapters with explicit user command intent.
//!
//! This module is pure. It performs no provider, network, filesystem, or
//! process IO. Provider absence surfaces as an explicit unsupported-provider
//! status, never a silent no-op. All candidate content remains package
//! status=draft and machine trust=unreviewed until human review, regardless
//! of gate results.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{Finding, Severity, scan_hidden_content};
use crate::check::CheckReport;
use crate::digest::{Digest, canonical_digest, canonical_json};
use crate::encoding::{Encoding, decode, decode_trait_with_warnings, encode};
use crate::manifest::PackageStatus;
use crate::scaffold::{ExplainScaffold, ReviewScaffold};
use crate::source_map::SourceMap;
use crate::synth::Provenance;
use crate::r#trait::{Trait, TrustVerdict};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Which LLM-assisted authoring operation produced this candidate.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Operation {
    Generate,
    Refine,
    Compose,
    Import,
    Explain,
    Critique,
    GenerateEvals,
}

/// The narrowly typed result accepted from the advisory trait narrator.
/// Identity is repeated in the payload so a provider cannot substitute a
/// different selected trait while retaining a superficially valid response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitNarration {
    pub source_trait_id: String,
    pub canonical_digest: String,
    pub explanation: String,
}

/// Decode and audit a generated narration before it crosses into CLI/IO
/// cache or dashboard state. Advisory findings are retained by callers' audit
/// reporting; warning and critical findings block display and persistence.
pub fn validate_trait_narration(
    raw: &str,
    source_trait_id: &str,
    canonical_digest: &str,
) -> Result<TraitNarration, String> {
    let narration: TraitNarration = serde_json::from_str(raw)
        .map_err(|error| format!("trait narration must be valid JSON: {error}"))?;
    if narration.source_trait_id != source_trait_id {
        return Err("trait narration source-trait-id does not match request".to_string());
    }
    if narration.canonical_digest != canonical_digest {
        return Err("trait narration canonical-digest does not match request".to_string());
    }
    if narration.explanation.trim().is_empty() {
        return Err("trait narration explanation must not be empty".to_string());
    }
    if scan_hidden_content(
        &narration.explanation,
        source_trait_id,
        Some("trait-narration"),
    )
    .iter()
    .any(|finding| !matches!(finding.severity, Severity::Advisory))
    {
        return Err("trait narration failed hidden-content audit".to_string());
    }
    Ok(narration)
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Lifecycle status of an assist candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateStatus {
    DraftPending,
    Validated,
    Written,
    Blocked,
    Applied,
    UnsupportedProvider,
}

/// Write/Apply status for an assist candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteStatus {
    NotWritten,
    Written,
    Applied,
}

/// Deterministic gate that produced an assist diagnostic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Gate {
    Build,
    Parse,
    Normalize,
    Audit,
    Check,
    Drift,
}

/// Stable assist diagnostic codes emitted by deterministic gates.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DiagnosticCode {
    CdkBuildFailed,
    DegenerateCandidate,
    ParsedCandidateDigestUnavailable,
    ParseInvalidManifest,
    ParseShapeMismatch,
    ParseFailed,
    LifecycleNormalized,
    CanonicalDigestUnavailable,
    TargetMissing,
    TargetDrift,
    ImportTraitMissing,
    ImportTraitDrift,
    ImportReportMissing,
    ImportReportDrift,
}

impl DiagnosticCode {
    fn as_str(self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// One sanitized diagnostic from a gate evaluation.
///
/// Messages are redacted: they never echo raw model output, raw URLs from
/// deceptive links, raw hidden comments, or raw suspicious CSS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Diagnostic {
    pub gate: Gate,
    pub code: DiagnosticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Summary of post-model gate evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GateSummary {
    pub parse: GateState,
    pub normalize: GateState,
    pub audit: AuditGate,
    pub check: CheckGate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_error_code: Option<String>,
}

/// Boolean gate status for parse/normalize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GateState {
    pub ok: bool,
}

/// Audit gate status and findings summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuditGate {
    pub ok: bool,
    pub blocking: usize,
    pub info: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
}

/// Check gate status and section summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckGate {
    pub ok: bool,
    pub warnings: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_sections: Vec<String>,
}

impl GateSummary {
    fn unevaluated() -> Self {
        Self {
            parse: GateState { ok: false },
            normalize: GateState { ok: false },
            audit: AuditGate {
                ok: false,
                blocking: 0,
                info: 0,
                codes: Vec::new(),
            },
            check: CheckGate {
                ok: false,
                warnings: 0,
                failed_sections: Vec::new(),
            },
            validation_error_code: None,
        }
    }

    /// True when all gates have passed and the candidate is eligible for write.
    pub fn all_passed(&self) -> bool {
        self.parse.ok && self.normalize.ok && self.audit.ok && self.check.ok
    }
}

/// Candidate lifecycle state before/after deterministic normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CandidateLifecycle {
    pub status: PackageStatus,
    pub trust: TrustVerdict,
}

/// Provider/model identity for assist provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProviderReport {
    pub provider_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_parameters: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Deterministic request plan for an LLM-assisted operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RequestPlan {
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_trait_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_digests: Vec<Digest>,
    pub user_request: String,
    pub user_request_digest: Digest,
    pub prompt_context_digest: Digest,
    pub target_path: String,
}

/// Unified candidate envelope for LLM-assisted trait authoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Candidate {
    pub operation: Operation,
    pub request_plan: RequestPlan,
    pub provider: ProviderReport,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed_candidate_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_output_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_trait_id: Option<String>,
    pub output_format: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<CandidateLifecycle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<CandidateLifecycle>,

    pub gate_summary: GateSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_report: Option<CheckReport>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_evidence: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_capabilities: Vec<String>,

    pub status: CandidateStatus,
    pub write_status: WriteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written_path: Option<String>,
}

/// Result of evaluating a supplied candidate through all gates.
///
/// Carries the normalized trait and deterministic TOML output text so the
/// CLI write path never writes raw model output.
#[derive(Debug)]
pub struct CandidateEvaluation {
    pub candidate: Candidate,
    pub normalized_trait: Option<Trait>,
    pub normalized_output_text: Option<String>,
}

/// The rung ladder a single generate round climbs, in evaluation order. A
/// round stops at the first rung it fails; `converged` is true only once a
/// round clears `Audit`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Rung {
    Build,
    SynthNormalize,
    NonDegenerate,
    Check,
    Audit,
}

/// Outcome of evaluating one candidate through the rung ladder
/// (`assist_round::evaluate_round`). Reused by both the loop's in-round
/// command rung and the `--candidate` single-round path — the round-evidence
/// envelope proper (0066.2) extends this; this stays the minimal shape the
/// loop needs to guard on `converged`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RoundReport {
    /// The last rung evaluated: the failing rung when `converged` is false,
    /// or `Audit` (the ladder's final rung) when `converged` is true.
    pub rung: Rung,
    pub converged: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// Result of evaluating an advisory review scaffold through the candidate gate.
#[derive(Debug)]
pub struct ScaffoldEvaluation<T> {
    pub candidate: Candidate,
    pub scaffold: Option<T>,
    pub error: Option<String>,
}

/// Result of evaluating an advisory review scaffold through the candidate gate.
pub type ReviewScaffoldEvaluation = ScaffoldEvaluation<ReviewScaffold>;

/// Result of evaluating a deterministic explanation scaffold through the candidate gate.
pub type ExplainScaffoldEvaluation = ScaffoldEvaluation<ExplainScaffold>;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Request for planning an assist boundary.
#[derive(Debug, Clone)]
pub struct BoundaryRequest {
    pub operation: Operation,
    pub source_trait_ids: Vec<String>,
    pub source_paths: Vec<String>,
    pub source_digests: Vec<Digest>,
    pub user_request: String,
    pub model: Option<String>,
    pub target_path: String,
    pub provider_available: bool,
    pub context: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Build a deterministic assist boundary plan without calling a provider.
pub fn plan_assist_boundary(request: BoundaryRequest) -> crate::Result<Candidate> {
    let user_request_digest = Digest::source(&request.user_request);

    let context_json = canonical_json(&request.context)?;
    let prompt_context_digest = Digest::canonical(&context_json);

    let request_plan = RequestPlan {
        operation: request.operation,
        source_trait_ids: request.source_trait_ids,
        source_paths: request.source_paths,
        source_digests: request.source_digests,
        user_request: request.user_request,
        user_request_digest,
        prompt_context_digest,
        target_path: request.target_path,
    };

    let provider = if request.provider_available {
        ProviderReport {
            provider_available: true,
            provider_id: None,
            model_id: request.model.clone(),
            provider_parameters: BTreeMap::new(),
            reason: None,
        }
    } else {
        ProviderReport {
            provider_available: false,
            provider_id: None,
            model_id: None,
            provider_parameters: BTreeMap::new(),
            reason: Some(
                "no model provider adapter is implemented; no candidate was produced or written"
                    .to_string(),
            ),
        }
    };

    let warnings = if request.provider_available {
        Vec::new()
    } else {
        vec![
            "no model provider adapter is implemented; no candidate was produced or written"
                .to_string(),
        ]
    };

    let unsupported_capabilities = if request.provider_available {
        Vec::new()
    } else {
        vec!["llm-provider".to_string()]
    };

    let status = if request.provider_available {
        CandidateStatus::DraftPending
    } else {
        CandidateStatus::UnsupportedProvider
    };

    Ok(Candidate {
        operation: request.operation,
        request_plan,
        provider,
        raw_output_digest: None,
        parsed_candidate_digest: None,
        canonical_digest: None,
        normalized_output_digest: None,
        candidate_trait_id: None,
        output_format: "toml".to_string(),
        before: None,
        after: None,
        gate_summary: GateSummary::unevaluated(),
        diagnostics: Vec::new(),
        check_report: None,
        context_evidence: None,
        warnings,
        unsupported_capabilities,
        status,
        write_status: WriteStatus::NotWritten,
        written_path: None,
    })
}

/// Build a candidate boundary for a deterministic core operation.
///
/// Unlike assisted operations, this path has no provider dependency or
/// unsupported capability. It still records an explicit provider report so
/// JSON envelopes describe the execution boundary truthfully.
pub fn plan_deterministic_boundary(mut request: BoundaryRequest) -> crate::Result<Candidate> {
    request.provider_available = true;
    request.model = None;
    let mut candidate = plan_assist_boundary(request)?;
    candidate.provider.provider_available = false;
    candidate.provider.reason =
        Some("deterministic core operation; no provider required".to_string());
    Ok(candidate)
}

/// Evaluate a supplied candidate through deterministic gates.
///
/// Gate order:
/// 1. **Parse**: decode raw candidate as structured value + full trait validation.
/// 2. **Normalize**: candidate lifecycle carries no document-level status/trust
///    field; it is package status=draft and machine trust=unreviewed until
///    explicit human review, and canonical digests are computed.
/// 3. **Audit**: scan raw text and normalized TOML output for hidden/deceptive content.
/// 4. **Check**: left unevaluated until CLI supplies a candidate-mode `CheckReport`.
pub fn evaluate_supplied_candidate(
    mut candidate: Candidate,
    raw_output: &str,
    encoding: Encoding,
) -> CandidateEvaluation {
    let raw_output_digest = Digest::source(raw_output);
    candidate.raw_output_digest = Some(raw_output_digest);

    // Gate 1a: Parse — decode raw candidate as a structured value for digest.
    let parsed_value: serde_json::Value = match decode::<serde_json::Value>(encoding, raw_output) {
        Ok(v) => v,
        Err(e) => {
            let diag = sanitize_parse_error(&e);
            candidate.gate_summary = GateSummary {
                ..GateSummary::unevaluated()
            };
            candidate.gate_summary.validation_error_code = Some(diag.code.as_str().to_string());
            candidate.diagnostics.push(diag);
            candidate.status = CandidateStatus::Blocked;
            candidate
                .warnings
                .push("parse gate failed: candidate was rejected before write/apply".to_string());
            return CandidateEvaluation {
                candidate,
                normalized_trait: None,
                normalized_output_text: None,
            };
        }
    };

    let parsed_json = match canonical_json(&parsed_value) {
        Ok(text) => text,
        Err(_) => {
            let diag = Diagnostic {
                gate: Gate::Parse,
                code: DiagnosticCode::ParsedCandidateDigestUnavailable,
                field: None,
                message: "candidate parsed but parsed digest could not be canonicalized; raw detail redacted"
                    .to_string(),
            };
            candidate.gate_summary = GateSummary {
                parse: GateState { ok: true },
                ..GateSummary::unevaluated()
            };
            candidate.gate_summary.validation_error_code = Some(diag.code.as_str().to_string());
            candidate.diagnostics.push(diag);
            candidate.status = CandidateStatus::Blocked;
            candidate.warnings.push(
                "parse gate failed: parsed candidate digest could not be computed".to_string(),
            );
            return CandidateEvaluation {
                candidate,
                normalized_trait: None,
                normalized_output_text: None,
            };
        }
    };
    candidate.parsed_candidate_digest = Some(Digest::canonical(&parsed_json));

    // Gate 1b: Parse — decode through full validation pipeline.
    let decoded_trait = match decode_trait_with_warnings(encoding, raw_output) {
        Ok((t, decode_warnings)) => {
            candidate.warnings.extend(decode_warnings);
            t
        }
        Err(e) => {
            let diag = sanitize_parse_error(&e);
            candidate.gate_summary = GateSummary {
                ..GateSummary::unevaluated()
            };
            candidate.gate_summary.validation_error_code = Some(diag.code.as_str().to_string());
            candidate.diagnostics.push(diag);
            candidate.status = CandidateStatus::Blocked;
            candidate
                .warnings
                .push("parse gate failed: candidate was rejected before write/apply".to_string());
            return CandidateEvaluation {
                candidate,
                normalized_trait: None,
                normalized_output_text: None,
            };
        }
    };

    candidate.candidate_trait_id = Some(decoded_trait.id.as_str().to_string());

    // Gate 2: Normalize. The canonical document carries no status/trust
    // field to force (Group 95, 2026-07-19), so there is nothing to mutate
    // here — `before` is not applicable. The structural invariant is that
    // LLM-assisted candidate output is never itself lifecycle-bearing: it is
    // only eligible for package status `ready` or a machine trust `verified`
    // record through an explicit, separate human review action.
    let normalized = decoded_trait.clone();

    candidate.before = None;
    candidate.after = Some(CandidateLifecycle {
        status: PackageStatus::Draft,
        trust: TrustVerdict::Unreviewed,
    });

    candidate.diagnostics.push(Diagnostic {
        gate: Gate::Normalize,
        code: DiagnosticCode::LifecycleNormalized,
        field: None,
        message: "candidate lifecycle normalized: assisted output carries no document-level \
                   status/trust field; it remains package status=draft and trust=unreviewed \
                   until explicit human review"
            .to_string(),
    });

    // Compute correct digests from the normalized trait.
    let canonical_digest = match canonical_digest(&normalized) {
        Ok(d) => d,
        Err(_) => {
            let diag = Diagnostic {
                gate: Gate::Normalize,
                code: DiagnosticCode::CanonicalDigestUnavailable,
                field: None,
                message: "normalized candidate canonical digest could not be computed; raw detail redacted"
                    .to_string(),
            };
            candidate.gate_summary = GateSummary {
                parse: GateState { ok: true },
                normalize: GateState { ok: false },
                validation_error_code: Some(diag.code.as_str().to_string()),
                ..GateSummary::unevaluated()
            };
            candidate.diagnostics.push(diag);
            candidate.status = CandidateStatus::Blocked;
            candidate.write_status = WriteStatus::NotWritten;
            candidate
                .warnings
                .push("normalize gate failed: canonical digest could not be computed".to_string());
            return CandidateEvaluation {
                candidate,
                normalized_trait: None,
                normalized_output_text: None,
            };
        }
    };
    candidate.canonical_digest = Some(canonical_digest);

    // Encode normalized trait to deterministic TOML.
    let normalized_toml = match encode(Encoding::Toml, &normalized) {
        Ok(text) => text,
        Err(e) => {
            let diag = sanitize_parse_error(&e);
            candidate.gate_summary = GateSummary {
                parse: GateState { ok: true },
                normalize: GateState { ok: false },
                ..GateSummary::unevaluated()
            };
            candidate.diagnostics.push(diag);
            candidate.status = CandidateStatus::Blocked;
            candidate
                .warnings
                .push("normalize gate failed: candidate could not be encoded to TOML".to_string());
            return CandidateEvaluation {
                candidate,
                normalized_trait: None,
                normalized_output_text: None,
            };
        }
    };

    candidate.normalized_output_digest = Some(Digest::source(&normalized_toml));
    candidate.gate_summary.normalize.ok = true;

    // Gate 3: Audit — scan both raw text and normalized TOML.
    let raw_findings = scan_hidden_content(raw_output, normalized.id.as_str(), None);
    let toml_findings = scan_hidden_content(&normalized_toml, normalized.id.as_str(), None);
    let all_findings: Vec<_> = raw_findings.into_iter().chain(toml_findings).collect();
    let blocking_count = all_findings
        .iter()
        .filter(|f| !matches!(f.severity, Severity::Advisory))
        .count();
    let info_count = all_findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Advisory))
        .count();
    let audit_ok = blocking_count == 0;
    let audit_codes: Vec<String> = all_findings
        .iter()
        .filter(|f| !matches!(f.severity, Severity::Advisory))
        .map(|f| f.code.as_str().to_string())
        .collect();

    candidate.gate_summary.parse.ok = true;
    candidate.gate_summary.audit.ok = audit_ok;
    candidate.gate_summary.audit.blocking = blocking_count;
    candidate.gate_summary.audit.info = info_count;
    candidate.gate_summary.audit.codes = audit_codes;

    // Gate 4 (check) is CLI-owned because resource/render evidence can require
    // filesystem context. Keep false until attach_check_report is called.
    candidate.gate_summary.check.ok = false;

    if !audit_ok {
        candidate.status = CandidateStatus::Blocked;
        candidate.warnings.push(
            "audit gate failed: hidden or deceptive content detected in candidate".to_string(),
        );
    } else {
        // Pre-check status: parse+normalize+audit pass; check pending.
        candidate.status = CandidateStatus::Validated;
    }

    CandidateEvaluation {
        candidate,
        normalized_trait: Some(normalized),
        normalized_output_text: Some(normalized_toml),
    }
}

/// Evaluate an advisory critique scaffold through the shared candidate envelope.
///
/// Unlike authoring candidates, critiques are evidence and never decode into or
/// normalize a writable trait. Their source-map validation remains exact.
pub fn evaluate_supplied_review_scaffold(
    candidate: Candidate,
    raw_output: &str,
    source_map: &SourceMap,
) -> ReviewScaffoldEvaluation {
    let mut evaluation = evaluate_supplied_scaffold(
        candidate,
        raw_output,
        "review-scaffold",
        ScaffoldFailureMessages {
            invalid_json: "review scaffold must be valid JSON",
            canonical_digest: "review scaffold parsed but its canonical digest could not be computed",
            shape_mismatch: "review scaffold must match the required shape",
            validation: "review scaffold failed source-map validation",
            warning: "parse gate failed: review scaffold was rejected",
        },
        |scaffold: &ReviewScaffold| scaffold.validate(source_map),
    );
    if let Some(scaffold) = &evaluation.scaffold {
        evaluation.candidate.candidate_trait_id = Some(scaffold.source_trait_id.clone());
    }
    evaluation
}

/// Evaluate an explanation scaffold through the same structured candidate flow.
pub fn evaluate_supplied_explain_scaffold(
    candidate: Candidate,
    raw_output: &str,
    source_map: &SourceMap,
) -> ExplainScaffoldEvaluation {
    let mut evaluation = evaluate_supplied_scaffold(
        candidate,
        raw_output,
        "explain-scaffold",
        ScaffoldFailureMessages {
            invalid_json: "explain scaffold must be valid JSON",
            canonical_digest: "explain scaffold parsed but its canonical digest could not be computed",
            shape_mismatch: "explain scaffold must match the required shape",
            validation: "explain scaffold failed source-map validation",
            warning: "parse gate failed: explain scaffold was rejected",
        },
        |scaffold: &ExplainScaffold| scaffold.validate(source_map),
    );
    if let Some(scaffold) = &evaluation.scaffold {
        evaluation.candidate.candidate_trait_id = Some(scaffold.trait_id.clone());
    }
    evaluation
}

fn evaluate_supplied_scaffold<T>(
    mut candidate: Candidate,
    raw_output: &str,
    label: &str,
    messages: ScaffoldFailureMessages,
    validate: impl FnOnce(&T) -> crate::Result<()>,
) -> ScaffoldEvaluation<T>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    candidate.raw_output_digest = Some(Digest::source(raw_output));
    candidate.output_format = "json".to_string();

    // Audit raw output before parsing so malformed scaffolds cannot bypass the
    // candidate boundary or lose their audit evidence.
    let findings = scan_hidden_content(raw_output, label, None);
    let blocking = findings
        .iter()
        .filter(|finding| !matches!(finding.severity, Severity::Advisory))
        .count();
    candidate.gate_summary.audit.ok = blocking == 0;
    candidate.gate_summary.audit.blocking = blocking;
    candidate.gate_summary.audit.info = findings.len() - blocking;
    candidate.gate_summary.audit.codes = findings
        .iter()
        .filter(|finding| !matches!(finding.severity, Severity::Advisory))
        .map(|finding| finding.code.as_str().to_string())
        .collect();
    candidate.gate_summary.check.ok = false;

    let value: serde_json::Value = match serde_json::from_str(raw_output) {
        Ok(value) => value,
        Err(_) => {
            return failed_scaffold(
                candidate,
                label,
                DiagnosticCode::ParseFailed,
                messages.invalid_json,
                messages.warning,
            );
        }
    };
    let canonical = match canonical_json(&value) {
        Ok(canonical) => canonical,
        Err(_) => {
            return failed_scaffold(
                candidate,
                label,
                DiagnosticCode::ParsedCandidateDigestUnavailable,
                messages.canonical_digest,
                messages.warning,
            );
        }
    };
    candidate.parsed_candidate_digest = Some(Digest::canonical(&canonical));
    let scaffold: T = match serde_json::from_value(value) {
        Ok(scaffold) => scaffold,
        Err(_) => {
            return failed_scaffold(
                candidate,
                label,
                DiagnosticCode::ParseShapeMismatch,
                messages.shape_mismatch,
                messages.warning,
            );
        }
    };
    if validate(&scaffold).is_err() {
        return failed_scaffold(
            candidate,
            label,
            DiagnosticCode::ParseShapeMismatch,
            messages.validation,
            messages.warning,
        );
    }
    candidate.canonical_digest = Some(Digest::canonical(&canonical));
    candidate.normalized_output_digest = Some(Digest::canonical(&canonical));
    candidate.gate_summary.parse.ok = true;
    candidate.gate_summary.normalize.ok = true;
    candidate.status = if candidate.gate_summary.audit.ok {
        CandidateStatus::Validated
    } else {
        CandidateStatus::Blocked
    };
    ScaffoldEvaluation {
        candidate,
        scaffold: Some(scaffold),
        error: None,
    }
}

struct ScaffoldFailureMessages {
    invalid_json: &'static str,
    canonical_digest: &'static str,
    shape_mismatch: &'static str,
    validation: &'static str,
    warning: &'static str,
}

fn failed_scaffold<T>(
    mut candidate: Candidate,
    label: &str,
    code: DiagnosticCode,
    message: &str,
    warning: &str,
) -> ScaffoldEvaluation<T> {
    candidate.gate_summary.validation_error_code = Some(code.as_str().to_string());
    candidate.diagnostics.push(Diagnostic {
        gate: Gate::Parse,
        code,
        field: Some(label.to_string()),
        message: message.to_string(),
    });
    candidate.status = CandidateStatus::Blocked;
    candidate.warnings.push(warning.to_string());
    ScaffoldEvaluation {
        candidate,
        scaffold: None,
        error: Some(message.to_string()),
    }
}

/// Audit structured wrapper output in addition to the extracted trait.
///
/// Meta-traits decode a proposed trait from a richer scaffold. The scaffold is
/// still untrusted model output, so its fields must participate in the same
/// candidate audit gate before they can be reported as evidence.
pub fn audit_wrapper_output(
    mut candidate: Candidate,
    raw_output: &str,
    trait_id: &str,
) -> Candidate {
    let findings = scan_hidden_content(raw_output, trait_id, None);
    let blocking = findings
        .iter()
        .filter(|finding| !matches!(finding.severity, Severity::Advisory))
        .count();
    let info = findings
        .iter()
        .filter(|finding| matches!(finding.severity, Severity::Advisory))
        .count();
    let mut codes = findings
        .iter()
        .filter(|finding| !matches!(finding.severity, Severity::Advisory))
        .map(|finding| finding.code.as_str().to_string())
        .collect::<Vec<_>>();
    candidate.gate_summary.audit.blocking += blocking;
    candidate.gate_summary.audit.info += info;
    candidate.gate_summary.audit.codes.append(&mut codes);
    candidate.gate_summary.audit.codes.sort();
    candidate.gate_summary.audit.codes.dedup();
    candidate.gate_summary.audit.ok = candidate.gate_summary.audit.blocking == 0;
    if !candidate.gate_summary.audit.ok {
        candidate.status = CandidateStatus::Blocked;
        candidate.warnings.push(
            "audit gate failed: hidden or deceptive content detected in candidate wrapper"
                .to_string(),
        );
    }
    candidate
}

/// Attach a candidate-mode check report and update gate status.
pub fn attach_check_report(mut candidate: Candidate, report: CheckReport) -> Candidate {
    let sanitized_report = sanitize_check_report_for_assist(report);
    let warning_count = sanitized_report.warnings.len();
    let mut failed_sections: Vec<String> = sanitized_report
        .sections
        .iter()
        .filter(|s| !s.ok)
        .map(|s| s.name.clone())
        .collect();
    failed_sections.sort();
    failed_sections.dedup();
    let check_ok = failed_sections.is_empty() && sanitized_report.passed;

    candidate.gate_summary.check.ok = check_ok;
    candidate.gate_summary.check.warnings = warning_count;
    candidate.gate_summary.check.failed_sections = failed_sections;
    candidate.check_report = Some(sanitized_report);

    if !check_ok && candidate.status == CandidateStatus::Validated {
        candidate.status = CandidateStatus::Blocked;
        candidate
            .warnings
            .push("check gate failed: candidate-mode check found blocking issues".to_string());
    }

    candidate
}

/// Mark a candidate as written (new draft package or --out path).
/// Status becomes `Written`, not `Applied`.
pub fn mark_written(mut candidate: Candidate, path: &str) -> Candidate {
    candidate.write_status = WriteStatus::Written;
    candidate.written_path = Some(path.to_string());
    candidate.status = CandidateStatus::Written;
    candidate
}

/// Mark a candidate as applied to existing canonical source (refine --apply only).
pub fn mark_applied(mut candidate: Candidate, path: &str) -> Candidate {
    candidate.write_status = WriteStatus::Applied;
    candidate.written_path = Some(path.to_string());
    candidate.status = CandidateStatus::Applied;
    candidate
}

/// Attach operation-specific context evidence to the candidate.
pub fn with_context_evidence(mut candidate: Candidate, evidence: serde_json::Value) -> Candidate {
    candidate.context_evidence = Some(evidence);
    candidate
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Sanitize a parse/validation error into a safe diagnostic.
///
/// Preserves field paths but redacts raw error messages that may contain
/// user-authored suspicious strings or hidden/deceptive payloads.
fn sanitize_parse_error(e: &crate::Error) -> Diagnostic {
    let (field, code) = match e {
        crate::Error::Manifest(crate::manifest::Error::InvalidField { field_path, .. })
        | crate::Error::Encoding(crate::encoding::Error::InvalidField { field_path, .. })
        | crate::Error::Trait(crate::r#trait::Error::InvalidField { field_path, .. })
        | crate::Error::Model(crate::procedure::Error::InvalidField { field_path, .. }) => (
            Some(field_path.clone()),
            DiagnosticCode::ParseInvalidManifest,
        ),
        crate::Error::Manifest(crate::manifest::Error::ShapeMismatch { field_path, .. })
        | crate::Error::Encoding(crate::encoding::Error::ShapeMismatch { field_path, .. })
        | crate::Error::Shared(crate::shared::Error::ShapeMismatch { field_path, .. })
        | crate::Error::Model(crate::procedure::Error::ShapeMismatch { field_path, .. }) => {
            (Some(field_path.clone()), DiagnosticCode::ParseShapeMismatch)
        }
        crate::Error::Parse(parse_error) if parse_error.is_shape_mismatch() => (
            Some(parse_error.detail_field_path()),
            DiagnosticCode::ParseShapeMismatch,
        ),
        crate::Error::Parse(parse_error) => (
            Some(parse_error.detail_field_path()),
            DiagnosticCode::ParseInvalidManifest,
        ),
        _ => (None, DiagnosticCode::ParseFailed),
    };

    Diagnostic {
        gate: Gate::Parse,
        code,
        field,
        message: "candidate failed to parse; raw parser detail redacted".to_string(),
    }
}

fn sanitize_check_report_for_assist(mut report: CheckReport) -> CheckReport {
    report.trait_id = safe_assist_identifier(&report.trait_id, "assist-candidate");
    report.validation_error = report
        .validation_error
        .map(|_| "candidate check validation failed; raw detail redacted".to_string());

    for finding in &mut report.audit_findings {
        sanitize_audit_finding_for_assist(finding);
    }

    for section in &mut report.sections {
        section.name = safe_assist_identifier(&section.name, "redacted-section");
        section.summary = safe_assist_text(&section.summary, "check-section-summary");
    }
    report.sections.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.summary.cmp(&b.summary))
            .then(a.ok.cmp(&b.ok))
    });
    report.sections.dedup();

    for warning in &mut report.warnings {
        warning.code = safe_assist_identifier(&warning.code, "redacted-warning-code");
        warning.field = warning
            .field
            .as_deref()
            .map(|field| safe_assist_field(field, "redacted-field"));
        warning.message = safe_assist_text(&warning.message, "check-warning-message");
    }
    report.warnings.sort_by(|a, b| {
        a.section
            .cmp(&b.section)
            .then(a.code.cmp(&b.code))
            .then(a.field.cmp(&b.field))
            .then(a.message.cmp(&b.message))
    });
    report.warnings.dedup();

    for drift in &mut report.drift {
        drift.expected = safe_assist_digest_text(&drift.expected);
        drift.actual = drift.actual.as_deref().map(safe_assist_digest_text);
        drift.summary = safe_assist_text(&drift.summary, "check-drift-summary");
    }

    report.unsupported_capabilities = report
        .unsupported_capabilities
        .into_iter()
        .map(|capability| safe_assist_identifier(&capability, "redacted-capability"))
        .collect();
    report.unsupported_capabilities.sort();
    report.unsupported_capabilities.dedup();

    for provenance in &mut report.synth_provenance {
        sanitize_synth_provenance_for_assist(provenance);
    }

    report
}

fn safe_assist_identifier(value: &str, fallback: &str) -> String {
    if value.is_empty() || value.len() > 80 || contains_disallowed_assist_marker(value) {
        return fallback.to_string();
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return fallback.to_string();
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return fallback.to_string();
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.') {
        return fallback.to_string();
    }
    if has_blocking_hidden_content(value, "assist-identifier") {
        return fallback.to_string();
    }
    value.to_string()
}

fn safe_assist_field(value: &str, fallback: &str) -> String {
    if value.is_empty() || value.len() > 160 || contains_disallowed_assist_marker(value) {
        return fallback.to_string();
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '[' | ']'))
    {
        return fallback.to_string();
    }
    if has_blocking_hidden_content(value, "assist-field") {
        return fallback.to_string();
    }
    value.to_string()
}

fn safe_assist_text(value: &str, field: &str) -> String {
    if value.len() > 240 {
        return "redacted long assist check evidence".to_string();
    }
    let lowered = value.to_ascii_lowercase();
    if has_blocking_hidden_content(value, field)
        || lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("data:")
        || lowered.contains("<!--")
        || lowered.contains("<span")
        || lowered.contains("<style")
        || lowered.contains("$(")
        || value.contains('`')
    {
        return "redacted hidden/deceptive content in assist check evidence".to_string();
    }
    value.to_string()
}

fn safe_assist_digest(value: &Digest) -> Digest {
    if Digest::parse(value.as_str()).is_ok() {
        value.clone()
    } else {
        Digest::from_unvalidated("redacted-non-digest")
    }
}

fn safe_assist_digest_text(value: &str) -> String {
    if Digest::parse(value).is_ok() {
        value.to_string()
    } else {
        "redacted-non-digest".to_string()
    }
}

fn safe_assist_optional_digest(value: Option<&Digest>) -> Option<Digest> {
    value.map(safe_assist_digest)
}

fn sanitize_audit_finding_for_assist(finding: &mut Finding) {
    let code = finding.code.as_str();
    let line = finding
        .line
        .map(|line| line.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    finding.trait_id = "assist-candidate".to_string();
    if finding.path.is_some() {
        finding.path = Some("assist-candidate".to_string());
    }
    finding.message = format!("{code} finding at line {line}; raw detail redacted");
    finding.remediation = "review and remove hidden/deceptive content before retrying".to_string();
}

fn sanitize_synth_provenance_for_assist(provenance: &mut Provenance) {
    provenance.generator_package = provenance
        .generator_package
        .as_deref()
        .map(|value| safe_assist_identifier(value, "redacted-generator"));
    provenance.generator_version = provenance
        .generator_version
        .as_deref()
        .map(|value| safe_assist_field(value, "redacted-version"));
    provenance.source_path = provenance
        .source_path
        .as_deref()
        .map(|value| safe_assist_field(value, "redacted-source-path"));
    provenance.source_path_digest =
        safe_assist_optional_digest(provenance.source_path_digest.as_ref());
    provenance.draft_digest = safe_assist_digest(&provenance.draft_digest);
    provenance.canonical_digest = safe_assist_digest(&provenance.canonical_digest);
    provenance.output_digest = safe_assist_digest(&provenance.output_digest);
    provenance.warnings = provenance
        .warnings
        .iter()
        .map(|warning| safe_assist_text(warning, "synth-provenance-warning"))
        .collect();
    provenance.warnings.sort();
    provenance.warnings.dedup();
}

fn contains_disallowed_assist_marker(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    value.chars().any(|ch| {
        ch.is_ascii_whitespace()
            || ch.is_ascii_control()
            || matches!(ch, '/' | '\\' | '<' | '>' | '"' | '\'' | '$' | '(' | ')')
    }) || lowered.contains("://")
        || lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("data:")
}

fn has_blocking_hidden_content(value: &str, field: &str) -> bool {
    scan_hidden_content(value, "assist-report", Some(field))
        .iter()
        .any(|finding| !matches!(finding.severity, Severity::Advisory))
}
