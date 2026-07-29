//! JSON ABI entry surface for pure core operations.
//!
//! Functions in this module accept UTF-8 JSON text and return UTF-8 JSON text.
//! They do not read files, fetch Git repositories, spawn processes, inspect host
//! configuration, perform host rendering, or persist cache data. Inputs must contain the
//! already-loaded source text/evidence needed by core.

use ctx_traits_core::cache::{
    CacheArtifactKey, StoredCacheArtifact, compare_cache_status, plan_cache_prune_from_stored,
    plan_cache_rebuild,
};
use ctx_traits_core::context::ledger::{CurrentRender, Ledger};
use ctx_traits_core::context::pack::plan_context_pack;
use ctx_traits_core::digest::{Digest, canonical_digest, canonical_json};
use ctx_traits_core::encoding::{self, Encoding};
use ctx_traits_core::import::plan::{
    AgentSkillsImportPlan, AgentSkillsImportRequest, ImportReport, ImportRequest,
    create_import_report, plan_agent_skills_import,
};
use ctx_traits_core::model_view::compile_model_view_with_evidence;
use ctx_traits_core::procedure::session::{
    AgentAssignment, CallResponse, CallSubmission, CallerProvenance, Provenance, Session,
    SessionId, deterministic_run_id, refresh_run_session, run_initial_values_from_json,
    start_run_session, submit_run_call,
};
use ctx_traits_core::render::{ExtendedRenderProfile, plan_render};
use ctx_traits_core::resolve::{
    CandidateEstimate, IndexRejection, Request as ResolveRequest, Response as ResolveResponse,
    resolve,
};
use ctx_traits_core::resource_plan::plan_resource_inclusion;
use ctx_traits_core::response::{
    CapabilityReport, Envelope, JsonAbiErrorCodes, ResponseError,
    decode_then as response_decode_then,
    decode_then_with_warnings as response_decode_then_with_warnings,
    envelope_to_json as response_envelope_to_json,
};
use ctx_traits_core::synth::{Request as SynthRequest, Response as SynthResponse, synthesize};
use ctx_traits_core::r#trait::Trait;
use ctx_traits_core::r#trait::activation::{Request as ActivationRequest, explain};
use ctx_traits_core::r#trait::composition::plan;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// ABI schema version used by this crate's request and response values.
pub const ABI_SCHEMA_VERSION: &str = "0.1.0";

const ABI_ERROR_CODES: JsonAbiErrorCodes = JsonAbiErrorCodes {
    decode_code: "abi.decode-request",
    decode_message: "failed to decode ABI request",
    serialize_code: "abi.serialize-envelope",
    serialize_message: "failed to serialize ABI envelope",
};

/// One already-loaded trait source document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraitDocument {
    pub encoding: Encoding,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    /// Host-resolved package manifest `[package].status` (`"draft"` or
    /// `"ready"`). wasm-core has no filesystem access to resolve this
    /// itself — the canonical trait document carries no status field
    /// (Group 95, 2026-07-19) — so the host adapter reads the package
    /// manifest and supplies it. Absent/unrecognized values default to
    /// `"draft"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Host-resolved machine trust store verdict (`"verified"` or
    /// `"blocked"`) for this document's canonical digest. Absent/
    /// unrecognized values default to `"unreviewed"`, matching "no
    /// trust-store record means unreviewed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,
    /// The canonical digest `trust` was resolved against. wasm-core has no
    /// trust-store access of its own — it can only take the host's word for
    /// `trust` — so it re-derives this document's *current* canonical
    /// digest and refuses to honor `trust` unless it matches this field
    /// exactly. Without this binding a verdict resolved for one canonical
    /// digest (e.g. cached from an earlier call, or asserted for a
    /// different document by a confused/malicious host) could be replayed
    /// against different current content. Absent defaults the verdict to
    /// unreviewed, same as a digest mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_digest: Option<String>,
}

impl TraitDocument {
    fn decode(&self) -> ctx_traits_core::Result<Trait> {
        encoding::decode_trait(self.encoding, &self.text)
    }

