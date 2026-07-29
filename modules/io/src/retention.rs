//! Safe retention primitives for derived run artifacts.
//!
//! Receipts are deliberately absent from this module: session ledgers and the
//! audit journal are evidence, not cache entries, and are never age-pruned.

use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use globset::Glob;

/// Default number of completed run trace directories retained.
pub const DEFAULT_DEBUG_RUN_LIMIT: usize = 50;
/// Default maximum age for debug traces and spawn logs.
pub const DEFAULT_DEBUG_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Default warm-resume window for expensive worktree artifacts.
pub const DEFAULT_EXPENSIVE_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// An explicitly declared regenerable path tier. Paths are always relative to
/// the worktree; the caller must reject symlinks before deleting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeArtifactTier {
    /// Deleted on every terminal outcome, including cancellation.
    Cheap,
    /// Kept until the warm-resume grace window expires.
    Expensive,
}

/// A declared worktree artifact path and its rebuild-cost tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeArtifact {
    pub relative_path: Utf8PathBuf,
    pub tier: WorktreeArtifactTier,
}

/// One declared cleanup action's immutable result. A failed deletion never
/// erases evidence of earlier successful removals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionOutcome {
    pub relative_path: Utf8PathBuf,
    pub removed: bool,
    pub error: Option<String>,
}

/// Remove a declared, regenerable artifact without following symlinks or
/// allowing a declaration to escape its worktree. Missing paths are a normal
/// no-op so terminal cleanup is idempotent.
pub fn prune_worktree_artifact(
    worktree: &Utf8Path,
    artifact: &WorktreeArtifact,
) -> crate::Result<bool> {
    if artifact.relative_path.as_str().is_empty()
        || artifact.relative_path == Utf8Path::new(".")
        || artifact.relative_path.is_absolute()
        || artifact.relative_path.components().any(|component| {
            matches!(
                component,
                camino::Utf8Component::ParentDir | camino::Utf8Component::CurDir
            )
        })
    {
        return Err(crate::Error::Usage {
            message: format!(
                "worktree retention path must be a relative path without '..': {}",
                artifact.relative_path
            ),
        });
    }

    let path = worktree.join(&artifact.relative_path);
    if artifact.relative_path == Utf8Path::new(".git")
        || artifact.relative_path.starts_with(".git/")
    {
        return Err(crate::Error::Usage {
            message: format!("refusing to prune protected worktree control path: {path}"),
        });
    }
    // A real worktree must only prune ignored, regenerable artifacts. Unit
    // scratch directories without Git metadata retain their direct primitive
    // coverage; production worktrees always carry a `.git` control file.
    if worktree.join(".git").exists()
        && crate::repository::check_ignored(worktree, artifact.relative_path.as_str())?
            != crate::repository::IgnoreStatus::Ignored
    {
        return Err(crate::Error::Usage {
            message: format!(
                "refusing to prune non-ignored worktree content: {}",
                artifact.relative_path
            ),
        });
    }
    // Validate the worktree and every existing parent without following links
    // before inspecting the leaf, then repeat the same chain check immediately
    // before the destructive operation below.
    crate::path_safety::ensure_no_symlink_ancestors(worktree, "worktree root")?;
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "worktree artifact ancestor")?;
    }
    let metadata = match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
            .into());
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(crate::Error::Usage {
            message: format!("refusing to prune symlinked worktree artifact: {path}"),
        });
    }
    if let Some(parent) = path.parent() {
        crate::path_safety::ensure_no_symlink_ancestors(parent, "worktree artifact ancestor")?;
    }
    if metadata.is_dir() {
        std::fs::remove_dir_all(path.as_std_path())
    } else if metadata.is_file() {
        std::fs::remove_file(path.as_std_path())
    } else {
        return Err(crate::Error::Usage {
            message: format!("refusing to prune non-file worktree artifact: {path}"),
        });
    }
    .map_err(|source| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    })?;
    Ok(true)
}

/// Prune every declared cheap artifact after a drive ends. This function never
/// examines source-control state and therefore cannot delete branches or
/// uncommitted files outside the explicit declarations.
pub fn prune_terminal_cheap_artifacts(
    worktree: &Utf8Path,
    declared_paths: &[String],
) -> Vec<RetentionOutcome> {
    declared_paths
        .iter()
        .flat_map(|pattern| match declared_artifact_paths(worktree, pattern) {
            Ok(paths) => paths.into_iter().map(Ok).collect::<Vec<_>>(),
            Err(error) => vec![Err((Utf8PathBuf::from(pattern), error.to_string()))],
        })
        .map(|path| match path {
            Ok(relative_path) => {
                let artifact = WorktreeArtifact {
                    relative_path,
                    tier: WorktreeArtifactTier::Cheap,
                };
                match prune_worktree_artifact(worktree, &artifact) {
                    Ok(removed) => RetentionOutcome {
                        relative_path: artifact.relative_path,
                        removed,
                        error: None,
                    },
                    Err(error) => RetentionOutcome {
                        relative_path: artifact.relative_path,
                        removed: false,
                        error: Some(error.to_string()),
                    },
                }
            }
            Err((relative_path, error)) => RetentionOutcome {
                relative_path,
                removed: false,
                error: Some(error),
            },
        })
        .collect()
}

