//! Resource inclusion planning: pure model for deciding which resources to
//! include for activation, render, and context packs.
//!
//! The planner inspects a trait's declared resources, procedure sequence
//! inputs, schema resource backing, and output-port schemas to produce a
//! deterministic inclusion plan. Each entry records the trigger reason and
//! optional file evidence from IO. Render compatibility and context budget
//! estimates are available as separate pure-core projections over the plan.
//!
//! This module performs no IO, provider calls, render execution, or host
//! injection. It plans; the IO and render edges execute.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::reference::{Kind, Reference};
use crate::r#trait::port::PortDirection;
use crate::r#trait::{GuardExpr, ResourceRender, ResourceTrigger, Trait};

// ---------------------------------------------------------------------------
// File evidence (core-owned, bridged from IO)
// ---------------------------------------------------------------------------

/// Combined file evidence for one resource, carrying digest data and
/// file-level issue flags from IO.
///
/// This is a core-owned type — it does not import IO types. The CLI edge
/// constructs it from IO outcomes and passes it into the planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FileEvidence {
    /// The resource ID this evidence belongs to.
    pub resource_id: String,
    /// SHA-256 digest hex string, if the file was read successfully.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    /// File size in bytes, if measured.
    #[serde(default)]
    pub byte_size: u64,
    /// Whether the content was flagged as binary-like.
    #[serde(default)]
    pub is_binary: bool,
    /// Whether IO reported the file as missing.
    #[serde(default)]
    pub missing_file: bool,
    /// Whether IO detected a symlink on the path.
    #[serde(default)]
    pub symlink_detected: bool,
    /// Whether IO reported the path as a directory — a presentable
    /// on-demand resource root whose files agents read with their own
    /// tools, never digestable file content itself (2026-07-31 task board).
    #[serde(default)]
    pub is_directory: bool,
}

/// Typed-ref evidence scanned from a text resource body by the IO layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct BodyEvidence {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// Whether a qualified dependency resource's presentation path is safe to
/// open as a reference. This is the core-owned mirror of
/// `ctx_traits_io::resource::PresentationStatus` from the shared IO safety
/// resolver — variant names and meaning match that type one-to-one — carried
/// across the core/IO boundary so dependency plan and render output can gate
/// on it exactly the way local reference resources already do. `Available`
/// is the only status where `DependencyResourceDecl.path` is populated; the
/// other three must never be presented as something to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DependencyResourceAvailability {
    /// Every path component exists, none is a symlink, and the leaf is a
    /// regular file.
    Available,
    /// The declared resource path does not exist.
    Missing,
    /// A symlink was detected at the leaf or an intermediate component.
    Symlink,
    /// The leaf exists but is not a regular file (directory, FIFO, device).
    SpecialFile,
}

/// The exact "not available" wording for a qualified dependency resource's
/// declared path, per resolved status. Matches the wording the runtime frame
/// (`ctx_traits_io::resource::PresentationStatus`) already uses for the same
/// statuses on a local reference resource, so a missing/symlinked/
/// special-file dependency resource is described identically wherever it
/// surfaces (model view, `ctx traits inspect`/`render` resource-plan output).
pub fn dependency_unavailable_reason(status: DependencyResourceAvailability) -> &'static str {
    match status {
        DependencyResourceAvailability::Available => "resolved path unexpectedly withheld",
        DependencyResourceAvailability::Missing => "declared resource file does not exist",
        DependencyResourceAvailability::Symlink => "a symlink was detected and rejected",
        DependencyResourceAvailability::SpecialFile => "the path is not a regular file",
    }
}

/// Declaration metadata for a qualified (local-dependency) resource, carried
/// independently of any scanned body. A reference-mode dependency resource has
/// no body text to scan, and a dependency resource whose body scan simply fails
/// still exists; either way the parent plan must retain its entry and a usable
/// pointer. This carries exactly the declaration facts the parent needs —
/// effective render mode, availability status, and a real presentation path —
/// so those never depend on whether [`BodyEvidence`] was produced. Body text,
/// when an inline dependency resource needs it, still travels separately as
/// `BodyEvidence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DependencyResourceDecl {
    /// Qualified id `<alias>/<id>`.
    pub resource_id: String,
    /// A real, openable presentation pointer — resolved against the
    /// dependency's actual loaded root via the same safe resource-path
    /// resolver used everywhere else in this codebase, never fabricated by
    /// concatenating the alias (a logical name with no relation to the
    /// dependency's filesystem location) onto the declared path or id.
    /// `None` for a content/inline dependency resource that declares no
    /// path, **and** whenever `availability` is anything other than
    /// `Available` — a missing, symlinked, or special-file path must never
    /// be handed out as an openable pointer, regardless of render mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The resolved availability of the declared path, from the shared IO
    /// safety resolver. `None` only when the resource declares no path at
    /// all (pure inline content); `Some` whenever a path was resolved,
    /// whether or not it turned out safe to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<DependencyResourceAvailability>,
    /// The dependency resource's effective render mode, read from its own
    /// declaration independent of whether any body text was scanned.
    pub render: ResourceRender,
}

