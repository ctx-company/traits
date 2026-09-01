//! Traits-pane projection assembled only from the center's served library answer.

use std::collections::HashSet;

use ctx_traits_io::center::LibraryWireResult;
use ctx_traits_io::library::{LibraryProvenance, LibraryResolution, LibraryRow, TierWire};

use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
use crate::placeholders;
use crate::rail::repo_display_name;
use crate::run_row::{StatePresentation, StateRole};
use crate::screen_header::ScreenHeader;

pub enum LibraryState {
    Loading,
    Accepted {
        answer: LibraryWireResult,
        stale: Option<String>,
    },
    Failed(String),
}

/// Fold a served-library request without discarding the last accepted answer
/// when a refresh is refused or fails.
pub fn fold_library_result(
    state: &LibraryState,
    result: Result<LibraryWireResult, String>,
) -> LibraryState {
    match result {
        Ok(answer) => LibraryState::Accepted {
            answer,
            stale: None,
        },
        Err(reason) => match state {
            LibraryState::Accepted { answer, .. } => LibraryState::Accepted {
                answer: answer.clone(),
                stale: Some(reason),
            },
            LibraryState::Loading | LibraryState::Failed(_) => LibraryState::Failed(reason),
        },
    }
}

/// The visual `skill-lock` label names aggregated package-local `trait.lock`
/// evidence. This only presents the center-served result; it never resolves it.
pub fn provenance_text(provenance: &LibraryProvenance) -> Option<String> {
    provenance.pinned.then(|| {
        format!(
            "skill-lock @ {} · {} digests",
            short_digest(provenance.sha.as_deref().unwrap_or_default()),
            provenance.digest_count
        )
    })
}

fn short_digest(digest: &str) -> String {
    digest
        .strip_prefix("sha256:")
        .unwrap_or(digest)
        .chars()
        .take(8)
        .collect()
}

/// Counts the rows the future authored section must render: one per family,
/// standalone repo-authored member, repo-authored unreadable row, and source-only package.
pub fn authored_row_count(resolution: &LibraryResolution) -> usize {
    let mut resolved = HashSet::new();
    let mut other_rows = 0;
    for row in &resolution.rows {
        match row {
            LibraryRow::Resolved(member) if member.tier == TierWire::RepoAuthored => {
                resolved.insert(
                    member
                        .family_key
                        .as_deref()
                        .unwrap_or(&member.id)
                        .to_string(),
                );
            }
            LibraryRow::Unreadable { tier, .. } if *tier == TierWire::RepoAuthored => {
                other_rows += 1;
            }
            LibraryRow::SourceOnly { .. } => other_rows += 1,
            LibraryRow::Resolved(_) | LibraryRow::Unreadable { .. } => {}
        }
    }
    resolved.len() + other_rows
}

pub fn traits_header(state: &LibraryState) -> ScreenHeader {
    match state {
        LibraryState::Loading => ScreenHeader {
            title: "traits".to_string(),
            summary: "loading trait library".to_string(),
        },
        LibraryState::Failed(reason) => ScreenHeader {
            title: "traits unavailable".to_string(),
            summary: reason.clone(),
        },
        LibraryState::Accepted { answer, stale } => {
            let summary = placeholders::traits_summary(
                authored_row_count(&answer.resolution),
                answer.resolution.provenance.pinned,
            );
            ScreenHeader {
                title: format!(
                    "traits — authored in {}",
                    repo_display_name(&answer.repo_key, &answer.repo_path)
                ),
                summary: stale.as_deref().map_or(summary.clone(), |reason| {
                    format!("stale: {reason} — {summary}")
                }),
            }
        }
    }
}

