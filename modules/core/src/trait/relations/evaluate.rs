// Trait relation evaluation.
/// Trait relation evaluation.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum EdgeEffect {
    /// Requires/suggests edge connected.
    Triggered,
    /// Requires target not active.
    Blocked,
    /// Targetless conflict applies to the declaring trait.
    SelfConflict,
    /// Edge participates in a cycle.
    Cyclic,
    /// `when` conditions not matched — edge inactive.
    WhenUnmatched,
}

/// One relation edge evaluation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EdgeEvaluation {
    pub source_trait_id: String,
    /// Target trait ID for requires/suggests; the declaring trait itself for
    /// targetless conflicts.
    pub target_trait_id: String,
    /// Original target ref text (e.g. `trait:dep-a`, `port:review-context`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<Reference>,
    pub kind: EdgeKind,
    pub effect: EdgeEffect,
    pub reason: String,
    /// Exact `when` ref strings carried on the edge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_refs: Vec<Reference>,
    /// Whether `when` conditions were matched for this evaluation.
    #[serde(default)]
    pub when_matched: bool,
    /// Binding proposals produced for port-target requires (requires only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding_proposals: Vec<Proposal>,
    /// Structured port-target outcomes for port-target edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_target_outcomes: Vec<PortTargetOutcome>,
}

/// Relation evaluation section for the activation explain report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Evaluation {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<EdgeEvaluation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<Cycle>,
}

impl Evaluation {
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.cycles.is_empty()
    }
}

