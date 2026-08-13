//! Project manifest structs: schema version, project metadata, default
//! targets, and trait dependency entries.
//!
//! Domain structs are the primary API. Scalar-or-array authoring sugar is
//! handled by `OneOrMany<String>` (`TargetList`) on `default_target` and
//! `target` fields.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::dependency::Dependency;
use super::source::TraitSource;
use crate::shared::TargetList;

/// Repo-level project manifest, embedded as `[vendor]` in the committed
/// config document (`.ctx/traits/config.toml`, 0177) — formerly the
/// standalone `vendor.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectManifest {
    pub schema_version: String,

    /// Optional depth-one project-scoped manifest inheritance (P443): an npm
    /// spec (e.g. `@org/team-base`, `@org/team-base@^1.0.0`) resolving to a
    /// published package whose payload is itself a project manifest at
    /// `.ctx/traits.*`. That base manifest's `[dependencies]` (npm project
    /// package installs) merge under this manifest's own `[dependencies]`
    /// before local entries, with local entries winning on alias collision.
    /// A base manifest that itself sets `extends` is refused: inheritance is
    /// depth-one only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectMetadata>,

    #[serde(default, rename = "trait", skip_serializing_if = "Vec::is_empty")]
    pub trait_entries: Vec<TraitEntry>,

    #[serde(default, rename = "dependency", skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,

    /// `[dependencies]` table: npm-transport (P438) or project-scoped local
    /// `path:` (P535) project installs, keyed by vendor alias. Distinct from
    /// `[[dependency]]` above (`dependencies` field, "dependency" TOML key):
    /// that table declares what a package needs from another trait; this
    /// table declares what the *project* installs from the npm registry or a
    /// sibling repository's committed packages. `ctx traits dependency
    /// add/update/remove` are the only writers of this table.
    #[serde(
        default,
        rename = "dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub packages: BTreeMap<String, ProjectPackageDependency>,
}

