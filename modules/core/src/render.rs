//! Render profiles and render planning.
//!
//! Extends the `RenderProfile` enum from `resource_plan` with additional
//! profiles and provides render request/plan structs. Render profiles
//! describe the target host for generated compatibility exports. Generated
//! exports are render artifacts, not canonical state.
//!
//! Render planning is pure in core — it computes what will be rendered and
//! what warnings apply. Filesystem writes happen in the IO layer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::export::{Identity, OwnershipKey};
use crate::model_view::{
    Normalization, Report, compile_model_view_with_evidence, sanitize_model_text,
};
use crate::resource_plan::{
    BodyEvidence, DependencyResourceDecl, FileEvidence, Plan, plan_resource_inclusion_with_bodies,
};
use crate::r#trait::Trait;

// ---------------------------------------------------------------------------
// Extended render profile
// ---------------------------------------------------------------------------

/// All render profiles supported by the system.
///
/// Extends the base `RenderProfile` from `resource_plan` with `Copilot` and
/// `MarkdownOnly`.
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
pub enum ExtendedRenderProfile {
    AgentSkills,
    Pi,
    Opencode,
    ClaudeCode,
    Codex,
    Copilot,
    MarkdownOnly,
}

impl ExtendedRenderProfile {
    /// Parse a profile from its kebab-case string form.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// Human-readable profile name.
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// The default export directory for profiles with P51 compatibility targets.
    pub fn default_export_dir(self) -> Option<&'static str> {
        match self {
            Self::AgentSkills => Some(".agents/skills"),
            Self::Pi => Some(".pi/skills"),
            Self::Opencode => Some(".opencode/skills"),
            Self::ClaudeCode => Some(".claude/skills"),
            Self::Codex => Some(".github/skills"),
            Self::Copilot | Self::MarkdownOnly => None,
        }
    }

    /// Whether this profile is text-only (cannot represent binary resources).
    pub fn is_text_only(self) -> bool {
        matches!(
            self,
            Self::AgentSkills
                | Self::Pi
                | Self::Opencode
                | Self::ClaudeCode
                | Self::Codex
                | Self::Copilot
                | Self::MarkdownOnly
        )
    }
}

// ---------------------------------------------------------------------------
// Render capability
// ---------------------------------------------------------------------------

/// A capability warning for a render profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RenderCapabilityWarning {
    /// The canonical field that is not fully supported by this profile.
    pub field: String,
    /// Why it is not supported.
    pub reason: String,
}

/// A warning about a resource that a render profile cannot represent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ExtendedRenderWarning {
    /// The resource ID.
    pub resource_id: String,
    /// The render profile that cannot represent this resource.
    pub profile: ExtendedRenderProfile,
    /// Why the profile cannot represent this resource.
    pub reason: String,
}

/// Get render capability warnings for a trait under a given profile.
///
/// Reports canonical fields that the target profile cannot fully represent.
pub fn render_capability_warnings(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
) -> Vec<RenderCapabilityWarning> {
    let mut warnings = Vec::new();

    // Markdown-only profiles cannot represent structured ports, signals,
    // procedures, or composition.
    if matches!(profile, ExtendedRenderProfile::MarkdownOnly) {
        if !trait_ref.ports.is_empty() {
            warnings.push(RenderCapabilityWarning {
                field: "port".to_string(),
                reason: "markdown-only profile does not render structured ports".to_string(),
            });
        }
        if !trait_ref.signals.is_empty() {
            warnings.push(RenderCapabilityWarning {
                field: "signal".to_string(),
                reason: "markdown-only profile does not render signals".to_string(),
            });
        }
        if trait_ref.procedure.is_some() {
            warnings.push(RenderCapabilityWarning {
                field: "procedure".to_string(),
                reason: "markdown-only profile does not render procedures".to_string(),
            });
        }
        if trait_ref.composition.is_some() {
            warnings.push(RenderCapabilityWarning {
                field: "composition".to_string(),
                reason: "markdown-only profile does not render composition".to_string(),
            });
        }
    }

    if has_agent_routing(trait_ref) {
        warnings.push(RenderCapabilityWarning {
            field: "agent".to_string(),
            reason: format!(
                "{} static render cannot enforce multi-agent frame routing; ctx runtime assignments and caller identity are required",
                profile.as_str()
            ),
        });
    }

    warnings.sort_by(|a, b| a.field.cmp(&b.field).then(a.reason.cmp(&b.reason)));
    warnings
}

