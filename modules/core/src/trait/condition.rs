//! Typed condition declarations and guard predicates.
//!
//! Conditions are deliberately small: signal presence, named condition refs,
//! slot predicates, ordered numeric comparisons, iteration predicates, and
//! closed logical composition. There is no expression language, script hook,
//! regex, provider call, or raw transcript matching.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::digest::Digest;
use crate::reference::{Kind, Reference};
use crate::r#trait::Trait;

const MAX_CONDITION_NESTING_DEPTH: usize = 32;

/// One guard expression. A string ref is either `signal:<id>` or
/// `condition:<id>`. An array is OR/any composition.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(untagged)]
pub enum GuardExpr {
    Ref(String),
    Any(Vec<GuardExpr>),
    Predicate(Box<GuardPredicate>),
}

impl Serialize for GuardExpr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Ref(ref_text) => serializer.serialize_str(ref_text),
            Self::Any(items) => items.serialize(serializer),
            Self::Predicate(predicate) => predicate.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for GuardExpr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(ref_text) => Ok(Self::Ref(ref_text)),
            Value::Array(items) => items
                .into_iter()
                .map(|item| serde_json::from_value(item).map_err(de::Error::custom))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Any),
            Value::Object(_) => serde_json::from_value(value)
                .map(Box::new)
                .map(Self::Predicate)
                .map_err(|e| de::Error::custom(format!("invalid guard predicate object: {e}"))),
            other => Err(de::Error::custom(format!(
                "guard must be a signal/condition ref string, guard array, or predicate object; got {other}"
            ))),
        }
    }
}

/// Inline guard predicate form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct GuardPredicate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u64>,
    #[serde(
        default,
        rename = "iteration-at-least",
        skip_serializing_if = "Option::is_none"
    )]
    pub iteration_at_least: Option<u64>,
    /// Cumulative active-drive elapsed seconds (runtime-supplied evidence,
    /// never a core wall clock) at least this threshold. Threshold is a JSON
    /// number or a `{ ref = "slot:..."/"port:..." }` numeric reference.
    #[serde(
        default,
        rename = "elapsed-seconds-at-least",
        skip_serializing_if = "Option::is_none"
    )]
    pub elapsed_seconds_at_least: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<GuardExpr>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    /// Presence-aware leaf (schema-version `"0.3"` only). Bare form is a local
    /// optional input `port:*`; with `field`, a declared optional field of a
    /// local `port:*`/`slot:*` object container. Yields `Matched`/`NotMatched`/`Unmeasurable`
    /// tri-state internally; only `Matched` routes `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    /// Filters `count` to elements whose `field` equals this value. Paired
    /// with `field`; meaningless alone. Kept distinct from `equals`, which is
    /// already the count's threshold — one predicate needs both a "which
    /// elements" and a "how many", and they cannot share a spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub less_than: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_most: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greater_than: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_least: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<GuardExpr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<GuardExpr>,
}

/// A named condition body keyed by `[condition.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Condition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u64>,
    #[serde(
        default,
        rename = "iteration-at-least",
        skip_serializing_if = "Option::is_none"
    )]
    pub iteration_at_least: Option<u64>,
    #[serde(
        default,
        rename = "elapsed-seconds-at-least",
        skip_serializing_if = "Option::is_none"
    )]
    pub elapsed_seconds_at_least: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<GuardExpr>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub less_than: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_most: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greater_than: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_least: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<GuardExpr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<GuardExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Condition {
    pub fn as_guard(&self) -> GuardExpr {
        GuardExpr::Predicate(Box::new(GuardPredicate {
            signal: self.signal.clone(),
            condition: self.condition.clone(),
            slot: self.slot.clone(),
            output: self.output.clone(),
            field: self.field.clone(),
            equals: self.equals.clone(),
            iteration: self.iteration,
            iteration_at_least: self.iteration_at_least,
            elapsed_seconds_at_least: self.elapsed_seconds_at_least.clone(),
            not: self.not.clone(),
            empty: self.empty.clone(),
            present: self.present.clone(),
            count: self.count.clone(),
            field_equals: self.field_equals.clone(),
            less_than: self.less_than.clone(),
            at_most: self.at_most.clone(),
            greater_than: self.greater_than.clone(),
            at_least: self.at_least.clone(),
            all: self.all.clone(),
            any: self.any.clone(),
        }))
    }
}

/// Duplicate-aware map of named conditions keyed by condition ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct ConditionMap(BTreeMap<String, Condition>);

impl ConditionMap {
    pub fn get(&self, key: &str) -> Option<&Condition> {
        self.0.get(key)
    }

    pub fn keys(&self) -> std::collections::btree_map::Keys<'_, String, Condition> {
        self.0.keys()
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, Condition> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for ConditionMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ConditionMapVisitor;

        impl<'de> Visitor<'de> for ConditionMapVisitor {
            type Value = ConditionMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a condition object/map keyed by condition id")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut map = BTreeMap::new();
                while let Some((id, body)) = access.next_entry::<String, Condition>()? {
                    if map.contains_key(&id) {
                        return Err(de::Error::custom(format!(
                            "duplicate condition key at condition.{id}"
                        )));
                    }
                    map.insert(id, body);
                }
                Ok(ConditionMap(map))
            }
        }

        deserializer.deserialize_map(ConditionMapVisitor)
    }
}

/// Tri-state outcome of one guard leaf/combinator. `Unmeasurable` and
/// `NotMatched` route identically (`false`) at the boundary — the distinction
/// only matters for `not`, whose fail-closed rule is the reason this type
/// exists rather than a plain `bool`: `not(Unmeasurable)` stays `Unmeasurable`,
/// never `Matched`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum GuardOutcome {
    Matched,
    NotMatched,
    Unmeasurable,
}

impl GuardOutcome {
    pub fn from_bool(matched: bool) -> Self {
        if matched {
            Self::Matched
        } else {
            Self::NotMatched
        }
    }

    /// Only `Matched` routes `true` at a guard boundary.
    pub fn routes_true(self) -> bool {
        matches!(self, Self::Matched)
    }

    pub fn negate(self) -> Self {
        match self {
            Self::Matched => Self::NotMatched,
            Self::NotMatched => Self::Matched,
            Self::Unmeasurable => Self::Unmeasurable,
        }
    }

    /// Strong-Kleene AND: `NotMatched` dominates, then `Unmeasurable`.
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::NotMatched, _) | (_, Self::NotMatched) => Self::NotMatched,
            (Self::Unmeasurable, _) | (_, Self::Unmeasurable) => Self::Unmeasurable,
            (Self::Matched, Self::Matched) => Self::Matched,
        }
    }

    /// Strong-Kleene OR: `Matched` dominates, then `Unmeasurable`.
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Matched, _) | (_, Self::Matched) => Self::Matched,
            (Self::Unmeasurable, _) | (_, Self::Unmeasurable) => Self::Unmeasurable,
            (Self::NotMatched, Self::NotMatched) => Self::NotMatched,
        }
    }
}

/// Runtime evidence for one evaluated guard predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ConditionEvaluation {
    pub predicate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ConditionEvaluationScope>,
    /// Replayable operands for a ref-backed equality or ordered comparison.
    /// Absent on aggregate/non-comparison evaluations and on older ledgers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison_evidence: Option<ConditionComparisonEvidence>,
    /// Tri-state outcome, serialized only when it is `Unmeasurable` — every
    /// `Matched`/`NotMatched` evaluation is fully described by `matched`
    /// already, so this stays absent and no `0.2` ledger byte moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<GuardOutcome>,
    pub matched: bool,
    pub reason: String,
}

