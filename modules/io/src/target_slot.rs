//! Stable, leased build-cache slots (0057).
//!
//! Every toolchain's build cache is anchored to absolute paths: cargo records
//! each unit's own output path in its dep-info (454 of 454 files in this
//! repository's target), tsc keys `.tsbuildinfo` the same way, and the rest
//! behave similarly. A cache that MOVES is therefore a cache that is thrown
//! away — measured 2026-08-03: cloning a warm target into a run's worktree
//! made cargo recompile every unit, third-party crates included, because only
//! the target path had changed.
//!
//! So warmth cannot come from copying a cache into each new worktree. It can
//! only come from a path that RECURS. This module hands out a small fixed set
//! of such paths, one per concurrently-live worktree, so:
//!
//! * a run gets a directory some earlier run already warmed, and
//! * no two live runs share one, which is what would otherwise serialise them
//!   on the toolchain's own build lock.
//!
//! Assignment is by worktree, recorded in a registry beside the slots, so the
//! same run resolves to the same slot on every frame and a parked worktree
//! keeps its slot (resuming it stays warm). A slot is reclaimed when the
//! worktree it was assigned to no longer exists — no pid liveness, no release
//! hook, nothing to leak when a driver is killed.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// How many build-cache slots a repository hands out. Four covers the
/// concurrency this product is actually driven at while bounding disk: each
/// slot is a full build tree. A fifth concurrent run does not fail — it
/// shares the least-recently-assigned slot and pays the toolchain's own build
/// lock, which is still cheaper than a guaranteed cold build.
pub const DEFAULT_TARGET_SLOTS: usize = 4;

const REGISTRY: &str = "slots.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    /// slot index → the worktree path it is currently assigned to.
    #[serde(default)]
    assignments: BTreeMap<String, String>,
    /// slot index → monotonic counter at assignment, for least-recent reuse
    /// when every slot is held.
    #[serde(default)]
    stamps: BTreeMap<String, u64>,
    #[serde(default)]
    counter: u64,
}

/// Resolve the build-cache slot directory for `worktree_root`, creating it if
/// needed. Repeated calls for the same worktree return the same path.
pub fn resolve(
    slots_root: &Utf8Path,
    worktree_root: &Utf8Path,
    slots: usize,
) -> crate::Result<Utf8PathBuf> {
    let slots = slots.max(1);
    std::fs::create_dir_all(slots_root.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: slots_root.to_string(),
            source,
        }
    })?;
    let registry_path = slots_root.join(REGISTRY);
    // Two runs dispatched at once resolve their slot at the same moment; the
    // registry read-modify-write has to be serialised or both take slot 0 and
    // then contend for the whole build.
    let lock_path = slots_root.join("slots.lock");
    let lock = crate::file_lock::open_lock_file_no_follow(&lock_path)
        .and_then(|file| crate::file_lock::lock_exclusive_blocking(&file).map(|()| file))
        .map_err(|source| crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        })?;
    let mut registry = read_registry(&registry_path)?;
    let worktree = worktree_root.to_string();

    if let Some((index, _)) = registry
        .assignments
        .iter()
        .find(|(_, assigned)| *assigned == &worktree)
    {
        let index = index.clone();
        let slot = ensure_slot_dir(slots_root, &index);
        drop(lock);
        return slot;
    }

    // Prefer a slot no live worktree holds: either never assigned, or
    // assigned to a worktree that has since been removed.
    let free = (0..slots).map(|i| i.to_string()).find(|index| {
        registry
            .assignments
            .get(index)
            .is_none_or(|assigned| !Utf8Path::new(assigned).exists())
    });

    let index = match free {
        Some(index) => index,
        // Every slot is held by a live worktree: share the least recently
        // assigned one rather than minting an unbounded number of cold trees.
        None => (0..slots)
            .map(|i| i.to_string())
            .min_by_key(|index| registry.stamps.get(index).copied().unwrap_or(0))
            .unwrap_or_else(|| "0".to_string()),
    };

    registry.counter = registry.counter.saturating_add(1);
    registry.assignments.insert(index.clone(), worktree.clone());
    registry.stamps.insert(index.clone(), registry.counter);
    write_registry(&registry_path, &registry)?;
    let slot = ensure_slot_dir(slots_root, &index);
    drop(lock);
    slot
}

fn ensure_slot_dir(slots_root: &Utf8Path, index: &str) -> crate::Result<Utf8PathBuf> {
    let path = slots_root.join(format!("slot-{index}"));
    std::fs::create_dir_all(path.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
    })?;
    Ok(path)
}

fn read_registry(path: &Utf8Path) -> crate::Result<Registry> {
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

fn write_registry(path: &Utf8Path, registry: &Registry) -> crate::Result<()> {
    let text = serde_json::to_string_pretty(registry).map_err(|error| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::other(error),
        }
    })?;
    std::fs::write(path.as_std_path(), text).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Utf8PathBuf {
        let base = Utf8PathBuf::from(std::env::temp_dir().to_string_lossy().to_string())
            .join(format!("ctx-target-slot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(base.as_std_path());
        std::fs::create_dir_all(base.as_std_path()).unwrap();
        base
    }

    #[test]
    fn the_same_worktree_always_resolves_to_the_same_slot() {
        let base = scratch("stable");
        let slots_root = base.join("targets");
        let worktree = base.join("wt-a");
        std::fs::create_dir_all(worktree.as_std_path()).unwrap();
        let first = resolve(&slots_root, &worktree, 4).unwrap();
        let second = resolve(&slots_root, &worktree, 4).unwrap();
        assert_eq!(
            first, second,
            "a run must keep its warmed slot across frames"
        );
    }

    #[test]
    fn live_worktrees_never_share_a_slot() {
        let base = scratch("distinct");
        let slots_root = base.join("targets");
        let mut seen = Vec::new();
        for name in ["wt-a", "wt-b", "wt-c", "wt-d"] {
            let worktree = base.join(name);
            std::fs::create_dir_all(worktree.as_std_path()).unwrap();
            seen.push(resolve(&slots_root, &worktree, 4).unwrap());
        }
        let unique: std::collections::BTreeSet<_> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            4,
            "concurrent runs must not contend: {seen:?}"
        );
    }

    #[test]
    fn a_removed_worktree_releases_its_slot_for_reuse() {
        let base = scratch("reclaim");
        let slots_root = base.join("targets");
        let first_worktree = base.join("wt-gone");
        std::fs::create_dir_all(first_worktree.as_std_path()).unwrap();
        let first = resolve(&slots_root, &first_worktree, 1).unwrap();
        std::fs::remove_dir_all(first_worktree.as_std_path()).unwrap();

        let next_worktree = base.join("wt-next");
        std::fs::create_dir_all(next_worktree.as_std_path()).unwrap();
        let next = resolve(&slots_root, &next_worktree, 1).unwrap();
        assert_eq!(
            first, next,
            "the warmed directory is the point — a finished run's slot must be reused, not abandoned"
        );
    }
}