fn has_agent_routing(trait_ref: &Trait) -> bool {
    if !trait_ref.agents.is_empty() {
        return true;
    }
    if trait_ref
        .procedure
        .as_ref()
        .is_some_and(|procedure| procedure.sequence.iter().any(|item| item.agent.is_some()))
    {
        return true;
    }
    trait_ref
        .sequences
        .iter()
        .any(|(_, sequence)| sequence.sequence.iter().any(|item| item.agent.is_some()))
}

// ---------------------------------------------------------------------------
// Render request and plan
// ---------------------------------------------------------------------------

/// A request to render a trait for a specific profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RenderRequest {
    /// The trait ID.
    pub trait_id: String,
    /// The render profile.
    pub profile: ExtendedRenderProfile,
    /// The source digest of the canonical trait.
    pub source_digest: Digest,
}

/// A planned render operation.
///
/// Pure-core planning: describes what will be rendered, what warnings apply,
/// and what resource inclusion is planned. The actual rendering and file
/// writing happen in later phases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RenderPlan {
    /// The trait ID.
    pub trait_id: String,
    /// The render profile.
    pub profile: ExtendedRenderProfile,
    /// The source digest of the canonical trait.
    pub source_digest: Digest,
    /// Capability warnings for this profile.
    pub capability_warnings: Vec<RenderCapabilityWarning>,
    /// Resource compatibility warnings from resource planning.
    pub resource_warnings: Vec<ExtendedRenderWarning>,
    /// Resource read warnings bridged from IO as displayable evidence.
    pub resource_read_warnings: Vec<String>,
    /// Resource manifest digest bridged from IO, if computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_manifest_digest: Option<Digest>,
    /// Planned resource inclusion for this render.
    pub resource_plan: Plan,
    /// Compiled model-visible output and digest evidence.
    pub model_view: Report,
    /// The generated-file marker text to include in output.
    pub generated_file_marker: String,
    /// Deterministic YAML frontmatter for profiles that use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<RenderFrontmatter>,
}

/// Render the deterministic Markdown export body described by a render plan.
///
/// Shared by both flat export formats (`compat` and `agents`): the body is
/// identical regardless of which filename it is ultimately written to.
pub fn render_export_content(plan: &RenderPlan) -> String {
    format!(
        "{}{}\n\n# {}\n\n{}\n",
        plan.frontmatter
            .as_ref()
            .map(|frontmatter| format!("{}\n\n", frontmatter.text))
            .unwrap_or_default(),
        plan.generated_file_marker,
        plan.trait_id,
        plan.model_view.full_text
    )
}

/// Render a progressive-disclosure Agent Skills-compatible `SKILL.md`.
///
/// This format is distinct from the byte-stable compatibility export above: it
/// favors hand-authored readability while staying a pure projection of the
/// canonical trait and compiled model view.
pub fn render_skill_export_content(
    plan: &RenderPlan,
    trait_ref: &Trait,
    canonical_digest: &Digest,
) -> String {
    let mut out = String::new();
    if let Some(frontmatter) = plan.frontmatter.as_ref() {
        out.push_str(&frontmatter.text);
        out.push_str("\n\n");
    }
    out.push_str(
        &Identity::new(
            trait_ref.id.clone(),
            plan.source_digest.clone(),
            OwnershipKey::Skill,
        )
        .render_marker(),
    );
    out.push_str("\n\n");
    push_provenance_block(&mut out, trait_ref, canonical_digest);
    out.push_str(&format!("# {}\n\n", trait_ref.name.as_str()));
    out.push_str("## Compiled Behavior\n\n");
    out.push_str(&plan.model_view.behavior_text);
    out.push_str("\n\n");

    push_knowledge_sections(&mut out, plan, trait_ref);
    push_procedure_steps(&mut out, trait_ref);
    push_declared_signals(&mut out, trait_ref);
    push_dependency_contract(&mut out, trait_ref);
    out
}