/// The authored LHS form of a ref-backed comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ConditionComparisonSubject {
    Slot,
    Output,
    /// Runtime-supplied cumulative active-drive elapsed seconds. There is no
    /// backing ref — the LHS operand is the exact evidence value observed at
    /// evaluation time, embedded as a literal so ledger replay can verify it.
    Elapsed,
}

/// Closed comparison operators stored in runtime evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ConditionComparisonOperator {
    Equals,
    LessThan,
    AtMost,
    GreaterThan,
    AtLeast,
}

impl ConditionComparisonOperator {
    pub(crate) fn matches_ordering(self, ordering: std::cmp::Ordering) -> bool {
        match self {
            Self::Equals => ordering.is_eq(),
            Self::LessThan => ordering.is_lt(),
            Self::AtMost => ordering.is_le(),
            Self::GreaterThan => ordering.is_gt(),
            Self::AtLeast => ordering.is_ge(),
        }
    }

    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Self::Equals => "==",
            Self::LessThan => "<",
            Self::AtMost => "<=",
            Self::GreaterThan => ">",
            Self::AtLeast => ">=",
        }
    }
}

/// One comparison operand and the exact source from which it was selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ComparisonOperandEvidence {
    /// A value selected from an accepted runtime ref.
    Ref {
        #[serde(rename = "ref")]
        #[schemars(rename = "ref")]
        ref_text: String,
        #[serde(rename = "source-value-digest")]
        #[schemars(rename = "source-value-digest")]
        source_value_digest: Digest,
        /// Embedded for current outputs whose accepted record may not retain
        /// the exact historical value; otherwise omitted.
        #[serde(
            default,
            rename = "source-value",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "source-value")]
        source_value: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(
            default,
            rename = "selected-value",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "selected-value")]
        selected_value: Option<Value>,
        #[serde(
            default,
            rename = "slot-revision-acceptance-order",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "slot-revision-acceptance-order")]
        slot_revision_acceptance_order: Option<usize>,
    },
    /// A ref for which no accepted runtime value was visible.
    MissingRef {
        #[serde(rename = "ref")]
        #[schemars(rename = "ref")]
        ref_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    /// An authored literal. It deliberately has no fabricated ref or digest.
    Literal { value: Value },
}

/// Replayable evidence for one atomic ref-backed comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ConditionComparisonEvidence {
    pub subject: ConditionComparisonSubject,
    pub lhs: ComparisonOperandEvidence,
    pub operator: ConditionComparisonOperator,
    pub rhs: ComparisonOperandEvidence,
    /// Result of the comparison before LHS loop-freshness gating.
    pub result: bool,
    /// True when the LHS slot was written in an earlier iteration of the
    /// evaluated loop. RHS refs intentionally do not carry this gate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// Structured scope for one guard evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ConditionEvaluationScope {
    pub loop_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    pub iteration_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
}

/// Validate all named condition declarations.
pub fn validate_conditions(trait_ref: &Trait) -> crate::Result<()> {
    let slot_ids: BTreeSet<&str> = trait_ref
        .slots
        .iter()
        .map(|slot| slot.id.as_str())
        .collect();
    let signal_ids: BTreeSet<&str> = trait_ref
        .signals
        .iter()
        .map(|signal| signal.id.as_str())
        .collect();

    for (id, condition) in trait_ref.conditions.iter() {
        crate::shared::validate_slug_shape(id, &format!("condition.{id}"))?;
        validate_guard_expr(
            trait_ref,
            &condition.as_guard(),
            &format!("condition.{id}"),
            &slot_ids,
            &signal_ids,
            true,
            false,
        )?;
    }
    validate_condition_graph(trait_ref)?;
    Ok(())
}

fn validate_condition_graph(trait_ref: &Trait) -> crate::Result<()> {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, condition) in trait_ref.conditions.iter() {
        let mut edges = Vec::new();
        collect_condition_ref_edges(&condition.as_guard(), &mut edges)?;
        graph.insert(id.clone(), edges);
    }

    let mut memo = BTreeMap::new();
    for id in trait_ref.conditions.keys() {
        let mut stack = Vec::new();
        validate_condition_depth(id, &graph, &mut stack, &mut memo)?;
    }
    Ok(())
}

fn collect_condition_ref_edges(guard: &GuardExpr, edges: &mut Vec<String>) -> crate::Result<()> {
    match guard {
        GuardExpr::Ref(ref_text) => {
            let parsed =
                Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: "condition".to_string(),
                    message: format!("invalid guard ref {ref_text:?}"),
                })?;
            if parsed.kind() == Kind::Condition && !parsed.is_qualified() {
                edges.push(parsed.id().to_string());
            }
        }
        GuardExpr::Any(items) => {
            for item in items {
                collect_condition_ref_edges(item, edges)?;
            }
        }
        GuardExpr::Predicate(predicate) => {
            if let Some(condition) = predicate.condition.as_deref() {
                let parsed = Reference::parse(condition).map_err(|_| {
                    crate::manifest::Error::InvalidField {
                        field_path: "condition.condition".to_string(),
                        message: format!("invalid condition ref {condition:?}"),
                    }
                })?;
                if parsed.kind() == Kind::Condition && !parsed.is_qualified() {
                    edges.push(parsed.id().to_string());
                }
            }
            if let Some(not) = predicate.not.as_deref() {
                collect_condition_ref_edges(not, edges)?;
            }
            for item in &predicate.all {
                collect_condition_ref_edges(item, edges)?;
            }
            for item in &predicate.any {
                collect_condition_ref_edges(item, edges)?;
            }
        }
    }
    Ok(())
}

fn iteration_dependent_condition_ids(trait_ref: &Trait) -> BTreeSet<String> {
    let mut direct = BTreeSet::new();
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, condition) in trait_ref.conditions.iter() {
        let guard = condition.as_guard();
        if guard_directly_uses_iteration(&guard) {
            direct.insert(id.clone());
        }
        let mut edges = Vec::new();
        let _ = collect_condition_ref_edges(&guard, &mut edges);
        graph.insert(id.clone(), edges);
    }
    let mut dependent = BTreeSet::new();
    for id in trait_ref.conditions.keys() {
        if condition_depends_on_iteration(id, &direct, &graph, &mut BTreeSet::new()) {
            dependent.insert(id.clone());
        }
    }
    dependent
}

fn condition_depends_on_iteration(
    id: &str,
    direct: &BTreeSet<String>,
    graph: &BTreeMap<String, Vec<String>>,
    seen: &mut BTreeSet<String>,
) -> bool {
    if direct.contains(id) {
        return true;
    }
    if !seen.insert(id.to_string()) {
        return false;
    }
    graph.get(id).is_some_and(|edges| {
        edges
            .iter()
            .any(|edge| condition_depends_on_iteration(edge, direct, graph, seen))
    })
}

fn guard_directly_uses_iteration(guard: &GuardExpr) -> bool {
    match guard {
        GuardExpr::Ref(_) => false,
        GuardExpr::Any(items) => items.iter().any(guard_directly_uses_iteration),
        GuardExpr::Predicate(predicate) => {
            predicate.iteration.is_some()
                || predicate.iteration_at_least.is_some()
                || predicate
                    .not
                    .as_deref()
                    .is_some_and(guard_directly_uses_iteration)
                || predicate.all.iter().any(guard_directly_uses_iteration)
                || predicate.any.iter().any(guard_directly_uses_iteration)
        }
    }
}