/// One `[dependencies]` entry: either an npm package the project installs
/// (`{ npm, version }`, P438) or a project-scoped local `path:` source
/// (`{ path }`, P535).
///
/// [`Deserialize`] is hand-written (below) rather than derived
/// `#[serde(untagged)]`: serde's untagged-enum derive does not accept
/// `deny_unknown_fields` on individual struct-like variants, and without it a
/// table declaring both `npm`/`version` and `path` would silently decode as
/// whichever variant's required fields matched first. The hand-written
/// decoder tries each variant with unknown fields denied, so a mixed
/// declaration (both sources) or an empty one (neither) matches nothing and
/// fails to decode instead of silently picking a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case", untagged)]
pub enum ProjectPackageDependency {
    Npm {
        /// Full npm package identifier, e.g. `@scope/name` or `name`.
        npm: String,
        /// Semver range or dist-tag selector authored for this dependency.
        version: String,
    },
    /// A project-scoped local path source (P535): the authored relative
    /// path, normalized at parse time
    /// ([`ctx_traits_core::distribution::parse_install_spec`]). Never an
    /// absolute path, and never resolved or persisted machine-specifically —
    /// only this authored text is committed.
    Path { path: String },
    /// A git-transport source (0191): full repo URL plus an optional pinned
    /// ref and an optional path to the trait package within the repo. Kebab
    /// keys match `TraitSource::Git`'s authored `source = { git, ref, path }`
    /// shape.
    Git {
        git: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

impl<'de> Deserialize<'de> for ProjectPackageDependency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct RawNpm {
            npm: String,
            version: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct RawPath {
            path: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct RawGit {
            git: String,
            #[serde(rename = "ref", default)]
            git_ref: Option<String>,
            #[serde(default)]
            path: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Npm(RawNpm),
            Path(RawPath),
            Git(RawGit),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Npm(RawNpm { npm, version }) => Ok(ProjectPackageDependency::Npm { npm, version }),
            Raw::Path(RawPath { path }) => {
                let normalized = crate::distribution::normalize_manifest_path_source(&path)
                    .map_err(serde::de::Error::custom)?;
                Ok(ProjectPackageDependency::Path { path: normalized })
            }
            Raw::Git(RawGit { git, git_ref, path }) => {
                let url = super::source::normalize_git_url(&git)
                    .ok_or_else(|| serde::de::Error::custom("invalid git URL"))?;
                Ok(ProjectPackageDependency::Git {
                    git: url,
                    git_ref,
                    path,
                })
            }
        }
    }
}

impl ProjectPackageDependency {
    pub fn npm(package: impl Into<String>, version: impl Into<String>) -> Self {
        Self::Npm {
            npm: package.into(),
            version: version.into(),
        }
    }

    pub fn path(relative_path: impl Into<String>) -> Self {
        Self::Path {
            path: relative_path.into(),
        }
    }

    pub fn git(url: impl Into<String>, git_ref: Option<String>, path: Option<String>) -> Self {
        Self::Git {
            git: url.into(),
            git_ref,
            path,
        }
    }

    /// The install spec string this entry decodes back into:
    /// `"<npm>@<version>"`, `"path:<path>"`, or
    /// `"git+<url>#ref=...&path=..."`.
    pub fn spec_input(&self) -> String {
        match self {
            Self::Npm { npm, version } => format!("{npm}@{version}"),
            Self::Path { path } => format!("path:{path}"),
            Self::Git { git, git_ref, path } => {
                let mut spec = format!("git+{git}");
                let mut params = Vec::new();
                if let Some(git_ref) = git_ref {
                    params.push(format!("ref={git_ref}"));
                }
                if let Some(path) = path {
                    params.push(format!("path={path}"));
                }
                if !params.is_empty() {
                    spec.push('#');
                    spec.push_str(&params.join("&"));
                }
                spec
            }
        }
    }

    /// The unambiguous source identity used for alias-collision and
    /// lock-compatibility comparisons: the full npm package identifier,
    /// `"path:<path>"` for a local source, or `"git+<url>#path=<p>"` for a
    /// git source. Never confusable with an npm identity, since npm package
    /// names cannot contain `:`.
    pub fn identity(&self) -> String {
        match self {
            Self::Npm { npm, .. } => npm.clone(),
            Self::Path { path } => format!("path:{path}"),
            Self::Git { git, path, .. } => match path {
                Some(path) => format!("git+{git}#path={path}"),
                None => format!("git+{git}"),
            },
        }
    }

    pub fn as_npm(&self) -> Option<(&str, &str)> {
        match self {
            Self::Npm { npm, version } => Some((npm.as_str(), version.as_str())),
            Self::Path { .. } | Self::Git { .. } => None,
        }
    }

    pub fn as_path(&self) -> Option<&str> {
        match self {
            Self::Path { path } => Some(path.as_str()),
            Self::Npm { .. } | Self::Git { .. } => None,
        }
    }

    pub fn as_git(&self) -> Option<(&str, Option<&str>, Option<&str>)> {
        match self {
            Self::Git { git, git_ref, path } => {
                Some((git.as_str(), git_ref.as_deref(), path.as_deref()))
            }
            Self::Npm { .. } | Self::Path { .. } => None,
        }
    }
}

/// Project-level metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Render/export targets. Accepts scalar-or-array at decode boundary via
    /// `TargetList` (`OneOrMany<String>`).
    #[serde(default, skip_serializing_if = "OneOrMany::is_empty")]
    pub default_target: TargetList,
}

/// A trait dependency entry in the project manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct TraitEntry {
    pub id: String,
    pub version: String,
    pub source: TraitSource,

    /// Render/export targets. Accepts scalar-or-array at decode boundary via
    /// `TargetList` (`OneOrMany<String>`).
    #[serde(default, skip_serializing_if = "OneOrMany::is_empty")]
    pub target: TargetList,
}

use crate::shared::OneOrMany;

#[cfg(test)]
mod project_package_dependency_tests {
    use super::*;

