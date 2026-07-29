// Plans Agent Skills-compatible imports.
// Agent skill import planning.

/// Conventional resource id used for a checklist derived from the entry
/// document's own guidance text (as opposed to a linked resource file, which
/// keeps its own path-derived id). Stable across re-imports so `prior_checklists`
/// lookups keep finding the same declaration.
pub const ENTRY_CHECKLIST_RESOURCE_ID: &str = "checklist";

pub fn plan_agent_skills_import(
    request: AgentSkillsImportRequest,
) -> crate::Result<AgentSkillsImportPlan> {
    if request.source_profile != ImportProfile::AgentSkills {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "source-profile".to_string(),
            message: format!(
                "P59.1 import conversion supports agent-skills only, got {:?}",
                request.source_profile
            ),
        }
        .into());
    }

    let parsed = parse_agent_skill_markdown(&request.skill_markdown);
    let h1_name = first_h1(&parsed.body);
    let frontmatter_name = parsed.frontmatter.get("name").cloned();
    let display_name = frontmatter_name
        .clone()
        .or(h1_name.clone())
        .unwrap_or_else(|| humanize_source_name(&request.source_name));
    let (summary, summary_method) = if let Some(value) = parsed
        .frontmatter
        .get("description")
        .or_else(|| parsed.frontmatter.get("summary"))
        .map(|value| compact_text(value))
        .filter(|value| !value.is_empty())
    {
        (value, InferenceMethod::Direct)
    } else if let Some(value) = first_paragraph(&parsed.body) {
        (value, InferenceMethod::Body)
    } else {
        (
            "Imported Agent Skills guidance pending review.".to_string(),
            InferenceMethod::FileName,
        )
    };

    let frontmatter_id = parsed.frontmatter.get("id").cloned();
    let valid_frontmatter_id = frontmatter_id
        .as_deref()
        .filter(|id| crate::shared::validate_slug_shape(id, "frontmatter.id").is_ok());
    let invalid_frontmatter_id = if valid_frontmatter_id.is_none() {
        frontmatter_id.clone()
    } else {
        None
    };
    let id_candidate = valid_frontmatter_id
        .map(str::to_string)
        .or_else(|| frontmatter_name.clone())
        .or_else(|| h1_name.clone())
        .unwrap_or_else(|| request.source_name.clone());
    let id_source = if valid_frontmatter_id.is_some() {
        "frontmatter.id"
    } else if frontmatter_name.is_some() {
        "frontmatter.name"
    } else if h1_name.is_some() {
        "body.h1"
    } else {
        "source.name"
    };
    let trait_id = slugify_candidate(&id_candidate, id_source)?;

    // Checklist structures are detected from the raw parsed body so a
    // fenced-code task marker stays inside intact fence delimiters; item
    // text/detail sanitization and id reconciliation both then run inside
    // `derive_checklist_resource` over that same canonical (post-sanitize)
    // text, so identity can never depend on which raw spelling produced it.
    let prior_checklist_items = request
        .prior_checklists
        .get(ENTRY_CHECKLIST_RESOURCE_ID)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let checklist_extraction = derive_checklist_resource(
        ENTRY_CHECKLIST_RESOURCE_ID,
        &parsed.body,
        prior_checklist_items,
    );
    let checklist_sanitize_warnings = checklist_extraction
        .as_ref()
        .map(|extraction| extraction.warnings.clone())
        .unwrap_or_default();

    let body_for_guidance = checklist_extraction
        .as_ref()
        .map(|extraction| extraction.residual_text.as_str())
        .unwrap_or(parsed.body.as_str());
    let (guidance_text, mut sanitized_warnings) = sanitize_prompt_guidance(body_for_guidance);
    sanitized_warnings.extend(checklist_sanitize_warnings);
    let mut seen_warnings = BTreeSet::new();
    sanitized_warnings.retain(|warning| seen_warnings.insert(warning.clone()));
    let guidance_text = match &checklist_extraction {
        Some(extraction) if extraction.residual_text.is_empty() => {
            "Imported Agent Skills guidance was entirely a checklist; see the checklist resource.".to_string()
        }
        _ => guidance_text,
    };

    let mut report = create_import_report(&ImportRequest {
        source: request.source.clone(),
        source_profile: request.source_profile.clone(),
        raw_source_digest: request.raw_source_digest.clone(),
    });
    report.hidden_content_findings = crate::audit::scan_hidden_content(
        &request.skill_markdown,
        &trait_id,
        Some(request.source_path.as_str()),
    );
    report.inferred_fields = inferred_fields(
        &trait_id,
        id_source,
        &display_name,
        h1_name.as_deref(),
        &summary,
        summary_method,
        &guidance_text,
    );
    report.unsupported_fields = parsed.unsupported_fields;
    if let Some(invalid_id) = invalid_frontmatter_id {
        report.unsupported_fields.push(UnsupportedField {
            source_field: "frontmatter.id".to_string(),
            value: invalid_id,
            reason: "frontmatter id is not a valid canonical trait id; deterministic fallback id was used".to_string(),
        });
    }
    if let Some(version) = parsed.frontmatter.get("version")
        && crate::r#trait::SemanticVersion::new(version.clone()).is_err() {
            report.unsupported_fields.push(UnsupportedField {
                source_field: "frontmatter.version".to_string(),
                value: version.clone(),
                reason: "frontmatter version does not satisfy canonical version validation; deterministic import default was used".to_string(),
            });
        }
    report.frontmatter = parsed.frontmatter_evidence;
    report.conversion_warnings = sanitized_warnings;
    if !report.conversion_warnings.is_empty() {
        report.warnings.push(ImportWarning::SanitizedGuidance);
        report.unsupported_fields.push(UnsupportedField {
            source_field: "body.prompt-safety".to_string(),
            value: "sanitized imported guidance text".to_string(),
            reason: "prompt text grammar cannot safely carry backticks, shell/template substitutions, or arbitrary braces; raw source is preserved under imported/".to_string(),
        });
    }
    if !report.hidden_content_findings.is_empty() {
        report.review_actions.push(ReviewAction {
            action: ReviewActionKind::CheckHiddenContent,
            target: request.source_path.clone(),
            detail: format!(
                "review {} hidden-content finding(s) from raw imported source",
                report.hidden_content_findings.len()
            ),
        });
    }
    if let Some(extraction) = &checklist_extraction {
        report.inferred_fields.push(InferredField {
            field_path: format!("resource[{ENTRY_CHECKLIST_RESOURCE_ID}].item"),
            value: format!(
                "{} checklist item(s) derived from imported guidance",
                extraction.resource.items.len()
            ),
            method: InferenceMethod::Body,
        });
    }
    for inferred in &report.inferred_fields {
        report.review_actions.push(ReviewAction {
            action: ReviewActionKind::VerifyInferred,
            target: inferred.field_path.clone(),
            detail: format!("verify imported value inferred by {:?}", inferred.method),
        });
    }
    for unsupported in &report.unsupported_fields {
        report.review_actions.push(ReviewAction {
            action: ReviewActionKind::DecideUnsupported,
            target: unsupported.source_field.clone(),
            detail: unsupported.reason.clone(),
        });
    }
    report.review_actions.sort_by(|a, b| {
        format!("{:?}:{}:{}", a.action, a.target, a.detail)
            .cmp(&format!("{:?}:{}:{}", b.action, b.target, b.detail))
    });
    report.warnings.sort();
    report.warnings.dedup();

    let version = parsed
        .frontmatter
        .get("version")
        .filter(|version| crate::r#trait::SemanticVersion::new((*version).clone()).is_ok())
        .cloned()
        .unwrap_or_else(|| "0.1.0".to_string());
    let mut tags = vec!["imported".to_string(), "agent-skills".to_string()];
    tags.extend(parsed.frontmatter_tags.clone());
    tags.sort();
    tags.dedup();

    let mut draft_json = serde_json::json!({
        "schema-version": "0.2",
        "id": trait_id,
        "version": version,
        "name": display_name,
        "summary": summary,
        "metadata": {
            "tag": tags
        },
        "prompt": {
            "imported-guidance": {
                "description": "Imported Agent Skills guidance. Raw source is preserved under imported/ and must be reviewed before activation.",
                "text": guidance_text
            }
        }
    });
    if let Some(extraction) = &checklist_extraction {
        let resource_json = serde_json::to_value(&extraction.resource).map_err(|e| {
            crate::manifest::Error::InvalidField {
                field_path: format!("resource[{ENTRY_CHECKLIST_RESOURCE_ID}]"),
                message: format!("failed to serialize derived checklist resource: {e}"),
            }
        })?;
        if let Some(object) = draft_json.as_object_mut() {
            object.insert(
                "resource".to_string(),
                serde_json::Value::Array(vec![resource_json]),
            );
        }
    }

    Ok(AgentSkillsImportPlan {
        trait_id,
        trait_name: display_name,
        summary,
        draft_json,
        report,
    })
}