fn validate_condition_depth(
    current: &str,
    graph: &BTreeMap<String, Vec<String>>,
    stack: &mut Vec<String>,
    memo: &mut BTreeMap<String, usize>,
) -> crate::Result<usize> {
    if let Some(depth) = memo.get(current) {
        if stack.len().saturating_add(*depth) > MAX_CONDITION_NESTING_DEPTH {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("condition.{current}"),
                message: format!(
                    "condition nesting exceeds maximum depth {MAX_CONDITION_NESTING_DEPTH}"
                ),
            }
            .into());
        }
        return Ok(*depth);
    }
    if stack.iter().any(|item| item == current) {
        let start = stack.iter().position(|item| item == current).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(current.to_string());
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("condition.{current}"),
            message: format!(
                "recursive/cyclic condition refs are not allowed: {}",
                cycle.join(" -> ")
            ),
        }
        .into());
    }
    if stack.len() >= MAX_CONDITION_NESTING_DEPTH {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("condition.{current}"),
            message: format!(
                "condition nesting exceeds maximum depth {MAX_CONDITION_NESTING_DEPTH}"
            ),
        }
        .into());
    }

    stack.push(current.to_string());
    let mut max_child_depth = 0;
    if let Some(edges) = graph.get(current) {
        for edge in edges {
            max_child_depth =
                max_child_depth.max(validate_condition_depth(edge, graph, stack, memo)?);
        }
    }
    stack.pop();
    let depth = max_child_depth + 1;
    memo.insert(current.to_string(), depth);
    Ok(depth)
}

pub fn validate_guard_expr(
    trait_ref: &Trait,
    guard: &GuardExpr,
    field_path: &str,
    slot_ids: &BTreeSet<&str>,
    signal_ids: &BTreeSet<&str>,
    allow_iteration: bool,
    allow_output: bool,
) -> crate::Result<()> {
    let validation = GuardValidation {
        trait_ref,
        slot_ids,
        signal_ids,
        allow_iteration,
        allow_output,
    };
    validate_guard_expr_at_depth(&validation, guard, field_path, 0)
}

struct GuardValidation<'a> {
    trait_ref: &'a Trait,
    slot_ids: &'a BTreeSet<&'a str>,
    signal_ids: &'a BTreeSet<&'a str>,
    allow_iteration: bool,
    allow_output: bool,
}

fn validate_guard_expr_at_depth(
    validation: &GuardValidation<'_>,
    guard: &GuardExpr,
    field_path: &str,
    not_depth: usize,
) -> crate::Result<()> {
    if not_depth > MAX_CONDITION_NESTING_DEPTH {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "condition nesting exceeds maximum depth {MAX_CONDITION_NESTING_DEPTH}"
            ),
        }
        .into());
    }
    match guard {
        GuardExpr::Ref(ref_text) => validate_guard_ref(
            validation.trait_ref,
            ref_text,
            field_path,
            validation.signal_ids,
            validation.allow_iteration,
        ),
        GuardExpr::Any(items) => {
            if items.is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: "guard array must not be empty".to_string(),
                }
                .into());
            }
            for (index, item) in items.iter().enumerate() {
                validate_guard_expr_at_depth(
                    validation,
                    item,
                    &format!("{field_path}[{index}]"),
                    not_depth,
                )?;
            }
            Ok(())
        }
        GuardExpr::Predicate(predicate) => {
            validate_guard_predicate(validation, predicate, field_path, not_depth)
        }
    }
}

fn validate_guard_ref(
    trait_ref: &Trait,
    ref_text: &str,
    field_path: &str,
    signal_ids: &BTreeSet<&str>,
    allow_iteration: bool,
) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid guard ref {ref_text:?}"),
    })?;
    match parsed.kind() {
        Kind::Signal => {
            if !parsed.is_qualified() && !signal_ids.contains(parsed.id()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!("unresolved local signal ref {ref_text:?}"),
                }
                .into());
            }
        }
        Kind::Condition => {
            if parsed.is_qualified() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: "dependency-qualified condition refs are not supported yet"
                        .to_string(),
                }
                .into());
            }
            if trait_ref.conditions.get(parsed.id()).is_none() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: format!("unresolved local condition ref {ref_text:?}"),
                }
                .into());
            }
            if !allow_iteration
                && iteration_dependent_condition_ids(trait_ref).contains(parsed.id())
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: field_path.to_string(),
                    message: "iteration-dependent condition refs are valid only inside loop guards"
                        .to_string(),
                }
                .into());
            }
        }
        other => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: format!(
                    "guard ref kind {other:?} not allowed; expected signal or condition"
                ),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_guard_predicate(
    validation: &GuardValidation<'_>,
    predicate: &GuardPredicate,
    field_path: &str,
    not_depth: usize,
) -> crate::Result<()> {
    let mut forms = 0;
    forms += usize::from(predicate.signal.is_some());
    forms += usize::from(predicate.condition.is_some());
    forms += usize::from(predicate.slot.is_some());
    forms += usize::from(predicate.output.is_some());
    forms += usize::from(predicate.iteration.is_some());
    forms += usize::from(predicate.iteration_at_least.is_some());
    forms += usize::from(predicate.elapsed_seconds_at_least.is_some());
    forms += usize::from(predicate.not.is_some());
    forms += usize::from(predicate.empty.is_some());
    forms += usize::from(predicate.present.is_some());
    forms += usize::from(predicate.count.is_some());
    forms += usize::from(!predicate.all.is_empty());
    forms += usize::from(!predicate.any.is_empty());
    if forms != 1 {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "guard predicate must declare exactly one of signal, condition, slot, output, iteration, iteration-at-least, elapsed-seconds-at-least, not, empty, present, count, all, or any".to_string(),
        }.into());
    }

    // `field-equals` narrows a count and means nothing anywhere else; a
    // slot/output predicate already compares with `equals`.
    if predicate.field_equals.is_some() && predicate.count.is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field-equals"),
            message: "field-equals requires count".to_string(),
        }
        .into());
    }

    if let Some(signal) = predicate.signal.as_deref() {
        validate_guard_ref_kind(signal, &format!("{field_path}.signal"), Kind::Signal)?;
        validate_guard_ref(
            validation.trait_ref,
            signal,
            &format!("{field_path}.signal"),
            validation.signal_ids,
            validation.allow_iteration,
        )?;
    }
    if let Some(condition) = predicate.condition.as_deref() {
        validate_guard_ref_kind(
            condition,
            &format!("{field_path}.condition"),
            Kind::Condition,
        )?;
        validate_guard_ref(
            validation.trait_ref,
            condition,
            &format!("{field_path}.condition"),
            validation.signal_ids,
            validation.allow_iteration,
        )?;
    }
    if (predicate.iteration.is_some() || predicate.iteration_at_least.is_some())
        && !validation.allow_iteration
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "iteration predicates are valid only inside loop guards".to_string(),
        }
        .into());
    }
    if let Some(slot_ref) = predicate.slot.as_deref() {
        validate_slot_predicate(
            validation.trait_ref,
            predicate,
            slot_ref,
            field_path,
            validation.slot_ids,
        )?;
    } else if let Some(output_ref) = predicate.output.as_deref() {
        validate_output_predicate(
            validation.trait_ref,
            predicate,
            output_ref,
            field_path,
            validation.allow_output,
        )?;
    } else if let Some(empty_ref) = predicate.empty.as_deref() {
        validate_list_slot_predicate(
            validation.trait_ref,
            empty_ref,
            field_path,
            "empty",
            validation.slot_ids,
            true,
        )?;
        validate_no_modifiers(predicate, field_path)?;
    } else if let Some(present_ref) = predicate.present.as_deref() {
        validate_present_predicate(
            validation.trait_ref,
            predicate,
            present_ref,
            field_path,
            validation.slot_ids,
        )?;
    } else if let Some(count_ref) = predicate.count.as_deref() {
        validate_list_slot_predicate(
            validation.trait_ref,
            count_ref,
            field_path,
            "count",
            validation.slot_ids,
            false,
        )?;
        validate_count_modifiers(predicate, field_path)?;
        validate_count_threshold(
            validation.trait_ref,
            predicate,
            field_path,
            validation.slot_ids,
        )?;
    } else if let Some(threshold) = predicate.elapsed_seconds_at_least.as_ref() {
        validate_elapsed_predicate(
            validation.trait_ref,
            threshold,
            &format!("{field_path}.elapsed-seconds-at-least"),
        )?;
        validate_no_modifiers(predicate, field_path)?;
    } else if predicate.field.is_some() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "field requires slot or output".to_string(),
        }
        .into());
    } else if let Some((name, _)) = first_modifier(predicate) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "comparison modifiers require slot, output, or count".to_string(),
        }
        .into());
    }
    if let Some(not) = predicate.not.as_deref() {
        validate_guard_expr_at_depth(validation, not, &format!("{field_path}.not"), not_depth + 1)?;
    }
    for (index, item) in predicate.all.iter().enumerate() {
        validate_guard_expr_at_depth(
            validation,
            item,
            &format!("{field_path}.all[{index}]"),
            not_depth,
        )?;
    }
    for (index, item) in predicate.any.iter().enumerate() {
        validate_guard_expr_at_depth(
            validation,
            item,
            &format!("{field_path}.any[{index}]"),
            not_depth,
        )?;
    }
    Ok(())
}

