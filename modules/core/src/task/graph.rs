//! Pure graph computations over a snapshot of task documents (0060).
//!
//! Every function here takes a `BTreeMap<String, TaskDocument>` snapshot
//! (keyed by [`TaskDocument::key`]) and derives facts from it: inverse
//! edges, derived statuses, cycle detection, dangling edges. Relations are
//! stored one-directional (`depends-on`, `parent`); `blocks` and `children`
//! are never stored, only computed here, so the two halves of a relation
//! can never disagree. No I/O, no backend — unit-testable with literals.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{TaskDocument, TaskStatus};

/// A status after derivation: the closed stored set plus `Blocked`, which
/// has no stored representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DerivedStatus {
    Ready,
    Blocked,
    Done,
    Cancelled,
}

impl DerivedStatus {
    /// Whether this status counts as closed for archival/aggregation
    /// purposes (`done` or `cancelled`).
    pub fn is_closed(self) -> bool {
        matches!(self, DerivedStatus::Done | DerivedStatus::Cancelled)
    }
}

/// One resolved edge: the other task's key, title, and current derived
/// status — enough for a consumer to render "blocked by 0049 (ready)"
/// without a second lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolvedEdge {
    pub key: String,
    pub title: String,
    pub status: DerivedStatus,
}

/// A document's relations, fully resolved: stored edges (`depends_on`,
/// `parent`) alongside their computed inverses (`blocks`, `children`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolvedRelations {
    pub depends_on: Vec<ResolvedEdge>,
    pub blocks: Vec<ResolvedEdge>,
    pub parent: Option<ResolvedEdge>,
    pub children: Vec<ResolvedEdge>,
}

/// A key referenced by a relation that does not exist in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DanglingEdge {
    /// The task carrying the dangling reference.
    pub from: String,
    /// The relation field naming the missing key (`depends-on` or `parent`).
    pub field: String,
    /// The missing key.
    pub to: String,
}

/// Both paths that together form a cycle a write refused to create: the
/// path already connecting the target back to the source, and the new edge
/// that would have closed the loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CyclePaths {
    pub path_a: Vec<String>,
    pub path_b: Vec<String>,
}

/// A relation kind graph computations walk. `depends-on` cycles and
/// `parent` cycles are independent — a parent chain forming a loop is a
/// different mistake from a dependency loop, so cycle detection is always
/// scoped to one kind at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    DependsOn,
    Parent,
}

/// Direct children of `key`: every document whose `relations.parent` names
/// it. Computed, not stored.
fn children_of<'a>(
    documents: &'a BTreeMap<String, TaskDocument>,
    key: &str,
) -> Vec<&'a TaskDocument> {
    documents
        .values()
        .filter(|doc| doc.relations.parent.as_deref() == Some(key))
        .collect()
}

/// Direct inbound `depends-on` edges onto `key` — the tasks `blocks`
/// resolves to. Computed, not stored. Public so a backend's dependents
/// sweep (0063.6) can reuse the same edge-finding as `resolved_relations`
/// rather than re-walking `documents` itself.
pub fn blockers_of<'a>(
    documents: &'a BTreeMap<String, TaskDocument>,
    key: &str,
) -> Vec<&'a TaskDocument> {
    documents
        .values()
        .filter(|doc| doc.relations.depends_on.iter().any(|dep| dep == key))
        .collect()
}

/// The derived status of `key`, computed over the whole snapshot.
///
/// A document with children derives its status from them (done only when
/// every child is closed) regardless of its own stored status — the
/// 0010/0051 pattern of a parent with no stored status at all is simply the
/// common case of this rule. A document with no children derives `Blocked`
/// when any `depends-on` target is not `Done`, otherwise its stored status
/// (or `Ready` if none is stored). A missing key, or a status recursion
/// that revisits a key already on the call stack (an already-refused-shape
/// cycle surviving in stored data), derives to `Ready` defensively rather
/// than looping.
pub fn derived_status(documents: &BTreeMap<String, TaskDocument>, key: &str) -> DerivedStatus {
    derived_status_inner(documents, key, &mut HashSet::new())
}

