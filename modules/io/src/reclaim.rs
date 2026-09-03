//! Shared terminal artifact reclaim for landed and killed runs.

use std::collections::BTreeSet;

use camino::Utf8Path;
use ctx_traits_core::procedure::session::{
    NamedCacheReclaimRecord, ReclaimEvidence, SlotReclaimRecord, WorktreeReclaimRecord,
};

/// Reclaim declared terminal artifacts. Every individual failure is represented
/// in the returned evidence so a killed run can still record its outcome.
pub fn terminal_reclaim(
    worktree_root: &Utf8Path,
    retention_paths: &[String],
    cache_names: &BTreeSet<String>,
) -> ReclaimEvidence {
    let worktree_paths =
        crate::retention::prune_terminal_cheap_artifacts(worktree_root, retention_paths)
            .into_iter()
            .map(|outcome| WorktreeReclaimRecord {
                path: outcome.relative_path.to_string(),
                removed: outcome.removed,
                error: outcome.error,
            })
            .collect();
    // Retention is local to the retained worktree. Preserve its result even
    // when this is no longer a discoverable repository.
    let repo_root = match crate::repository::discover_main_repo_root(worktree_root) {
        Ok(path) => path,
        Err(error) => return failed_evidence(worktree_paths, cache_names, error.to_string()),
    };
    let slots_root = match crate::state::build_slots_root(&repo_root) {
        Ok(path) => path,
        Err(error) => return failed_evidence(worktree_paths, cache_names, error.to_string()),
    };
    crate::target_slot::reclaim_terminal(
        &slots_root,
        &repo_root,
        worktree_root,
        &repo_root.join(crate::layout::WORKTREE_ROOT),
        cache_names,
        worktree_paths,
    )
}

fn failed_evidence(
    worktree_paths: Vec<WorktreeReclaimRecord>,
    cache_names: &BTreeSet<String>,
    error: String,
) -> ReclaimEvidence {
    ReclaimEvidence {
        worktree_paths,
        slot: SlotReclaimRecord::Failed {
            error: error.clone(),
        },
        named_caches: cache_names
            .iter()
            .map(|name| NamedCacheReclaimRecord::Failed {
                name: name.clone(),
                error: error.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use camino::Utf8PathBuf;
    use ctx_traits_core::procedure::session::SlotReclaimRecord;

    use super::terminal_reclaim;

    #[test]
    fn retention_is_recorded_when_repository_discovery_fails() {
        let root = std::env::temp_dir().join(format!(
            "ctx-terminal-reclaim-discovery-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("retained-build-output"), "bytes").unwrap();
        let worktree = Utf8PathBuf::from_path_buf(root.clone()).unwrap();

        let evidence = terminal_reclaim(
            &worktree,
            &["retained-build-output".to_string()],
            &BTreeSet::new(),
        );

        assert!(!root.join("retained-build-output").exists());
        assert_eq!(evidence.worktree_paths.len(), 1);
        assert_eq!(evidence.worktree_paths[0].path, "retained-build-output");
        assert!(evidence.worktree_paths[0].removed);
        assert!(matches!(evidence.slot, SlotReclaimRecord::Failed { .. }));
        let _ = fs::remove_dir_all(root);
    }
}