/// Digest evidence extracted from file evidence, for serialization on plan
/// entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DigestEvidence {
    /// The resource ID this evidence belongs to.
    pub resource_id: String,
    /// The SHA-256 digest of the file bytes.
    pub digest: Digest,
    /// File size in bytes.
    pub byte_size: u64,
    /// Whether the content was flagged as binary-like.
    pub is_binary: bool,
}

// ---------------------------------------------------------------------------
// Inclusion reason
// ---------------------------------------------------------------------------

/// Why a resource is included in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum InclusionReason {
    /// Loaded when the trait is activated.
    ActivationTriggered,
    /// Referenced by a procedure sequence item input.
    ProcedureInput {
        /// The zero-based sequence index that references this resource.
        sequence_index: usize,
    },
    /// Referenced by a guard-conditioned procedure sequence item input
    /// (P290). Static plans cannot evaluate the guard against a runtime
    /// ledger, so this is only a candidate; the runtime includes the
    /// resource in a step's frame only when the guard matches at readiness.
    ConditionalInput {
        /// The zero-based sequence index that references this resource.
        sequence_index: usize,
        /// The declared inclusion guard.
        guard: GuardExpr,
    },
    /// Backs a schema used by an output port via `schema.resource`.
    OutputPortSchema {
        /// The output port ID whose schema uses this resource.
        port_id: String,
        /// The schema ID backed by this resource.
        schema_id: String,
    },
    /// Backs a declared schema via `schema.resource`, not otherwise
    /// attributed to an output port.
    SchemaBacking {
        /// The schema ID this resource backs.
        schema_id: String,
    },
}

// ---------------------------------------------------------------------------
// Inclusion entry
// ---------------------------------------------------------------------------

/// One planned resource inclusion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Inclusion {
    /// The trait ID this resource belongs to.
    pub trait_id: String,
    /// The resource ID.
    pub resource_id: String,
    /// The declared package-relative path, if this is file-backed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The declared trigger (or the default `OnDemand`).
    pub trigger: ResourceTrigger,
    /// The effective render mode (or its source-aware default): whether this
    /// resource is presented as an openable file reference or embedded
    /// inline. For a qualified dependency inclusion with no local
    /// declaration, this is the dependency's own effective mode.
    pub render: ResourceRender,
    /// For a qualified dependency inclusion whose declaration carries a
    /// path, the resolved availability of that path from the shared IO
    /// safety resolver — `None` for a local (non-dependency) inclusion,
    /// which does not resolve or verify its declared path at plan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_availability: Option<DependencyResourceAvailability>,
    /// Declared template inputs for this resource.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_inputs: Vec<String>,
    /// Nested local resource inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested_resources: Vec<String>,
    /// Dependency-qualified refs that cannot be resolved locally in static render.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_pending_inputs: Vec<String>,
    /// Inputs detected in body evidence but not declared in `resource.input`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_inputs: Vec<String>,
    /// Declared inputs absent from body evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unused_inputs: Vec<String>,
    /// Template diagnostics produced while scanning inline content in core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_diagnostics: Vec<String>,
    /// Why this resource is included.
    pub reason: InclusionReason,
    /// IO digest evidence, if computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest_evidence: Option<DigestEvidence>,
    /// Digest of text body evidence supplied by IO, if template-scanned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_digest: Option<Digest>,
    /// Text body evidence supplied by IO for template/static prompt rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
}

// ---------------------------------------------------------------------------
// Warnings
// ---------------------------------------------------------------------------

