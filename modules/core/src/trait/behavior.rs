//! Behavior model: how the trait should act.
//!
//! Additive fields (tone, method, format) are vector-like (scalar-or-array
//! via `SlugList`, always serialized as arrays). Mutually exclusive controls
//! (verbosity, directness, scope-control, initiative, uncertainty) are
//! scalar validated slugs. Slug validation happens in the taxonomy
//! validation pass.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::r#trait::guidance::{
    GuidanceItem, GuidanceItemList, deserialize_optional_guidance_item, validate_guidance_list,
    validate_guidance_option,
};

pub type BehaviorItem = GuidanceItem;
pub type BehaviorItemList = GuidanceItemList;

/// `[behavior]` model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Behavior {
    /// Tone descriptors (e.g. "technical", "direct").
    #[serde(default, skip_serializing_if = "BehaviorItemList::is_empty")]
    pub tone: BehaviorItemList,

    /// Method descriptors (e.g. "evidence-first").
    #[serde(default, skip_serializing_if = "BehaviorItemList::is_empty")]
    pub method: BehaviorItemList,

    /// Format descriptors (e.g. "findings-first", "bullets").
    #[serde(default, skip_serializing_if = "BehaviorItemList::is_empty")]
    pub format: BehaviorItemList,

    /// Verbosity level: scalar slug.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_guidance_item",
        skip_serializing_if = "Option::is_none"
    )]
    pub verbosity: Option<BehaviorItem>,

    /// Directness level: scalar slug.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_guidance_item",
        skip_serializing_if = "Option::is_none"
    )]
    pub directness: Option<BehaviorItem>,

    /// Scope control: scalar slug.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_guidance_item",
        skip_serializing_if = "Option::is_none"
    )]
    pub scope_control: Option<BehaviorItem>,

    /// Initiative: scalar slug.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_guidance_item",
        skip_serializing_if = "Option::is_none"
    )]
    pub initiative: Option<BehaviorItem>,

    /// Uncertainty handling: scalar slug.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_guidance_item",
        skip_serializing_if = "Option::is_none"
    )]
    pub uncertainty: Option<BehaviorItem>,
}

impl Behavior {
    pub fn validate_taxonomy(&self) -> crate::Result<()> {
        self.validate_axes("behavior")
    }

    pub(crate) fn validate_scoped(&self, field_prefix: &str) -> crate::Result<()> {
        self.validate_axes(field_prefix)
    }

    fn validate_axes(&self, field_prefix: &str) -> crate::Result<()> {
        validate_guidance_list(&self.tone, &format!("{field_prefix}.tone"))?;
        validate_guidance_list(&self.method, &format!("{field_prefix}.method"))?;
        validate_guidance_list(&self.format, &format!("{field_prefix}.format"))?;
        validate_guidance_option(&self.verbosity, &format!("{field_prefix}.verbosity"))?;
        validate_guidance_option(&self.directness, &format!("{field_prefix}.directness"))?;
        validate_guidance_option(
            &self.scope_control,
            &format!("{field_prefix}.scope-control"),
        )?;
        validate_guidance_option(&self.initiative, &format!("{field_prefix}.initiative"))?;
        validate_guidance_option(&self.uncertainty, &format!("{field_prefix}.uncertainty"))?;
        Ok(())
    }
}
