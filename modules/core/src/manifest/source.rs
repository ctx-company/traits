//! Trait source declarations.
//!
//! The semantic model uses a normalized enum that makes invalid states
//! unrepresentable: a `ref` without `git`, a local source without `path`, or
//! an empty source object cannot be constructed. A raw serde DTO preserves
//! the canonical `source = { git = "...", ref = "...", path = "..." }`
//! authoring shape at the serialization boundary and normalizes into the
//! enum before core code uses the source.
//!
//! Direct Git and local path are MVP source types. GitHub shorthand
//! (`owner/repo`) is CLI-edge sugar only and must never appear in canonical
//! persisted data.

use serde::{Deserialize, Serialize};

/// Normalized trait dependency source.
///
/// Invalid states are unrepresentable: a Git source always has a URL, a local
/// source always has a path, and `ref` only exists on Git sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraitSource {
    /// Local filesystem path source.
    Local {
        /// Filesystem path to the trait package.
        path: String,
    },
    /// Direct Git repository source.
    Git {
        /// Git repository URL. GitHub shorthand is not accepted here; it must
        /// be expanded to a full URL at the CLI edge before writing canonical
        /// data.
        url: String,
        /// Requested tag, branch, or revision. `None` means use the default
        /// branch. Tags or commits are preferred; branches produce
        /// drift/update warnings.
        requested_ref: Option<String>,
        /// Path to the trait package within the Git repo. `None` or empty
        /// means repo root.
        package_path: Option<String>,
    },
    /// npm package source, resolved and vendored via the Rust registry client.
    Npm {
        /// npm package name, including optional scope (for example
        /// `@ctx/trait-aws`).
        package: String,
        /// Optional path to the trait package inside the npm package.
        package_path: Option<String>,
    },
}

impl TraitSource {
    pub fn local(path: impl Into<String>) -> Self {
        Self::Local { path: path.into() }
    }

    pub fn git(url: impl Into<String>) -> Self {
        Self::Git {
            url: url.into(),
            requested_ref: None,
            package_path: None,
        }
    }

    pub fn with_ref(self, requested_ref: impl Into<String>) -> Self {
        match self {
            Self::Git {
                url, package_path, ..
            } => Self::Git {
                url,
                requested_ref: Some(requested_ref.into()),
                package_path,
            },
            Self::Local { .. } | Self::Npm { .. } => self,
        }
    }

    pub fn with_package_path(self, package_path: impl Into<String>) -> Self {
        match self {
            Self::Git {
                url, requested_ref, ..
            } => Self::Git {
                url,
                requested_ref,
                package_path: Some(package_path.into()),
            },
            Self::Local { .. } | Self::Npm { .. } => self,
        }
    }

    pub fn is_git(&self) -> bool {
        matches!(self, Self::Git { .. })
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// Stable source-kind label for provenance/search surfaces.
    pub fn manifest_source_kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local-path",
            Self::Git { .. } => "git",
            Self::Npm { .. } => "npm",
        }
    }
}

/// Raw serde DTO preserving the canonical `source = { git = "...", ref =
/// "...", path = "..." }` authoring shape.
///
/// Used only at the serialization boundary. Normalizes into `TraitSource`
/// before core code uses the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct TraitSourceRaw {
    #[serde(skip_serializing_if = "Option::is_none")]
    git: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    r#ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(rename = "package-index", skip_serializing_if = "Option::is_none")]
    package_index: Option<String>,
}

impl TraitSourceRaw {
    /// Normalize the raw DTO into the semantic enum, rejecting invalid and
    /// ambiguous states.
    fn normalize(self, field_path: &str) -> crate::Result<TraitSource> {
        let TraitSourceRaw {
            git,
            r#ref,
            path,
            registry,
            package,
            package_index,
        } = self;

        if package_index.is_some() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.source.package-index"),
                message:
                    "package indexes are not supported; npm packages resolve from the default registry"
                        .to_string(),
            }
            .into());
        }

        if let Some(package) = package {
            if git.is_some() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.source"),
                    message: "source.package cannot be combined with source.git".to_string(),
                }
                .into());
            }
            if r#ref.is_some() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.source.ref"),
                    message: "npm package sources use dependency.version, not source.ref"
                        .to_string(),
                }
                .into());
            }
            if registry.as_deref().is_some_and(|value| value != "npm") {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.source.registry"),
                    message: "only registry = \"npm\" is supported for package sources".to_string(),
                }
                .into());
            }
            if package.trim().is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}.source.package"),
                    message: "empty npm package".to_string(),
                }
                .into());
            }
            let package_path = path.and_then(|path| {
                if path.trim().is_empty() {
                    None
                } else {
                    Some(path)
                }
            });
            return Ok(TraitSource::Npm {
                package,
                package_path,
            });
        }

        if registry.is_some() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.source.registry"),
                message: "registry requires source.package".to_string(),
            }
            .into());
        }

        match (git, r#ref, path) {
            // Git source with URL.
            (Some(url), requested_ref, package_path) => {
                let url = normalize_git_url(&url).ok_or_else(|| {
                    crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.source.git"),
                        message: "must be a full Git URL with a supported scheme; GitHub shorthand and scheme-less URLs are not canonical source input".to_string(),
                    }
                })?;

                if matches!(requested_ref.as_deref(), Some(value) if value.trim().is_empty()) {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.source.ref"),
                        message: "empty Git ref".to_string(),
                    }
                    .into());
                }

                let package_path = package_path.and_then(|path| {
                    if path.trim().is_empty() {
                        None
                    } else {
                        Some(path)
                    }
                });

                Ok(TraitSource::Git {
                    url,
                    requested_ref,
                    package_path,
                })
            }
            // `ref` without `git` is invalid.
            (None, Some(_), _) => Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.source.ref"),
                message: "ref without git is invalid".to_string(),
            }
            .into()),
            // Local source: path required.
            (None, None, Some(path)) => {
                if path.trim().is_empty() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.source.path"),
                        message: "empty local path".to_string(),
                    }
                    .into());
                }
                Ok(TraitSource::Local { path })
            }
            // Empty source object.
            (None, None, None) => Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}.source"),
                message: "missing source fields".to_string(),
            }
            .into()),
        }
    }
}

impl Serialize for TraitSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let raw = match self {
            Self::Local { path } => TraitSourceRaw {
                git: None,
                r#ref: None,
                path: Some(path.clone()),
                registry: None,
                package: None,
                package_index: None,
            },
            Self::Git {
                url,
                requested_ref,
                package_path,
            } => TraitSourceRaw {
                git: Some(url.clone()),
                r#ref: requested_ref.clone(),
                path: package_path.clone(),
                registry: None,
                package: None,
                package_index: None,
            },
            Self::Npm {
                package,
                package_path,
            } => TraitSourceRaw {
                git: None,
                r#ref: None,
                path: package_path.clone(),
                registry: Some("npm".to_string()),
                package: Some(package.clone()),
                package_index: None,
            },
        };
        raw.serialize(serializer)
    }
}

fn normalize_git_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.chars().any(char::is_whitespace) {
        return None;
    }
    if ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
    {
        Some(url.to_string())
    } else {
        None
    }
}

impl<'de> Deserialize<'de> for TraitSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = TraitSourceRaw::deserialize(deserializer)?;
        raw.normalize("trait").map_err(serde::de::Error::custom)
    }
}