/// A warning about resource inclusion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum InclusionWarning {
    /// A resource file was flagged as binary-like.
    BinaryContent { resource_id: String, byte_size: u64 },
    /// A resource path was detected as a symlink.
    SymlinkDetected { resource_id: String },
    /// A qualified dependency resource's declared path is missing or not a
    /// regular file (never a symlink — that case is reported as
    /// `SymlinkDetected`, the same warning a local resource gets). The
    /// dependency resource's `path` is withheld; it must never be presented
    /// as an openable pointer.
    DependencyResourceUnavailable {
        resource_id: String,
        status: DependencyResourceAvailability,
    },
    /// A resource body contains a typed-ref interpolation not declared in input.
    UndeclaredTemplateInput {
        resource_id: String,
        ref_text: String,
    },
    /// A declared template input was not observed in scanned body evidence.
    UnusedTemplateInput {
        resource_id: String,
        ref_text: String,
    },
    /// Body scan found unsupported template syntax.
    TemplateDiagnostic {
        resource_id: String,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// The complete resource inclusion plan for one trait.
///
/// Entries are ordered by inclusion priority:
/// 1. Declaration-triggered (`on-activation`) resources (declaration order)
/// 2. Model-input-referenced resources (sequence order)
/// 3. Output-port-schema-backed resources (port declaration order)
/// 4. Schema-backed resources (schema declaration order)
///
/// A resource appears at most once. If multiple reasons apply, the
/// highest-priority reason wins. An `on-demand` resource (the default for an
/// omitted `trigger`) appears only when one of reasons 2-4 demands it;
/// otherwise it is omitted from the plan entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Plan {
    /// The trait ID.
    pub trait_id: String,
    /// Planned resource inclusions in priority order.
    pub entries: Vec<Inclusion>,
    /// Warnings about the plan.
    pub warnings: Vec<InclusionWarning>,
}

// ---------------------------------------------------------------------------
// Render compatibility
// ---------------------------------------------------------------------------

/// A render profile for resource compatibility checking.
///
/// Text-only profiles cannot represent binary resources. This is a pure-core
/// projection — the actual render execution lives in the IO/render edge.
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
pub enum RenderProfile {
    AgentSkills,
    Pi,
    Opencode,
    ClaudeCode,
    Codex,
}

impl RenderProfile {
    /// Parse a profile from its string name.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// Whether this profile can only represent text content.
    pub fn is_text_only(self) -> bool {
        matches!(
            self,
            Self::AgentSkills | Self::Pi | Self::Opencode | Self::ClaudeCode | Self::Codex
        )
    }

    /// Human-readable profile name.
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// A warning about a resource that a render profile cannot represent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RenderWarning {
    /// The resource ID.
    pub resource_id: String,
    /// The render profile that cannot represent this resource.
    pub profile: RenderProfile,
    /// Why the profile cannot represent this resource.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Context budget
// ---------------------------------------------------------------------------

/// Estimated context budget for included resources.
///
/// Pure estimate from file evidence. No host injection occurs here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ContextBudget {
    /// Total bytes across all resources with digest evidence.
    pub total_bytes: u64,
    /// Rough token estimate (bytes / 4).
    pub estimated_tokens: u64,
    /// Number of resources with digest evidence.
    pub measured_count: usize,
    /// Number of resources without digest evidence.
    pub unmeasured_count: usize,
}

// ---------------------------------------------------------------------------
// Candidate reason for planning
// ---------------------------------------------------------------------------

/// A candidate inclusion reason with its sort key.
///
/// Collected per resource; the best candidate is chosen by
/// `(priority, source_index, declaration_index)`.
#[derive(Clone)]
struct CandidateReason {
    reason: InclusionReason,
    priority: u8,
    /// Primary source-order key within the priority group.
    source_index: usize,
}

impl CandidateReason {
    fn new(reason: InclusionReason, priority: u8, source_index: usize) -> Self {
        Self {
            reason,
            priority,
            source_index,
        }
    }
}

