//! Writes/refreshes the `[family]` table of a native trait family package's
//! root `trait.toml` (P530 Stage B): `default` plus one
//! `[family.variant.<name>]` per resolved variant, carrying its generated
//! path and legacy aliases.
//!
//! P531 Stage 1 adds the read side: [`read_family_table`] lets the
//! resolution seam ([`crate::run::try_resolve_trait_id`]) recognize a local
//! package as a native family and route `family:variant` straight to its
//! variant's generated output, without a second writer-side format.

use std::collections::BTreeMap;

use camino::Utf8Path;

/// One variant's generated-output path and legacy compatibility aliases,
/// ready to write into `[family.variant.<name>]`.
pub struct FamilyVariantManifestEntry {
    pub name: String,
    pub relative_path: String,
    pub aliases: Vec<String>,
    pub run_config: Option<String>,
}

/// A parsed `[family]` table: the default variant's name plus every
/// variant's generated-output path and legacy aliases, keyed by name.
#[derive(Debug, Clone)]
pub struct FamilyTable {
    pub default: String,
    pub variants: BTreeMap<String, FamilyVariant>,
}

#[derive(Debug, Clone)]
pub struct FamilyVariant {
    pub relative_path: String,
    pub aliases: Vec<String>,
    pub run_config: Option<String>,
}

impl FamilyTable {
    /// Resolve `variant` (already desugared: `"default"` means "use the
    /// family's declared default variant") to its entry.
    pub fn variant(&self, variant: &str) -> Option<(&str, &FamilyVariant)> {
        let name = if variant == "default" {
            self.default.as_str()
        } else {
            variant
        };
        self.variants
            .get_key_value(name)
            .map(|(name, variant)| (name.as_str(), variant))
    }

    /// Resolve a published legacy package alias to its selected variant.
    pub fn variant_for_alias(&self, alias: &str) -> Option<(&str, &FamilyVariant)> {
        self.variants
            .iter()
            .find(|(_, variant)| variant.aliases.iter().any(|candidate| candidate == alias))
            .map(|(name, variant)| (name.as_str(), variant))
    }

    /// Every variant name this family declares, for typo/variant
    /// diagnostics — `default` first (even when it aliases another
    /// name), then the rest in table order.
    pub fn variant_names(&self) -> Vec<String> {
        let mut names = vec!["default".to_string()];
        for name in self.variants.keys() {
            if name != &self.default {
                names.push(name.clone());
            }
        }
        names
    }
}

