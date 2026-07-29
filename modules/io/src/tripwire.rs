//! P479 out-of-tree mutation tripwire.
//!
//! A `--worktree` run must be unable to mutate the invocation repository (the
//! main checkout the run was launched from, not the run's own worktree)
//! without that mutation becoming a loud, typed, attributable, un-landable
//! event. This module owns the whole mechanism: a two-legged snapshot (`git
//! status` over the invocation repo, plus digests of a sentinel set of
//! config-layer files that `--untracked-files=all` cannot see because they
//! are gitignored), a frame-boundary checkpoint that diffs against the prior
//! snapshot, and the `warn | park` policy this repository's config resolves.
//!
//! Attribution here is deliberately a *window*, not a process claim: a
//! [`Finding`] names the frame label that was current when the checkpoint
//! that first observed the mutation ran, never a PID or "the agent wrote" —
//! this codebase ships no daemon and no filesystem watcher, so it cannot
//! honestly say more than "the invocation repository changed while this
//! frame was the only ctx-dispatched work in this run's worktree".
//!
//! Status-leg coverage limit: the diff compares `(xy-code, path)` sets, so a
//! frame that further modifies a tracked/untracked path already dirty at the
//! prior snapshot produces no status-leg delta and is not reported that way.
//! The sentinel leg (content-digest based) is unaffected, and `ctx traits
//! merge`'s own clean-`main` preflight independently refuses to land while
//! the invocation repo is dirty, so the un-landability guarantee still holds
//! for this case — only the tripwire's own attribution of it is silent.

use std::cell::Cell;
use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use serde::Deserialize;

/// `[worktree.tripwire] policy`: `park` (default, fail-closed per the
/// receipts doctrine) or `warn` (evidence + stderr warning, run continues and
/// may land).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TripwirePolicy {
    #[default]
    Park,
    Warn,
}

impl TripwirePolicy {
    /// The one canonical spelling persisted into `OutOfTreeMutationEvidence`,
    /// rendered by `doctor --config`, and read back by the merge-side
    /// precondition — mirroring the `DeepMergeRule::as_str` precedent
    /// (`ctx_traits_core::procedure::session::DeepMergeRule::as_str`) so the
    /// writer and every reader agree by construction, never by convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Park => "park",
            Self::Warn => "warn",
        }
    }

    /// True for the exact persisted spelling this policy would have written
    /// via [`Self::as_str`] — the single predicate the merge-side
    /// un-landability check must use, so a future rename of either side
    /// cannot silently desync them.
    pub fn matches_persisted(self, persisted: &str) -> bool {
        persisted == self.as_str()
    }
}

/// `[worktree.tripwire]`: the two keys this phase owns. `sentinel` names
/// extra watched FILES (never directories — no recursive-walk feature),
/// repository-relative or absolute, in addition to the resolved config-layer
/// files the tripwire always watches.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorktreeTripwireConfig {
    #[serde(default)]
    pub policy: TripwirePolicy,
    #[serde(default)]
    pub sentinel: Vec<String>,
}

impl Default for WorktreeTripwireConfig {
    fn default() -> Self {
        Self {
            policy: TripwirePolicy::Park,
            sentinel: Vec::new(),
        }
    }
}

/// One frame-boundary escape: the offending repository-relative (or, for a
/// sentinel outside the repository, absolute) paths, and the frame label that
/// was current when the checkpoint that first observed them ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub paths: Vec<String>,
    pub frame: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    /// `(xy-code, repository-relative path)`, excluding the worktrees
    /// subtree. A `BTreeMap`-free `Vec` compared as sets in [`Tripwire::diff`]
    /// — order does not matter for equality, only membership.
    status: Vec<(String, String)>,
    sentinels: BTreeMap<Utf8PathBuf, Option<Digest>>,
}

/// The label used for every checkpoint taken before this run's first frame
/// has been resolved (the loop-top checkpoint of iteration 1, which becomes
/// the baseline and reports no finding).
const BEFORE_FIRST_FRAME: &str = "before first frame";

