//! Cache lifecycle: plan rebuild, status, and prune for static artifacts.
//!
//! Cache artifacts are keyed by trait ID, version, source digest, render
//! profile, load level, and resource digests. Cache commands never mutate
//! canonical trait state. Stale or unreachable artifacts are reported with
//! deterministic per-field reasons.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::digest::Digest;

// ---------------------------------------------------------------------------
// Cache artifact key
// ---------------------------------------------------------------------------

/// A deterministic cache artifact key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CacheArtifactKey {
    pub trait_id: String,
    pub version: String,
    pub source_digest: Digest,
    pub render_profile: String,
    pub load_level: String,

    /// Resource digests contributing to cache identity. Always serialized
    /// (even when empty) so coverage is visible.
    #[serde(default)]
    pub resource_digests: Vec<Digest>,

    /// Coverage note: `"not-loaded"` or `"loaded"`.
    #[serde(default)]
    pub resource_digest_coverage: String,
}

impl CacheArtifactKey {
    /// Compute a deterministic filesystem-safe artifact identifier.
    ///
    /// Includes all key fields: trait ID, version, source digest, render
    /// profile, load level, and sorted resource digests.
    pub fn artifact_id(&self) -> String {
        let sorted_resources = self.normalized_resource_digests();

        let resource_part = if sorted_resources.is_empty() {
            "no-resources".to_string()
        } else {
            sorted_resources
                .iter()
                .map(Digest::as_str)
                .collect::<Vec<_>>()
                .join(",")
        };

        format!(
            "{}-{}-{}-{}-{}-{}",
            self.trait_id,
            self.version,
            self.source_digest,
            self.render_profile,
            self.load_level,
            Digest::source(&resource_part).as_str(),
        )
    }

    /// Compare this key against another and return per-field mismatch reasons.
    fn field_mismatches(&self, other: &CacheArtifactKey) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.source_digest != other.source_digest {
            reasons.push("source-digest-changed".to_string());
        }
        if self.version != other.version {
            reasons.push("version-changed".to_string());
        }
        if self.render_profile != other.render_profile {
            reasons.push("render-profile-changed".to_string());
        }
        if self.load_level != other.load_level {
            reasons.push("load-level-changed".to_string());
        }
        let self_res = self.normalized_resource_digests();
        let other_res = other.normalized_resource_digests();
        if self_res != other_res {
            reasons.push("resource-digests-changed".to_string());
        }
        reasons.sort();
        reasons
    }

    fn normalized_resource_digests(&self) -> Vec<Digest> {
        let mut digests = self.resource_digests.clone();
        digests.sort();
        digests.dedup();
        digests
    }
}

/// A cache artifact record as stored in generated cache metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct StoredCacheArtifact {
    pub artifact_id: String,
    pub key: CacheArtifactKey,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_size: Option<u64>,
}

// ---------------------------------------------------------------------------
// Cache artifact status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CacheArtifactStatus {
    pub key: CacheArtifactKey,
    #[serde(default)]
    pub artifact_id: String,
    pub fresh: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_reasons: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_size: Option<u64>,
}

// ---------------------------------------------------------------------------
// Cache plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CachePlan {
    #[serde(default, rename = "entry", skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<CacheArtifactStatus>,

    pub total_count: usize,
    pub fresh_count: usize,
    pub stale_count: usize,
    pub prune_count: usize,
    pub missing_count: usize,
}

// ---------------------------------------------------------------------------
// Planning functions
// ---------------------------------------------------------------------------

/// Plan a cache rebuild from current artifact keys.
///
/// All entries are marked fresh because a rebuild regenerates everything.
pub fn plan_cache_rebuild(keys: &[CacheArtifactKey]) -> CachePlan {
    let mut entries: Vec<CacheArtifactStatus> = keys
        .iter()
        .map(|key| CacheArtifactStatus {
            key: key.clone(),
            artifact_id: key.artifact_id(),
            fresh: true,
            stale_reason: None,
            stale_reasons: Vec::new(),
            artifact_size: None,
        })
        .collect();
    sort_status_entries(&mut entries);

    let count = entries.len();
    CachePlan {
        entries,
        total_count: count,
        fresh_count: count,
        stale_count: 0,
        prune_count: 0,
        missing_count: 0,
    }
}

