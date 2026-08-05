// Model-view compilation sections.
// Model-view section compilation.

fn format_activation(
    activation: &crate::r#trait::activation::Declaration,
    trait_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut lines = vec!["Static activation is advisory only; ctx resolver/runtime paths enforce activation when explicit request facts and capability evidence are supplied.".to_string()];
    // No lifecycle/trust gate line here: composed/rendered canonical output
    // carries no status or trust field (Group 95, 2026-07-19). Status lives
    // in the package manifest, trust in the machine-local trust store, and
    // gate evaluation requires both plus a resolved canonical digest — none
    // of which is available to a pure static render of one document. `ctx
    // traits check`/`activate`/`resolve` remain the authoritative gate
    // surfaces; static Markdown never bypasses them.
    lines.push(
        "Lifecycle/trust gate: not evaluated here; see `ctx traits check` for the authoritative \
         package-status/machine-trust gate. Static Markdown does not bypass ctx lifecycle/trust gates."
            .to_string(),
    );
    lines.push(format!("Manual activation required: {}", activation.manual));
    lines.push(format!(
        "Priority: {}",
        activation
            .priority
            .map_or("none".to_string(), |v| v.to_string())
    ));
    lines.push(format!(
        "Minimum score: {}",
        activation
            .min_score
            .map_or("default".to_string(), |v| v.to_string())
    ));
    if activation.rules.is_empty() {
        warnings.push("activation has no rules for static trigger guidance".to_string());
    }
    format_activation_rules(activation, warnings, normalizations, &mut lines);
    leaf_element(
        "activation",
        &[],
        &lines.join("\n"),
        "activation.body",
        trait_id,
        warnings,
        normalizations,
        findings,
    )
}

fn format_activation_rules(
    activation: &crate::r#trait::activation::Declaration,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    lines: &mut Vec<String>,
) {
    for (i, rule) in activation.rules.iter().enumerate() {
        let id = if rule.id.is_empty() {
            format!("rule-{i}")
        } else {
            sanitize_model_text(
                &rule.id,
                &format!("activation.rule[{i}].id"),
                warnings,
                normalizations,
            )
        };
        let reason = if rule.reason.is_empty() {
            warnings.push(format!(
                "activation.rule[{i}] has no reason for static render"
            ));
            "no reason supplied".to_string()
        } else {
            sanitize_model_text(
                &rule.reason,
                &format!("activation.rule[{i}].reason"),
                warnings,
                normalizations,
            )
        };
        lines.push(format!("- {id}: {reason}"));
        lines.push(format_sanitized_refs(
            "  Mode",
            &rule.mode.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].mode"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Language",
            &rule.language.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].language"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  File globs",
            &rule.file_glob.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].file-glob"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Task keywords",
            &rule.task_keyword.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].task-keyword"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Signals",
            &rule.signal.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].signal"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Explicit phrases",
            &rule.explicit_phrase.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].explicit-phrase"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Exclude file globs",
            &rule.exclude_file_glob.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].exclude-file-glob"),
            warnings,
            normalizations,
        ));
        lines.push(format_sanitized_refs(
            "  Exclude keywords",
            &rule.exclude_keyword.iter().cloned().collect::<Vec<_>>(),
            &format!("activation.rule[{i}].exclude-keyword"),
            warnings,
            normalizations,
        ));
    }
}

