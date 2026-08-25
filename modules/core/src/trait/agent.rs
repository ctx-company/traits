//! Agent role declarations for portable multi-harness routing.
//!
//! `[[agent]]` declares an abstract role such as `worker` or `reviewer`.
//! Concrete harness assignments stay outside trait source; this module only
//! validates the portable role identities used by procedure sequence items.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Behavior, Intent};

/// Model-quality intent attached to built-in authoring templates.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum AgentModelTier {
    /// Prefer the highest-quality configured model for difficult consultation.
    Top,
    /// Prefer a fast configured model for retrieval work.
    Fast,
}

impl AgentModelTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Fast => "fast",
        }
    }
}

/// A built-in authoring template that materializes as an ordinary [`Agent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentTemplate {
    /// Template name used by authoring SDKs.
    pub name: &'static str,

    /// Default role description.
    pub description: &'static str,

    /// Default short role summary.
    pub summary: &'static str,

    /// Optional model-quality intent for runtime configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<AgentModelTier>,
}

/// Built-in agent role templates in stable SDK generation order.
pub static AGENT_TEMPLATES: &[AgentTemplate] = &[
    AgentTemplate {
        name: "worker",
        description: "Completes assigned work and produces the requested outputs.",
        summary: "Execution role.",
        model_tier: None,
    },
    AgentTemplate {
        name: "reviewer",
        description: "Reviews assigned work against its requirements and reports actionable findings.",
        summary: "Review role.",
        model_tier: None,
    },
    AgentTemplate {
        name: "planner",
        description: "Structures assigned work into a clear, actionable plan.",
        summary: "Planning role.",
        model_tier: None,
    },
    AgentTemplate {
        name: "oracle",
        description: "Provides high-quality consultation for difficult decisions without owning execution.",
        summary: "Top-tier consultation role.",
        model_tier: Some(AgentModelTier::Top),
    },
    AgentTemplate {
        name: "searcher",
        description: "Retrieves and summarizes relevant information without modifying the workspace.",
        summary: "Fast read-only retrieval role.",
        model_tier: Some(AgentModelTier::Fast),
    },
];

/// A portable abstract agent role declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Agent {
    /// Role identifier, referenced by sequence items as `agent:<id>`.
    pub id: String,

    /// Required human-readable role description.
    pub description: String,

    /// Optional short role summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Optional standing instructions for this role, delivered through the
    /// harness's system-prompt channel once per session rather than repeated
    /// in every frame's user message. Holds doctrine the role always obeys —
    /// not per-step work, which belongs in the step's own prompt. Model-visible
    /// like any other instruction text: compiled into the model view, audited
    /// for hidden content, and covered by the canonical digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    /// Optional declaration-only guidance for this role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,

    /// Optional declaration-only behavioral guidance for this role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<Behavior>,

    /// Optional session binding for this agent's frames: `session:<id>`
    /// (shared with every other agent bound to the same declared session),
    /// or a bare `per-frame`/`persistent` lifecycle value, which is always
    /// agent-local and never implies sharing. Omitted preserves legacy
    /// configuration-driven behavior during migration; absent both canonical
    /// and configured values remains `per-frame`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// Instantiate a built-in template as an ordinary agent declaration.
pub fn instantiate_agent_template(
    template: &AgentTemplate,
    id: impl Into<String>,
    description: Option<String>,
    summary: Option<String>,
) -> Agent {
    Agent {
        id: id.into(),
        description: description.unwrap_or_else(|| template.description.to_string()),
        summary: Some(summary.unwrap_or_else(|| template.summary.to_string())),
        system: None,
        intent: None,
        behavior: None,
        session: None,
    }
}

/// Agent-local lifecycle value: a fresh session per frame, never shared.
pub const SESSION_LIFECYCLE_PER_FRAME: &str = "per-frame";
/// Agent-local lifecycle value: one session for the agent's whole lifetime, never shared.
pub const SESSION_LIFECYCLE_PERSISTENT: &str = "persistent";

