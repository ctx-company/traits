//! Lexical search over trait inventory.
//!
//! Builds search documents from trusted trait models and ranks them against
//! a query string. Ranking uses tier-based dominance: exact ID/name matches
//! always rank above substring/keyword/resource/profile/body matches.
//! Search is discovery only — it does not imply activation, execution, or
//! provider/model calls.

use serde::{Deserialize, Serialize};

use crate::r#trait::Trait;

// ---------------------------------------------------------------------------
// Rank tier
// ---------------------------------------------------------------------------

/// Internal rank tier for deterministic dominance ordering.
///
/// Lower number = higher rank. Tier 0 (exact ID) always outranks any
/// combination of lower-tier matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RankTier {
    ExactId = 0,
    ExactName = 1,
    IdSubstring = 2,
    NameSubstring = 3,
    Other = 4,
}

// ---------------------------------------------------------------------------
// Search document
// ---------------------------------------------------------------------------

/// Provenance terms derived from canonical trait data and optional context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Provenance {
    pub status: String,
    pub trust: String,
    pub version: String,
    /// Source layout evidence, e.g. `"local-package"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_layout: String,
    /// Package-relative path evidence under the repo-local trait source root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package_path: String,
    /// Project-manifest source kind evidence, e.g. `local-path` or `git`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_source_kind: String,
}

/// Resource evidence available without reading resource bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResourceEvidence {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Context that can be supplied to `build_search_document_with_context` when
/// the caller has loaded target-profile or source-layout evidence from a
/// project manifest or package context.
#[derive(Debug, Clone, Default)]
pub struct BuildContext<'a> {
    /// Flat list of `(trait_id, profile)` pairs.
    pub target_profiles: &'a [(&'a str, &'a str)],
    /// `"loaded"` or `"not-loaded"`.
    pub target_profile_evidence: &'a str,
    /// Source layout label, e.g. `"local-package"`.
    pub source_layout: &'a str,
    /// Package-relative path evidence under the repo-local trait source root.
    pub package_path: &'a str,
    /// Project-manifest source kind evidence, e.g. `local-path` or `git`.
    pub manifest_source_kind: &'a str,
    /// Caller-resolved package manifest `[package].status` display string.
    /// The canonical trait document carries no status field of its own.
    pub status: &'a str,
    /// Caller-resolved machine trust store verdict display string. The
    /// canonical trait document carries no trust field of its own.
    pub trust: &'a str,
}

/// A precompiled search document for one trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Document {
    pub trait_id: String,
    pub name: String,
    pub summary: String,
    pub keywords: Vec<String>,
    pub body_text: String,
    pub resource_evidence: Vec<ResourceEvidence>,
    pub provenance: Provenance,
    pub target_profiles: Vec<String>,
    /// `"loaded"` or `"not-loaded"`.
    pub target_profile_evidence: String,
}

/// Build a search document from a trusted trait model with no external context.
///
/// Target profiles will be empty and `target_profile_evidence` will be
/// `"not-loaded"`. `status`/`trust` are caller-resolved package manifest
/// status and machine trust verdict display strings — the canonical trait
/// document carries neither field.
pub fn build_search_document(t: &Trait, status: &str, trust: &str) -> Document {
    let ctx = BuildContext {
        target_profile_evidence: "not-loaded",
        status,
        trust,
        ..BuildContext::default()
    };
    build_search_document_with_context(t, ctx)
}

