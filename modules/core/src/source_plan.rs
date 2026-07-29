//! Source resolution planning: describes IO actions for dependency sources.
//!
//! Core produces typed source plans; IO later executes them. No filesystem or
//! network IO is performed here. The plan describes what needs to be resolved;
//! the IO boundary translates plans into concrete actions.
//!
//! GitHub shorthand (`owner/repo`) is CLI-edge sugar, not canonical model
//! input. It must be expanded to a full URL before reaching the source plan.

use serde::{Deserialize, Serialize};

use crate::manifest::{Dependency, TraitSource};

/// A planned IO action to resolve a dependency source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceAction {
    /// Read from a local filesystem path.
    ReadLocal {
        /// Normalized filesystem path to the trait package.
        path: String,
    },
    /// Clone or fetch from a Git URL.
    CloneGit {
        /// Normalized Git repository URL (full URL, not GitHub shorthand).
        url: String,
        /// Requested tag, branch, or commit. `None` means default branch
        /// (produces a drift warning).
        requested_ref: Option<String>,
        /// Path to the trait package within the Git repo. `None` or empty
        /// means repo root.
        package_path: Option<String>,
    },
    /// Read an already-installed npm package from `node_modules`.
    ReadNpm {
        /// npm package name, including optional scope.
        package: String,
        /// Optional path to the trait package within the npm package.
        package_path: Option<String>,
    },
    /// Source type is not supported in the current MVP scope.
    Unsupported {
        /// Why this source cannot be resolved.
        reason: String,
    },
}

/// Warning about a source resolution plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceWarning {
    /// No ref specified; the default branch will be used. This introduces
    /// drift risk: the resolved content may change without notice.
    MissingRef,
    /// A ref was specified but may be a branch rather than a tag or commit.
    /// Tags and commits are preferred for reproducible resolution.
    FloatingRef {
        /// The requested ref text.
        requested_ref: String,
    },
}

/// A planned resolution for one dependency source.
///
/// Produced by [`plan_dependency`] from a manifest [`Dependency`]. Contains
/// the IO action needed and any warnings about the resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SourcePlan {
    /// The dependency alias.
    pub alias: String,
    /// The trait ID being resolved.
    pub trait_id: String,
    /// The IO action to execute.
    pub action: SourceAction,
    /// Warnings about this resolution (deterministically sorted).
    pub warnings: Vec<SourceWarning>,
}

/// Plan the resolution of a single dependency source.
///
/// Translates a [`Dependency`]'s source metadata into a typed [`SourcePlan`]
/// without performing any IO. Dependencies without a source produce an
/// `Unsupported` action.
pub fn plan_dependency(dep: &Dependency) -> SourcePlan {
    let (action, mut warnings) = match &dep.source {
        Some(TraitSource::Local { path }) => {
            (SourceAction::ReadLocal { path: path.clone() }, Vec::new())
        }
        Some(TraitSource::Git {
            url,
            requested_ref,
            package_path,
        }) => {
            let mut w = Vec::new();
            match requested_ref {
                None => w.push(SourceWarning::MissingRef),
                Some(r) if !is_offline_pinned_ref(r) => {
                    w.push(SourceWarning::FloatingRef {
                        requested_ref: r.clone(),
                    });
                }
                Some(_) => {}
            }
            (
                SourceAction::CloneGit {
                    url: url.clone(),
                    requested_ref: requested_ref.clone(),
                    package_path: package_path.clone(),
                },
                w,
            )
        }
        Some(TraitSource::Npm {
            package,
            package_path,
        }) => (
            SourceAction::ReadNpm {
                package: package.clone(),
                package_path: package_path.clone(),
            },
            Vec::new(),
        ),
        None => (
            SourceAction::Unsupported {
                reason: "no source declared; must be resolved from lockfile or provided explicitly"
                    .to_string(),
            },
            Vec::new(),
        ),
    };

    warnings.sort();
    SourcePlan {
        alias: dep.alias.clone(),
        trait_id: dep.id.clone(),
        action,
        warnings,
    }
}

fn is_offline_pinned_ref(requested_ref: &str) -> bool {
    let ref_text = requested_ref.trim();
    ref_text.starts_with("refs/tags/")
        || ((ref_text.len() == 40 || ref_text.len() == 64)
            && ref_text.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Plan resolutions for all dependencies in a manifest.
///
/// Returns one [`SourcePlan`] per dependency, preserving manifest order.
pub fn plan_dependencies(deps: &[Dependency]) -> Vec<SourcePlan> {
    deps.iter().map(plan_dependency).collect()
}