pub fn traits_bar(state: &LibraryState) -> BottomBar {
    let (word, role, detail) = match state {
        LibraryState::Accepted {
            answer,
            stale: None,
        } if answer.resolution.provenance.pinned => (
            "pinned",
            StateRole::Ok,
            provenance_text(&answer.resolution.provenance)
                .into_iter()
                .collect(),
        ),
        LibraryState::Accepted {
            answer,
            stale: None,
        } => (
            "unpinned",
            StateRole::Danger,
            vec![
                answer
                    .resolution
                    .provenance
                    .error
                    .clone()
                    .unwrap_or_else(|| "provenance unavailable".to_string()),
            ],
        ),
        LibraryState::Loading => (
            "unavailable",
            StateRole::Danger,
            vec!["loading trait library".to_string()],
        ),
        LibraryState::Failed(reason) => ("unavailable", StateRole::Danger, vec![reason.clone()]),
        LibraryState::Accepted {
            stale: Some(reason),
            ..
        } => (
            "unavailable",
            StateRole::Danger,
            vec![format!("stale: {reason}")],
        ),
    };
    BottomBar {
        state: StatePresentation { word, role },
        detail,
        actions: vec![BarAction {
            id: BarActionId::AuthorTrait,
            label: placeholders::AUTHOR_TRAIT.label.to_string(),
            tone: ActionTone::Primary,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::r#trait::TrustVerdict;
    use ctx_traits_io::library::{LibraryMember, LibraryTrustState};

    fn answer(rows: Vec<LibraryRow>, pinned: bool, digest_count: usize) -> LibraryWireResult {
        LibraryWireResult {
            repo_key: "repo".to_string(),
            repo_path: "/work/acme/repo".to_string(),
            resolution: LibraryResolution {
                rows,
                provenance: LibraryProvenance {
                    pinned,
                    sha: Some("sha256:123456789abcdef".to_string()),
                    digest_count,
                    error: (!pinned).then(|| "lock unavailable".to_string()),
                },
            },
        }
    }

    fn member(id: &str, family_key: Option<&str>) -> LibraryMember {
        LibraryMember {
            id: id.to_string(),
            version: "1".to_string(),
            schema_version: "1".to_string(),
            status: "draft".to_string(),
            trust: TrustVerdict::Unreviewed,
            trust_state: LibraryTrustState::Unreviewed,
            canonical_digest: "sha256:fixture".to_string(),
            source_path: String::new(),
            trait_root: String::new(),
            tier: TierWire::RepoAuthored,
            origin: "trait-id".to_string(),
            family: family_key.map(str::to_string),
            family_key: family_key.map(str::to_string),
            variant: None,
            shadow: None,
            name: id.to_string(),
            summary: String::new(),
        }
    }

    #[test]
    fn pinned_bar_uses_the_single_provenance_presentation() {
        let state = LibraryState::Accepted {
            answer: answer(vec![], true, 5),
            stale: None,
        };
        let bar = traits_bar(&state);
        assert_eq!(
            bar.state,
            StatePresentation {
                word: "pinned",
                role: StateRole::Ok
            }
        );
        assert_eq!(bar.detail, vec!["skill-lock @ 12345678 · 5 digests"]);
        assert_eq!(bar.actions[0].id, BarActionId::AuthorTrait);
        assert_eq!(bar.actions[0].label, placeholders::AUTHOR_TRAIT.label);
    }

    #[test]
    fn failed_provenance_never_claims_a_pin_or_partial_evidence() {
        let state = LibraryState::Accepted {
            answer: answer(vec![], false, 5),
            stale: None,
        };
        let bar = traits_bar(&state);
        assert_eq!(bar.state.role, StateRole::Danger);
        assert_eq!(bar.detail, vec!["lock unavailable"]);
        assert!(!bar.detail.join(" ").contains("12345678"));
    }

    #[test]
    fn header_uses_served_identity_and_unavailable_states_claim_no_repository() {
        let header = traits_header(&LibraryState::Accepted {
            answer: answer(vec![], true, 5),
            stale: None,
        });
        assert_eq!(header.title, "traits — authored in acme/repo");
        for state in [
            LibraryState::Loading,
            LibraryState::Failed("scope refused".to_string()),
        ] {
            assert!(!traits_header(&state).title.contains("authored in"));
            assert!(!traits_header(&state).summary.is_empty());
        }
    }

    #[test]
    fn rejected_refresh_keeps_the_accepted_answer_stale() {
        let previous = LibraryState::Accepted {
            answer: answer(vec![], true, 5),
            stale: None,
        };
        let folded = fold_library_result(&previous, Err("center down".to_string()));
        let header = traits_header(&folded);
        assert!(header.title.contains("acme/repo"));
        assert!(header.summary.contains("stale: center down"));
        assert_eq!(traits_bar(&folded).detail, vec!["stale: center down"]);
    }

    #[test]
    fn source_only_rows_are_counted_without_affecting_pinned_provenance() {
        let state = LibraryState::Accepted {
            answer: answer(
                vec![LibraryRow::SourceOnly {
                    id: "source".to_string(),
                    source_path: "/repo/source".to_string(),
                }],
                true,
                9,
            ),
            stale: None,
        };
        assert_eq!(
            authored_row_count(match &state {
                LibraryState::Accepted { answer, .. } => &answer.resolution,
                _ => unreachable!(),
            }),
            1
        );
        assert_eq!(traits_bar(&state).state.word, "pinned");
        assert!(
            traits_header(&state)
                .summary
                .starts_with("1 authored trait")
        );
    }

    #[test]
    fn authored_count_groups_variants_and_keeps_it_distinct_from_digest_count() {
        let resolution = LibraryResolution {
            rows: vec![
                LibraryRow::Resolved(Box::new(member("one", Some("family")))),
                LibraryRow::Resolved(Box::new(member("two", Some("family")))),
                LibraryRow::Resolved(Box::new(member("standalone", None))),
                LibraryRow::Unreadable {
                    id: "bad".to_string(),
                    path: "/repo/bad".to_string(),
                    error: "unreadable".to_string(),
                    tier: TierWire::RepoAuthored,
                    origin: "trait-id".to_string(),
                },
                LibraryRow::SourceOnly {
                    id: "source".to_string(),
                    source_path: "/repo/source".to_string(),
                },
            ],
            provenance: LibraryProvenance {
                pinned: true,
                sha: Some("sha256:123456789".to_string()),
                digest_count: 9,
                error: None,
            },
        };
        assert_eq!(authored_row_count(&resolution), 4);
        assert_ne!(
            authored_row_count(&resolution),
            resolution.provenance.digest_count
        );
    }

    #[test]
    fn digest_shortening_is_display_only_and_total() {
        assert_eq!(short_digest("sha256:123456789"), "12345678");
        assert_eq!(short_digest("short"), "short");
    }
}