/// Evaluate relations for scored candidates.
///
/// `when` conditions are matched exactly using **source-trait-scoped** rule
/// facts: a local `rule:<id>` in a relation matches only a matched activation
/// rule with that ID on the declaring/source trait. Signals are matched
/// against validated signal facts. An empty `when` list is unconditional.
/// All refs inside one entry's `when` list must match (AND); separate relation
/// entries model OR because each entry evaluates independently.
///
/// `scoped_rule_facts` maps `trait_id → set of matched rule ref strings`
/// (`rule:<id>`) for the source trait. `signal_facts` is a flat set of
/// validated signal ref strings.
///
/// For `port:*` targets, the edge is not dropped — it produces requirement
/// or binding evidence even when there is no target trait ID.
pub fn evaluate(
    graph: &Graph,
    active_trait_ids: &BTreeSet<String>,
    scoped_rule_facts: &BTreeMap<String, BTreeSet<String>>,
    signal_facts: &BTreeSet<String>,
    traits: &[Trait],
) -> Evaluation {
    let cyclic_pairs: BTreeSet<(String, String)> = graph
        .cycles
        .iter()
        .flat_map(|c| {
            c.path
                .iter()
                .zip(c.path.iter().skip(1).chain(c.path.first()))
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    let mut edges = Vec::new();
    for edge in &graph.edges {
        let source_active = active_trait_ids.contains(&edge.source_trait_id);
        if !source_active {
            continue;
        }

        // Exact `when` matching: empty = unconditional; all refs must match.
        // Rule facts are scoped to the source trait.
        let source_rules = scoped_rule_facts.get(&edge.source_trait_id);
        let when_matched = evaluate_when_refs(&edge.when_refs, source_rules, signal_facts);

        match edge.kind {
            EdgeKind::Requires => {
                if let Some(ref target_id) = edge.resolved_target_trait_id {
                    let target_active = active_trait_ids.contains(target_id);
                    let is_cyclic =
                        cyclic_pairs.contains(&(edge.source_trait_id.clone(), target_id.clone()));

                    let effect = if is_cyclic {
                        EdgeEffect::Cyclic
                    } else if !when_matched {
                        EdgeEffect::WhenUnmatched
                    } else if !target_active {
                        EdgeEffect::Blocked
                    } else {
                        EdgeEffect::Triggered
                    };

                    edges.push(EdgeEvaluation {
                        source_trait_id: edge.source_trait_id.clone(),
                        target_trait_id: target_id.clone(),
                        target_ref: edge.target.clone(),
                        kind: EdgeKind::Requires,
                        effect,
                        reason: edge.reason.clone(),
                        when_refs: edge.when_refs.clone(),
                        when_matched,
                        binding_proposals: Vec::new(),
                        port_target_outcomes: Vec::new(),
                    });
                } else if let Some(ref target_ref_text) = edge.target {
                    // Port target: evaluate structured outcomes.
                    let mut binding_proposals = Vec::new();
                    let port_target_outcomes = evaluate_port_target(
                        EdgeKind::Requires,
                        &edge.source_trait_id,
                        target_ref_text,
                        when_matched,
                        traits,
                        &mut binding_proposals,
                    );
                    let effect = if !when_matched {
                        EdgeEffect::WhenUnmatched
                    } else {
                        EdgeEffect::Triggered
                    };
                    edges.push(EdgeEvaluation {
                        source_trait_id: edge.source_trait_id.clone(),
                        target_trait_id: target_ref_text.to_string(),
                        target_ref: Some(target_ref_text.clone()),
                        kind: EdgeKind::Requires,
                        effect,
                        reason: edge.reason.clone(),
                        when_refs: edge.when_refs.clone(),
                        when_matched,
                        binding_proposals,
                        port_target_outcomes,
                    });
                }
            }
            EdgeKind::Suggests => {
                if let Some(ref target_id) = edge.resolved_target_trait_id {
                    let is_cyclic =
                        cyclic_pairs.contains(&(edge.source_trait_id.clone(), target_id.clone()));
                    let effect = if is_cyclic {
                        EdgeEffect::Cyclic
                    } else if !when_matched {
                        EdgeEffect::WhenUnmatched
                    } else {
                        EdgeEffect::Triggered
                    };
                    edges.push(EdgeEvaluation {
                        source_trait_id: edge.source_trait_id.clone(),
                        target_trait_id: target_id.clone(),
                        target_ref: edge.target.clone(),
                        kind: EdgeKind::Suggests,
                        effect,
                        reason: edge.reason.clone(),
                        when_refs: edge.when_refs.clone(),
                        when_matched,
                        binding_proposals: Vec::new(),
                        port_target_outcomes: Vec::new(),
                    });
                } else if let Some(ref target_ref_text) = edge.target {
                    let port_target_outcomes = evaluate_port_target(
                        EdgeKind::Suggests,
                        &edge.source_trait_id,
                        target_ref_text,
                        when_matched,
                        traits,
                        &mut Vec::new(),
                    );
                    let effect = if !when_matched {
                        EdgeEffect::WhenUnmatched
                    } else {
                        EdgeEffect::Triggered
                    };
                    edges.push(EdgeEvaluation {
                        source_trait_id: edge.source_trait_id.clone(),
                        target_trait_id: target_ref_text.to_string(),
                        target_ref: Some(target_ref_text.clone()),
                        kind: EdgeKind::Suggests,
                        effect,
                        reason: edge.reason.clone(),
                        when_refs: edge.when_refs.clone(),
                        when_matched,
                        binding_proposals: Vec::new(),
                        port_target_outcomes,
                    });
                }
            }
            EdgeKind::Conflicts => {
                let effect = if !when_matched {
                    EdgeEffect::WhenUnmatched
                } else {
                    EdgeEffect::SelfConflict
                };
                edges.push(EdgeEvaluation {
                    source_trait_id: edge.source_trait_id.clone(),
                    target_trait_id: edge.source_trait_id.clone(),
                    target_ref: None,
                    kind: EdgeKind::Conflicts,
                    effect,
                    reason: edge.reason.clone(),
                    when_refs: edge.when_refs.clone(),
                    when_matched,
                    binding_proposals: Vec::new(),
                    port_target_outcomes: Vec::new(),
                });
            }
        }
    }

    Evaluation {
        edges,
        cycles: graph.cycles.clone(),
    }
}

/// Evaluate exact `when` refs: empty = unconditional; all refs must match.
///
/// `source_rules` is the set of matched rule ref strings (`rule:<id>`)
/// scoped to the source/declaring trait. `signal_facts` is a flat set of
/// validated signal ref strings. Bare IDs are not matched — only exact
/// `rule:<id>` / `signal:<id>` ref strings.
fn evaluate_when_refs(
    when_refs: &[Reference],
    source_rules: Option<&BTreeSet<String>>,
    signal_facts: &BTreeSet<String>,
) -> bool {
    if when_refs.is_empty() {
        return true;
    }
    when_refs.iter().all(|raw| {
        if let Ok(parsed) = Reference::parse(raw.as_str()) {
            let ref_text = parsed.to_string();
            match parsed.kind() {
                Kind::Rule => source_rules
                    .map(|rules| rules.contains(&ref_text))
                    .unwrap_or(false),
                Kind::Signal => signal_facts.contains(&ref_text),
                _ => false,
            }
        } else {
            false
        }
    })
}
