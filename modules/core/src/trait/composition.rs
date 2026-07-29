//! Contract contract and structured merge planning.
//!
//! `[composition]` declares static compatibility and merge behavior across
//! selected traits. It is canonical data only: it does not resolve activation,
//! bypass lifecycle/trust gates, call providers/models, or inspect runtime
//! values.
//!
//! Contract is structured, not raw concatenation. Additive fields (tone,
//! method, format) merge safely with source attribution; scalar fields
//! (verbosity, directness, etc.) report conflicts when they differ across
//! traits. Declared conflicts (trait/behavior), duplicate resources, and
//! incompatible port/schema pairs are reported explicitly — no hidden
//! precedence wins.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::reference::{Kind, Reference};
use crate::r#trait::{Behavior, Trait};

// ===========================================================================
// P33: Contract data model
// ===========================================================================

crate::shared::string_list_wrapper! {
    /// Scalar-or-array string list for composition typed refs.
    ///
    /// Authoring accepts a string or an array of strings; canonical serialization
    /// is always an array.
    #[schemars(rename = "CompositionRefList")]
    #[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
    pub struct RefList
}

/// The optional `[composition]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Contract {
    /// Traits or behavior axes that should not co-activate.
    /// Accepted ref kinds: `trait`, `behavior`.
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub conflict: RefList,

    /// How to combine with other traits: `structured`, `isolated`, `render-only`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_strategy: Option<String>,

    /// What to do when conflicts are found: `require-decision`,
    /// `warn-and-isolate`, `fail-plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_policy: Option<String>,
}

/// Validate a `[composition]` section independently of runtime composition.
pub fn validate(composition: &Contract) -> crate::Result<()> {
    for (i, raw) in composition.conflict.iter().enumerate() {
        let path = format!("composition.conflict[{i}]");
        let parsed = parse_composition_ref(raw, &path)?;
        match parsed.kind() {
            Kind::Trait | Kind::Behavior => {}
            Kind::Capability => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: path,
                    message: "capability refs are not valid composition conflicts; \
                     use trait: or behavior: refs. Port/schema conflicts are derived \
                     from compatibility/binding analysis, not authored here."
                        .to_string(),
                }
                .into());
            }
            other => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: path,
                    message: format!(
                        "composition.conflict must be trait: or behavior: ref, got {other}:"
                    ),
                }
                .into());
            }
        }
    }

    if let Some(ref strategy) = composition.merge_strategy {
        validate_merge_strategy(strategy)?;
    }
    if let Some(ref policy) = composition.conflict_policy {
        validate_conflict_policy(policy)?;
    }

    Ok(())
}

fn parse_composition_ref(raw: &str, field_path: &str) -> crate::Result<Reference> {
    Reference::parse(raw).map_err(|e| {
        crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("invalid composition ref {raw:?}: {e}"),
        }
        .into()
    })
}

fn validate_merge_strategy(value: &str) -> crate::Result<()> {
    match value {
        "structured" | "isolated" | "render-only" => Ok(()),
        _ => Err(crate::manifest::Error::InvalidField {
            field_path: "composition.merge-strategy".to_string(),
            message: "merge-strategy must be one of structured, isolated, or render-only"
                .to_string(),
        }
        .into()),
    }
}

fn validate_conflict_policy(value: &str) -> crate::Result<()> {
    match value {
        "require-decision" | "warn-and-isolate" | "fail-plan" => Ok(()),
        _ => Err(crate::manifest::Error::InvalidField {
            field_path: "composition.conflict-policy".to_string(),
            message:
                "conflict-policy must be one of require-decision, warn-and-isolate, or fail-plan"
                    .to_string(),
        }
        .into()),
    }
}

// ===========================================================================
// P35-fix: Structured composition plan types
// ===========================================================================

/// A source trait in a composition plan with a stable ordered index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SourceEntry {
    pub source_index: usize,
    pub trait_id: String,
    pub version: String,
}

/// A by-source section summary for CLI/JSON inspection.
///
/// One entry per modeled trait section (metadata, intent, behavior, activation,
/// composition, resource, schema, prompt, step, procedure, output). Does not
/// expose raw bodies — only stable IDs and compact summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SectionEntry {
    pub source_index: usize,
    pub trait_id: String,
    /// Section category: "metadata", "intent", "behavior", "activation",
    /// "composition", "resource", "schema", "prompt", "step", "procedure",
    /// "output".
    pub section: String,
    /// Canonical field path (e.g. `behavior.verbosity`, `resource[0].id`).
    pub field_path: String,
    /// Stable item ID when one exists (resource ID, prompt ID, schema ID, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Compact summary for inspection.
    pub summary: String,
}