    fn decode(toml: &str) -> Result<ProjectPackageDependency, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn decodes_legacy_npm_shape() {
        let dep = decode(
            r#"npm = "@scope/name"
version = "^1.0.0""#,
        )
        .unwrap();
        assert_eq!(dep, ProjectPackageDependency::npm("@scope/name", "^1.0.0"));
    }

    #[test]
    fn decodes_path_shape() {
        let dep = decode(r#"path = "../sibling/.ctx/traits/authored/implement""#).unwrap();
        assert_eq!(
            dep,
            ProjectPackageDependency::path("../sibling/.ctx/traits/authored/implement")
        );
    }

    #[test]
    fn rejects_empty_path_value() {
        decode(r#"path = """#).unwrap_err();
    }

    #[test]
    fn rejects_whitespace_only_path_value() {
        decode(r#"path = "   ""#).unwrap_err();
    }

    #[test]
    fn rejects_absolute_path_value() {
        decode(r#"path = "/etc/passwd""#).unwrap_err();
    }

    #[test]
    fn rejects_mixed_source_declaration() {
        let error = decode(
            r#"npm = "@scope/name"
version = "^1.0.0"
path = "../sibling""#,
        )
        .unwrap_err();
        let _ = error;
    }

    #[test]
    fn rejects_empty_source_declaration() {
        decode("").unwrap_err();
    }

    #[test]
    fn round_trips_through_serialize() {
        let npm = ProjectPackageDependency::npm("@scope/name", "^1.0.0");
        let text = toml::to_string(&npm).unwrap();
        assert_eq!(decode(&text).unwrap(), npm);

        let path = ProjectPackageDependency::path("packages/agents");
        let text = toml::to_string(&path).unwrap();
        assert_eq!(decode(&text).unwrap(), path);
    }

    #[test]
    fn spec_input_and_identity() {
        let npm = ProjectPackageDependency::npm("@scope/name", "^1.0.0");
        assert_eq!(npm.spec_input(), "@scope/name@^1.0.0");
        assert_eq!(npm.identity(), "@scope/name");

        let path = ProjectPackageDependency::path("packages/agents");
        assert_eq!(path.spec_input(), "path:packages/agents");
        assert_eq!(path.identity(), "path:packages/agents");
    }

    #[test]
    fn decodes_git_shape() {
        let dep = decode(
            r#"git = "https://github.com/ctx-company/traits.git"
ref = "v1.0.0"
path = "refactor""#,
        )
        .unwrap();
        assert_eq!(
            dep,
            ProjectPackageDependency::git(
                "https://github.com/ctx-company/traits.git",
                Some("v1.0.0".to_string()),
                Some("refactor".to_string()),
            )
        );
    }

    #[test]
    fn rejects_git_shape_with_shorthand_url() {
        decode(r#"git = "ctx-company/traits""#).unwrap_err();
    }

    #[test]
    fn rejects_git_shape_with_unknown_field() {
        decode(
            r#"git = "https://github.com/ctx-company/traits.git"
bogus = "x""#,
        )
        .unwrap_err();
    }

    #[test]
    fn git_spec_input_and_identity_round_trip_optional_fields() {
        let bare = ProjectPackageDependency::git("https://github.com/o/r.git", None, None);
        assert_eq!(bare.spec_input(), "git+https://github.com/o/r.git");
        assert_eq!(bare.identity(), "git+https://github.com/o/r.git");

        let full = ProjectPackageDependency::git(
            "https://github.com/o/r.git",
            Some("main".to_string()),
            Some("trait-a".to_string()),
        );
        assert_eq!(
            full.spec_input(),
            "git+https://github.com/o/r.git#ref=main&path=trait-a"
        );
        assert_eq!(
            full.identity(),
            "git+https://github.com/o/r.git#path=trait-a"
        );
    }

    #[test]
    fn round_trips_git_through_serialize() {
        let git = ProjectPackageDependency::git(
            "https://github.com/o/r.git",
            Some("main".to_string()),
            Some("trait-a".to_string()),
        );
        let text = toml::to_string(&git).unwrap();
        assert_eq!(decode(&text).unwrap(), git);
    }
}