/// Validate `[[agent]]` declarations.
pub fn validate_agents(agents: &[Agent], schema_version: &str) -> crate::Result<()> {
    let mut seen_ids = BTreeSet::new();

    for (i, agent) in agents.iter().enumerate() {
        let id_path = format!("agent[{i}].id");
        crate::shared::validate_slug_shape(&agent.id, &id_path)?;
        if !seen_ids.insert(agent.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate agent id {:?}", agent.id),
            }
            .into());
        }

        if agent.description.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("agent[{i}].description"),
                message: "must not be empty".to_string(),
            }
            .into());
        }

        if agent
            .summary
            .as_deref()
            .is_some_and(|summary| summary.trim().is_empty())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("agent[{i}].summary"),
                message: "must not be empty when supplied".to_string(),
            }
            .into());
        }

        if let Some(intent) = &agent.intent {
            if !super::schema_version_at_least(schema_version, "0.6") {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("agent[{i}].intent"),
                    message: "requires schema-version \"0.6\" or later".to_string(),
                }
                .into());
            }
            intent.validate_scoped(&format!("agent[{i}].intent"))?;
        }

        if let Some(behavior) = &agent.behavior {
            if !super::schema_version_at_least(schema_version, "0.6") {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("agent[{i}].behavior"),
                    message: "requires schema-version \"0.6\" or later".to_string(),
                }
                .into());
            }
            behavior.validate_scoped(&format!("agent[{i}].behavior"))?;
        }

        if agent
            .system
            .as_deref()
            .is_some_and(|system| system.trim().is_empty())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("agent[{i}].system"),
                message: "must not be empty when supplied".to_string(),
            }
            .into());
        }
    }

    Ok(())
}

