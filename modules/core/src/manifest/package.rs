//! Package manifest: the root `trait.toml` of a trait package.
//!
//! Unlike the canonical trait document (`generated/index.toml`), the package
//! manifest is Cargo-like package metadata: identity under `[package]` and
//! fetchable dependency sources under `[dependencies]`. It never carries
//! trait behavior. Detection is structural: a root `trait.toml` containing a
//! `[package]` table is a package manifest; without one it is a legacy flat
//! canonical trait document.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::dependency::Dependency;
use super::source::TraitSource;

/// The root `trait.toml` package manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageManifest {
    pub package: PackageMetadata,

    /// Native family topology. Absent for ordinary packages and therefore
    /// byte-compatible with the pre-family manifest format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<PackageFamily>,

    /// `[dependencies]` table keyed by alias: `alias = { path = ".." }`,
    /// `alias = { npm = "@scope/pkg" }`, or `alias = { git = "..." }`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, PackageDependencySpec>,

    /// Where and under what name this package publishes. Absent for a package
    /// that never publishes, and therefore byte-compatible with every existing
    /// manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PackagePublish>,

    /// npm dependencies the package's own `source/` authoring code needs at
    /// build/type-check time — copied verbatim into the generated
    /// `package.json`'s `dependencies`. Table shape only; pinning semantics
    /// are task 0170's territory.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub authoring_dependencies: BTreeMap<String, String>,
}

/// Publication identity: the npm name, registry, and access this package is
/// published under.
///
/// Without this the name was derived as `@ctx-traits/<id>` — a scope only we
/// can publish to — so every third-party package was unpublishable by
/// construction. `id` stays the resolution identity inside ctx; this is the
/// separate question of what the outside world calls it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackagePublish {
    /// npm package name, scope included. Defaults to `@ctx-traits/<id>` only
    /// because that is this repository's own scope; every other publisher must
    /// state theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Registry URL. Absent means npm's default, or whatever the publishing
    /// environment already points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

    /// npm `--access`: `public` or `restricted`. Absent leaves it to npm,
    /// which defaults a NEW scoped package to restricted — the reason a first
    /// publish silently lands private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,

    /// Paths excluded from the published payload, replacing the built-in
    /// defaults when stated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// The package-level name map for a native trait family.
///
/// `variant` accepts the legacy `leaf` key too (compat read for packages
/// still on the pre-rename manifest shape); a manifest declaring both is a
/// duplicate-field error rather than a silent preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageFamily {
    pub default: String,
    #[serde(alias = "leaf")]
    pub variant: BTreeMap<String, PackageFamilyVariant>,
}

/// One native family variant's generated path and compatibility selectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageFamilyVariant {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_config: Option<String>,
}

/// `[package]` identity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageMetadata {
    pub id: String,
    pub version: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Team-edited lifecycle status: `draft` (default) or `ready`.
    ///
    /// This is the sole owner of trait lifecycle status. The canonical trait
    /// document never carries a status field (Group 95, 2026-07-19); trust is
    /// a separate machine-local concept keyed by canonical digest in
    /// `~/.config/ctx/trust.toml`, not a manifest field.
    #[serde(default)]
    pub status: PackageStatus,
}

/// Package manifest lifecycle status: `draft | ready`.
///
/// Owned exclusively by `[package].status` in `trait.toml`. Activation and
/// check gates read this field; they never read a status field from the
/// canonical trait document, which does not have one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PackageStatus {
    #[default]
    Draft,
    Ready,
}

impl PackageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
        }
    }

    /// User-facing display name (same as `as_str`; kept for parity with
    /// other lifecycle/trust display helpers).
    pub fn display_name(self) -> &'static str {
        self.as_str()
    }
}

impl std::fmt::Display for PackageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `[dependencies]` entry in authoring shape.
///
/// Exactly one of `path`, `npm`, or `git` selects the source kind. The
/// depended-on trait id defaults to the entry's alias key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageDependencySpec {
    pub version: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,

    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_path: Option<String>,
}

