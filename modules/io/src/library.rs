//! Repository-scoped trait library resolution.
//!
//! This is the single filesystem join used by list/reporting clients and the
//! center. It deliberately receives an already-read trust document so one
//! answer cannot combine verdicts from different store revisions.

use camino::Utf8PathBuf;
use ctx_traits_core::digest::{Digest, canonical_json};
use ctx_traits_core::r#trait::TrustVerdict;
use serde::{Deserialize, Serialize};

use crate::inventory::{InventoryContext, Tier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryTrustState {
    AllVerified,
    Blocked,
    Partial,
    Unreviewed,
    Moved,
    Unreadable,
    SourceOnly,
    Orphaned,
}

impl LibraryTrustState {
    pub fn label(self, verified: usize, total: usize) -> String {
        match self {
            Self::AllVerified => "verified".to_string(),
            Self::Blocked => format!("blocked ({verified}/{total} verified)"),
            Self::Partial => format!("partial ({verified}/{total})"),
            Self::Unreviewed => "unreviewed".to_string(),
            Self::Moved => "moved".to_string(),
            Self::Unreadable => "unreadable".to_string(),
            Self::SourceOnly => "source-only".to_string(),
            Self::Orphaned => "orphaned".to_string(),
        }
    }
}

/// Semantic colour role for a library trust answer. Faces translate this small
/// vocabulary to their own token types without reinterpreting trust evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryTrustRole {
    SettledGood,
    Danger,
    Warn,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryPresentationFace {
    Tui,
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryTrustPresentation {
    pub word: &'static str,
    pub role: LibraryTrustRole,
}

impl LibraryTrustState {
    pub fn presentation(self, face: LibraryPresentationFace) -> LibraryTrustPresentation {
        let word = match (self, face) {
            (Self::AllVerified, LibraryPresentationFace::Desktop) => "trusted",
            (Self::AllVerified, LibraryPresentationFace::Tui) => "verified",
            (Self::Blocked, _) => "blocked",
            (Self::Partial, _) => "partial",
            (Self::Unreviewed, _) => "unreviewed",
            (Self::Moved, _) => "moved",
            (Self::Unreadable, _) => "unreadable",
            (Self::SourceOnly, _) => "source-only",
            (Self::Orphaned, _) => "orphaned",
        };
        let role = match self {
            Self::AllVerified => LibraryTrustRole::SettledGood,
            Self::Blocked | Self::Unreadable => LibraryTrustRole::Danger,
            Self::Partial | Self::Moved | Self::Orphaned => LibraryTrustRole::Warn,
            Self::Unreviewed | Self::SourceOnly => LibraryTrustRole::Neutral,
        };
        LibraryTrustPresentation { word, role }
    }
}

/// Stable address of one served library row. It is deliberately not a list
/// index: a refresh may reorder rows while this still identifies the member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryDetailSelector {
    pub trait_id: String,
    pub canonical_digest: Option<String>,
    pub member: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)] // wire result; one instance per response
