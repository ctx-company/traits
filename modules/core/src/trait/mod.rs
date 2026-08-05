//! Canonical trait root schema: identity and summary.
//!
//! Typed IDs and value objects keep trait identity distinct from display
//! names and search labels. Lifecycle status and trust are owned by separate
//! stores, never by this document: status is a team-edited `[package]`
//! manifest field (`draft | ready`) in `trait.toml`; trust is always
//! machine-local, keyed by canonical digest, in `~/.config/ctx/trust.toml`
//! (Group 95, 2026-07-19). `derive_more` reduces wrapper boilerplate while
//! keeping types explicit.

pub mod activation;
pub mod agent;
pub mod behavior;
pub mod checklist;
pub mod composition;
pub mod condition;
pub mod eval;
pub mod extensions;
pub mod guidance;
pub mod intent;
pub mod metadata;
pub mod port;
pub mod procedure;
pub mod prompt;
pub mod relations;
pub mod resource;
pub mod scenario;
pub mod schema;
pub mod sequence;
pub mod session;
pub mod signal;
pub mod slot;

pub use agent::{
    AGENT_TEMPLATES, Agent, AgentModelTier, AgentTemplate, SESSION_LIFECYCLE_PER_FRAME,
    SESSION_LIFECYCLE_PERSISTENT, instantiate_agent_template,
};
pub use behavior::{Behavior, BehaviorItem, BehaviorItemList};
pub use condition::{
    ComparisonOperandEvidence, Condition, ConditionComparisonEvidence, ConditionComparisonOperator,
    ConditionComparisonSubject, ConditionEvaluation, ConditionMap, GuardExpr, GuardPredicate,
};
pub use eval::{Eval, EvalAuditKind, EvalAuditWarning, EvalResult, EvalVariant};
pub use extensions::{CtxExtensions, Extensions, SubagentIntent};
pub use guidance::{GuidanceItem, GuidanceItemList};
pub use intent::{Intent, IntentItem, IntentItemList};
pub use metadata::Metadata;
pub use port::{Port, PortDefault, PortDefaultCommand, PortDirection};
pub use prompt::{Prompt, PromptMap};
pub use resource::{
    CHECKLIST_VARIANT, ChecklistItem, Resource, ResourceInputList, ResourceRender, ResourceRoot,
    ResourceTrigger,
};
pub use scenario::{Scenario, ScenarioAuditKind, ScenarioAuditWarning, ScenarioVariant};
pub use schema::{Schema, SchemaField};
pub use sequence::{NamedSequence, NamedSequenceMap};
pub use session::{Session, SessionBinding, parse_session_binding};
pub use signal::{Signal, SignalTraceEvent, SignalTraceEvidence, SignalTraceScope};
pub use slot::Slot;

use derive_more::Display;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error as ThisError;

use crate::manifest::Dependency;

/// Trait model validation errors.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("invalid manifest at {field_path}: {message}")]
    InvalidField { field_path: String, message: String },
}

impl Error {
    pub(crate) fn invalid_field(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field_path: field_path.into(),
            message: message.into(),
        }
    }
}

/// Stable, durable trait identifier (not a display name).
///
/// Must be non-empty and use only lowercase letters, digits, and hyphens.
/// Display names and search labels are separate fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        validate_non_empty("trait.id", &id)?;
        validate_identifier("trait.id", &id)?;
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Trait schema version string (e.g. "0.2").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct SchemaVersion(String);

impl SchemaVersion {
    pub fn new(version: impl Into<String>) -> crate::Result<Self> {
        let version = version.into();
        validate_schema_version(&version)?;
        Ok(Self(version))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Shared seam every `SchemaVersion` construction path passes through:
/// `SchemaVersion::new` and the hand-written `Deserialize` impl below (which
/// every TOML/JSON/YAML decode of a `Trait` goes through by construction).
fn validate_schema_version(version: &str) -> crate::Result<()> {
    validate_non_empty("schema-version", version)?;
    if !is_schema_version_supported(version) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "schema-version".to_string(),
            message: format!(
                "unsupported schema-version {version:?}; this binary supports {SUPPORTED_SCHEMA_VERSIONS:?} — upgrade ctx"
            ),
        }
        .into());
    }
    Ok(())
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = String::deserialize(deserializer)?;
        validate_schema_version(&version).map_err(serde::de::Error::custom)?;
        Ok(Self(version))
    }
}