/// Priority order for reasons. Lower = higher priority.
fn reason_priority(reason: &InclusionReason) -> u8 {
    match reason {
        InclusionReason::ActivationTriggered => 0,
        InclusionReason::ProcedureInput { .. } => 1,
        InclusionReason::ConditionalInput { .. } => 1,
        InclusionReason::OutputPortSchema { .. } => 2,
        InclusionReason::SchemaBacking { .. } => 3,
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// Plan resource inclusion for a trait.
///
/// Inspects declared resources, procedure sequence inputs, output-port
/// schemas, and schema resource backing to produce a deterministic inclusion
/// plan. File evidence from IO is attached when provided.
///
/// Entries are sorted by reason priority, then by source order within each
/// priority group (declaration index, sequence index, port index, or schema
/// index depending on the reason kind).
pub fn plan_resource_inclusion(trait_ref: &Trait, file_evidence: &[FileEvidence]) -> Plan {
    plan_resource_inclusion_with_bodies(trait_ref, file_evidence, &[], &[])
}

/// Plan resource inclusion with optional IO-supplied resource body scan
/// evidence and qualified dependency-resource declarations.
///
/// `dependency_resources` carries the declaration metadata (render mode + real
/// path) for qualified local-dependency resources referenced by this trait.
/// It drives their inclusion independently of `body_evidence`, so a
/// reference-mode dependency resource — or one whose body scan produced no
/// text — still gets a plan entry and a usable pointer. Body text for an
/// inline dependency resource is joined in from `body_evidence` when present.
pub fn plan_resource_inclusion_with_bodies(
    trait_ref: &Trait,
    file_evidence: &[FileEvidence],
    body_evidence: &[BodyEvidence],
    dependency_resources: &[DependencyResourceDecl],
) -> Plan {
    let trait_id = trait_ref.id.as_str().to_string();

    let evidence_map: BTreeMap<&str, &FileEvidence> = file_evidence
        .iter()
        .map(|e| (e.resource_id.as_str(), e))
        .collect();
    let body_map: BTreeMap<&str, &BodyEvidence> = body_evidence
        .iter()
        .map(|e| (e.resource_id.as_str(), e))
        .collect();

    // Collect candidate reasons per resource. Multiple candidates may apply;
    // the best (lowest priority, source_index, declaration_index) wins.
    let mut candidates: BTreeMap<String, Vec<CandidateReason>> = BTreeMap::new();

    collect_declaration_candidates(trait_ref, &mut candidates);
    collect_procedure_input_candidates(trait_ref, &mut candidates);
    collect_output_port_schema_candidates(trait_ref, &mut candidates);
    collect_schema_backing_candidates(trait_ref, &mut candidates);

    // Build entries with sort keys: (priority, source_index, declaration_index).
    let mut keyed_entries: Vec<(u8, usize, usize, Inclusion)> = Vec::new();

    for (decl_idx, resource) in trait_ref.resources.iter().enumerate() {
        let best = candidates
            .get(&resource.id)
            .and_then(|cands| {
                cands
                    .iter()
                    .min_by_key(|c| (c.priority, c.source_index, decl_idx))
            })
            .cloned();

        // An `on-demand` resource with no demand candidate (procedure input,
        // output-port schema, or schema backing) is not planned at all. An
        // `on-activation` resource always has a declaration candidate, so
        // this only skips undemanded on-demand resources.
        let Some(best) = best else {
            continue;
        };

        let reason = best.reason.clone();
        let priority = best.priority;
        let source_index = best.source_index;

        let file_ev = evidence_map.get(resource.id.as_str());

        let digest_evidence = file_ev.and_then(|ev| {
            ev.digest.as_ref().map(|d| DigestEvidence {
                resource_id: resource.id.clone(),
                digest: d.clone(),
                byte_size: ev.byte_size,
                is_binary: ev.is_binary,
            })
        });
        let template_inputs: Vec<String> = resource.input.iter().cloned().collect();
        let nested_resources = template_inputs
            .iter()
            .filter_map(|ref_text| resource_ref_id(ref_text))
            .collect::<Vec<_>>();
        let dependency_pending_inputs = template_inputs
            .iter()
            .filter(|ref_text| is_dependency_pending_ref(ref_text))
            .cloned()
            .collect::<Vec<_>>();
        // Both pathless inline forms materialize their body here so the model
        // view embeds actual text, not just a "Render: Inline" header: a
        // `content` body verbatim, and a typed checklist rendered through the
        // one shared `checklist::render_items` formatter (never a second
        // hand-rolled one) so a plan and a frame present identical items.
        let inline_body_text = if resource.is_checklist() {
            Some(crate::r#trait::checklist::render_items(resource))
        } else {
            resource.content.clone()
        };
        let inline_body = inline_body_text.map(|text| {
            let (detected_inputs, diagnostics) = scan_template_inputs(&text);
            BodyEvidence {
                resource_id: resource.id.clone(),
                digest: Some(Digest::source(&text)),
                text: Some(text),
                detected_inputs,
                diagnostics,
            }
        });
        let body_ev = inline_body
            .as_ref()
            .or_else(|| body_map.get(resource.id.as_str()).copied());
        let (unresolved_inputs, unused_inputs) =
            template_body_deltas(template_inputs.as_slice(), body_ev);

        keyed_entries.push((
            priority,
            source_index,
            decl_idx,
            Inclusion {
                trait_id: trait_id.clone(),
                resource_id: resource.id.clone(),
                path: resource.path.clone(),
                trigger: resource.effective_trigger(),
                render: resource.effective_render(),
                // Local (non-dependency) resources do not resolve/verify
                // their declared path against the filesystem at plan time.
                dependency_availability: None,
                template_inputs,
                nested_resources,
                dependency_pending_inputs,
                unresolved_inputs,
                unused_inputs,
                template_diagnostics: inline_body
                    .as_ref()
                    .map(|body| body.diagnostics.clone())
                    .unwrap_or_default(),
                reason,
                digest_evidence,
                body_digest: body_ev.and_then(|body| body.digest.clone()),
                body_text: body_ev.and_then(|body| body.text.clone()),
            },
        ));
    }

    // Qualified dependency-resource inclusions are declaration-driven: each is
    // built from its `DependencyResourceDecl` so it survives (with a real
    // pointer and its own render mode) whether or not body text was scanned.
    // Body text is joined in from `body_evidence` only for an inline
    // dependency resource that actually needs to embed it.
    for decl in dependency_resources {
        let inline_body = body_map
            .get(decl.resource_id.as_str())
            .copied()
            .filter(|_| decl.render == ResourceRender::Inline);
        let (unresolved_inputs, unused_inputs) = template_body_deltas(&[], inline_body);
        keyed_entries.push((
            1,
            usize::MAX,
            usize::MAX,
            Inclusion {
                trait_id: trait_id.clone(),
                resource_id: decl.resource_id.clone(),
                // The dependency's declared path (qualified by alias), a real
                // openable pointer — never the fabricated `<alias>/<id>`, and
                // `None` whenever `availability` is not `Available` (see
                // `DependencyResourceDecl::path`).
                path: decl.path.clone(),
                trigger: ResourceTrigger::OnActivation,
                render: decl.render,
                dependency_availability: decl.availability,
                template_inputs: Vec::new(),
                nested_resources: Vec::new(),
                dependency_pending_inputs: Vec::new(),
                unresolved_inputs,
                unused_inputs,
                // Dependency diagnostics remain warnings only; serializing them
                // here would alter established dependency resource-plan JSON.
                template_diagnostics: Vec::new(),
                reason: InclusionReason::ActivationTriggered,
                digest_evidence: None,
                // A reference-mode dependency resource carries no body: its
                // pointer is the contract, exactly like a local reference.
                body_digest: inline_body.and_then(|body| body.digest.clone()),
                body_text: inline_body.and_then(|body| body.text.clone()),
            },
        ));
    }

    // Sort by (priority, source_index, declaration_index).
    keyed_entries
        .sort_by(|(pa, sa, da, _), (pb, sb, db, _)| pa.cmp(pb).then(sa.cmp(sb)).then(da.cmp(db)));

    let final_entries: Vec<Inclusion> = keyed_entries.into_iter().map(|(_, _, _, e)| e).collect();

    let warnings = build_warnings(&final_entries, &evidence_map, &body_map);

    Plan {
        trait_id,
        entries: final_entries,
        warnings,
    }
}

fn resource_ref_id(ref_text: &str) -> Option<String> {
    let parsed = Reference::parse(ref_text).ok()?;
    if parsed.kind() == Kind::Resource && !parsed.is_qualified() {
        Some(parsed.id().to_string())
    } else {
        None
    }
}

fn is_dependency_pending_ref(ref_text: &str) -> bool {
    Reference::parse(ref_text).is_ok_and(|parsed| parsed.is_qualified())
}

fn template_body_deltas(
    declared: &[String],
    body: Option<&BodyEvidence>,
) -> (Vec<String>, Vec<String>) {
    let Some(body) = body else {
        return (Vec::new(), Vec::new());
    };
    let declared_set: std::collections::BTreeSet<&str> =
        declared.iter().map(String::as_str).collect();
    let detected_set: std::collections::BTreeSet<&str> =
        body.detected_inputs.iter().map(String::as_str).collect();
    let unresolved = detected_set
        .iter()
        .filter(|input| !declared_set.contains(**input))
        .map(|input| (*input).to_string())
        .collect();
    let unused = declared_set
        .iter()
        .filter(|input| !detected_set.contains(**input))
        .map(|input| (*input).to_string())
        .collect();
    (unresolved, unused)
}

/// Collect declaration-trigger candidates for explicit `on-activation`
/// resources. Effective `on-demand` resources (including an omitted
/// `trigger`) create no declaration candidate; they are included only when
/// demanded by a procedure input, output-port schema, or schema backing.
fn collect_declaration_candidates(
    trait_ref: &Trait,
    candidates: &mut BTreeMap<String, Vec<CandidateReason>>,
) {
    for (decl_idx, resource) in trait_ref.resources.iter().enumerate() {
        if resource.effective_trigger() != ResourceTrigger::OnActivation {
            continue;
        }
        let reason = InclusionReason::ActivationTriggered;
        let priority = reason_priority(&reason);
        candidates
            .entry(resource.id.clone())
            .or_default()
            .push(CandidateReason::new(reason, priority, decl_idx));
    }
}

/// Collect procedure-input candidates for resources referenced in sequence
/// item inputs.
fn collect_procedure_input_candidates(
    trait_ref: &Trait,
    candidates: &mut BTreeMap<String, Vec<CandidateReason>>,
) {
    let Some(ref proc) = trait_ref.procedure else {
        return;
    };

    for (seq_idx, item) in proc.sequence.iter().enumerate() {
        for input in item.input.iter() {
            let ref_text = input.ref_text();
            if let Ok(parsed) = Reference::parse(ref_text)
                && parsed.kind() == Kind::Resource
                && !parsed.is_qualified()
            {
                let id = parsed.id().to_string();
                let reason = match input.guard() {
                    Some(guard) => InclusionReason::ConditionalInput {
                        sequence_index: seq_idx,
                        guard: guard.clone(),
                    },
                    None => InclusionReason::ProcedureInput {
                        sequence_index: seq_idx,
                    },
                };
                candidates
                    .entry(id)
                    .or_default()
                    .push(CandidateReason::new(reason, 2, seq_idx));
            }
        }
    }
}

/// Collect output-port-schema candidates for resources that back schemas
/// used by output ports.
///
/// An output port with `schema = "schema:scope"` whose `scope` schema
/// declares `resource = "resource:template"` attributes `template` to this
/// output port.
fn collect_output_port_schema_candidates(
    trait_ref: &Trait,
    candidates: &mut BTreeMap<String, Vec<CandidateReason>>,
) {
    let schema_map: BTreeMap<&str, &crate::r#trait::Schema> = trait_ref
        .schemas
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();

    for (port_idx, port) in trait_ref.ports.iter().enumerate() {
        if !matches!(port.direction, PortDirection::Output) {
            continue;
        }

        let schema_id = unwrap_schema_ref(&port.schema);
        let Some(schema_id) = schema_id else {
            continue;
        };
        let Some(schema) = schema_map.get(schema_id.as_str()) else {
            continue;
        };
        let Some(ref resource_ref) = schema.resource else {
            continue;
        };
        let Ok(parsed) = Reference::parse(resource_ref) else {
            continue;
        };
        if parsed.kind() != Kind::Resource || parsed.is_qualified() {
            continue;
        }

        let resource_id = parsed.id().to_string();
        candidates
            .entry(resource_id)
            .or_default()
            .push(CandidateReason::new(
                InclusionReason::OutputPortSchema {
                    port_id: port.id.clone(),
                    schema_id: schema_id.clone(),
                },
                3,
                port_idx,
            ));
    }
}

/// Collect schema-backing candidates for resources that back declared
/// schemas but are not already attributed to output-port schemas.
fn collect_schema_backing_candidates(
    trait_ref: &Trait,
    candidates: &mut BTreeMap<String, Vec<CandidateReason>>,
) {
    for (schema_idx, schema) in trait_ref.schemas.iter().enumerate() {
        let Some(ref resource_ref) = schema.resource else {
            continue;
        };
        let Ok(parsed) = Reference::parse(resource_ref) else {
            continue;
        };
        if parsed.kind() != Kind::Resource || parsed.is_qualified() {
            continue;
        }

        let resource_id = parsed.id().to_string();

        // Skip if this resource already has an OutputPortSchema candidate.
        let already_output_port = candidates.get(&resource_id).is_some_and(|cands| {
            cands
                .iter()
                .any(|c| matches!(c.reason, InclusionReason::OutputPortSchema { .. }))
        });
        if already_output_port {
            continue;
        }

        candidates
            .entry(resource_id)
            .or_default()
            .push(CandidateReason::new(
                InclusionReason::SchemaBacking {
                    schema_id: schema.id.clone(),
                },
                4,
                schema_idx,
            ));
    }
}

/// Build inclusion warnings from entries and file evidence.
fn build_warnings(
    entries: &[Inclusion],
    evidence_map: &BTreeMap<&str, &FileEvidence>,
    body_map: &BTreeMap<&str, &BodyEvidence>,
) -> Vec<InclusionWarning> {
    let mut warnings = Vec::new();

    for entry in entries {
        let ev = evidence_map.get(entry.resource_id.as_str());

        let symlink_detected = ev.is_some_and(|e| e.symlink_detected);
        let is_binary = ev.is_some_and(|e| e.is_binary);

        if entry.path.is_some() {
            if symlink_detected {
                warnings.push(InclusionWarning::SymlinkDetected {
                    resource_id: entry.resource_id.clone(),
                });
            }

            if is_binary {
                warnings.push(InclusionWarning::BinaryContent {
                    resource_id: entry.resource_id.clone(),
                    byte_size: ev.map(|e| e.byte_size).unwrap_or(0),
                });
            }
        }

        // A qualified dependency resource's path was resolved against the
        // dependency's real root and checked with the same shared IO safety
        // resolver a local resource's path is checked with. `path` was
        // already withheld when unsafe (see `DependencyResourceDecl::path`);
        // this reports the exact reason so the plan never silently drops the
        // resource without explaining why no pointer is available.
        match entry.dependency_availability {
            Some(DependencyResourceAvailability::Symlink) => {
                warnings.push(InclusionWarning::SymlinkDetected {
                    resource_id: entry.resource_id.clone(),
                });
            }
            Some(status @ DependencyResourceAvailability::Missing)
            | Some(status @ DependencyResourceAvailability::SpecialFile) => {
                warnings.push(InclusionWarning::DependencyResourceUnavailable {
                    resource_id: entry.resource_id.clone(),
                    status,
                });
            }
            Some(DependencyResourceAvailability::Available) | None => {}
        }

        for ref_text in &entry.unresolved_inputs {
            warnings.push(InclusionWarning::UndeclaredTemplateInput {
                resource_id: entry.resource_id.clone(),
                ref_text: ref_text.clone(),
            });
        }
        for ref_text in &entry.unused_inputs {
            warnings.push(InclusionWarning::UnusedTemplateInput {
                resource_id: entry.resource_id.clone(),
                ref_text: ref_text.clone(),
            });
        }
        for message in &entry.template_diagnostics {
            warnings.push(InclusionWarning::TemplateDiagnostic {
                resource_id: entry.resource_id.clone(),
                message: message.clone(),
            });
        }
        if entry.path.is_some()
            && let Some(body) = body_map.get(entry.resource_id.as_str())
        {
            for message in &body.diagnostics {
                warnings.push(InclusionWarning::TemplateDiagnostic {
                    resource_id: entry.resource_id.clone(),
                    message: message.clone(),
                });
            }
        }
    }

    warnings.sort_by(|a, b| {
        warning_resource_id(a)
            .cmp(warning_resource_id(b))
            .then(warning_kind_rank(a).cmp(&warning_kind_rank(b)))
    });
    warnings
}

fn scan_template_inputs(text: &str) -> (Vec<String>, Vec<String>) {
    let (interpolations, mut diagnostics) = crate::r#trait::prompt::scan_interpolations(text);
    let mut refs = Vec::new();
    for interpolation in interpolations {
        match Reference::parse(&interpolation.ref_text) {
            Ok(reference)
                if matches!(reference.kind(), Kind::Port | Kind::Slot | Kind::Resource) =>
            {
                refs.push(interpolation.ref_text);
            }
            Ok(reference) => diagnostics.push(format!(
                "interpolation {{{}}} kind {:?} not allowed in resource template",
                interpolation.ref_text,
                reference.kind()
            )),
            Err(_) => diagnostics.push(format!(
                "interpolation {{{}}} is not a valid typed ref",
                interpolation.ref_text
            )),
        }
    }
    refs.sort();
    refs.dedup();
    (refs, diagnostics)
}

/// Stable rank for warning kind ordering within the same resource.
///
/// Order: SymlinkDetected, BinaryContent, template warnings.
fn warning_kind_rank(w: &InclusionWarning) -> u8 {
    match w {
        InclusionWarning::SymlinkDetected { .. } => 0,
        InclusionWarning::DependencyResourceUnavailable { .. } => 1,
        InclusionWarning::BinaryContent { .. } => 2,
        InclusionWarning::UndeclaredTemplateInput { .. } => 3,
        InclusionWarning::UnusedTemplateInput { .. } => 4,
        InclusionWarning::TemplateDiagnostic { .. } => 5,
    }
}

/// Extract the resource ID from a warning variant.
fn warning_resource_id(w: &InclusionWarning) -> &str {
    match w {
        InclusionWarning::BinaryContent { resource_id, .. }
        | InclusionWarning::SymlinkDetected { resource_id }
        | InclusionWarning::DependencyResourceUnavailable { resource_id, .. }
        | InclusionWarning::UndeclaredTemplateInput { resource_id, .. }
        | InclusionWarning::UnusedTemplateInput { resource_id, .. }
        | InclusionWarning::TemplateDiagnostic { resource_id, .. } => resource_id,
    }
}

/// Extract the schema ID from a port schema ref string.
///
/// Handles bare refs (`schema:scope`) and list-wrapped refs
/// (`[schema:scope]`). Returns `None` for non-schema or unparseable refs.
fn unwrap_schema_ref(schema_ref: &str) -> Option<String> {
    let trimmed = schema_ref.trim();
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let parsed = Reference::parse(inner).ok()?;
    if parsed.kind() == Kind::Schema && !parsed.is_qualified() {
        Some(parsed.id().to_string())
    } else {
        None
    }
}

/// Check render compatibility for a resource inclusion plan against a
/// render profile.
///
/// Text-only profiles cannot represent binary resources. Returns warnings
/// for each incompatible resource.
pub fn check_render_compatibility(plan: &Plan, profile: RenderProfile) -> Vec<RenderWarning> {
    if !profile.is_text_only() {
        return Vec::new();
    }

    let mut warnings = Vec::new();

    for entry in &plan.entries {
        if let Some(ref evidence) = entry.digest_evidence
            && evidence.is_binary
        {
            warnings.push(RenderWarning {
                resource_id: entry.resource_id.clone(),
                profile,
                reason: format!(
                    "binary resource {:?} cannot be represented by text-only profile {:?}",
                    entry.resource_id, profile
                ),
            });
        }
    }

    warnings
}

/// Estimate the context budget for included resources.
///
/// Sums byte sizes from digest evidence and produces a rough token estimate
/// (bytes / 4). Resources without evidence are counted as unmeasured.
pub fn estimate_context_budget(plan: &Plan) -> ContextBudget {
    let mut total_bytes: u64 = 0;
    let mut measured_count = 0;
    let mut unmeasured_count = 0;

    for entry in &plan.entries {
        match entry.digest_evidence.as_ref() {
            Some(evidence) => {
                total_bytes += evidence.byte_size;
                measured_count += 1;
            }
            None => {
                unmeasured_count += 1;
            }
        }
    }

    ContextBudget {
        total_bytes,
        estimated_tokens: total_bytes / 4,
        measured_count,
        unmeasured_count,
    }
}

/// Bridge helper: construct core `FileEvidence` from IO digest data.
///
/// Callers pass the IO `ResourceFileDigest` fields plus issue flags.
pub fn file_evidence_from_io(
    resource_id: &str,
    digest: Option<&crate::digest::Digest>,
    byte_size: u64,
    is_binary: bool,
    missing_file: bool,
    symlink_detected: bool,
    is_directory: bool,
) -> FileEvidence {
    FileEvidence {
        resource_id: resource_id.to_string(),
        digest: digest.cloned(),
        byte_size,
        is_binary,
        missing_file,
        symlink_detected,
        is_directory,
    }
}