impl PackageManifest {
    /// Normalize `[dependencies]` entries into dependency declarations.
    pub fn dependencies(&self) -> crate::Result<Vec<Dependency>> {
        self.dependencies
            .iter()
            .map(|(alias, spec)| spec.normalize(alias))
            .collect()
    }
}

impl PackageDependencySpec {
    fn normalize(&self, alias: &str) -> crate::Result<Dependency> {
        let field_path = format!("dependencies.{alias}");
        let source = match (&self.path, &self.npm, &self.git) {
            (Some(path), None, None) => {
                if self.package_path.is_some() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.package-path"),
                        message: "package-path applies to npm and git sources only".to_string(),
                    }
                    .into());
                }
                TraitSource::Local { path: path.clone() }
            }
            (None, Some(package), None) => TraitSource::Npm {
                package: package.clone(),
                package_path: self.package_path.clone(),
            },
            (None, None, Some(url)) => TraitSource::Git {
                url: url.clone(),
                requested_ref: self.git_ref.clone(),
                package_path: self.package_path.clone(),
            },
            _ => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path,
                    message: "declare exactly one of path, npm, or git".to_string(),
                }
                .into());
            }
        };
        if self.git_ref.is_some() && self.git.is_none() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.ref"),
                message: "ref applies to git sources only".to_string(),
            }
            .into());
        }
        Ok(Dependency {
            alias: alias.to_string(),
            id: self.id.clone().unwrap_or_else(|| alias.to_string()),
            version: self.version.clone(),
            source: Some(source),
        })
    }
}

/// Decode a package manifest from root `trait.toml` text.
///
/// Returns `Ok(None)` when the document has no `[package]` table — the
/// structural marker distinguishing a package manifest from a legacy flat
/// canonical trait document sharing the same file name.
pub fn decode_package_manifest(text: &str, origin: &str) -> crate::Result<Option<PackageManifest>> {
    let probe: toml::Value =
        toml::from_str(text).map_err(|error| crate::manifest::Error::InvalidField {
            field_path: origin.to_string(),
            message: format!("invalid TOML: {error}"),
        })?;
    if probe.get("package").is_none() {
        return Ok(None);
    }
    let manifest: PackageManifest =
        toml::from_str(text).map_err(|error| crate::manifest::Error::InvalidField {
            field_path: origin.to_string(),
            message: format!("invalid package manifest: {error}"),
        })?;
    if manifest.package.id.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "package.id".to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }
    if manifest.package.version.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "package.version".to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }
    super::dependency::validate_dependencies(&manifest.dependencies()?)?;
    if let Some(family) = &manifest.family {
        if !family.variant.contains_key(&family.default) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "family.default".to_string(),
                message: "must name a declared family.variant".to_string(),
            }
            .into());
        }
        let mut names = std::collections::BTreeSet::new();
        for (name, variant) in &family.variant {
            crate::shared::validate_slug_shape(name, &format!("family.variant.{name}"))?;
            names.insert(name);
            validate_family_relative_path(&variant.path, &format!("family.variant.{name}.path"))?;
            if let Some(config) = &variant.run_config {
                validate_family_relative_path(
                    config,
                    &format!("family.variant.{name}.run-config"),
                )?;
            }
            for alias in &variant.aliases {
                crate::shared::validate_slug_shape(
                    alias,
                    &format!("family.variant.{name}.aliases"),
                )?;
                if !names.insert(alias) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("family.variant.{name}.aliases"),
                        message: format!("duplicate family name {alias:?}"),
                    }
                    .into());
                }
            }
        }
    }
    Ok(Some(manifest))
}

fn validate_family_relative_path(path: &str, field_path: &str) -> crate::Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "must be a confined non-empty relative path".to_string(),
        }
        .into());
    }
    Ok(())
}
