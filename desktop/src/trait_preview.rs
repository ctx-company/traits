//! Pure Traits preview projection from one center-served detail answer.

use ctx_traits_io::library::{
    LibraryDetailResolution, LibraryPresentationFace, LibraryTrustRole, LibraryTrustState,
};

use crate::preview::{KeyValueRow, NamedBlock, ValueSegment};
use crate::run_row::StateRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitPreview {
    pub trait_block: NamedBlock,
    pub lede: Option<String>,
    pub facts_block: NamedBlock,
}

fn role(role: LibraryTrustRole) -> StateRole {
    match role {
        LibraryTrustRole::SettledGood => StateRole::Ok,
        LibraryTrustRole::Danger => StateRole::Danger,
        LibraryTrustRole::Warn => StateRole::Warn,
        LibraryTrustRole::Neutral => StateRole::Neutral,
    }
}

fn state_value(state: LibraryTrustState, verified: usize, total: usize) -> ValueSegment {
    let presentation = state.presentation(LibraryPresentationFace::Desktop);
    let text = match state {
        LibraryTrustState::Blocked => format!("{} ({verified}/{total} verified)", presentation.word),
        LibraryTrustState::Partial => format!("{} ({verified}/{total})", presentation.word),
        _ => presentation.word.to_string(),
    };
    ValueSegment::toned(text, role(presentation.role))
}

fn failure(text: impl Into<String>) -> TraitPreview {
    let value = vec![ValueSegment::toned(text, StateRole::Danger)];
    TraitPreview {
        trait_block: NamedBlock { heading: "trait".to_string(), rows: vec![KeyValueRow { key: "name".to_string(), value: value.clone() }] },
        lede: None,
        facts_block: NamedBlock { heading: "facts".to_string(), rows: vec![KeyValueRow { key: "status".to_string(), value }] },
    }
}

pub fn project(detail: Option<&Result<LibraryDetailResolution, String>>) -> TraitPreview {
    match detail {
        None => failure("loading"),
        Some(Err(reason)) => failure(format!("unavailable: {reason}")),
        Some(Ok(LibraryDetailResolution::Resolved { display_identity, lede, version, canonical_digest, trust_state, verified, total, agents, .. })) => {
            let trust = state_value(*trust_state, *verified, *total);
            TraitPreview {
                trait_block: NamedBlock {
                    heading: "trait".to_string(),
                    rows: vec![KeyValueRow { key: "name".to_string(), value: vec![ValueSegment::neutral(display_identity), ValueSegment::dot(), trust.clone()] }],
                },
                lede: Some(lede.clone()),
                facts_block: NamedBlock {
                    heading: "facts".to_string(),
                    rows: vec![
                        KeyValueRow { key: "version".to_string(), value: vec![ValueSegment::neutral(version)] },
                        KeyValueRow { key: "trust".to_string(), value: vec![trust] },
                        KeyValueRow { key: "digest".to_string(), value: vec![ValueSegment::neutral(canonical_digest)] },
                        KeyValueRow { key: "agents".to_string(), value: vec![ValueSegment::neutral(if agents.is_empty() { "none".to_string() } else { agents.join(" · ") })] },
                    ],
                },
            }
        }
        Some(Ok(LibraryDetailResolution::SourceOnly { id, source_path })) => TraitPreview {
            trait_block: NamedBlock { heading: "trait".to_string(), rows: vec![KeyValueRow { key: "name".to_string(), value: vec![ValueSegment::neutral(id), ValueSegment::dot(), ValueSegment::toned("source-only", StateRole::Neutral)] }] },
            lede: None,
            facts_block: NamedBlock { heading: "facts".to_string(), rows: vec![KeyValueRow { key: "source".to_string(), value: vec![ValueSegment::neutral(source_path)] }] },
        },
        Some(Ok(LibraryDetailResolution::Unreadable { id, path, error })) => failure(format!("unreadable · {id} · {path} · {error}")),
        Some(Ok(LibraryDetailResolution::Missing)) => failure("missing"),
        Some(Ok(LibraryDetailResolution::Stale { .. })) => failure("stale: selection changed"),
        Some(Ok(LibraryDetailResolution::Refused { reason })) => failure(format!("refused: {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_preview_keeps_full_digest_and_empty_agents_are_data() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let preview = project(Some(&Ok(LibraryDetailResolution::Resolved { display_identity: "fixture".to_string(), name: "Fixture".to_string(), lede: "served summary".to_string(), version: "1.0.0".to_string(), canonical_digest: digest.clone(), trust_state: LibraryTrustState::AllVerified, verified: 1, total: 1, agents: vec![] })));
        assert_eq!(preview.trait_block.heading, "trait");
        assert_eq!(preview.facts_block.heading, "facts");
        assert_eq!(preview.lede.as_deref(), Some("served summary"));
        assert_eq!(preview.facts_block.rows[2].value[0].text, digest);
        assert_eq!(preview.facts_block.rows[3].value[0].text, "none");
        assert_eq!(preview.trait_block.rows[0].value[0].role, None);
        assert_eq!(preview.trait_block.rows[0].value[2].role, Some(StateRole::Ok));
    }
}
