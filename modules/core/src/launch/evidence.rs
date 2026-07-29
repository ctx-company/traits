// Launch evidence reporting.
/// Launch evidence definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EvidenceBundle {
    pub trait_id: String,
    pub version: String,
    pub lifecycle: String,
    pub trust: String,
    pub owner_reviewer: String,
    pub digests: EvidenceDigests,
    pub resource_manifest: Vec<String>,
    pub policy_manifest: Vec<PolicyItem>,
    pub render_warnings: Vec<String>,
    pub unsupported_fields: Vec<String>,
    pub activation_explanation: String,
    pub lock_comparison_status: String,
    pub scenarios: Vec<ScenarioEvidence>,
    pub evals: Vec<EvalEvidence>,
    pub host_model_provenance: Vec<String>,
    pub non_claim: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EvidenceDigests {
    pub source: Digest,
    pub canonical: Digest,
    pub model_view: Digest,
    pub generated_exports: Vec<Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScenarioEvidence {
    pub id: String,
    pub variant: String,
    pub expected_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EvalEvidence {
    pub id: String,
    pub variant: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub scenarios: Vec<String>,
    pub supported_for_execution: bool,
}

/// `status`/`trust` are caller-resolved from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn evidence_bundle(
    trait_ref: &Trait,
    status: &PackageStatus,
    trust: &TrustVerdict,
    source_text: &str,
    profile: ExtendedRenderProfile,
) -> EvidenceBundle {
    let canonical_text = serde_json::to_string(trait_ref).unwrap_or_default();
    let source_digest = Digest::source(source_text);
    let resource_plan = plan_resource_inclusion(trait_ref, &[]);
    let model_view = compile_model_view_with_evidence(
        trait_ref,
        profile,
        Some(source_digest.as_str()),
        &resource_plan,
    );
    let policy = policy_report(trait_ref, profile);
    EvidenceBundle {
        trait_id: trait_ref.id.as_str().to_string(),
        version: trait_ref.version.as_str().to_string(),
        lifecycle: status_str(status).to_string(),
        trust: trust_str(trust).to_string(),
        owner_reviewer: "not-modeled-in-canonical-metadata".to_string(),
        digests: EvidenceDigests {
            source: source_digest,
            canonical: Digest::source(&canonical_text),
            model_view: model_view.content_digest,
            generated_exports: Vec::new(),
        },
        resource_manifest: trait_ref
            .resources
            .iter()
            .map(|resource| match &resource.path {
                Some(path) => format!("{} -> {path}", resource.id),
                None => format!(
                    "{} -> inline:{}",
                    resource.id,
                    Digest::source(resource.content.as_deref().unwrap_or(""))
                ),
            })
            .collect(),
        policy_manifest: policy.items,
        render_warnings: model_view.warnings,
        unsupported_fields: compatibility_profile(profile).unsupported_fields,
        activation_explanation: activation_note(trait_ref),
        lock_comparison_status: "not-compared-no-lock-evidence-supplied".to_string(),
        scenarios: trait_ref
            .scenarios
            .iter()
            .map(|scenario| ScenarioEvidence {
                id: scenario.id.clone(),
                variant: scenario.variant.as_str().to_string(),
                expected_output: scenario.output.clone(),
            })
            .collect(),
        evals: trait_ref
            .evals
            .iter()
            .map(|eval| EvalEvidence {
                id: eval.id.clone(),
                variant: eval.variant.as_str().to_string(),
                input: eval.input.clone(),
                output: eval.output.clone(),
                scenarios: eval.scenarios.clone(),
                supported_for_execution: eval.variant.is_mvp_supported(),
            })
            .collect(),
        host_model_provenance: vec!["user-supplied host/model provenance only; no provider calls performed".to_string()],
        non_claim: "evidence bundles show reviewed inputs and deterministic artifacts; they do not prove future model behavior".to_string(),
    }
}

fn status_str(status: &PackageStatus) -> &'static str {
    status.as_str()
}

fn trust_str(trust: &TrustVerdict) -> &'static str {
    trust.display_name()
}
