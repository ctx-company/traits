//! Writes/refreshes the `[family]` table of a native trait family package's
//! root `trait.toml` (P530 Stage B): `default` plus one `[family.leaf.<selector>]`
//! per resolved leaf, carrying its generated path and legacy aliases.
//!
//! P531 Stage 1 adds the read side: [`read_family_table`] lets the
//! resolution seam ([`crate::run::try_resolve_trait_id`]) recognize a local
//! package as a native family and route `family:variant` straight to its
//! leaf's generated output, without a second writer-side format.

use std::collections::BTreeMap;

use camino::Utf8Path;

/// One leaf's generated-output path and legacy compatibility aliases, ready
/// to write into `[family.leaf.<selector>]`.
pub struct FamilyLeafManifestEntry {
    pub selector: String,
    pub relative_path: String,
    pub aliases: Vec<String>,
    pub run_config: Option<String>,
}

/// A parsed `[family]` table: the default leaf's selector plus every leaf's
/// generated-output path and legacy aliases, keyed by selector.
#[derive(Debug, Clone)]
pub struct FamilyTable {
    pub default: String,
    pub leaves: BTreeMap<String, FamilyLeaf>,
}

#[derive(Debug, Clone)]
pub struct FamilyLeaf {
    pub relative_path: String,
    pub aliases: Vec<String>,
    pub run_config: Option<String>,
}

impl FamilyTable {
    /// Resolve `variant` (already desugared: `"default"` means "use the
    /// family's declared default leaf") to its leaf entry.
    pub fn leaf_for_variant(&self, variant: &str) -> Option<(&str, &FamilyLeaf)> {
        let selector = if variant == "default" {
            self.default.as_str()
        } else {
            variant
        };
        self.leaves
            .get_key_value(selector)
            .map(|(selector, leaf)| (selector.as_str(), leaf))
    }

    /// Resolve a published legacy package alias to its selected leaf.
    pub fn leaf_for_alias(&self, alias: &str) -> Option<(&str, &FamilyLeaf)> {
        self.leaves
            .iter()
            .find(|(_, leaf)| leaf.aliases.iter().any(|candidate| candidate == alias))
            .map(|(selector, leaf)| (selector.as_str(), leaf))
    }

    /// Every leaf selector this family declares, for typo/variant
    /// diagnostics — `default` first (even when it aliases another
    /// selector), then the rest in table order.
    pub fn variant_names(&self) -> Vec<String> {
        let mut names = vec!["default".to_string()];
        for selector in self.leaves.keys() {
            if selector != &self.default {
                names.push(selector.clone());
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
    let mut leaves = BTreeMap::new();
    let leaf_table = family
        .get("leaf")
        .and_then(|item| item.as_table_like())
        .ok_or_else(|| crate::Error::Usage {
            message: format!("{manifest_path}: [family] is missing table `leaf`"),
        })?;
    for (selector, entry) in leaf_table.iter() {
        ctx_traits_core::shared::validate_slug_shape(selector, "family.leaf selector")
            .map_err(crate::Error::from)?;
        let entry = entry.as_table_like().ok_or_else(|| crate::Error::Usage {
            message: format!("{manifest_path}: [family.leaf.{selector}] must be a table"),
        })?;
        let relative_path = entry
            .get("path")
            .and_then(|item| item.as_str())
            .ok_or_else(|| crate::Error::Usage {
                message: format!(
                    "{manifest_path}: [family.leaf.{selector}] is missing string `path`"
                ),
            })?;
        validate_relative_path(manifest_path, selector, "path", relative_path)?;
        let aliases = entry
                .get("aliases")
                .and_then(|item| item.as_array())
                .map(|array| {
                    array
                        .iter()
                        .map(|item| item.as_str().map(str::to_string).ok_or_else(|| crate::Error::Usage {
                            message: format!("{manifest_path}: [family.leaf.{selector}].aliases must contain only strings"),
                        }))
                        .collect::<crate::Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
        for alias in &aliases {
            ctx_traits_core::shared::validate_slug_shape(alias, "family leaf alias")
                .map_err(crate::Error::from)?;
        }
        let run_config = entry
            .get("run-config")
            .and_then(|item| item.as_str())
            .map(str::to_string);
        if entry.get("run-config").is_some() && run_config.is_none() {
            return Err(crate::Error::Usage {
                message: format!(
                    "{manifest_path}: [family.leaf.{selector}].run-config must be a string"
                ),
            });
        }
        if let Some(run_config) = &run_config {
            validate_relative_path(manifest_path, selector, "run-config", run_config)?;
        }
        leaves.insert(
            selector.to_string(),
            FamilyLeaf {
                relative_path: relative_path.to_string(),
                aliases,
                run_config,
            },
        );
    }
    if !leaves.contains_key(default) {
        return Err(crate::Error::Usage {
            message: format!(
                "{manifest_path}: [family].default {default:?} does not name a declared leaf"
            ),
        });
    }
    Ok(Some(FamilyTable {
        default: default.to_string(),
        leaves,
    }))
}

fn validate_relative_path(
    manifest_path: &Utf8Path,
    selector: &str,
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
                "{manifest_path}: [family.leaf.{selector}].{field} must be a relative path within the package"
            ),
        });
    }
    Ok(())
}

/// Write/refresh the `[family]` table in `manifest_path`, preserving every
/// other table (`[package]`, `[dependencies]`) byte-for-byte via `toml_edit`.
pub fn write_family_table(
    manifest_path: &Utf8Path,
    default_selector: &str,
    leaves: &[FamilyLeafManifestEntry],
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
    document["family"]["default"] = toml_edit::value(default_selector);
    document["family"]["leaf"] = toml_edit::table();
    for leaf in leaves {
        document["family"]["leaf"][&leaf.selector] = toml_edit::table();
        document["family"]["leaf"][&leaf.selector]["path"] =
            toml_edit::value(leaf.relative_path.as_str());
        if !leaf.aliases.is_empty() {
            let mut aliases = toml_edit::Array::new();
            for alias in &leaf.aliases {
                aliases.push(alias.as_str());
            }
            document["family"]["leaf"][&leaf.selector]["aliases"] = toml_edit::value(aliases);
        }
        if let Some(run_config) = &leaf.run_config {
            document["family"]["leaf"][&leaf.selector]["run-config"] =
                toml_edit::value(run_config.as_str());
        }
    }
    crate::write::write_text(manifest_path, &document.to_string())?;
    Ok(())
}
