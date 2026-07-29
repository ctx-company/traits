// Launch publication preparation reporting.
/// Launch publication preparation.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PublishPrepReport {
    pub trait_id: String,
    pub findings: Vec<PublishFinding>,
    pub packaging_plan: Vec<PackagingPlanEntry>,
    pub launch_kit: Vec<String>,
    pub requires_human_review: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PublishFinding {
    pub field: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PackagingPlanEntry {
    pub part: String,
    pub recommendation: String,
    pub provenance: String,
}

pub fn publish_prep_report(trait_ref: &Trait) -> PublishPrepReport {
    let mut findings = Vec::new();
    scan_private_text(&mut findings, "summary", trait_ref.summary.as_str());
    for resource in &trait_ref.resources {
        if let Some(path) = resource.path.as_deref() {
            scan_private_text(&mut findings, &format!("resource.{}.path", resource.id), path);
        }
        if let Some(hint) = &resource.hint {
            scan_private_text(
                &mut findings,
                &format!("resource.{}.hint", resource.id),
                hint,
            );
        }
    }
    for (id, prompt) in trait_ref.prompts.iter() {
        if let Some(text) = prompt.text.as_deref() {
            scan_private_text(&mut findings, &format!("prompt.{id}.text"), text);
        }
        if let Some(source) = prompt.source.as_deref() {
            scan_private_text(&mut findings, &format!("prompt.{id}.source"), source);
        }
    }
    let packaging_plan = vec![
        PackagingPlanEntry {
            part: "public-core".to_string(),
            recommendation: "include canonical behavior, activation rationale, non-private prompts, and host compatibility warnings".to_string(),
            provenance: Digest::source(trait_ref.id.as_str()).as_str().to_string(),
        },
        PackagingPlanEntry {
            part: "private-overlay".to_string(),
            recommendation: "keep private resources, internal paths, customer references, and local tool assumptions outside the public core".to_string(),
            provenance: "local-review-required".to_string(),
        },
    ];
    PublishPrepReport {
        trait_id: trait_ref.id.as_str().to_string(),
        findings,
        packaging_plan,
        launch_kit: vec![
            "README with tested profiles and known unsupported fields".to_string(),
            "demo checklist tied to claim/evidence matrix".to_string(),
            "setup and removal steps that do not imply host enforcement".to_string(),
        ],
        requires_human_review: true,
    }
}

fn scan_private_text(findings: &mut Vec<PublishFinding>, field: &str, text: &str) {
    let lowered = text.to_ascii_lowercase();
    let patterns: &[(&str, &[&str])] = &[
        ("private-path", &["/users/", "/var/", "c:\\", "~/"]),
        (
            "internal-domain",
            &[".internal", "corp.", "intranet", "localhost"],
        ),
        (
            "credential-like",
            &["api_key", "apikey", "token=", "secret", "password"],
        ),
        ("private-issue", &["jira-", "linear-", "gh-"]),
    ];
    for (kind, values) in patterns {
        if values.iter().any(|needle| lowered.contains(needle)) {
            findings.push(PublishFinding {
                field: field.to_string(),
                kind: kind.to_string(),
                message: "potentially private or organization-specific text requires human redaction review".to_string(),
            });
        }
    }
}