/// Snapshot/diff guard for one drive/resume invocation. Constructed once (in
/// `drive()`, D1) with the invocation repository root and the resolved
/// sentinel file list; `checkpoint()` is called at the top of every loop
/// iteration and once more, as a terminal sweep, after the loop returns.
///
/// The baseline is lazy: the first `checkpoint()` call takes it and returns
/// `None`, so a clean N-frame run costs exactly N+1 snapshots (N loop tops
/// plus the terminal sweep) — two for a single-frame run, matching the
/// contract's stated clean-run overhead.
pub struct Tripwire {
    main_root: Utf8PathBuf,
    sentinel_paths: Vec<Utf8PathBuf>,
    policy: TripwirePolicy,
    baseline: Option<Snapshot>,
    frame_label: String,
    snapshot_count: Cell<u64>,
}

impl Tripwire {
    pub fn new(
        main_root: Utf8PathBuf,
        sentinel_paths: Vec<Utf8PathBuf>,
        policy: TripwirePolicy,
    ) -> Self {
        Self {
            main_root,
            sentinel_paths,
            policy,
            baseline: None,
            frame_label: BEFORE_FIRST_FRAME.to_string(),
            snapshot_count: Cell::new(0),
        }
    }

    pub fn policy(&self) -> TripwirePolicy {
        self.policy
    }

    /// Record the label naming the frame this drive is about to dispatch, so
    /// the NEXT checkpoint (the following iteration's loop-top check, or the
    /// terminal sweep if this was the last frame) attributes any observed
    /// mutation to it rather than to a generic "a frame ran" statement.
    pub fn set_frame_label(&mut self, label: String) {
        self.frame_label = label;
    }

    /// Snapshot the invocation repository now and diff against the stored
    /// baseline. `Ok(None)` on the very first call (nothing to diff against
    /// yet — this call becomes the baseline) and on every call that finds no
    /// change. The stored baseline is unconditionally replaced on every call,
    /// so one escape is reported exactly once, at the boundary where it first
    /// appeared.
    pub fn checkpoint(&mut self) -> crate::Result<Option<Finding>> {
        let snapshot = self.snapshot()?;
        let finding = match self.baseline.as_ref() {
            None => None,
            Some(previous) => {
                let paths = self.diff(previous, &snapshot);
                (!paths.is_empty()).then(|| Finding {
                    paths,
                    frame: self.frame_label.clone(),
                })
            }
        };
        self.baseline = Some(snapshot);
        Ok(finding)
    }

    fn snapshot(&self) -> crate::Result<Snapshot> {
        self.snapshot_count.set(self.snapshot_count.get() + 1);
        let status = status_records(&self.main_root)?;
        let mut sentinels = BTreeMap::new();
        for path in &self.sentinel_paths {
            let digest = if path.is_file() {
                Some(crate::config_source::hash_file(path)?)
            } else {
                None
            };
            sentinels.insert(path.clone(), digest);
        }
        Ok(Snapshot { status, sentinels })
    }

    fn diff(&self, previous: &Snapshot, current: &Snapshot) -> Vec<String> {
        let mut paths = std::collections::BTreeSet::new();
        let previous_status: std::collections::BTreeSet<_> =
            previous.status.iter().cloned().collect();
        let current_status: std::collections::BTreeSet<_> =
            current.status.iter().cloned().collect();
        for (_, path) in previous_status.symmetric_difference(&current_status) {
            paths.insert(path.clone());
        }
        for (path, digest) in &current.sentinels {
            if previous.sentinels.get(path) != Some(digest) {
                paths.insert(self.display_path(path));
            }
        }
        paths.into_iter().collect()
    }

    fn display_path(&self, path: &Utf8Path) -> String {
        match path.strip_prefix(&self.main_root) {
            Ok(relative) => relative.to_string(),
            Err(_) => path.to_string(),
        }
    }

    #[cfg(test)]
    fn snapshot_count(&self) -> u64 {
        self.snapshot_count.get()
    }
}

/// `git -C <main_root> status --porcelain=v1 -z --untracked-files=all`,
/// excluding any path under [`crate::layout::WORKTREE_ROOT`] (the run's own,
/// legitimate write surface). `-z` is load-bearing: without it,
/// `core.quotePath` and rename records (`R  old -> new`) make path extraction
/// lossy, and this module must report exact offending paths.
/// Generous relative to the default 16KiB capture ceiling: a status listing
/// over a large working tree must not read as "no change" because it was
/// silently truncated — [`crate::command::RunOutput::refuse_if_truncated`]
/// below is the actual guarantee; this cap only has to be big enough that a
/// realistic repository never legitimately hits it.
const STATUS_CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