/// Read the `[family]` table from a package's root `trait.toml`, if present.
///
/// Returns `Ok(None)` only when the manifest does not exist or has no
/// `[family]` table. A present family table is a package contract: malformed
/// entries fail visibly rather than silently falling through to legacy lookup.
///
/// Accepts either the current `[family.variant.<name>]` shape or the legacy
/// `[family.leaf.<selector>]` shape (compat read); a manifest declaring both
/// is ambiguous and rejected outright rather than silently preferring one.
pub fn read_family_table(manifest_path: &Utf8Path) -> crate::Result<Option<FamilyTable>> {
    let Some(text) = crate::read::read_optional_text(manifest_path)? else {
        return Ok(None);
    };
    let document = text.parse::<toml_edit::DocumentMut>().map_err(|source| {
        crate::parse::Error::TomlEditDecode {
            context: format!("parse {manifest_path} for family resolution"),
            source: Box::new(source),
        }
    })?;
    let Some(family) = document.get("family").and_then(|item| item.as_table_like()) else {
        return Ok(None);
    };
    let default = family
        .get("default")
        .and_then(|item| item.as_str())
        .ok_or_else(|| crate::Error::Usage {
            message: format!("{manifest_path}: [family] is missing string `default`"),
        })?;
    let variant_table = family.get("variant").and_then(|item| item.as_table_like());
    let legacy_leaf_table = family.get("leaf").and_then(|item| item.as_table_like());
    let (label, member_table) = match (variant_table, legacy_leaf_table) {
        (Some(_), Some(_)) => {
            return Err(crate::Error::Usage {
                message: format!(
                    "{manifest_path}: [family] declares both `variant` and legacy `leaf` tables; \
                     remove one"
                ),
            });
        }
        (Some(table), None) => ("variant", table),
        (None, Some(table)) => ("leaf", table),
        (None, None) => {
            return Err(crate::Error::Usage {
                message: format!("{manifest_path}: [family] is missing table `variant`"),
            });
        }
    };
    let mut variants = BTreeMap::new();
    for (name, entry) in member_table.iter() {
        ctx_traits_core::shared::validate_slug_shape(name, "family.variant name")
            .map_err(crate::Error::from)?;
        let entry = entry.as_table_like().ok_or_else(|| crate::Error::Usage {
            message: format!("{manifest_path}: [family.{label}.{name}] must be a table"),
        })?;
        let relative_path = entry
            .get("path")
            .and_then(|item| item.as_str())
            .ok_or_else(|| crate::Error::Usage {
                message: format!(
                    "{manifest_path}: [family.{label}.{name}] is missing string `path`"
                ),
            })?;
        validate_relative_path(manifest_path, label, name, "path", relative_path)?;
        let aliases = entry
                .get("aliases")
                .and_then(|item| item.as_array())
                .map(|array| {
                    array
                        .iter()
                        .map(|item| item.as_str().map(str::to_string).ok_or_else(|| crate::Error::Usage {
                            message: format!("{manifest_path}: [family.{label}.{name}].aliases must contain only strings"),
                        }))
                        .collect::<crate::Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
        for alias in &aliases {
            ctx_traits_core::shared::validate_slug_shape(alias, "family variant alias")
                .map_err(crate::Error::from)?;
        }
        let run_config = entry
            .get("run-config")
            .and_then(|item| item.as_str())
            .map(str::to_string);
        if entry.get("run-config").is_some() && run_config.is_none() {
            return Err(crate::Error::Usage {
                message: format!(
                    "{manifest_path}: [family.{label}.{name}].run-config must be a string"
                ),
            });
        }
        if let Some(run_config) = &run_config {
            validate_relative_path(manifest_path, label, name, "run-config", run_config)?;
        }
        variants.insert(
            name.to_string(),
            FamilyVariant {
                relative_path: relative_path.to_string(),
                aliases,
                run_config,
            },
        );
    }
    if !variants.contains_key(default) {
        return Err(crate::Error::Usage {
            message: format!(
                "{manifest_path}: [family].default {default:?} does not name a declared variant"
            ),
        });
    }
    Ok(Some(FamilyTable {
        default: default.to_string(),
        variants,
    }))
}

fn validate_relative_path(
    manifest_path: &Utf8Path,
    label: &str,
    name: &str,
    field: &str,
    value: &str,
) -> crate::Result<()> {
    let path = Utf8Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, camino::Utf8Component::ParentDir))
    {
        return Err(crate::Error::Usage {
            message: format!(
                "{manifest_path}: [family.{label}.{name}].{field} must be a relative path within the package"
            ),
        });
    }
    Ok(())
}

/// Write/refresh the `[family]` table in `manifest_path`, preserving every
/// other table (`[package]`, `[dependencies]`) byte-for-byte via `toml_edit`.
///
/// Always emits `[family.variant.<name>]`; a legacy `[family.leaf.*]` table
/// on disk is dropped once this rewrites the document, migrating the store
/// to the new shape on next publish.
pub fn write_family_table(
    manifest_path: &Utf8Path,
    default_name: &str,
    variants: &[FamilyVariantManifestEntry],
) -> crate::Result<()> {
    let existing = crate::read::read_optional_text(manifest_path)?;
    let text = existing.unwrap_or_default();
    let mut document = text.parse::<toml_edit::DocumentMut>().map_err(|source| {
        crate::parse::Error::TomlEditDecode {
            context: format!("parse {manifest_path} for family publish"),
            source: Box::new(source),
        }
    })?;
    document["family"] = toml_edit::table();
    document["family"]["default"] = toml_edit::value(default_name);
    document["family"]["variant"] = toml_edit::table();
    for variant in variants {
        document["family"]["variant"][&variant.name] = toml_edit::table();
        document["family"]["variant"][&variant.name]["path"] =
            toml_edit::value(variant.relative_path.as_str());
        if !variant.aliases.is_empty() {
            let mut aliases = toml_edit::Array::new();
            for alias in &variant.aliases {
                aliases.push(alias.as_str());
            }
            document["family"]["variant"][&variant.name]["aliases"] = toml_edit::value(aliases);
        }
        if let Some(run_config) = &variant.run_config {
            document["family"]["variant"][&variant.name]["run-config"] =
                toml_edit::value(run_config.as_str());
        }
    }
    crate::write::write_text(manifest_path, &document.to_string())?;
    Ok(())
}
