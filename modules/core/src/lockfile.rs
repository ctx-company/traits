//! Package-local lock evidence model.
//!
//! `trait.lock` records resolved trait and dependency evidence, generated
//! exports, eval results, and imported source snapshots. It is generated
//! evidence, not a behavioral-safety claim.

use serde::{Deserialize, Serialize};

/// The package-local `trait.lock` document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Lockfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    #[serde(rename = "trait", default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<LockEntry>,
    #[serde(rename = "dependency", default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<LockDependencyEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<crate::import::plan::TraitLock>,
}

impl Lockfile {
    pub fn new(metadata: Metadata) -> Self {
        Self {
            metadata: Some(metadata),
            ..Self::default()
        }
    }

    pub fn insert(&mut self, trait_id: impl Into<String>, mut entry: LockEntry) {
        entry.id = trait_id.into();
        let variant = entry.variant.clone();
        if let Some(existing) = self.trait_entry_mut(&entry.id, variant.as_deref()) {
            *existing = entry;
        } else {
            self.traits.push(entry);
        }
    }

    pub fn trait_entry(&self, trait_id: &str, variant: Option<&str>) -> Option<&LockEntry> {
        self.traits
            .iter()
            .find(|entry| entry.id == trait_id && entry.variant.as_deref() == variant)
    }

    pub fn trait_entry_mut(
        &mut self,
        trait_id: &str,
        variant: Option<&str>,
    ) -> Option<&mut LockEntry> {
        self.traits
            .iter_mut()
            .find(|entry| entry.id == trait_id && entry.variant.as_deref() == variant)
    }

    pub fn dependency_entry(&self, alias: &str) -> Option<&LockDependencyEntry> {
        self.dependencies.iter().find(|entry| entry.alias == alias)
    }

    pub fn upsert_dependency_entry(&mut self, entry: LockDependencyEntry) {
        if let Some(existing) = self
            .dependencies
            .iter_mut()
            .find(|existing| existing.alias == entry.alias)
        {
            *existing = entry;
        } else {
            self.dependencies.push(entry);
        }
    }

    pub fn sort_for_output(&mut self) {
        self.traits.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.variant.cmp(&right.variant))
        });
        self.dependencies
            .sort_by(|left, right| left.alias.cmp(&right.alias));
        for entry in &mut self.traits {
            entry
                .exports
                .sort_by(|left, right| left.target.cmp(&right.target));
            entry
                .projections
                .sort_by(|left, right| left.target_profile.cmp(&right.target_profile));
            entry.eval_results.sort();
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// Generated lockfile metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// Per-trait lock entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<LockDigests>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synth_provenance: Vec<crate::synth::Provenance>,
    #[serde(rename = "export", default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<LockExport>,
    #[serde(rename = "projection", default, skip_serializing_if = "Vec::is_empty")]
    pub projections: Vec<LockProjection>,
    #[serde(rename = "eval-result", default, skip_serializing_if = "Vec::is_empty")]
    pub eval_results: Vec<crate::r#trait::EvalResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_variant_entries_have_distinct_identity_and_stable_order() {
        let mut lock: Lockfile = toml::from_str(
            "[[trait]]\nid = \"standalone\"\n\n[[trait]]\nid = \"family\"\nvariant = \"quick\"\n",
        )
        .unwrap();
        assert!(lock.trait_entry("standalone", None).is_some());
        assert!(lock.trait_entry("standalone", Some("default")).is_none());

        lock.insert(
            "family",
            LockEntry {
                variant: Some("default".to_string()),
                ..LockEntry::default()
            },
        );
        lock.insert(
            "family",
            LockEntry {
                variant: Some("strict".to_string()),
                ..LockEntry::default()
            },
        );
        lock.sort_for_output();

        let identities = lock
            .traits
            .iter()
            .map(|entry| (entry.id.as_str(), entry.variant.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            vec![
                ("family", Some("default")),
                ("family", Some("quick")),
                ("family", Some("strict")),
                ("standalone", None),
            ]
        );
        assert!(lock.trait_entry("family", Some("quick")).is_some());
        assert!(lock.trait_entry("family", Some("strict")).is_some());
    }
}

impl LockEntry {
    pub fn source_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.source.as_deref())
    }

    pub fn canonical_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.canonical.as_deref())
    }

    pub fn model_visible_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.model_visible.as_deref())
    }

    pub fn policy_manifest_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.policy_manifest.as_deref())
    }

    pub fn resource_manifest_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.resource_manifest.as_deref())
    }

    pub fn projection(&self, target_profile: &str) -> Option<&LockProjection> {
        self.projections
            .iter()
            .find(|projection| projection.target_profile == target_profile)
    }

    pub fn upsert_projection(&mut self, projection: LockProjection) {
        if let Some(existing) = self
            .projections
            .iter_mut()
            .find(|existing| existing.target_profile == projection.target_profile)
        {
            *existing = projection;
        } else {
            self.projections.push(projection);
        }
    }
}

/// Digest evidence for one locked trait entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockDigests {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_visible: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_manifest: Option<String>,
}

/// Generated export evidence for one target profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockExport {
    pub target: String,
    pub path: String,
    pub digest: String,
}

