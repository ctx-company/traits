//! Resolve planning: budgeted activation plans.
//!
//! Reuses activation scoring to select traits under a token budget. Resolve
//! returns reasons, load levels, budget decisions, relation evidence,
//! conflicts, and rejected candidates. It does not inject context into any
//! host.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::manifest::PackageStatus;
use crate::response::CapabilityReport;
use crate::r#trait::activation::{ExplainReport, Request as ActivationRequest, explain};
use crate::r#trait::{Trait, TrustVerdict};

const UNKNOWN_FALLBACK_ESTIMATED_TOKENS: u64 = 100;

// ---------------------------------------------------------------------------
// Load level
// ---------------------------------------------------------------------------

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum LoadLevel {
    Discovery,
    Summary,
    Full,
}

impl LoadLevel {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Budget decision
// ---------------------------------------------------------------------------

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum BudgetDecision {
    SelectedFull,
    SelectedSummary,
    SelectedDiscovery,
    DowngradedToSummary,
    DowngradedToDiscovery,
    RejectedBudgetExceeded,
    RejectedInactive,
}

impl BudgetDecision {
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Index estimate input
// ---------------------------------------------------------------------------

/// Discovery-index estimated token cost for one trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CandidateEstimate {
    pub trait_id: String,
    pub estimated_tokens: u64,
}

// ---------------------------------------------------------------------------
// Relation evidence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RelationEvidence {
    pub expansion_depth_used: u32,
    pub expansion_depth_allowed: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_edges: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_edges: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_edges: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_targets: Vec<String>,

    /// Whether relation expansion changed selected/rejected status.
    /// At MVP depth 0 this is always false.
    pub changed_status: bool,

    /// Capability note, e.g. `"relation-expansion-depth>0 unsupported in this phase"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub support_note: String,
}

// ---------------------------------------------------------------------------
// Resolve request and response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Request {
    pub task: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_hint: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_invocation: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Candidate {
    pub trait_id: String,
    pub version: String,
    pub active: bool,
    pub load_level: String,
    pub estimated_tokens: u64,
    pub priority: i32,
    pub score: i32,
    pub budget_decision: String,
    pub estimate_source: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reason_codes: Vec<String>,

    /// Exact remediation commands for this candidate's lifecycle/trust
    /// gates, kept separate from `reason_codes` so codes stay stable
    /// machine-readable identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remedies: Vec<String>,

    /// The trait's canonical source digest, populated by the CLI layer from
    /// the loaded inventory after `resolve` returns (P498 (f)). Additive and
    /// optional so every existing bare-object `resolve --json` parser keeps
    /// working unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<crate::digest::Digest>,

    /// The trait's rendered model-view content digest. Left `None` by
    /// `resolve` itself — computing it requires a full render, and `resolve`
    /// must stay a cheap planning verb (P498 §4 step 6); only `context plan`,
    /// which renders every selected trait anyway, ever populates this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_view_digest: Option<crate::digest::Digest>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct BudgetSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
    pub exceeded: bool,
}

/// Traits excluded by index prefiltering before full-load activation scoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct IndexRejection {
    pub trait_id: String,
    pub reason_codes: Vec<String>,

    /// Exact remediation commands for the status/trust gates behind this
    /// rejection, kept separate from `reason_codes` so codes stay stable
    /// machine-readable identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remedies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Response {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected: Vec<Candidate>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<Candidate>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index_rejections: Vec<IndexRejection>,

    pub budget: BudgetSummary,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilityReport>,

    pub relation_evidence: RelationEvidence,

    pub activation_report: ExplainReport,
}

