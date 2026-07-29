//! Context pack planning: convert activation plans into context packs.
//!
//! A context pack is a planned collection of rendered frames for selected
//! traits, with token estimates, de-duplication decisions, and warnings. The
//! context ledger is an operational estimate, not proof that a model retained
//! context.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::render::{ExtendedRenderProfile, RenderPlan, plan_render};
use crate::resolve::{LoadLevel, Response};
use crate::r#trait::Trait;

// ---------------------------------------------------------------------------
// Context pack frame
// ---------------------------------------------------------------------------

/// Structured host-injection capability warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HostInjectionWarning {
    pub code: String,
    pub capability: String,
    pub status: String,
    pub message: String,
}

impl Default for HostInjectionWarning {
    fn default() -> Self {
        Self {
            code: "host-injection-unsupported-or-unknown".to_string(),
            capability: "host-injection".to_string(),
            status: "unsupported-or-unknown".to_string(),
            message: "context pack is a local planning artifact, not proof of model memory or host injection".to_string(),
        }
    }
}

/// De-duplication status for a context pack frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DeduplicationStatus {
    Included,
    DuplicateSkipped,
}

/// A single rendered frame in a context pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Frame {
    pub trait_id: String,
    pub version: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,

    pub render_profile: String,
    pub load_level: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<Digest>,

    pub estimated_tokens: u64,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,

    pub deduplication_status: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of_content_digest: Option<Digest>,
}

// ---------------------------------------------------------------------------
// Context pack
// ---------------------------------------------------------------------------

/// A planned context pack.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Pack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,

    pub total_estimated_tokens: u64,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub de_duplication_notes: Vec<String>,

    pub host_injection_warning: HostInjectionWarning,
}

/// Plan a context pack from a resolve response and loaded traits.
///
/// Each selected trait is rendered using `plan_render` with the specified
/// profile. Load level controls how much content is included:
/// - Discovery: frame metadata only (no body)
/// - Summary: frame with model-view full text
/// - Full: frame with model-view full text (same as summary in MVP)
///
/// De-duplication removes duplicate content digests across frames.
pub fn plan_context_pack(
    resolve_response: &Response,
    traits: &[Trait],
    profile: ExtendedRenderProfile,
    source_digests: &[(&str, &str)],
) -> Pack {
    let mut frames = Vec::new();
    let mut warnings = Vec::new();
    let mut de_dup_notes = Vec::new();
    let mut seen_digests: std::collections::BTreeSet<Digest> = std::collections::BTreeSet::new();
    let mut total_tokens: u64 = 0;

    for candidate in &resolve_response.selected {
        let trait_ref = match traits.iter().find(|t| t.id.as_str() == candidate.trait_id) {
            Some(t) => t,
            None => {
                warnings.push(format!(
                    "selected trait {} not found in loaded traits",
                    candidate.trait_id
                ));
                continue;
            }
        };

        let source_digest = source_digests
            .iter()
            .find(|(id, _)| *id == candidate.trait_id)
            .map(|(_, digest)| *digest);

        let load_level = parse_load_level(&candidate.load_level);

        let content = match load_level {
            LoadLevel::Discovery => String::new(),
            LoadLevel::Summary | LoadLevel::Full => {
                let sd = match source_digest {
                    Some(s) => s,
                    None => {
                        warnings.push(format!(
                            "selected trait {} skipped: missing source digest",
                            candidate.trait_id
                        ));
                        continue;
                    }
                };
                let render_plan: RenderPlan = plan_render(trait_ref, profile, sd);
                let artifact = render_plan
                    .model_view
                    .artifact_for_load_level(load_level)
                    .expect("content-bearing load levels have a model-view artifact");
                artifact.0.to_string()
            }
        };

        let content_digest = if content.is_empty() {
            None
        } else {
            let digest = Digest::source(&content);
            if seen_digests.contains(&digest) {
                de_dup_notes.push(format!(
                    "duplicate content digest for trait {} skipped",
                    candidate.trait_id
                ));
                frames.push(Frame {
                    trait_id: candidate.trait_id.clone(),
                    version: candidate.version.clone(),
                    source_digest: source_digest.map(Digest::from_unvalidated),
                    render_profile: profile.as_str().to_string(),
                    load_level: candidate.load_level.clone(),
                    content_digest: Some(digest.clone()),
                    estimated_tokens: crate::discovery_index::estimate_tokens(&content),
                    content: String::new(),
                    deduplication_status: "duplicate-skipped".to_string(),
                    duplicate_of_content_digest: Some(digest),
                });
                continue;
            }
            seen_digests.insert(digest.clone());
            Some(digest)
        };

        let estimated_tokens = crate::discovery_index::estimate_tokens(&content);
        total_tokens = total_tokens.saturating_add(estimated_tokens);

        frames.push(Frame {
            trait_id: candidate.trait_id.clone(),
            version: candidate.version.clone(),
            source_digest: source_digest.map(Digest::from_unvalidated),
            render_profile: profile.as_str().to_string(),
            load_level: candidate.load_level.clone(),
            content_digest,
            estimated_tokens,
            content,
            deduplication_status: "included".to_string(),
            duplicate_of_content_digest: None,
        });
    }

    if resolve_response.budget.exceeded {
        warnings.push(format!(
            "budget exceeded: used {} of {} tokens",
            resolve_response.budget.used,
            resolve_response.budget.total.unwrap_or(0)
        ));
    }

    if frames.is_empty() && resolve_response.selected.is_empty() {
        warnings.push("no traits selected for context pack".to_string());
    }

    Pack {
        frames,
        total_estimated_tokens: total_tokens,
        warnings,
        de_duplication_notes: de_dup_notes,
        host_injection_warning: HostInjectionWarning::default(),
    }
}

fn parse_load_level(s: &str) -> LoadLevel {
    match s {
        "summary" => LoadLevel::Summary,
        "full" => LoadLevel::Full,
        _ => LoadLevel::Discovery,
    }
}