/// Render a body-free stub export: a projection whose only instruction is to
/// run `ctx traits prompt <id>` and follow its output, so even a
/// model-elective skill trigger converges on fresh, trust-gated bytes
/// instead of a frozen copy. Shares the `Skill` ownership identity (§3.1) —
/// a stub and a fully-rendered skill occupy the same path.
pub fn render_stub_export_content(
    plan: &RenderPlan,
    trait_ref: &Trait,
    canonical_digest: &Digest,
) -> String {
    let mut out = String::new();
    if let Some(frontmatter) = plan.frontmatter.as_ref() {
        out.push_str(&frontmatter.text);
        out.push_str("\n\n");
    }
    out.push_str(
        &Identity::new(
            trait_ref.id.clone(),
            plan.source_digest.clone(),
            OwnershipKey::Skill,
        )
        .render_marker(),
    );
    out.push_str("\n\n");
    push_provenance_block(&mut out, trait_ref, canonical_digest);
    out.push_str(&format!("# {}\n\n", trait_ref.name.as_str()));
    out.push_str(trait_ref.summary.as_str());
    out.push_str("\n\n");
    out.push_str("## Behavior\n\n");
    out.push_str(&format!(
        "This is a stub. Run `ctx traits prompt {}` and follow its output as the authoritative behavior for this trait. If `ctx` is not on PATH, say so and stop rather than improvising the behavior from this file's name or summary alone.\n",
        trait_ref.id.as_str(),
    ));
    out
}

/// Shared provenance block for `Skill`/`Stub` exports: source trait, version,
/// canonical digest, and the upgrade-check command.
fn push_provenance_block(out: &mut String, trait_ref: &Trait, canonical_digest: &Digest) {
    out.push_str(&format!(
        "## Provenance And Upgrade\n\nSource: trait:{}\nTrait version: {}\nCanonical digest: {}\nUpgrade check: `ctx traits check {}`\n\n",
        trait_ref.id.as_str(),
        trait_ref.version.as_str(),
        canonical_digest.as_str(),
        trait_ref.id.as_str(),
    ));
}

