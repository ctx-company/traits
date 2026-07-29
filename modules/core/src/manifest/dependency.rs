//! Trait dependency declarations.
//!
//! A `[[dependency]]` section declares a dependency on another trait package.
//! The `alias` field establishes a typed reference namespace: refs such as
//! `prompt:aws/prepare-query` use `aws` as the alias qualifying the dependency's
//! definitions.
//!
//! MVP dependency fields are `alias`, `id`, `version`, and optional `source`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::source::TraitSource;

/// A dependency declaration: alias, trait identity, version, and optional
/// source metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct Dependency {
    /// Typed reference namespace for this dependency (e.g. `"aws"`).
    ///
    /// Must be unique within the manifest. Used to qualify refs such as
    /// `slot:aws/scope`. Must be a single-segment namespace without `:`, `.`,
    /// `/`, or whitespace.
    pub alias: String,

    /// The trait ID being depended on (e.g. `"ctx/aws-log-primitives"`).
    pub id: String,

    /// The trait version (e.g. `"0.1.0"`).
    pub version: String,

    /// Optional source metadata. When omitted, the source must be resolved
    /// from the package-local `trait.lock` or provided explicitly by the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub source: Option<TraitSource>,
}

/// Validate a list of dependencies.
///
/// Checks for:
/// - Empty or invalid `alias` (must be a valid typed-ref namespace segment)
/// - Empty `id` or `version`
/// - Duplicate aliases
///
/// Returns the first error found, or `Ok(())` if all dependencies are valid.
/// All diagnostics use field-specific paths such as `dependency[0].alias`.
pub fn validate_dependencies(deps: &[Dependency]) -> crate::Result<()> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();

    for (i, dep) in deps.iter().enumerate() {
        validate_alias(&dep.alias, i)?;

        if dep.id.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("dependency[{i}].id"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
        if dep.version.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("dependency[{i}].version"),
                message: "must not be empty".to_string(),
            }
            .into());
        }

        if let Some(&first_idx) = seen.get(&dep.alias) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("dependency[{i}].alias"),
                message: format!(
                    "duplicate alias {:?} (first declared at dependency[{first_idx}].alias)",
                    dep.alias
                ),
            }
            .into());
        }
        seen.insert(dep.alias.clone(), i);
    }

    Ok(())
}

/// Validate that an alias is a valid typed-reference namespace segment.
///
/// Uses the shared kebab-case slug validator ([`validate_slug_shape`]) so
/// alias grammar is consistent across manifest declarations and dependency
/// handles. Rejects empty, uppercase, underscore, dotted, colon, slash,
/// whitespace, leading/trailing hyphen, and repeated-hyphen aliases.
///
/// [`validate_slug_shape`]: crate::shared::validate_slug_shape
fn validate_alias(alias: &str, index: usize) -> crate::Result<()> {
    let field_path = format!("dependency[{index}].alias");
    if alias == "procedure" {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: "reserved for virtual procedure-step sourcemap refs".to_string(),
        }
        .into());
    }
    crate::shared::validate_slug_shape(alias, &field_path)
}
