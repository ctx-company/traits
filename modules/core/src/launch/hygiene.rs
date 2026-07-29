// Launch hygiene reporting.
/// Launch hygiene checks.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitHygieneReport {
    pub traits: Vec<TraitHygieneEntry>,
    pub inventory: Vec<TraitInventoryEntry>,
    pub prune_plan: Vec<PrunePlanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitHygieneEntry {
    pub trait_id: String,
    pub action: String,
    pub findings: Vec<HygieneFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct HygieneFinding {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitInventoryEntry {
    pub trait_id: String,
    pub why_this_exists: String,
    pub when_it_should_trigger: String,
    pub conflicts_with: Vec<String>,
    pub last_reviewed: String,
    pub render_targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PrunePlanEntry {
    pub trait_id: String,
    pub recommendation: String,
    pub reason: String,
    pub replacement_suggestions: Vec<String>,
    pub render_export_impact: String,
    pub requires_review: bool,
}

/// `lifecycle` carries the caller-resolved `(package status, trust verdict)`
/// pair for each entry in `traits`, in the same order — the canonical trait
/// document has no status/trust field of its own.
pub fn hygiene_report(
    traits: &[Trait],
    lifecycle: &[(PackageStatus, TrustVerdict)],
) -> TraitHygieneReport {
    let mut entries = traits
        .iter()
        .zip(lifecycle.iter())
        .map(|(t, (status, trust))| hygiene_entry(t, status, trust))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.trait_id.cmp(&b.trait_id));

    let duplicate_summary_ids = duplicate_summaries(traits);
    for entry in &mut entries {
        if duplicate_summary_ids.contains(&entry.trait_id) {
            entry.findings.push(HygieneFinding {
                code: "hygiene.duplicate-summary".to_string(),
                severity: "advisory".to_string(),
                message: "another loaded trait has the same summary; review overlap before publishing or pruning".to_string(),
            });
        }
    }
    add_catalog_hygiene_findings(traits, &mut entries);
    for entry in &mut entries {
        entry
            .findings
            .sort_by(|a, b| a.code.cmp(&b.code).then(a.message.cmp(&b.message)));
        entry.action = recommended_action(&entry.findings).to_string();
    }

    let mut inventory = traits
        .iter()
        .zip(lifecycle.iter())
        .map(|(t, (status, _trust))| inventory_entry(t, status))
        .collect::<Vec<_>>();
    inventory.sort_by(|a, b| a.trait_id.cmp(&b.trait_id));

    let prune_plan = entries
        .iter()
        .filter(|entry| {
            entry
                .findings
                .iter()
                .any(|finding| finding.code == "hygiene.trust-blocked")
        })
        .map(|entry| PrunePlanEntry {
            trait_id: entry.trait_id.clone(),
            recommendation: "plan-deprecation-or-removal".to_string(),
            reason: "machine trust store records this canonical digest as blocked; keep source \
                     unchanged until explicit review"
                .to_string(),
            replacement_suggestions: Vec::new(),
            render_export_impact:
                "derived host exports should be regenerated after a reviewed removal or replacement"
                    .to_string(),
            requires_review: true,
        })
        .collect();

    TraitHygieneReport {
        traits: entries,
        inventory,
        prune_plan,
    }
}

fn hygiene_entry(trait_ref: &Trait, status: &PackageStatus, trust: &TrustVerdict) -> TraitHygieneEntry {
    let mut findings = Vec::new();
    if matches!(trust, TrustVerdict::Blocked) {
        findings.push(finding(
            "hygiene.trust-blocked",
            "warning",
            "machine trust store records this canonical digest as blocked; generate a prune plan but do not mutate files automatically",
        ));
    }
    if matches!(status, PackageStatus::Draft) {
        findings.push(finding(
            "hygiene.unreviewed",
            "warning",
            "package status is draft; keep out of launch evidence until reviewed and activated",
        ));
    }
    if !matches!(trust, TrustVerdict::Verified) {
        findings.push(finding(
            "hygiene.untrusted",
            "warning",
            "machine trust store has no verified record for this canonical digest; require human review before team-wide activation",
        ));
    }
    findings.push(finding(
        "hygiene.owner-reviewer-not-modeled",
        "advisory",
        "current canonical metadata has no owner/reviewer field; capture reviewer evidence outside the trait or in a future schema",
    ));
    if let Some(activation) = &trait_ref.activation {
        for rule in &activation.rules {
            if rule.reason.trim().is_empty() {
                findings.push(finding(
                    "hygiene.missing-trigger-rationale",
                    "advisory",
                    &format!("activation rule {} has no trigger rationale", rule.id),
                ));
            }
        }
    }
    if activation_note(trait_ref).contains("no activation") {
        findings.push(finding(
            "hygiene.no-activation-path",
            "advisory",
            "no activation rule or manual invocation path is declared",
        ));
    }
    if trait_ref.summary.as_str().chars().count() > 240 {
        findings.push(finding(
            "hygiene.oversized-summary",
            "advisory",
            "summary is long for always-visible inventory output",
        ));
    }
    TraitHygieneEntry {
        trait_id: trait_ref.id.as_str().to_string(),
        action: recommended_action(&findings).to_string(),
        findings,
    }
}

fn finding(code: &str, severity: &str, message: &str) -> HygieneFinding {
    HygieneFinding {
        code: code.to_string(),
        severity: severity.to_string(),
        message: message.to_string(),
    }
}

fn recommended_action(findings: &[HygieneFinding]) -> &'static str {
    if findings.iter().any(|f| f.code == "hygiene.trust-blocked") {
        "plan-prune"
    } else if findings.iter().any(|f| f.severity == "warning") {
        "review"
    } else if findings.is_empty() {
        "keep"
    } else {
        "fix"
    }
}

fn duplicate_summaries(traits: &[Trait]) -> BTreeSet<String> {
    let mut by_summary: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for trait_ref in traits {
        by_summary
            .entry(trait_ref.summary.as_str().trim().to_ascii_lowercase())
            .or_default()
            .push(trait_ref.id.as_str().to_string());
    }
    by_summary
        .into_values()
        .filter(|ids| ids.len() > 1)
        .flatten()
        .collect()
}

fn add_catalog_hygiene_findings(traits: &[Trait], entries: &mut [TraitHygieneEntry]) {
    let mut activation_facets: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut behavior_axes: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();

    for trait_ref in traits {
        for (facet, value) in activation_facets_for(trait_ref) {
            activation_facets
                .entry((facet, value))
                .or_default()
                .push(trait_ref.id.as_str().to_string());
        }
        for (axis, value) in behavior_axes_for(trait_ref) {
            behavior_axes
                .entry(axis)
                .or_default()
                .entry(value)
                .or_default()
                .push(trait_ref.id.as_str().to_string());
        }
    }

    for ((facet, value), ids) in activation_facets {
        if ids.len() > 1 {
            for id in &ids {
                add_finding(
                    entries,
                    id,
                    finding(
                        "hygiene.overlapping-activation",
                        "advisory",
                        &format!(
                            "shares activation {facet}={value:?} with {}",
                            ids.iter()
                                .filter(|other| *other != id)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ),
                );
            }
        }
    }

    for (axis, values) in behavior_axes {
        if values.len() > 1 {
            let summary = values
                .iter()
                .map(|(value, ids)| format!("{value}:{}", ids.join("+")))
                .collect::<Vec<_>>()
                .join(", ");
            for ids in values.values() {
                for id in ids {
                    add_finding(
                        entries,
                        id,
                        finding(
                            "hygiene.conflicting-behavior-axis",
                            "advisory",
                            &format!("loaded traits set different {axis} values ({summary})"),
                        ),
                    );
                }
            }
        }
    }
}

fn add_finding(entries: &mut [TraitHygieneEntry], trait_id: &str, finding: HygieneFinding) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.trait_id == trait_id) {
        entry.findings.push(finding);
    }
}

