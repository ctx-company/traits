//! Declaration rule declarations.
//!
//! `[activation]` and `[[activation.rule]]` are canonical data only. They
//! describe candidate matching predicates for deterministic activation scoring,
//! but they do not bypass lifecycle/trust gates, load resources, call
//! providers/models, or inspect runtime values.
//!
//! Rules compose as OR. Inside one rule, non-empty positive predicate groups
//! compose as AND. Flat `exclude-file-glob` and `exclude-keyword` predicates
//! block a rule before positive scoring and do not make a rule usable alone.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::manifest::PackageStatus;
use crate::reference::{Kind, Reference};
use crate::response::CapabilityReport;
use crate::r#trait::relations::{Evaluation, build_graph, evaluate};
use crate::r#trait::{Trait, TrustVerdict};

crate::shared::string_list_wrapper! {
    /// Scalar-or-array string list for activation predicates.
    ///
    /// Authoring accepts either a string or an array of strings; canonical
    /// serialization is always an array.
    pub struct PredicateList
}

/// One `[[activation.rule]]` declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Rule {
    /// Stable rule ID. Required and unique within `[activation]`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    /// Human-readable reason shown by explanation surfaces.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,

    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub mode: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub language: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub file_glob: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub task_keyword: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub signal: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub explicit_phrase: PredicateList,

    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub exclude_file_glob: PredicateList,
    #[serde(default, skip_serializing_if = "PredicateList::is_empty")]
    pub exclude_keyword: PredicateList,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    /// Preferred initial load level: `discovery`, `summary`, or `full`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_level: Option<String>,
}

impl Rule {
    fn has_positive_predicate(&self) -> bool {
        !self.mode.is_empty()
            || !self.language.is_empty()
            || !self.file_glob.is_empty()
            || !self.task_keyword.is_empty()
            || !self.signal.is_empty()
            || !self.explicit_phrase.is_empty()
    }
}

/// The optional `[activation]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Declaration {
    /// Manual activation is declarative data only; later phases gate runtime use.
    #[serde(default, skip_serializing_if = "is_false")]
    pub manual: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score: Option<i32>,

    /// Ordered rules. Rules compose as OR during scoring.
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

/// Explicit host/runtime facts used to explain activation decisions.
///
/// These are request facts only. Search metadata from the trait is not folded
/// into activation evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Request {
    pub task_text: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_invocation: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,

    /// Optional request-side trait ID filter. Filtering is candidate gathering,
    /// not scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability_reports: Vec<CapabilityReport>,
}

/// Whether a candidate can be evaluated from local data alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DependencyStatus {
    LocalOnly,
    DependencyPending,
}

/// Stable summary of one activation rule for candidate gathering surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RuleSummary {
    pub id: String,
    pub reason: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positive_predicate_groups: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusion_predicate_groups: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_level: Option<String>,
}

/// Candidate data gathered before scoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Candidate {
    pub trait_id: String,
    pub version: String,
    pub lifecycle: PackageStatus,
    pub trust: TrustVerdict,
    pub dependency_status: DependencyStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_load_level: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_summaries: Vec<RuleSummary>,
}

/// A request fact that matched one activation predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct MatchedFact {
    pub kind: String,
    pub pattern: String,
    pub value: String,
}

/// Score and reason capture for one rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RuleScore {
    pub rule_id: String,
    pub reason: String,
    pub matched: bool,
    pub excluded: bool,
    pub score: i32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reason_codes: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_facts: Vec<MatchedFact>,
}

/// Lifecycle/trust gate applied after scoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Gate {
    pub code: String,
    pub message: String,
    /// Exact command that clears this gate, e.g. `ctx traits internal state --active <id>`,
    /// when one canonical command exists. `None` for gates with no single
    /// canonical command (e.g. manual-activation direct-invocation evidence,
    /// which can be satisfied several ways).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
}

/// Deterministic activation explanation for one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Decision {
    pub candidate: Candidate,
    pub active: bool,
    /// Rule-match score only. Priority never inflates this value.
    pub score: i32,
    pub min_score: i32,
    /// Declared `[activation].priority` carried as tie-breaker/evidence only.
    /// It never contributes to `score` or `score_accepted`.
    #[serde(default)]
    pub priority: i32,
    pub score_accepted: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleScore>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<Gate>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reason_codes: Vec<String>,
}