fn derived_status_inner(
    documents: &BTreeMap<String, TaskDocument>,
    key: &str,
    visiting: &mut HashSet<String>,
) -> DerivedStatus {
    let Some(doc) = documents.get(key) else {
        return DerivedStatus::Ready;
    };
    if !visiting.insert(key.to_string()) {
        return DerivedStatus::Ready;
    }
    let children = children_of(documents, key);
    let status = if !children.is_empty() {
        let all_closed = children
            .iter()
            .all(|child| derived_status_inner(documents, &child.key, visiting).is_closed());
        if all_closed {
            DerivedStatus::Done
        } else {
            DerivedStatus::Ready
        }
    } else {
        let blocked = doc.relations.depends_on.iter().any(|dep| {
            !matches!(
                derived_status_inner(documents, dep, visiting),
                DerivedStatus::Done
            )
        });
        if blocked {
            DerivedStatus::Blocked
        } else {
            match doc.status {
                Some(TaskStatus::Done) => DerivedStatus::Done,
                Some(TaskStatus::Cancelled) => DerivedStatus::Cancelled,
                Some(TaskStatus::Ready) | None => DerivedStatus::Ready,
            }
        }
    };
    visiting.remove(key);
    status
}

fn edge(documents: &BTreeMap<String, TaskDocument>, key: &str) -> Option<ResolvedEdge> {
    let doc = documents.get(key)?;
    Some(ResolvedEdge {
        key: doc.key.clone(),
        title: doc.title.clone(),
        status: derived_status(documents, key),
    })
}

/// `key`'s relations fully resolved: stored edges (`depends_on`, `parent`)
/// plus their computed inverses (`blocks`, `children`). A stored edge
/// naming a key absent from the snapshot is silently omitted here — that
/// gap is [`dangling_edges`]'s job to surface, not this function's.
pub fn resolved_relations(
    documents: &BTreeMap<String, TaskDocument>,
    key: &str,
) -> ResolvedRelations {
    let mut relations = ResolvedRelations::default();
    let Some(doc) = documents.get(key) else {
        return relations;
    };
    relations.depends_on = doc
        .relations
        .depends_on
        .iter()
        .filter_map(|dep| edge(documents, dep))
        .collect();
    relations.parent = doc
        .relations
        .parent
        .as_deref()
        .and_then(|parent| edge(documents, parent));
    relations.blocks = blockers_of(documents, key)
        .into_iter()
        .filter_map(|doc| edge(documents, &doc.key))
        .collect();
    relations.children = children_of(documents, key)
        .into_iter()
        .filter_map(|doc| edge(documents, &doc.key))
        .collect();
    relations.blocks.sort_by(|a, b| a.key.cmp(&b.key));
    relations.children.sort_by(|a, b| a.key.cmp(&b.key));
    relations
}

/// Every relation edge in the snapshot pointing at a key that does not
/// exist — never treated as satisfied, only ever reported.
pub fn dangling_edges(documents: &BTreeMap<String, TaskDocument>) -> Vec<DanglingEdge> {
    let mut found = Vec::new();
    for doc in documents.values() {
        for dep in &doc.relations.depends_on {
            if !documents.contains_key(dep) {
                found.push(DanglingEdge {
                    from: doc.key.clone(),
                    field: "depends-on".to_string(),
                    to: dep.clone(),
                });
            }
        }
        if let Some(parent) = &doc.relations.parent
            && !documents.contains_key(parent)
        {
            found.push(DanglingEdge {
                from: doc.key.clone(),
                field: "parent".to_string(),
                to: parent.clone(),
            });
        }
    }
    found
}

