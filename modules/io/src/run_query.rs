//! Query-run selection over local trait packages.
//!
//! This owns the IO-bearing inventory load for query starts/inspection. CLI and
//! MCP adapters call this once instead of copying search/gate semantics.
//!
//! Candidate ids and their resolved manifest paths come from the same
//! cross-tier [`crate::inventory::InventoryContext`] explicit-id resolution
//! uses (P439), so a query run and an explicit-id run of the same trait can
//! never disagree about which tier's package wins.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

#[derive(Debug, Clone)]
pub struct LoadedTrait {
    pub trait_ref: ctx_traits_core::Trait,
    pub path: Utf8PathBuf,
    pub trait_root: Utf8PathBuf,
    pub source_kind: String,
    pub source_digest: String,
    pub canonical_digest: String,
    /// Resolved from the package manifest's `[package].status`. The
    /// canonical trait document carries no status field of its own.
    pub status: ctx_traits_core::manifest::PackageStatus,
    /// Resolved from the machine trust store for `canonical_digest`. The
    /// canonical trait document carries no trust field of its own.
    pub trust: ctx_traits_core::r#trait::TrustVerdict,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub status: ctx_traits_core::run_info::RunInfoSelectionStatus,
    pub selection: ctx_traits_core::run_info::RunInfoSelectionSummary,
    pub loaded: Option<LoadedTrait>,
}

pub fn select(
    query: &str,
    context: &crate::inventory::InventoryContext,
) -> crate::Result<Selection> {
    let ids = context.candidate_ids()?;
    let mut loaded_by_id = BTreeMap::new();
    let mut docs = Vec::new();

    for id in ids {
        let Some(resolution) = context.resolve_tiers(&id)? else {
            continue;
        };
        let loaded = load(&resolution.winner.path, &resolution.winner.origin)?;
        docs.push(ctx_traits_core::search::build_search_document(
            &loaded.trait_ref,
            loaded.status.display_name(),
            loaded.trust.display_name(),
        ));
        loaded_by_id.insert(loaded.trait_ref.id.as_str().to_string(), loaded);
    }

    let results = ctx_traits_core::search::search_traits(query, &docs);
    if results.is_empty() {
        return Ok(Selection {
            status: ctx_traits_core::run_info::RunInfoSelectionStatus::NoMatch,
            selection: ctx_traits_core::run_info::RunInfoSelectionSummary {
                status: ctx_traits_core::run_info::RunInfoSelectionStatus::NoMatch,
                query: Some(query.to_string()),
                selected_trait_id: None,
                candidates: Vec::new(),
                reasons: vec!["no search matches in trait source root".to_string()],
            },
            loaded: None,
        });
    }

    let mut candidates = Vec::new();
    let mut runnable = Vec::new();
    for result in results {
        let Some(loaded) = loaded_by_id.get(&result.trait_id) else {
            continue;
        };
        let gates = ctx_traits_core::r#trait::activation::lifecycle_trust_gates_for_check(
            loaded.trait_ref.id.as_str(),
            &loaded.status,
            &loaded.trust,
        );
        if gates.is_empty() {
            runnable.push((result.clone(), loaded.trait_ref.id.as_str().to_string()));
        }
        candidates.push(ctx_traits_core::run_info::RunInfoCandidateSummary {
            trait_id: result.trait_id.clone(),
            name: result.name.clone(),
            score: result.score,
            rank_tier: result.rank_tier,
            gates,
            reasons: result
                .match_reasons
                .iter()
                .map(|reason| format!("{}:{}", reason.field, reason.matched_term))
                .collect(),
        });
    }

    if runnable.is_empty() {
        return Ok(Selection {
            status: ctx_traits_core::run_info::RunInfoSelectionStatus::Blocked,
            selection: ctx_traits_core::run_info::RunInfoSelectionSummary {
                status: ctx_traits_core::run_info::RunInfoSelectionStatus::Blocked,
                query: Some(query.to_string()),
                selected_trait_id: None,
                candidates,
                reasons: vec!["matched traits are blocked by lifecycle/trust gates".to_string()],
            },
            loaded: None,
        });
    }

    let top = &runnable[0].0;
    let ambiguous = runnable
        .iter()
        .skip(1)
        .any(|(result, _)| result.rank_tier == top.rank_tier && result.score == top.score);
    if ambiguous {
        return Ok(Selection {
            status: ctx_traits_core::run_info::RunInfoSelectionStatus::Ambiguous,
            selection: ctx_traits_core::run_info::RunInfoSelectionSummary {
                status: ctx_traits_core::run_info::RunInfoSelectionStatus::Ambiguous,
                query: Some(query.to_string()),
                selected_trait_id: None,
                candidates,
                reasons: vec!["multiple runnable traits share the top query rank".to_string()],
            },
            loaded: None,
        });
    }

    let selected_id = runnable[0].1.clone();
    let loaded =
        loaded_by_id
            .remove(&selected_id)
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: selected_id.clone(),
                source: std::io::Error::other(format!(
                    "selected trait {selected_id} was not loaded"
                )),
            })?;
    Ok(Selection {
        status: ctx_traits_core::run_info::RunInfoSelectionStatus::Selected,
        selection: ctx_traits_core::run_info::RunInfoSelectionSummary {
            status: ctx_traits_core::run_info::RunInfoSelectionStatus::Selected,
            query: Some(query.to_string()),
            selected_trait_id: Some(selected_id),
            candidates,
            reasons: Vec::new(),
        },
        loaded: Some(loaded),
    })
}

fn load(path: &Utf8Path, source_kind: &str) -> crate::Result<LoadedTrait> {
    let (trait_ref, trait_root, source_digest, canonical_digest) =
        crate::run::load_trait(path.as_str())?;
    let (status, trust) = crate::lifecycle::resolve_named(
        &trait_root,
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;
    Ok(LoadedTrait {
        trait_ref,
        path: path.to_path_buf(),
        trait_root,
        source_kind: source_kind.to_string(),
        source_digest: source_digest.as_str().to_string(),
        canonical_digest: canonical_digest.as_str().to_string(),
        status,
        trust,
    })
}