/// Declaration explanation report for CLI/JSON consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ExplainReport {
    pub request: Request,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Decision>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilityReport>,

    #[serde(default, skip_serializing_if = "Evaluation::is_empty")]
    pub relations: Evaluation,
}

/// Gather candidate metadata from a trusted trait model without scoring it.
///
/// `status`/`trust` are resolved by the caller from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn candidate_from_trait(t: &Trait, status: PackageStatus, trust: TrustVerdict) -> Candidate {
    let rule_summaries = t
        .activation
        .as_ref()
        .map(|a| a.rules.iter().map(rule_summary).collect())
        .unwrap_or_default();
    let estimated_load_level = t.activation.as_ref().and_then(first_load_level);

    Candidate {
        trait_id: t.id.as_str().to_string(),
        version: t.version.as_str().to_string(),
        lifecycle: status,
        trust,
        dependency_status: DependencyStatus::LocalOnly,
        estimated_load_level,
        rule_summaries,
    }
}

/// Explain activation decisions for a set of already-loaded traits.
///
/// `lifecycle` carries the caller-resolved `(package status, trust verdict)`
/// pair for each entry in `traits`, in the same order; the canonical trait
/// document has no status/trust field of its own.
///
/// Base activation scoring runs first, then relations are evaluated
/// against the set of active candidates. Relation edges can block, defer,
/// flag candidates as conflicting or cyclic. Lifecycle/trust gates are never
/// bypassed by relation edges.
pub fn explain(
    request: Request,
    traits: &[Trait],
    lifecycle: &[(PackageStatus, TrustVerdict)],
) -> ExplainReport {
    let mut candidates = Vec::new();
    for (t, (status, trust)) in traits.iter().zip(lifecycle.iter()) {
        if let Some(filter) = &request.trait_id
            && t.id.as_str() != filter
        {
            continue;
        }
        candidates.push(score(t, &request, *status, *trust));
    }

    let active_trait_ids: std::collections::BTreeSet<String> = candidates
        .iter()
        .filter(|d| d.active)
        .map(|d| d.candidate.trait_id.clone())
        .collect();

    // Collect matched activation rule IDs scoped by trait ID for `when` matching.
    let scoped_rule_facts: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        candidates
            .iter()
            .filter(|d| d.active)
            .map(|d| {
                let rules: std::collections::BTreeSet<String> = d
                    .rules
                    .iter()
                    .filter(|r| r.matched)
                    .map(|r| format!("rule:{}", r.rule_id))
                    .collect();
                (d.candidate.trait_id.clone(), rules)
            })
            .collect();

    // Parse validated signal facts from the request for relation `when` matching.
    let signal_facts: std::collections::BTreeSet<String> = request
        .signals
        .iter()
        .filter_map(|raw| Reference::parse(raw).ok())
        .filter(|r| r.kind() == Kind::Signal)
        .map(|r| r.to_string())
        .collect();

    let graph = build_graph(traits);
    let relations = if graph.edges.is_empty() && graph.cycles.is_empty() {
        Evaluation::default()
    } else {
        evaluate(
            &graph,
            &active_trait_ids,
            &scoped_rule_facts,
            &signal_facts,
            traits,
        )
    };

    ExplainReport {
        capabilities: request.capability_reports.clone(),
        request,
        candidates,
        relations,
    }
}