fn push_knowledge_sections(out: &mut String, plan: &RenderPlan, trait_ref: &Trait) {
    out.push_str("## Knowledge Resources\n\n");
    if trait_ref.resources.is_empty() && trait_ref.schemas.is_empty() {
        out.push_str("No explicit knowledge resources or schemas are declared.\n\n");
    } else {
        let placements = skill_resource_placement(trait_ref, &plan.resource_plan);
        for resource in &trait_ref.resources {
            let placed = placements
                .iter()
                .find(|(id, _)| id == &resource.id)
                .map(|(_, path)| path);
            let source_desc = match (
                resource.path.as_deref(),
                resource.effective_render(),
                placed,
            ) {
                (Some(_), crate::r#trait::ResourceRender::Reference, Some(placed)) => {
                    format!("Path: `{placed}` (placed alongside this file; reference)")
                }
                (Some(path), crate::r#trait::ResourceRender::Reference, None) => {
                    format!("Path: `{path}` (reference; not placed — see export-partial)")
                }
                (Some(path), crate::r#trait::ResourceRender::Inline, _) => {
                    format!("Path: `{path}` (rendered inline)")
                }
                (None, _, _) => "Source: inline content".to_string(),
            };
            out.push_str(&format!(
                "### resource:{}\n\n{}{}.\n\n",
                resource.id,
                source_desc,
                resource
                    .hint
                    .as_deref()
                    .map(|hint| format!(" Hint: {hint}"))
                    .unwrap_or_default(),
            ));
        }
        for schema in &trait_ref.schemas {
            out.push_str(&format!("### schema:{}\n\n", schema.id));
            if let Some(description) = schema.description.as_deref() {
                out.push_str(description);
                out.push_str("\n\n");
            }
            if let Some(resource) = schema.resource.as_deref() {
                out.push_str(&format!("Backed by `{resource}`.\n\n"));
            } else if let Some(fields) = schema.fields.as_ref() {
                out.push_str(&format!(
                    "Fields: {}.\n\n",
                    fields.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            } else if let Some(schema_ref) = schema.schema.as_deref() {
                out.push_str(&format!("Scalar schema: `{schema_ref}`.\n\n"));
            }
        }
    }
    push_model_section(out, plan, "Prompts", "## Prompt Resources\n\n");
}

/// Map each placeable resource of `trait_ref` to its companion file path
/// under a skill directory export: `resources/<resource-id>.<original-ext>`.
/// Resource ids are unique within a trait, so this mapping is
/// collision-free and stable, and it never reproduces a declared
/// package-relative path (no traversal risk, no nesting surprises).
///
/// Placeable: path-backed, successfully read (`digest_evidence` present —
/// absent for missing/symlinked resources), non-binary, and rendered as
/// [`crate::r#trait::ResourceRender::Reference`] (inline/checklist bodies
/// are already inlined in the rendered body; copying them out would
/// duplicate bytes). This is the one seam both the renderer (which must
/// print the placed path, not the declared package path) and the writer
/// derive placement from.
pub fn skill_resource_placement(
    trait_ref: &Trait,
    resource_plan: &Plan,
) -> Vec<(String, camino::Utf8PathBuf)> {
    let mut placements = Vec::new();
    for resource in &trait_ref.resources {
        if resource.path.is_none() {
            continue;
        }
        if resource.effective_render() != crate::r#trait::ResourceRender::Reference {
            continue;
        }
        let Some(entry) = resource_plan
            .entries
            .iter()
            .find(|entry| entry.resource_id == resource.id)
        else {
            continue;
        };
        let Some(digest_evidence) = entry.digest_evidence.as_ref() else {
            continue;
        };
        if digest_evidence.is_binary {
            continue;
        }
        let extension = resource
            .path
            .as_deref()
            .and_then(|path| camino::Utf8Path::new(path).extension())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_default();
        placements.push((
            resource.id.clone(),
            camino::Utf8PathBuf::from(format!("resources/{}{extension}", resource.id)),
        ));
    }
    placements
}

fn push_procedure_steps(out: &mut String, trait_ref: &Trait) {
    out.push_str("## Procedure Steps\n\n");
    let Some(procedure) = trait_ref.procedure.as_ref() else {
        out.push_str("No deterministic procedure is declared.\n\n");
        return;
    };
    out.push_str(&procedure.description);
    out.push_str("\n\n");
    if procedure.worktree_required {
        out.push_str(
            "Runtime requirement: start this procedure with prepared worktree provenance.\n\n",
        );
    }
    let ordered = ordered_skill_sequence_items(procedure);
    for (index, item) in ordered.iter().enumerate() {
        let title = if item.title.is_empty() {
            item.id.as_deref().unwrap_or("unnamed step")
        } else {
            item.title.as_str()
        };
        let construct = if item.effective_kind() == crate::r#trait::procedure::SequenceKind::Prompt
            && item.prompt.starts_with("prompt:")
        {
            item.prompt.as_str()
        } else {
            skill_sequence_kind(item.effective_kind())
        };
        out.push_str(&format!(
            "{}. {} (`{}`){}\n",
            index + 1,
            title,
            construct,
            item.agent
                .as_deref()
                .map(|agent| format!(" by `{agent}`"))
                .unwrap_or_default(),
        ));
        if !item.input.is_empty() {
            out.push_str(&format!(
                "   Input: `{}`.\n",
                item.input.ref_texts().collect::<Vec<_>>().join("`, `")
            ));
        }
        if !item.output.is_empty() {
            out.push_str(&format!(
                "   Output: `{}`.\n",
                item.output.ref_texts().collect::<Vec<_>>().join("`, `")
            ));
        }
        if let Some(sequence) = item.sequence.as_deref() {
            out.push_str(&format!("   Sequence target: `{sequence}`.\n"));
        }
        if let Some(when) = item.when.as_ref() {
            let guard =
                serde_json::to_string(when).unwrap_or_else(|_| "<unrenderable guard>".to_string());
            out.push_str(&format!("   Branch guard: `{guard}`.\n"));
        }
        if let Some(otherwise) = item.otherwise.as_deref() {
            out.push_str(&format!("   Otherwise target: `{otherwise}`.\n"));
        }
        if let Some(limit) = item.max_iterations {
            out.push_str(&format!("   Loop bound: max {limit} iteration(s).\n"));
        } else if let Some(source) = item.max_iterations_from.as_deref() {
            out.push_str(&format!("   Loop bound: resolved once from `{source}`.\n"));
        }
        if let Some(limit) = item.max_items {
            out.push_str(&format!("   For-each bound: max {limit} item(s).\n"));
        }
        out.push('\n');
    }
}

fn ordered_skill_sequence_items(
    procedure: &crate::r#trait::procedure::Model,
) -> Vec<&crate::r#trait::procedure::SequenceItem> {
    let Some(order) = procedure.sequence_order.as_ref() else {
        return procedure.sequence.iter().collect();
    };
    let by_id: std::collections::BTreeMap<&str, &crate::r#trait::procedure::SequenceItem> =
        procedure
            .sequence
            .iter()
            .filter_map(|item| item.id.as_deref().map(|id| (id, item)))
            .collect();
    order
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect()
}

