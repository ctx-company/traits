//! `ctx traits run --task` (0195): board-driven dispatch queue building and
//! the per-task 0144 auto-close hook. The queue orchestration loop itself
//! (drive each queued task, halt/continue, report outcomes) lives in
//! [`crate::app::run::handle_task_queue_run`], which drives every queued
//! task through [`crate::app::run::drive_session`] — the same preflight,
//! worktree, and merge path a single `--task-dispatch` run uses. This
//! module owns only the parts specific to a queue: turning `--task` values
//! into a flat ordered list of task keys, and closing a landed task by
//! reusing [`super::task_proposals::close_disposition`] and
//! [`super::task_checks::run_checks`] — never a second implementation of
//! either.

use camino::Utf8Path;
use ctx_traits_core::task::provider::{TaskProvider, TaskProviderMut, TaskUpdate};
use ctx_traits_core::task::{CheckOutcome, CheckRecord, Closure, TaskStatus};

/// One `--task` value resolved into the queue it names: a bare/dotted task
/// key stays itself; a charter (any task with children) expands to its
/// non-closed children, sorted by their dotted ordinal ascending — never
/// lexicographically, so `0001.10` sorts after `0001.9`. Multiple `--task`
/// values (comma-separated or repeated — clap normalizes both spellings to
/// one `Vec` before this ever runs) flatten into one queue in the order
/// given.
pub(crate) fn expand_task_queue(
    provider: &dyn TaskProvider,
    entries: &[String],
) -> Result<Vec<String>, String> {
    let mut queue = Vec::new();
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = provider
            .resolve(trimmed)
            .map_err(|error| format!("task board error resolving {trimmed:?}: {error}"))?
            .ok_or_else(|| format!("no task {trimmed:?} on the board"))?;
        let resolved = provider
            .get(&key)
            .map_err(|error| format!("task board error reading {key:?}: {error}"))?
            .ok_or_else(|| format!("no task {key:?} on the board"))?;
        if resolved.relations.children.is_empty() {
            queue.push(key);
            continue;
        }
        let mut children: Vec<String> = resolved
            .relations
            .children
            .iter()
            .filter(|child| !child.status.is_closed())
            .map(|child| child.key.clone())
            .collect();
        children.sort_by_key(|child_key| dotted_ordinal(child_key));
        queue.extend(children);
    }
    Ok(queue)
}