/// Walk `edge_kind` edges from `from` looking for `to`, returning the path
/// (`from` through to `to`, each step's own key) if one exists. `parent` has
/// at most one outbound edge per document, so it walks a single chain;
/// `depends-on` has many, so it delegates to a depth-first search over all
/// of them. Both bound the walk to the snapshot's size (a `visited` set) so
/// a corrupt stored cycle cannot loop forever.
fn find_path(
    documents: &BTreeMap<String, TaskDocument>,
    edge_kind: EdgeKind,
    from: &str,
    to: &str,
) -> Option<Vec<String>> {
    match edge_kind {
        EdgeKind::Parent => {
            let mut path = vec![from.to_string()];
            let mut visited: HashSet<String> = HashSet::new();
            visited.insert(from.to_string());
            let mut current = from.to_string();
            loop {
                let doc = documents.get(&current)?;
                let next = doc.relations.parent.clone()?;
                if next == to {
                    path.push(next);
                    return Some(path);
                }
                if !visited.insert(next.clone()) {
                    return None;
                }
                path.push(next.clone());
                current = next;
            }
        }
        EdgeKind::DependsOn => {
            let mut visited: HashSet<String> = HashSet::new();
            visited.insert(from.to_string());
            let mut path = vec![from.to_string()];
            find_path_multi(documents, from, to, &mut visited, &mut path)
        }
    }
}

/// `depends-on` has multiple outbound edges per document, so its path
/// search is a depth-first walk over all of them rather than the single
/// successor chain [`find_path`] follows for `parent`.
fn find_path_multi(
    documents: &BTreeMap<String, TaskDocument>,
    current: &str,
    to: &str,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    let doc = documents.get(current)?;
    for dep in &doc.relations.depends_on {
        if dep == to {
            path.push(dep.clone());
            return Some(path.clone());
        }
        if visited.insert(dep.clone()) {
            path.push(dep.clone());
            if let Some(found) = find_path_multi(documents, dep, to, visited, path) {
                return Some(found);
            }
            path.pop();
        }
    }
    None
}