pub enum LibraryDetailResolution {
    Resolved {
        display_identity: String,
        name: String,
        lede: String,
        version: String,
        canonical_digest: String,
        trust_state: LibraryTrustState,
        verified: usize,
        total: usize,
        agents: Vec<String>,
        variants: Vec<LibraryVariantSummary>,
        ports: Vec<LibraryPortSummary>,
        status: String,
        procedure: LibraryProcedureShape,
        /// Locked-package drift is not evaluated by this endpoint yet.
        source_drift_checked: bool,
        drift: String,
        source_path: String,
        source_excerpt: Vec<String>,
        trust_reason: String,
        trust_stale: bool,
        has_trust_record: bool,
        trust_record: Option<crate::trust::TrustReportRow>,
    },
    SourceOnly {
        id: String,
        source_path: String,
    },
    Unreadable {
        id: String,
        path: String,
        error: String,
    },
    Missing,
    Stale {
        current_digest: Option<String>,
    },
    Refused {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum LibraryProcedureShape {
    Sequence(Vec<(String, String)>),
    GuidanceOnly,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryVariantSummary {
    pub key: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryPortSummary {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryMember {
    pub id: String,
    pub version: String,
    pub schema_version: String,
    pub status: String,
    pub trust: TrustVerdict,
    pub trust_state: LibraryTrustState,
    pub canonical_digest: String,
    pub source_path: String,
    pub trait_root: String,
    pub tier: TierWire,
    pub origin: String,
    pub family: Option<String>,
    /// Actual `[family.variant]` key, never the synthetic `default` selector.
    pub family_key: Option<String>,
    pub variant: Option<String>,
    pub shadow: Option<String>,
    pub name: String,
    pub summary: String,
    /// Declared trait agent ids, retained from the decode already performed for
    /// this inventory row.
    #[serde(default)]
    pub agents: Vec<String>,
    pub record: Option<crate::trust::TrustReportRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TierWire {
    RepoAuthored,
    RepoVendored,
    UserGlobal,
    BuiltIn,
}

impl From<Tier> for TierWire {
    fn from(value: Tier) -> Self {
        match value {
            Tier::RepoAuthored => Self::RepoAuthored,
            Tier::RepoVendored => Self::RepoVendored,
            Tier::UserGlobal => Self::UserGlobal,
            Tier::BuiltIn => Self::BuiltIn,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum LibraryRow {
    Resolved(Box<LibraryMember>),
    Unreadable {
        id: String,
        path: String,
        error: String,
        tier: TierWire,
        origin: String,
    },
    SourceOnly {
        id: String,
        source_path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryProvenance {
    pub pinned: bool,
    pub sha: Option<String>,
    pub digest_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryResolution {
    pub rows: Vec<LibraryRow>,
    pub orphans: Vec<crate::trust::TrustReportRow>,
    pub provenance: LibraryProvenance,
}

/// Aggregate current member states conservatively. A historical block wins
/// even when the current bytes have moved; moved approvals only matter once
/// no current member is verified.
pub fn family_aggregate(members: &[LibraryMember]) -> (LibraryTrustState, usize, usize) {
    let total = members.len();
    if total == 0 {
        return (LibraryTrustState::Unreviewed, 0, 0);
    }
    let verified = members
        .iter()
        .filter(|member| member.trust == TrustVerdict::Verified)
        .count();
    if members
        .iter()
        .any(|member| member.trust_state == LibraryTrustState::Blocked)
    {
        return (LibraryTrustState::Blocked, verified, total);
    }
    if members
        .iter()
        .any(|member| member.trust_state == LibraryTrustState::Unreadable)
    {
        return (LibraryTrustState::Unreadable, verified, total);
    }
    if verified == total {
        return (LibraryTrustState::AllVerified, verified, total);
    }
    if verified > 0 {
        return (LibraryTrustState::Partial, verified, total);
    }
    if members
        .iter()
        .any(|member| member.trust_state == LibraryTrustState::Moved)
    {
        return (LibraryTrustState::Moved, verified, total);
    }
    (LibraryTrustState::Unreviewed, verified, total)
}

pub fn resolve_library(
    context: &InventoryContext,
    document: &crate::trust::Document,
) -> crate::Result<LibraryResolution> {
    let mut rows = Vec::new();
    for id in context.candidate_ids()? {
        let Some(resolution) = context.resolve_tiers(&id)? else {
            continue;
        };
        let candidate = resolution.winner;
        let shadow = resolution
            .shadowed
            .first()
            .map(|candidate| candidate.origin.clone());
        let root = crate::layout::package_root_for_manifest(&candidate.path);
        let family = match root {
            Some(root) => crate::family_manifest::read_family_table(
                &crate::layout::package_manifest_path(root),
            )?,
            None => None,
        };
        if let (Some(root), Some(table)) = (root, family) {
            for (key, variant) in table.variants {
                push_member(
                    &mut rows,
                    &id,
                    root.join(variant.relative_path),
                    &candidate,
                    shadow.clone(),
                    Some(id.clone()),
                    Some(key),
                    document,
                )?;
            }
        } else {
            push_member(
                &mut rows,
                &id,
                candidate.path.clone(),
                &candidate,
                shadow,
                None,
                None,
                document,
            )?;
        }
    }
    let present: std::collections::BTreeSet<_> = rows
        .iter()
        .filter_map(|row| match row {
            LibraryRow::Resolved(member) => Some(member.id.clone()),
            LibraryRow::Unreadable { id, .. } => Some(id.clone()),
            LibraryRow::SourceOnly { .. } => None,
        })
        .collect();
    for package in crate::discovery::trait_authoring_packages(context.repo_root_for_paths())? {
        if !present.contains(&package.trait_id) {
            rows.push(LibraryRow::SourceOnly {
                id: package.trait_id,
                source_path: package.source_path.to_string(),
            });
        }
    }
    let current: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            LibraryRow::Resolved(member) => {
                Some((member.id.clone(), member.canonical_digest.clone()))
            }
            _ => None,
        })
        .collect();
    let orphans = crate::trust::classify_records(document, &current)
        .into_iter()
        .filter(|record| record.freshness == crate::trust::TrustFreshness::Orphaned)
        .collect();
    let provenance = provenance(&rows);
    Ok(LibraryResolution {
        rows,
        orphans,
        provenance,
    })
}

/// Resolve one selected library member without performing the whole-inventory
/// `candidate_ids` scan used by the compact list endpoint.
pub fn resolve_library_detail(
    context: &InventoryContext,
    document: &crate::trust::Document,
    selector: &LibraryDetailSelector,
) -> crate::Result<LibraryDetailResolution> {
    let Some(resolution) = context.resolve_tiers(&selector.trait_id)? else {
        return source_only_detail(context, selector);
    };
    let candidate = resolution.winner;
    let root = crate::layout::package_root_for_manifest(&candidate.path);
    let family = root
        .as_ref()
        .map(|root| {
            crate::family_manifest::read_family_table(&crate::layout::package_manifest_path(root))
        })
        .transpose()?
        .flatten();
    let (display_identity, paths) = match (root, family) {
        (Some(root), Some(table)) => {
            let Some((default_key, default)) = table.variant("default") else {
                return Err(crate::Error::Usage {
                    message: "family default is absent".to_string(),
                });
            };
            let mut paths = vec![(
                Some(default_key.to_string()),
                root.join(&default.relative_path),
            )];
            paths.extend(
                table
                    .variants
                    .iter()
                    .filter(|(key, _)| key.as_str() != default_key)
                    .map(|(key, variant)| {
                        (Some(key.to_string()), root.join(&variant.relative_path))
                    }),
            );
            (selector.trait_id.clone(), paths)
        }
        _ => (
            selector.trait_id.clone(),
            vec![(None, candidate.path.clone())],
        ),
    };
    let selected_index = paths.iter().position(|(key, _)| key == &selector.member);
    let Some(selected_index) = selected_index else {
        return Ok(LibraryDetailResolution::Stale {
            current_digest: None,
        });
    };
    let mut members = Vec::new();
    for (key, path) in &paths {
        match crate::run::load_trait(path.as_str()) {
            Ok((trait_ref, trait_root, _, digest)) => {
                let trust = crate::lifecycle::resolve_trust_verdict_for_trait_in(
                    document,
                    trait_ref.id.as_str(),
                    digest.as_str(),
                );
                let current = document.record_for_current(trait_ref.id.as_str(), digest.as_str());
                let trust_state = match trust {
                    TrustVerdict::Verified => LibraryTrustState::AllVerified,
                    TrustVerdict::Blocked => LibraryTrustState::Blocked,
                    TrustVerdict::Unreviewed
                        if current.is_some_and(|record| {
                            record.state == crate::trust::TrustState::Blocked
                        }) =>
                    {
                        LibraryTrustState::Blocked
                    }
                    TrustVerdict::Unreviewed if current.is_some() => LibraryTrustState::Moved,
                    TrustVerdict::Unreviewed => LibraryTrustState::Unreviewed,
                };
                members.push((
                    key.clone(),
                    path.clone(),
                    trait_ref,
                    trait_root,
                    digest,
                    trust,
                    trust_state,
                ));
            }
            Err(error) => {
                return Ok(LibraryDetailResolution::Unreadable {
                    id: selector.trait_id.clone(),
                    path: path.to_string(),
                    error: error.to_string(),
                });
            }
        }
    }
    let Some((_, selected_path, trait_ref, trait_root, digest, _, _)) = members.get(selected_index)
    else {
        return Ok(LibraryDetailResolution::Missing);
    };
    if selector.canonical_digest.as_deref() != Some(digest.as_str()) {
        return Ok(LibraryDetailResolution::Stale {
            current_digest: Some(digest.as_str().to_string()),
        });
    }
    let aggregate_members: Vec<LibraryMember> = members
        .iter()
        .map(
            |(_, _, trait_ref, trait_root, digest, trust, trust_state)| LibraryMember {
                id: trait_ref.id.as_str().to_string(),
                version: trait_ref.version.as_str().to_string(),
                schema_version: trait_ref.schema_version.as_str().to_string(),
                // `family_aggregate` uses only trust fields; avoid another
                // package-status read while constructing this local evidence.
                status: String::new(),
                trust: *trust,
                trust_state: *trust_state,
                canonical_digest: digest.as_str().to_string(),
                source_path: String::new(),
                trait_root: trait_root.to_string(),
                tier: candidate.tier.into(),
                origin: candidate.origin.clone(),
                family: None,
                family_key: None,
                variant: trait_ref.variant.clone(),
                shadow: None,
                name: trait_ref.id.as_str().to_string(),
                summary: trait_ref.effective_summary().to_string(),
                agents: trait_ref
                    .agents
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect(),
                record: document
                    .record_for_current(trait_ref.id.as_str(), digest.as_str())
                    .map(|record| crate::trust::TrustReportRow {
                        trait_id: Some(trait_ref.id.as_str().to_string()),
                        digest: record.digest.clone(),
                        current_digest: Some(digest.as_str().to_string()),
                        state: record.state,
                        freshness: if record.digest == digest.as_str() {
                            crate::trust::TrustFreshness::Current
                        } else {
                            crate::trust::TrustFreshness::Stale
                        },
                        updated_at: record.updated_at.clone(),
                        reason: record.reason.clone(),
                        seq: record.seq,
                        superseded: false,
                    }),
            },
        )
        .collect();
    let (trust_state, verified, total) = family_aggregate(&aggregate_members);
    let variants = members
        .iter()
        .filter_map(|(key, _, trait_ref, _, _, _, _)| {
            key.as_ref().map(|key| LibraryVariantSummary {
                key: key.clone(),
                summary: trait_ref.effective_summary().to_string(),
            })
        })
        .collect();
    let ports = trait_ref
        .ports
        .iter()
        .map(|port| LibraryPortSummary {
            id: port.id.clone(),
            description: port.description.clone(),
        })
        .collect();
    let trust_record = document
        .record_for_current(trait_ref.id.as_str(), digest.as_str())
        .map(|record| crate::trust::TrustReportRow {
            trait_id: Some(trait_ref.id.as_str().to_string()),
            digest: record.digest.clone(),
            current_digest: Some(digest.as_str().to_string()),
            state: record.state,
            freshness: if record.digest == digest.as_str() {
                crate::trust::TrustFreshness::Current
            } else {
                crate::trust::TrustFreshness::Stale
            },
            updated_at: record.updated_at.clone(),
            reason: record.reason.clone(),
            seq: record.seq,
            superseded: false,
        });
    let procedure = match &trait_ref.procedure {
        None => LibraryProcedureShape::GuidanceOnly,
        Some(procedure) => LibraryProcedureShape::Sequence(
            procedure
                .sequence
                .iter()
                .map(|item| {
                    (
                        item.id.clone().unwrap_or_else(|| "(unnamed)".to_string()),
                        item.kind
                            .map(sequence_kind_label)
                            .map(str::to_string)
                            .unwrap_or_else(|| "(no kind)".to_string()),
                    )
                })
                .collect(),
        ),
    };
    let source_path = editable_source(selected_path);
    let source_excerpt = read_source_excerpt(&source_path, 40);
    Ok(LibraryDetailResolution::Resolved {
        display_identity,
        name: trait_ref.name.as_str().to_string(),
        lede: trait_ref.effective_summary().to_string(),
        version: trait_ref.version.as_str().to_string(),
        canonical_digest: digest.as_str().to_string(),
        trust_state,
        verified,
        total,
        agents: trait_ref
            .agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect(),
        variants,
        ports,
        status: crate::lifecycle::resolve_package_status(trait_root)?
            .display_name()
            .to_string(),
        procedure,
        source_drift_checked: false,
        drift: "unverified".to_string(),
        source_path: source_path.to_string(),
        source_excerpt,
        trust_reason: trust_record
            .as_ref()
            .and_then(|record| record.reason.clone())
            .unwrap_or_default(),
        trust_stale: trust_record
            .as_ref()
            .is_some_and(|record| record.digest != digest.as_str()),
        has_trust_record: trust_record.is_some(),
        trust_record,
    })
}

fn source_only_detail(
    context: &InventoryContext,
    selector: &LibraryDetailSelector,
) -> crate::Result<LibraryDetailResolution> {
    let package = crate::discovery::trait_authoring_packages(context.repo_root_for_paths())?
        .into_iter()
        .find(|package| package.trait_id == selector.trait_id);
    Ok(match package {
        Some(package) => LibraryDetailResolution::SourceOnly {
            id: package.trait_id,
            source_path: package.source_path.to_string(),
        },
        None => LibraryDetailResolution::Missing,
    })
}

#[allow(clippy::too_many_arguments)] // cohesive row-assembly params; a struct would only scatter them
fn push_member(
    rows: &mut Vec<LibraryRow>,
    id: &str,
    path: Utf8PathBuf,
    candidate: &crate::inventory::Candidate,
    shadow: Option<String>,
    family: Option<String>,
    family_key: Option<String>,
    document: &crate::trust::Document,
) -> crate::Result<()> {
    match crate::run::load_trait(path.as_str()) {
        Ok((trait_ref, trait_root, _, digest)) => {
            let trust = crate::lifecycle::resolve_trust_verdict_for_trait_in(
                document,
                trait_ref.id.as_str(),
                digest.as_str(),
            );
            let current = document.record_for_current(trait_ref.id.as_str(), digest.as_str());
            let trust_state = match trust {
                TrustVerdict::Verified => LibraryTrustState::AllVerified,
                TrustVerdict::Blocked => LibraryTrustState::Blocked,
                TrustVerdict::Unreviewed
                    if current.is_some_and(|record| {
                        record.state == crate::trust::TrustState::Blocked
                    }) =>
                {
                    LibraryTrustState::Blocked
                }
                TrustVerdict::Unreviewed if current.is_some() => LibraryTrustState::Moved,
                TrustVerdict::Unreviewed => LibraryTrustState::Unreviewed,
            };
            let metadata = trait_ref.metadata.as_ref();
            rows.push(LibraryRow::Resolved(Box::new(LibraryMember {
                id: trait_ref.id.as_str().to_string(),
                version: trait_ref.version.as_str().to_string(),
                schema_version: trait_ref.schema_version.as_str().to_string(),
                status: crate::lifecycle::resolve_package_status(&trait_root)?
                    .display_name()
                    .to_string(),
                trust,
                trust_state,
                canonical_digest: digest.as_str().to_string(),
                source_path: path.to_string(),
                trait_root: trait_root.to_string(),
                tier: candidate.tier.into(),
                origin: candidate.origin.clone(),
                family: family.or_else(|| {
                    metadata
                        .and_then(|value| value.family.as_ref())
                        .map(|value| value.as_str().to_string())
                }),
                family_key,
                variant: trait_ref.variant.clone(),
                shadow,
                name: trait_ref.id.as_str().to_string(),
                summary: trait_ref.effective_summary().to_string(),
                agents: trait_ref
                    .agents
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect(),
                record: current.map(|record| crate::trust::TrustReportRow {
                    trait_id: Some(trait_ref.id.as_str().to_string()),
                    digest: record.digest.clone(),
                    current_digest: Some(digest.as_str().to_string()),
                    state: record.state,
                    freshness: if record.digest == digest.as_str() {
                        crate::trust::TrustFreshness::Current
                    } else {
                        crate::trust::TrustFreshness::Stale
                    },
                    updated_at: record.updated_at.clone(),
                    reason: record.reason.clone(),
                    seq: record.seq,
                    superseded: false,
                }),
            })));
        }
        Err(error) => rows.push(LibraryRow::Unreadable {
            id: id.to_string(),
            path: path.to_string(),
            error: error.to_string(),
            tier: candidate.tier.into(),
            origin: candidate.origin.clone(),
        }),
    }
    Ok(())
}

fn editable_source(path: &camino::Utf8Path) -> Utf8PathBuf {
    let Some(root) = crate::layout::package_root_for_manifest(path) else {
        return path.to_path_buf();
    };
    for candidate in [
        "source/index.ts",
        "source/index.mjs",
        "index.ts",
        "index.mjs",
    ] {
        let candidate = root.join(candidate);
        if candidate.is_file() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn read_source_excerpt(path: &camino::Utf8Path, max_lines: usize) -> Vec<String> {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path.as_std_path()) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .take(max_lines)
        .map_while(Result::ok)
        .collect()
}

fn sequence_kind_label(kind: ctx_traits_core::r#trait::procedure::SequenceKind) -> &'static str {
    use ctx_traits_core::r#trait::procedure::SequenceKind;
    match kind {
        SequenceKind::Prompt => "prompt",
        SequenceKind::Ask => "ask",
        SequenceKind::Command => "command",
        SequenceKind::Check => "check",
        SequenceKind::Project => "project",
        SequenceKind::Sequence => "sequence",
        SequenceKind::Branch => "branch",
        SequenceKind::Loop => "loop",
        SequenceKind::ForEach => "for-each",
        SequenceKind::Parallel => "parallel",
        SequenceKind::Terminal => "terminal",
    }
}

#[derive(Serialize)]
struct ProvenanceEvidence {
    id: String,
    variant: Option<String>,
    digest: String,
}

fn provenance(rows: &[LibraryRow]) -> LibraryProvenance {
    let mut evidence = Vec::new();
    for row in rows {
        let LibraryRow::Resolved(member) = row else {
            continue;
        };
        if member.tier != TierWire::RepoAuthored {
            continue;
        }
        let root = Utf8PathBuf::from(&member.trait_root);
        let lock = match crate::lockfile::read_lockfile(&root) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return withheld(format!(
                    "missing lockfile at {}",
                    crate::layout::package_lock_path(&root)
                ));
            }
            Err(error) => return withheld(error.to_string()),
        };
        let Some(entry) = lock.trait_entry(&member.id, member.variant.as_deref()) else {
            return withheld(format!("missing lock entry for {}", member.id));
        };
        let Some(digest) = entry.canonical_digest() else {
            return withheld(format!("missing canonical digest for {}", member.id));
        };
        if digest != member.canonical_digest {
            return withheld(format!("lock digest mismatch for {}", member.id));
        }
        evidence.push(ProvenanceEvidence {
            id: member.id.clone(),
            variant: member.variant.clone(),
            digest: digest.to_string(),
        });
    }
    evidence.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.variant.cmp(&b.variant)));
    match canonical_json(&evidence) {
        Ok(serialized) => LibraryProvenance {
            pinned: true,
            sha: Some(Digest::from_bytes(serialized.as_bytes()).to_string()),
            digest_count: evidence.len(),
            error: None,
        },
        Err(error) => withheld(error.to_string()),
    }
}

fn withheld(error: String) -> LibraryProvenance {
    LibraryProvenance {
        pinned: false,
        sha: None,
        digest_count: 0,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(trust: TrustVerdict, trust_state: LibraryTrustState) -> LibraryMember {
        LibraryMember {
            id: "fixture".to_string(),
            version: "1".to_string(),
            schema_version: "1".to_string(),
            status: "draft".to_string(),
            trust,
            trust_state,
            canonical_digest: "sha256:fixture".to_string(),
            source_path: String::new(),
            trait_root: String::new(),
            tier: TierWire::RepoAuthored,
            origin: "trait-id".to_string(),
            family: None,
            family_key: None,
            variant: None,
            shadow: None,
            name: "fixture".to_string(),
            summary: String::new(),
            agents: vec![],
            record: None,
        }
    }

    #[test]
    fn family_precedence_keeps_historical_blocks_conservative() {
        let members = [
            member(TrustVerdict::Verified, LibraryTrustState::AllVerified),
            member(TrustVerdict::Unreviewed, LibraryTrustState::Blocked),
        ];
        assert_eq!(
            family_aggregate(&members),
            (LibraryTrustState::Blocked, 1, 2)
        );
    }

    #[test]
    fn moved_approval_is_distinct_when_no_current_member_is_verified() {
        let members = [member(TrustVerdict::Unreviewed, LibraryTrustState::Moved)];
        assert_eq!(family_aggregate(&members), (LibraryTrustState::Moved, 0, 1));
    }

    #[test]
    fn trust_presentation_is_shared_and_face_specific_only_for_verified() {
        assert_eq!(
            LibraryTrustState::AllVerified
                .presentation(LibraryPresentationFace::Desktop)
                .word,
            "trusted"
        );
        assert_eq!(
            LibraryTrustState::AllVerified
                .presentation(LibraryPresentationFace::Tui)
                .word,
            "verified"
        );
        assert_eq!(
            LibraryTrustState::Unreadable
                .presentation(LibraryPresentationFace::Desktop)
                .role,
            LibraryTrustRole::Danger
        );
    }

    #[test]
    fn detail_summaries_use_the_kebab_case_wire_shape() {
        let detail = LibraryDetailResolution::Resolved {
            display_identity: "fixture".to_string(),
            name: "Fixture".to_string(),
            lede: "summary".to_string(),
            version: "1".to_string(),
            canonical_digest: "sha256:fixture".to_string(),
            trust_state: LibraryTrustState::Unreviewed,
            verified: 0,
            total: 1,
            agents: vec![],
            variants: vec![LibraryVariantSummary {
                key: "member-name".to_string(),
                summary: "member summary".to_string(),
            }],
            ports: vec![LibraryPortSummary {
                id: "port-name".to_string(),
                description: "port description".to_string(),
            }],
            status: "draft".to_string(),
            procedure: LibraryProcedureShape::GuidanceOnly,
            source_drift_checked: false,
            drift: "unverified".to_string(),
            source_path: String::new(),
            source_excerpt: vec![],
            trust_reason: String::new(),
            trust_stale: false,
            has_trust_record: false,
            trust_record: None,
        };
        let value = serde_json::to_value(detail).unwrap();
        assert_eq!(value["data"]["variants"][0]["key"], "member-name");
        assert_eq!(value["data"]["ports"][0]["id"], "port-name");
    }
}