fn status_records(main_root: &Utf8Path) -> crate::Result<Vec<(String, String)>> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(main_root),
        cwd: None,
        args: &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        success_exit_code: &[0],
        timeout_ms: crate::git_process::PLUMBING_TIMEOUT_MS,
        capture_limit: STATUS_CAPTURE_LIMIT,
    })?;
    if !output.success {
        return Err(crate::git_process::error(
            "git status --porcelain=v1 -z --untracked-files=all",
            &output,
        ));
    }
    output.refuse_if_truncated("git status --porcelain=v1 -z --untracked-files=all")?;

    let tokens: Vec<&str> = output
        .stdout
        .split('\0')
        .filter(|token| !token.is_empty())
        .collect();
    let mut records = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        index += 1;
        if token.len() < 3 {
            continue;
        }
        let (xy, rest) = token.split_at(2);
        // `strip_prefix`, not `trim_start_matches`: exactly one separator
        // space follows the XY code, and a filename that itself legitimately
        // starts with a space must not have it eaten.
        let path = rest.strip_prefix(' ').unwrap_or(rest);
        push_if_in_scope(&mut records, xy, path);
        // A rename/copy record's second half (the original path) arrives as
        // its own NUL-terminated token with no `XY` prefix.
        if (xy.contains('R') || xy.contains('C'))
            && let Some(orig) = tokens.get(index)
        {
            push_if_in_scope(&mut records, xy, orig);
            index += 1;
        }
    }
    Ok(records)
}

fn push_if_in_scope(records: &mut Vec<(String, String)>, xy: &str, path: &str) {
    if path.starts_with(crate::layout::WORKTREE_ROOT) {
        return;
    }
    records.push((xy.to_string(), path.to_string()));
}