/// Resolve activation plans for loaded traits under a token budget.
///
/// Uses `explain` for scoring, then applies budgeted load-level
/// selection. When `index_estimates` is supplied, discovery-index estimated
/// tokens are used for cost computation; otherwise a fallback estimate is
/// derived from trait text length. `lifecycle` carries the caller-resolved
/// `(package status, trust verdict)` pair for each entry in `traits`, in the
/// same order — the canonical trait document has no status/trust field of
/// its own.
pub fn resolve(
    request: &Request,
    traits: &[Trait],
    lifecycle: &[(PackageStatus, TrustVerdict)],
    index_estimates: &[CandidateEstimate],
    index_rejections: &[IndexRejection],
    all_discovered_trait_ids: &[&str],
) -> Response {
    let activation_request = ActivationRequest {
        task_text: request.task.clone(),
        mode: request.mode.clone(),
        files: request.files.clone(),
        language_hints: request.language_hints.clone(),
        explicit_invocation: request.explicit_invocation.clone(),
        signals: Vec::new(),
        trait_id: request.trait_id.clone(),
        capability_reports: Vec::new(),
    };

    let activation_report = explain(activation_request, traits, lifecycle);

    let estimate_map: std::collections::BTreeMap<&str, u64> = index_estimates
        .iter()
        .map(|e| (e.trait_id.as_str(), e.estimated_tokens))
        .collect();

    let mut selected = Vec::new();
    let mut rejected = Vec::new();
    let mut used: u64 = 0;
    let mut rejected_for_budget = false;
    let total = request.budget_tokens;

    let mut active_candidates: Vec<_> = activation_report
        .candidates
        .iter()
        .filter(|d| d.active)
        .collect();

    active_candidates.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.candidate.trait_id.cmp(&b.candidate.trait_id))
    });

    for decision in &active_candidates {
        let trait_ref = traits
            .iter()
            .find(|t| t.id.as_str() == decision.candidate.trait_id);

        let (discovery_cost, discovery_source) = match trait_ref {
            Some(t) => {
                if let Some(idx_tokens) = estimate_map.get(t.id.as_str()) {
                    (*idx_tokens, "discovery-index")
                } else {
                    (fallback_cost(t), "fallback")
                }
            }
            None => (UNKNOWN_FALLBACK_ESTIMATED_TOKENS, "fallback"),
        };

        let (full_cost, summary_cost) = trait_ref
            .map(|trait_ref| {
                let report = crate::model_view::compile_model_view(
                    trait_ref,
                    crate::render::ExtendedRenderProfile::AgentSkills,
                );
                (
                    crate::discovery_index::estimate_tokens(&report.behavior_text),
                    crate::discovery_index::estimate_tokens(&report.summary_text),
                )
            })
            .unwrap_or((
                UNKNOWN_FALLBACK_ESTIMATED_TOKENS,
                UNKNOWN_FALLBACK_ESTIMATED_TOKENS,
            ));

        // Determine desired load level from activation candidate preference.
        let desired = parse_desired_load_level(&decision.candidate.estimated_load_level);

        // Build ordered list of levels to try: desired down to discovery.
        let levels_to_try: Vec<(LoadLevel, u64, BudgetDecision, BudgetDecision)> = match desired {
            LoadLevel::Full => vec![
                (
                    LoadLevel::Full,
                    full_cost,
                    BudgetDecision::SelectedFull,
                    BudgetDecision::DowngradedToSummary,
                ),
                (
                    LoadLevel::Summary,
                    summary_cost,
                    BudgetDecision::SelectedSummary,
                    BudgetDecision::DowngradedToDiscovery,
                ),
                (
                    LoadLevel::Discovery,
                    discovery_cost,
                    BudgetDecision::SelectedDiscovery,
                    BudgetDecision::DowngradedToDiscovery,
                ),
            ],
            LoadLevel::Summary => vec![
                (
                    LoadLevel::Summary,
                    summary_cost,
                    BudgetDecision::SelectedSummary,
                    BudgetDecision::DowngradedToDiscovery,
                ),
                (
                    LoadLevel::Discovery,
                    discovery_cost,
                    BudgetDecision::SelectedDiscovery,
                    BudgetDecision::DowngradedToDiscovery,
                ),
            ],
            LoadLevel::Discovery => vec![(
                LoadLevel::Discovery,
                discovery_cost,
                BudgetDecision::SelectedDiscovery,
                BudgetDecision::SelectedDiscovery,
            )],
        };

        let mut placed = false;
        for (idx, (level, cost, select_decision, _downgrade_decision)) in
            levels_to_try.iter().enumerate()
        {
            if total.is_none_or(|budget| used.saturating_add(*cost) <= budget) {
                used = used.saturating_add(*cost);
                let final_decision = if idx == 0 {
                    *select_decision
                } else if *select_decision == BudgetDecision::SelectedFull {
                    BudgetDecision::SelectedFull
                } else if *level == LoadLevel::Discovery {
                    BudgetDecision::DowngradedToDiscovery
                } else {
                    BudgetDecision::DowngradedToSummary
                };
                let rc = if idx == 0 {
                    reason_codes_for_decision(decision)
                } else {
                    reason_codes_with(
                        decision,
                        &format!("budget-downgraded-to-{}", level.as_str()),
                    )
                };
                selected.push(make_candidate(
                    decision,
                    *level,
                    *cost,
                    final_decision,
                    estimate_source_for(*level, discovery_source),
                    rc,
                ));
                placed = true;
                break;
            }
        }

        if !placed {
            rejected_for_budget = true;
            rejected.push(make_candidate(
                decision,
                LoadLevel::Discovery,
                discovery_cost,
                BudgetDecision::RejectedBudgetExceeded,
                discovery_source,
                reason_codes_with(decision, "budget-exceeded"),
            ));
        }
    }

    for decision in &activation_report.candidates {
        if !decision.active {
            let (base_cost, estimate_source) = {
                if let Some(idx_tokens) = estimate_map.get(decision.candidate.trait_id.as_str()) {
                    (*idx_tokens, "discovery-index")
                } else if let Some(t) = traits
                    .iter()
                    .find(|t| t.id.as_str() == decision.candidate.trait_id)
                {
                    (fallback_cost(t), "fallback")
                } else {
                    (UNKNOWN_FALLBACK_ESTIMATED_TOKENS, "fallback")
                }
            };
            rejected.push(make_candidate(
                decision,
                LoadLevel::Discovery,
                base_cost,
                BudgetDecision::RejectedInactive,
                estimate_source,
                reason_codes_for_decision(decision),
            ));
        }
    }

    let exceeded = rejected_for_budget;
    let remaining = total.map(|budget| budget.saturating_sub(used));

    let relation_evidence = build_relation_evidence(&activation_report, all_discovered_trait_ids);

    selected.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.trait_id.cmp(&b.trait_id))
    });
    rejected.sort_by(|a, b| a.trait_id.cmp(&b.trait_id));

    Response {
        selected,
        rejected,
        index_rejections: index_rejections.to_vec(),
        budget: BudgetSummary {
            total,
            used,
            remaining,
            exceeded,
        },
        capabilities: activation_report.capabilities.clone(),
        relation_evidence,
        activation_report,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn fallback_cost(t: &Trait) -> u64 {
    let text_len = t.name.as_str().len() + t.description.as_str().len();
    let estimated = (text_len as u64).div_ceil(4);
    estimated.max(50)
}

fn estimate_source_for(level: LoadLevel, discovery_source: &str) -> &str {
    match level {
        LoadLevel::Full => "rendered-full",
        LoadLevel::Summary => "rendered-summary",
        LoadLevel::Discovery => discovery_source,
    }
}

fn make_candidate(
    decision: &crate::r#trait::activation::Decision,
    load_level: LoadLevel,
    estimated_tokens: u64,
    budget_decision: BudgetDecision,
    estimate_source: &str,
    reason_codes: Vec<String>,
) -> Candidate {
    Candidate {
        trait_id: decision.candidate.trait_id.clone(),
        version: decision.candidate.version.clone(),
        active: decision.active,
        load_level: load_level.as_str().to_string(),
        estimated_tokens,
        priority: decision.priority,
        score: decision.score,
        budget_decision: budget_decision.as_str().to_string(),
        estimate_source: estimate_source.to_string(),
        reason_codes,
        remedies: remedies_for_decision(decision),
        source_digest: None,
        model_view_digest: None,
    }
}

fn build_relation_evidence(
    report: &ExplainReport,
    all_known_trait_ids: &[&str],
) -> RelationEvidence {
    let rel = &report.relations;

    let mut required_edges = Vec::new();
    let mut suggested_edges = Vec::new();
    let mut conflict_edges = Vec::new();

    for edge in &rel.edges {
        let label = format!("{} -> {}", edge.source_trait_id, edge.target_trait_id);
        match edge.kind {
            crate::r#trait::relations::EdgeKind::Requires => {
                required_edges.push(label);
            }
            crate::r#trait::relations::EdgeKind::Suggests => {
                suggested_edges.push(label);
            }
            crate::r#trait::relations::EdgeKind::Conflicts => {
                conflict_edges.push(label);
            }
        }
    }

    let cycles: Vec<String> = rel.cycles.iter().map(|c| c.description.clone()).collect();

    // Populate unresolved relation targets: trait:* targets not in discovered/indexed set.
    let known_ids: std::collections::BTreeSet<&str> = all_known_trait_ids.iter().copied().collect();

    let mut unresolved_targets = Vec::new();
    for edge in &rel.edges {
        if let Some(target_ref) = &edge.target_ref
            && let Ok(parsed) = crate::reference::Reference::parse(target_ref)
            && parsed.kind() == crate::reference::Kind::Trait
        {
            let target_id = parsed.id();
            if !known_ids.contains(target_id) {
                unresolved_targets.push(target_ref.to_string());
            }
        }
    }
    unresolved_targets.sort();
    unresolved_targets.dedup();

    RelationEvidence {
        expansion_depth_used: 0,
        expansion_depth_allowed: 0,
        required_edges,
        suggested_edges,
        conflict_edges,
        cycles,
        unresolved_targets,
        changed_status: false,
        support_note: "relation-expansion-depth>0 unsupported in this phase".to_string(),
    }
}

