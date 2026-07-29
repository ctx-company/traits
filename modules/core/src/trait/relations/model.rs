// Trait relation model definitions.
/// Defines trait relation model data.
/// Trait relation model definitions.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::reference::{Kind, Reference};
use crate::r#trait::Trait;

// ===========================================================================
// Model data model
// ===========================================================================

crate::shared::reference_list_wrapper! {
    /// Scalar-or-array typed reference list for `when` conditions.
    #[schemars(rename = "RelationsRefList")]
    #[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
    pub struct RefList
}

/// A grouped relation endpoint for consumer/provider port evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PortEndpoint {
    pub trait_id: String,
    pub port_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_ref: Option<Reference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<Reference>,
}

impl PortEndpoint {
    pub fn new(trait_id: &str, port_id: &str) -> Self {
        Self {
            trait_id: trait_id.to_string(),
            port_id: port_id.to_string(),
            port_ref: None,
            digest: None,
            schema_ref: None,
        }
    }

    pub fn with_port_ref(mut self, port_ref: Reference) -> Self {
        self.port_ref = Some(port_ref);
        self
    }

    pub fn with_digest(mut self, digest: Option<Digest>) -> Self {
        self.digest = digest;
        self
    }

    pub fn with_schema_ref(mut self, schema_ref: Reference) -> Self {
        self.schema_ref = Some(schema_ref);
        self
    }
}

/// A `[[relations.requires]]` entry: the source needs the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Requires {
    /// Required target: `port:*` or `trait:*`.
    pub target: Reference,

    /// Conditions that enable this relation: `rule:*` or `signal:*` refs.
    /// Entries in one `when` list are AND; multiple entries model OR.
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub when: RefList,

    /// Required explanation.
    pub reason: String,
}

/// A `[[relations.suggests]]` entry: advisory influence toward target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Suggests {
    /// Suggested target: `trait:*` or `port:*`.
    pub target: Reference,

    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub when: RefList,

    pub reason: String,
}

/// A `[[relations.conflicts]]` entry: targetless — applies to the declaring
/// trait when `when` matches. Rejects `target`, `required`, `suggested`,
/// `conflicting`, `strength`, `max-depth`, `follows`, and `suppresses`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Conflicts {
    #[serde(default, skip_serializing_if = "RefList::is_empty")]
    pub when: RefList,

    pub reason: String,
}

/// The optional `[relations]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case", rename = "Relations")]
pub struct Model {
    #[serde(default, rename = "requires", skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Requires>,

    #[serde(default, rename = "suggests", skip_serializing_if = "Vec::is_empty")]
    pub suggests: Vec<Suggests>,

    #[serde(default, rename = "conflicts", skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflicts>,
}

impl Model {
    pub fn is_empty(&self) -> bool {
        self.requires.is_empty() && self.suggests.is_empty() && self.conflicts.is_empty()
    }
}