/// The numeric ordinal after the last `.` in a dotted child key
/// (`"0001.10"` -> `10`) — the sort key charter expansion orders by. A key
/// with no dot, or a non-numeric suffix, sorts first (`0`); neither ever
/// occurs for an actual child key, this is only defensive.
fn dotted_ordinal(key: &str) -> u64 {
    key.rsplit('.')
        .next()
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Attempt the 0144 auto-close for `key` after a landed queue run, reusing
/// the exact primitives the dashboard's own close path uses
/// ([`super::task_proposals::resolve_auto_close_policy`],
/// [`super::task_proposals::close_disposition`],
/// [`super::task_checks::run_checks`]) rather than a parallel
/// implementation. Returns `true` only when the write actually closed the
/// task — `false` covers every reason it did not (no policy resolves, the
/// disposition stayed a `Proposal` a human must confirm, no landed revision
/// to run checks or record against, or the write itself failed) since a
/// queue continuing past an unclosed task is not itself an error.
pub(crate) fn auto_close_landed_task(
    board_dir: &Utf8Path,
    key: &str,
    repo_root: &Utf8Path,
    revision: Option<&str>,
) -> bool {
    let Some(sha) = revision else {
        return false;
    };
    let provider = ctx_traits_io::task_files::FilesTaskBoard::open_read_write(board_dir.to_owned());
    let Ok(Some(resolved)) = provider.get(key) else {
        return false;
    };
    let document = &resolved.document;
    let config_default =
        ctx_traits_io::harness_config::resolve_runtime_config(repo_root)
            .ok()
            .and_then(|config| config.effective_auto_close());
    let Some(policy) =
        super::task_proposals::resolve_auto_close_policy(document.auto_close, config_default)
    else {
        return false;
    };
    let results = if document.checks.is_empty() {
        None
    } else {
        Some(
            match super::task_checks::run_checks(&document.checks, repo_root, sha) {
                Ok(records) => records,
                Err(unrunnable) => vec![CheckRecord {
                    name: "(check set)".to_string(),
                    command: String::new(),
                    outcome: CheckOutcome::Unrunnable,
                    detail: unrunnable.reason,
                }],
            },
        )
    };
    let disposition =
        super::task_proposals::close_disposition(policy, &document.checks, results.as_deref());
    let checks = match disposition {
        super::task_proposals::CloseDisposition::AutoClose { checks } => checks,
        super::task_proposals::CloseDisposition::Proposal { .. } => return false,
    };
    let closure = Closure {
        mode: policy,
        commit: Some(sha.to_string()),
        checks,
    };
    provider
        .update(
            key,
            TaskUpdate {
                status: Some(TaskStatus::Done),
                expected_digest: Some(resolved.digest),
                set_closure: Some(closure),
                ..Default::default()
            },
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::provider::{ProviderError, ResolvedTask, SyncReport, TaskSummary};
    use ctx_traits_core::task::{Relations, TaskDocument};
    use ctx_traits_core::task::graph::{DerivedStatus, ResolvedEdge, ResolvedRelations};
    use std::collections::BTreeMap;

    /// A minimal in-memory [`TaskProvider`] fixture — no filesystem — so
    /// queue expansion is provable without a real board directory.
    struct FixtureProvider(BTreeMap<String, TaskDocument>);

    impl TaskProvider for FixtureProvider {
        fn resolve(&self, task_value: &str) -> Result<Option<String>, ProviderError> {
            Ok(self.0.contains_key(task_value).then(|| task_value.to_string()))
        }

        fn get(&self, key: &str) -> Result<Option<ResolvedTask>, ProviderError> {
            let Some(doc) = self.0.get(key) else {
                return Ok(None);
            };
            let children = self
                .0
                .values()
                .filter(|child| child.relations.parent.as_deref() == Some(key))
                .map(|child| ResolvedEdge {
                    key: child.key.clone(),
                    title: child.title.clone(),
                    status: if matches!(child.status, Some(TaskStatus::Done) | Some(TaskStatus::Cancelled)) {
                        DerivedStatus::Done
                    } else {
                        DerivedStatus::Ready
                    },
                })
                .collect();
            Ok(Some(ResolvedTask {
                document: doc.clone(),
                derived_status: DerivedStatus::Ready,
                relations: ResolvedRelations {
                    children,
                    ..Default::default()
                },
                archived: false,
                open_steps: Vec::new(),
                digest: "sha256:fixture".to_string(),
            }))
        }

        fn list(&self, _include_archived: bool) -> Result<Vec<TaskSummary>, ProviderError> {
            Ok(Vec::new())
        }

        fn sync(&self) -> Result<SyncReport, ProviderError> {
            Ok(SyncReport::default())
        }
    }

    fn doc(key: &str, status: Option<TaskStatus>, parent: Option<&str>) -> TaskDocument {
        TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: key.to_string(),
            title: format!("title {key}"),
            status,
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: String::new(),
            scope: String::new(),
            validation: String::new(),
            relations: Relations {
                parent: parent.map(str::to_string),
                ..Default::default()
            },
            steps: Vec::new(),
            checks: Vec::new(),
            auto_close: None,
            closure: None,
        }
    }

    #[test]
    fn bare_task_passes_through_unexpanded() {
        let provider = FixtureProvider(BTreeMap::from([(
            "0001".to_string(),
            doc("0001", Some(TaskStatus::Ready), None),
        )]));
        let queue = expand_task_queue(&provider, &["0001".to_string()]).unwrap();
        assert_eq!(queue, vec!["0001".to_string()]);
    }

    #[test]
    fn charter_expands_to_non_closed_children_in_numeric_ordinal_order() {
        let documents = BTreeMap::from([
            ("0001".to_string(), doc("0001", None, None)),
            (
                "0001.2".to_string(),
                doc("0001.2", Some(TaskStatus::Ready), Some("0001")),
            ),
            (
                "0001.10".to_string(),
                doc("0001.10", Some(TaskStatus::Ready), Some("0001")),
            ),
            (
                "0001.9".to_string(),
                doc("0001.9", Some(TaskStatus::Ready), Some("0001")),
            ),
            (
                "0001.5".to_string(),
                doc("0001.5", Some(TaskStatus::Done), Some("0001")),
            ),
        ]);
        let provider = FixtureProvider(documents);
        let queue = expand_task_queue(&provider, &["0001".to_string()]).unwrap();
        // 0001.5 is done -> excluded; 0001.10 sorts after 0001.9, not before
        // it lexicographically.
        assert_eq!(
            queue,
            vec!["0001.2".to_string(), "0001.9".to_string(), "0001.10".to_string()]
        );
    }

    #[test]
    fn comma_and_repeated_entries_flatten_into_one_ordered_queue() {
        let documents = BTreeMap::from([
            ("0001".to_string(), doc("0001", Some(TaskStatus::Ready), None)),
            ("0002".to_string(), doc("0002", Some(TaskStatus::Ready), None)),
        ]);
        let provider = FixtureProvider(documents);
        // `--task 0001,0002` and `--task 0001 --task 0002` both normalize to
        // this same flat `Vec` before reaching `expand_task_queue` (clap's
        // `value_delimiter` does the comma split) — this proves the queue
        // builder treats that normalized list the same regardless of shape.
        let queue =
            expand_task_queue(&provider, &["0001".to_string(), "0002".to_string()]).unwrap();
        assert_eq!(queue, vec!["0001".to_string(), "0002".to_string()]);
    }

    #[test]
    fn unknown_task_refuses_before_expansion() {
        let provider = FixtureProvider(BTreeMap::new());
        let error = expand_task_queue(&provider, &["9999".to_string()]).unwrap_err();
        assert!(error.contains("9999"));
    }
}
