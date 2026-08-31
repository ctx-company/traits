//! Rule 10's rail as a gpui-free model, mirroring `frame_list.rs`'s split:
//! grouping, ordering, the dot rule and the brightness invariant are all
//! assertable in plain `#[test]`s here, with no gpui `App` needed.
//! `rail_view.rs` paints exactly this model.
//!
//! # The brightness invariant
//!
//! [`RailRepo`] carries no `active`/`bright` field, the same shape
//! `frame_list.rs` documents for `FrameRow`: only [`Rail::active`] knows
//! which index (if any) is active, and [`Rail::is_active`] is the one place
//! that answers the question. A caller holding one `RailRepo` cannot make
//! it active.

use std::collections::BTreeMap;

use ctx_traits_io::center::CenterPublicRow;

use crate::frame_list::DotTone;
use crate::placeholders;
use crate::tokens;

/// The rail's own two-segment name projection (goal 3), deliberately not
/// `run_row::repo_label_for`: that is the compact row's one-segment label
/// with two sibling-owned consumers (the run row, the spawn picker), while
/// the rail and its footer share this different, two-segment form. Pure and
/// total — no `std::fs`, no `env`, no cwd, no `canonicalize` — so a
/// `repo_key` is always non-empty on the wire and this never panics or
/// fabricates a name.
pub fn repo_display_name(repo_key: &str, repo_path: &str) -> String {
    let segments: Vec<&str> = repo_path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.len() {
        0 | 1 => repo_key.to_string(),
        n => format!("{}/{}", segments[n - 2], segments[n - 1]),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailRepo {
    pub repo_key: String,
    pub name: String,
    pub live: bool,
}

/// One row per distinct repository of the accepted center row model, in a
/// deterministic total order derived from center data alone. No row carries
/// its own `active`/`bright` state — see the brightness invariant above.
pub struct Rail {
    repos: Vec<RailRepo>,
    active: Option<usize>,
    owner: &'static str,
    space: Option<String>,
    stale: bool,
}

impl Rail {
    pub fn repos(&self) -> &[RailRepo] {
        &self.repos
    }

    pub fn is_active(&self, index: usize) -> bool {
        self.active == Some(index)
    }

    pub fn owner(&self) -> &'static str {
        self.owner
    }

    /// The active repository's projected name, or `None` when there is no
    /// active repository — including when the active identity no longer
    /// matches any repository in the current list (goal 6's "the active
    /// repository's last row ends" case). The same string [`Rail::name_color`]
    /// and the row itself render, by construction, so the two cannot drift.
    pub fn space(&self) -> Option<&str> {
        self.space.as_deref()
    }

    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// The dot rule, stated once: active rows are `Bright`; a non-active
    /// repository with at least one live accepted row is `Accent`;
    /// everything else is `Idle`. No `ok`/`warn`/`danger` aggregation, no
    /// count, no per-row status rollup.
    pub fn dot(&self, index: usize) -> DotTone {
        if self.is_active(index) {
            DotTone::Bright
        } else if self.repos[index].live {
            DotTone::Accent
        } else {
            DotTone::Idle
        }
    }

    /// Rule 4: at most one bright name, the active row; every other name is
    /// plain `text`.
    pub fn name_color(&self, index: usize) -> u32 {
        if self.is_active(index) {
            tokens::TEXT_BRIGHT
        } else {
            tokens::TEXT
        }
    }
}

/// Project a [`Rail`] from the accepted center row model. `rows` must come
/// from the *unfiltered* keyed map (the same doctrine
/// `Dashboard::repositories`/`contains_session`/`row_liveness` already
/// follow) — a view filter must not shrink the repository set.
pub fn project<'a>(
    rows: impl Iterator<Item = &'a CenterPublicRow>,
    active_repo_key: Option<&str>,
    stale: bool,
) -> Rail {
    // `repo_key`/`repo_path` are taken verbatim from the wire row — never
    // recomputed, never derived from cwd or a "current repo" notion — so an
    // `adhoc-…` key survives untouched. First-seen `repo_path` per key.
    let mut folded: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for row in rows {
        let entry = folded
            .entry(row.repo_key.clone())
            .or_insert_with(|| (row.repo_path.clone(), false));
        entry.1 |= row.live;
    }

    let mut repos: Vec<RailRepo> = folded
        .into_iter()
        .map(|(repo_key, (repo_path, live))| RailRepo {
            name: repo_display_name(&repo_key, &repo_path),
            repo_key,
            live,
        })
        .collect();
    // Total and deterministic, insertion-independent even when two
    // repositories project the same display name.
    repos.sort_by(|left, right| (&left.name, &left.repo_key).cmp(&(&right.name, &right.repo_key)));

    // Load-bearing for goal 6: the active identity is only active if it
    // still matches a repository in the current list, so when the active
    // repository's last row ends, the active row and the footer's space
    // line disappear together. No fallback promotion.
    let active = active_repo_key.and_then(|key| repos.iter().position(|r| r.repo_key == key));
    let space = active.map(|i| repos[i].name.clone());

    Rail {
        repos,
        active,
        owner: placeholders::OWNER_HANDLE,
        space,
        stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::run_summary::RunSummary;

    fn row(repo_key: &str, repo_path: &str, ledger_path: &str, live: bool) -> CenterPublicRow {
        CenterPublicRow {
            summary: RunSummary::unreadable(ledger_path.to_string(), "unused".to_string()),
            repo_key: repo_key.to_string(),
            repo_path: repo_path.to_string(),
            ledger_path: ledger_path.to_string(),
            live,
            modified_epoch_secs: 0,
        }
    }

    #[test]
    fn name_projection_takes_the_final_two_segments() {
        assert_eq!(
            repo_display_name("k", "/Users/o/ctx-company/traits"),
            "ctx-company/traits"
        );
    }

    #[test]
    fn name_projection_falls_back_to_repo_key_for_one_segment() {
        assert_eq!(repo_display_name("k", "/traits"), "k");
    }

    #[test]
    fn name_projection_falls_back_to_repo_key_for_empty_path() {
        assert_eq!(repo_display_name("k", ""), "k");
    }

    #[test]
    fn name_projection_drops_empty_segments_from_a_trailing_slash() {
        assert_eq!(
            repo_display_name("k", "/Users/o/ctx-company/traits/"),
            "ctx-company/traits"
        );
    }

    #[test]
    fn three_rows_sharing_a_repo_key_project_one_rail_row() {
        let rows = [
            row("repo-a", "/x/traits", "/x/traits/a", true),
            row("repo-a", "/x/traits", "/x/traits/b", false),
            row("repo-a", "/x/traits", "/x/traits/c", false),
        ];
        let rail = project(rows.iter(), None, false);
        assert_eq!(rail.repos().len(), 1);
    }

    #[test]
    fn repositories_sharing_a_basename_but_not_a_key_stay_separate() {
        let rows = [
            row("repo-a", "/a/traits", "/a/traits/x", false),
            row("repo-b", "/b/traits", "/b/traits/x", false),
        ];
        let rail = project(rows.iter(), None, false);
        assert_eq!(rail.repos().len(), 2);
    }

    #[test]
    fn an_adhoc_key_round_trips_verbatim() {
        let rows = [row("adhoc-1234", "", "/adhoc-1234/x", false)];
        let rail = project(rows.iter(), None, false);
        assert_eq!(rail.repos()[0].repo_key, "adhoc-1234");
        assert_eq!(rail.repos()[0].name, "adhoc-1234");
    }

    #[test]
    fn ordering_is_insertion_independent() {
        let forward = [
            row("repo-a", "/x/aaa", "/x/aaa/1", false),
            row("repo-b", "/x/bbb", "/x/bbb/1", false),
        ];
        let backward = [
            row("repo-b", "/x/bbb", "/x/bbb/1", false),
            row("repo-a", "/x/aaa", "/x/aaa/1", false),
        ];
        let a = project(forward.iter(), None, false);
        let b = project(backward.iter(), None, false);
        assert_eq!(a.repos(), b.repos());
    }

    #[test]
    fn same_display_name_orders_by_repo_key() {
        let rows = [
            row("repo-b", "/x/dup/traits", "/x/dup/traits/1", false),
            row("repo-a", "/y/dup/traits", "/y/dup/traits/1", false),
        ];
        let rail = project(rows.iter(), None, false);
        assert_eq!(rail.repos()[0].repo_key, "repo-a");
        assert_eq!(rail.repos()[1].repo_key, "repo-b");
    }

    #[test]
    fn dot_and_brightness_rules() {
        let rows = [
            row("repo-a", "/x/aaa", "/x/aaa/1", true),
            row("repo-b", "/x/bbb", "/x/bbb/1", false),
        ];
        let rail = project(rows.iter(), Some("repo-a"), false);
        assert_eq!(rail.dot(0), DotTone::Bright);
        assert_eq!(rail.name_color(0), tokens::TEXT_BRIGHT);
        assert_eq!(rail.dot(1), DotTone::Idle);
        assert_eq!(rail.name_color(1), tokens::TEXT);
    }

    #[test]
    fn non_active_live_repository_is_accent() {
        let rows = [
            row("repo-a", "/x/aaa", "/x/aaa/1", true),
            row("repo-b", "/x/bbb", "/x/bbb/1", true),
        ];
        let rail = project(rows.iter(), Some("repo-a"), false);
        assert_eq!(rail.dot(1), DotTone::Accent);
    }

    #[test]
    fn at_most_one_bright_row_ever() {
        for active in [None, Some("repo-a"), Some("repo-b"), Some("missing")] {
            let rows = [
                row("repo-a", "/x/aaa", "/x/aaa/1", true),
                row("repo-b", "/x/bbb", "/x/bbb/1", true),
            ];
            let rail = project(rows.iter(), active, false);
            let bright_count = (0..rail.repos().len())
                .filter(|i| rail.is_active(*i))
                .count();
            assert!(bright_count <= 1);
        }
    }

    #[test]
    fn footer_space_matches_the_active_row_name() {
        let rows = [row("repo-a", "/x/traits", "/x/traits/1", false)];
        let rail = project(rows.iter(), Some("repo-a"), false);
        assert_eq!(rail.space(), Some(rail.repos()[0].name.as_str()));
    }

    #[test]
    fn footer_space_is_none_with_no_active_repository() {
        let rows = [row("repo-a", "/x/traits", "/x/traits/1", false)];
        let rail = project(rows.iter(), None, false);
        assert_eq!(rail.space(), None);
    }

    #[test]
    fn footer_space_is_none_when_the_active_repository_has_no_rows_left() {
        let rows = [row("repo-b", "/x/bbb", "/x/bbb/1", false)];
        let rail = project(rows.iter(), Some("repo-a"), false);
        assert_eq!(rail.space(), None);
        assert_eq!(
            (0..rail.repos().len())
                .filter(|i| rail.is_active(*i))
                .count(),
            0
        );
    }

    #[test]
    fn owner_routes_through_the_placeholder_entry() {
        let rail = project(std::iter::empty(), None, false);
        assert_eq!(rail.owner(), placeholders::OWNER_HANDLE);
        assert_eq!(rail.owner(), "Oskar Cieslik");
    }
}