fn push_declared_signals(out: &mut String, trait_ref: &Trait) {
    out.push_str("## Declared Signals\n\n");
    if trait_ref.signals.is_empty() {
        out.push_str("No explicit signals are declared.\n\n");
        return;
    }
    for signal in &trait_ref.signals {
        out.push_str(&format!("- `signal:{}`: {}", signal.id, signal.description));
        out.push('\n');
    }
    out.push('\n');
}

fn push_dependency_contract(out: &mut String, trait_ref: &Trait) {
    out.push_str("## Dependency Contract\n\n");
    if trait_ref.dependencies.is_empty() {
        out.push_str("No dependency traits are declared.\n");
        return;
    }
    for dependency in &trait_ref.dependencies {
        out.push_str(&format!(
            "- `{}` -> `{}` version `{}`\n",
            dependency.alias, dependency.id, dependency.version
        ));
    }
}

fn push_model_section(out: &mut String, plan: &RenderPlan, heading: &str, emitted_heading: &str) {
    if let Some(section) = model_section(plan, heading) {
        out.push_str(emitted_heading);
        out.push_str(section.content.trim());
        out.push_str("\n\n");
    }
}

fn model_section<'a>(
    plan: &'a RenderPlan,
    heading: &str,
) -> Option<&'a crate::model_view::Section> {
    plan.model_view
        .sections
        .iter()
        .find(|section| section.heading == heading)
}

fn skill_sequence_kind(kind: crate::r#trait::procedure::SequenceKind) -> &'static str {
    match kind {
        crate::r#trait::procedure::SequenceKind::Prompt => "prompt",
        crate::r#trait::procedure::SequenceKind::Ask => "ask",
        crate::r#trait::procedure::SequenceKind::Command => "command",
        crate::r#trait::procedure::SequenceKind::Check => "check",
        crate::r#trait::procedure::SequenceKind::Project => "project",
        crate::r#trait::procedure::SequenceKind::Sequence => "sequence",
        crate::r#trait::procedure::SequenceKind::Branch => "branch",
        crate::r#trait::procedure::SequenceKind::Loop => "loop",
        crate::r#trait::procedure::SequenceKind::ForEach => "for-each",
        crate::r#trait::procedure::SequenceKind::Parallel => "parallel",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RenderFrontmatter {
    pub profile: ExtendedRenderProfile,
    pub text: String,
    pub emitted_keys: Vec<String>,
    pub trusted_policy: bool,
    pub warning: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sanitization_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub normalizations: Vec<Normalization>,
}

/// Plan a render operation for a trait.
///
/// Computes capability warnings, resource compatibility warnings, and the
/// generated-file marker text. Pure — no filesystem or rendering execution.
pub fn plan_render(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
    source_digest: &str,
) -> RenderPlan {
    plan_render_with_evidence(trait_ref, profile, source_digest, &[], None, Vec::new())
}

/// Plan render with caller-supplied resource evidence.
pub fn plan_render_with_evidence(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
    source_digest: &str,
    resource_file_evidence: &[FileEvidence],
    resource_manifest_digest: Option<&str>,
    resource_read_warnings: Vec<String>,
) -> RenderPlan {
    plan_render_with_resource_body_evidence(
        trait_ref,
        profile,
        source_digest,
        ResourceEvidenceInputs {
            file_evidence: resource_file_evidence,
            body_evidence: &[],
            dependency_resources: &[],
            manifest_digest: resource_manifest_digest,
            read_warnings: resource_read_warnings,
        },
    )
}

/// Caller-supplied resource evidence for [`plan_render_with_resource_body_evidence`].
///
/// `dependency_resources` carries qualified local-dependency resource
/// declarations so their inclusions survive independently of body text; see
/// [`plan_resource_inclusion_with_bodies`]. Bundled into one struct because
/// the individual evidence pieces are always threaded together end to end
/// from IO through to the render plan.
pub struct ResourceEvidenceInputs<'a> {
    pub file_evidence: &'a [FileEvidence],
    pub body_evidence: &'a [BodyEvidence],
    pub dependency_resources: &'a [DependencyResourceDecl],
    pub manifest_digest: Option<&'a str>,
    pub read_warnings: Vec<String>,
}