/// Build a search document with optional target-profile and provenance context.
pub fn build_search_document_with_context(t: &Trait, ctx: BuildContext<'_>) -> Document {
    let mut keywords = Vec::new();

    if let Some(metadata) = &t.metadata {
        for slug in metadata.tag.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in metadata.job.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in metadata.domain.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in metadata.artifact.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in metadata.audience.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in metadata.environment.iter() {
            keywords.push(slug.as_str().to_string());
        }
    }

    if let Some(activation) = &t.activation {
        for rule in &activation.rules {
            for mode in rule.mode.iter() {
                keywords.push(mode.clone());
            }
            for lang in rule.language.iter() {
                keywords.push(lang.clone());
            }
            for kw in rule.task_keyword.iter() {
                keywords.push(kw.clone());
            }
            for phrase in rule.explicit_phrase.iter() {
                keywords.push(phrase.clone());
            }
        }
    }

    if let Some(behavior) = &t.behavior {
        for slug in behavior.tone.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in behavior.method.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in behavior.format.iter() {
            keywords.push(slug.as_str().to_string());
        }
    }

    if let Some(intent) = &t.intent {
        for slug in intent.focus.iter() {
            keywords.push(slug.as_str().to_string());
        }
        for slug in intent.require.iter() {
            keywords.push(slug.as_str().to_string());
        }
    }

    keywords.sort();
    keywords.dedup();

    let mut body_parts = Vec::new();
    if let Some(intent) = &t.intent {
        for slug in intent.avoid.iter() {
            body_parts.push(format!("avoid: {}", slug.as_str()));
        }
        for slug in intent.block.iter() {
            body_parts.push(format!("block: {}", slug.as_str()));
        }
    }
    for scenario in &t.scenarios {
        if let Some(input) = &scenario.input {
            body_parts.push(input.clone());
        }
        if let Some(output) = &scenario.output {
            body_parts.push(output.clone());
        }
    }
    if let Some(proc) = &t.procedure {
        body_parts.push(proc.description.clone());
        for seq in &proc.sequence {
            body_parts.push(seq.title.clone());
            match seq.effective_kind() {
                crate::r#trait::procedure::SequenceKind::Prompt => {
                    body_parts.push(seq.prompt.clone())
                }
                crate::r#trait::procedure::SequenceKind::Ask => body_parts.push(seq.prompt.clone()),
                crate::r#trait::procedure::SequenceKind::Command => {
                    body_parts.push("runtime command step".to_string())
                }
                crate::r#trait::procedure::SequenceKind::Check => {
                    body_parts.push("runtime check step".to_string())
                }
                crate::r#trait::procedure::SequenceKind::Project => {
                    body_parts.push("deterministic runtime projection".to_string())
                }
                crate::r#trait::procedure::SequenceKind::Sequence => {
                    if let Some(sequence_ref) = seq.sequence.as_ref() {
                        body_parts.push(format!("nested runtime sequence {sequence_ref}"));
                    }
                }
                crate::r#trait::procedure::SequenceKind::Branch => {
                    body_parts.push("typed runtime branch".to_string());
                }
                crate::r#trait::procedure::SequenceKind::Loop => {
                    body_parts.push("bounded runtime loop".to_string());
                }
                crate::r#trait::procedure::SequenceKind::ForEach => {
                    body_parts.push("typed runtime for-each".to_string());
                }
                crate::r#trait::procedure::SequenceKind::Parallel => {
                    body_parts.push("typed runtime parallel".to_string());
                }
                crate::r#trait::procedure::SequenceKind::Terminal => {
                    body_parts.push("runtime authored terminal".to_string());
                    if let Some(message) = seq.message.as_ref() {
                        body_parts.push(message.clone());
                    }
                }
            }
        }
    }
    for (id, sequence) in t.sequences.iter() {
        body_parts.push(format!("named runtime sequence {id}"));
        for item in &sequence.sequence {
            body_parts.push(item.title.clone());
        }
    }
    for id in t.conditions.keys() {
        body_parts.push(format!("typed runtime condition {id}"));
    }

    let resource_evidence: Vec<ResourceEvidence> = t
        .resources
        .iter()
        .map(|r| ResourceEvidence {
            id: r.id.clone(),
            path: r.path.clone(),
            hint: r.hint.clone(),
        })
        .collect();

    let target_profiles: Vec<String> = ctx
        .target_profiles
        .iter()
        .filter(|(id, _)| *id == t.id.as_str())
        .map(|(_, profile)| profile.to_string())
        .collect();

    let provenance = Provenance {
        status: ctx.status.to_string(),
        trust: ctx.trust.to_string(),
        version: t.version.as_str().to_string(),
        source_layout: ctx.source_layout.to_string(),
        package_path: ctx.package_path.to_string(),
        manifest_source_kind: ctx.manifest_source_kind.to_string(),
    };

    let tpe = if ctx.target_profile_evidence.is_empty() {
        "not-loaded"
    } else {
        ctx.target_profile_evidence
    };

    Document {
        trait_id: t.id.as_str().to_string(),
        name: t.name.as_str().to_string(),
        summary: t.effective_summary().to_string(),
        keywords,
        body_text: body_parts.join(" "),
        resource_evidence,
        provenance,
        target_profiles,
        target_profile_evidence: tpe.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Search result
// ---------------------------------------------------------------------------

/// A single match reason for a search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MatchReason {
    pub field: String,
    pub matched_term: String,
}

/// A ranked search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Hit {
    pub trait_id: String,
    pub name: String,
    pub rank_tier: u32,
    pub score: i32,
    pub match_reasons: Vec<MatchReason>,
}