struct ParsedAgentSkill {
    frontmatter: BTreeMap<String, String>,
    frontmatter_tags: Vec<String>,
    frontmatter_evidence: Option<FrontmatterEvidence>,
    body: String,
    unsupported_fields: Vec<UnsupportedField>,
}

fn parse_agent_skill_markdown(markdown: &str) -> ParsedAgentSkill {
    let (frontmatter_text, body) = split_frontmatter(markdown);
    let mut unsupported_fields = Vec::new();
    let mut frontmatter = BTreeMap::new();
    let mut frontmatter_tags = Vec::new();
    let mut frontmatter_evidence = frontmatter_text.as_ref().map(|text| FrontmatterEvidence {
        raw_digest: Some(Digest::source(text)),
        mapped_keys: Vec::new(),
        unsupported_keys: Vec::new(),
        trusted_policy: false,
    });

    if let Some(frontmatter_text) = frontmatter_text {
        match serde_yaml::from_str::<serde_yaml::Value>(&frontmatter_text) {
            Ok(serde_yaml::Value::Mapping(mapping)) => {
                for (key, value) in mapping {
                    let key_text = yaml_key(&key);
                    let value_text = yaml_value_text(&value);
                    match key_text.as_str() {
                        "id" | "name" | "description" | "summary" | "version" => {
                            if let Some(evidence) = &mut frontmatter_evidence {
                                evidence.mapped_keys.push(FrontmatterMappedKey {
                                    source_key: key_text.clone(),
                                    target_field: frontmatter_target_field(&key_text).to_string(),
                                });
                            }
                            frontmatter.insert(key_text.clone(), value_text);
                        }
                        "tag" | "tags" => {
                            let values = yaml_string_values(&value);
                            for (index, value) in values.into_iter().enumerate() {
                                if crate::shared::validate_slug_shape(&value, &format!("frontmatter.{key_text}[{index}]")).is_ok() {
                                    frontmatter_tags.push(value);
                                } else {
                                    unsupported_fields.push(UnsupportedField {
                                        source_field: format!("frontmatter.{key_text}[{index}]"),
                                        value,
                                        reason: "frontmatter tag is not a valid canonical metadata tag".to_string(),
                                    });
                                }
                            }
                            if let Some(evidence) = &mut frontmatter_evidence {
                                evidence.mapped_keys.push(FrontmatterMappedKey {
                                    source_key: key_text.clone(),
                                    target_field: "metadata.tag".to_string(),
                                });
                            }
                        }
                        _ => unsupported_fields.push(UnsupportedField {
                            source_field: format!("frontmatter.{key_text}"),
                            value: value_text,
                            reason: "Agent Skills frontmatter field is preserved as unsupported import evidence".to_string(),
                        }),
                    }
                    if !matches!(
                        key_text.as_str(),
                        "id" | "name" | "description" | "summary" | "version" | "tag" | "tags"
                    )
                        && let Some(evidence) = &mut frontmatter_evidence {
                            evidence.unsupported_keys.push(key_text);
                        }
                }
            }
            Ok(other) => unsupported_fields.push(UnsupportedField {
                source_field: "frontmatter".to_string(),
                value: yaml_value_text(&other),
                reason: "Agent Skills frontmatter was not a mapping".to_string(),
            }),
            Err(e) => unsupported_fields.push(UnsupportedField {
                source_field: "frontmatter".to_string(),
                value: frontmatter_text,
                reason: format!("Agent Skills frontmatter could not be parsed as YAML: {e}"),
            }),
        }
    }

    unsupported_fields.sort_by(|a, b| a.source_field.cmp(&b.source_field));
    ParsedAgentSkill {
        frontmatter,
        frontmatter_tags,
        frontmatter_evidence,
        body,
        unsupported_fields,
    }
}

