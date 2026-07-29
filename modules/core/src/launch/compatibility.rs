// Launch compatibility reporting.
/// Launch compatibility checks.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HostCompatibilityMatrix {
    pub profiles: Vec<HostCompatibilityProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HostCompatibilityProfile {
    pub profile: String,
    pub supported_paths: Vec<String>,
    pub render_profile: String,
    pub activation_approximation: String,
    pub resource_support: String,
    pub script_support: String,
    pub policy_enforceability: String,
    pub generated_marker_support: String,
    pub unsupported_fields: Vec<String>,
    pub recommended_fallback: String,
}

pub fn compatibility_matrix() -> HostCompatibilityMatrix {
    let profiles = [
        ExtendedRenderProfile::AgentSkills,
        ExtendedRenderProfile::MarkdownOnly,
        ExtendedRenderProfile::Opencode,
        ExtendedRenderProfile::Codex,
        ExtendedRenderProfile::ClaudeCode,
        ExtendedRenderProfile::Pi,
    ]
    .into_iter()
    .map(compatibility_profile)
    .collect();
    HostCompatibilityMatrix { profiles }
}

fn compatibility_profile(profile: ExtendedRenderProfile) -> HostCompatibilityProfile {
    match profile {
        ExtendedRenderProfile::AgentSkills => compat(CompatSpec {
            profile: "agent-skills",
            supported_paths: &[".agents/skills/<trait-id>/SKILL.md"],
            render_profile: "agent-skills",
            activation_approximation: "static markdown selection by host; canonical activation is advisory after export",
            resource_support: "text summaries and declared paths only; resource bodies remain package evidence",
            script_support: "no script execution by ctx.traits export",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &[],
            recommended_fallback: "keep canonical trait as source and regenerate SKILL.md",
        }),
        ExtendedRenderProfile::MarkdownOnly => compat(CompatSpec {
            profile: "agents-md",
            supported_paths: &["AGENTS.md", "CLAUDE.md", ".cursor/rules/*.mdc"],
            render_profile: "markdown-only",
            activation_approximation: "manual/static inclusion; no dynamic activation claim",
            resource_support: "inline text only; structured resources become warnings",
            script_support: "host-specific outside ctx.traits",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &["structured ports", "signals", "runtime procedure ledger"],
            recommended_fallback: "render concise advisory text plus link to evidence bundle",
        }),
        ExtendedRenderProfile::Opencode => compat(CompatSpec {
            profile: "opencode",
            supported_paths: &[
                ".opencode/skills/<trait-id>/SKILL.md",
            ],
            render_profile: "opencode",
            activation_approximation: "static skill plus optional reviewed plugin action plan when host capability exists",
            resource_support: "text resources and explicit MCP/resource plans only",
            script_support: "plugin scaffold may plan actions but does not execute core algorithms",
            policy_enforceability: "host-hook-plan where can-* capability evidence exists; otherwise advisory",
            unsupported_fields: &["unproven dynamic injection", "automatic install"],
            recommended_fallback: "use static export unless capability report proves hook support",
        }),
        ExtendedRenderProfile::Codex => compat(CompatSpec {
            profile: "codex",
            supported_paths: &[".github/skills/<trait-id>/SKILL.md"],
            render_profile: "codex",
            activation_approximation: "static file approximation",
            resource_support: "text only",
            script_support: "unsupported by ctx.traits",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &["dynamic hooks", "runtime enforcement"],
            recommended_fallback: "render warnings and keep canonical evidence nearby",
        }),
        ExtendedRenderProfile::ClaudeCode => compat(CompatSpec {
            profile: "claude-code",
            supported_paths: &[".claude/skills/<trait-id>/SKILL.md"],
            render_profile: "claude-code",
            activation_approximation: "static skill approximation",
            resource_support: "text only",
            script_support: "unsupported by ctx.traits",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &["dynamic hooks", "runtime enforcement"],
            recommended_fallback: "prefer static generated skill with explicit non-enforcement warning",
        }),
        ExtendedRenderProfile::Pi => compat(CompatSpec {
            profile: "pi",
            supported_paths: &[".pi/skills/<trait-id>/SKILL.md"],
            render_profile: "pi",
            activation_approximation: "static profile approximation",
            resource_support: "text only",
            script_support: "unsupported by ctx.traits",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &["dynamic hooks", "runtime enforcement"],
            recommended_fallback: "use static export and inspect warnings",
        }),
        ExtendedRenderProfile::Copilot => compat(CompatSpec {
            profile: "copilot",
            supported_paths: &["explicit --out required"],
            render_profile: "copilot",
            activation_approximation: "static markdown approximation",
            resource_support: "text only",
            script_support: "unsupported by ctx.traits",
            policy_enforceability: "advisory markdown only",
            unsupported_fields: &["default export directory", "dynamic hooks"],
            recommended_fallback: "use explicit output directory and review semantic loss",
        }),
    }
}

struct CompatSpec<'a> {
    profile: &'a str,
    supported_paths: &'a [&'a str],
    render_profile: &'a str,
    activation_approximation: &'a str,
    resource_support: &'a str,
    script_support: &'a str,
    policy_enforceability: &'a str,
    unsupported_fields: &'a [&'a str],
    recommended_fallback: &'a str,
}

fn compat(spec: CompatSpec<'_>) -> HostCompatibilityProfile {
    HostCompatibilityProfile {
        profile: spec.profile.to_string(),
        supported_paths: spec
            .supported_paths
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        render_profile: spec.render_profile.to_string(),
        activation_approximation: spec.activation_approximation.to_string(),
        resource_support: spec.resource_support.to_string(),
        script_support: spec.script_support.to_string(),
        policy_enforceability: spec.policy_enforceability.to_string(),
        generated_marker_support: "visible generated-file marker in rendered Markdown".to_string(),
        unsupported_fields: spec
            .unsupported_fields
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        recommended_fallback: spec.recommended_fallback.to_string(),
    }
}