/// Search a collection of documents and return deterministic ranked results.
///
/// Ranking uses tier-based dominance:
/// - Tier 0: exact ID match
/// - Tier 1: exact name match
/// - Tier 2: ID substring
/// - Tier 3: name substring
/// - Tier 4: all other lexical matches (keyword 100, resource 80,
///   target-profile 60, summary 50, provenance 30, body 10)
///
/// Results are sorted by tier ascending, then score descending, then trait ID
/// ascending for deterministic ties.
pub fn search_traits(query: &str, documents: &[Document]) -> Vec<Hit> {
    let query_lower = query.to_lowercase();
    let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results = Vec::new();

    for doc in documents {
        let mut score = 0i32;
        let mut reasons = Vec::new();
        let mut best_tier = RankTier::Other;

        let id_lower = doc.trait_id.to_lowercase();
        let name_lower = doc.name.to_lowercase();

        if id_lower == query_lower && !query_lower.is_empty() {
            score += 1000;
            best_tier = best_tier.min(RankTier::ExactId);
            reasons.push(MatchReason {
                field: "id".to_string(),
                matched_term: doc.trait_id.clone(),
            });
        } else if !query_lower.is_empty() && id_lower.contains(&query_lower) {
            score += 600;
            best_tier = best_tier.min(RankTier::IdSubstring);
            reasons.push(MatchReason {
                field: "id".to_string(),
                matched_term: doc.trait_id.clone(),
            });
        }

        if name_lower == query_lower && !query_lower.is_empty() {
            score += 800;
            best_tier = best_tier.min(RankTier::ExactName);
            reasons.push(MatchReason {
                field: "name".to_string(),
                matched_term: doc.name.clone(),
            });
        } else if !query_lower.is_empty() && name_lower.contains(&query_lower) {
            score += 400;
            best_tier = best_tier.min(RankTier::NameSubstring);
            reasons.push(MatchReason {
                field: "name".to_string(),
                matched_term: doc.name.clone(),
            });
        }

        for keyword in &doc.keywords {
            let kw_lower = keyword.to_lowercase();
            for term in &query_terms {
                if kw_lower.contains(term) {
                    score += 100;
                    reasons.push(MatchReason {
                        field: "keyword".to_string(),
                        matched_term: keyword.clone(),
                    });
                    break;
                }
            }
        }

        for resource in &doc.resource_evidence {
            let res_text = format!(
                "resource:{} {} {}",
                resource.id,
                resource.path.as_deref().unwrap_or(""),
                resource.hint.as_deref().unwrap_or("")
            );
            let res_lower = res_text.to_lowercase();
            for term in &query_terms {
                if res_lower.contains(term) {
                    score += 80;
                    reasons.push(MatchReason {
                        field: "resource".to_string(),
                        matched_term: resource.id.clone(),
                    });
                    break;
                }
            }
        }

        for profile in &doc.target_profiles {
            let prof_lower = profile.to_lowercase();
            for term in &query_terms {
                if prof_lower.contains(term) {
                    score += 60;
                    reasons.push(MatchReason {
                        field: "target-profile".to_string(),
                        matched_term: profile.clone(),
                    });
                    break;
                }
            }
        }

        let summary_lower = doc.summary.to_lowercase();
        for term in &query_terms {
            if summary_lower.contains(term) {
                score += 50;
                reasons.push(MatchReason {
                    field: "summary".to_string(),
                    matched_term: term.to_string(),
                });
                break;
            }
        }

        let prov_text = format!(
            "{} {} {} {} {} {}",
            doc.provenance.status,
            doc.provenance.trust,
            doc.provenance.version,
            doc.provenance.source_layout,
            doc.provenance.package_path,
            doc.provenance.manifest_source_kind,
        );
        let prov_lower = prov_text.to_lowercase();
        for term in &query_terms {
            if prov_lower.contains(term) {
                score += 30;
                reasons.push(MatchReason {
                    field: "provenance".to_string(),
                    matched_term: term.to_string(),
                });
                break;
            }
        }

        let body_lower = doc.body_text.to_lowercase();
        for term in &query_terms {
            if body_lower.contains(term) {
                score += 10;
                reasons.push(MatchReason {
                    field: "body".to_string(),
                    matched_term: term.to_string(),
                });
                break;
            }
        }

        if score > 0 {
            reasons.sort_by(|a, b| (&a.field, &a.matched_term).cmp(&(&b.field, &b.matched_term)));
            reasons.dedup_by(|a, b| a.field == b.field && a.matched_term == b.matched_term);

            results.push(Hit {
                trait_id: doc.trait_id.clone(),
                name: doc.name.clone(),
                rank_tier: best_tier as u32,
                score,
                match_reasons: reasons,
            });
        }
    }

    results.sort_by(|a, b| {
        a.rank_tier
            .cmp(&b.rank_tier)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.trait_id.cmp(&b.trait_id))
    });

    results
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------