fn validate_guard_ref_kind(ref_text: &str, field_path: &str, expected: Kind) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid typed ref {ref_text:?}"),
    })?;
    if parsed.kind() != expected {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "guard predicate ref kind {:?} not allowed; expected {}",
                parsed.kind(),
                expected
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_slot_predicate(
    trait_ref: &Trait,
    predicate: &GuardPredicate,
    slot_ref: &str,
    field_path: &str,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    validate_value_predicate_modifiers(predicate, field_path)?;
    if predicate.field.is_some()
        && predicate.equals.is_none()
        && ordered_modifier(predicate).is_none()
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "slot field predicates require a comparison".to_string(),
        }
        .into());
    }
    let parsed = Reference::parse(slot_ref).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: format!("{field_path}.slot"),
        message: format!("invalid slot ref {slot_ref:?}"),
    })?;
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.slot"),
            message: "condition slot predicate must use a local slot:* ref".to_string(),
        }
        .into());
    }
    if !slot_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.slot"),
            message: format!("unresolved local slot ref {slot_ref:?}"),
        }
        .into());
    }
    let Some(slot) = trait_ref.slots.iter().find(|slot| slot.id == parsed.id()) else {
        return Ok(());
    };
    if let Some((name, value)) = ordered_modifier(predicate) {
        validate_numeric_comparison(
            trait_ref,
            slot.schema.as_ref().map(ToString::to_string).as_deref(),
            predicate.field.as_deref(),
            name,
            value,
            field_path,
        )?;
    }
    if let Some(equals) = predicate.equals.as_ref() {
        let Some(schema_ref) = slot.schema.as_ref().map(ToString::to_string) else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.slot"),
                message: "slot equality requires the slot to declare a schema".to_string(),
            }
            .into());
        };
        validate_literal_for_schema(
            trait_ref,
            &schema_ref,
            predicate.field.as_deref(),
            equals,
            field_path,
        )?;
    }
    Ok(())
}

fn validate_output_predicate(
    trait_ref: &Trait,
    predicate: &GuardPredicate,
    output_ref: &str,
    field_path: &str,
    allow_output: bool,
) -> crate::Result<()> {
    validate_value_predicate_modifiers(predicate, field_path)?;
    if !allow_output {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.output"),
            message: "output predicates are valid only in same-step output guards".to_string(),
        }
        .into());
    }
    if predicate.field.is_some()
        && predicate.equals.is_none()
        && ordered_modifier(predicate).is_none()
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "output field predicates require a comparison".to_string(),
        }
        .into());
    }
    let parsed =
        Reference::parse(output_ref).map_err(|_| crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.output"),
            message: format!("invalid output ref {output_ref:?}"),
        })?;
    if parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.output"),
            message: "condition output predicate must use a local ref".to_string(),
        }
        .into());
    }
    let schema_ref = match parsed.kind() {
        Kind::Slot => {
            let Some(slot) = trait_ref.slots.iter().find(|slot| slot.id == parsed.id()) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.output"),
                    message: format!("unresolved local slot output ref {output_ref:?}"),
                }
                .into());
            };
            slot.schema.as_ref().map(ToString::to_string)
        }
        Kind::Port => {
            let Some(port) = trait_ref.ports.iter().find(|port| {
                port.id == parsed.id()
                    && matches!(port.direction, crate::r#trait::PortDirection::Output)
            }) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.output"),
                    message: format!("unresolved local output port ref {output_ref:?}"),
                }
                .into());
            };
            Some(port.schema.clone())
        }
        Kind::Schema => {
            if !is_builtin_schema_ref(output_ref)
                && !trait_ref
                    .schemas
                    .iter()
                    .any(|schema| schema.id == parsed.id())
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.output"),
                    message: format!("unresolved local schema output ref {output_ref:?}"),
                }
                .into());
            }
            Some(output_ref.to_string())
        }
        other => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.output"),
                message: format!(
                    "condition output predicate kind {other:?} not allowed; expected slot, port, or schema"
                ),
            }
            .into());
        }
    };

    if let Some((name, value)) = ordered_modifier(predicate) {
        validate_numeric_comparison(
            trait_ref,
            schema_ref.as_deref(),
            predicate.field.as_deref(),
            name,
            value,
            field_path,
        )?;
    }

    if let Some(equals) = predicate.equals.as_ref() {
        let Some(schema_ref) = schema_ref.as_deref() else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.output"),
                message: "output equality requires a declared schema".to_string(),
            }
            .into());
        };
        validate_literal_for_schema(
            trait_ref,
            schema_ref,
            predicate.field.as_deref(),
            equals,
            field_path,
        )?;
    }
    Ok(())
}

/// Validate a `present` leaf: bare form is a local optional input `port:*`;
/// with `field`, a declared optional field of a local `port:*`/`slot:*`
/// object container. Shared by both forms so there is exactly one
/// subject/optionality resolution path for `present`.
fn validate_present_predicate(
    trait_ref: &Trait,
    predicate: &GuardPredicate,
    subject_ref: &str,
    field_path: &str,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    if trait_ref.schema_version.as_str() != "0.3" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.present"),
            message: "present requires a trait declaring schema-version \"0.3\"".to_string(),
        }
        .into());
    }
    if let Some((name, _)) = first_modifier(predicate) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "present does not accept comparison modifiers".to_string(),
        }
        .into());
    }
    let parsed =
        Reference::parse(subject_ref).map_err(|_| crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.present"),
            message: format!("invalid present ref {subject_ref:?}"),
        })?;
    if parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.present"),
            message: "present must use a local port:*/slot:* ref".to_string(),
        }
        .into());
    }
    match parsed.kind() {
        Kind::Port => {
            let Some(port) = trait_ref.ports.iter().find(|port| {
                port.id == parsed.id()
                    && matches!(port.direction, crate::r#trait::PortDirection::Input)
            }) else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.present"),
                    message: format!("unresolved local input port ref {subject_ref:?}"),
                }
                .into());
            };
            if !port.optional {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.present"),
                    message:
                        "present over a required port is always true; declare the port optional"
                            .to_string(),
                }
                .into());
            }
            match predicate.field.as_deref() {
                None => Ok(()),
                Some(field_name) => validate_present_field(
                    trait_ref,
                    Some(port.schema.as_str()),
                    field_name,
                    field_path,
                ),
            }
        }
        Kind::Slot => {
            let Some(field_name) = predicate.field.as_deref() else {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.present"),
                    message: "bare present over a slot is not supported; pair it with field over a declared optional field".to_string(),
                }
                .into());
            };
            if !slot_ids.contains(parsed.id()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.present"),
                    message: format!("unresolved local slot ref {subject_ref:?}"),
                }
                .into());
            }
            let schema_ref = trait_ref
                .slots
                .iter()
                .find(|slot| slot.id == parsed.id())
                .and_then(|slot| slot.schema.as_ref().map(ToString::to_string));
            validate_present_field(trait_ref, schema_ref.as_deref(), field_name, field_path)
        }
        other => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.present"),
            message: format!("present ref kind {other:?} not allowed; expected port or slot"),
        }
        .into()),
    }
}