/// Plan render with caller-supplied resource file and body-scan evidence.
pub fn plan_render_with_resource_body_evidence(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
    source_digest: &str,
    resource_evidence: ResourceEvidenceInputs<'_>,
) -> RenderPlan {
    let capability_warnings = render_capability_warnings(trait_ref, profile);
    let resource_plan = plan_resource_inclusion_with_bodies(
        trait_ref,
        resource_evidence.file_evidence,
        resource_evidence.body_evidence,
        resource_evidence.dependency_resources,
    );
    let resource_manifest_digest = resource_evidence.manifest_digest;
    let resource_read_warnings = resource_evidence.read_warnings;
    let resource_warnings = check_extended_render_compatibility(&resource_plan, profile);
    let model_view =
        compile_model_view_with_evidence(trait_ref, profile, Some(source_digest), &resource_plan);

    let generated_file_marker = Identity::new(
        trait_ref.id.clone(),
        Digest::from_unvalidated(source_digest),
        OwnershipKey::RenderProfile(profile),
    )
    .render_marker();
    let frontmatter = render_frontmatter(trait_ref, profile);

    RenderPlan {
        trait_id: trait_ref.id.as_str().to_string(),
        profile,
        source_digest: Digest::from_unvalidated(source_digest),
        capability_warnings,
        resource_warnings,
        resource_read_warnings,
        resource_manifest_digest: resource_manifest_digest.map(Digest::from_unvalidated),
        resource_plan,
        model_view,
        generated_file_marker,
        frontmatter,
    }
}

/// Claude's combined description+when-to-use cap, per §3.5. No Codex-specific
/// cap is invented (we cannot know the model's context size); the 2%-of-context
/// budget is instead respected by keeping descriptions short by construction
/// (summary + phrases, nothing else).
const DESCRIPTION_CAP: usize = 1536;

fn render_frontmatter(
    trait_ref: &Trait,
    profile: ExtendedRenderProfile,
) -> Option<RenderFrontmatter> {
    match profile {
        ExtendedRenderProfile::AgentSkills
        | ExtendedRenderProfile::ClaudeCode
        | ExtendedRenderProfile::Opencode
        | ExtendedRenderProfile::Codex
        | ExtendedRenderProfile::Pi => Some(skill_frontmatter(trait_ref, profile)),
        // Cursor Project Rules (`.cursor/rules/*.mdc`) and GitHub Copilot
        // path-specific instructions (`.github/instructions/*.instructions.md`)
        // are both silently ignored by their host unless the file opens with
        // host-required frontmatter.
        ExtendedRenderProfile::Copilot => Some(copilot_frontmatter(trait_ref)),
        _ => None,
    }
}

/// Frontmatter shared by every `SKILL.md`-shaped profile: `name`/`description`
/// from canonical name/summary plus declared activation phrases, and (Claude
/// only) a `paths:` key derived from positive `file-glob` predicates — the
/// sole profile whose frontmatter scoping key is binary-verified.
fn skill_frontmatter(trait_ref: &Trait, profile: ExtendedRenderProfile) -> RenderFrontmatter {
    let mut sanitization_warnings = Vec::new();
    let mut normalizations = Vec::new();
    let name = sanitize_model_text(
        trait_ref.name.as_str(),
        "frontmatter.name",
        &mut sanitization_warnings,
        &mut normalizations,
    );
    let summary = sanitize_model_text(
        trait_ref.summary.as_str(),
        "frontmatter.description",
        &mut sanitization_warnings,
        &mut normalizations,
    );

    let phrases = activation_phrases(trait_ref);
    let (description, description_warning) = compose_description(&summary, &phrases);

    let mut emitted_keys = vec!["name".to_string(), "description".to_string()];
    let mut lines = vec![
        format!("name: {}", yaml_quote(&name)),
        format!("description: {}", yaml_quote(&description)),
    ];
    let mut warning = "frontmatter is generated from canonical trait name/summary (plus declared activation phrases) only; unknown imported policy/tool fields are not echoed as trusted host behavior".to_string();
    if let Some(description_warning) = description_warning {
        warning.push_str("; ");
        warning.push_str(&description_warning);
    }

    if matches!(profile, ExtendedRenderProfile::ClaudeCode) {
        let (globs, dropped_excludes) = activation_globs(trait_ref);
        if !globs.is_empty() {
            lines.push(format!("paths:{}", yaml_string_list(&globs)));
            emitted_keys.push("paths".to_string());
        }
        if dropped_excludes {
            warning.push_str("; exclude-file-glob predicates have no frontmatter negation and were dropped rather than silently narrowing paths");
        }
    }

    RenderFrontmatter {
        profile,
        text: format!("---\n{}\n---", lines.join("\n")),
        emitted_keys,
        trusted_policy: false,
        warning,
        sanitization_warnings,
        normalizations,
    }
}