fn activation_facets_for(trait_ref: &Trait) -> Vec<(String, String)> {
    let mut facets = Vec::new();
    if let Some(activation) = &trait_ref.activation {
        for rule in &activation.rules {
            push_activation_values(&mut facets, "mode", rule.mode.iter());
            push_activation_values(&mut facets, "language", rule.language.iter());
            push_activation_values(&mut facets, "file-glob", rule.file_glob.iter());
            push_activation_values(&mut facets, "task-keyword", rule.task_keyword.iter());
        }
    }
    facets
}

fn push_activation_values<'a>(
    facets: &mut Vec<(String, String)>,
    facet: &str,
    values: impl Iterator<Item = &'a String>,
) {
    for value in values {
        if !value.trim().is_empty() {
            facets.push((facet.to_string(), value.to_string()));
        }
    }
}

fn behavior_axes_for(trait_ref: &Trait) -> Vec<(String, String)> {
    let Some(behavior) = &trait_ref.behavior else {
        return Vec::new();
    };
    let mut axes = Vec::new();
    push_behavior_axis(&mut axes, "verbosity", &behavior.verbosity);
    push_behavior_axis(&mut axes, "directness", &behavior.directness);
    push_behavior_axis(&mut axes, "scope-control", &behavior.scope_control);
    push_behavior_axis(&mut axes, "initiative", &behavior.initiative);
    push_behavior_axis(&mut axes, "uncertainty", &behavior.uncertainty);
    axes
}

fn push_behavior_axis(
    axes: &mut Vec<(String, String)>,
    axis: &str,
    value: &Option<crate::r#trait::GuidanceItem>,
) {
    if let Some(value) = value {
        axes.push((axis.to_string(), value.as_str().to_string()));
    }
}

fn inventory_entry(trait_ref: &Trait, status: &PackageStatus) -> TraitInventoryEntry {
    TraitInventoryEntry {
        trait_id: trait_ref.id.as_str().to_string(),
        why_this_exists: trait_ref.summary.as_str().to_string(),
        when_it_should_trigger: activation_note(trait_ref),
        conflicts_with: trait_ref
            .relations
            .as_ref()
            .map(|relations| {
                relations
                    .conflicts
                    .iter()
                    .map(|entry| entry.reason.clone())
                    .collect()
            })
            .unwrap_or_default(),
        last_reviewed: match status {
            PackageStatus::Ready => "ready-status-without-date".to_string(),
            PackageStatus::Draft => "not-reviewed".to_string(),
        },
        render_targets: compatibility_matrix()
            .profiles
            .into_iter()
            .map(|profile| profile.profile)
            .collect(),
    }
}

fn activation_note(trait_ref: &Trait) -> String {
    let Some(activation) = trait_ref.activation.as_ref() else {
        return "no activation rules declared".to_string();
    };
    let mut parts = Vec::new();
    if activation.manual {
        parts.push("manual invocation".to_string());
    }
    for rule in &activation.rules {
        if rule.reason.trim().is_empty() {
            parts.push(format!("rule {} has no trigger rationale", rule.id));
        } else {
            parts.push(format!("{}: {}", rule.id, rule.reason));
        }
    }
    if parts.is_empty() {
        "activation section has no manual flag or rules".to_string()
    } else {
        parts.join("; ")
    }
}
