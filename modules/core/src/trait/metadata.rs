//! Metadata facets for search, browsing, dedupe, and inventory.
//!
//! Metadata supports search, browsing, dedupe, and inventory only. It does
//! not drive activation. Slugs are kebab-case strings validated in the
//! taxonomy validation pass via `validate_taxonomy`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::shared::{Slug, SlugList};

/// `[metadata]` facets: family, variant, job, domain, artifact, audience,
/// environment, tag.
///
/// All facets use kebab-case slug strings with scalar-or-array authoring.
/// Slug validation happens in the post-decode taxonomy pass. `family` and
/// `variant` are display-only: they never drive activation or canonical
/// trait resolution. Since P451, a declared `variant` additionally selects
/// which `[agent.variant.<v>.role.*]` config qualifier a run resolves
/// against (config routing, not activation or canonical resolution — see
/// `ctx_traits_io::harness_config::resolve_run_variant`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Metadata {
    /// Declared family identity for list grouping and consistency
    /// advisories against the trait package ID. Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<Slug>,
    /// Declared variant identity within `family`. Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<Slug>,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub job: SlugList,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub domain: SlugList,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub artifact: SlugList,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub audience: SlugList,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub environment: SlugList,
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub tag: SlugList,
}

impl Metadata {
    /// Validate every taxonomy slug with canonical field paths.
    ///
    /// Returns the first invalid slug's diagnostic, or `Ok(())` if all
    /// slugs are valid. Element indexes are included for list fields:
    /// `metadata.job[0]`, `metadata.domain[1]`, etc.
    pub fn validate_taxonomy(&self) -> crate::Result<()> {
        if let Some(ref family) = self.family {
            family.check("metadata.family")?;
        }
        if let Some(ref variant) = self.variant {
            variant.check("metadata.variant")?;
        }
        slug_list_check(&self.job, "metadata.job")?;
        slug_list_check(&self.domain, "metadata.domain")?;
        slug_list_check(&self.artifact, "metadata.artifact")?;
        slug_list_check(&self.audience, "metadata.audience")?;
        slug_list_check(&self.environment, "metadata.environment")?;
        slug_list_check(&self.tag, "metadata.tag")?;
        Ok(())
    }
}

fn slug_list_check(list: &SlugList, field_path: &str) -> crate::Result<()> {
    for (i, slug) in list.iter().enumerate() {
        slug.check(&format!("{field_path}[{i}]"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::encoding::{Encoding, decode_trait};
    use crate::shared::SlugList;

    use super::Metadata;

    const MINIMAL_TRAIT: &str = r#"
id = "golden-canonical"
schema-version = "0.2"
version = "0.1.0"
name = "Golden Canonical"
summary = "A minimal deterministic trait fixture for command-output comparisons."
status = "active"
trust = "local"
"#;

    #[test]
    fn decode_accepts_family_and_variant() {
        let text = format!("{MINIMAL_TRAIT}\n[metadata]\nfamily = \"plan\"\nvariant = \"quick\"\n");
        let decoded = decode_trait(Encoding::Toml, &text).expect("valid family/variant decode");
        let metadata = decoded.metadata.expect("metadata present");
        assert_eq!(metadata.family.as_ref().map(|s| s.as_str()), Some("plan"));
        assert_eq!(metadata.variant.as_ref().map(|s| s.as_str()), Some("quick"));
    }

    #[test]
    fn decode_rejects_invalid_family() {
        let text = format!("{MINIMAL_TRAIT}\n[metadata]\nfamily = \"Not_Valid\"\n");
        let err = decode_trait(Encoding::Toml, &text).expect_err("invalid family rejected");
        assert!(err.to_string().contains("metadata.family"));
    }

    #[test]
    fn decode_rejects_invalid_variant() {
        let text =
            format!("{MINIMAL_TRAIT}\n[metadata]\nfamily = \"plan\"\nvariant = \"Not_Valid\"\n");
        let err = decode_trait(Encoding::Toml, &text).expect_err("invalid variant rejected");
        assert!(err.to_string().contains("metadata.variant"));
    }

    #[test]
    fn legacy_metadata_serializes_without_family_or_variant() {
        let metadata = Metadata {
            job: SlugList::new(vec![crate::shared::Slug::raw("build")]),
            ..Metadata::default()
        };

        let toml = crate::encoding::encode(Encoding::Toml, &metadata).expect("toml encode");
        assert_eq!(toml, "job = [\"build\"]\n");
        assert!(!toml.contains("family"));
        assert!(!toml.contains("variant"));

        let json = crate::encoding::encode(Encoding::Json, &metadata).expect("json encode");
        assert_eq!(json, "{\n  \"job\": [\n    \"build\"\n  ]\n}");
        assert!(!json.contains("family"));
        assert!(!json.contains("variant"));
    }
}