/// Score one loaded trait against explicit activation request facts.
///
/// Scoring is deterministic and pure. A trait activates only when at least one
/// positive rule matches with enough weight to satisfy `min-score`, no negative
/// predicate blocks the matched rules, `manual` direct-invocation evidence is
/// present when required, and lifecycle/trust gates pass. Priority is carried
/// as tie-breaker evidence only — it never inflates the match score.
pub fn score(t: &Trait, request: &Request, status: PackageStatus, trust: TrustVerdict) -> Decision {
    let candidate = candidate_from_trait(t, status, trust);

    let Some(activation) = t.activation.as_ref() else {
        let gates = lifecycle_trust_gates(t.id.as_str(), &status, &trust);
        return Decision {
            candidate,
            active: false,
            score: 0,
            min_score: 1,
            priority: 0,
            score_accepted: false,
            rules: Vec::new(),
            gates: gates.clone(),
            reason_codes: activation_none_reasons(&gates),
        };
    };

    let min_score = activation.min_score.unwrap_or(1);
    let priority = activation.priority.unwrap_or(0);

    let mut rules = Vec::with_capacity(activation.rules.len());
    for rule in &activation.rules {
        rules.push(score_rule(rule, request));
    }

    // Match score comes from matched, non-excluded rules only. Excluded and
    // unmatched rules contribute zero, so filtering on `matched` is equivalent
    // but makes the intent explicit.
    let score: i32 = rules.iter().filter(|r| r.matched).map(|r| r.score).sum();
    let matched_any = rules.iter().any(|r| r.matched);
    let score_accepted = matched_any && score >= min_score;

    let direct_evidence = has_direct_invocation_evidence(t, request, &rules);
    let mut gates = lifecycle_trust_gates(t.id.as_str(), &status, &trust);
    if activation.manual && !direct_evidence {
        gates.push(Gate {
            code: "manual.required".to_string(),
            message: "manual activation requires direct invocation evidence".to_string(),
            // No single canonical command clears this: direct-invocation
            // evidence can come from an explicit-phrase match, a direct
            // `trait-id` request, or an `explicit-invocation` value.
            remedy: None,
        });
    }

    let active = score_accepted && gates.is_empty();

    let mut reason_codes = Vec::new();
    if priority != 0 {
        reason_codes.push("priority.tie-breaker".to_string());
    }
    if score_accepted {
        reason_codes.push("score.accepted".to_string());
    } else {
        reason_codes.push("score.below-min-score".to_string());
    }
    for gate in &gates {
        reason_codes.push(gate.code.clone());
    }

    Decision {
        candidate,
        active,
        score,
        min_score,
        priority,
        score_accepted,
        rules,
        gates,
        reason_codes,
    }
}

/// Whether explicit request facts constitute direct invocation evidence.
///
/// Direct invocation is proven when the request carries a non-empty
/// `explicit-invocation`, the request `trait-id` matches this candidate, or at
/// least one matched (non-excluded) rule matched an `explicit-phrase`
/// predicate. Excluded rules never provide direct-invocation evidence.
fn has_direct_invocation_evidence(t: &Trait, request: &Request, rules: &[RuleScore]) -> bool {
    request
        .explicit_invocation
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
        || request
            .trait_id
            .as_ref()
            .is_some_and(|id| id == t.id.as_str())
        || rules
            .iter()
            .any(|r| r.matched && r.matched_facts.iter().any(|f| f.kind == "explicit-phrase"))
}

fn activation_none_reasons(gates: &[Gate]) -> Vec<String> {
    let mut codes = vec!["activation.none".to_string()];
    for gate in gates {
        codes.push(gate.code.clone());
    }
    codes
}

fn rule_summary(rule: &Rule) -> RuleSummary {
    RuleSummary {
        id: rule.id.clone(),
        reason: rule.reason.clone(),
        positive_predicate_groups: positive_predicate_groups(rule),
        exclusion_predicate_groups: exclusion_predicate_groups(rule),
        weight: rule.weight,
        load_level: rule.load_level.clone(),
    }
}

fn first_load_level(activation: &Declaration) -> Option<String> {
    activation
        .rules
        .iter()
        .find_map(|rule| rule.load_level.clone())
}

fn positive_predicate_groups(rule: &Rule) -> Vec<String> {
    let mut groups = Vec::new();
    push_group(&mut groups, "mode", &rule.mode);
    push_group(&mut groups, "language", &rule.language);
    push_group(&mut groups, "file-glob", &rule.file_glob);
    push_group(&mut groups, "task-keyword", &rule.task_keyword);
    push_group(&mut groups, "signal", &rule.signal);
    push_group(&mut groups, "explicit-phrase", &rule.explicit_phrase);
    groups
}

fn exclusion_predicate_groups(rule: &Rule) -> Vec<String> {
    let mut groups = Vec::new();
    push_group(&mut groups, "exclude-file-glob", &rule.exclude_file_glob);
    push_group(&mut groups, "exclude-keyword", &rule.exclude_keyword);
    groups
}

