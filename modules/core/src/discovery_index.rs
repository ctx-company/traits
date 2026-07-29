//! Discovery index: lightweight metadata for resolve without loading every body.
//!
//! Index records are Tier 0 metadata — small enough for quick scanning. They
//! contain ID, name, summary, activation scope, ports, signals, relation
//! summaries, composition conflicts, digests, and estimated tokens. Stale
//! markers are computed from source and canonical digests so callers can
//! decide whether to rebuild or reuse a cached index.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::manifest::PackageStatus;
use crate::r#trait::{Trait, TrustVerdict};

// ---------------------------------------------------------------------------
// Relation summary
// ---------------------------------------------------------------------------

/// One relation summary entry preserving kind, target, reason, and when refs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RelationSummary {
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_refs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Declaration summary
// ---------------------------------------------------------------------------

/// Summary of activation rules for an index record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ActivationSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_levels: Vec<String>,

    /// True when at least one activation rule declares no mode predicate at
    /// all. Such a rule can match regardless of the requested mode, so a
    /// trait-wide union of constrained rules' modes must never be used to
    /// exclude this record on the mode facet — only the full scorer, which
    /// evaluates each rule independently, can decide.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mode_unconstrained: bool,
    /// Same semantics as `mode_unconstrained`, for the language facet.
    #[serde(default, skip_serializing_if = "is_false")]
    pub language_unconstrained: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------
// Index record
// ---------------------------------------------------------------------------

/// A Tier 0 discovery index record for one trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct IndexedTrait {
    pub trait_id: String,
    pub name: String,
    pub summary: String,
    pub version: String,
    pub status: String,
    pub trust: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<Digest>,

    pub estimated_tokens: u64,
    pub stale: bool,

    /// Deterministic stale-reason codes, e.g. `"source-digest-mismatch"`,
    /// `"canonical-digest-mismatch"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_reasons: Vec<String>,

    pub activation: ActivationSummary,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signal_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composition_conflicts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relation_summaries: Vec<RelationSummary>,
}

/// A collection of discovery index records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Index {
    #[serde(default, rename = "record", skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<IndexedTrait>,
}

// ---------------------------------------------------------------------------
// Discovery index filter
// ---------------------------------------------------------------------------

/// Filter criteria for pre-ranking candidate selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Filter {
    /// Only include records with one of these lifecycle statuses.
    /// Empty means all statuses pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,

    /// Only include records with one of these trust levels.
    /// Empty means all trust levels pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusts: Vec<String>,

    /// Only include records matching at least one of these activation modes.
    /// Empty means no mode filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,

    /// Only include records matching at least one of these activation languages.
    /// Empty means no language filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,

    /// Whether to exclude stale records. Default false.
    #[serde(default)]
    pub exclude_stale: bool,

    /// Optional trait ID filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
}

/// Filter index records by the given criteria.
///
/// Returns a new vector of references to records that pass all filter
/// criteria. An empty filter returns all records.
pub fn filter_index_records<'a>(
    records: &'a [IndexedTrait],
    filter: &Filter,
) -> Vec<&'a IndexedTrait> {
    records
        .iter()
        .filter(|r| record_passes_filter(r, filter))
        .collect()
}

fn record_passes_filter(r: &IndexedTrait, filter: &Filter) -> bool {
    if let Some(id) = &filter.trait_id
        && r.trait_id != *id
    {
        return false;
    }

    if !filter.statuses.is_empty() && !filter.statuses.contains(&r.status) {
        return false;
    }

    if !filter.trusts.is_empty() && !filter.trusts.contains(&r.trust) {
        return false;
    }

    if filter.exclude_stale && r.stale {
        return false;
    }

    // A record is unconstrained on mode when it has no declared mode
    // predicate anywhere, OR when at least one of its OR'd activation rules
    // declares no mode predicate of its own (that rule can match regardless
    // of the requested mode, mirroring `apply_positive_group`'s per-rule
    // semantics: an empty predicate group never fails a match). Only exclude
    // a record where every rule DOES declare a mode set and none intersects
    // the requested fact.
    if !filter.modes.is_empty()
        && !r.activation.mode_unconstrained
        && !r.activation.modes.is_empty()
    {
        let has_mode = r
            .activation
            .modes
            .iter()
            .any(|mode| contains_ignore_ascii_case(&filter.modes, mode));
        if !has_mode {
            return false;
        }
    }

    // Same unconstrained-when-absent semantics for language.
    if !filter.languages.is_empty()
        && !r.activation.language_unconstrained
        && !r.activation.languages.is_empty()
    {
        let has_lang = r
            .activation
            .languages
            .iter()
            .any(|language| contains_ignore_ascii_case(&filter.languages, language));
        if !has_lang {
            return false;
        }
    }

    true
}

