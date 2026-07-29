// Launch subagent reporting.
/// Launch subagent definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SubagentReport {
    pub trait_id: String,
    pub profile: String,
    pub guidance: Vec<String>,
    pub declared_intent: Vec<SubagentDeclaredIntentItem>,
    pub propagation: Vec<SubagentPropagationItem>,
    pub non_claim: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SubagentDeclaredIntentItem {
    pub field: String,
    pub values: Vec<String>,
    pub enforceability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SubagentPropagationItem {
    pub item: String,
    pub should_propagate: bool,
    pub enforceability: String,
    pub evidence: String,
}

/// `status`/`trust` are caller-resolved from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn subagent_report(
    trait_ref: &Trait,
    status: &PackageStatus,
    trust: &TrustVerdict,
    profile: ExtendedRenderProfile,
) -> SubagentReport {
    let profile_enforceability = subagent_profile_enforceability(profile);
    let mut propagation = vec![
        SubagentPropagationItem {
            item: "lifecycle-and-trust".to_string(),
            should_propagate: true,
            enforceability: profile_enforceability.to_string(),
            evidence: format!(
                "status={}, trust={}",
                status_str(status),
                trust_str(trust)
            ),
        },
        SubagentPropagationItem {
            item: "activation-rationale".to_string(),
            should_propagate: true,
            enforceability: "advisory".to_string(),
            evidence: activation_note(trait_ref),
        },
    ];
    for port in &trait_ref.ports {
        if matches!(port.direction, PortDirection::Input) {
            propagation.push(SubagentPropagationItem {
                item: format!("input-port.{}", port.id),
                should_propagate: !port.optional,
                enforceability: "advisory".to_string(),
                evidence: port.description.clone(),
            });
        }
    }
    SubagentReport {
        trait_id: trait_ref.id.as_str().to_string(),
        profile: profile.as_str().to_string(),
        guidance: vec![
            "subagent role/model/service-tier hints are advisory unless the host exposes an enforceable capability".to_string(),
            "required inherited traits and handoff evidence should be rendered into static profiles".to_string(),
            "plugin profiles may warn on mismatch only when capability reports prove hook support".to_string(),
        ],
        declared_intent: declared_subagent_intent(trait_ref, profile),
        propagation,
        non_claim: "ctx.traits cannot force a host subagent to use a model, effort level, service tier, or tool policy".to_string(),
    }
}

fn subagent_profile_enforceability(profile: ExtendedRenderProfile) -> &'static str {
    if matches!(profile, ExtendedRenderProfile::Opencode) {
        "host-hook-plan-requires-capability-report"
    } else {
        "advisory"
    }
}

fn declared_subagent_intent(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
) -> Vec<SubagentDeclaredIntentItem> {
    let Some(intent) = trait_ref
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.ctx.as_ref())
        .and_then(|ctx| ctx.subagent.as_ref())
    else {
        return Vec::new();
    };
    let mut items = Vec::new();
    push_optional_intent(&mut items, "role", &intent.role, "advisory");
    push_optional_intent(
        &mut items,
        "task-boundary",
        &intent.task_boundary,
        "advisory",
    );
    push_optional_intent(&mut items, "load-level", &intent.load_level, "advisory");
    push_intent_list(
        &mut items,
        "allowed-tool",
        &intent.allowed_tools,
        "advisory",
    );
    push_intent_list(
        &mut items,
        "allowed-surface",
        &intent.allowed_surfaces,
        "advisory",
    );
    push_intent_list(
        &mut items,
        "required-inherited-trait",
        &intent.required_inherited_traits,
        "advisory",
    );
    push_intent_list(
        &mut items,
        "forbidden-model-hint",
        &intent.forbidden_model_hints,
        subagent_profile_enforceability(profile),
    );
    push_intent_list(
        &mut items,
        "forbidden-service-tier-hint",
        &intent.forbidden_service_tier_hints,
        subagent_profile_enforceability(profile),
    );
    push_intent_list(
        &mut items,
        "handoff-expectation",
        &intent.handoff_expectations,
        "advisory",
    );
    push_intent_list(
        &mut items,
        "evidence-expectation",
        &intent.evidence_expectations,
        "advisory",
    );
    items
}

fn push_optional_intent(
    items: &mut Vec<SubagentDeclaredIntentItem>,
    field: &str,
    value: &Option<String>,
    enforceability: &str,
) {
    if let Some(value) = value {
        push_intent_list(items, field, std::slice::from_ref(value), enforceability);
    }
}

fn push_intent_list(
    items: &mut Vec<SubagentDeclaredIntentItem>,
    field: &str,
    values: &[String],
    enforceability: &str,
) {
    if !values.is_empty() {
        items.push(SubagentDeclaredIntentItem {
            field: field.to_string(),
            values: values.to_vec(),
            enforceability: enforceability.to_string(),
        });
    }
}