fn frontmatter_target_field(key: &str) -> &str {
    match key {
        "id" => "id",
        "name" => "name",
        "description" | "summary" => "summary",
        "version" => "version",
        _ => "unsupported",
    }
}

fn yaml_string_values(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(value) => vec![value.clone()],
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn split_frontmatter(markdown: &str) -> (Option<String>, String) {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, markdown.to_string());
    }

    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter && line.trim() == "---" {
            in_frontmatter = false;
            continue;
        }
        if in_frontmatter {
            frontmatter.push(line);
        } else {
            body.push(line);
        }
    }

    if in_frontmatter {
        return (None, markdown.to_string());
    }

    (Some(frontmatter.join("\n")), body.join("\n"))
}

fn first_h1(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let trimmed = line.trim();
        let title = trimmed.strip_prefix("# ")?.trim();
        if title.is_empty() {
            None
        } else {
            Some(compact_text(title))
        }
    })
}

fn first_paragraph(body: &str) -> Option<String> {
    let mut paragraph = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') && paragraph.is_empty() {
            continue;
        }
        paragraph.push(trimmed);
    }
    let text = compact_text(&paragraph.join(" "));
    if text.is_empty() { None } else { Some(text) }
}

fn compact_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn humanize_source_name(source_name: &str) -> String {
    let stem = source_name
        .strip_suffix(".md")
        .or_else(|| source_name.strip_suffix(".MD"))
        .unwrap_or(source_name);
    let text = stem
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>();
    let compact = compact_text(&text);
    if compact.is_empty() {
        "Imported Agent Skill".to_string()
    } else {
        compact
    }
}