/// `present`'s `field` half: the container must declare a local object
/// schema with the named field present and declared optional.
fn validate_present_field(
    trait_ref: &Trait,
    schema_ref: Option<&str>,
    field_name: &str,
    field_path: &str,
) -> crate::Result<()> {
    let Some(schema_ref) = schema_ref else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "present field requires the container to declare a schema".to_string(),
        }
        .into());
    };
    let field_schema =
        resolve_object_field_path_schema(trait_ref, schema_ref, field_name, field_path)?;
    if field_schema.required {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "present field must be a declared optional field".to_string(),
        }
        .into());
    }
    Ok(())
}

fn first_modifier(predicate: &GuardPredicate) -> Option<(&'static str, &Value)> {
    predicate
        .equals
        .as_ref()
        .map(|value| ("equals", value))
        .or_else(|| {
            predicate
                .less_than
                .as_ref()
                .map(|value| ("less-than", value))
        })
        .or_else(|| predicate.at_most.as_ref().map(|value| ("at-most", value)))
        .or_else(|| {
            predicate
                .greater_than
                .as_ref()
                .map(|value| ("greater-than", value))
        })
        .or_else(|| predicate.at_least.as_ref().map(|value| ("at-least", value)))
}

pub(crate) fn ordered_modifier(predicate: &GuardPredicate) -> Option<(&'static str, &Value)> {
    predicate
        .less_than
        .as_ref()
        .map(|value| ("less-than", value))
        .or_else(|| predicate.at_most.as_ref().map(|value| ("at-most", value)))
        .or_else(|| {
            predicate
                .greater_than
                .as_ref()
                .map(|value| ("greater-than", value))
        })
        .or_else(|| predicate.at_least.as_ref().map(|value| ("at-least", value)))
}

fn modifier_count(predicate: &GuardPredicate) -> usize {
    [
        predicate.equals.is_some(),
        predicate.less_than.is_some(),
        predicate.at_most.is_some(),
        predicate.greater_than.is_some(),
        predicate.at_least.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
}

fn validate_no_modifiers(predicate: &GuardPredicate, field_path: &str) -> crate::Result<()> {
    if let Some((name, _)) = first_modifier(predicate) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "this guard form does not accept comparison modifiers".to_string(),
        }
        .into());
    }
    if predicate.field.is_some() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.field"),
            message: "this guard form does not accept field".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_value_predicate_modifiers(
    predicate: &GuardPredicate,
    field_path: &str,
) -> crate::Result<()> {
    if modifier_count(predicate) > 1 {
        let name = second_modifier_name(predicate).expect("multiple modifiers are present");
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "slot/output predicates accept at most one comparison modifier".to_string(),
        }
        .into());
    }
    Ok(())
}

fn second_modifier_name(predicate: &GuardPredicate) -> Option<&'static str> {
    [
        ("equals", predicate.equals.is_some()),
        ("less-than", predicate.less_than.is_some()),
        ("at-most", predicate.at_most.is_some()),
        ("greater-than", predicate.greater_than.is_some()),
        ("at-least", predicate.at_least.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .nth(1)
}

fn validate_count_modifiers(predicate: &GuardPredicate, field_path: &str) -> crate::Result<()> {
    if predicate.less_than.is_some()
        || predicate.at_most.is_some()
        || predicate.greater_than.is_some()
    {
        let (name, _) = ordered_modifier(predicate).expect("ordered modifier is present");
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "count accepts only equals or at-least".to_string(),
        }
        .into());
    }
    let thresholds =
        usize::from(predicate.equals.is_some()) + usize::from(predicate.at_least.is_some());
    if thresholds > 1 {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.at-least"),
            message: "count requires exactly one of equals or at-least".to_string(),
        }
        .into());
    }
    if thresholds == 0 {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.count"),
            message: "count requires exactly one of equals or at-least".to_string(),
        }
        .into());
    }
    // `count` may narrow to elements matching one field value. Both halves are
    // required: a bare `field` would silently count everything, and a bare
    // `field-equals` has no field to test — either way the guard would report a
    // number nobody asked for.
    match (predicate.field.as_deref(), predicate.field_equals.as_ref()) {
        (Some(_), Some(_)) | (None, None) => {}
        (Some(_), None) => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: "count with field requires field-equals".to_string(),
            }
            .into());
        }
        (None, Some(_)) => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field-equals"),
                message: "count with field-equals requires field".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_count_threshold(
    trait_ref: &Trait,
    predicate: &GuardPredicate,
    field_path: &str,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let (name, threshold) = first_modifier(predicate).expect("count threshold is present");
    if threshold.as_u64().is_some() {
        return Ok(());
    }
    if trait_ref.schema_version.as_str() != "0.3" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: "count-to-count comparisons require a trait declaring schema-version \"0.3\""
                .to_string(),
        }
        .into());
    }
    let operand: CountOperand = serde_json::from_value(threshold.clone()).map_err(|error| {
        crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}"),
            message: format!(
                "count threshold must be a non-negative integer or a closed count operand: {error}"
            ),
        }
    })?;
    validate_list_slot_predicate(trait_ref, &operand.count, field_path, name, slot_ids, false)?;
    match (&operand.field, &operand.field_equals) {
        (Some(Some(_)), Some(_)) | (None, None) => Ok(()),
        (Some(Some(_)), None) => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}.field"),
            message: "count operand with field requires field-equals".to_string(),
        }
        .into()),
        (None, Some(_)) => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}.field-equals"),
            message: "count operand with field-equals requires field".to_string(),
        }
        .into()),
        (Some(None), _) => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.{name}.field"),
            message: "count operand field must be a string".to_string(),
        }
        .into()),
    }
}

/// Strict, shared count operand parser used after validation by dependency and
/// runtime consumers. Present members must have their declared JSON types.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct CountOperand {
    pub count: String,
    #[serde(default)]
    pub field: Option<Option<String>>,
    #[serde(default)]
    pub field_equals: Option<Option<Value>>,
}

fn validate_list_slot_predicate(
    trait_ref: &Trait,
    slot_ref: &str,
    field_path: &str,
    field: &str,
    slot_ids: &BTreeSet<&str>,
    allow_text: bool,
) -> crate::Result<()> {
    let path = format!("{field_path}.{field}");
    let parsed = Reference::parse(slot_ref).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: path.clone(),
        message: format!("invalid slot ref {slot_ref:?}"),
    })?;
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: path,
            message: format!("{field} predicate must use a local slot:* ref"),
        }
        .into());
    }
    if !slot_ids.contains(parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: path,
            message: format!("unresolved local slot ref {slot_ref:?}"),
        }
        .into());
    }
    let schema = trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == parsed.id())
        .and_then(|slot| slot.schema.as_ref());
    let is_list =
        schema.is_some_and(|schema| matches!(schema, crate::schema::form::Schema::List(_)));
    // `empty` also covers zero-length text: a schema-less slot and an
    // explicit `schema:text` are both string-valued at runtime.
    let is_text = schema.is_none()
        || schema.is_some_and(|schema| {
            matches!(
                schema,
                crate::schema::form::Schema::Builtin(crate::schema::form::Builtin::Text)
            )
        });
    if !is_list && !(allow_text && is_text) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: path,
            message: if allow_text {
                format!("{field} predicate requires a list- or text-schema slot")
            } else {
                format!("{field} predicate requires a list-schema slot")
            },
        }
        .into());
    }
    Ok(())
}

