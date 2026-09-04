//! Digest drift report types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A digest drift summary comparing expected and actual digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DriftSummary {
    /// What layer the drift was detected on.
    pub layer: DriftLayer,
    /// The expected (locked) digest.
    pub expected: String,
    /// The actual (current) digest, if computable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Human-readable summary.
    pub summary: String,
    /// Whether this layer's comparison is not yet implemented.
    pub unsupported: bool,
}

/// The layers that can be checked for drift.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DriftLayer {
    /// Canonical source trait file.
    CanonicalSource,
    /// Compiled model-visible text.
    ModelView,
    /// Resource manifest.
    ResourceManifest,
    /// Policy manifest.
    PolicyManifest,
    /// Generated export.
    Export,
    /// Lockfile state.
    Lock,
    /// Package-local static skill projection lock.
    ProjectionLock,
    /// Vendored dependency source/model/resource evidence.
    Dependency,
}