/// Canonical trait schema versions this binary can decode, in preference
/// order. Bump when a new schema version is introduced; never remove an
/// entry a released binary already reads. Adding a version here requires a
/// `MIGRATION_STEPS` entry; see migrate.rs release-gate tests / task 0029.
pub const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["0.2", "0.3"];

/// Whether this binary supports decoding a canonical document declaring
/// `schema_version`.
///
/// Callers that only have the raw `schema-version` string (project-lock
/// evidence, a pre-decode preflight scan) can check support before ever
/// invoking the full TOML/JSON/YAML decoder, so an unsupported newer schema
/// produces a concise "upgrade ctx" error instead of surfacing as a generic
/// unknown-field decode failure deep inside `encoding`.
pub fn is_schema_version_supported(schema_version: &str) -> bool {
    SUPPORTED_SCHEMA_VERSIONS.contains(&schema_version)
}

/// Semantic version text (e.g. "0.1.0").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SemanticVersion(String);

impl SemanticVersion {
    pub fn new(version: impl Into<String>) -> crate::Result<Self> {
        let version = version.into();
        validate_non_empty("version", &version)?;
        validate_semver("version", &version)?;
        Ok(Self(version))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Human-readable display name, not a durable identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Name(String);

impl Name {
    pub fn new(name: impl Into<String>) -> crate::Result<Self> {
        let name = name.into();
        validate_non_empty("name", &name)?;
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Short human-readable description.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Summary(String);

impl Summary {
    pub fn new(summary: impl Into<String>) -> crate::Result<Self> {
        let summary = summary.into();
        validate_non_empty("summary", &summary)?;
        Ok(Self(summary))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Machine-local trust verdict for a trait's canonical digest.
///
/// This is never a document field. It is resolved at the IO boundary from
/// the machine trust store (`~/.config/ctx/trust.toml`, keyed by canonical
/// digest) and passed explicitly into pure gate/report functions. Absence of
/// a trust-store record means `Unreviewed`, not `Blocked`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum TrustVerdict {
    Verified,
    Blocked,
    Unreviewed,
}

impl TrustVerdict {
    /// User-facing display name.
    pub fn display_name(&self) -> &'static str {
        self.into()
    }
}

/// A trust verdict asserted by a caller outside the process (a WASM ABI
/// host, a plugin) for a specific canonical digest.
///
/// The machine trust store is keyed by canonical digest precisely so a
/// verdict never survives a content edit; any boundary that receives a
/// verdict as a bare string (rather than resolving it itself from the
/// store) must carry the digest it was resolved for, so the receiving side
/// can refuse to honor a verdict asserted for different content than what
/// it currently holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEvidence {
    /// Canonical digest the verdict was resolved against.
    pub digest: String,
    pub verdict: TrustVerdict,
}

impl TrustEvidence {
    /// Resolve the effective verdict for `current_digest`, failing closed to
    /// `Unreviewed` when `evidence` is absent or was asserted for a
    /// different digest than `current_digest` (stale trust never survives a
    /// content edit, whether the edit happened before or after the verdict
    /// crossed a process boundary).
    pub fn resolve(evidence: Option<&TrustEvidence>, current_digest: &str) -> TrustVerdict {
        match evidence {
            Some(evidence) if evidence.digest == current_digest => evidence.verdict,
            _ => TrustVerdict::Unreviewed,
        }
    }
}

/// Stable gate reason codes for activation/lifecycle enforcement.
///
/// Every activation, check, resolve, and lifecycle command path must reference
/// these constants rather than ad-hoc string literals. Status is package
/// manifest `[package].status` (`draft | ready`); trust is the machine trust
/// store verdict for the trait's canonical digest (`verified | blocked`,
/// absent = unreviewed). (Group 95, 2026-07-19.)
pub mod gate_code {
    pub const STATUS_DRAFT: &str = "blocked.status.draft";
    pub const TRUST_UNREVIEWED: &str = "blocked.trust.unreviewed";
    pub const TRUST_BLOCKED: &str = "blocked.trust.blocked";
}

/// The canonical trait root aggregate: identity, guidance, activation, ports,
/// slots, schemas, signals, prompts, procedure, composition, and relations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Trait {
    pub id: Id,
    pub schema_version: SchemaVersion,
    pub version: SemanticVersion,
    pub name: Name,
    pub summary: Summary,

    /// Native family variant identity. This is deliberately absent from 0.2
    /// documents, preserving their encoded bytes and digest semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,

    /// Dependencies that this trait references by alias, e.g.
    /// `resource:aws/rubric`. Resolution is performed at the IO/check boundary.
    #[serde(default, rename = "dependency", skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,

    /// Searchable metadata facets. Separate from activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,

    /// Intent: what the trait requires, focuses on, avoids, and blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,

    /// Behavior: how the trait should act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<Behavior>,

    /// Declaration matching declarations. Pure data only; runtime activation is
    /// resolved by later gated phases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<activation::Declaration>,

    /// Port declarations: boundary contracts for trait dataflow.
    #[serde(default, rename = "port", skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,

    /// Slot definitions: procedure run ledger value contracts.
    #[serde(default, rename = "slot", skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<Slot>,

    /// Resource declarations: opaque blob references for schema bodies and
    /// external data.
    #[serde(default, rename = "resource", skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,

    /// Local schema declarations: named object schema contracts backed by
    /// resource refs or inline fields.
    #[serde(default, rename = "schema", skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<Schema>,

    /// Declared signal identities for procedure sequence emission.
    #[serde(default, rename = "signal", skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<Signal>,

    /// Declared abstract agent roles for multi-harness procedure routing.
    #[serde(default, rename = "agent", skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,

    /// Declared session identities: named, shareable session bindings that
    /// `[[agent]].session` references target.
    #[serde(default, rename = "session", skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<Session>,

    /// Prompt definitions: reusable prompt bodies with optional contracts.
    /// Map key is the prompt ID (TOML `[prompt.<id>]`).
    #[serde(
        default,
        rename = "prompt",
        skip_serializing_if = "PromptMap::is_empty"
    )]
    pub prompts: PromptMap,