fn contains_ignore_ascii_case(values: &[String], value: &str) -> bool {
    values
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(value))
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Estimated token count using a simple character-based approximation.
///
/// Uses approximately 4 characters per token. This is an estimate, not exact
/// billing data.
pub fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(4)
}

// ---------------------------------------------------------------------------
// Index building
// ---------------------------------------------------------------------------

/// Build a discovery index record from a trusted trait and optional digests.
///
/// `status`/`trust` are resolved by the caller from the package manifest and
/// machine trust store respectively — the canonical trait document carries
/// neither field.
pub fn build_index_record(
    t: &Trait,
    status: &PackageStatus,
    trust: &TrustVerdict,
    source_digest: Option<&str>,
    canonical_digest: Option<&str>,
    stored_source_digest: Option<&str>,
    stored_canonical_digest: Option<&str>,
) -> IndexedTrait {
    let activation_summary = build_activation_summary(t);

    let port_refs: Vec<String> = t.ports.iter().map(|p| p.id.clone()).collect();
    let signal_refs: Vec<String> = t.signals.iter().map(|s| s.id.clone()).collect();

    let composition_conflicts: Vec<String> = t
        .composition
        .as_ref()
        .map(|c| c.conflict.iter().cloned().collect())
        .unwrap_or_default();

    let relation_summaries = build_relation_summaries(t);

    let estimated_tokens = estimate_model_visible_text(t);

    let stale =
        stored_source_digest != source_digest || stored_canonical_digest != canonical_digest;

    let mut stale_reasons = Vec::new();
    if stored_source_digest != source_digest {
        stale_reasons.push("source-digest-mismatch".to_string());
    }
    if stored_canonical_digest != canonical_digest {
        stale_reasons.push("canonical-digest-mismatch".to_string());
    }
    stale_reasons.sort();

    IndexedTrait {
        trait_id: t.id.as_str().to_string(),
        name: t.name.as_str().to_string(),
        summary: t.summary.as_str().to_string(),
        version: t.version.as_str().to_string(),
        status: status_str(status),
        trust: trust_str(trust),
        source_digest: source_digest.map(Digest::from_unvalidated),
        canonical_digest: canonical_digest.map(Digest::from_unvalidated),
        estimated_tokens,
        stale,
        stale_reasons,
        activation: activation_summary,
        port_refs,
        signal_refs,
        composition_conflicts,
        relation_summaries,
    }
}

/// Build a discovery index from loaded traits with their digests and
/// caller-resolved `(package status, trust verdict)`.
pub fn build_index(
    traits_with_digests: &[(&str, &str, &Trait, PackageStatus, TrustVerdict)],
) -> Index {
    let mut records: Vec<IndexedTrait> = traits_with_digests
        .iter()
        .map(|(src, canon, t, status, trust)| {
            build_index_record(
                t,
                status,
                trust,
                Some(src),
                Some(canon),
                Some(src),
                Some(canon),
            )
        })
        .collect();

    records.sort_by(|a, b| a.trait_id.cmp(&b.trait_id));

    Index { records }
}

/// One entry for staleness-checked index building.
pub struct Staleness<'a> {
    pub source_digest: &'a str,
    pub canonical_digest: &'a str,
    pub stored_source_digest: Option<&'a str>,
    pub stored_canonical_digest: Option<&'a str>,
    pub trait_ref: &'a Trait,
    pub status: PackageStatus,
    pub trust: TrustVerdict,
}