fn format_ports(
    trait_ref: &Trait,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    trait_ref
        .ports
        .iter()
        .map(|p| format_port_element(p, trait_ref.id.as_str(), warnings, normalizations, findings))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_port_element(
    p: &crate::r#trait::port::Port,
    trait_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let direction = match p.direction {
        crate::r#trait::port::PortDirection::Input => "input",
        crate::r#trait::port::PortDirection::Output => "output",
    };
    let description = sanitize_model_text(
        &p.description,
        &format!("port.{}.description", p.id),
        warnings,
        normalizations,
    );
    let schema = sanitize_model_text(
        &p.schema,
        &format!("port.{}.schema", p.id),
        warnings,
        normalizations,
    );
    let optional = if p.optional { "optional" } else { "required" };
    let value = p.value.as_deref().map_or("none".to_string(), |v| {
        sanitize_model_text(v, &format!("port.{}.value", p.id), warnings, normalizations)
    });
    let mut lines = vec![
        format!("Optionality: {optional}"),
        format!("Schema: {schema}"),
        format!("Backing slot: {value}"),
        format!("Description: {description}"),
    ];
    if matches!(p.direction, crate::r#trait::port::PortDirection::Output) {
        lines.extend(format_output_contract_lines(p, warnings, normalizations));
    }
    leaf_element(
        "port",
        &[("id", p.id.as_str()), ("direction", direction)],
        &lines.join("\n"),
        &format!("port.{}.body", p.id),
        trait_id,
        warnings,
        normalizations,
        findings,
    )
}

fn format_output_contract_lines(
    p: &crate::r#trait::port::Port,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> Vec<String> {
    if p.title.as_deref().is_none_or(str::is_empty) {
        warnings.push(format!("port.{} output contract has no title", p.id));
    }
    if p.format.is_empty() {
        warnings.push(format!(
            "port.{} output contract has no format guidance",
            p.id
        ));
    }
    let title = match p.title.as_deref() {
        Some(t) if !t.trim().is_empty() => {
            sanitize_model_text(t, &format!("port.{}.title", p.id), warnings, normalizations)
        }
        _ => format!("Output port {}", p.id),
    };
    let raw_format: Vec<String> = p.format.iter().map(|s| s.to_string()).collect();
    let format_tags = sanitize_model_values(
        &raw_format,
        &format!("port.{}.format", p.id),
        warnings,
        normalizations,
    )
    .join(", ");
    vec![
        format!("Output contract title: {title}"),
        format!(
            "Output contract format: {}",
            if format_tags.is_empty() {
                "none"
            } else {
                &format_tags
            }
        ),
    ]
}

fn procedure_has_signal_emits(trait_ref: &Trait) -> bool {
    trait_ref
        .procedure
        .as_ref()
        .is_some_and(|proc| proc.sequence.iter().any(|item| !item.on_complete.is_empty()))
}

fn format_agents(
    trait_ref: &Trait,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    trait_ref
        .agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            format_agent_element(trait_ref, index, agent, warnings, normalizations, findings)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_arguments)]
fn format_agent_element(
    trait_ref: &Trait,
    index: usize,
    agent: &crate::r#trait::agent::Agent,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut lines = vec![
        "Static render advisory: agent roles describe intended frame routing for the ctx controlled runtime; static Markdown cannot enforce multi-agent routing, harness selection, or caller identity."
            .to_string(),
    ];
    let description = sanitize_model_text(
        &agent.description,
        &format!("agent[{index}].description"),
        warnings,
        normalizations,
    );
    lines.push(format!("Description: {description}"));
    if let Some(summary) = agent.summary.as_deref() {
        lines.push(format!(
            "Summary: {}",
            sanitize_model_text(
                summary,
                &format!("agent[{index}].summary"),
                warnings,
                normalizations,
            )
        ));
    }
    if let Some(system) = agent.system.as_deref() {
        // Standing instructions reach the model through the harness system
        // channel at run time, so they must appear here to stay reviewable
        // and audited — an instruction the model sees but the model view
        // omits is exactly the hidden channel audit exists to catch.
        lines.push(format!(
            "System: {}",
            sanitize_model_text(
                system,
                &format!("agent[{index}].system"),
                warnings,
                normalizations,
            )
        ));
    }
    let assigned = assigned_items_for_agent(trait_ref, &agent.id, warnings, normalizations);
    lines.push(format!(
        "Assigned sequence items: {}",
        if assigned.is_empty() {
            "none declared".to_string()
        } else {
            assigned.join(", ")
        }
    ));
    leaf_element(
        "agent",
        &[("id", agent.id.as_str())],
        &lines.join("\n"),
        &format!("agent[{index}].body"),
        trait_ref.id.as_str(),
        warnings,
        normalizations,
        findings,
    )
}