/// Resolve the default sentinel set for `main_root`: the same resolved
/// config-layer files config resolution itself reads (never a second
/// hand-listed copy — see [`crate::harness_config::runtime_config_layer_paths`]),
/// unioned with `configured` (repository-relative entries joined onto
/// `main_root`, absolute entries passed through), deduped and sorted.
pub fn resolve_sentinel_paths(
    main_root: &Utf8Path,
    configured: &[String],
) -> crate::Result<Vec<Utf8PathBuf>> {
    let mut paths = crate::harness_config::runtime_config_layer_paths(main_root)?;
    for entry in configured {
        let path = Utf8Path::new(entry);
        paths.push(if path.is_absolute() {
            path.to_path_buf()
        } else {
            main_root.join(path)
        });
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchRepo {
        path: Utf8PathBuf,
    }

    impl Drop for ScratchRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.path.as_std_path());
        }
    }

    fn run(dir: &Utf8Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.as_std_path())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(tag: &str) -> ScratchRepo {
        let base = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let path = base.join(format!(
            "ctx-tripwire-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(path.as_std_path()).expect("clear stale scratch dir");
        }
        std::fs::create_dir_all(path.as_std_path()).expect("mkdir");
        run(&path, &["init", "-q"]);
        run(&path, &["config", "user.email", "test@example.com"]);
        run(&path, &["config", "user.name", "Test"]);
        std::fs::write(path.join("README.md").as_std_path(), b"hello\n").expect("write");
        run(&path, &["add", "."]);
        run(&path, &["commit", "-q", "-m", "init"]);
        ScratchRepo { path }
    }

    fn tripwire(repo: &ScratchRepo, sentinel: Vec<Utf8PathBuf>) -> Tripwire {
        Tripwire::new(repo.path.clone(), sentinel, TripwirePolicy::Park)
    }

    #[test]
    fn clean_repo_two_checkpoints_yield_no_finding() {
        let repo = init_repo("clean");
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        assert!(wire.checkpoint().unwrap().is_none());
    }

    #[test]
    fn tracked_file_modified_between_checkpoints_is_a_finding() {
        let repo = init_repo("tracked-modified");
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(repo.path.join("README.md").as_std_path(), b"changed\n").unwrap();
        let finding = wire.checkpoint().unwrap().expect("finding");
        assert_eq!(finding.paths, vec!["README.md".to_string()]);
        assert_eq!(finding.frame, BEFORE_FIRST_FRAME);
    }

    #[test]
    fn untracked_file_created_is_a_finding() {
        let repo = init_repo("untracked-created");
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(repo.path.join("new-file.txt").as_std_path(), b"x\n").unwrap();
        let finding = wire.checkpoint().unwrap().expect("finding");
        assert_eq!(finding.paths, vec!["new-file.txt".to_string()]);
    }

    #[test]
    fn gitignored_sentinel_modification_is_caught_by_the_sentinel_leg_but_absent_from_status() {
        let repo = init_repo("gitignored-sentinel");
        std::fs::create_dir_all(repo.path.join(".ctx").as_std_path()).unwrap();
        std::fs::write(repo.path.join(".gitignore").as_std_path(), b".ctx/\n").unwrap();
        run(&repo.path, &["add", ".gitignore"]);
        run(&repo.path, &["commit", "-q", "-m", "ignore .ctx"]);
        let sentinel = repo.path.join(".ctx/config.toml");
        std::fs::write(sentinel.as_std_path(), b"a = 1\n").unwrap();

        let mut wire = tripwire(&repo, vec![sentinel.clone()]);
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(sentinel.as_std_path(), b"a = 2\n").unwrap();

        let finding = wire.checkpoint().unwrap().expect("finding");
        assert_eq!(finding.paths, vec![".ctx/config.toml".to_string()]);

        // Prove the same mutation is absent from the status leg alone.
        let status_only = status_records(&repo.path).unwrap();
        assert!(status_only.is_empty());
    }

    #[test]
    fn write_under_worktrees_subtree_is_not_a_finding() {
        let repo = init_repo("worktrees-subtree");
        std::fs::create_dir_all(repo.path.join(".ctx/worktrees/self").as_std_path()).unwrap();
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(
            repo.path
                .join(".ctx/worktrees/self/scratch.txt")
                .as_std_path(),
            b"x\n",
        )
        .unwrap();
        assert!(wire.checkpoint().unwrap().is_none());
    }

    #[test]
    fn rename_reports_both_paths_unquoted_and_intact() {
        let repo = init_repo("rename");
        std::fs::write(repo.path.join("a weird name.txt").as_std_path(), b"x\n").unwrap();
        run(&repo.path, &["add", "."]);
        run(&repo.path, &["commit", "-q", "-m", "add"]);
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        run(&repo.path, &["mv", "a weird name.txt", "renamed name.txt"]);
        let finding = wire.checkpoint().unwrap().expect("finding");
        assert!(finding.paths.contains(&"a weird name.txt".to_string()));
        assert!(finding.paths.contains(&"renamed name.txt".to_string()));
    }

    #[test]
    fn sentinel_created_and_sentinel_deleted_are_both_findings() {
        let repo = init_repo("sentinel-create-delete");
        let sentinel = repo.path.join("sentinel.toml");
        let mut wire = tripwire(&repo, vec![sentinel.clone()]);
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(sentinel.as_std_path(), b"a = 1\n").unwrap();
        let created = wire.checkpoint().unwrap().expect("created finding");
        assert!(created.paths.contains(&"sentinel.toml".to_string()));
        std::fs::remove_file(sentinel.as_std_path()).unwrap();
        let deleted = wire.checkpoint().unwrap().expect("deleted finding");
        assert!(deleted.paths.contains(&"sentinel.toml".to_string()));
    }

    #[test]
    fn one_escape_is_reported_exactly_once_across_three_checkpoints() {
        let repo = init_repo("reported-once");
        let mut wire = tripwire(&repo, Vec::new());
        assert!(wire.checkpoint().unwrap().is_none());
        std::fs::write(repo.path.join("once.txt").as_std_path(), b"x\n").unwrap();
        assert!(wire.checkpoint().unwrap().is_some());
        assert!(wire.checkpoint().unwrap().is_none());
    }

    #[test]
    fn a_two_checkpoint_clean_cycle_issues_exactly_one_git_status_per_checkpoint() {
        let repo = init_repo("snapshot-count");
        let mut wire = tripwire(&repo, Vec::new());
        wire.checkpoint().unwrap();
        wire.checkpoint().unwrap();
        assert_eq!(wire.snapshot_count(), 2);
    }
}
