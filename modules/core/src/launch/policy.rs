// Launch policy reporting.
/// Launch policy definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PolicyReport {
    pub trait_id: String,
    pub profile: String,
    pub items: Vec<PolicyItem>,
    pub hook_plan: Vec<HostHookPlanEntry>,
    pub drift_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PolicyItem {
    pub field: String,
    pub class: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HostHookPlanEntry {
    pub hook: String,
    pub required_capability: String,
    pub supported: bool,
    pub capability_status: String,
    pub fallback: String,
}

pub fn policy_report(trait_ref: &Trait, profile: ExtendedRenderProfile) -> PolicyReport {
    let mut items = Vec::new();
    if trait_ref.behavior.is_some() || trait_ref.intent.is_some() {
        items.push(PolicyItem {
            field: "intent-behavior".to_string(),
            class: "advisory".to_string(),
            message: "canonical guidance renders as model-visible instructions, not runtime permission control".to_string(),
        });
    }
    let hook_plan = ["session-start", "pre-tool", "post-tool", "prompt-submit"]
        .into_iter()
        .map(|hook| HostHookPlanEntry {
            hook: hook.to_string(),
            required_capability: match hook {
                "session-start" => "can-hook-session-created",
                "pre-tool" => "can-hook-tool-before",
                "post-tool" => "can-hook-tool-after",
                "prompt-submit" => "can-append-prompt",
                _ => "unsupported",
            }
            .to_string(),
            supported: false,
            capability_status: "requires-explicit-can-capability-evidence".to_string(),
            fallback: "render advisory warning; host/runtime enforcement requires explicit can-* capability evidence".to_string(),
        })
        .collect();
    PolicyReport {
        trait_id: trait_ref.id.as_str().to_string(),
        profile: profile.as_str().to_string(),
        items,
        hook_plan,
        drift_evidence: vec![
            "compare advisory text, hook plan, capability report, and generated export digest in locked checks when lock evidence exists".to_string(),
        ],
    }
}
