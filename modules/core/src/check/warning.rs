use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Section;

/// A structured advisory warning surfaced by `ctx traits check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CheckWarning {
    /// Report section that produced this warning.
    pub section: Section,
    /// Stable warning code.
    pub code: String,
    /// Field path or resource/ref evidence when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Human-readable warning message.
    pub message: String,
}

/// Build the deterministic portability advisory for each `root = "repo"`
/// resource declaration.
///
/// Repo-root resources resolve against the invocation repository rather than
/// the trait package, so they are not portable when the trait package is
/// copied, vendored, or moved to another checkout. This is advisory only: it
/// never changes pass/fail status.
pub fn resource_root_advisories(resources: &[crate::r#trait::Resource]) -> Vec<CheckWarning> {
    resources
        .iter()
        .filter(|resource| resource.effective_root() == crate::r#trait::ResourceRoot::Repo)
        .map(|resource| CheckWarning {
            section: Section::Resources,
            code: "resource-root-repo-coupled".to_string(),
            field: Some(format!("resource.{}.root", resource.id)),
            message: format!(
                "resource {:?} declares root = \"repo\": repo-coupled, not portable across checkouts",
                resource.id
            ),
        })
        .collect()
}

/// Build advisory warnings comparing declared `[metadata] family`/`variant`
/// identity against the trait package ID.
///
/// Display-only consistency check: mismatches never affect activation,
/// resolution, or pass/fail status. Missing metadata, or an empty family
/// and variant, produces no warnings.
pub fn family_variant_advisories(
    trait_id: &str,
    metadata: Option<&crate::r#trait::Metadata>,
) -> Vec<CheckWarning> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let family = metadata.family.as_ref().map(|slug| slug.as_str());
    let variant = metadata.variant.as_ref().map(|slug| slug.as_str());

    let mut warnings = Vec::new();

    let Some(family) = family else {
        if variant.is_some() {
            warnings.push(CheckWarning {
                section: Section::Metadata,
                code: "metadata-variant-without-family".to_string(),
                field: Some("metadata.variant".to_string()),
                message: "metadata.variant is declared without metadata.family".to_string(),
            });
        }
        return warnings;
    };

    let matches = match variant {
        Some("default") => trait_id == family || trait_id == format!("{family}-default"),
        Some(v) => trait_id == format!("{family}-{v}"),
        None => trait_id == family || trait_id.starts_with(&format!("{family}-")),
    };

    if !matches {
        let expected = match variant {
            Some("default") => format!("{family} or {family}-default"),
            Some(v) => format!("{family}-{v}"),
            None => format!("{family} or {family}-*"),
        };
        warnings.push(CheckWarning {
            section: Section::Metadata,
            code: "metadata-family-id-mismatch".to_string(),
            field: Some("metadata.family".to_string()),
            message: format!(
                "declared identity family={family:?} variant={variant:?} expects trait ID {expected:?}, got {trait_id:?}"
            ),
        });
    }

    warnings
}

#[cfg(test)]
mod tests {
    use crate::shared::Slug;
    use crate::r#trait::Metadata;

    use super::{Section, family_variant_advisories};

    fn metadata(family: Option<&str>, variant: Option<&str>) -> Metadata {
        Metadata {
            family: family.map(Slug::raw),
            variant: variant.map(Slug::raw),
            ..Metadata::default()
        }
    }

    #[test]
    fn no_metadata_produces_no_warnings() {
        assert!(family_variant_advisories("plan", None).is_empty());
    }

    #[test]
    fn empty_metadata_produces_no_warnings() {
        let m = metadata(None, None);
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
    }

    #[test]
    fn variant_without_family_warns() {
        let m = metadata(None, Some("quick"));
        let warnings = family_variant_advisories("plan-quick", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].section, Section::Metadata);
        assert_eq!(warnings[0].code, "metadata-variant-without-family");
        assert_eq!(warnings[0].field.as_deref(), Some("metadata.variant"));
    }

    #[test]
    fn family_only_matching_bare_id_is_clean() {
        let m = metadata(Some("plan"), None);
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
    }

    #[test]
    fn family_only_matching_prefixed_id_is_clean() {
        let m = metadata(Some("plan"), None);
        assert!(family_variant_advisories("plan-quick", Some(&m)).is_empty());
    }

    #[test]
    fn family_only_mismatching_id_warns() {
        let m = metadata(Some("plan"), None);
        let warnings = family_variant_advisories("refactor", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].section, Section::Metadata);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
        assert_eq!(warnings[0].field.as_deref(), Some("metadata.family"));
        assert!(warnings[0].message.contains("plan"));
        assert!(warnings[0].message.contains("refactor"));
    }

    #[test]
    fn matching_non_default_variant_is_clean() {
        let m = metadata(Some("plan"), Some("quick"));
        assert!(family_variant_advisories("plan-quick", Some(&m)).is_empty());
    }

    #[test]
    fn mismatching_non_default_variant_warns() {
        let m = metadata(Some("plan"), Some("quick"));
        let warnings = family_variant_advisories("plan", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn default_variant_accepts_bare_or_family_default_id() {
        let m = metadata(Some("plan"), Some("default"));
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
        assert!(family_variant_advisories("plan-default", Some(&m)).is_empty());

        let warnings = family_variant_advisories("plan-other", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn hyphenated_family_matches_full_string() {
        let m = metadata(Some("refactor-quick"), None);
        assert!(family_variant_advisories("refactor-quick", Some(&m)).is_empty());
        assert!(family_variant_advisories("refactor-quick-style", Some(&m)).is_empty());

        let warnings = family_variant_advisories("refactor", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn hyphenated_family_with_variant_matches_full_string() {
        let m = metadata(Some("refactor-quick"), Some("style"));
        assert!(family_variant_advisories("refactor-quick-style", Some(&m)).is_empty());

        let warnings = family_variant_advisories("refactor-quick", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }
}
