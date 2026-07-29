//! Shared rich guidance items for behavior and intent sections.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::shared::Slug;

/// A slug-identified guidance item with optional local render text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, derive_more::Display)]
#[display("{id}")]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct GuidanceItem {
    pub id: Slug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl GuidanceItem {
    pub fn from_slug(id: Slug) -> Self {
        Self {
            id,
            summary: None,
            description: None,
        }
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }
}

impl<'de> Deserialize<'de> for GuidanceItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawItem {
            Slug(Slug),
            Rich {
                id: Slug,
                #[serde(default)]
                summary: Option<String>,
                #[serde(default)]
                description: Option<String>,
            },
        }

        match RawItem::deserialize(deserializer)? {
            RawItem::Slug(id) => Ok(GuidanceItem::from_slug(id)),
            RawItem::Rich {
                id,
                summary,
                description,
            } => Ok(GuidanceItem {
                id,
                summary,
                description,
            }),
        }
    }
}

/// Scalar-or-array guidance list. Strings normalize to slug-only items.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
pub struct GuidanceItemList(Vec<GuidanceItem>);

impl GuidanceItemList {
    pub fn as_slice(&self) -> &[GuidanceItem] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, GuidanceItem> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for GuidanceItemList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GuidanceListVisitor;

        impl<'de> Visitor<'de> for GuidanceListVisitor {
            type Value = GuidanceItemList;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string, object, or array of strings/objects")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(GuidanceItemList(vec![GuidanceItem::from_slug(Slug::raw(
                    value.to_string(),
                ))]))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(GuidanceItemList(vec![GuidanceItem::from_slug(Slug::raw(
                    value,
                ))]))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let item = GuidanceItem::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(GuidanceItemList(vec![item]))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<GuidanceItem>()? {
                    items.push(item);
                }
                Ok(GuidanceItemList(items))
            }
        }

        deserializer.deserialize_any(GuidanceListVisitor)
    }
}

/// Decode an optional scalar guidance item from string or object authoring.
pub fn deserialize_optional_guidance_item<'de, D>(
    deserializer: D,
) -> Result<Option<GuidanceItem>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<GuidanceItem>::deserialize(deserializer)
}

pub fn validate_guidance_list(list: &GuidanceItemList, field_path: &str) -> crate::Result<()> {
    let mut seen = BTreeSet::new();
    for (i, item) in list.iter().enumerate() {
        validate_guidance_item(item, &format!("{field_path}[{i}]"))?;
        if !seen.insert(item.id.as_str().to_string()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{i}].id"),
                message: format!("duplicate guidance id {:?}", item.id.as_str()),
            }
            .into());
        }
    }
    Ok(())
}

pub fn validate_guidance_option(
    item: &Option<GuidanceItem>,
    field_path: &str,
) -> crate::Result<()> {
    if let Some(item) = item {
        validate_guidance_item(item, field_path)?;
    }
    Ok(())
}

fn validate_guidance_item(item: &GuidanceItem, field_path: &str) -> crate::Result<()> {
    item.id.check(&format!("{field_path}.id"))?;
    if item
        .summary
        .as_deref()
        .is_some_and(|text| text.trim().is_empty())
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.summary"),
            message: "must not be empty when present".to_string(),
        }
        .into());
    }
    if item
        .description
        .as_deref()
        .is_some_and(|text| text.trim().is_empty())
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}.description"),
            message: "must not be empty when present".to_string(),
        }
        .into());
    }
    for (name, value) in [
        ("summary", item.summary.as_deref()),
        ("description", item.description.as_deref()),
    ] {
        if value.is_some_and(contains_second_person_guidance) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.{name}"),
                message: "must use imperative, second-personless guidance".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

fn contains_second_person_guidance(value: &str) -> bool {
    const FORMS: &[&str] = &[
        "you",
        "your",
        "yours",
        "yourself",
        "yourselves",
        "you're",
        "you've",
        "you'll",
        "you'd",
    ];
    value
        .split(|character: char| !character.is_ascii_alphabetic() && character != '\'')
        .any(|word| FORMS.iter().any(|form| word.eq_ignore_ascii_case(form)))
}