fn parse_desired_load_level(s: &Option<String>) -> LoadLevel {
    match s.as_deref() {
        Some("discovery") => LoadLevel::Discovery,
        Some("summary") => LoadLevel::Summary,
        // Default to Full for unrecognized/missing values.
        _ => LoadLevel::Full,
    }
}

fn reason_codes_for_decision(decision: &crate::r#trait::activation::Decision) -> Vec<String> {
    let mut codes = decision.reason_codes.clone();
    if decision.active {
        codes.push("active".to_string());
    } else {
        codes.push("inactive".to_string());
    }
    codes.sort();
    codes.dedup();
    codes
}

/// Exact remediation commands for a decision's lifecycle/trust gates, kept
/// separate from `reason_codes_for_decision` so codes stay stable
/// machine-readable identifiers and remediation text lives in one place.
fn remedies_for_decision(decision: &crate::r#trait::activation::Decision) -> Vec<String> {
    let mut remedies: Vec<String> = decision
        .gates
        .iter()
        .filter_map(|gate| gate.remedy.clone())
        .collect();
    remedies.sort();
    remedies.dedup();
    remedies
}

fn reason_codes_with(decision: &crate::r#trait::activation::Decision, extra: &str) -> Vec<String> {
    let mut codes = reason_codes_for_decision(decision);
    codes.push(extra.to_string());
    codes.sort();
    codes.dedup();
    codes
}