fn declared_artifact_paths(worktree: &Utf8Path, pattern: &str) -> crate::Result<Vec<Utf8PathBuf>> {
    if !pattern.contains(['*', '?', '[', '{']) {
        return Ok(vec![Utf8PathBuf::from(pattern)]);
    }
    let glob = Glob::new(pattern)
        .map_err(|error| crate::Error::Usage {
            message: format!("invalid worktree retention glob {pattern:?}: {error}"),
        })?
        .compile_matcher();
    let mut paths = Vec::new();
    collect_matching_paths(worktree, Utf8Path::new(""), &glob, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_matching_paths(
    root: &Utf8Path,
    relative: &Utf8Path,
    glob: &globset::GlobMatcher,
    paths: &mut Vec<Utf8PathBuf>,
) -> crate::Result<()> {
    let directory = root.join(relative);
    for entry in std::fs::read_dir(directory.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: directory.to_string(),
            source,
        }
    })? {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: directory.to_string(),
            source,
        })?;
        let name = Utf8PathBuf::from_path_buf(entry.file_name().into()).map_err(|_| {
            crate::Error::Usage {
                message: "worktree retention path is not UTF-8".to_string(),
            }
        })?;
        let child = relative.join(name);
        let metadata =
            std::fs::symlink_metadata(root.join(&child).as_std_path()).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: child.to_string(),
                    source,
                }
            })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if glob.is_match(child.as_str()) {
            paths.push(child.clone());
        }
        if metadata.is_dir() {
            collect_matching_paths(root, &child, glob, paths)?;
        }
    }
    Ok(())
}

/// Whether the expensive tier remains available for a warm resume.
pub fn expensive_tier_is_warm(last_active: SystemTime, now: SystemTime) -> bool {
    now.duration_since(last_active)
        .map(|elapsed| elapsed <= DEFAULT_EXPENSIVE_GRACE)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> Utf8PathBuf {
        let path =
            std::env::temp_dir().join(format!("ctx-retention-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Utf8PathBuf::from_path_buf(path).unwrap()
    }

    #[test]
    fn grace_window_is_inclusive_and_never_expires_future_activity() {
        let now = SystemTime::now();
        assert!(expensive_tier_is_warm(now - DEFAULT_EXPENSIVE_GRACE, now));
        assert!(!expensive_tier_is_warm(
            now - DEFAULT_EXPENSIVE_GRACE - Duration::from_secs(1),
            now
        ));
        assert!(expensive_tier_is_warm(now + Duration::from_secs(1), now));
    }

    #[test]
    fn pruning_refuses_root_equivalents_and_symlinked_ancestors() {
        let root = scratch("safety");
        let outside = scratch("outside");
        let sentinel = outside.join("sentinel");
        fs::write(&sentinel, "keep").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        for relative_path in ["", ".", "link/sentinel"] {
            let artifact = WorktreeArtifact {
                relative_path: Utf8PathBuf::from(relative_path),
                tier: WorktreeArtifactTier::Cheap,
            };
            assert!(prune_worktree_artifact(&root, &artifact).is_err());
        }
        assert!(root.is_dir());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "keep");

        let file = root.join("derived");
        fs::write(&file, "discard").unwrap();
        let artifact = WorktreeArtifact {
            relative_path: Utf8PathBuf::from("derived"),
            tier: WorktreeArtifactTier::Cheap,
        };
        assert!(prune_worktree_artifact(&root, &artifact).unwrap());
        assert!(!file.exists());

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn terminal_cleanup_only_removes_declared_cheap_artifacts() {
        let root = scratch("terminal");
        fs::write(root.join("derived"), "discard").unwrap();
        fs::write(root.join("source"), "keep").unwrap();
        let outcomes = prune_terminal_cheap_artifacts(&root, &["derived".to_string()]);
        assert_eq!(outcomes[0].relative_path, Utf8PathBuf::from("derived"));
        assert!(outcomes[0].removed);
        assert!(!root.join("derived").exists());
        assert_eq!(fs::read_to_string(root.join("source")).unwrap(), "keep");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_cleanup_reports_removals_before_later_failure() {
        let root = scratch("partial");
        fs::write(root.join("derived"), "discard").unwrap();
        let outcomes =
            prune_terminal_cheap_artifacts(&root, &["derived".to_string(), "..".to_string()]);
        assert!(outcomes[0].removed);
        assert!(outcomes[0].error.is_none());
        assert_eq!(outcomes[1].relative_path, Utf8PathBuf::from(".."));
        assert!(outcomes[1].error.is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_cleanup_expands_declared_globs_without_following_symlinks() {
        let root = scratch("glob");
        for path in ["target/a/incremental", "target/b/incremental"] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        fs::create_dir_all(root.join("target/a/keep")).unwrap();
        let outside = scratch("glob-outside");
        fs::create_dir_all(outside.join("incremental")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("target/link")).unwrap();
        let outcomes =
            prune_terminal_cheap_artifacts(&root, &["target/**/incremental".to_string()]);
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|outcome| outcome.removed));
        assert!(root.join("target/a/keep").is_dir());
        assert!(outside.join("incremental").is_dir());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }
}
