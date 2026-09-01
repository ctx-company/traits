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
    pub variants_block: NamedBlock,
    pub ports_block: NamedBlock,
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
        LibraryTrustState::Blocked => {
            format!("{} ({verified}/{total} verified)", presentation.word)
        }
        LibraryTrustState::Partial => format!("{} ({verified}/{total})", presentation.word),
        _ => presentation.word.to_string(),
    };
    ValueSegment::toned(text, role(presentation.role))
}

fn failure(text: impl Into<String>) -> TraitPreview {
    let value = vec![ValueSegment::toned(text, StateRole::Danger)];
    TraitPreview {
        trait_block: NamedBlock {
            heading: "trait".to_string(),
            rows: vec![KeyValueRow {
                key: "name".to_string(),
                value: value.clone(),
            }],
        },
        lede: None,
        facts_block: NamedBlock {
            heading: "facts".to_string(),
            rows: vec![KeyValueRow {
                key: "status".to_string(),
                value: value.clone(),
            }],
        },
        variants_block: unavailable_block("variants", value.clone()),
        ports_block: unavailable_block("ports", value),
    }
}

fn unavailable_block(heading: &str, value: Vec<ValueSegment>) -> NamedBlock {
    NamedBlock {
        heading: heading.to_string(),
        rows: vec![KeyValueRow {
            key: String::new(),
            value,
        }],
    }
}

fn summary_block(heading: &str, rows: impl Iterator<Item = (String, String)>) -> NamedBlock {
    let rows: Vec<_> = rows
        .map(|(key, value)| KeyValueRow {
            key,
            value: vec![ValueSegment::neutral(value)],
        })
        .collect();
    NamedBlock {
        heading: heading.to_string(),
        rows: if rows.is_empty() {
            vec![KeyValueRow {
                key: String::new(),
                value: vec![ValueSegment::neutral("none")],
            }]
        } else {
            rows
        },
    }
}