fn copilot_frontmatter(trait_ref: &Trait) -> RenderFrontmatter {
    let (globs, dropped_excludes) = activation_globs(trait_ref);
    let apply_to = if globs.is_empty() {
        "**".to_string()
    } else {
        globs.join(",")
    };
    let mut warning = "applyTo is the union of declared positive file-glob activation predicates, falling back to \"**\" when none are declared".to_string();
    if dropped_excludes {
        warning.push_str("; exclude-file-glob predicates have no frontmatter negation and were dropped rather than silently narrowing applyTo");
    }
    RenderFrontmatter {
        profile: ExtendedRenderProfile::Copilot,
        text: format!("---\napplyTo: {}\n---", yaml_quote(&apply_to)),
        emitted_keys: vec!["applyTo".to_string()],
        trusted_policy: false,
        warning,
        sanitization_warnings: Vec::new(),
        normalizations: Vec::new(),
    }
}

/// Union of positive `explicit-phrase` and `task-keyword` predicates across
/// every declared activation rule, in first-seen order, deduplicated.
fn activation_phrases(trait_ref: &Trait) -> Vec<String> {
    let mut phrases = Vec::new();
    if let Some(activation) = trait_ref.activation.as_ref() {
        for rule in &activation.rules {
            for phrase in rule.explicit_phrase.iter().chain(rule.task_keyword.iter()) {
                if !phrases.contains(phrase) {
                    phrases.push(phrase.clone());
                }
            }
        }
    }
    phrases
}

/// Union of positive `file-glob` predicates across every declared activation
/// rule, in first-seen order, deduplicated, plus whether any rule declared an
/// `exclude-file-glob` (which has no frontmatter negation and must be
/// reported as dropped rather than silently narrowing scope).
fn activation_globs(trait_ref: &Trait) -> (Vec<String>, bool) {
    let mut globs = Vec::new();
    let mut dropped_excludes = false;
    if let Some(activation) = trait_ref.activation.as_ref() {
        for rule in &activation.rules {
            for glob in rule.file_glob.iter() {
                if !globs.contains(glob) {
                    globs.push(glob.clone());
                }
            }
            if !rule.exclude_file_glob.is_empty() {
                dropped_excludes = true;
            }
        }
    }
    (globs, dropped_excludes)
}

/// Compose a frontmatter `description` from a sanitized summary plus optional
/// activation phrases, capped at [`DESCRIPTION_CAP`] and truncated
/// deterministically at a word boundary with a visible marker when over —
/// the P489 no-silent-truncation doctrine. Returns the description and, if
/// truncation occurred, a warning naming what was dropped. The cap applies to
/// the *emitted* value — content plus marker — not to the content alone: the
/// marker's own length is reserved out of the truncation budget first, via a
/// small fixed-point loop (the marker's length depends on its own dropped-count
/// digit width, which depends on how much content the marker leaves room for).
fn compose_description(summary: &str, phrases: &[String]) -> (String, Option<String>) {
    let mut description = summary.to_string();
    if !phrases.is_empty() {
        description.push_str(&format!(" Use when: {}.", phrases.join(", ")));
    }
    let total_chars = description.chars().count();
    if total_chars <= DESCRIPTION_CAP {
        return (description, None);
    }
    let mut content_cap = DESCRIPTION_CAP;
    let (content, marker) = loop {
        let content = truncate_at_word_boundary(&description, content_cap);
        let dropped_chars = total_chars - content.chars().count();
        let marker = format!(" …[truncated, {dropped_chars} char(s) dropped]");
        if content.chars().count() + marker.chars().count() <= DESCRIPTION_CAP {
            break (content, marker);
        }
        content_cap = DESCRIPTION_CAP.saturating_sub(marker.chars().count());
    };
    let emitted = format!("{content}{marker}");
    debug_assert!(emitted.chars().count() <= DESCRIPTION_CAP);
    let dropped_chars = total_chars - content.chars().count();
    (
        emitted,
        Some(format!(
            "description truncated at {DESCRIPTION_CAP} chars ({dropped_chars} char(s) dropped) to respect Claude's combined description+when-to-use cap"
        )),
    )
}

fn truncate_at_word_boundary(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    let truncated: String = text.chars().take(cap).collect();
    match truncated.rfind(char::is_whitespace) {
        Some(index) => truncated[..index].to_string(),
        None => truncated,
    }
}

fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn yaml_string_list(values: &[String]) -> String {
    let mut out = String::new();
    for value in values {
        out.push_str(&format!("\n  - {}", yaml_quote(value)));
    }
    out
}

