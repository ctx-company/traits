//! Source anchor sidecar data for CDK-authored traits.
//!
//! Source maps are deterministic sidecar artifacts. They are not embedded in
//! canonical trait bytes and never participate in canonical digest computation.
//! Virtual procedure-step anchors use `sequence:procedure/<step-id>`; dependency
//! alias `procedure` is reserved so these keys cannot collide with dependency refs.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Line-range anchor for an authored trait construct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SourceAnchor {
    /// Authored source file path as reported by the CDK runtime.
    pub file: String,
    /// One-based start line.
    pub start: usize,
    /// One-based inclusive end line.
    pub end: usize,
}

/// Map from canonical construct refs to authored source anchors.
pub type SourceMap = BTreeMap<String, SourceAnchor>;

/// Validate a source map read from CDK output or a `trait.map` sidecar.
pub fn validate_source_map(map: &SourceMap) -> crate::Result<()> {
    for (ref_text, anchor) in map {
        crate::reference::Reference::parse(ref_text)?;
        if anchor.file.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("source-map.{ref_text}.file"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
        if anchor.start == 0 {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("source-map.{ref_text}.start"),
                message: "must be a one-based line number".to_string(),
            }
            .into());
        }
        if anchor.end < anchor.start {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("source-map.{ref_text}.end"),
                message: "must be greater than or equal to start".to_string(),
            }
            .into());
        }
    }
    Ok(())
}