fn validate_numeric_comparison(
    trait_ref: &Trait,
    schema_ref: Option<&str>,
    field: Option<&str>,
    modifier: &str,
    threshold: &Value,
    field_path: &str,
) -> crate::Result<()> {
    let lhs_schema = match field {
        Some(field) => schema_ref
            .and_then(|schema_ref| numeric_object_field_schema(trait_ref, schema_ref, field)),
        None => schema_ref.map(str::to_string),
    };
    if !lhs_schema
        .as_deref()
        .is_some_and(|schema| numeric_schema(trait_ref, schema, &mut BTreeSet::new()))
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: if field.is_some() {
                format!("{field_path}.field")
            } else {
                format!("{field_path}.{modifier}")
            },
            message: "ordered comparison requires a numeric slot/output or numeric top-level object field".to_string(),
        }
        .into());
    }
    if threshold.is_number() {
        return Ok(());
    }
    validate_numeric_threshold_ref(trait_ref, threshold, &format!("{field_path}.{modifier}"))
}

/// Shared RHS-threshold-ref validator for both ordered numeric comparisons
/// and the `elapsed-seconds-at-least` guard: parses `{ ref = "slot:..." }`/
/// `{ ref = "port:..." }`, confirms it is a local slot or input port, and
/// confirms that ref resolves to a numeric schema. `base_field_path` is the
/// full path up to (and not including) the trailing `.ref` this function
/// appends on ref-specific errors.
fn validate_numeric_threshold_ref(
    trait_ref: &Trait,
    threshold: &Value,
    base_field_path: &str,
) -> crate::Result<()> {
    let Some(ref_text) = numeric_comparison_ref(threshold) else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: base_field_path.to_string(),
            message:
                "threshold must be a JSON number or { ref = \"slot:...\" }/{ ref = \"port:...\" }"
                    .to_string(),
        }
        .into());
    };
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: format!("{base_field_path}.ref"),
        message: format!("invalid numeric comparison ref {ref_text:?}"),
    })?;
    if parsed.is_qualified() || !matches!(parsed.kind(), Kind::Slot | Kind::Port) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base_field_path}.ref"),
            message: "RHS ref must be a local slot or input port".to_string(),
        }
        .into());
    }
    let rhs_schema = match parsed.kind() {
        Kind::Slot => trait_ref
            .slots
            .iter()
            .find(|slot| slot.id == parsed.id())
            .and_then(|slot| slot.schema.as_ref())
            .map(ToString::to_string),
        Kind::Port => trait_ref
            .ports
            .iter()
            .find(|port| {
                port.id == parsed.id()
                    && matches!(port.direction, crate::r#trait::PortDirection::Input)
            })
            .map(|port| port.schema.clone()),
        _ => None,
    };
    if !rhs_schema
        .as_deref()
        .is_some_and(|schema| numeric_schema(trait_ref, schema, &mut BTreeSet::new()))
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base_field_path}.ref"),
            message: "RHS ref must resolve to a numeric schema".to_string(),
        }
        .into());
    }
    Ok(())
}

/// Validate the threshold of a closed `elapsed-seconds-at-least` guard form.
/// The LHS is always the runtime-supplied cumulative elapsed-seconds
/// evidence (never a slot/output/schema), so only the RHS threshold needs
/// checking: a non-negative JSON number, or `{ ref = "slot:.../port:..." }`
/// resolving to a numeric local slot or input port.
fn validate_elapsed_predicate(
    trait_ref: &Trait,
    threshold: &Value,
    field_path: &str,
) -> crate::Result<()> {
    if let Some(number) = threshold.as_f64() {
        if number.is_sign_negative() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: field_path.to_string(),
                message: "elapsed-seconds-at-least threshold must be non-negative".to_string(),
            }
            .into());
        }
        return Ok(());
    }
    if numeric_comparison_ref(threshold).is_none() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "elapsed-seconds-at-least threshold must be a non-negative JSON number or { ref = \"slot:...\" }/{ ref = \"port:...\" }".to_string(),
        }
        .into());
    }
    // A ref-backed threshold's runtime value is checked for non-negativity
    // at guard-evaluation time (see `guards.rs`), since only the schema, not
    // the runtime value, is knowable here.
    validate_numeric_threshold_ref(trait_ref, threshold, field_path)
}

fn numeric_object_field_schema(trait_ref: &Trait, schema_ref: &str, field: &str) -> Option<String> {
    resolve_object_field_path_schema(trait_ref, schema_ref, field, "")
        .ok()
        .map(|field_schema| field_schema.schema.clone())
}

/// Walk a dot-joined field path against a chain of locally declared inline
/// object schemas, one hop per segment: each intermediate segment's field
/// must itself declare a local object schema with inline fields, and the
/// final segment's field schema is returned.
///
/// Refuses an empty path segment, and refuses outright if any container
/// schema on the path declares a literal field name containing `'.'`
/// (unreachable for slug-validated inline schemas —
/// [`crate::shared::validate_slug_shape`] admits no dot — but defended here
/// for any schema source that bypasses slug validation), naming the schema
/// and the offending field.
pub(crate) fn resolve_object_field_path_schema<'a>(
    trait_ref: &'a Trait,
    schema_ref: &str,
    path: &str,
    field_path: &str,
) -> crate::Result<&'a crate::r#trait::schema::SchemaField> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut current_schema_ref = schema_ref.to_string();
    let mut resolved_field: Option<&'a crate::r#trait::schema::SchemaField> = None;
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: format!("field path {path:?} has an empty segment"),
            }
            .into());
        }
        let parsed = Reference::parse(&current_schema_ref).map_err(|_| {
            crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: format!("invalid schema ref {current_schema_ref:?}"),
            }
        })?;
        if parsed.kind() != Kind::Schema || parsed.is_qualified() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: "field path requires a local object schema".to_string(),
            }
            .into());
        }
        let schema = trait_ref
            .schemas
            .iter()
            .find(|schema| schema.id == parsed.id())
            .ok_or_else(|| crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: format!("schema {current_schema_ref:?} is not declared"),
            })?;
        let fields =
            schema
                .fields
                .as_ref()
                .ok_or_else(|| crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.field"),
                    message: "field path requires inline object schema fields".to_string(),
                })?;
        if let Some(dotted) = fields.keys().find(|name| name.contains('.')) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.field"),
                message: format!(
                    "schema {:?} declares field {dotted:?} containing '.', which is ambiguous in a dotted field path",
                    schema.id
                ),
            }
            .into());
        }
        let field_schema =
            fields
                .get(*segment)
                .ok_or_else(|| crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.field"),
                    message: format!("unknown schema field {segment:?}"),
                })?;
        resolved_field = Some(field_schema);
        if index + 1 < segments.len() {
            current_schema_ref = field_schema.schema.clone();
        }
    }
    Ok(resolved_field.expect("path always has at least one segment"))
}

/// Return the sole `ref` member of a tagged numeric comparison RHS.
pub(crate) fn numeric_comparison_ref(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object.get("ref")?.as_str()
}

/// Returns the RHS list slot of a previously validated closed count operand.
pub(crate) fn parse_count_operand(value: &Value) -> Option<CountOperand> {
    serde_json::from_value(value.clone()).ok()
}

