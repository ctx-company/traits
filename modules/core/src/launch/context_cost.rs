// Launch context cost reporting.
/// Launch context cost evaluation.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextCostReport {
    pub trait_id: String,
    pub tokenizer: String,
    pub layers: Vec<ContextCostLayer>,
    pub total_estimated_tokens: u64,
    pub budget: Option<u64>,
    pub budget_remaining: Option<i64>,
    pub over_budget_by: Option<u64>,
    pub budget_status: String,
    pub warnings: Vec<ContextCostWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextCostLayer {
    pub layer: String,
    pub item: String,
    /// Whether this layer is required, for non-resource layers. `None` for
    /// declared-resource layers, whose inclusion is governed by trigger and
    /// references rather than a required/optional axis. Omitted from
    /// serialization when absent so resource layers carry no requiredness
    /// axis at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    pub selected: bool,
    pub skip_reason: Option<String>,
    pub estimated_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextCostWarning {
    pub code: String,
    pub message: String,
    pub items: Vec<String>,
}

pub fn context_cost_report(trait_ref: &Trait, budget: Option<u64>) -> ContextCostReport {
    let mut layers = Vec::new();
    let canonical_text = serde_json::to_string(trait_ref).unwrap_or_default();
    push_cost(
        &mut layers,
        "canonical-body",
        trait_ref.id.as_str(),
        Some(true),
        true,
        None,
        &canonical_text,
    );
    push_cost(
        &mut layers,
        "trait-summary",
        trait_ref.id.as_str(),
        Some(true),
        true,
        None,
        trait_ref.summary.as_str(),
    );
    if let Some(behavior) = &trait_ref.behavior {
        let behavior_text = serde_json::to_string(behavior).unwrap_or_default();
        push_cost(
            &mut layers,
            "behavior",
            trait_ref.id.as_str(),
            Some(true),
            true,
            None,
            &behavior_text,
        );
    }
    for (id, prompt) in trait_ref.prompts.iter() {
        let prompt_text = prompt
            .text
            .as_deref()
            .or(prompt.source.as_deref())
            .unwrap_or_default();
        push_cost(
            &mut layers,
            "prompt",
            id,
            Some(true),
            true,
            None,
            prompt_text,
        );
    }
    if let Some(procedure) = &trait_ref.procedure {
        push_cost(
            &mut layers,
            "procedure",
            trait_ref.id.as_str(),
            Some(true),
            true,
            None,
            &procedure.description,
        );
        for item in &procedure.sequence {
            let item_text = sequence_item_cost_text(item);
            push_cost(
                &mut layers,
                "procedure-sequence",
                &item.title,
                Some(true),
                true,
                None,
                &item_text,
            );
        }
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        push_cost(
            &mut layers,
            "named-sequence",
            sequence_id,
            Some(true),
            true,
            None,
            sequence.description.as_deref().unwrap_or_default(),
        );
        for item in &sequence.sequence {
            let layer_id = format!(
                "{sequence_id}/{}",
                item.id.as_deref().unwrap_or(item.title.as_str())
            );
            let item_text = sequence_item_cost_text(item);
            push_cost(
                &mut layers,
                "named-sequence-item",
                &layer_id,
                Some(true),
                true,
                None,
                &item_text,
            );
        }
    }
    for resource in &trait_ref.resources {
        let selected =
            resource.effective_trigger() == crate::r#trait::ResourceTrigger::OnActivation;
        push_cost(
            &mut layers,
            "declared-resource",
            &resource.id,
            None,
            selected,
            (!selected).then(|| {
                "on-demand resource not selected by this static estimate".to_string()
            }),
            resource
                .hint
                .as_deref()
                .or(resource.path.as_deref())
                .unwrap_or("inline resource content"),
        );
    }
    let total_estimated_tokens = layers
        .iter()
        .filter(|layer| layer.selected)
        .map(|layer| layer.estimated_tokens)
        .sum();
    let mut warnings = Vec::new();
    for layer in layers
        .iter()
        .filter(|layer| !layer.selected && layer.skip_reason.is_some())
    {
        warnings.push(ContextCostWarning {
            code: "budget.on-demand-resource-skipped-static-estimate".to_string(),
            message: layer.skip_reason.clone().unwrap_or_default(),
            items: vec![format!("{}:{}", layer.layer, layer.item)],
        });
    }
    let (budget_status, budget_remaining, over_budget_by) = match budget {
        Some(limit) if total_estimated_tokens > limit => {
            let over_by = total_estimated_tokens - limit;
            let required_items = layers
                .iter()
                .filter(|layer| layer.selected && layer.required == Some(true))
                .map(|layer| format!("{}:{}", layer.layer, layer.item))
                .collect::<Vec<_>>();
            warnings.push(ContextCostWarning {
                code: "budget.required-over-budget".to_string(),
                message:
                    "required selected layers exceed the budget and cannot be silently dropped"
                        .to_string(),
                items: required_items,
            });
            (
                "exceeded".to_string(),
                Some(-saturating_i64(over_by)),
                Some(over_by),
            )
        }
        Some(limit) => (
            "within-budget".to_string(),
            Some(saturating_i64(limit - total_estimated_tokens)),
            None,
        ),
        None => ("not-set".to_string(), None, None),
    };
    ContextCostReport {
        trait_id: trait_ref.id.as_str().to_string(),
        tokenizer: "rough-char-div-4-estimate-not-billing".to_string(),
        layers,
        total_estimated_tokens,
        budget,
        budget_remaining,
        over_budget_by,
        budget_status,
        warnings,
    }
}

fn sequence_item_cost_text(item: &crate::r#trait::procedure::SequenceItem) -> String {
    match item.effective_kind() {
        crate::r#trait::procedure::SequenceKind::Prompt => item.prompt.clone(),
        crate::r#trait::procedure::SequenceKind::Ask => item.prompt.clone(),
        crate::r#trait::procedure::SequenceKind::Command => "runtime command step".to_string(),
        crate::r#trait::procedure::SequenceKind::Check => "runtime check step".to_string(),
        crate::r#trait::procedure::SequenceKind::Project => {
            "deterministic runtime projection".to_string()
        }
        crate::r#trait::procedure::SequenceKind::Sequence => format!(
            "runtime nested sequence {}",
            item.sequence.as_deref().unwrap_or("sequence:<missing>")
        ),
        crate::r#trait::procedure::SequenceKind::Branch => format!(
            "runtime branch then={} otherwise={}",
            item.sequence.as_deref().unwrap_or("sequence:<missing>"),
            item.otherwise.as_deref().unwrap_or("none")
        ),
        crate::r#trait::procedure::SequenceKind::Loop => format!(
            "bounded runtime loop {} max-iterations={}",
            item.sequence.as_deref().unwrap_or("sequence:<missing>"),
            item.max_iterations
                .map(|value| value.to_string())
                .or_else(|| item.max_iterations_from.clone())
                .unwrap_or_else(|| "missing".to_string())
        ),
        crate::r#trait::procedure::SequenceKind::ForEach => format!(
            "typed runtime for-each over {} item {} sequence {}",
            item.over.as_deref().unwrap_or("slot:<missing>"),
            item.item.as_deref().unwrap_or("slot:<missing>"),
            item.sequence.as_deref().unwrap_or("sequence:<missing>")
        ),
        crate::r#trait::procedure::SequenceKind::Parallel => format!(
            "typed runtime parallel branches {}",
            item.branches.iter().cloned().collect::<Vec<_>>().join(" ")
        ),
    }
}

fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn push_cost(
    layers: &mut Vec<ContextCostLayer>,
    layer: &str,
    item: &str,
    required: Option<bool>,
    selected: bool,
    skip_reason: Option<String>,
    text: &str,
) {
    layers.push(ContextCostLayer {
        layer: layer.to_string(),
        item: item.to_string(),
        required,
        selected,
        skip_reason,
        estimated_tokens: estimate_tokens(text),
    });
}

fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(4).max(1)
}