/// A value with source attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AttributedValue {
    pub source_index: usize,
    pub trait_id: String,
    pub value: String,
}

/// Additive merge result: unioned values from multiple traits with attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AdditiveMerge {
    pub field_path: String,
    pub values: Vec<AttributedValue>,
}

/// One participant in a composition conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ConflictParticipant {
    pub source_index: usize,
    pub trait_id: String,
    pub field_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// What kind of composition conflict was detected.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ConflictKind {
    ScalarBehavior,
    DuplicateResourceId,
    IncompatiblePort,
    DeclaredConflict,
    UnsatisfiedPortRequirement,
    PolicyDisagreement,
}

impl ConflictKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One detected conflict in a composition plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Conflict {
    pub kind: ConflictKind,
    pub field_path: String,
    pub participants: Vec<ConflictParticipant>,
    pub description: String,
}

/// Advisory warning in a composition plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub trait_ids: Vec<String>,
}

/// Summary of merge-strategy and conflict-policy across selected traits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PolicySummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_strategies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_policies: Vec<String>,
    pub policies_disagree: bool,
    pub strategies_disagree: bool,
}

/// The conflict-resolution result of the structured plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PlanResult {
    /// No composition conflicts detected.
    NoConflicts,
    /// Conflicts require explicit user/profile choice.
    NeedsDecision,
    /// Conflicts should be isolated with warnings.
    WarnAndIsolate,
    /// Conflicts caused plan failure under fail-plan policy.
    Failed,
}

/// A structured composition plan over selected traits.
///
/// Produced by [`plan`]. Pure and deterministic: no IO, no
/// activation resolution, no provider/model calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Plan {
    pub sources: Vec<SourceEntry>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionEntry>,

    /// Union of output-port refs with source attribution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supplied_refs: Vec<AttributedValue>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additive_merges: Vec<AdditiveMerge>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflict>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,

    /// Proposed bindings between consumer input ports and provider output ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding_proposals: Vec<crate::r#trait::relations::Proposal>,

    /// Port compatibility evidence between all consumer/provider port pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_compatibility: Vec<crate::r#trait::relations::Evidence>,

    pub policy_summary: PolicySummary,
    pub result: PlanResult,
}

impl Plan {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

// ===========================================================================
// P35-fix: Deterministic composition context
// ===========================================================================

/// A source digest with index and trait ID for stable provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDigest {
    pub source_index: usize,
    pub trait_id: String,
    pub digest: Digest,
}

/// Deterministic composition context covering the full reported plan evidence.
///
/// Built from the structured plan and IO-edge source digests. Canonical JSON
/// serialization produces the context digest for provenance. The digest
/// changes whenever any reported source boundary, merge, conflict participant,
/// warning, binding proposal, policy, or result changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub plan: Plan,
    pub source_digests: Vec<SourceDigest>,
}

impl Context {
    /// Build a context from a plan and IO-edge source digests.
    pub fn from_plan(plan: &Plan, source_digests: &[Digest]) -> Self {
        let digests = plan
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| SourceDigest {
                source_index: i,
                trait_id: s.trait_id.clone(),
                digest: source_digests
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| Digest::from_unvalidated("")),
            })
            .collect();
        Self {
            plan: plan.clone(),
            source_digests: digests,
        }
    }
}

// ===========================================================================
// P35-fix: Planning
// ===========================================================================

