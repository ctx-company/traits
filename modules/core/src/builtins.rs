//! Typed built-in SDK vocabulary generated from `builtins.toml` at compile time.

use serde::Serialize;

/// A built-in vocabulary entry with its render summary and SDK-facing description.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BuiltinDefinition {
    pub slug: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    /// Judgment-shaped, imperative render line for the tagged behavior render
    /// (rule 4/5). Falls back mechanically to `summary` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directive: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/builtins.rs"));
