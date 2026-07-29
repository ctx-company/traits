//! Intent model: what the trait requires, focuses on, avoids, and blocks.
//!
//! Additive fields are vector-like (scalar-or-array via `SlugList`).
//! Mutually exclusive controls are scalar. Intent is separate from
//! activation — it describes desired outcomes, not trigger conditions.
//! Slug validation happens in the taxonomy validation pass.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::r#trait::guidance::{GuidanceItem, GuidanceItemList, validate_guidance_list};

pub type IntentItem = GuidanceItem;
pub type IntentItemList = GuidanceItemList;

/// `[intent]` model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Intent {
    /// Required outcomes or actions.
    #[serde(default, skip_serializing_if = "IntentItemList::is_empty")]
    pub require: IntentItemList,

    /// Areas to focus attention on.
    #[serde(default, skip_serializing_if = "IntentItemList::is_empty")]
    pub focus: IntentItemList,

    /// Things to avoid doing.
    #[serde(default, skip_serializing_if = "IntentItemList::is_empty")]
    pub avoid: IntentItemList,

    /// Things that must never happen.
    #[serde(default, skip_serializing_if = "IntentItemList::is_empty")]
    pub block: IntentItemList,
}

impl Intent {
    pub fn validate_taxonomy(&self) -> crate::Result<()> {
        validate_guidance_list(&self.require, "intent.require")?;
        validate_guidance_list(&self.focus, "intent.focus")?;
        validate_guidance_list(&self.avoid, "intent.avoid")?;
        validate_guidance_list(&self.block, "intent.block")?;
        Ok(())
    }
}