fn push_group(groups: &mut Vec<String>, name: &str, predicates: &PredicateList) {
    if !predicates.is_empty() {
        groups.push(name.to_string());
    }
}

fn score_rule(rule: &Rule, request: &Request) -> RuleScore {
    if let Some((code, facts)) = first_exclusion_match(rule, request) {
        return RuleScore {
            rule_id: rule.id.clone(),
            reason: rule.reason.clone(),
            matched: false,
            excluded: true,
            score: 0,
            reason_codes: vec![code],
            matched_facts: facts,
        };
    }

    let mut matched_facts = Vec::new();
    let mut reason_codes = Vec::new();
    let mut matched = true;

    apply_positive_group(
        &rule.mode,
        "mode",
        request
            .mode
            .as_ref()
            .map(|mode| vec![mode.as_str()])
            .unwrap_or_default(),
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );
    apply_positive_group(
        &rule.language,
        "language",
        request.language_hints.iter().map(String::as_str).collect(),
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );
    apply_file_glob_group(
        &rule.file_glob,
        request,
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );
    apply_contains_group(
        &rule.task_keyword,
        "task-keyword",
        std::slice::from_ref(&request.task_text),
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );
    apply_positive_group(
        &rule.signal,
        "signal",
        request.signals.iter().map(String::as_str).collect(),
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );
    let explicit_values = explicit_values(request);
    apply_contains_group(
        &rule.explicit_phrase,
        "explicit-phrase",
        &explicit_values,
        &mut matched,
        &mut reason_codes,
        &mut matched_facts,
    );

    RuleScore {
        rule_id: rule.id.clone(),
        reason: rule.reason.clone(),
        matched,
        excluded: false,
        score: if matched { rule.weight.unwrap_or(1) } else { 0 },
        reason_codes,
        matched_facts,
    }
}

fn first_exclusion_match(rule: &Rule, request: &Request) -> Option<(String, Vec<MatchedFact>)> {
    find_file_glob_match(&rule.exclude_file_glob, request)
        .map(|facts| ("excluded.file-glob".to_string(), facts))
        .or_else(|| {
            find_contains_match(
                &rule.exclude_keyword,
                "exclude-keyword",
                std::slice::from_ref(&request.task_text),
            )
            .map(|facts| ("excluded.keyword".to_string(), facts))
        })
}

fn apply_positive_group(
    predicates: &PredicateList,
    kind: &str,
    facts: Vec<&str>,
    matched: &mut bool,
    reason_codes: &mut Vec<String>,
    matched_facts: &mut Vec<MatchedFact>,
) {
    if predicates.is_empty() {
        return;
    }

    if let Some(mut facts) = find_exact_match(predicates, kind, facts) {
        reason_codes.push(format!("matched.{kind}"));
        matched_facts.append(&mut facts);
    } else {
        reason_codes.push(format!("missing.{kind}"));
        *matched = false;
    }
}

fn apply_contains_group(
    predicates: &PredicateList,
    kind: &str,
    values: &[String],
    matched: &mut bool,
    reason_codes: &mut Vec<String>,
    matched_facts: &mut Vec<MatchedFact>,
) {
    if predicates.is_empty() {
        return;
    }

    if let Some(mut facts) = find_contains_match(predicates, kind, values) {
        reason_codes.push(format!("matched.{kind}"));
        matched_facts.append(&mut facts);
    } else {
        reason_codes.push(format!("missing.{kind}"));
        *matched = false;
    }
}

fn apply_file_glob_group(
    predicates: &PredicateList,
    request: &Request,
    matched: &mut bool,
    reason_codes: &mut Vec<String>,
    matched_facts: &mut Vec<MatchedFact>,
) {
    if predicates.is_empty() {
        return;
    }

    if let Some(mut facts) = find_file_glob_match(predicates, request) {
        reason_codes.push("matched.file-glob".to_string());
        matched_facts.append(&mut facts);
    } else {
        reason_codes.push("missing.file-glob".to_string());
        *matched = false;
    }
}