/// Whether writing an edge `from -> to` of `edge_kind` would create a
/// cycle, given the rest of the snapshot (which does not yet carry this
/// edge). Returns both paths that would together form the cycle: the path
/// already connecting `to` back to `from`, and the new edge itself.
pub fn would_create_cycle(
    documents: &BTreeMap<String, TaskDocument>,
    edge_kind: EdgeKind,
    from: &str,
    to: &str,
) -> Option<CyclePaths> {
    if from == to {
        return Some(CyclePaths {
            path_a: vec![from.to_string()],
            path_b: vec![from.to_string(), to.to_string()],
        });
    }
    let path_a = find_path(documents, edge_kind, to, from)?;
    Some(CyclePaths {
        path_a,
        path_b: vec![from.to_string(), to.to_string()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Relations, Step};

    fn doc(key: &str, status: Option<TaskStatus>) -> TaskDocument {
        TaskDocument {
            schema_version: crate::task::SCHEMA_VERSION.to_string(),
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
            relations: Relations::default(),
            steps: Vec::<Step>::new(),
        }
    }

    fn snapshot(docs: Vec<TaskDocument>) -> BTreeMap<String, TaskDocument> {
        docs.into_iter().map(|d| (d.key.clone(), d)).collect()
    }

    #[test]
    fn inverse_children_and_blocks_are_computed() {
        let mut parent = doc("0010", None);
        let mut child = doc("0010.1", Some(TaskStatus::Ready));
        child.relations.parent = Some("0010".to_string());
        let mut blocked = doc("0050", Some(TaskStatus::Ready));
        blocked.relations.depends_on = vec!["0010.1".to_string()];
        parent.relations = Relations::default();
        let documents = snapshot(vec![parent, child, blocked]);

        let relations = resolved_relations(&documents, "0010");
        assert_eq!(relations.children.len(), 1);
        assert_eq!(relations.children[0].key, "0010.1");

        let relations = resolved_relations(&documents, "0010.1");
        assert_eq!(relations.blocks.len(), 1);
        assert_eq!(relations.blocks[0].key, "0050");
    }

    #[test]
    fn blocked_from_unmet_depends_on() {
        let dependency = doc("0001", Some(TaskStatus::Ready));
        let mut dependent = doc("0002", Some(TaskStatus::Ready));
        dependent.relations.depends_on = vec!["0001".to_string()];
        let documents = snapshot(vec![dependency, dependent]);

        assert_eq!(derived_status(&documents, "0002"), DerivedStatus::Blocked);
    }

    #[test]
    fn ready_once_dependency_is_done() {
        let dependency = doc("0001", Some(TaskStatus::Done));
        let mut dependent = doc("0002", Some(TaskStatus::Ready));
        dependent.relations.depends_on = vec!["0001".to_string()];
        let documents = snapshot(vec![dependency, dependent]);

        assert_eq!(derived_status(&documents, "0002"), DerivedStatus::Ready);
    }

    #[test]
    fn parent_status_derives_done_only_when_all_children_done() {
        let mut parent = doc("0010", None);
        parent.relations = Relations::default();
        let mut child_a = doc("0010.1", Some(TaskStatus::Done));
        child_a.relations.parent = Some("0010".to_string());
        let mut child_b = doc("0010.2", Some(TaskStatus::Ready));
        child_b.relations.parent = Some("0010".to_string());
        let documents = snapshot(vec![parent.clone(), child_a.clone(), child_b.clone()]);
        assert_eq!(derived_status(&documents, "0010"), DerivedStatus::Ready);

        let mut child_b_done = child_b;
        child_b_done.status = Some(TaskStatus::Done);
        let documents = snapshot(vec![parent, child_a, child_b_done]);
        assert_eq!(derived_status(&documents, "0010"), DerivedStatus::Done);
    }

    #[test]
    fn dangling_edges_are_reported_not_satisfied() {
        let mut dependent = doc("0002", Some(TaskStatus::Ready));
        dependent.relations.depends_on = vec!["9999".to_string()];
        let documents = snapshot(vec![dependent]);

        let found = dangling_edges(&documents);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].to, "9999");
        assert_eq!(found[0].field, "depends-on");
        // A dangling dependency is never treated as satisfied: the
        // dependent is blocked, not ready.
        assert_eq!(derived_status(&documents, "0002"), DerivedStatus::Blocked);
    }

    #[test]
    fn cycle_refusal_names_both_paths() {
        let mut a = doc("0001", Some(TaskStatus::Ready));
        a.relations.depends_on = vec!["0002".to_string()];
        let b = doc("0002", Some(TaskStatus::Ready));
        let documents = snapshot(vec![a, b]);

        // 0002 -> 0001 would close a loop, since 0001 already depends on 0002.
        let cycle = would_create_cycle(&documents, EdgeKind::DependsOn, "0002", "0001");
        let cycle = cycle.expect("cycle expected");
        assert_eq!(cycle.path_a, vec!["0001".to_string(), "0002".to_string()]);
        assert_eq!(cycle.path_b, vec!["0002".to_string(), "0001".to_string()]);
    }

    #[test]
    fn no_cycle_when_paths_are_independent() {
        let a = doc("0001", Some(TaskStatus::Ready));
        let b = doc("0002", Some(TaskStatus::Ready));
        let documents = snapshot(vec![a, b]);

        assert!(would_create_cycle(&documents, EdgeKind::DependsOn, "0001", "0002").is_none());
    }

    #[test]
    fn parent_cycle_is_detected() {
        let mut a = doc("0001", None);
        a.relations.parent = Some("0002".to_string());
        let b = doc("0002", None);
        let documents = snapshot(vec![a, b]);

        // 0002's parent becoming 0001 would close a loop.
        let cycle = would_create_cycle(&documents, EdgeKind::Parent, "0002", "0001");
        assert!(cycle.is_some());
    }
}