    /// Named reusable sequence blocks (`[sequence.<id>]`).
    #[serde(
        default,
        rename = "sequence",
        skip_serializing_if = "NamedSequenceMap::is_empty"
    )]
    pub sequences: NamedSequenceMap,

    /// Named typed conditions (`[condition.<id>]`).
    #[serde(
        default,
        rename = "condition",
        skip_serializing_if = "ConditionMap::is_empty"
    )]
    pub conditions: ConditionMap,

    /// Optional procedure: deterministic sequence orchestration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<procedure::Model>,

    /// Contract contract: conflict declarations and merge policy.
    /// Pure data only; runtime composition planning is separate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<composition::Contract>,

    /// Canonical relations: trait-to-trait requires, suggests, and targetless
    /// conflicts. Pure data only; graph construction and evaluation are
    /// separate deterministic passes.
    #[serde(default, rename = "relations", skip_serializing_if = "Option::is_none")]
    pub relations: Option<relations::Model>,

    /// Non-standard ctx-specific advisory/reporting extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,

    /// Product review scenarios: positive, negative, and edge behavior
    /// examples. Not executable test cases.
    #[serde(default, rename = "scenario", skip_serializing_if = "Vec::is_empty")]
    pub scenarios: Vec<Scenario>,

    /// Product eval declarations: declared checks that can be run.
    #[serde(default, rename = "eval", skip_serializing_if = "Vec::is_empty")]
    pub evals: Vec<Eval>,
}

impl Trait {
    /// The name this trait exports under: variant-qualified when it is a
    /// family variant, and the plain id otherwise.
    ///
    /// Every variant of a native family shares one `id` — all five
    /// `implement` variants are `implement` — so exporting by id alone made
    /// them collide on a single path, each overwriting the last, and the
    /// lock then recorded several entries for one file with different
    /// digests. At most one could match, so the locked check failed
    /// permanently.
    ///
    /// One definition, used by both the export path and the generated file's
    /// ownership marker: they must agree, or a re-export reads back a marker
    /// naming a different owner and refuses. `<id>-<variant>` is the spelling
    /// the family already declares as each variant's alias.
    pub fn export_id(&self) -> Id {
        match self.variant.as_deref() {
            Some(variant) => Id::new(format!("{}-{variant}", self.id.as_str()))
                .unwrap_or_else(|_| self.id.clone()),
            None => self.id.clone(),
        }
    }