/// Build a deterministic composition plan over already-loaded trusted traits.
///
/// Ordering is stable: source entries, section summaries, supplied refs,
/// additive merges, conflicts, and warnings follow input trait order.
/// The plan does not activate traits, resolve runtime capabilities, or call
/// providers/models.
pub fn plan(traits: &[Trait]) -> Plan {
    let sources: Vec<SourceEntry> = traits
        .iter()
        .enumerate()
        .map(|(i, t)| SourceEntry {
            source_index: i,
            trait_id: t.id.as_str().to_string(),
            version: t.version.as_str().to_string(),
        })
        .collect();

    let mut sections = Vec::new();
    for (i, t) in traits.iter().enumerate() {
        collect_sections(t, i, &mut sections);
    }

    let supplied_refs = collect_supplied_refs(traits);

    let additive_merges = collect_additive_merges(traits);

    let policy_summary = collect_policy_summary(traits);

    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();

    detect_declared_conflicts(traits, &supplied_refs, &mut conflicts);
    detect_scalar_behavior_conflicts(traits, &mut conflicts);
    detect_duplicate_resources(traits, &mut conflicts);

    // Collect port compatibility evidence and derive conflicts/warnings.
    let port_compatibility = collect_port_compatibility_all(traits);
    let trait_index_map: BTreeMap<String, usize> = traits
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str().to_string(), i))
        .collect();
    detect_port_conflicts_from_evidence(
        &port_compatibility,
        &trait_index_map,
        &mut conflicts,
        &mut warnings,
    );

    // Produce binding proposals between consumer input ports and provider output ports.
    let binding_proposals = collect_binding_proposals(traits);

    if policy_summary.policies_disagree {
        let participants: Vec<ConflictParticipant> = traits
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                t.composition.as_ref().and_then(|c| {
                    c.conflict_policy.as_ref().map(|p| ConflictParticipant {
                        source_index: i,
                        trait_id: t.id.as_str().to_string(),
                        field_path: "composition.conflict-policy".to_string(),
                        value: Some(p.clone()),
                    })
                })
            })
            .collect();
        conflicts.push(Conflict {
            kind: ConflictKind::PolicyDisagreement,
            field_path: "composition.conflict-policy".to_string(),
            participants,
            description: "selected traits use different conflict-policies".to_string(),
        });
    }

    if policy_summary.strategies_disagree {
        let participants: Vec<ConflictParticipant> = traits
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                t.composition.as_ref().and_then(|c| {
                    c.merge_strategy.as_ref().map(|s| ConflictParticipant {
                        source_index: i,
                        trait_id: t.id.as_str().to_string(),
                        field_path: "composition.merge-strategy".to_string(),
                        value: Some(s.clone()),
                    })
                })
            })
            .collect();
        conflicts.push(Conflict {
            kind: ConflictKind::PolicyDisagreement,
            field_path: "composition.merge-strategy".to_string(),
            participants,
            description: "selected traits use different merge-strategies".to_string(),
        });
    }

    collect_merge_strategy_warnings(traits, &conflicts, &mut warnings);

    let result = compute_plan_result(&conflicts, &policy_summary);

    Plan {
        sources,
        sections,
        supplied_refs,
        additive_merges,
        conflicts,
        warnings,
        binding_proposals,
        port_compatibility,
        policy_summary,
        result,
    }
}

// --- Section collection ---

fn collect_sections(t: &Trait, source_index: usize, sections: &mut Vec<SectionEntry>) {
    let tid = t.id.as_str();

    if t.metadata.is_some() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "metadata".to_string(),
            field_path: "metadata".to_string(),
            item_id: None,
            summary: "metadata present".to_string(),
        });
    }
    if t.intent.is_some() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "intent".to_string(),
            field_path: "intent".to_string(),
            item_id: None,
            summary: "intent present".to_string(),
        });
    }
    if let Some(ref behavior) = t.behavior {
        push_behavior_sections(behavior, tid, source_index, sections);
    }
    if t.activation.is_some() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "activation".to_string(),
            field_path: "activation".to_string(),
            item_id: None,
            summary: "activation rules present".to_string(),
        });
    }
    if let Some(ref comp) = t.composition {
        let parts: Vec<String> = [
            (!comp.conflict.is_empty()).then_some("conflict"),
            comp.merge_strategy.as_deref(),
            comp.conflict_policy.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(String::from)
        .collect();
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "composition".to_string(),
            field_path: "composition".to_string(),
            item_id: None,
            summary: parts.join(", "),
        });
    }
    for (ri, resource) in t.resources.iter().enumerate() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "resource".to_string(),
            field_path: format!("resource[{ri}].id"),
            item_id: Some(resource.id.as_str().to_string()),
            summary: resource
                .path
                .as_ref()
                .map(|path| format!("path={path}"))
                .unwrap_or_else(|| "source=inline".to_string()),
        });
    }
    for (si, schema) in t.schemas.iter().enumerate() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "schema".to_string(),
            field_path: format!("schema[{si}].id"),
            item_id: Some(schema.id.as_str().to_string()),
            summary: format!(
                "resource={}",
                schema.resource.as_deref().unwrap_or("inline-fields")
            ),
        });
    }
    for (pi, prompt_id) in t.prompts.keys().enumerate() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "prompt".to_string(),
            field_path: format!("prompt.{prompt_id}"),
            item_id: Some(prompt_id.to_string()),
            summary: format!("prompt {pi}"),
        });
    }
    if t.procedure.is_some() {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "procedure".to_string(),
            field_path: "procedure".to_string(),
            item_id: None,
            summary: "procedure present".to_string(),
        });
    }
    for port in &t.ports {
        sections.push(SectionEntry {
            source_index,
            trait_id: tid.to_string(),
            section: "port".to_string(),
            field_path: format!("port[{}].id", port.id),
            item_id: Some(port.id.clone()),
            summary: format!("direction={:?}", port.direction),
        });
    }
}

