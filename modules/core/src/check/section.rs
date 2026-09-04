use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed vocabulary of check report sections. A `CheckWarning` or
/// `CheckReport::with_section` naming a section outside this set fails to
/// compile rather than silently dropping the warning from the styled
/// grouped renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Section {
    Validation,
    DerivedKind,
    IoContract,
    ControlBounds,
    Sequence,
    LifecycleStatus,
    MachineTrust,
    Activation,
    Dependencies,
    DependencyTrust,
    Resources,
    ResourceProtection,
    RenderReadiness,
    HiddenContentAudit,
    ScenarioEvalAudit,
    ImportEvidence,
    RuntimeEvidence,
    EvalEvidence,
    ModelView,
    ProjectionLock,
    CdkDrift,
    CdkAuthoring,
    RunConfig,
    Extensions,
    Metadata,
    CandidateLifecycleTrust,
    /// Fork provenance (`[forked-from]` in `trait.toml`, 0213) — present
    /// only for a package `ctx traits fork` produced.
    ForkedFrom,
}

impl Section {
    pub const fn as_str(self) -> &'static str {
        match self {
            Section::Validation => "validation",
            Section::DerivedKind => "derived-kind",
            Section::IoContract => "io-contract",
            Section::ControlBounds => "control-bounds",
            Section::Sequence => "sequence",
            Section::LifecycleStatus => "lifecycle-status",
            Section::MachineTrust => "machine-trust",
            Section::Activation => "activation",
            Section::Dependencies => "dependencies",
            Section::DependencyTrust => "dependency-trust",
            Section::Resources => "resources",
            Section::ResourceProtection => "resource-protection",
            Section::RenderReadiness => "render-readiness",
            Section::HiddenContentAudit => "hidden-content-audit",
            Section::ScenarioEvalAudit => "scenario-eval-audit",
            Section::ImportEvidence => "import-evidence",
            Section::RuntimeEvidence => "runtime-evidence",
            Section::EvalEvidence => "eval-evidence",
            Section::ModelView => "model-view",
            Section::ProjectionLock => "projection-lock",
            Section::CdkDrift => "cdk-drift",
            Section::CdkAuthoring => "cdk-authoring",
            Section::RunConfig => "run-config",
            Section::Extensions => "extensions",
            Section::Metadata => "metadata",
            Section::CandidateLifecycleTrust => "candidate-lifecycle-trust",
            Section::ForkedFrom => "forked-from",
        }
    }
}

impl std::fmt::Display for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialOrd for Section {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Section {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// A named section of the check report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CheckSection {
    /// The section name.
    pub name: String,
    /// Human-readable summary text.
    pub summary: String,
    /// Whether the section passed without issues.
    pub ok: bool,
}