fn numeric_schema(trait_ref: &Trait, schema_ref: &str, seen: &mut BTreeSet<String>) -> bool {
    if matches!(schema_ref, "schema:number" | "schema:integer") {
        return true;
    }
    let Ok(parsed) = Reference::parse(schema_ref) else {
        return false;
    };
    if parsed.kind() != Kind::Schema
        || parsed.is_qualified()
        || !seen.insert(parsed.id().to_string())
    {
        return false;
    }
    trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
        .and_then(|schema| schema.schema.as_deref())
        .is_some_and(|base| numeric_schema(trait_ref, base, seen))
}

fn is_builtin_schema_ref(schema_ref: &str) -> bool {
    matches!(
        schema_ref,
        "schema:text" | "schema:boolean" | "schema:number" | "schema:integer" | "schema:any"
    )
}

fn validate_literal_for_schema(
    trait_ref: &Trait,
    schema_ref: &str,
    field: Option<&str>,
    literal: &Value,
    field_path: &str,
) -> crate::Result<()> {
    if schema_ref == "schema:any" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "equality over schema:any is not allowed".to_string(),
        }
        .into());
    }
    if let Some(field_name) = field {
        let field_schema =
            resolve_object_field_path_schema(trait_ref, schema_ref, field_name, field_path)?;
        return validate_literal_for_schema_field(trait_ref, field_schema, literal, field_path);
    }

    match schema_ref {
        "schema:text" if literal.is_string() => Ok(()),
        "schema:boolean" if literal.is_boolean() => Ok(()),
        "schema:number" if literal.is_number() => Ok(()),
        "schema:integer" if literal.is_i64() || literal.is_u64() => Ok(()),
        "schema:text" | "schema:boolean" | "schema:number" | "schema:integer" => {
            Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.equals"),
                message: format!("literal does not match {schema_ref}"),
            }
            .into())
        }
        ref_text if ref_text.starts_with('[') => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "whole-list equality predicates are not supported".to_string(),
        }
        .into()),
        ref_text if ref_text.starts_with('(') => Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "whole-union equality predicates are not supported".to_string(),
        }
        .into()),
        _ => {
            if try_validate_scalar_schema_literal(trait_ref, schema_ref, literal, field_path)? {
                return Ok(());
            }
            if literal.is_object() {
                return validate_object_literal_for_schema(
                    trait_ref, schema_ref, literal, field_path,
                );
            }
            Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.equals"),
                message: "whole-slot equality for local object schemas requires an object literal"
                    .to_string(),
            }
            .into())
        }
    }
}

fn validate_object_literal_for_schema(
    trait_ref: &Trait,
    schema_ref: &str,
    literal: &Value,
    field_path: &str,
) -> crate::Result<()> {
    let parsed =
        Reference::parse(schema_ref).map_err(|_| crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.slot"),
            message: format!("invalid schema ref {schema_ref:?}"),
        })?;
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "whole-object equality requires a local object schema".to_string(),
        }
        .into());
    }
    let Some(schema) = trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
    else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: format!("schema {schema_ref:?} is not declared"),
        }
        .into());
    };
    let Some(fields) = schema.fields.as_ref() else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "whole-object equality requires inline object schema fields".to_string(),
        }
        .into());
    };
    let Some(object) = literal.as_object() else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.equals"),
            message: "whole-object equality requires an object literal".to_string(),
        }
        .into());
    };
    for (key, value) in object {
        let Some(field_schema) = fields.get(key) else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.equals.{key}"),
                message: format!("unknown schema field {key:?}"),
            }
            .into());
        };
        validate_literal_for_schema_field(trait_ref, field_schema, value, field_path)?;
    }
    Ok(())
}

fn validate_literal_for_schema_field(
    trait_ref: &Trait,
    field_schema: &crate::r#trait::schema::SchemaField,
    literal: &Value,
    field_path: &str,
) -> crate::Result<()> {
    validate_literal_for_schema(trait_ref, &field_schema.schema, None, literal, field_path)?;
    if let Some(allowed) = field_schema.allowed.as_ref() {
        crate::r#trait::schema::validate_allowed_literal(
            allowed,
            literal,
            &format!("{field_path}.equals"),
        )?;
    }
    Ok(())
}