fn push_behavior_sections(
    behavior: &Behavior,
    tid: &str,
    source_index: usize,
    sections: &mut Vec<SectionEntry>,
) {
    for (field, values) in [
        ("tone", &behavior.tone),
        ("method", &behavior.method),
        ("format", &behavior.format),
    ] {
        if !values.is_empty() {
            let summary = values
                .iter()
                .map(|s| s.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(SectionEntry {
                source_index,
                trait_id: tid.to_string(),
                section: "behavior".to_string(),
                field_path: format!("behavior.{field}"),
                item_id: None,
                summary,
            });
        }
    }
    for (field, opt) in [
        ("verbosity", &behavior.verbosity),
        ("directness", &behavior.directness),
        ("scope-control", &behavior.scope_control),
        ("initiative", &behavior.initiative),
        ("uncertainty", &behavior.uncertainty),
    ] {
        if let Some(val) = opt {
            sections.push(SectionEntry {
                source_index,
                trait_id: tid.to_string(),
                section: "behavior".to_string(),
                field_path: format!("behavior.{field}"),
                item_id: None,
                summary: val.as_str().to_string(),
            });
        }
    }
}

// --- Supplied refs and additive merges ---

fn collect_supplied_refs(traits: &[Trait]) -> Vec<AttributedValue> {
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, t) in traits.iter().enumerate() {
        for port in &t.ports {
            if matches!(port.direction, crate::r#trait::PortDirection::Output) {
                let port_ref = format!("port:{}", port.id);
                if seen.insert(port_ref.clone()) {
                    refs.push(AttributedValue {
                        source_index: i,
                        trait_id: t.id.as_str().to_string(),
                        value: port_ref,
                    });
                }
            }
        }
    }
    refs
}

fn collect_additive_merges(traits: &[Trait]) -> Vec<AdditiveMerge> {
    let mut merges = Vec::new();
    for field in ["tone", "method", "format"] {
        let mut values = Vec::new();
        let mut seen = BTreeSet::new();
        for (i, t) in traits.iter().enumerate() {
            if let Some(ref behavior) = t.behavior {
                for slug in extract_additive_behavior(field, behavior).iter() {
                    if seen.insert(slug.as_str().to_string()) {
                        values.push(AttributedValue {
                            source_index: i,
                            trait_id: t.id.as_str().to_string(),
                            value: slug.as_str().to_string(),
                        });
                    }
                }
            }
        }
        if !values.is_empty() {
            merges.push(AdditiveMerge {
                field_path: format!("behavior.{field}"),
                values,
            });
        }
    }
    merges
}

fn extract_additive_behavior<'a>(
    field: &str,
    behavior: &'a Behavior,
) -> &'a crate::r#trait::GuidanceItemList {
    match field {
        "tone" => &behavior.tone,
        "method" => &behavior.method,
        "format" => &behavior.format,
        _ => &behavior.tone,
    }
}

// --- Policy summary ---

fn collect_policy_summary(traits: &[Trait]) -> PolicySummary {
    let mut strategies = Vec::new();
    let mut policies = Vec::new();
    for t in traits {
        if let Some(ref comp) = t.composition {
            if let Some(ref s) = comp.merge_strategy
                && !strategies.contains(s)
            {
                strategies.push(s.clone());
            }
            if let Some(ref p) = comp.conflict_policy
                && !policies.contains(p)
            {
                policies.push(p.clone());
            }
        }
    }
    let policies_disagree = policies.len() > 1;
    let strategies_disagree = strategies.len() > 1;
    PolicySummary {
        merge_strategies: strategies,
        conflict_policies: policies,
        policies_disagree,
        strategies_disagree,
    }
}