fn assigned_items_for_agent(
    trait_ref: &Trait,
    agent_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> Vec<String> {
    let agent_ref = format!("agent:{agent_id}");
    let mut assigned = Vec::new();
    if let Some(proc) = trait_ref.procedure.as_ref() {
        for (i, item) in proc.sequence.iter().enumerate() {
            if item.agent.as_deref() == Some(agent_ref.as_str()) {
                let item_id = item.id.as_deref().unwrap_or("-");
                let title = sanitize_model_text(
                    &item.title,
                    &format!("procedure.sequence[{i}].title"),
                    warnings,
                    normalizations,
                );
                assigned.push(format!("procedure.sequence[{i}] {item_id} ({title})"));
            }
        }
    }
    for (sequence_id, sequence) in trait_ref.sequences.iter() {
        for (index, item) in sequence.sequence.iter().enumerate() {
            if item.agent.as_deref() == Some(agent_ref.as_str()) {
                let item_id = item.id.as_deref().unwrap_or("-");
                let title = sanitize_model_text(
                    &item.title,
                    &format!("sequence.{sequence_id}.sequence[{index}].title"),
                    warnings,
                    normalizations,
                );
                assigned.push(format!(
                    "sequence.{sequence_id}.sequence[{index}] {item_id} ({title})"
                ));
            }
        }
    }
    assigned
}

fn format_signals(
    trait_ref: &Trait,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut blocks: Vec<String> = trait_ref
        .signals
        .iter()
        .map(|s| {
            let description = sanitize_model_text(
                &s.description,
                &format!("signal.{}.description", s.id),
                warnings,
                normalizations,
            );
            let emitted_by = trait_ref.procedure.as_ref().map(|proc| {
                proc.sequence.iter().enumerate().filter_map(|(i, item)| {
                    if item.on_complete.iter().any(|emit| emit.signal_ref() == format!("signal:{}", s.id)) {
                        let item_id = item.id.as_deref().unwrap_or("-");
                        let title = sanitize_model_text(
                            &item.title,
                            &format!("procedure.sequence[{i}].title"),
                            warnings,
                            normalizations,
                        );
                        Some(format!("sequence[{i}] {item_id} ({title})"))
                    } else { None }
                }).collect::<Vec<_>>()
            }).unwrap_or_default();
            let body = format!("Meaning: {}\nStatic note: allowed runtime/trace fact, not an arbitrary text match\nEmitted by: {}", description, if emitted_by.is_empty() { "none declared".to_string() } else { emitted_by.join(", ") });
            leaf_element(
                "signal",
                &[("id", s.id.as_str())],
                &body,
                &format!("signal.{}.body", s.id),
                trait_ref.id.as_str(),
                warnings,
                normalizations,
                findings,
            )
        })
        .collect();

    // Add dependency-pending signal evidence for qualified emitted signals
    // that are not local [[signal]] declarations.
    let _local_signal_ids: std::collections::BTreeSet<&str> =
        trait_ref.signals.iter().map(|s| s.id.as_str()).collect();
    if let Some(proc) = &trait_ref.procedure {
        let mut dep_pending: Vec<String> = Vec::new();
        for (i, item) in proc.sequence.iter().enumerate() {
            for (j, emit) in item.on_complete.iter().enumerate() {
                let ref_text = emit.signal_ref();
                if !ref_text.starts_with("signal:") {
                    continue;
                }
                let parsed_id = ref_text.strip_prefix("signal:").unwrap_or(ref_text);
                if parsed_id.contains('/') {
                    // `safe_ref`/`title` are author-controlled (a
                    // dependency-qualified ref path has no character
                    // restriction and a sequence title is free text), so
                    // this must reach `leaf_element` like every other
                    // author-derived body rather than joining straight into
                    // `blocks` (which becomes `Section.content` unescaped).
                    // `parsed_id` is deliberately NOT used as an attribute:
                    // unlike a local `signal.id` (a validated `Slug`), a
                    // dependency-qualified ref path is unvalidated free text
                    // and attribute values are never escaped, so it stays in
                    // the body instead.
                    let safe_ref = sanitize_model_text(
                        ref_text,
                        &format!("procedure.sequence[{i}].on-complete[{j}]"),
                        warnings,
                        normalizations,
                    );
                    let item_id = item.id.as_deref().unwrap_or("-");
                    let title = sanitize_model_text(
                        &item.title,
                        &format!("procedure.sequence[{i}].title"),
                        warnings,
                        normalizations,
                    );
                    let body = format!(
                        "Dependency-pending: {safe_ref}\nEmitted by: sequence[{i}] {item_id} ({title})\nNote: dependency signal semantics are not resolved from this local trait"
                    );
                    dep_pending.push(leaf_element(
                        "signal",
                        &[],
                        &body,
                        &format!("procedure.sequence[{i}].on-complete[{j}].dependency-pending"),
                        trait_ref.id.as_str(),
                        warnings,
                        normalizations,
                        findings,
                    ));
                }
            }
        }
        if !dep_pending.is_empty() {
            blocks.push(format!(
                "Dependency-pending emitted signals:\n{}",
                dep_pending.join("\n")
            ));
        }
    }

    if blocks.is_empty() {
        "(no signal declarations or emitted signals)".to_string()
    } else {
        blocks.join("\n")
    }
}

fn format_relations(
    relations: &crate::r#trait::relations::Model,
    trait_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut lines = vec!["Static relations are advisory compatibility/composition guidance; rendered Markdown does not resolve dependencies, activate targets, or enforce conflicts.".to_string()];
    for (i, entry) in relations.requires.iter().enumerate() {
        let target = sanitize_model_text(
            &entry.target,
            &format!("relations.requires[{i}].target"),
            warnings,
            normalizations,
        );
        lines.push(format!(
            "- requires {target}: {}",
            relation_reason(
                &entry.reason,
                warnings,
                normalizations,
                &format!("relations.requires[{i}].reason")
            )
        ));
        lines.push(format!(
            "  When: {}",
            relation_when_sanitized(
                entry.when.as_slice(),
                &format!("relations.requires[{i}].when"),
                warnings,
                normalizations
            )
        ));
        if target.starts_with("port:") {
            lines.push("  Binding caveat: port binding proposals require ctx resolver/composition evidence; static Markdown cannot bind ports.".to_string());
        }
    }
    for (i, entry) in relations.suggests.iter().enumerate() {
        let target = sanitize_model_text(
            &entry.target,
            &format!("relations.suggests[{i}].target"),
            warnings,
            normalizations,
        );
        lines.push(format!(
            "- suggests {target}: {}",
            relation_reason(
                &entry.reason,
                warnings,
                normalizations,
                &format!("relations.suggests[{i}].reason")
            )
        ));
        lines.push(format!(
            "  When: {}",
            relation_when_sanitized(
                entry.when.as_slice(),
                &format!("relations.suggests[{i}].when"),
                warnings,
                normalizations
            )
        ));
        if target.starts_with("port:") {
            lines.push("  Binding caveat: port binding proposals require ctx resolver/composition evidence; static Markdown cannot bind ports.".to_string());
        }
    }
    for (i, entry) in relations.conflicts.iter().enumerate() {
        lines.push(format!(
            "- conflicts targetless: {}",
            relation_reason(
                &entry.reason,
                warnings,
                normalizations,
                &format!("relations.conflicts[{i}].reason")
            )
        ));
        lines.push(format!(
            "  When: {}",
            relation_when_sanitized(
                entry.when.as_slice(),
                &format!("relations.conflicts[{i}].when"),
                warnings,
                normalizations
            )
        ));
    }
    leaf_element(
        "relations",
        &[],
        &lines.join("\n"),
        "relations.body",
        trait_id,
        warnings,
        normalizations,
        findings,
    )
}

fn relation_reason(
    reason: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    field: &str,
) -> String {
    if reason.trim().is_empty() {
        warnings.push(format!("{field} missing relation reason for static render"));
        "no reason supplied".to_string()
    } else {
        sanitize_model_text(reason, field, warnings, normalizations)
    }
}

fn relation_when_sanitized<T: AsRef<str>>(
    when: &[T],
    field_prefix: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    if when.is_empty() {
        return "always / no when predicates".to_string();
    }
    let sanitized: Vec<String> = when
        .iter()
        .enumerate()
        .map(|(i, value)| {
            sanitize_model_text(
                value.as_ref(),
                &format!("{field_prefix}[{i}]"),
                warnings,
                normalizations,
            )
        })
        .collect();
    sanitized.join(", ")
}

fn format_sanitized_on_complete(
    on_complete: &[String],
    seq_index: usize,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    if on_complete.is_empty() {
        return "  On-complete: none".to_string();
    }
    let parts: Vec<String> = on_complete
        .iter()
        .enumerate()
        .map(|(j, raw)| {
            let safe = sanitize_model_text(
                raw,
                &format!("procedure.sequence[{seq_index}].on-complete[{j}]"),
                warnings,
                normalizations,
            );
            if raw.contains('/') && raw.starts_with("signal:") {
                format!("{safe} (dependency-pending)")
            } else {
                safe
            }
        })
        .collect();
    format!("  Emits: {}", parts.join(", "))
}

fn format_scenarios(
    trait_ref: &Trait,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    trait_ref
        .scenarios
        .iter()
        .map(|s| {
            let input = s
                .input
                .as_deref()
                .map_or("(no input)".to_string(), |value| {
                    sanitize_model_text(
                        value,
                        &format!("scenario.{}.input", s.id),
                        warnings,
                        normalizations,
                    )
                });
            let output = s
                .output
                .as_deref()
                .map_or("(no output)".to_string(), |value| {
                    sanitize_model_text(
                        value,
                        &format!("scenario.{}.output", s.id),
                        warnings,
                        normalizations,
                    )
                });
            leaf_element(
                "scenario",
                &[("id", s.id.as_str()), ("variant", s.variant.as_str())],
                &format!("Input: {input}\nOutput: {output}"),
                &format!("scenario.{}.body", s.id),
                trait_ref.id.as_str(),
                warnings,
                normalizations,
                findings,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_prompts(
    trait_ref: &Trait,
    resource_plan: &Plan,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    trait_ref
        .prompts
        .iter()
        .map(|(id, prompt)| {
            format_prompt_element(
                id,
                prompt,
                resource_plan,
                trait_ref.id.as_str(),
                warnings,
                normalizations,
                findings,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_arguments)]
fn format_prompt_element(
    id: &str,
    prompt: &crate::r#trait::prompt::Prompt,
    resource_plan: &Plan,
    trait_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut lines = Vec::new();
    if let Some(ref description) = prompt.description {
        lines.push(format!(
            "Description: {}",
            sanitize_model_text(
                description,
                &format!("prompt.{id}.description"),
                warnings,
                normalizations,
            )
        ));
    }
    lines.push(format_sanitized_refs(
        "Input",
        prompt.input.as_slice(),
        &format!("prompt.{id}.input"),
        warnings,
        normalizations,
    ));
    lines.push(format_sanitized_refs(
        "Output",
        prompt.output.as_slice(),
        &format!("prompt.{id}.output"),
        warnings,
        normalizations,
    ));
    lines.push(format!(
        "Body: {}",
        match (&prompt.text, &prompt.source) {
            (Some(text), None) => sanitize_model_text(
                text,
                &format!("prompt.{id}.text"),
                warnings,
                normalizations
            ),
            (None, Some(source)) => format_resource_backed_prompt_body(
                id,
                source,
                resource_plan,
                warnings,
                normalizations,
            ),
            _ => "invalid prompt body shape".to_string(),
        }
    ));
    leaf_element(
        "prompt",
        &[("id", id)],
        &lines.join("\n"),
        &format!("prompt.{id}.body"),
        trait_id,
        warnings,
        normalizations,
        findings,
    )
}

fn format_resource_backed_prompt_body(
    prompt_id: &str,
    source: &str,
    resource_plan: &Plan,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let Ok(parsed) = crate::reference::Reference::parse(source) else {
        let safe = sanitize_model_text(
            source,
            &format!("prompt.{prompt_id}.source"),
            warnings,
            normalizations,
        );
        return format!("resource-backed prompt source {safe} (invalid source ref)");
    };
    let lookup_id = if parsed.is_qualified() {
        parsed.path()
    } else {
        parsed.id()
    };
    let Some(entry) = resource_plan
        .entries
        .iter()
        .find(|entry| entry.resource_id == lookup_id)
    else {
        let safe = sanitize_model_text(
            source,
            &format!("prompt.{prompt_id}.source"),
            warnings,
            normalizations,
        );
        return if parsed.is_qualified() {
            format!(
                "resource-backed prompt source {safe} (dependency-pending; body not materialized)"
            )
        } else {
            format!("resource-backed prompt source {safe} (no local resource evidence)")
        };
    };
    let body = entry.body_text.as_deref().map(|text| {
        sanitize_model_text(
            text,
            &format!("prompt.{prompt_id}.source.body"),
            warnings,
            normalizations,
        )
    });
    match body {
        Some(body) => format!(
            "Body source: {}\n  Resource digest: {}\n  Required template inputs: {}\n  Unresolved inputs: {}\n  Body:\n  {}",
            sanitize_model_text(
                source,
                &format!("prompt.{prompt_id}.source"),
                warnings,
                normalizations
            ),
            entry
                .body_digest
                .as_deref()
                .or_else(|| entry.digest_evidence.as_ref().map(|ev| ev.digest.as_str()))
                .unwrap_or("none"),
            comma_or_none_sanitized(
                &entry.template_inputs,
                &format!("prompt.{prompt_id}.source.required-input"),
                warnings,
                normalizations
            ),
            comma_or_none_sanitized(
                &entry.unresolved_inputs,
                &format!("prompt.{prompt_id}.source.unresolved-input"),
                warnings,
                normalizations
            ),
            body.lines().collect::<Vec<_>>().join("\n  "),
        ),
        None => format!(
            "Body source: {}\n  Resource digest: {}\n  Body: unavailable in supplied IO evidence; static render preserves source ref and typed inputs only\n  Required template inputs: {}",
            sanitize_model_text(
                source,
                &format!("prompt.{prompt_id}.source"),
                warnings,
                normalizations
            ),
            entry
                .digest_evidence
                .as_ref()
                .map(|ev| ev.digest.as_str())
                .unwrap_or("none"),
            comma_or_none_sanitized(
                &entry.template_inputs,
                &format!("prompt.{prompt_id}.source.required-input"),
                warnings,
                normalizations
            ),
        ),
    }
}

fn format_procedure(
    trait_ref: &Trait,
    proc: &crate::r#trait::procedure::Model,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    let mut lines = vec![format!(
        "Description: {}",
        sanitize_model_text(
            &proc.description,
            "procedure.description",
            warnings,
            normalizations,
        )
    )];
    if proc.worktree_required {
        lines.push("Worktree required: yes".to_string());
    }
    lines.push(format_sanitized_refs(
        "Input",
        proc.input.as_slice(),
        "procedure.input",
        warnings,
        normalizations,
    ));
    lines.push(format_sanitized_refs(
        "Output",
        proc.output.as_slice(),
        "procedure.output",
        warnings,
        normalizations,
    ));
    lines.push("Static host note: this render describes the procedure contract and runtime-only sequence-control declarations but cannot enforce sequence state, slot validation, command execution, loop exits, for-each iteration, or runtime completion outside the ctx controlled runtime.".to_string());

    for (i, item) in proc.sequence.iter().enumerate() {
        let id = item
            .id
            .as_deref()
            .map(|id| {
                sanitize_model_text(
                    id,
                    &format!("procedure.sequence[{i}].id"),
                    warnings,
                    normalizations,
                )
            })
            .unwrap_or_else(|| "-".to_string());
        let display_title = if item.title.trim().is_empty() {
            item.id.as_deref().unwrap_or("sequence item")
        } else {
            &item.title
        };
        let title = sanitize_model_text(
            display_title,
            &format!("procedure.sequence[{i}].title"),
            warnings,
            normalizations,
        );
        lines.push(format!("{}. [{id}] {title}", i + 1));
        if let Some(agent) = item.agent.as_deref() {
            lines.push(format!(
                "  Agent: {}",
                sanitize_model_text(
                    agent,
                    &format!("procedure.sequence[{i}].agent"),
                    warnings,
                    normalizations,
                )
            ));
        }
        match item.effective_kind() {
            crate::r#trait::procedure::SequenceKind::Prompt => lines.push(format!(
                "  Prompt: {}",
                sanitize_model_text(
                    &item.prompt,
                    &format!("procedure.sequence[{i}].prompt"),
                    warnings,
                    normalizations,
                )
            )),
            crate::r#trait::procedure::SequenceKind::Ask => lines.push(format!(
                "- ask {}",
                item.id.as_deref().unwrap_or("unnamed")
            )),
            crate::r#trait::procedure::SequenceKind::Command => {
                match crate::r#trait::procedure::command_plan_for_item(
                    item,
                    &format!("procedure.sequence[{i}]"),
                ) {
                    Ok(Some(command)) => {
                        lines.push(
                            "  Runtime command: command-backed step; static hosts cannot execute it."
                                .to_string(),
                        );
                        if let Some(argv_from) = command.argv_from.as_deref() {
                            lines.push(format!(
                                "  Command argv: resolved at runtime from {argv_from}"
                            ));
                        } else {
                            lines.push(format!("  Command argv: {}", command.argv.join(" ")));
                        }
                        if let Some(output) = item.output.iter().next() {
                            lines.push(format!("  Command output slot: {}", output.ref_text()));
                        }
                        lines.push(
                            "  Command permission: requires explicit runtime approval before execution."
                                .to_string(),
                        );
                    }
                    Ok(None) => warnings.push(format!(
                        "procedure.sequence[{i}] command item has no command plan"
                    )),
                    Err(err) => {
                        warnings.push(format!("procedure.sequence[{i}] command warning: {err}"));
                    }
                }
            }
            crate::r#trait::procedure::SequenceKind::Check => {
                match crate::r#trait::procedure::command_plan_for_item(
                    item,
                    &format!("procedure.sequence[{i}]"),
                ) {
                    Ok(Some(command)) => {
                        lines.push(
                            "  Runtime check: command-backed boolean verdict step; static hosts cannot execute it."
                                .to_string(),
                        );
                        if let Some(argv_from) = command.argv_from.as_deref() {
                            lines.push(format!(
                                "  Check argv: resolved at runtime from {argv_from}"
                            ));
                        } else {
                            lines.push(format!("  Check argv: {}", command.argv.join(" ")));
                        }
                        if let Some(output) = item.output.iter().next() {
                            lines.push(format!("  Check output slot: {}", output.ref_text()));
                        }
                        lines.push(
                            "  Check permission: requires explicit runtime approval before execution."
                                .to_string(),
                        );
                    }
                    Ok(None) => warnings.push(format!(
                        "procedure.sequence[{i}] check item has no command plan"
                    )),
                    Err(err) => {
                        warnings.push(format!("procedure.sequence[{i}] check warning: {err}"));
                    }
                }
            }
            crate::r#trait::procedure::SequenceKind::Project => {
                lines.push(format!(
                    "  Runtime projection: {} deterministic atomic write(s)",
                    item.projection.len()
                ));
            }
            crate::r#trait::procedure::SequenceKind::Sequence => {
                lines.push(format!(
                    "  Runtime nested sequence: {}",
                    item.sequence.as_deref().unwrap_or("sequence:<missing>")
                ));
            }
            crate::r#trait::procedure::SequenceKind::Branch => {
                lines.push(format!(
                    "  Runtime branch: guard={} then={} otherwise={}",
                    if item.when.is_some() {
                        "declared"
                    } else {
                        "missing"
                    },
                    item.sequence.as_deref().unwrap_or("sequence:<missing>"),
                    item.otherwise.as_deref().unwrap_or("none")
                ));
            }
            crate::r#trait::procedure::SequenceKind::Loop => {
                lines.push(format!(
                    "  Runtime loop: {} max-iterations={}",
                    item.sequence.as_deref().unwrap_or("sequence:<missing>"),
                    item.max_iterations
                        .map(|value| value.to_string())
                        .or_else(|| item.max_iterations_from.clone())
                        .unwrap_or_else(|| "unbounded".to_string())
                ));
                if item.until.is_some() {
                    lines.push("  Loop exit: typed until guard declared".to_string());
                }
                if item.abort_if.is_some() {
                    let refs = item.on_abort.as_ref().map(crate::r#trait::procedure::ExhaustionTarget::signals).unwrap_or_default();
                    if refs.is_empty() {
                        lines.push("  Loop stop: typed abort-if guard declared".to_string());
                    } else {
                        lines.push(format!(
                            "  Loop stop: typed abort-if guard declared, on-abort={}",
                            refs.join(",")
                        ));
                    }
                }
            }
            crate::r#trait::procedure::SequenceKind::ForEach => {
                lines.push(format!(
                    "  Runtime for-each: over={} item={} sequence={} max-items={}",
                    item.over.as_deref().unwrap_or("slot:<missing>"),
                    item.item.as_deref().unwrap_or("slot:<missing>"),
                    item.sequence.as_deref().unwrap_or("sequence:<missing>"),
                    item.max_items
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "missing".to_string())
                ));
                if item.concurrent {
                    lines.push(
                        "  Runtime for-each concurrency: requested (execution lands in P263)"
                            .to_string(),
                    );
                }
            }
            crate::r#trait::procedure::SequenceKind::Parallel => {
                lines.push(format!(
                    "  Runtime parallel: branches=[{}] max-branches={}",
                    item.branches.iter().cloned().collect::<Vec<_>>().join(", "),
                    item.max_branches
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "missing".to_string())
                ));
                let join_label = item.join.as_ref().map_or(
                    "collect-in-order",
                    crate::r#trait::procedure::JoinPolicy::label,
                );
                lines.push(format!("  Parallel join: {join_label}"));
                if !item.branch_failure.is_empty() {
                    let policies: Vec<String> = item
                        .branch_failure
                        .iter()
                        .map(|entry| {
                            format!(
                                "{}={}",
                                entry.branch,
                                match entry.on_failure {
                                    crate::r#trait::procedure::BranchFailurePolicy::Skip => "skip",
                                    crate::r#trait::procedure::BranchFailurePolicy::Park => "park",
                                    crate::r#trait::procedure::BranchFailurePolicy::PanelFail =>
                                        "panel-fail",
                                }
                            )
                        })
                        .collect();
                    lines.push(format!(
                        "  Parallel branch-failure: {}",
                        policies.join(", ")
                    ));
                }
            }
        }
        let input_refs: Vec<String> = item.input.ref_texts().map(str::to_string).collect();
        lines.push(format_sanitized_refs(
            "  Input",
            &input_refs,
            &format!("procedure.sequence[{i}].input"),
            warnings,
            normalizations,
        ));
        let output_refs: Vec<String> = item.output.ref_texts().map(str::to_string).collect();
        lines.push(format_sanitized_refs(
            "  Output",
            &output_refs,
            &format!("procedure.sequence[{i}].output"),
            warnings,
            normalizations,
        ));
        let format_tags = item
            .format
            .as_ref()
            .map(|tags| {
                let raw: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
                sanitize_model_values(
                    &raw,
                    &format!("procedure.sequence[{i}].format"),
                    warnings,
                    normalizations,
                )
                .join(", ")
            })
            .filter(|text| !text.is_empty())
            .map_or("none".to_string(), |text| text);
        lines.push(format!("  Format: {format_tags}"));
        let on_complete: Vec<String> = item
            .on_complete
            .iter()
            .map(|emit| emit.signal_ref().to_string())
            .collect();
        lines.push(format_sanitized_on_complete(&on_complete, i, warnings, normalizations));
    }

    if !trait_ref.sequences.is_empty() {
        lines.push("Named runtime sequences:".to_string());
        for (id, sequence) in trait_ref.sequences.iter() {
            lines.push(format!(
                "- sequence:{id} ({} item(s)): runtime-only reusable sequence block",
                sequence.sequence.len()
            ));
            for (index, item) in sequence.sequence.iter().enumerate() {
                if let Some(agent) = item.agent.as_deref() {
                    lines.push(format!(
                        "  - item[{index}] agent: {}",
                        sanitize_model_text(
                            agent,
                            &format!("sequence.{id}.sequence[{index}].agent"),
                            warnings,
                            normalizations,
                        )
                    ));
                }
            }
        }
    }

    if !trait_ref.conditions.is_empty() {
        lines.push("Typed runtime conditions:".to_string());
        for id in trait_ref.conditions.keys() {
            lines.push(format!(
                "- condition:{id}: typed guard evidence; raw model prose does not satisfy it"
            ));
        }
    }

    let output_ports = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, crate::r#trait::port::PortDirection::Output))
        .map(|port| format!("port:{}", port.id))
        .collect::<Vec<_>>();
    lines.push(format!(
        "Output ports: {}",
        if output_ports.is_empty() {
            "none".to_string()
        } else {
            output_ports.join(", ")
        }
    ));

    leaf_element(
        "procedure",
        &[],
        &lines.join("\n"),
        "procedure.body",
        trait_ref.id.as_str(),
        warnings,
        normalizations,
        findings,
    )
}