fn find_exact_match(
    predicates: &PredicateList,
    kind: &str,
    values: Vec<&str>,
) -> Option<Vec<MatchedFact>> {
    let mut matches = Vec::new();
    for predicate in predicates.iter() {
        for value in &values {
            if value.eq_ignore_ascii_case(predicate) {
                matches.push(MatchedFact {
                    kind: kind.to_string(),
                    pattern: predicate.clone(),
                    value: (*value).to_string(),
                });
            }
        }
    }
    (!matches.is_empty()).then_some(matches)
}

fn find_contains_match(
    predicates: &PredicateList,
    kind: &str,
    values: &[String],
) -> Option<Vec<MatchedFact>> {
    let mut matches = Vec::new();
    for predicate in predicates.iter() {
        let predicate_lower = predicate.to_lowercase();
        for value in values {
            if value.to_lowercase().contains(&predicate_lower) {
                matches.push(MatchedFact {
                    kind: kind.to_string(),
                    pattern: predicate.clone(),
                    value: value.clone(),
                });
            }
        }
    }
    (!matches.is_empty()).then_some(matches)
}

fn find_file_glob_match(predicates: &PredicateList, request: &Request) -> Option<Vec<MatchedFact>> {
    let mut matches = Vec::new();
    for predicate in predicates.iter() {
        let Ok(glob) = globset::Glob::new(predicate) else {
            continue;
        };
        let matcher = glob.compile_matcher();
        for file in &request.files {
            if matcher.is_match(file) {
                matches.push(MatchedFact {
                    kind: "file-glob".to_string(),
                    pattern: predicate.clone(),
                    value: file.clone(),
                });
            }
        }
    }
    (!matches.is_empty()).then_some(matches)
}

fn explicit_values(request: &Request) -> Vec<String> {
    let mut values = vec![request.task_text.clone()];
    if let Some(explicit) = &request.explicit_invocation {
        values.push(explicit.clone());
    }
    values
}

/// Public wrapper for check/resolve surfaces to get the same lifecycle/trust
/// gates as activation. Returns the gate list; empty means pass.
///
/// `trait_id` is embedded into each gate's remedy command so refusals name the
/// exact `ctx traits internal state --active <id>` / `ctx traits trust --approved <id>` fix.
pub fn lifecycle_trust_gates_for_check(
    trait_id: &str,
    status: &PackageStatus,
    trust: &TrustVerdict,
) -> Vec<Gate> {
    lifecycle_trust_gates(trait_id, status, trust)
}

/// Check receipts use package manifest status as canonical package data;
/// trust is resolved separately from the machine trust store.
pub fn lifecycle_status_gates_for_check(trait_id: &str, status: &PackageStatus) -> Vec<Gate> {
    lifecycle_status_gates(trait_id, status)
}

/// Trust-only gate, for check receipts that report status and trust as two
/// independent sections rather than one combined lifecycle/trust decision.
pub fn trust_gates_for_check(trait_id: &str, trust: &TrustVerdict) -> Vec<Gate> {
    trust_gates(trait_id, trust)
}

fn lifecycle_trust_gates(
    trait_id: &str,
    status: &PackageStatus,
    trust: &TrustVerdict,
) -> Vec<Gate> {
    let mut gates = lifecycle_status_gates(trait_id, status);
    gates.extend(trust_gates(trait_id, trust));
    gates
}

fn trust_gates(trait_id: &str, trust: &TrustVerdict) -> Vec<Gate> {
    use crate::r#trait::gate_code;

    let mut gates = Vec::new();
    match trust {
        TrustVerdict::Blocked => gates.push(Gate {
            code: gate_code::TRUST_BLOCKED.to_string(),
            message: "traits blocked in the machine trust store cannot activate".to_string(),
            remedy: Some(format!("ctx traits trust --approved {trait_id}")),
        }),
        TrustVerdict::Unreviewed => gates.push(Gate {
            code: gate_code::TRUST_UNREVIEWED.to_string(),
            message:
                "traits with no verified record in the machine trust store do not auto-activate"
                    .to_string(),
            remedy: Some(format!("ctx traits trust --approved {trait_id}")),
        }),
        TrustVerdict::Verified => {}
    }
    gates
}

