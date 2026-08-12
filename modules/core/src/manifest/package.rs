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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageManifest {
    pub package: PackageMetadata,

    /// Native family topology. Absent for ordinary packages and therefore
    /// byte-compatible with the pre-family manifest format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<PackageFamily>,

    /// `[budget]`: the author's default execution envelope (0176). Budgets
    /// are DATA, not behavior — they live outside every digest, so retuning
    /// one never forces re-trust (the same boundary 0172 drew for resolved
    /// setting values). For a family this is the default variant's budget;
    /// `[variant.<vid>.budget]` overlays it per non-default variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<ManifestBudget>,

    /// `[variant.<vid>]`: per-variant author config, budget-only (0176).
    /// Deliberately a sibling of `[family]`, never inside it — the build
    /// regenerates `[family]` wholesale, while this table is hand-authored
    /// and must survive a rebuild untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variant: BTreeMap<String, PackageVariantConfig>,

    /// `[defaults.port]`: author-supplied caller-arg fallbacks (0176).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<ManifestDefaults>,

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
}

/// One `[variant.<vid>]` table: the author's per-variant config. Accepts
/// `budget` and nothing else — **permission narrows as authority moves away
/// from the machine owner**; do not widen this for symmetry with the machine
/// tier's variant tables.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageVariantConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<ManifestBudget>,
}

/// `[defaults]`: author-supplied input fallbacks. Accepts `port` and nothing
/// else (same narrowing rule as [`PackageVariantConfig`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ManifestDefaults {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub port: BTreeMap<String, String>,
}

/// The author's execution-envelope statement: the engine-defined budget
/// vocabulary, every field optional (stated replaces, omitted inherits).
/// Mirrors the io-layer run budget field-for-field; the io layer converts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ManifestBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_wait_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_idle_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
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
            digest: None,
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
    for name in manifest.variant.keys() {
        crate::shared::validate_slug_shape(name, &format!("variant.{name}"))?;
        let Some(family) = &manifest.family else {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("variant.{name}"),
                message: "per-variant config requires a [family]; a single-trait package \
                          states its budget under [budget]"
                    .to_string(),
            }
            .into());
        };
        if !family.variant.contains_key(name) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("variant.{name}"),
                message: format!(
                    "does not name a declared family.variant (declared: {})",
                    family
                        .variant
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
            .into());
        }
        if name == &family.default {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("variant.{name}"),
                message: format!(
                    "names the family default {name:?}; the default variant's budget is \
                     the top-level [budget]"
                ),
            }
            .into());
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

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY_HEADER: &str = concat!(
        "[package]\n",
        "id = \"fixture\"\n",
        "version = \"0.1.0\"\n",
        "\n",
        "[family]\n",
        "default = \"quick\"\n",
        "[family.variant.quick]\n",
        "path = \"generated/quick/index.toml\"\n",
        "[family.variant.gated]\n",
        "path = \"generated/gated/index.toml\"\n",
    );

    fn decode(text: &str) -> crate::Result<Option<PackageManifest>> {
        decode_package_manifest(text, "trait.toml")
    }

    #[test]
    fn budget_and_variant_overlay_and_port_defaults_decode() {
        let text = format!(
            "{FAMILY_HEADER}\n[budget]\nframe-seconds = 1200\nmax-cost-usd = 2.5\n\n\
             [variant.gated.budget]\ntotal-seconds = 86400\n\n[defaults.port]\nplan = \"x\"\n"
        );
        let manifest = decode(&text).expect("decodes").expect("is a manifest");
        let budget = manifest.budget.expect("budget stated");
        assert_eq!(budget.frame_seconds, Some(1200));
        assert_eq!(budget.max_cost_usd, Some(2.5));
        assert_eq!(
            manifest.variant["gated"]
                .budget
                .as_ref()
                .and_then(|b| b.total_seconds),
            Some(86400)
        );
        assert_eq!(
            manifest.defaults.expect("defaults stated").port["plan"],
            "x"
        );
    }

    #[test]
    fn budget_rejects_out_of_scope_keys() {
        for section in [
            "[budget]\nharness = \"opencode\"\n",
            "[variant.gated]\nmodel = \"x\"\n",
            "[variant.gated.budget]\nworktree = true\n",
            "[defaults]\nsetting = { a = 1 }\n",
        ] {
            let text = format!("{FAMILY_HEADER}\n{section}");
            assert!(
                decode(&text).is_err(),
                "out-of-scope key must refuse: {section}"
            );
        }
    }

    #[test]
    fn variant_overlay_for_an_undeclared_variant_refuses_naming_the_declared_set() {
        let text = format!("{FAMILY_HEADER}\n[variant.turbo.budget]\nframe-seconds = 1\n");
        let error = decode(&text).expect_err("unknown variant refuses");
        let message = format!("{error:#}");
        assert!(message.contains("variant.turbo"), "{message}");
        assert!(message.contains("gated, quick"), "{message}");
    }

    #[test]
    fn variant_overlay_for_the_family_default_refuses() {
        let text = format!("{FAMILY_HEADER}\n[variant.quick.budget]\nframe-seconds = 1\n");
        let error = decode(&text).expect_err("default-variant overlay refuses");
        assert!(format!("{error:#}").contains("top-level [budget]"));
    }

    #[test]
    fn variant_overlay_without_a_family_refuses() {
        let text = "[package]\nid = \"fixture\"\nversion = \"0.1.0\"\n\n\
                    [variant.quick.budget]\nframe-seconds = 1\n";
        let error = decode(text).expect_err("no-family overlay refuses");
        assert!(format!("{error:#}").contains("requires a [family]"));
    }
}