    /// Decode, also returning legacy-field deprecation warnings (retired
    /// top-level `status`/`trust` keys) so ABI callers can surface them on
    /// the response envelope instead of silently discarding them the way
    /// `decode` does.
    fn decode_with_warnings(
        &self,
    ) -> ctx_traits_core::Result<(Trait, Vec<ctx_traits_core::response::Warning>)> {
        let (trait_ref, warnings) =
            encoding::decode_trait_with_warnings(self.encoding, &self.text)?;
        let warnings = warnings
            .into_iter()
            .map(|message| {
                ctx_traits_core::response::Warning::new(
                    ctx_traits_core::response::warning_code::TRAIT_DEPRECATED_FIELD,
                    message,
                )
            })
            .collect();
        Ok((trait_ref, warnings))
    }

    /// Host-resolved `(package status, trust verdict)` pair for `trait_ref`
    /// (this document, already decoded by the caller so its current
    /// canonical digest can be computed once and shared with other work).
    /// `status` is parsed from the `status` JSON field with a safe default;
    /// `trust` is honored only when `trust_digest` matches `trait_ref`'s
    /// current canonical digest, otherwise it fails closed to `Unreviewed`
    /// via `TrustEvidence::resolve`.
    fn lifecycle(
        &self,
        trait_ref: &Trait,
    ) -> ctx_traits_core::Result<(
        ctx_traits_core::manifest::PackageStatus,
        ctx_traits_core::r#trait::TrustVerdict,
    )> {
        let status = match self.status.as_deref() {
            Some("ready") => ctx_traits_core::manifest::PackageStatus::Ready,
            _ => ctx_traits_core::manifest::PackageStatus::Draft,
        };
        let asserted_verdict = match self.trust.as_deref() {
            Some("verified") => ctx_traits_core::r#trait::TrustVerdict::Verified,
            Some("blocked") => ctx_traits_core::r#trait::TrustVerdict::Blocked,
            _ => ctx_traits_core::r#trait::TrustVerdict::Unreviewed,
        };
        let evidence =
            self.trust_digest
                .as_ref()
                .map(|digest| ctx_traits_core::r#trait::TrustEvidence {
                    digest: digest.clone(),
                    verdict: asserted_verdict,
                });
        let current_digest = canonical_digest(trait_ref)?;
        let trust = ctx_traits_core::r#trait::TrustEvidence::resolve(
            evidence.as_ref(),
            current_digest.as_str(),
        );
        Ok((status, trust))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ValidateRequest {
    pub trait_document: TraitDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ValidateResponse {
    pub schema_version: String,
    pub valid: bool,
    pub trait_id: String,
    pub trait_version: String,
    pub canonical_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NormalizeRequest {
    pub trait_document: TraitDocument,
    pub output_format: Encoding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NormalizeResponse {
    pub schema_version: String,
    pub canonical: Trait,
    pub canonical_json: String,
    pub canonical_digest: String,
    pub output_text: String,
    pub output_format: Encoding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuditRequest {
    pub text: String,
    pub trait_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImportPlanRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<ImportRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_skills: Option<AgentSkillsImportRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "plan-kind", content = "value")]
pub enum ImportPlanResponse {
    Baseline(Box<ImportReport>),
    AgentSkills(Box<AgentSkillsImportPlan>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RenderRequest {
    pub trait_document: TraitDocument,
    pub profile: ExtendedRenderProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ModelViewRequest {
    pub trait_document: TraitDocument,
    pub profile: ExtendedRenderProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ComposeRequest {
    #[serde(default)]
    pub trait_documents: Vec<TraitDocument>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolveAbiRequest {
    pub request: ResolveRequest,
    #[serde(default)]
    pub trait_documents: Vec<TraitDocument>,
    #[serde(default)]
    pub index_estimates: Vec<CandidateEstimate>,
    #[serde(default)]
    pub index_rejections: Vec<IndexRejection>,
    #[serde(default)]
    pub all_discovered_trait_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExplainRequest {
    pub request: ActivationRequest,
    #[serde(default)]
    pub trait_documents: Vec<TraitDocument>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunStartRequest {
    pub trait_document: TraitDocument,
    pub inputs: Value,
    #[serde(default)]
    pub resource_evidence: Vec<ctx_traits_core::procedure::runtime::ResourceEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(
        default,
        rename = "agent-assignments",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_assignments: Option<Vec<AgentAssignment>>,
    /// Host-resolved approval evidence. WASM cannot read machine trust state,
    /// so a verified start is accepted only with this pin for its bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_approval: Option<ctx_traits_core::procedure::session::TrustApprovalProvenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunCallRequest {
    pub trait_document: TraitDocument,
    pub session: Session,
    pub submission: CallSubmission,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunInspectRequest {
    pub trait_document: TraitDocument,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackRequest {
    pub resolve_response: ResolveResponse,
    #[serde(default)]
    pub trait_documents: Vec<TraitDocument>,
    pub profile: ExtendedRenderProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CachePlanOperation {
    Rebuild,
    Status,
    Prune,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CachePlanRequest {
    pub operation: CachePlanOperation,
    #[serde(default)]
    pub keys: Vec<CacheArtifactKey>,
    #[serde(default)]
    pub stored: Vec<StoredCacheArtifact>,
    #[serde(default)]
    pub current: Vec<CacheArtifactKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LedgerDigestInput {
    pub trait_id: String,
    /// The current render's model-view content digest (P498 decision 3:
    /// this — not the source digest — is the sole freshness key
    /// `Ledger::reconcile` compares against).
    pub model_view_digest: String,
    pub load_level: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LedgerUpdateRequest {
    pub ledger: Ledger,
    #[serde(default)]
    pub current_digests: Vec<LedgerDigestInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_host_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CapabilitySummary {
    pub schema_version: String,
    pub capabilities: Vec<CapabilityReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UnsupportedOperationRequest {
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UnsupportedOperationResponse {
    pub operation: String,
    pub status: String,
    pub reason: String,
    pub capabilities: Vec<CapabilityReport>,
}

pub fn validate_json(input: &str) -> String {
    decode_then_with_warnings(input, |request: ValidateRequest| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let digest = canonical_digest(&trait_ref)?;
        Ok((
            ValidateResponse {
                schema_version: ABI_SCHEMA_VERSION.to_string(),
                valid: true,
                trait_id: trait_ref.id.as_str().to_string(),
                trait_version: trait_ref.version.as_str().to_string(),
                canonical_digest: digest.as_str().to_string(),
            },
            warnings,
        ))
    })
}

pub fn normalize_json(input: &str) -> String {
    decode_then_with_warnings(input, |request: NormalizeRequest| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let canonical_json = canonical_json(&trait_ref)?;
        let canonical_digest = canonical_digest(&trait_ref)?;
        let output_text = encoding::encode(request.output_format, &trait_ref)?;
        Ok((
            NormalizeResponse {
                schema_version: ABI_SCHEMA_VERSION.to_string(),
                canonical: trait_ref,
                canonical_json,
                canonical_digest: canonical_digest.as_str().to_string(),
                output_text,
                output_format: request.output_format,
            },
            warnings,
        ))
    })
}

pub fn synth_json(input: &str) -> String {
    decode_then::<SynthRequest, SynthResponse, _>(input, synthesize)
}

pub fn audit_json(input: &str) -> String {
    decode_then(input, |request: AuditRequest| {
        Ok(ctx_traits_core::audit::scan_hidden_content(
            &request.text,
            &request.trait_id,
            request.path.as_deref(),
        ))
    })
}

pub fn import_plan_json(input: &str) -> String {
    decode_then(input, |request: ImportPlanRequest| {
        if let Some(agent_skills) = request.agent_skills {
            return plan_agent_skills_import(agent_skills)
                .map(|plan| ImportPlanResponse::AgentSkills(Box::new(plan)));
        }
        if let Some(baseline) = request.baseline {
            return Ok(ImportPlanResponse::Baseline(Box::new(
                create_import_report(&baseline),
            )));
        }
        Err(ctx_traits_core::manifest::Error::InvalidField {
            field_path: "import-plan".to_string(),
            message: "expected baseline or agent-skills import request".to_string(),
        }
        .into())
    })
}

pub fn render_json(input: &str) -> String {
    decode_then_with_warnings(input, |request: RenderRequest| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let source_digest = source_digest_for_document(
            request.source_digest,
            request.trait_document.source_digest.as_deref(),
            &request.trait_document.text,
        );
        Ok((
            plan_render(&trait_ref, request.profile, &source_digest),
            warnings,
        ))
    })
}

pub fn model_view_json(input: &str) -> String {
    decode_then_with_warnings(input, |request: ModelViewRequest| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let source_digest = source_digest_for_document(
            request.source_digest,
            request.trait_document.source_digest.as_deref(),
            &request.trait_document.text,
        );
        let resource_plan = plan_resource_inclusion(&trait_ref, &[]);
        Ok((
            compile_model_view_with_evidence(
                &trait_ref,
                request.profile,
                Some(source_digest.as_str()),
                &resource_plan,
            ),
            warnings,
        ))
    })
}

pub fn compose_json(input: &str) -> String {
    decode_then(input, |request: ComposeRequest| {
        let traits = decode_trait_documents(&request.trait_documents)?;
        Ok(plan(&traits))
    })
}

pub fn resolve_json(input: &str) -> String {
    decode_then(input, |request: ResolveAbiRequest| {
        let traits = decode_trait_documents(&request.trait_documents)?;
        let lifecycle = request
            .trait_documents
            .iter()
            .zip(traits.iter())
            .map(|(document, trait_ref)| document.lifecycle(trait_ref))
            .collect::<ctx_traits_core::Result<Vec<_>>>()?;
        let all_ids: Vec<&str> = request
            .all_discovered_trait_ids
            .iter()
            .map(String::as_str)
            .collect();
        Ok(resolve(
            &request.request,
            &traits,
            &lifecycle,
            &request.index_estimates,
            &request.index_rejections,
            &all_ids,
        ))
    })
}

pub fn explain_json(input: &str) -> String {
    decode_then(input, |request: ExplainRequest| {
        let traits = decode_trait_documents(&request.trait_documents)?;
        let lifecycle = request
            .trait_documents
            .iter()
            .zip(traits.iter())
            .map(|(document, trait_ref)| document.lifecycle(trait_ref))
            .collect::<ctx_traits_core::Result<Vec<_>>>()?;
        Ok(explain(request.request, &traits, &lifecycle))
    })
}

pub fn pack_json(input: &str) -> String {
    decode_then(input, |request: PackRequest| {
        let traits = decode_trait_documents(&request.trait_documents)?;
        let source_digest_pairs: Vec<(String, String)> = request
            .trait_documents
            .iter()
            .zip(traits.iter())
            .filter_map(|(doc, trait_ref)| {
                doc.source_digest
                    .as_ref()
                    .map(|digest| (trait_ref.id.as_str().to_string(), digest.clone()))
            })
            .collect();
        let source_digests: Vec<(&str, &str)> = source_digest_pairs
            .iter()
            .map(|(id, digest)| (id.as_str(), digest.as_str()))
            .collect();
        Ok(plan_context_pack(
            &request.resolve_response,
            &traits,
            request.profile,
            &source_digests,
        ))
    })
}

pub fn run_start_json(input: &str) -> String {
    decode_then_run(input, false, |request: RunStartRequest| {
        let source_digest = source_digest_for_document(
            request.source_digest,
            request.trait_document.source_digest.as_deref(),
            &request.trait_document.text,
        );
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let canonical_digest = canonical_digest(&trait_ref)?;
        let initial_values = run_initial_values_from_json(request.inputs)?;
        let session_id = SessionId::deterministic(
            trait_ref.id.as_str(),
            Some(source_digest.as_str()),
            Some(canonical_digest.as_str()),
            &initial_values,
        )?;
        let run_id = deterministic_run_id(
            Some(source_digest.as_str()),
            Some(canonical_digest.as_str()),
        )?;
        let (status, trust) = request.trait_document.lifecycle(&trait_ref)?;
        let session = start_run_session(
            &trait_ref,
            &status,
            &trust,
            ctx_traits_core::procedure::session::StartRequest {
                strict_loops: false,
                session_id,
                run_id,
                initial_port_values: initial_values,
                resource_evidence: request.resource_evidence,
                provider_capability_reports: Vec::new(),
                source_digest: Some(ctx_traits_core::digest::Digest::parse(&source_digest)?),
                canonical_digest: Some(canonical_digest.clone()),
                agent_assignments: request.agent_assignments,
                provider_warnings: Vec::new(),
                harness_probes: Vec::new(),
                provenance: Provenance {
                    started_by: CallerProvenance {
                        surface: "wasm".to_string(),
                        caller: "ctx traits wasm abi".to_string(),
                        agent: None,
                        harness: None,
                    },
                    state_source: "wasm-run-start".to_string(),
                    agent_assignments: None,
                    harness_probes: Vec::new(),
                    warnings: Vec::new(),
                    trait_source: None,
                    query_selection: None,
                    worktree: None,
                    merge_frames: Vec::new(),
                    merge_intent: None,
                    out_of_tree_mutations: Vec::new(),
                    started_at_epoch: None,
                    trust_approval: request.trust_approval,
                },
            },
        )?;
        let resource_supported =
            ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                &session.resource_evidence,
            );
        Ok((session, resource_supported, warnings))
    })
}

pub fn run_call_json(input: &str) -> String {
    if serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|value| value.get("submission").cloned())
        .and_then(|submission| submission.as_object().cloned())
        .is_some_and(|submission| {
            submission.contains_key("command-execution")
                || submission.contains_key("command_execution")
        })
    {
        let envelope = Envelope::<CallResponse>::err_response(ResponseError::new(
            "runtime.command-execution-untrusted",
            "pure wasm-core cannot accept caller-supplied command execution evidence",
        ));
        return envelope_to_json(with_run_capabilities(envelope, true, false));
    }
    decode_then_run::<RunCallRequest, CallResponse, _>(input, true, |request| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let response = submit_run_call(&trait_ref, request.session, request.submission)?;
        let resource_supported =
            ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                &response.session.resource_evidence,
            );
        Ok((response, resource_supported, warnings))
    })
}

pub fn run_status_json(input: &str) -> String {
    decode_then_run::<RunInspectRequest, Session, _>(input, false, |request| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let session = refresh_run_session(&trait_ref, request.session)?;
        let resource_supported =
            ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                &session.resource_evidence,
            );
        Ok((session, resource_supported, warnings))
    })
}

pub fn run_frame_json(input: &str) -> String {
    decode_then_run(input, false, |request: RunInspectRequest| {
        let (trait_ref, warnings) = request.trait_document.decode_with_warnings()?;
        let session = refresh_run_session(&trait_ref, request.session)?;
        let resource_supported =
            ctx_traits_core::procedure::session::declared_resource_evidence_supported(
                &session.resource_evidence,
            );
        Ok((session.next_frame, resource_supported, warnings))
    })
}

pub fn cache_plan_json(input: &str) -> String {
    decode_then(input, |request: CachePlanRequest| {
        let plan = match request.operation {
            CachePlanOperation::Rebuild => plan_cache_rebuild(&request.keys),
            CachePlanOperation::Status => compare_cache_status(&request.stored, &request.current),
            CachePlanOperation::Prune => {
                plan_cache_prune_from_stored(&request.stored, &request.current)
            }
        };
        Ok(plan)
    })
}

pub fn ledger_update_json(input: &str) -> String {
    decode_then(input, |request: LedgerUpdateRequest| {
        let mut ledger = request.ledger;
        let current: Vec<CurrentRender> = request
            .current_digests
            .iter()
            .map(|item| {
                Ok(CurrentRender {
                    trait_id: item.trait_id.clone(),
                    model_view_digest: Digest::parse(&item.model_view_digest)?,
                    load_level: item.load_level.clone(),
                })
            })
            .collect::<Result<_, ctx_traits_core::Error>>()?;
        ledger.reconcile(&current, request.expected_host_key.as_deref());
        Ok(ledger)
    })
}

pub fn pure_wasm_capabilities_json() -> String {
    let capabilities = pure_capabilities();
    envelope_to_json(Envelope::ok(CapabilitySummary {
        schema_version: ABI_SCHEMA_VERSION.to_string(),
        capabilities,
    }))
}

/// Plan an assist boundary without calling a provider.
///
/// Accepts a JSON request with operation, source info, user request, and
/// context. Returns an assist candidate envelope with `UnsupportedProvider`
/// status since WASM never calls model providers.
pub fn assist_plan_json(input: &str) -> String {
    #[derive(Deserialize)]
    struct AssistPlanRequest {
        #[serde(rename = "operation")]
        operation: String,
        #[serde(default, rename = "source-trait-ids")]
        source_trait_ids: Vec<String>,
        #[serde(default, rename = "source-paths")]
        source_paths: Vec<String>,
        #[serde(default, rename = "source-digests")]
        source_digests: Vec<String>,
        #[serde(rename = "user-request")]
        user_request: String,
        #[serde(default, rename = "model")]
        model: Option<String>,
        #[serde(rename = "target-path")]
        target_path: String,
        #[serde(default, rename = "context")]
        context: serde_json::Value,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct AssistPlanResponse {
        candidate: ctx_traits_core::assist::Candidate,
    }

    let parsed: AssistPlanRequest = match serde_json::from_str(input) {
        Ok(r) => r,
        Err(e) => {
            return envelope_to_json::<()>(Envelope::err_response(ResponseError::new(
                "abi.decode-request",
                format!("failed to decode assist plan request: {e}"),
            )));
        }
    };

    let operation = match parsed.operation.as_str() {
        "generate" => ctx_traits_core::assist::Operation::Generate,
        "refine" => ctx_traits_core::assist::Operation::Refine,
        "compose" => ctx_traits_core::assist::Operation::Compose,
        "import" => ctx_traits_core::assist::Operation::Import,
        "explain" => ctx_traits_core::assist::Operation::Explain,
        other => {
            return envelope_to_json::<()>(Envelope::err_response(ResponseError::new(
                "abi.invalid-operation",
                format!("unknown assist operation: {other}"),
            )));
        }
    };

    let source_digests = match parsed
        .source_digests
        .iter()
        .map(|digest| ctx_traits_core::digest::Digest::parse(digest))
        .collect::<ctx_traits_core::Result<Vec<_>>>()
    {
        Ok(source_digests) => source_digests,
        Err(error) => {
            return envelope_to_json::<()>(Envelope::err_response(ResponseError::from_core_error(
                &error,
            )));
        }
    };

    match ctx_traits_core::assist::plan_assist_boundary(ctx_traits_core::assist::BoundaryRequest {
        operation,
        source_trait_ids: parsed.source_trait_ids,
        source_paths: parsed.source_paths,
        source_digests,
        user_request: parsed.user_request,
        model: parsed.model,
        target_path: parsed.target_path,
        provider_available: false,
        context: parsed.context,
    }) {
        Ok(candidate) => envelope_to_json(Envelope::ok(AssistPlanResponse { candidate })),
        Err(e) => {
            envelope_to_json::<()>(Envelope::err_response(ResponseError::from_core_error(&e)))
        }
    }
}

pub fn unsupported_operation_json(input: &str) -> String {
    decode_then(input, |request: UnsupportedOperationRequest| {
        Ok(unsupported_operation_response(request.operation))
    })
}

pub fn unsupported_import_copy_json() -> String {
    envelope_to_json(Envelope::ok(unsupported_operation_response("import-copy")))
}

pub fn unsupported_git_fetch_json() -> String {
    envelope_to_json(Envelope::ok(unsupported_operation_response("git-fetch")))
}

pub fn unsupported_export_write_json() -> String {
    envelope_to_json(Envelope::ok(unsupported_operation_response("export-write")))
}

pub fn unsupported_cache_persistence_json() -> String {
    envelope_to_json(Envelope::ok(unsupported_operation_response(
        "cache-persistence",
    )))
}

fn decode_then<Request, Value, F>(input: &str, f: F) -> String
where
    Request: serde::de::DeserializeOwned,
    Value: Serialize,
    F: FnOnce(Request) -> ctx_traits_core::Result<Value>,
{
    response_decode_then(
        input,
        ABI_ERROR_CODES,
        |error| ResponseError::from_core_error(&error),
        f,
    )
}

fn decode_then_with_warnings<Request, Value, F>(input: &str, f: F) -> String
where
    Request: serde::de::DeserializeOwned,
    Value: Serialize,
    F: FnOnce(Request) -> ctx_traits_core::Result<(Value, Vec<ctx_traits_core::response::Warning>)>,
{
    response_decode_then_with_warnings(
        input,
        ABI_ERROR_CODES,
        |error| ResponseError::from_core_error(&error),
        f,
    )
}

fn decode_then_run<Request, Value, F>(input: &str, call_payload: bool, f: F) -> String
where
    Request: serde::de::DeserializeOwned,
    Value: Serialize,
    F: FnOnce(
        Request,
    )
        -> ctx_traits_core::Result<(Value, bool, Vec<ctx_traits_core::response::Warning>)>,
{
    let request = match serde_json::from_str::<Request>(input) {
        Ok(request) => request,
        Err(error) => {
            let envelope = Envelope::<Value>::err_response(
                ResponseError::new("abi.decode-request", "failed to decode ABI request")
                    .with_detail("serde-error", error.to_string()),
            );
            return envelope_to_json(with_run_capabilities(envelope, call_payload, false));
        }
    };
    match f(request) {
        Ok((value, resource_supported, warnings)) => {
            let mut envelope = Envelope::ok(value);
            for warning in warnings {
                envelope = envelope.with_warning(warning);
            }
            envelope_to_json(with_run_capabilities(
                envelope,
                call_payload,
                resource_supported,
            ))
        }
        Err(error) => envelope_to_json(with_run_capabilities(
            Envelope::<Value>::err_response(ResponseError::from_core_error(&error)),
            call_payload,
            false,
        )),
    }
}

fn with_run_capabilities<T>(
    mut envelope: Envelope<T>,
    call_payload: bool,
    declared_resource_evidence: bool,
) -> Envelope<T> {
    for capability in ctx_traits_core::procedure::session::run_session_capability_reports(
        false,
        false,
        call_payload,
        declared_resource_evidence,
        false,
        false,
        false,
    ) {
        envelope = envelope.with_capability(capability);
    }
    envelope
}

fn envelope_to_json<T: Serialize>(envelope: Envelope<T>) -> String {
    response_envelope_to_json(envelope, ABI_ERROR_CODES)
}

fn source_digest_for_document(
    explicit: Option<String>,
    document_digest: Option<&str>,
    text: &str,
) -> String {
    explicit
        .or_else(|| document_digest.map(str::to_string))
        .unwrap_or_else(|| {
            ctx_traits_core::digest::Digest::source(text)
                .as_str()
                .to_string()
        })
}

fn decode_trait_documents(documents: &[TraitDocument]) -> ctx_traits_core::Result<Vec<Trait>> {
    documents.iter().map(TraitDocument::decode).collect()
}

fn pure_capabilities() -> Vec<CapabilityReport> {
    let mut capabilities = vec![
        CapabilityReport::supported("validate"),
        CapabilityReport::supported("normalize"),
        CapabilityReport::supported("synth"),
        CapabilityReport::supported("audit"),
        CapabilityReport::supported("import-plan"),
        CapabilityReport::supported("model-view"),
        CapabilityReport::supported("render-plan"),
        CapabilityReport::supported("compose"),
        CapabilityReport::supported("resolve"),
        CapabilityReport::supported("explain"),
        CapabilityReport::supported("context-pack-plan"),
        CapabilityReport::supported("cache-plan"),
        CapabilityReport::supported("ledger-update-plan"),
        CapabilityReport::unsupported(
            "filesystem",
            "host or CLI must supply already-loaded text and perform writes",
        ),
        CapabilityReport::unsupported(
            "git",
            "host or CLI must fetch before calling pure wasm-core",
        ),
        CapabilityReport::unsupported("process", "pure wasm-core never spawns commands"),
        CapabilityReport::unsupported("network", "pure wasm-core never opens network connections"),
        CapabilityReport::unsupported(
            "host-hook",
            "plugin/host adapter must provide hook execution",
        ),
        CapabilityReport::unsupported("persistence", "host or CLI owns cache/ledger persistence"),
        CapabilityReport::unsupported(
            "llm-provider",
            "pure wasm-core never calls model providers; host adapters must provide candidates explicitly",
        ),
    ];
    capabilities.sort();
    capabilities
}

fn unsupported_operation_response(operation: impl Into<String>) -> UnsupportedOperationResponse {
    let operation = operation.into();
    let capabilities = unsupported_io_capabilities(&operation);
    UnsupportedOperationResponse {
        operation,
        status: "unsupported".to_string(),
        reason: "operation requires host/IO capability outside pure wasm-core".to_string(),
        capabilities,
    }
}

fn unsupported_io_capabilities(operation: &str) -> Vec<CapabilityReport> {
    let mut capabilities = vec![
        CapabilityReport::unsupported(
            "filesystem",
            format!("{operation} requires host filesystem IO"),
        ),
        CapabilityReport::unsupported(
            "git",
            format!("{operation} may require host Git/network IO"),
        ),
        CapabilityReport::unsupported(
            "process",
            format!("{operation} cannot spawn helper processes from pure wasm-core"),
        ),
        CapabilityReport::unsupported(
            "network",
            format!("{operation} cannot access network from pure wasm-core"),
        ),
        CapabilityReport::unsupported(
            "host-hook",
            format!("{operation} requires an explicit host adapter hook"),
        ),
        CapabilityReport::unsupported(
            "persistence",
            format!("{operation} requires host-owned persistence"),
        ),
    ];
    capabilities.sort();
    capabilities
}