/// Format a gate list into one refusal message naming each gate's exact
/// remediation command when it has one. Shared by every caller that turns a
/// failed gate list into an error/refusal string, so remediation wording
/// never drifts between core, IO, and CLI surfaces.
pub fn format_gate_refusal(gates: &[Gate]) -> String {
    gates
        .iter()
        .map(|gate| match &gate.remedy {
            Some(remedy) => format!("{} ({}); run `{}`", gate.code, gate.message, remedy),
            None => format!("{} ({})", gate.code, gate.message),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn lifecycle_status_gates(trait_id: &str, status: &PackageStatus) -> Vec<Gate> {
    use crate::r#trait::gate_code;
    let mut gates = Vec::new();
    if matches!(status, PackageStatus::Draft) {
        gates.push(Gate {
            code: gate_code::STATUS_DRAFT.to_string(),
            message: "draft traits do not auto-activate".to_string(),
            remedy: Some(format!("ctx traits internal state --active {trait_id}")),
        });
    }
    gates
}

/// Validate activation declarations independently of runtime activation.
pub fn validate(activation: &Declaration) -> crate::Result<()> {
    if let Some(min_score) = activation.min_score
        && min_score < 0
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "activation.min-score".to_string(),
            message: "must be zero or greater".to_string(),
        }
        .into());
    }

    let mut seen_ids = BTreeSet::new();
    for (i, rule) in activation.rules.iter().enumerate() {
        validate_rule(rule, i, &mut seen_ids)?;
    }

    Ok(())
}

fn validate_rule(rule: &Rule, index: usize, seen_ids: &mut BTreeSet<String>) -> crate::Result<()> {
    let base = format!("activation.rule[{index}]");
    let id_path = format!("{base}.id");

    if rule.id.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: id_path,
            message: "id is required".to_string(),
        }
        .into());
    }
    crate::shared::validate_slug_shape(&rule.id, &id_path)?;
    if !seen_ids.insert(rule.id.clone()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: id_path,
            message: format!("duplicate activation rule id {:?}", rule.id),
        }
        .into());
    }

    if rule.reason.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.reason"),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    if !rule.has_positive_predicate() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: base.clone(),
            message: "activation rule must contain at least one positive predicate group"
                .to_string(),
        }
        .into());
    }

    validate_predicates(&rule.mode, &format!("{base}.mode"))?;
    validate_predicates(&rule.language, &format!("{base}.language"))?;
    validate_globs(&rule.file_glob, &format!("{base}.file-glob"))?;
    validate_predicates(&rule.task_keyword, &format!("{base}.task-keyword"))?;
    validate_predicates(&rule.signal, &format!("{base}.signal"))?;
    validate_predicates(&rule.explicit_phrase, &format!("{base}.explicit-phrase"))?;
    validate_globs(
        &rule.exclude_file_glob,
        &format!("{base}.exclude-file-glob"),
    )?;
    validate_predicates(&rule.exclude_keyword, &format!("{base}.exclude-keyword"))?;

    if let Some(weight) = rule.weight
        && weight <= 0
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.weight"),
            message: "must be greater than zero".to_string(),
        }
        .into());
    }

    if let Some(load_level) = &rule.load_level {
        validate_load_level(load_level, &format!("{base}.load-level"))?;
    }

    Ok(())
}

fn validate_predicates(list: &PredicateList, field_path: &str) -> crate::Result<()> {
    for (i, value) in list.iter().enumerate() {
        if value.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{i}]"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_load_level(value: &str, field_path: &str) -> crate::Result<()> {
    match value {
        "discovery" | "summary" | "full" => Ok(()),
        _ => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "load-level must be one of discovery, summary, or full".to_string(),
        }
        .into()),
    }
}

fn validate_globs(list: &PredicateList, field_path: &str) -> crate::Result<()> {
    for (i, pattern) in list.iter().enumerate() {
        let path = format!("{field_path}[{i}]");
        if pattern.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: path,
                message: "must not be empty".to_string(),
            }
            .into());
        }
        globset::Glob::new(pattern).map_err(|e| crate::manifest::Error::InvalidField {
            field_path: path,
            message: format!("invalid file glob {:?}: {e}", pattern),
        })?;
    }
    Ok(())
}

fn is_false(value: &bool) -> bool {
    !*value
}