fn check_extended_render_compatibility(
    plan: &Plan,
    profile: ExtendedRenderProfile,
) -> Vec<ExtendedRenderWarning> {
    if !profile.is_text_only() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    for entry in &plan.entries {
        if let Some(ref evidence) = entry.digest_evidence {
            if evidence.is_binary {
                warnings.push(ExtendedRenderWarning {
                    resource_id: entry.resource_id.clone(),
                    profile,
                    reason: format!(
                        "binary resource {:?} cannot be represented by text-only profile {}",
                        entry.resource_id,
                        profile.as_str()
                    ),
                });
            }
        }
    }

    warnings.sort_by(|a, b| {
        a.resource_id
            .cmp(&b.resource_id)
            .then(a.reason.cmp(&b.reason))
    });
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn representative_trait() -> Trait {
        serde_json::from_value(serde_json::json!({
            "id": "example-trait",
            "schema-version": "0.2",
            "version": "1.0.0",
            "name": "Example Trait",
            "summary": "A representative trait."
        }))
        .expect("representative trait is valid")
    }

    #[test]
    // Behavioral assertions only — parse/inspect structure, never
    // byte-compare a frozen string (P461: the byte-compare battery and
    // goldens are out of the gate since 2026-07-24; render v2 re-stamps
    // every model-visible byte per rule 9).
    fn export_rendering_has_expected_structure() {
        let trait_ref = representative_trait();
        let compat = plan_render(
            &trait_ref,
            ExtendedRenderProfile::MarkdownOnly,
            "source-digest",
        );
        let compat_with_frontmatter = plan_render(
            &trait_ref,
            ExtendedRenderProfile::AgentSkills,
            "source-digest",
        );

        let plain = render_export_content(&compat);
        assert!(plain.contains("> GENERATED FILE - DO NOT EDIT DIRECTLY"));
        assert!(plain.contains("> Render profile: markdown-only"));
        assert!(plain.contains("# example-trait"));
        assert!(
            plain.contains("<trait id=\"example-trait\" version=\"1.0.0\" model-view=\"sha256:")
        );
        assert!(plain.contains("<summary>"));
        assert!(plain.contains("A representative trait."));
        assert!(!plain.contains("## Provenance"));
        assert!(!plain.contains("## Identity"));

        let with_frontmatter = render_export_content(&compat_with_frontmatter);
        assert!(with_frontmatter.starts_with("---\nname: \"Example Trait\""));
        assert!(with_frontmatter.contains("> Render profile: agent-skills"));
        assert!(with_frontmatter.contains("<summary>"));
    }

    #[test]
    fn skill_export_rendering_has_expected_structure() {
        let trait_ref = representative_trait();
        let plan = plan_render(
            &trait_ref,
            ExtendedRenderProfile::AgentSkills,
            "source-digest",
        );
        let canonical_digest = Digest::from_unvalidated("canonical-digest");
        let content = render_skill_export_content(&plan, &trait_ref, &canonical_digest);
        assert!(content.starts_with("---\nname: \"Example Trait\""));
        assert!(content.contains("> Render profile: skill"));
        assert!(content.contains("## Provenance And Upgrade"));
        assert!(content.contains("# Example Trait"));
        assert!(content.contains("## Compiled Behavior"));
        assert_eq!(content.matches(&plan.model_view.behavior_text).count(), 1);
        assert!(content.contains("## Procedure Steps"));
        assert!(content.contains("## Declared Signals"));
        assert!(content.contains("## Dependency Contract"));
    }

    #[test]
    fn skill_behavior_payload_reuses_the_sanitized_compiled_envelope_once() {
        let trait_ref: Trait = serde_json::from_value(serde_json::json!({
            "id": "sanitized-skill-fixture",
            "schema-version": "0.2",
            "version": "1.0.0",
            "name": "Sanitized Skill Fixture",
            "summary": "Visible <!-- hidden --> summary."
        }))
        .expect("sanitized skill fixture is valid");
        let plan = plan_render(
            &trait_ref,
            ExtendedRenderProfile::AgentSkills,
            "source-digest",
        );
        let content = render_skill_export_content(
            &plan,
            &trait_ref,
            &Digest::from_unvalidated("canonical-digest"),
        );

        assert!(!plan.model_view.behavior_text.contains("<!--"));
        assert_eq!(content.matches(&plan.model_view.behavior_text).count(), 1);
    }
}
