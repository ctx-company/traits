//! Pure installed-harness model catalog discovery and resolution.
//!
//! Core owns catalog probe plans, parsing, normalization, aliases, and tier
//! preference. Host adapters execute the planned command and return its text.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

/// A side-effect-free command plan for reading one installed harness catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProbePlan {
    pub argv: Vec<String>,
}

/// One normalized model exposed by an installed harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub canonical_id: String,
    pub aliases: Vec<String>,
}

/// A normalized, canonical-ID-sorted harness model catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub harness_kind: String,
    pub entries: Vec<CatalogEntry>,
}

/// The model request remaining after runtime-profile precedence is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selector<'a> {
    Explicit(&'a str),
}

impl Selector<'_> {
    pub fn evidence(self) -> String {
        match self {
            Self::Explicit(model) => format!("model:{model}"),
        }
    }
}

/// Catalog availability supplied by the host after executing a probe plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogAccess<'a> {
    Available(&'a Catalog),
    Unavailable(&'a str),
}

/// Stable explanation for a successful model resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionReason {
    ExactCatalogMatch,
    AliasResolution,
    ExplicitUnverified,
}

impl ResolutionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactCatalogMatch => "exact-catalog-match",
            Self::AliasResolution => "alias-resolution",
            Self::ExplicitUnverified => "explicit-unverified",
        }
    }
}

/// A concrete model selected for one harness assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub requested: String,
    pub canonical_model: String,
    pub reason: ResolutionReason,
}

/// Deterministic model-catalog failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    #[error("harness kind {harness_kind:?} does not support installed model catalog discovery")]
    UnsupportedCapability { harness_kind: String },

    #[error("model catalog line {line} is not a canonical model id: {value:?}")]
    InvalidCatalogLine { line: usize, value: String },

    #[error("model catalog for harness kind {harness_kind:?} is empty")]
    EmptyCatalog { harness_kind: String },

    #[error("model catalog belongs to harness kind {catalog_kind:?}, not {requested_kind:?}")]
    CatalogKindMismatch {
        requested_kind: String,
        catalog_kind: String,
    },

    #[error("model alias {selector:?} is ambiguous: {candidates}")]
    AliasCollision {
        selector: String,
        candidates: String,
    },

    #[error("model {selector:?} is not present in the {harness_kind:?} catalog")]
    ModelNotFound {
        harness_kind: String,
        selector: String,
    },
}

/// Return the installed-catalog command supported by a harness kind.
pub fn catalog_probe_plan(harness_kind: &str) -> Result<CatalogProbePlan, Error> {
    match harness_kind {
        "opencode" => Ok(CatalogProbePlan {
            argv: vec!["models".to_string()],
        }),
        _ => Err(Error::UnsupportedCapability {
            harness_kind: harness_kind.to_string(),
        }),
    }
}

/// Parse and normalize captured catalog output for a supported harness kind.
pub fn parse_catalog(harness_kind: &str, output: &str) -> Result<Catalog, Error> {
    catalog_probe_plan(harness_kind)?;
    let mut canonical_ids = BTreeSet::new();
    for (index, raw) in output.lines().enumerate() {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        if value.contains(char::is_whitespace)
            || !value.contains('/')
            || value.split('/').any(str::is_empty)
        {
            return Err(Error::InvalidCatalogLine {
                line: index + 1,
                value: value.to_string(),
            });
        }
        canonical_ids.insert(value.to_string());
    }
    if canonical_ids.is_empty() {
        return Err(Error::EmptyCatalog {
            harness_kind: harness_kind.to_string(),
        });
    }
    let entries = canonical_ids
        .into_iter()
        .map(|canonical_id| CatalogEntry {
            aliases: canonical_id
                .rsplit_once('/')
                .map(|(_, alias)| vec![alias.to_string()])
                .unwrap_or_default(),
            canonical_id,
        })
        .collect();
    Ok(Catalog {
        harness_kind: harness_kind.to_string(),
        entries,
    })
}

/// Resolve one effective selector within its already-selected harness.
pub fn resolve_model(
    harness_kind: &str,
    selector: Selector<'_>,
    catalog: CatalogAccess<'_>,
) -> Result<Resolution, Error> {
    let requested = selector.evidence();
    let catalog = match catalog {
        CatalogAccess::Available(catalog) => catalog,
        CatalogAccess::Unavailable(_reason) => {
            let Selector::Explicit(model) = selector;
            return Ok(Resolution {
                requested,
                canonical_model: model.to_string(),
                reason: ResolutionReason::ExplicitUnverified,
            });
        }
    };
    if catalog.harness_kind != harness_kind {
        return Err(Error::CatalogKindMismatch {
            requested_kind: harness_kind.to_string(),
            catalog_kind: catalog.harness_kind.clone(),
        });
    }

    let Selector::Explicit(model) = selector;
    resolve_explicit(harness_kind, model, requested, catalog)
}

fn resolve_explicit(
    harness_kind: &str,
    model: &str,
    requested: String,
    catalog: &Catalog,
) -> Result<Resolution, Error> {
    if catalog
        .entries
        .iter()
        .any(|entry| entry.canonical_id == model)
    {
        return Ok(Resolution {
            requested,
            canonical_model: model.to_string(),
            reason: ResolutionReason::ExactCatalogMatch,
        });
    }

    let alias_index = alias_index(catalog);
    match alias_index.get(model).map(Vec::as_slice) {
        Some([canonical_model]) => Ok(Resolution {
            requested,
            canonical_model: canonical_model.clone(),
            reason: ResolutionReason::AliasResolution,
        }),
        Some(candidates) if candidates.len() > 1 => Err(Error::AliasCollision {
            selector: model.to_string(),
            candidates: candidates.join(", "),
        }),
        _ => Err(Error::ModelNotFound {
            harness_kind: harness_kind.to_string(),
            selector: model.to_string(),
        }),
    }
}

fn alias_index(catalog: &Catalog) -> BTreeMap<&str, Vec<String>> {
    let mut aliases = BTreeMap::<&str, Vec<String>>::new();
    for entry in &catalog.entries {
        for alias in &entry.aliases {
            aliases
                .entry(alias)
                .or_default()
                .push(entry.canonical_id.clone());
        }
    }
    aliases
}