    /// Validate every taxonomy slug with canonical field paths.
    ///
    /// Returns the first invalid slug's structured diagnostic, or `Ok(())`.
    pub fn validate_taxonomy(&self) -> crate::Result<()> {
        match (&self.variant, self.schema_version.as_str()) {
            (Some(_), "0.3") => {}
            (Some(_), _) => {
                return Err(
                    Error::invalid_field("variant", "requires schema-version \"0.3\"").into(),
                );
            }
            (None, _) => {}
        }
        if let Some(variant) = &self.variant {
            for (index, segment) in variant.split(':').enumerate() {
                crate::shared::validate_slug_shape(segment, &format!("variant[{index}]"))?;
            }
        }
        if let Some(ref m) = self.metadata {
            m.validate_taxonomy()?;
        }
        if let Some(ref i) = self.intent {
            i.validate_taxonomy()?;
        }
        if let Some(ref b) = self.behavior {
            b.validate_taxonomy()?;
        }
        crate::r#trait::extensions::validate_extensions(self.extensions.as_ref())?;
        Ok(())
    }

    /// Classify this trait's runtime shape.
    ///
    /// `GuidanceOnly` means `Trait.procedure` is `None`. The trait may still
    /// have prompts, behavior, metadata, or ports — it simply has no
    /// procedure-run ledger to plan. `ProcedureBacked` means `[procedure]`
    /// exists and P25/P26 validation applies.
    pub fn runtime_shape(&self) -> TraitRuntimeShape {
        if self.procedure.is_some() {
            TraitRuntimeShape::ProcedureBacked
        } else {
            TraitRuntimeShape::GuidanceOnly
        }
    }

    /// Compute capability kinds from present content without storing a kind
    /// field in the canonical artifact.
    ///
    /// This follows the design taxonomy: knowledge is carried by resources or
    /// schemas, behavior by an explicit behavior block, and procedures by a
    /// procedure section. Prompts, activation, and intent are supporting
    /// metadata and do not independently change the derived kind.
    pub fn derived_kinds(&self) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        if !self.resources.is_empty() || !self.schemas.is_empty() {
            kinds.push("knowledge");
        }
        if self.behavior.is_some() {
            kinds.push("behavior");
        }
        if self.procedure.is_some() {
            kinds.push("procedure");
        }
        if kinds.len() > 1 {
            kinds.push("mixed");
        }
        if kinds.is_empty() {
            kinds.push("knowledge");
        }
        kinds
    }
}

/// The runtime shape of a trait: guidance-only or procedure-backed.
///
/// Guidance-only traits are valid canonical traits that describe behavior,
/// communication, reasoning, or constraints without declaring a procedure.
/// They are not empty, inactive, or invalid — they simply have no
/// procedure-run ledger to plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum TraitRuntimeShape {
    /// No `[procedure]` section. The trait provides guidance, prompts, or
    /// behavior without a deterministic step orchestration.
    GuidanceOnly,
    /// A `[procedure]` section exists. P25/P26 validation and dry planning
    /// apply.
    ProcedureBacked,
}

fn validate_non_empty(field: &str, value: &str) -> crate::Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid_field(field, "must not be empty").into());
    }
    Ok(())
}

fn validate_identifier(field: &str, value: &str) -> crate::Result<()> {
    // Transition note: trait IDs still use the legacy charset-only rule to
    // avoid changing accepted manifests without explicit owner approval. Most
    // nested IDs use `shared::validate_slug_shape`, which is stricter about
    // leading/trailing/repeated hyphens.
    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(Error::invalid_field(
            field,
            "must contain only lowercase letters, digits, and hyphens",
        )
        .into());
    }
    Ok(())
}

fn validate_semver(field: &str, value: &str) -> crate::Result<()> {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
    {
        return Err(Error::invalid_field(field, "must be a semantic version (e.g. 0.1.0)").into());
    }
    Ok(())
}
