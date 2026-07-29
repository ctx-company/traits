// Launch context contract reporting.
/// Launch context contract definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextContractReport {
    pub trait_id: String,
    pub layers: Vec<ContextContractLayer>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextContractLayer {
    pub layer: String,
    pub belongs_in: String,
    pub evidence: String,
}

pub fn context_contract_report(trait_ref: &Trait) -> ContextContractReport {
    let mut warnings = Vec::new();
    for (id, prompt) in trait_ref.prompts.iter() {
        let text = prompt
            .text
            .as_deref()
            .or(prompt.source.as_deref())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if [
            "todo",
            "current sprint",
            "yesterday",
            "today",
            "chat transcript",
        ]
        .iter()
        .any(|needle| text.contains(needle))
        {
            warnings.push(format!("prompt.{id} may embed mutable project/session state; prefer a declared resource or context contract"));
        }
    }
    ContextContractReport {
        trait_id: trait_ref.id.as_str().to_string(),
        layers: vec![
            layer(
                "behavior",
                "trait.toml",
                "stable behavior, activation, procedure, and policy guidance",
            ),
            layer(
                "project-facts",
                "Atlas/project docs",
                "stable project truth and architectural decisions",
            ),
            layer(
                "task-state",
                "specs/plans/handoffs",
                "mutable work state and review decisions",
            ),
            layer(
                "live-state",
                "scripts/tools/MCP resources",
                "derivable state with freshness metadata",
            ),
            layer(
                "external-knowledge",
                "declared resources",
                "source, trust, freshness, and digest evidence",
            ),
        ],
        warnings,
    }
}

fn layer(layer: &str, belongs_in: &str, evidence: &str) -> ContextContractLayer {
    ContextContractLayer {
        layer: layer.to_string(),
        belongs_in: belongs_in.to_string(),
        evidence: evidence.to_string(),
    }
}