// --- Conflict detection ---

// detect_unsatisfied_inputs removed: composition.input is no longer modeled.

fn detect_declared_conflicts(
    traits: &[Trait],
    supplied: &[AttributedValue],
    conflicts: &mut Vec<Conflict>,
) {
    let _supplied_set: BTreeSet<&str> = supplied.iter().map(|v| v.value.as_str()).collect();
    for (i, t) in traits.iter().enumerate() {
        if let Some(ref comp) = t.composition {
            for conflict_ref in comp.conflict.iter() {
                let Ok(parsed) = Reference::parse(conflict_ref) else {
                    continue;
                };
                match parsed.kind() {
                    Kind::Trait => {
                        let target_id = parsed.ref_path().id();
                        for (j, other) in traits.iter().enumerate() {
                            if i != j && other.id.as_str() == target_id {
                                conflicts.push(Conflict {
                                    kind: ConflictKind::DeclaredConflict,
                                    field_path: format!("composition.conflict:{conflict_ref}"),
                                    participants: vec![
                                        ConflictParticipant {
                                            source_index: i,
                                            trait_id: t.id.as_str().to_string(),
                                            field_path: "composition.conflict".to_string(),
                                            value: Some(conflict_ref.clone()),
                                        },
                                        ConflictParticipant {
                                            source_index: j,
                                            trait_id: other.id.as_str().to_string(),
                                            field_path: "trait.id".to_string(),
                                            value: Some(target_id.to_string()),
                                        },
                                    ],
                                    description: format!(
                                        "trait {} declares conflict with {conflict_ref}",
                                        t.id.as_str()
                                    ),
                                });
                            }
                        }
                    }
                    Kind::Capability => {
                        // Capability conflicts are runtime host-support evidence,
                        // not trait dataflow. They are rejected by validate.
                        continue;
                    }
                    Kind::Behavior => {
                        for (j, other) in traits.iter().enumerate() {
                            if i == j {
                                continue;
                            }
                            // Match against derived behavior evidence atoms.
                            let evidence_match = collect_behavior_evidence(other)
                                .iter()
                                .any(|atom| atom == conflict_ref);
                            if evidence_match {
                                conflicts.push(Conflict {
                                    kind: ConflictKind::DeclaredConflict,
                                    field_path: format!("composition.conflict:{conflict_ref}"),
                                    participants: vec![
                                        ConflictParticipant {
                                            source_index: i,
                                            trait_id: t.id.as_str().to_string(),
                                            field_path: "composition.conflict".to_string(),
                                            value: Some(conflict_ref.clone()),
                                        },
                                        ConflictParticipant {
                                            source_index: j,
                                            trait_id: other.id.as_str().to_string(),
                                            field_path: "behavior".to_string(),
                                            value: Some(conflict_ref.clone()),
                                        },
                                    ],
                                    description: format!(
                                        "trait {} declares conflict with behavior {conflict_ref}",
                                        t.id.as_str()
                                    ),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn detect_scalar_behavior_conflicts(traits: &[Trait], conflicts: &mut Vec<Conflict>) {
    for field_name in [
        "verbosity",
        "directness",
        "scope-control",
        "initiative",
        "uncertainty",
    ] {
        let mut participants = Vec::new();
        for (i, t) in traits.iter().enumerate() {
            if let Some(ref behavior) = t.behavior
                && let Some(value) = extract_scalar_behavior(field_name, behavior)
            {
                participants.push(ConflictParticipant {
                    source_index: i,
                    trait_id: t.id.as_str().to_string(),
                    field_path: format!("behavior.{field_name}"),
                    value: Some(value),
                });
            }
        }
        if participants.len() > 1 {
            let distinct: BTreeSet<&str> = participants
                .iter()
                .filter_map(|p| p.value.as_deref())
                .collect();
            if distinct.len() > 1 {
                let values: Vec<String> = participants
                    .iter()
                    .filter_map(|p| p.value.clone())
                    .collect();
                conflicts.push(Conflict {
                    kind: ConflictKind::ScalarBehavior,
                    field_path: format!("behavior.{field_name}"),
                    participants,
                    description: format!(
                        "scalar behavior.{field_name} differs across traits: {}",
                        values.join(", ")
                    ),
                });
            }
        }
    }
}

fn extract_scalar_behavior(field: &str, behavior: &Behavior) -> Option<String> {
    match field {
        "verbosity" => behavior.verbosity.as_ref().map(|s| s.as_str().to_string()),
        "directness" => behavior.directness.as_ref().map(|s| s.as_str().to_string()),
        "scope-control" => behavior
            .scope_control
            .as_ref()
            .map(|s| s.as_str().to_string()),
        "initiative" => behavior.initiative.as_ref().map(|s| s.as_str().to_string()),
        "uncertainty" => behavior
            .uncertainty
            .as_ref()
            .map(|s| s.as_str().to_string()),
        _ => None,
    }
}

/// Derive deterministic `behavior:<atom>` evidence refs from modeled behavior.
///
/// Scalar fields produce both `behavior:<axis>-<value>` and `behavior:<value>`.
/// Additive slugs (tone, method, format) produce `behavior:<slug>`.
fn collect_behavior_evidence(t: &Trait) -> Vec<String> {
    let Some(ref behavior) = t.behavior else {
        return Vec::new();
    };
    let mut atoms = Vec::new();
    for (axis, opt) in [
        ("verbosity", &behavior.verbosity),
        ("directness", &behavior.directness),
        ("scope", &behavior.scope_control),
        ("initiative", &behavior.initiative),
        ("uncertainty", &behavior.uncertainty),
    ] {
        if let Some(val) = opt {
            atoms.push(format!("behavior:{axis}-{}", val.as_str()));
            atoms.push(format!("behavior:{}", val.as_str()));
        }
    }
    for slug in behavior
        .tone
        .iter()
        .chain(behavior.method.iter())
        .chain(behavior.format.iter())
    {
        atoms.push(format!("behavior:{slug}"));
    }
    atoms
}

fn detect_duplicate_resources(traits: &[Trait], conflicts: &mut Vec<Conflict>) {
    let mut seen: BTreeMap<&str, Vec<ConflictParticipant>> = BTreeMap::new();
    for (i, t) in traits.iter().enumerate() {
        for (ri, resource) in t.resources.iter().enumerate() {
            seen.entry(resource.id.as_str())
                .or_default()
                .push(ConflictParticipant {
                    source_index: i,
                    trait_id: t.id.as_str().to_string(),
                    field_path: format!("resource[{ri}].id"),
                    value: Some(resource.id.as_str().to_string()),
                });
        }
    }
    for (resource_id, owners) in &seen {
        if owners.len() > 1 {
            conflicts.push(Conflict {
                kind: ConflictKind::DuplicateResourceId,
                field_path: format!("resource:{resource_id}"),
                participants: owners.clone(),
                description: format!("resource id {resource_id:?} declared by multiple traits"),
            });
        }
    }
}

/// Collect port compatibility evidence between all consumer input ports and
/// provider output ports across all selected traits.
fn collect_port_compatibility_all(traits: &[Trait]) -> Vec<crate::r#trait::relations::Evidence> {
    let mut results = Vec::new();
    for consumer in traits {
        for provider in traits {
            if consumer.id == provider.id {
                continue;
            }
            let mut evidence = crate::r#trait::relations::collect_compatibility(consumer, provider);
            results.append(&mut evidence);
        }
    }
    results
}

/// Collect proposed bindings between consumer input ports and provider output
/// ports across all selected traits.
fn collect_binding_proposals(traits: &[Trait]) -> Vec<crate::r#trait::relations::Proposal> {
    let mut proposals = Vec::new();
    for consumer in traits {
        for provider in traits {
            if consumer.id == provider.id {
                continue;
            }
            let mut new = crate::r#trait::relations::produce_proposals(consumer, provider);
            proposals.append(&mut new);
        }
    }
    proposals.sort_by(|a, b| {
        (
            &a.consumer.trait_id,
            &a.consumer.port_id,
            &a.provider.trait_id,
            &a.provider.port_id,
        )
            .cmp(&(
                &b.consumer.trait_id,
                &b.consumer.port_id,
                &b.provider.trait_id,
                &b.provider.port_id,
            ))
    });
    proposals
}

/// Derive composition conflicts and warnings from port compatibility evidence.
///
/// - `Incompatible` → `ConflictKind::IncompatiblePort`
/// - `AnyWildcardWarning` → warning with code `composition.port-schema-any`
/// - `IoPending` → warning with code `composition.port-schema-io-pending`
fn detect_port_conflicts_from_evidence(
    evidence: &[crate::r#trait::relations::Evidence],
    trait_index_map: &BTreeMap<String, usize>,
    conflicts: &mut Vec<Conflict>,
    warnings: &mut Vec<Warning>,
) {
    for ev in evidence {
        match ev.outcome {
            crate::r#trait::relations::Outcome::Incompatible => {
                let consumer_idx = trait_index_map
                    .get(&ev.consumer.trait_id)
                    .copied()
                    .unwrap_or(usize::MAX);
                let provider_idx = trait_index_map
                    .get(&ev.provider.trait_id)
                    .copied()
                    .unwrap_or(usize::MAX);
                conflicts.push(Conflict {
                    kind: ConflictKind::IncompatiblePort,
                    field_path: format!("port[{}].schema", ev.consumer.port_id),
                    participants: vec![
                        ConflictParticipant {
                            source_index: consumer_idx,
                            trait_id: ev.consumer.trait_id.clone(),
                            field_path: format!("port[{}].schema", ev.consumer.port_id),
                            value: ev.consumer.schema_ref.as_ref().map(ToString::to_string),
                        },
                        ConflictParticipant {
                            source_index: provider_idx,
                            trait_id: ev.provider.trait_id.clone(),
                            field_path: format!("port[{}].schema", ev.provider.port_id),
                            value: ev.provider.schema_ref.as_ref().map(ToString::to_string),
                        },
                    ],
                    description: ev.reason.clone(),
                });
            }
            crate::r#trait::relations::Outcome::AnyWildcardWarning => {
                warnings.push(Warning {
                    code: "composition.port-schema-any".to_string(),
                    message: format!(
                        "port {}/{} uses schema:any wildcard with provider {}/{}",
                        ev.consumer.trait_id,
                        ev.consumer.port_id,
                        ev.provider.trait_id,
                        ev.provider.port_id
                    ),
                    trait_ids: vec![ev.consumer.trait_id.clone(), ev.provider.trait_id.clone()],
                });
            }
            crate::r#trait::relations::Outcome::IoPending => {
                warnings.push(Warning {
                    code: "composition.port-schema-io-pending".to_string(),
                    message: format!(
                        "port {}/{} schema comparison is IO-pending (resource-backed or dependency): {}",
                        ev.consumer.trait_id,
                        ev.consumer.port_id,
                        ev.reason
                    ),
                    trait_ids: vec![ev.consumer.trait_id.clone(), ev.provider.trait_id.clone()],
                });
            }
            _ => {}
        }
    }
}

fn collect_merge_strategy_warnings(
    traits: &[Trait],
    conflicts: &[Conflict],
    warnings: &mut Vec<Warning>,
) {
    let has_conflicts = !conflicts.is_empty();
    for t in traits {
        let Some(ref comp) = t.composition else {
            continue;
        };
        match comp.merge_strategy.as_deref() {
            Some("isolated") if has_conflicts => {
                warnings.push(Warning {
                    code: "composition.isolated-merge".to_string(),
                    message: format!(
                        "trait {} uses isolated merge-strategy; conflicts will be kept in separate sections",
                        t.id.as_str()
                    ),
                    trait_ids: vec![t.id.as_str().to_string()],
                });
            }
            Some("render-only") => {
                warnings.push(Warning {
                    code: "composition.render-only-merge".to_string(),
                    message: format!(
                        "trait {} uses render-only merge-strategy; semantic merging is deferred to render time",
                        t.id.as_str()
                    ),
                    trait_ids: vec![t.id.as_str().to_string()],
                });
            }
            _ => {}
        }
        if comp.conflict_policy.as_deref() == Some("warn-and-isolate") && has_conflicts {
            warnings.push(Warning {
                code: "composition.warn-and-isolate".to_string(),
                message: format!(
                    "trait {} uses warn-and-isolate policy; conflicting content will be isolated",
                    t.id.as_str()
                ),
                trait_ids: vec![t.id.as_str().to_string()],
            });
        }
    }
}

fn compute_plan_result(conflicts: &[Conflict], policy: &PolicySummary) -> PlanResult {
    if conflicts.is_empty() {
        return PlanResult::NoConflicts;
    }

    if policy.policies_disagree {
        return PlanResult::NeedsDecision;
    }

    let active_policy = policy.conflict_policies.first().map(String::as_str);
    match active_policy {
        Some("fail-plan") => PlanResult::Failed,
        Some("warn-and-isolate") => PlanResult::WarnAndIsolate,
        _ => PlanResult::NeedsDecision,
    }
}
