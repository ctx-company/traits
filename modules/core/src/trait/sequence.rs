//! Named reusable procedure sequences.
//!
//! `[sequence.<id>]` declares a local sequence block that control items can
//! reference with `sequence:<id>`. The declarations are pure authoring/runtime
//! structure; execution still happens through `procedure_runtime`.

use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::r#trait::procedure::SequenceItem;

/// A named sequence block keyed by `[sequence.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct NamedSequence {
    /// Human-readable description of the sequence block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Ordered items in this sequence block.
    #[serde(default, rename = "sequence", skip_serializing_if = "Vec::is_empty")]
    pub sequence: Vec<SequenceItem>,
}

/// Duplicate-aware map of named sequences keyed by sequence ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct NamedSequenceMap(BTreeMap<String, NamedSequence>);

impl NamedSequenceMap {
    pub fn get(&self, key: &str) -> Option<&NamedSequence> {
        self.0.get(key)
    }

    pub fn keys(&self) -> std::collections::btree_map::Keys<'_, String, NamedSequence> {
        self.0.keys()
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, NamedSequence> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for NamedSequenceMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NamedSequenceMapVisitor;

        impl<'de> Visitor<'de> for NamedSequenceMapVisitor {
            type Value = NamedSequenceMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a sequence object/map keyed by sequence id")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut map = BTreeMap::new();
                while let Some((id, body)) = access.next_entry::<String, NamedSequence>()? {
                    if map.contains_key(&id) {
                        return Err(de::Error::custom(format!(
                            "duplicate sequence key at sequence.{id}"
                        )));
                    }
                    map.insert(id, body);
                }
                Ok(NamedSequenceMap(map))
            }
        }

        deserializer.deserialize_map(NamedSequenceMapVisitor)
    }
}