fn try_validate_scalar_schema_literal(
    trait_ref: &Trait,
    schema_ref: &str,
    literal: &Value,
    field_path: &str,
) -> crate::Result<bool> {
    let Ok(parsed) = Reference::parse(schema_ref) else {
        return Ok(false);
    };
    if parsed.kind() != Kind::Schema || parsed.is_qualified() {
        return Ok(false);
    }
    let Some(schema) = trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == parsed.id())
    else {
        return Ok(false);
    };
    let Some(base_schema) = schema.schema.as_deref() else {
        return Ok(false);
    };
    validate_literal_for_schema(trait_ref, base_schema, None, literal, field_path)?;
    if let Some(allowed) = schema.allowed.as_ref() {
        crate::r#trait::schema::validate_allowed_literal(
            allowed,
            literal,
            &format!("{field_path}.equals"),
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod present_validation_tests {
    use crate::encoding::{Encoding, decode_trait};

    const HEADER_03: &str = r#"
id = "present-validate"
schema-version = "0.3"
version = "0.1.0"
name = "Present validate"
description = "Present validation fixture."
"#;

    const HEADER_02: &str = r#"
id = "present-validate"
schema-version = "0.2"
version = "0.1.0"
name = "Present validate"
description = "Present validation fixture."
"#;

    #[test]
    fn accepts_bare_present_over_optional_input_port_in_0_3() {
        let text = format!(
            "{HEADER_03}\n[[port]]\nid = \"cap\"\ndirection = \"input\"\nschema = \"schema:number\"\noptional = true\ndescription = \"Cap.\"\n\n[condition.has-cap]\npresent = \"port:cap\"\n"
        );
        decode_trait(Encoding::Toml, &text).expect("bare present over optional port accepted");
    }

    #[test]
    fn rejects_present_in_0_2_document() {
        let text = format!(
            "{HEADER_02}\n[[port]]\nid = \"cap\"\ndirection = \"input\"\nschema = \"schema:number\"\noptional = true\ndescription = \"Cap.\"\n\n[condition.has-cap]\npresent = \"port:cap\"\n"
        );
        let err = decode_trait(Encoding::Toml, &text).expect_err("present rejected under 0.2");
        assert!(err.to_string().contains("present"));
    }

    #[test]
    fn rejects_present_over_required_port() {
        let text = format!(
            "{HEADER_03}\n[[port]]\nid = \"cap\"\ndirection = \"input\"\nschema = \"schema:number\"\ndescription = \"Cap.\"\n\n[condition.has-cap]\npresent = \"port:cap\"\n"
        );
        let err =
            decode_trait(Encoding::Toml, &text).expect_err("present over required port rejected");
        assert!(err.to_string().contains("present"));
    }

    #[test]
    fn rejects_present_with_comparison_modifier() {
        let text = format!(
            "{HEADER_03}\n[[port]]\nid = \"cap\"\ndirection = \"input\"\nschema = \"schema:number\"\noptional = true\ndescription = \"Cap.\"\n\n[condition.has-cap]\npresent = \"port:cap\"\nequals = 1\n"
        );
        let err = decode_trait(Encoding::Toml, &text).expect_err("present with modifier rejected");
        assert!(err.to_string().contains("equals"));
    }

    #[test]
    fn rejects_bare_present_over_a_slot() {
        let text = format!(
            "{HEADER_03}\n[[slot]]\nid = \"scratch\"\nschema = \"schema:number\"\n\n[condition.has-scratch]\npresent = \"slot:scratch\"\n"
        );
        let err =
            decode_trait(Encoding::Toml, &text).expect_err("bare present over a slot rejected");
        assert!(err.to_string().contains("present"));
    }

    #[test]
    fn accepts_present_field_over_optional_field_of_local_object_schema_slot() {
        let text = format!(
            "{HEADER_03}\n[[schema]]\nid = \"cap-report\"\n\n[schema.fields.cost-report]\nschema = \"schema:text\"\nrequired = false\n\n[[slot]]\nid = \"cap\"\nschema = \"schema:cap-report\"\n\n[condition.has-report]\npresent = \"slot:cap\"\nfield = \"cost-report\"\n"
        );
        decode_trait(Encoding::Toml, &text).expect("present field over optional field accepted");
    }

    #[test]
    fn rejects_present_field_that_is_declared_required() {
        let text = format!(
            "{HEADER_03}\n[[schema]]\nid = \"cap-report\"\n\n[schema.fields.cost-report]\nschema = \"schema:text\"\nrequired = true\n\n[[slot]]\nid = \"cap\"\nschema = \"schema:cap-report\"\n\n[condition.has-report]\npresent = \"slot:cap\"\nfield = \"cost-report\"\n"
        );
        let err =
            decode_trait(Encoding::Toml, &text).expect_err("present over required field rejected");
        assert!(err.to_string().contains("field"));
    }

    #[test]
    fn guard_outcome_boolean_and_kleene_combinators() {
        use super::GuardOutcome;

        assert!(GuardOutcome::Matched.routes_true());
        assert!(!GuardOutcome::NotMatched.routes_true());
        assert!(!GuardOutcome::Unmeasurable.routes_true());

        assert_eq!(GuardOutcome::Matched.negate(), GuardOutcome::NotMatched);
        assert_eq!(GuardOutcome::NotMatched.negate(), GuardOutcome::Matched);
        assert_eq!(
            GuardOutcome::Unmeasurable.negate(),
            GuardOutcome::Unmeasurable
        );

        assert_eq!(
            GuardOutcome::Matched.and(GuardOutcome::Unmeasurable),
            GuardOutcome::Unmeasurable
        );
        assert_eq!(
            GuardOutcome::NotMatched.and(GuardOutcome::Unmeasurable),
            GuardOutcome::NotMatched
        );
        assert_eq!(
            GuardOutcome::Matched.or(GuardOutcome::Unmeasurable),
            GuardOutcome::Matched
        );
        assert_eq!(
            GuardOutcome::NotMatched.or(GuardOutcome::Unmeasurable),
            GuardOutcome::Unmeasurable
        );
    }
}

/// Proves task 0085's build-time Done-when clauses: a dotted path validates
/// through `condition.equals`, an unknown segment is refused by name, and a
/// literal field name containing `'.'` is refused by name.
#[cfg(test)]
mod nested_field_path_validation_tests {
    use crate::encoding::{Encoding, decode_trait};

    const HEADER: &str = r#"
id = "nested-field-path-validate"
schema-version = "0.3"
version = "0.1.0"
name = "Nested field path validate"
description = "Nested field path validation fixture."
"#;

    const NESTED_SCHEMAS: &str = r#"
[[schema]]
id = "decision"

[schema.fields.behavior]
schema = "schema:text"
required = false

[[schema]]
id = "hook-specific-output"

[schema.fields.decision]
schema = "schema:decision"
required = false

[[slot]]
id = "hook-output"
schema = "schema:hook-specific-output"
"#;

    #[test]
    fn accepts_a_dotted_path_over_a_declared_nested_field() {
        let text = format!(
            "{HEADER}\n{NESTED_SCHEMAS}\n[condition.approved]\nslot = \"slot:hook-output\"\nfield = \"decision.behavior\"\nequals = \"approve\"\n"
        );
        decode_trait(Encoding::Toml, &text)
            .expect("dotted field path over a nested schema accepted");
    }

    #[test]
    fn rejects_an_unknown_segment_naming_it() {
        let text = format!(
            "{HEADER}\n{NESTED_SCHEMAS}\n[condition.approved]\nslot = \"slot:hook-output\"\nfield = \"decision.unknown-field\"\nequals = \"approve\"\n"
        );
        let err = decode_trait(Encoding::Toml, &text).expect_err("unknown nested segment rejected");
        assert!(
            err.to_string().contains("unknown-field"),
            "error must name the offending segment: {err}"
        );
    }

    #[test]
    fn rejects_an_intermediate_segment_that_is_not_an_object_schema() {
        let text = format!(
            "{HEADER}\n{NESTED_SCHEMAS}\n[condition.approved]\nslot = \"slot:hook-output\"\nfield = \"decision.behavior.bogus\"\nequals = \"approve\"\n"
        );
        decode_trait(Encoding::Toml, &text)
            .expect_err("walking past a scalar leaf field must be rejected");
    }

    /// Inline object-schema field ids are slug-validated at decode time
    /// (`validate_slug_shape` admits no `'.'`), so a dotted literal field
    /// name can never reach `resolve_object_field_path_schema` through
    /// normal decoding — this defends any schema source that bypasses that
    /// validation, so the test bypasses it too: decode a valid fixture, then
    /// mutate the decoded schema's field map directly.
    #[test]
    fn rejects_a_literal_field_name_containing_a_dot_naming_the_schema_and_field() {
        let text = format!(
            "{HEADER}\n[[schema]]\nid = \"malformed\"\n\n[schema.fields.placeholder]\nschema = \"schema:text\"\nrequired = false\n\n[[slot]]\nid = \"scratch\"\nschema = \"schema:malformed\"\n"
        );
        let mut trait_ref = decode_trait(Encoding::Toml, &text).expect("fixture decodes");
        let schema = trait_ref
            .schemas
            .iter_mut()
            .find(|schema| schema.id == "malformed")
            .expect("malformed schema declared");
        let fields = schema.fields.as_mut().expect("inline fields declared");
        let placeholder = fields
            .remove("placeholder")
            .expect("placeholder field declared");
        fields.insert("a.b".to_string(), placeholder);

        let err = super::resolve_object_field_path_schema(
            &trait_ref,
            "schema:malformed",
            "a.b",
            "condition.approved",
        )
        .expect_err("a literal field name containing a dot must be refused");
        let message = err.to_string();
        assert!(
            message.contains("malformed") && message.contains("a.b"),
            "error must name the schema and the offending field: {message}"
        );
    }

    #[test]
    fn present_field_validates_through_the_same_dotted_path() {
        let text = format!(
            "{HEADER}\n{NESTED_SCHEMAS}\n[condition.has-behavior]\npresent = \"slot:hook-output\"\nfield = \"decision.behavior\"\n"
        );
        decode_trait(Encoding::Toml, &text).expect("present over a dotted field path accepted");
    }

    #[test]
    fn one_level_field_ref_still_emits_the_exact_bytes_it_always_has() {
        let text = format!(
            "{HEADER}\n[[schema]]\nid = \"cap-report\"\n\n[schema.fields.cost-microusd]\nschema = \"schema:integer\"\nrequired = false\n\n[[slot]]\nid = \"cap\"\nschema = \"schema:cap-report\"\n\n[condition.under-cap]\nslot = \"slot:cap\"\nfield = \"cost-microusd\"\nat-most = 5\n"
        );
        let trait_ref = decode_trait(Encoding::Toml, &text).expect("one-level field ref accepted");
        let canonical = crate::digest::canonical_json(&trait_ref).expect("trait canonicalizes");
        assert!(
            canonical.contains("\"field\":\"cost-microusd\""),
            "one-level field must still serialize as a bare name, not a path: {canonical}"
        );
    }
}
