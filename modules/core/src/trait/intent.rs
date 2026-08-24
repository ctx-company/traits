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
        self.validate_groups("intent")?;
        Ok(())
    }

    /// Validate intent declared inside one agent role. Unlike root intent,
    /// requiring and avoiding the same canonical guidance id is contradictory
    /// in a role-local instruction set.
    pub fn validate_scoped(&self, field_path: &str) -> crate::Result<()> {
        self.validate_groups(field_path)?;
        let required = self
            .require
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for (index, item) in self.avoid.iter().enumerate() {
            if required.contains(item.id.as_str()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.avoid[{index}].id"),
                    message: format!(
                        "guidance id {:?} cannot appear in both require and avoid",
                        item.id.as_str()
                    ),
                }
                .into());
            }
        }
        Ok(())
    }

    fn validate_groups(&self, field_path: &str) -> crate::Result<()> {
        validate_guidance_list(&self.require, &format!("{field_path}.require"))?;
        validate_guidance_list(&self.focus, &format!("{field_path}.focus"))?;
        validate_guidance_list(&self.avoid, &format!("{field_path}.avoid"))?;
        validate_guidance_list(&self.block, &format!("{field_path}.block"))?;
        Ok(())
    }
}