fn slugify_candidate(candidate: &str, field_path: &str) -> crate::Result<String> {
    let slug = crate::synth::slugify_trait_id(candidate).map_err(|_| {
        crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "cannot derive a non-empty trait id from import source".to_string(),
        }
    })?;
    crate::shared::validate_slug_shape(&slug, "id")?;
    Ok(slug)
}

/// The prompt-safety replacements shared by guidance text and extracted
/// checklist item text: prompt grammar cannot safely carry backticks,
/// shell/template substitutions, or arbitrary braces, regardless of which
/// canonical field the text ends up in.
fn sanitize_prompt_text(text: &str) -> (String, Vec<String>) {
    let mut warnings = Vec::new();
    let mut sanitized = text.to_string();
    let replacements = [
        ("`", "'", "replaced backticks"),
        ("${", "dollar(", "replaced shell/template opening ${"),
        ("$(", "dollar(", "replaced shell substitution opening $("),
        ("{{", "(", "replaced double-brace template opening"),
        ("}}", ")", "replaced double-brace template closing"),
        ("{%", "(", "replaced template tag opening"),
        ("%}", ")", "replaced template tag closing"),
        ("{", "(", "replaced remaining opening braces"),
        ("}", ")", "replaced remaining closing braces"),
    ];
    for (from, to, warning) in replacements {
        if sanitized.contains(from) {
            sanitized = sanitized.replace(from, to);
            warnings.push(warning.to_string());
        }
    }
    (sanitized.trim().to_string(), warnings)
}

fn sanitize_prompt_guidance(body: &str) -> (String, Vec<String>) {
    let (sanitized, warnings) = sanitize_prompt_text(body);
    if sanitized.is_empty() {
        (
            "Imported Agent Skills guidance was empty; review preserved raw source under imported/.".to_string(),
            warnings,
        )
    } else {
        (sanitized, warnings)
    }
}

fn inferred_fields(
    trait_id: &str,
    id_source: &str,
    name: &str,
    h1_name: Option<&str>,
    summary: &str,
    summary_method: InferenceMethod,
    guidance_text: &str,
) -> Vec<InferredField> {
    let name_method = if name == h1_name.unwrap_or_default() {
        InferenceMethod::Heading
    } else {
        InferenceMethod::Direct
    };
    vec![
        InferredField {
            field_path: "id".to_string(),
            value: trait_id.to_string(),
            method: if id_source == "source.name" {
                InferenceMethod::FileName
            } else {
                InferenceMethod::Direct
            },
        },
        InferredField {
            field_path: "name".to_string(),
            value: name.to_string(),
            method: name_method,
        },
        InferredField {
            field_path: "summary".to_string(),
            value: summary.to_string(),
            method: summary_method,
        },
        InferredField {
            field_path: "package.status".to_string(),
            value: "draft".to_string(),
            method: InferenceMethod::Direct,
        },
        InferredField {
            field_path: "prompt.imported-guidance.text".to_string(),
            value: format!(
                "{} bytes of prompt-safe imported guidance",
                guidance_text.len()
            ),
            method: InferenceMethod::Body,
        },
    ]
}

fn yaml_key(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(text) => text.clone(),
        _ => yaml_value_text(value),
    }
}

fn yaml_value_text(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Bool(value) => value.to_string(),
        serde_yaml::Value::Number(value) => value.to_string(),
        serde_yaml::Value::String(value) => value.clone(),
        serde_yaml::Value::Sequence(_)
        | serde_yaml::Value::Mapping(_)
        | serde_yaml::Value::Tagged(_) => serde_yaml::to_string(value)
            .map(|text| text.trim().to_string())
            .unwrap_or_else(|_| format!("{value:?}")),
    }
}

fn import_source_target(source: &ImportSource) -> String {
    match source {
        ImportSource::Local { path } => path.clone(),
        ImportSource::Git { url } => url.clone(),
    }
}

// ---------------------------------------------------------------------------
// P91: Package-local trait.lock model
// ---------------------------------------------------------------------------

/// Schema version for package-local `trait.lock`.
pub const TRAIT_LOCK_SCHEMA_VERSION: &str = "0.1.0";
/// Import command/profile version recorded in snapshots.
pub const IMPORT_COMMAND_VERSION: &str = "ctx-traits-import-0.1.0";