/// Validate `[[agent]].session` bindings against declared `[[session]]` ids.
///
/// A binding of `per-frame`/`persistent` is agent-local and requires no
/// cross-reference. A `session:<id>` binding must be a local, unqualified
/// `Kind::Session` reference resolving to a declared session. Assignment
/// compatibility (harness/transport agreement, serialized concurrent access)
/// is P328 runtime scope, not decode-time validation.
pub fn validate_agent_session_bindings(
    agents: &[Agent],
    session_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    for (i, agent) in agents.iter().enumerate() {
        let Some(binding) = agent.session.as_deref() else {
            continue;
        };
        let field_path = format!("agent[{i}].session");

        let parsed = super::session::parse_session_binding(binding, &field_path)?;

        if let super::session::SessionBinding::Shared(id) = parsed
            && !session_ids.contains(id.as_str())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path,
                message: format!("unresolved local session ref {binding:?}"),
            }
            .into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(system: Option<&str>) -> Agent {
        Agent {
            id: "smart-1".to_string(),
            description: "Reviews the work.".to_string(),
            summary: None,
            system: system.map(str::to_string),
            intent: None,
            behavior: None,
            session: None,
        }
    }

    #[test]
    fn system_instructions_are_accepted() {
        validate_agents(
            &[agent(Some("Approve only what you verified with tools."))],
            "0.6",
        )
        .expect("standing instructions are a valid agent declaration");
    }

    #[test]
    fn blank_system_instructions_are_rejected() {
        let error = validate_agents(&[agent(Some("   \n  "))], "0.6")
            .expect_err("a whitespace-only system field is authoring noise, not instructions");
        assert!(
            format!("{error}").contains("agent[0].system"),
            "error must name the offending field: {error}"
        );
    }

    #[test]
    fn absent_system_leaves_canonical_bytes_unchanged() {
        // The canonical digest keys machine trust, so adding this field must not
        // silently invalidate every already-reviewed trait in the wild.
        let without = crate::digest::canonical_json(&agent(None)).expect("canonical json");
        assert_eq!(
            without, r#"{"description":"Reviews the work.","id":"smart-1"}"#,
            "an unset system field must not appear in canonical form"
        );

        let with = crate::digest::canonical_json(&agent(Some("Verify before approving.")))
            .expect("canonical json");
        assert!(
            with.contains(r#""system":"Verify before approving.""#),
            "a set system field must be part of the canonical, digested document: {with}"
        );
    }

    #[test]
    fn agent_intent_is_scoped_to_schema_0_6_and_rejects_only_require_avoid_collisions() {
        let mut guided = agent(None);
        guided.intent = Some(
            serde_json::from_value(serde_json::json!({
                "require": ["correctness"],
                "focus": ["correctness"],
                "avoid": ["correctness"],
                "block": ["correctness"],
            }))
            .expect("intent fixture"),
        );

        for version in ["0.2", "0.3", "0.4", "0.5"] {
            let error = validate_agents(&[guided.clone()], version)
                .expect_err("pre-0.6 rejects agent intent");
            assert!(error.to_string().contains("agent[0].intent"));
        }
        let error = validate_agents(&[guided], "0.6").expect_err("require/avoid collision rejects");
        assert!(error.to_string().contains("agent[0].intent.avoid[0].id"));

        let mut overlapping_non_conflicting = agent(None);
        overlapping_non_conflicting.intent = Some(
            serde_json::from_value(serde_json::json!({
                "focus": ["correctness"],
                "block": ["correctness"],
            }))
            .expect("intent fixture"),
        );
        validate_agents(&[overlapping_non_conflicting], "0.6")
            .expect("focus/block overlap remains valid");
    }

    #[test]
    fn agent_intent_validates_all_groups_and_has_stable_populated_canonical_bytes() {
        let mut guided = agent(None);
        guided.intent = Some(
            serde_json::from_value(serde_json::json!({
                "require": ["correctness", { "id": "custom-require", "summary": "Preserve relevant evidence." }],
                "focus": [{ "id": "custom-focus", "description": "Inspect boundary conditions." }],
                "avoid": ["scope-creep", { "id": "custom-avoid", "summary": "Avoid unrelated changes." }],
                "block": [{ "id": "custom-block", "description": "Reject unverifiable claims." }]
            }))
            .expect("intent fixture"),
        );

        validate_agents(&[guided.clone()], "0.6").expect("all guidance forms are valid");
        let first = crate::digest::canonical_json(&guided).expect("canonical json");
        let second = crate::digest::canonical_json(&guided).expect("canonical json");
        assert_eq!(
            first,
            r#"{"description":"Reviews the work.","id":"smart-1","intent":{"avoid":[{"id":"scope-creep"},{"id":"custom-avoid","summary":"Avoid unrelated changes."}],"block":[{"description":"Reject unverifiable claims.","id":"custom-block"}],"focus":[{"description":"Inspect boundary conditions.","id":"custom-focus"}],"require":[{"id":"correctness"},{"id":"custom-require","summary":"Preserve relevant evidence."}]}}"#,
            "populated agent intent must serialize deterministically"
        );
        assert_eq!(first, second);
    }

    #[test]
    fn agent_intent_reports_shared_guidance_failures_at_indexed_paths() {
        for (intent, path) in [
            (
                serde_json::json!({ "require": ["same", "same"] }),
                "agent[0].intent.require[1].id",
            ),
            (
                serde_json::json!({ "focus": ["Not A Slug"] }),
                "agent[0].intent.focus[0].id",
            ),
            (
                serde_json::json!({ "avoid": [{ "id": "blank", "summary": "  " }] }),
                "agent[0].intent.avoid[0].summary",
            ),
            (
                serde_json::json!({ "block": [{ "id": "second-person", "description": "You must verify evidence." }] }),
                "agent[0].intent.block[0].description",
            ),
        ] {
            let mut guided = agent(None);
            guided.intent = Some(serde_json::from_value(intent).expect("intent fixture"));
            let error = validate_agents(&[guided], "0.6").expect_err("invalid scoped guidance");
            assert!(
                error.to_string().contains(path),
                "expected {path}, got {error}"
            );
        }
    }

    #[test]
    fn agent_intent_only_rejects_canonical_require_avoid_id_intersections() {
        let mut prose_match = agent(None);
        prose_match.intent = Some(
            serde_json::from_value(serde_json::json!({
                "require": [{ "id": "first", "summary": "Preserve shared prose." }],
                "avoid": [{ "id": "second", "summary": "Preserve shared prose." }],
                "focus": ["first"],
                "block": ["second"]
            }))
            .expect("intent fixture"),
        );
        validate_agents(&[prose_match], "0.6").expect("prose and focus/block overlaps are valid");

        let mut collision = agent(None);
        collision.intent = Some(
            serde_json::from_value(serde_json::json!({ "require": ["same"], "avoid": ["same"] }))
                .expect("intent fixture"),
        );
        let error = validate_agents(&[collision], "0.6").expect_err("id intersection rejects");
        assert!(error.to_string().contains("agent[0].intent.avoid[0].id"));
    }

    #[test]
    fn absent_agent_intent_is_omitted_from_canonical_bytes() {
        let without = crate::digest::canonical_json(&agent(None)).expect("canonical json");
        assert!(!without.contains("intent"));
    }

    #[test]
    fn agent_behavior_decodes_all_axes_from_builtin_custom_and_rich_forms() {
        let mut guided = agent(None);
        guided.behavior = Some(
            serde_json::from_value(serde_json::json!({
                "tone": ["direct", { "id": "custom-tone", "summary": "State conclusions plainly." }],
                "method": "evidence-first",
                "format": [{ "id": "custom-format", "description": "Use concise bullets." }],
                "verbosity": "brief",
                "directness": "direct",
                "scope-control": "strict",
                "initiative": "proactive",
                "uncertainty": "explicit"
            }))
            .expect("behavior fixture"),
        );
        validate_agents(&[guided], "0.6").expect("all behavior axes and guidance forms are valid");
    }

    #[test]
    fn agent_behavior_requires_schema_0_6() {
        let mut guided = agent(None);
        guided.behavior = Some(
            serde_json::from_value(serde_json::json!({ "tone": "direct" }))
                .expect("behavior fixture"),
        );
        for version in ["0.2", "0.3", "0.4", "0.5"] {
            let error = validate_agents(&[guided.clone()], version)
                .expect_err("pre-0.6 rejects agent behavior");
            assert!(error.to_string().contains("agent[0].behavior"));
        }
    }

    #[test]
    fn agent_behavior_rejects_arrays_on_scalar_axes_during_decode() {
        let scalar_array: Result<Behavior, _> =
            serde_json::from_value(serde_json::json!({ "verbosity": ["brief"] }));
        assert!(
            scalar_array.is_err(),
            "scalar behavior axes must not decode arrays"
        );
    }

    #[test]
    fn agent_behavior_reports_shared_validation_failures_at_indexed_paths() {
        let mut guided = agent(None);
        guided.behavior = Some(
            serde_json::from_value(serde_json::json!({ "tone": ["direct", "direct"] }))
                .expect("behavior fixture"),
        );
        let error =
            validate_agents(&[guided], "0.6").expect_err("duplicate additive guidance rejects");
        assert!(error.to_string().contains("agent[0].behavior.tone[1].id"));
    }

    #[test]
    fn agent_behavior_is_deterministic_and_absence_preserves_legacy_bytes() {
        let mut guided = agent(None);
        guided.behavior = Some(
            serde_json::from_value(serde_json::json!({ "tone": "direct", "verbosity": "brief" }))
                .expect("behavior fixture"),
        );
        let first = crate::digest::canonical_json(&guided).expect("canonical json");
        let second = crate::digest::canonical_json(&guided).expect("canonical json");
        assert_eq!(
            first,
            r#"{"behavior":{"tone":[{"id":"direct"}],"verbosity":{"id":"brief"}},"description":"Reviews the work.","id":"smart-1"}"#
        );
        assert_eq!(first, second);

        let without = crate::digest::canonical_json(&agent(None)).expect("canonical json");
        assert_eq!(
            without,
            r#"{"description":"Reviews the work.","id":"smart-1"}"#
        );
    }
}