/// Compare stored cache artifacts against current trait inventory.
///
/// Uses complete artifact identity (all key fields) for comparison.
/// Reports fresh, stale (with per-field reasons), unreachable, and missing.
pub fn compare_cache_status(
    stored: &[StoredCacheArtifact],
    current: &[CacheArtifactKey],
) -> CachePlan {
    let current_by_artifact_id: BTreeMap<String, &CacheArtifactKey> =
        current.iter().map(|key| (key.artifact_id(), key)).collect();

    let mut current_by_trait_id: BTreeMap<&str, Vec<&CacheArtifactKey>> = BTreeMap::new();
    for key in current {
        current_by_trait_id
            .entry(key.trait_id.as_str())
            .or_default()
            .push(key);
    }

    let stored_artifact_ids: BTreeSet<&str> = stored
        .iter()
        .map(|artifact| artifact.artifact_id.as_str())
        .collect();

    let mut entries = Vec::new();
    let mut fresh = 0usize;
    let mut stale = 0usize;
    let mut missing = 0usize;

    for artifact in stored {
        let mut reasons = Vec::new();
        if artifact.artifact_id != artifact.key.artifact_id() {
            reasons.push("artifact-id-mismatch".to_string());
        }

        if let Some(current_key) = current_by_artifact_id.get(artifact.artifact_id.as_str()) {
            reasons.extend(artifact.key.field_mismatches(current_key));
        } else if let Some(current_key) = closest_current_key(&artifact.key, &current_by_trait_id) {
            reasons.extend(artifact.key.field_mismatches(current_key));
            if reasons.is_empty() {
                reasons.push("artifact-id-changed".to_string());
            }
        } else {
            reasons.push("unreachable".to_string());
        }

        reasons.sort();
        reasons.dedup();

        if reasons.is_empty() {
            fresh += 1;
            entries.push(status_from_stored_artifact(artifact, true, Vec::new()));
        } else {
            stale += 1;
            entries.push(status_from_stored_artifact(artifact, false, reasons));
        }
    }

    for current_key in current {
        let current_artifact_id = current_key.artifact_id();
        if !stored_artifact_ids.contains(current_artifact_id.as_str()) {
            missing += 1;
            entries.push(CacheArtifactStatus {
                key: current_key.clone(),
                artifact_id: current_artifact_id,
                fresh: false,
                stale_reason: Some("missing-cache-artifact".to_string()),
                stale_reasons: vec!["missing-cache-artifact".to_string()],
                artifact_size: None,
            });
        }
    }

    sort_status_entries(&mut entries);

    CachePlan {
        entries,
        total_count: fresh + stale + missing,
        fresh_count: fresh,
        stale_count: stale,
        prune_count: 0,
        missing_count: missing,
    }
}

/// Plan cache prune from stored artifacts compared to current inventory.
///
/// Uses complete artifact identity. Stale or unreachable artifacts are marked
/// for pruning.
pub fn plan_cache_prune_from_stored(
    stored: &[StoredCacheArtifact],
    current: &[CacheArtifactKey],
) -> CachePlan {
    let mut plan = compare_cache_status(stored, current);
    plan.prune_count = plan.stale_count;
    plan
}

/// Given a prune plan, return the stored artifacts that survive (are fresh).
pub fn surviving_artifacts(plan: &CachePlan) -> Vec<StoredCacheArtifact> {
    plan.entries
        .iter()
        .filter(|e| e.fresh)
        .map(|e| StoredCacheArtifact {
            artifact_id: e.artifact_id.clone(),
            key: e.key.clone(),
            artifact_size: e.artifact_size,
        })
        .collect()
}

fn closest_current_key<'a>(
    stored_key: &CacheArtifactKey,
    current_by_trait_id: &BTreeMap<&str, Vec<&'a CacheArtifactKey>>,
) -> Option<&'a CacheArtifactKey> {
    current_by_trait_id
        .get(stored_key.trait_id.as_str())
        .and_then(|keys| {
            keys.iter().copied().min_by(|left, right| {
                let left_mismatches = stored_key.field_mismatches(left).len();
                let right_mismatches = stored_key.field_mismatches(right).len();
                left_mismatches
                    .cmp(&right_mismatches)
                    .then_with(|| left.artifact_id().cmp(&right.artifact_id()))
            })
        })
}

fn status_from_stored_artifact(
    artifact: &StoredCacheArtifact,
    fresh: bool,
    stale_reasons: Vec<String>,
) -> CacheArtifactStatus {
    CacheArtifactStatus {
        key: artifact.key.clone(),
        artifact_id: artifact.artifact_id.clone(),
        fresh,
        stale_reason: if stale_reasons.is_empty() {
            None
        } else {
            Some(stale_reasons.join("; "))
        },
        stale_reasons,
        artifact_size: artifact.artifact_size,
    }
}

fn sort_status_entries(entries: &mut [CacheArtifactStatus]) {
    entries.sort_by(|a, b| {
        a.artifact_id
            .cmp(&b.artifact_id)
            .then_with(|| a.key.trait_id.cmp(&b.key.trait_id))
    });
}
