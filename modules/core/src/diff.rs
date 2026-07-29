//! Layer-aware diff: structured comparison across review layers.
//!
//! Diffs separate review layers rather than flattening all text. Each layer
//! (canonical source, model-view, resource manifest, policy, export, lock) is
//! compared independently. The diff report records before/after digests,
//! summaries, unsupported status, and optional text hunks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::check::DriftLayer;
use crate::digest::Digest;

// ---------------------------------------------------------------------------
// Diff entry and report
// ---------------------------------------------------------------------------

/// A text hunk showing a diff fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DiffHunk {
    /// Line number in the before content (1-based), if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_line: Option<usize>,
    /// Line number in the after content (1-based), if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_line: Option<usize>,
    /// The diff text fragment.
    pub text: String,
}

/// A single diff entry comparing one layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DiffEntry {
    /// The layer being compared.
    pub layer: DriftLayer,
    /// The before (old) digest, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<Digest>,
    /// The after (new) digest, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<Digest>,
    /// Whether the digests differ.
    pub changed: bool,
    /// Human-readable summary.
    pub summary: String,
    /// Whether this layer's comparison is not yet implemented.
    pub unsupported: bool,
    /// Optional text hunks showing the diff content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<DiffHunk>,
}

/// A diff request specifying which layers to compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DiffRequest {
    /// The trait ID to diff.
    pub trait_id: String,
    /// Which layers to compare (empty = all supported layers).
    pub layers: Vec<DriftLayer>,
    /// Whether to compare against lock data.
    pub from_lock: bool,
}

/// The complete diff report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DiffReport {
    /// The trait ID being diffed.
    pub trait_id: String,
    /// One entry per compared layer.
    pub entries: Vec<DiffEntry>,
}

/// Create a diff entry for an unsupported layer.
pub fn unsupported_layer(layer: DriftLayer, reason: &str) -> DiffEntry {
    let name = layer_name(&layer);
    DiffEntry {
        layer,
        before_digest: None,
        after_digest: None,
        changed: false,
        summary: format!("{name}: {reason}"),
        unsupported: true,
        hunks: Vec::new(),
    }
}

/// Create a diff entry for a digest comparison.
pub fn digest_diff(layer: DriftLayer, before: Option<&str>, after: Option<&str>) -> DiffEntry {
    let changed = match (before, after) {
        (Some(b), Some(a)) => b != a,
        _ => true,
    };

    let name = layer_name(&layer);

    DiffEntry {
        layer,
        before_digest: before.map(Digest::from_unvalidated),
        after_digest: after.map(Digest::from_unvalidated),
        changed,
        summary: if changed {
            format!("{name}: digest changed")
        } else if before.is_none() && after.is_some() {
            format!("{name}: no baseline evidence")
        } else if before.is_some() && after.is_none() {
            format!("{name}: current evidence missing")
        } else if before.is_none() && after.is_none() {
            format!("{name}: comparison evidence missing")
        } else {
            format!("{name}: no change")
        },
        unsupported: false,
        hunks: Vec::new(),
    }
}

/// Create a diff entry for a missing baseline.
pub fn missing_baseline(layer: DriftLayer, after: Option<&str>, reason: &str) -> DiffEntry {
    let name = layer_name(&layer);
    DiffEntry {
        layer,
        before_digest: None,
        after_digest: after.map(Digest::from_unvalidated),
        changed: true,
        summary: format!("{name}: missing baseline evidence ({reason})"),
        unsupported: false,
        hunks: Vec::new(),
    }
}

fn layer_name(layer: &DriftLayer) -> &'static str {
    match layer {
        DriftLayer::CanonicalSource => "canonical-source",
        DriftLayer::ModelView => "model-view",
        DriftLayer::ResourceManifest => "resource-manifest",
        DriftLayer::PolicyManifest => "policy-manifest",
        DriftLayer::Export => "export",
        DriftLayer::Lock => "lock",
        DriftLayer::ProjectionLock => "projection-lock",
        DriftLayer::Dependency => "dependency",
    }
}