/// Build a discovery index with stale checking against stored digests.
pub fn build_index_with_staleness(entries: &[Staleness]) -> Index {
    let mut records: Vec<IndexedTrait> = entries
        .iter()
        .map(|e| {
            build_index_record(
                e.trait_ref,
                &e.status,
                &e.trust,
                Some(e.source_digest),
                Some(e.canonical_digest),
                e.stored_source_digest,
                e.stored_canonical_digest,
            )
        })
        .collect();

    records.sort_by(|a, b| a.trait_id.cmp(&b.trait_id));

    Index { records }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_relation_summaries(t: &Trait) -> Vec<RelationSummary> {
    let mut summaries = Vec::new();

    if let Some(relations) = &t.relations {
        for req in &relations.requires {
            summaries.push(RelationSummary {
                kind: "requires".to_string(),
                target: req.target.to_string(),
                reason: req.reason.clone(),
                when_refs: req.when.iter().map(|s| s.to_string()).collect(),
            });
        }
        for sug in &relations.suggests {
            summaries.push(RelationSummary {
                kind: "suggests".to_string(),
                target: sug.target.to_string(),
                reason: sug.reason.clone(),
                when_refs: sug.when.iter().map(|s| s.to_string()).collect(),
            });
        }
        for conf in &relations.conflicts {
            summaries.push(RelationSummary {
                kind: "conflicts".to_string(),
                target: String::new(),
                reason: conf.reason.clone(),
                when_refs: conf.when.iter().map(|s| s.to_string()).collect(),
            });
        }
    }

    summaries.sort_by(|a, b| (&a.kind, &a.target, &a.reason).cmp(&(&b.kind, &b.target, &b.reason)));
    summaries
}

fn build_activation_summary(t: &Trait) -> ActivationSummary {
    let act = match &t.activation {
        Some(a) => a,
        None => return ActivationSummary::default(),
    };

    let mut rule_ids = Vec::new();
    let mut modes = std::collections::BTreeSet::new();
    let mut languages = std::collections::BTreeSet::new();
    let mut load_levels = std::collections::BTreeSet::new();
    let mut mode_unconstrained = false;
    let mut language_unconstrained = false;

    for rule in &act.rules {
        rule_ids.push(rule.id.clone());
        if rule.mode.is_empty() {
            mode_unconstrained = true;
        }
        for mode in rule.mode.iter() {
            modes.insert(mode.clone());
        }
        if rule.language.is_empty() {
            language_unconstrained = true;
        }
        for lang in rule.language.iter() {
            languages.insert(lang.clone());
        }
        if let Some(ll) = &rule.load_level {
            load_levels.insert(ll.clone());
        }
    }

    rule_ids.sort();

    ActivationSummary {
        manual: if act.manual { Some(true) } else { None },
        priority: act.priority,
        rule_ids,
        modes: modes.into_iter().collect(),
        languages: languages.into_iter().collect(),
        load_levels: load_levels.into_iter().collect(),
        mode_unconstrained,
        language_unconstrained,
    }
}

fn estimate_model_visible_text(t: &Trait) -> u64 {
    let mut text = String::new();
    text.push_str(t.name.as_str());
    text.push_str(t.summary.as_str());
    if let Some(intent) = &t.intent {
        for slug in intent.require.iter() {
            text.push_str(slug.as_str());
        }
        for slug in intent.focus.iter() {
            text.push_str(slug.as_str());
        }
    }
    if let Some(proc) = &t.procedure {
        text.push_str(&proc.description);
        for seq in &proc.sequence {
            text.push_str(&seq.title);
            text.push_str(&seq.prompt);
        }
    }
    for scenario in &t.scenarios {
        if let Some(input) = &scenario.input {
            text.push_str(input);
        }
        if let Some(output) = &scenario.output {
            text.push_str(output);
        }
    }
    estimate_tokens(&text)
}

fn status_str(status: &PackageStatus) -> String {
    status.as_str().to_string()
}

fn trust_str(trust: &TrustVerdict) -> String {
    trust.display_name().to_string()
}