/// Per-dependency lock entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockDependencyEntry {
    pub alias: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<toml::Value>,
    pub vendored_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<LockDependencyDigests>,
    /// Resolved Git commit for `git`-sourced dependencies; `None` for local
    /// and npm sources, and for Git sources not yet synced under this schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<String>,
}

impl LockDependencyEntry {
    pub fn source_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.source.as_deref())
    }

    pub fn canonical_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.canonical.as_deref())
    }

    pub fn model_visible_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.model_visible.as_deref())
    }

    pub fn resource_manifest_digest(&self) -> Option<&str> {
        self.digests
            .as_ref()
            .and_then(|digests| digests.resource_manifest.as_deref())
    }
}

/// Digest evidence for one locked dependency entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockDependencyDigests {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_visible: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_manifest: Option<String>,
}

/// Renderer version recorded on projection evidence.
pub const PROJECTION_RENDERER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Static skill-projection evidence for one target profile.
///
/// Replaces the former standalone `trait.skill.lock`: the same digests,
/// warnings, output state, and provenance now live under the trait's
/// `[[trait.projection]]` entries in `trait.lock`, keyed by target profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockProjection {
    pub target_profile: String,
    pub renderer_version: String,
    pub profile_version: String,
    pub digests: LockProjectionDigests,
    #[serde(default, skip_serializing_if = "LockProjectionWarnings::is_empty")]
    pub warnings: LockProjectionWarnings,
    pub static_output: LockProjectionOutput,
    pub provenance: LockProjectionProvenance,
}

/// Digest evidence for one projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockProjectionDigests {
    pub source: String,
    pub canonical: String,
    pub model_visible: String,
    pub render: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_markdown: Option<String>,
}

/// Semantic-loss and advisory warnings recorded at projection time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockProjectionWarnings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_loss: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisory_policy: Vec<String>,
}

impl LockProjectionWarnings {
    pub fn is_empty(&self) -> bool {
        self.semantic_loss.is_empty() && self.advisory_policy.is_empty()
    }
}

/// Static-output state recorded at projection time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockProjectionOutput {
    pub drift_status: ProjectionDriftStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Projection drift verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectionDriftStatus {
    InSync,
    SourceDrifted,
    MissingLock,
    OutputNotWritten,
}

/// Provenance of the command that recorded the projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockProjectionProvenance {
    pub command: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Inputs for building projection evidence from a render plan.
pub struct ProjectionBuild<'a> {
    pub source_digest: &'a str,
    pub canonical_digest: &'a str,
    pub plan: &'a crate::render::RenderPlan,
    pub generated_markdown: Option<&'a str>,
    pub output_path: Option<&'a str>,
    pub command: &'a str,
    pub options: Vec<String>,
}

pub fn build_lock_projection(input: ProjectionBuild<'_>) -> LockProjection {
    let render_digest =
        crate::digest::Digest::from_bytes(render_projection_basis(input.plan).as_bytes());
    let generated_markdown = input
        .generated_markdown
        .map(|content| crate::digest::Digest::from_bytes(content.as_bytes()).to_string());
    LockProjection {
        target_profile: input.plan.profile.as_str().to_string(),
        renderer_version: PROJECTION_RENDERER_VERSION.to_string(),
        profile_version: input.plan.profile.as_str().to_string(),
        digests: LockProjectionDigests {
            source: input.source_digest.to_string(),
            canonical: input.canonical_digest.to_string(),
            model_visible: input.plan.model_view.content_digest.to_string(),
            render: render_digest.to_string(),
            generated_markdown,
        },
        warnings: LockProjectionWarnings {
            semantic_loss: input
                .plan
                .capability_warnings
                .iter()
                .map(|warning| format!("{}: {}", warning.field, warning.reason))
                .chain(
                    input.plan.resource_warnings.iter().map(|warning| {
                        format!("resource.{}: {}", warning.resource_id, warning.reason)
                    }),
                )
                .collect(),
            advisory_policy: input.plan.model_view.warnings.clone(),
        },
        static_output: LockProjectionOutput {
            drift_status: if input.output_path.is_some() {
                ProjectionDriftStatus::InSync
            } else {
                ProjectionDriftStatus::OutputNotWritten
            },
            path: input.output_path.map(str::to_string),
        },
        provenance: LockProjectionProvenance {
            command: input.command.to_string(),
            profile: input.plan.profile.as_str().to_string(),
            options: input.options,
        },
    }
}

/// Compare recorded projection evidence against current digests.
pub fn projection_drift_status(
    projection: Option<&LockProjection>,
    source_digest: &str,
    canonical_digest: &str,
    model_visible_digest: &str,
) -> ProjectionDriftStatus {
    let Some(projection) = projection else {
        return ProjectionDriftStatus::MissingLock;
    };
    if projection.digests.source != source_digest
        || projection.digests.canonical != canonical_digest
        || projection.digests.model_visible != model_visible_digest
    {
        ProjectionDriftStatus::SourceDrifted
    } else {
        ProjectionDriftStatus::InSync
    }
}

fn render_projection_basis(plan: &crate::render::RenderPlan) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        plan.trait_id,
        plan.profile.as_str(),
        plan.source_digest,
        plan.model_view.content_digest,
        plan.generated_file_marker,
    )
}