pub fn project(detail: Option<&Result<LibraryDetailResolution, String>>) -> TraitPreview {
    match detail {
        None => failure("loading"),
        Some(Err(reason)) => failure(format!("unavailable: {reason}")),
        Some(Ok(LibraryDetailResolution::Resolved {
            display_identity,
            lede,
            version,
            canonical_digest,
            trust_state,
            verified,
            total,
            agents,
            variants,
            ports,
            ..
        })) => {
            let trust = state_value(*trust_state, *verified, *total);
            TraitPreview {
                trait_block: NamedBlock {
                    heading: "trait".to_string(),
                    rows: vec![KeyValueRow {
                        key: "name".to_string(),
                        value: vec![
                            ValueSegment::neutral(display_identity),
                            ValueSegment::dot(),
                            trust.clone(),
                        ],
                    }],
                },
                lede: Some(lede.clone()),
                facts_block: NamedBlock {
                    heading: "facts".to_string(),
                    rows: vec![
                        KeyValueRow {
                            key: "version".to_string(),
                            value: vec![ValueSegment::neutral(version)],
                        },
                        KeyValueRow {
                            key: "trust".to_string(),
                            value: vec![trust],
                        },
                        KeyValueRow {
                            key: "digest".to_string(),
                            value: vec![ValueSegment::neutral(canonical_digest)],
                        },
                        KeyValueRow {
                            key: "agents".to_string(),
                            value: vec![ValueSegment::neutral(if agents.is_empty() {
                                "none".to_string()
                            } else {
                                agents.join(" · ")
                            })],
                        },
                    ],
                },
                variants_block: summary_block(
                    "variants",
                    variants
                        .iter()
                        .map(|variant| (variant.key.clone(), variant.summary.clone())),
                ),
                ports_block: summary_block(
                    "ports",
                    ports
                        .iter()
                        .map(|port| (port.id.clone(), port.description.clone())),
                ),
            }
        }
        Some(Ok(LibraryDetailResolution::SourceOnly { id, source_path })) => TraitPreview {
            trait_block: NamedBlock {
                heading: "trait".to_string(),
                rows: vec![KeyValueRow {
                    key: "name".to_string(),
                    value: vec![
                        ValueSegment::neutral(id),
                        ValueSegment::dot(),
                        ValueSegment::toned("source-only", StateRole::Neutral),
                    ],
                }],
            },
            lede: None,
            facts_block: NamedBlock {
                heading: "facts".to_string(),
                rows: vec![KeyValueRow {
                    key: "source".to_string(),
                    value: vec![ValueSegment::neutral(source_path)],
                }],
            },
            variants_block: unavailable_block(
                "variants",
                vec![ValueSegment::toned("source-only", StateRole::Neutral)],
            ),
            ports_block: unavailable_block(
                "ports",
                vec![ValueSegment::toned("source-only", StateRole::Neutral)],
            ),
        },
        Some(Ok(LibraryDetailResolution::Unreadable { id, path, error })) => {
            failure(format!("unreadable · {id} · {path} · {error}"))
        }
        Some(Ok(LibraryDetailResolution::Missing)) => failure("missing"),
        Some(Ok(LibraryDetailResolution::Stale { .. })) => failure("stale: selection changed"),
        Some(Ok(LibraryDetailResolution::Refused { reason })) => {
            failure(format!("refused: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_preview_keeps_full_digest_and_empty_agents_are_data() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let preview = project(Some(&Ok(LibraryDetailResolution::Resolved {
            display_identity: "fixture".to_string(),
            name: "Fixture".to_string(),
            lede: "served summary".to_string(),
            version: "1.0.0".to_string(),
            canonical_digest: digest.clone(),
            trust_state: LibraryTrustState::AllVerified,
            verified: 1,
            total: 1,
            agents: vec![],
            variants: vec![],
            ports: vec![],
            status: String::new(),
            procedure: ctx_traits_io::library::LibraryProcedureShape::Unknown,
            source_drift_checked: false,
            drift: String::new(),
            source_path: String::new(),
            source_excerpt: vec![],
            trust_reason: String::new(),
            trust_stale: false,
            has_trust_record: false,
            trust_record: None,
        })));
        assert_eq!(preview.trait_block.heading, "trait");
        assert_eq!(preview.facts_block.heading, "facts");
        assert_eq!(preview.lede.as_deref(), Some("served summary"));
        assert_eq!(preview.facts_block.rows[2].value[0].text, digest);
        assert_eq!(preview.facts_block.rows[3].value[0].text, "none");
        assert_eq!(preview.trait_block.rows[0].value[0].role, None);
        assert_eq!(
            preview.trait_block.rows[0].value[2].role,
            Some(StateRole::Ok)
        );
        assert_eq!(preview.variants_block.rows[0].value[0].text, "none");
        assert_eq!(preview.ports_block.rows[0].value[0].text, "none");
    }

    #[test]
    fn unavailable_details_keep_all_blocks_typed() {
        let preview = project(Some(&Ok(LibraryDetailResolution::Refused {
            reason: "limit".to_string(),
        })));
        for block in [
            &preview.trait_block,
            &preview.facts_block,
            &preview.variants_block,
            &preview.ports_block,
        ] {
            assert_eq!(block.rows[0].value[0].role, Some(StateRole::Danger));
        }
    }

    #[test]
    fn resolved_preview_preserves_served_variant_and_port_order() {
        let preview = project(Some(&Ok(LibraryDetailResolution::Resolved {
            display_identity: "fixture".to_string(),
            name: "Fixture".to_string(),
            lede: "served summary".to_string(),
            version: "1.0.0".to_string(),
            canonical_digest: "sha256:fixture".to_string(),
            trust_state: LibraryTrustState::Unreviewed,
            verified: 0,
            total: 2,
            agents: vec![],
            variants: vec![
                ctx_traits_io::library::LibraryVariantSummary {
                    key: "first".to_string(),
                    summary: "first summary".to_string(),
                },
                ctx_traits_io::library::LibraryVariantSummary {
                    key: "second".to_string(),
                    summary: "second summary".to_string(),
                },
            ],
            ports: vec![
                ctx_traits_io::library::LibraryPortSummary {
                    id: "input".to_string(),
                    description: "input description".to_string(),
                },
                ctx_traits_io::library::LibraryPortSummary {
                    id: "output".to_string(),
                    description: "output description".to_string(),
                },
            ],
            status: String::new(),
            procedure: ctx_traits_io::library::LibraryProcedureShape::Unknown,
            source_drift_checked: false,
            drift: String::new(),
            source_path: String::new(),
            source_excerpt: vec![],
            trust_reason: String::new(),
            trust_stale: false,
            has_trust_record: false,
            trust_record: None,
        })));
        assert_eq!(preview.variants_block.rows[0].key, "first");
        assert_eq!(preview.variants_block.rows[1].key, "second");
        assert_eq!(preview.ports_block.rows[0].key, "input");
        assert_eq!(preview.ports_block.rows[1].key, "output");
    }
}
