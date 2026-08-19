//! Explain activation decisions and inspect decoded traits.

use ctx_traits_core::response::CommandOutput;

use crate::app::tui::{Line, Tone, command_line, emit_report, labeled_line, write_plain_line as w};

pub(crate) struct ExplainInputs<'a> {
    pub(crate) task: Option<&'a str>,
    pub(crate) scaffold: bool,
    pub(crate) trait_files: &'a [String],
    pub(crate) files: &'a [String],
    pub(crate) mode: Option<&'a str>,
    pub(crate) languages: &'a [String],
    pub(crate) signals: &'a [String],
    pub(crate) explicit_invocation: Option<&'a str>,
    pub(crate) active_only: bool,
    pub(crate) json: bool,
    pub(crate) trait_id: Option<&'a str>,
    pub(crate) source_map: Option<&'a str>,
    pub(crate) verbose: bool,
    pub(crate) llm_assisted: bool,
    pub(crate) candidate_path: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) budget_document: Option<&'a str>,
    pub(crate) assignments: &'a [String],
}

pub(crate) fn handle_explain(input: ExplainInputs<'_>) -> crate::Result<CommandOutput<()>> {
    if input.scaffold {
        if input.task.is_some() {
            return Err(crate::Error::Command {
                message: "explain accepts either --task or --scaffold, not both".to_string(),
            });
        }
        return handle_explain_scaffold(input);
    }
    let Some(task) = input.task else {
        return Err(crate::Error::Command {
            message: "explain requires --task for activation mode or --scaffold for scaffold mode"
                .to_string(),
        });
    };
    let mut traits = Vec::new();
    let mut lifecycle = Vec::new();
    for path_str in input.trait_files {
        let path = camino::Utf8Path::new(path_str);
        let encoding = ctx_traits_core::encoding::Encoding::from_path(path)?;
        let text = ctx_traits_io::read::read_text(path)?;
        let (decoded, decode_warnings) =
            ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &text)?;
        ctx_traits_io::decode_diagnostics::print_decode_warnings(path_str, &decode_warnings);
        lifecycle.push(inspect_lifecycle(path, &decoded)?);
        traits.push(decoded);
    }

    let request = ctx_traits_core::r#trait::activation::Request {
        task_text: task.to_string(),
        mode: input.mode.map(str::to_string),
        files: input.files.to_vec(),
        language_hints: input.languages.to_vec(),
        explicit_invocation: input.explicit_invocation.map(str::to_string),
        signals: input.signals.to_vec(),
        trait_id: input.trait_id.map(str::to_string),
        capability_reports: activation_capability_reports(),
    };
    let mut report = ctx_traits_core::r#trait::activation::explain(request, &traits, &lifecycle);
    if input.active_only {
        report.candidates.retain(|candidate| candidate.active);
    }

    use crate::app::presentation::{
        HumanOutputMode, OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
    };

    match OutputMode::select(input.json, input.verbose) {
        OutputMode::Json => {
            crate::app::command_handlers::print_json_report(&report, "activation explanation")?;
        }
        OutputMode::Human(mode) => {
            let active = report.candidates.iter().filter(|c| c.active).count();
            let mut panel = Panel::new(
                "ctx",
                "explain",
                PanelStatus::Passed(format!("{active}/{} active", report.candidates.len())),
            )
            .row(PanelRow::toned("task", task, RowTone::Default));
            for decision in &report.candidates {
                panel = panel.row(PanelRow::toned(
                    &decision.candidate.trait_id,
                    if decision.active {
                        "active"
                    } else {
                        "inactive"
                    },
                    if decision.active {
                        RowTone::Pass
                    } else {
                        RowTone::Default
                    },
                ));
            }
            if mode == HumanOutputMode::Compact {
                panel = panel.next(PanelRow::toned(
                    "next",
                    "ctx traits explain --verbose for the full activation report",
                    RowTone::Default,
                ));
            }
            emit_human(false, &panel, mode, || {
                emit_report(
                    false,
                    || styled_activation_explain_lines(&report, input.active_only),
                    || emit_plain_activation_explain_report(&report, input.active_only),
                )
            })?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// Deterministic evidence shared by every `explain --scaffold` path: the
/// rebased source map, the check receipt, and the receipt-anchored scaffold
/// built from them. The deterministic path, the `--llm-assisted` path, and
/// the dashboard's `explain` runner all build this the same way and
/// must never drift from one another (task 0124).
pub(crate) struct ExplainEvidence {
    pub(crate) scaffold: ctx_traits_core::scaffold::ExplainScaffold,
    pub(crate) check_report: ctx_traits_core::check::CheckReport,
    pub(crate) trait_ref: ctx_traits_core::r#trait::Trait,
    pub(crate) source_digest: ctx_traits_core::digest::Digest,
    pub(crate) source_map: ctx_traits_core::source_map::SourceMap,
    pub(crate) map_path: camino::Utf8PathBuf,
}

pub(crate) fn build_explain_evidence(
    file: &str,
    source_map_arg: Option<&str>,
) -> crate::Result<ExplainEvidence> {
    let trait_path = camino::Utf8Path::new(file);
    let map_path = match source_map_arg {
        Some(path) => camino::Utf8Path::new(path).to_path_buf(),
        None => crate::app::cdk_build::package_source_map(trait_path)?,
    };
    let map_text = ctx_traits_io::read::read_text(&map_path)?;
    let source_map: ctx_traits_core::source_map::SourceMap = serde_json::from_str(&map_text)
        .map_err(|e| crate::Error::json(format!("decode source map {map_path}"), e))?;
    ctx_traits_core::source_map::validate_source_map(&source_map)?;
    let source_map = crate::app::cdk_build::rebase_source_map(
        source_map,
        &crate::app::cdk_build::stable_repo_root(trait_path)?,
    );

    let eval_reports: Vec<String> = Vec::new();
    let check_input = crate::app::report_handlers::CheckInputs {
        file,
        locked: false,
        skip_cdk_drift: false,
        json: false,
        plain: true,
        no_animate: true,
        verbose: false,
        run_ledger: None,
        eval_reports: &eval_reports,
    };
    let check_report = crate::app::report_handlers::build_check_report(&check_input)?;
    let (trait_ref, _, source_digest, _) = ctx_traits_io::run::load_trait(file)?;
    let scaffold =
        ctx_traits_core::scaffold::build_explain_scaffold(&check_report, &trait_ref, &source_map)?;
    scaffold.validate(&source_map)?;
    Ok(ExplainEvidence {
        scaffold,
        check_report,
        trait_ref,
        source_digest,
        source_map,
        map_path,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExplainAssistContext<'a> {
    trait_id: &'a str,
    receipt_digest: &'a str,
    scaffold: &'a ctx_traits_core::scaffold::ExplainScaffold,
}

fn handle_explain_scaffold(input: ExplainInputs<'_>) -> crate::Result<CommandOutput<()>> {
    if input.trait_files.len() != 1 {
        return Err(crate::Error::Command {
            message: "explain --scaffold requires exactly one --file".to_string(),
        });
    }
    let file = &input.trait_files[0];
    let ExplainEvidence {
        scaffold,
        check_report,
        trait_ref,
        source_digest,
        source_map,
        map_path,
    } = build_explain_evidence(file, input.source_map)?;

    let candidate = if input.llm_assisted {
        handle_explain_llm_assisted(
            &input,
            file,
            &scaffold,
            check_report.clone(),
            &trait_ref,
            &source_digest,
            &source_map,
        )?
    } else {
        let boundary_candidate = ctx_traits_core::assist::plan_deterministic_boundary(
            ctx_traits_core::assist::BoundaryRequest {
                operation: ctx_traits_core::assist::Operation::Explain,
                source_trait_ids: vec![trait_ref.id.as_str().to_string()],
                source_paths: vec![file.to_string()],
                source_digests: vec![source_digest],
                user_request: "deterministic receipt-grounded explanation".to_string(),
                model: None,
                target_path: "advisory-no-write-target".to_string(),
                provider_available: false,
                context: serde_json::json!({ "source-map": source_map }),
            },
        )?;
        let raw = serde_json::to_string(&scaffold)
            .map_err(|error| crate::Error::json("serialize explain scaffold", error))?;
        let evaluation = ctx_traits_core::assist::evaluate_supplied_explain_scaffold(
            boundary_candidate,
            &raw,
            &source_map,
        );
        let mut candidate =
            ctx_traits_core::assist::attach_check_report(evaluation.candidate, check_report);
        if candidate.gate_summary.all_passed() {
            candidate = ctx_traits_core::assist::with_context_evidence(
                candidate,
                serde_json::json!({ "explain-scaffold": scaffold }),
            );
        }
        candidate
    };

    use crate::app::presentation::{
        HumanOutputMode, OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
    };

    match OutputMode::select(input.json, input.verbose) {
        OutputMode::Json => {
            crate::app::command_handlers::print_json_report(
                &candidate,
                "explain scaffold candidate",
            )?;
        }
        OutputMode::Human(mode) => {
            if candidate.status != ctx_traits_core::assist::CandidateStatus::Blocked {
                let panel = Panel::new(
                    "ctx",
                    "explain --scaffold",
                    PanelStatus::Passed("passed".to_string()),
                )
                .row(PanelRow::toned(
                    "trait",
                    scaffold.trait_id.as_str(),
                    RowTone::Default,
                ))
                .row(PanelRow::toned("map", map_path.as_str(), RowTone::Default))
                .row(PanelRow::toned(
                    "gates",
                    if candidate.gate_summary.all_passed() {
                        "passed"
                    } else {
                        "blocked"
                    },
                    if candidate.gate_summary.all_passed() {
                        RowTone::Pass
                    } else {
                        RowTone::Fail
                    },
                ));
                let panel = if mode == HumanOutputMode::Compact {
                    panel.next(PanelRow::toned(
                        "next",
                        "ctx traits explain --scaffold --verbose for the full scaffold report",
                        RowTone::Default,
                    ))
                } else {
                    panel
                };
                emit_human(false, &panel, mode, || {
                    emit_report(
                        false,
                        || styled_explain_scaffold_lines(&scaffold, &map_path),
                        || emit_plain_explain_scaffold_report(&scaffold, &map_path),
                    )
                })?;
            }
        }
    }

    if candidate.status == ctx_traits_core::assist::CandidateStatus::Blocked {
        return Err(crate::Error::Command {
            message: "explain scaffold failed candidate validation gates".to_string(),
        });
    }

    Ok(CommandOutput::new(()))
}

/// `--llm-assisted` path: run `explain` (or read `--candidate`) to
/// narrate the same deterministic scaffold every other path builds, then
/// gate it through `explain`'s own receipt-grounding gate
/// (`evaluate_supplied_explain_scaffold`) plus an evidence-equality check —
/// the narrator may add `explanation`, never alter a fact, anchor, or
/// section (Watch: "narrates, never invents"). Always returns a `Candidate`,
/// blocked or not, so the caller's single print/gate-check path handles
/// every outcome uniformly, exactly like the deterministic branch.
#[allow(clippy::too_many_arguments)]
fn handle_explain_llm_assisted(
    input: &ExplainInputs<'_>,
    file: &str,
    scaffold: &ctx_traits_core::scaffold::ExplainScaffold,
    check_report: ctx_traits_core::check::CheckReport,
    trait_ref: &ctx_traits_core::r#trait::Trait,
    source_digest: &ctx_traits_core::digest::Digest,
    source_map: &ctx_traits_core::source_map::SourceMap,
) -> crate::Result<ctx_traits_core::assist::Candidate> {
    let trait_id = trait_ref.id.as_str();
    let boundary_candidate =
        ctx_traits_core::assist::plan_assist_boundary(ctx_traits_core::assist::BoundaryRequest {
            operation: ctx_traits_core::assist::Operation::Explain,
            source_trait_ids: vec![trait_id.to_string()],
            source_paths: vec![file.to_string()],
            source_digests: vec![source_digest.clone()],
            user_request: "receipt-grounded narrated explanation".to_string(),
            model: input.model.map(str::to_string),
            target_path: "advisory-no-write-target".to_string(),
            provider_available: input.candidate_path.is_none(),
            context: serde_json::to_value(ExplainAssistContext {
                trait_id,
                receipt_digest: scaffold.receipt_digest.as_str(),
                scaffold,
            })
            .map_err(|error| crate::Error::json("serialize explain assist context", error))?,
        })?;

    let raw = match input.candidate_path {
        Some(path) => ctx_traits_io::read::read_text(camino::Utf8Path::new(path))?,
        None => {
            let budget_document_document = input
                .budget_document
                .map(camino::Utf8Path::new)
                .map(ctx_traits_io::harness_config::load_budget_document)
                .transpose()?;
            let scaffold_text = serde_json::to_string(scaffold)
                .map_err(|error| crate::Error::json("serialize explain scaffold input", error))?;
            match crate::app::generate::run_builtin_trait(
                "explain",
                vec![
                    crate::app::generate::runtime_input("source-trait-id", trait_id),
                    crate::app::generate::runtime_input(
                        "receipt-digest",
                        scaffold.receipt_digest.as_str(),
                    ),
                    crate::app::generate::runtime_input("scaffold", scaffold_text),
                ],
                input.assignments,
                input.model,
                budget_document_document.as_ref(),
            ) {
                Ok(outcome) => outcome.output,
                Err(error) => {
                    let mut candidate = boundary_candidate;
                    candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
                    candidate.warnings.push(error.to_string());
                    return Ok(candidate);
                }
            }
        }
    };

    let evaluation = ctx_traits_core::assist::evaluate_supplied_explain_scaffold(
        boundary_candidate,
        &raw,
        source_map,
    );
    let mut candidate = evaluation.candidate;
    let Some(narrated) = evaluation.scaffold else {
        // `evaluate_supplied_explain_scaffold` already blocked `candidate`
        // and recorded a diagnostic/warning for the parse/normalize/audit
        // failure; nothing further to add.
        return Ok(candidate);
    };
    if narrated
        .explanation
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate
            .warnings
            .push("explain narration explanation must not be empty".to_string());
        return Ok(candidate);
    }
    if !narrated.evidence_matches(scaffold) {
        candidate.status = ctx_traits_core::assist::CandidateStatus::Blocked;
        candidate
            .warnings
            .push("explain narration altered the deterministic evidence".to_string());
        return Ok(candidate);
    }

    candidate = ctx_traits_core::assist::attach_check_report(candidate, check_report);
    if candidate.gate_summary.all_passed() {
        candidate = ctx_traits_core::assist::with_context_evidence(
            candidate,
            serde_json::json!({ "explain-scaffold": narrated }),
        );
    }
    Ok(candidate)
}

fn activation_capability_reports() -> Vec<ctx_traits_core::response::CapabilityReport> {
    vec![
        ctx_traits_core::response::CapabilityReport::supported("activation.static-explain"),
        ctx_traits_core::response::CapabilityReport::unsupported(
            "activation.dynamic-host",
            "host/plugin activation and live prompt injection are not implemented; this command only explains explicit request facts",
        ),
        ctx_traits_core::response::CapabilityReport::unsupported(
            "activation.trait-registry",
            "trait registry/index candidate loading is not implemented in this phase",
        ),
    ]
}

fn emit_plain_explain_scaffold_report(
    scaffold: &ctx_traits_core::scaffold::ExplainScaffold,
    map_path: &camino::Utf8Path,
) -> crate::Result<()> {
    w("ctx traits explain")?;
    w(format!("  trait: {}", scaffold.trait_id))?;
    w(format!("  source-map: {map_path}"))?;
    w(format!("  receipt-digest: {}", scaffold.receipt_digest))?;
    w(format!("  sections: {}", scaffold.sections.len()))?;
    for section in &scaffold.sections {
        w(format!(
            "  - {} -> {}:{}-{} ({})",
            section.receipt_section,
            section.anchor.file,
            section.anchor.start,
            section.anchor.end,
            section.construct_ref
        ))?;
    }
    for warning in &scaffold.warnings {
        w(format!("  warning: {warning}"))?;
    }
    Ok(())
}

fn styled_explain_scaffold_lines(
    scaffold: &ctx_traits_core::scaffold::ExplainScaffold,
    map_path: &camino::Utf8Path,
) -> Vec<Line> {
    let mut lines = vec![
        command_line("ctx traits explain"),
        Line::blank(),
        labeled_line("  trait: ", &scaffold.trait_id),
        labeled_line("  source-map: ", map_path.as_str()),
        labeled_line("  receipt-digest: ", &scaffold.receipt_digest),
        labeled_line("  sections: ", &scaffold.sections.len().to_string()),
    ];
    for section in &scaffold.sections {
        let mut line = Line::blank();
        line.push("  - ", Tone::Muted);
        line.push(section.receipt_section.clone(), Tone::Default);
        line.push(" -> ", Tone::Muted);
        line.push(
            format!(
                "{}:{}-{}",
                section.anchor.file, section.anchor.start, section.anchor.end
            ),
            Tone::Default,
        );
        line.push(" (", Tone::Muted);
        line.push(section.construct_ref.clone(), Tone::Default);
        line.push(")", Tone::Muted);
        lines.push(line);
    }
    for warning in &scaffold.warnings {
        let mut line = Line::blank();
        line.push("  warning: ", Tone::Muted);
        line.push(warning.clone(), Tone::Warn);
        lines.push(line);
    }
    lines
}

fn emit_plain_activation_explain_report(
    report: &ctx_traits_core::r#trait::activation::ExplainReport,
    active_only: bool,
) -> crate::Result<()> {
    w("ctx traits explain")?;
    w(format!("task: {}", report.request.task_text))?;
    w(format!(
        "mode: {}",
        report.request.mode.as_deref().unwrap_or("none")
    ))?;
    w(format!("files: {}", format_strings(&report.request.files)))?;
    w(format!(
        "languages: {}",
        format_strings(&report.request.language_hints)
    ))?;
    w(format!(
        "signals: {}",
        format_strings(&report.request.signals)
    ))?;
    w(format!(
        "explicit-invocation: {}",
        report
            .request
            .explicit_invocation
            .as_deref()
            .unwrap_or("none")
    ))?;
    w(format!(
        "trait-id: {}",
        report.request.trait_id.as_deref().unwrap_or("none")
    ))?;
    w(format!("active-only: {active_only}"))?;

    w("capabilities:")?;
    for capability in &report.capabilities {
        let status = if capability.supported {
            "supported"
        } else {
            "unsupported"
        };
        match &capability.reason {
            Some(reason) => w(format!(
                "  - {} {}: {}",
                capability.capability, status, reason
            ))?,
            None => w(format!("  - {} {}", capability.capability, status))?,
        }
    }

    if report.candidates.is_empty() {
        w("candidates: none")?;
        w("note: no trait candidates are loaded by this phase")?;
        return Ok(());
    }

    w("candidates:")?;
    for decision in &report.candidates {
        w(format!(
            "  - {}: {} (score {}/{}, priority {})",
            decision.candidate.trait_id,
            if decision.active {
                "active"
            } else {
                "inactive"
            },
            decision.score,
            decision.min_score,
            decision.priority,
        ))?;
        w(format!(
            "    reasons: {}",
            format_strings(&decision.reason_codes)
        ))?;
        for gate in &decision.gates {
            w(format!("    gate {}: {}", gate.code, gate.message))?;
        }
        for rule in &decision.rules {
            w(format!(
                "    rule {}: matched={}, excluded={}, score={}, reasons={}",
                rule.rule_id,
                rule.matched,
                rule.excluded,
                rule.score,
                format_strings(&rule.reason_codes)
            ))?;
            if !rule.matched_facts.is_empty() {
                w("      matched-facts:")?;
                for fact in &rule.matched_facts {
                    w(format!(
                        "        - kind={}, pattern={}, value={}",
                        fact.kind, fact.pattern, fact.value
                    ))?;
                }
            }
        }
    }

    if !report.relations.edges.is_empty() {
        w("relation-edges:")?;
        for edge in &report.relations.edges {
            let target_ref = edge.target_ref.as_deref().unwrap_or("-");
            w(format!(
                "  - {} {:?}-> {}: {:?} (reason: {})",
                edge.source_trait_id, edge.kind, edge.target_trait_id, edge.effect, edge.reason,
            ))?;
            w(format!("    target-ref: {target_ref}"))?;
            if edge.when_refs.is_empty() {
                w("    when: none (unconditional)")?;
            } else {
                w(format!(
                    "    when: {}",
                    edge.when_refs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))?;
            }
            w(format!("    when-matched: {}", edge.when_matched))?;

            for outcome in &edge.port_target_outcomes {
                w(format!(
                    "    port-target-outcome: kind={:?}, target-ref={}, port-id={}, reason={}",
                    outcome.kind,
                    outcome.target_ref,
                    outcome.target_port_id.as_deref().unwrap_or("-"),
                    outcome.reason,
                ))?;
                if !outcome.binding_proposal_keys.is_empty() {
                    w(format!(
                        "      proposal-keys: {}",
                        outcome.binding_proposal_keys.join("; ")
                    ))?;
                }
            }

            for proposal in &edge.binding_proposals {
                w(format!(
                    "    binding-proposal: consumer={}/{}, provider={}/{}, compat={:?}, status={:?}",
                    proposal.consumer.trait_id,
                    proposal.consumer.port_id,
                    proposal.provider.trait_id,
                    proposal.provider.port_id,
                    proposal.compatibility,
                    proposal.status,
                ))?;
                w(format!(
                    "      consumer-schema: {}",
                    proposal
                        .consumer
                        .schema_ref
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                ))?;
                w(format!(
                    "      provider-schema: {}",
                    proposal
                        .provider
                        .schema_ref
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                ))?;
                if let Some(ref evidence) = proposal.schema_evidence {
                    w(format!("      evidence: {evidence}"))?;
                }
                w(format!("      reason: {}", proposal.reason))?;
            }
        }
    }

    if !report.relations.cycles.is_empty() {
        w("relation-cycles:")?;
        for cycle in &report.relations.cycles {
            w(format!("  - {}", cycle.description))?;
        }
    }
    Ok(())
}

fn styled_activation_explain_lines(
    report: &ctx_traits_core::r#trait::activation::ExplainReport,
    active_only: bool,
) -> Vec<Line> {
    let mut lines = vec![
        command_line("ctx traits explain"),
        Line::blank(),
        labeled_line("task: ", &report.request.task_text),
        labeled_line("mode: ", report.request.mode.as_deref().unwrap_or("none")),
        labeled_line("files: ", &format_strings(&report.request.files)),
        labeled_line(
            "languages: ",
            &format_strings(&report.request.language_hints),
        ),
        labeled_line("signals: ", &format_strings(&report.request.signals)),
        labeled_line(
            "explicit-invocation: ",
            report
                .request
                .explicit_invocation
                .as_deref()
                .unwrap_or("none"),
        ),
        labeled_line(
            "trait-id: ",
            report.request.trait_id.as_deref().unwrap_or("none"),
        ),
        labeled_line("active-only: ", &active_only.to_string()),
    ];

    let mut capabilities_header = Line::blank();
    capabilities_header.push("capabilities:", Tone::Muted);
    lines.push(capabilities_header);
    for capability in &report.capabilities {
        let (status, tone) = if capability.supported {
            ("supported", Tone::Pass)
        } else {
            ("unsupported", Tone::Warn)
        };
        let mut line = Line::blank();
        line.push("  - ", Tone::Muted);
        line.push(capability.capability.clone(), Tone::Default);
        line.push(" ", Tone::Muted);
        line.push(status, tone);
        if let Some(reason) = &capability.reason {
            line.push(": ", Tone::Muted);
            line.push(reason.clone(), Tone::Default);
        }
        lines.push(line);
    }

    if report.candidates.is_empty() {
        let mut none_line = Line::blank();
        none_line.push("candidates: ", Tone::Muted);
        none_line.push("none", Tone::Default);
        lines.push(none_line);
        let mut note_line = Line::blank();
        note_line.push(
            "note: no trait candidates are loaded by this phase",
            Tone::Muted,
        );
        lines.push(note_line);
        return lines;
    }

    let mut candidates_header = Line::blank();
    candidates_header.push("candidates:", Tone::Muted);
    lines.push(candidates_header);
    for decision in &report.candidates {
        let (active_text, active_tone) = if decision.active {
            ("active", Tone::Pass)
        } else {
            ("inactive", Tone::Warn)
        };
        let mut header = Line::blank();
        header.push("  - ", Tone::Muted);
        header.push(decision.candidate.trait_id.clone(), Tone::Default);
        header.push(": ", Tone::Muted);
        header.push(active_text, active_tone);
        header.push(
            format!(
                " (score {}/{}, priority {})",
                decision.score, decision.min_score, decision.priority
            ),
            Tone::Muted,
        );
        lines.push(header);
        lines.push(labeled_line(
            "    reasons: ",
            &format_strings(&decision.reason_codes),
        ));
        for gate in &decision.gates {
            let mut line = Line::blank();
            line.push("    gate ", Tone::Muted);
            line.push(gate.code.clone(), Tone::Default);
            line.push(": ", Tone::Muted);
            line.push(gate.message.clone(), Tone::Default);
            lines.push(line);
        }
        for rule in &decision.rules {
            let mut line = Line::blank();
            line.push("    rule ", Tone::Muted);
            line.push(rule.rule_id.clone(), Tone::Default);
            line.push(
                format!(
                    ": matched={}, excluded={}, score={}, reasons={}",
                    rule.matched,
                    rule.excluded,
                    rule.score,
                    format_strings(&rule.reason_codes)
                ),
                Tone::Muted,
            );
            lines.push(line);
            if !rule.matched_facts.is_empty() {
                let mut header = Line::blank();
                header.push("      matched-facts:", Tone::Muted);
                lines.push(header);
                for fact in &rule.matched_facts {
                    let mut fact_line = Line::blank();
                    fact_line.push(
                        format!(
                            "        - kind={}, pattern={}, value={}",
                            fact.kind, fact.pattern, fact.value
                        ),
                        Tone::Default,
                    );
                    lines.push(fact_line);
                }
            }
        }
    }

    if !report.relations.edges.is_empty() {
        let mut header = Line::blank();
        header.push("relation-edges:", Tone::Muted);
        lines.push(header);
        for edge in &report.relations.edges {
            let target_ref = edge.target_ref.as_deref().unwrap_or("-");
            let mut edge_line = Line::blank();
            edge_line.push(
                format!(
                    "  - {} {:?}-> {}: {:?} (reason: {})",
                    edge.source_trait_id, edge.kind, edge.target_trait_id, edge.effect, edge.reason,
                ),
                Tone::Default,
            );
            lines.push(edge_line);
            lines.push(labeled_line("    target-ref: ", target_ref));
            if edge.when_refs.is_empty() {
                let mut when_line = Line::blank();
                when_line.push("    when: none (unconditional)", Tone::Muted);
                lines.push(when_line);
            } else {
                lines.push(labeled_line(
                    "    when: ",
                    &edge
                        .when_refs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            lines.push(labeled_line(
                "    when-matched: ",
                &edge.when_matched.to_string(),
            ));

            for outcome in &edge.port_target_outcomes {
                let mut outcome_line = Line::blank();
                outcome_line.push(
                    format!(
                        "    port-target-outcome: kind={:?}, target-ref={}, port-id={}, reason={}",
                        outcome.kind,
                        outcome.target_ref,
                        outcome.target_port_id.as_deref().unwrap_or("-"),
                        outcome.reason,
                    ),
                    Tone::Default,
                );
                lines.push(outcome_line);
                if !outcome.binding_proposal_keys.is_empty() {
                    lines.push(labeled_line(
                        "      proposal-keys: ",
                        &outcome.binding_proposal_keys.join("; "),
                    ));
                }
            }

            for proposal in &edge.binding_proposals {
                let mut proposal_line = Line::blank();
                proposal_line.push(
                    format!(
                        "    binding-proposal: consumer={}/{}, provider={}/{}, compat={:?}, status={:?}",
                        proposal.consumer.trait_id,
                        proposal.consumer.port_id,
                        proposal.provider.trait_id,
                        proposal.provider.port_id,
                        proposal.compatibility,
                        proposal.status,
                    ),
                    Tone::Default,
                );
                lines.push(proposal_line);
                lines.push(labeled_line(
                    "      consumer-schema: ",
                    &proposal
                        .consumer
                        .schema_ref
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                ));
                lines.push(labeled_line(
                    "      provider-schema: ",
                    &proposal
                        .provider
                        .schema_ref
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                ));
                if let Some(ref evidence) = proposal.schema_evidence {
                    lines.push(labeled_line("      evidence: ", evidence));
                }
                lines.push(labeled_line("      reason: ", &proposal.reason));
            }
        }
    }

    if !report.relations.cycles.is_empty() {
        let mut header = Line::blank();
        header.push("relation-cycles:", Tone::Muted);
        lines.push(header);
        for cycle in &report.relations.cycles {
            let mut line = Line::blank();
            line.push("  - ", Tone::Muted);
            line.push(cycle.description.clone(), Tone::Default);
            lines.push(line);
        }
    }
    lines
}

fn format_strings(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

pub(crate) fn handle_inspect(
    file: Option<&str>,
    dry_plan: bool,
    profile: Option<&str>,
) -> crate::Result<CommandOutput<()>> {
    // Non-dry inspect: load and decode the trait, then print scenario/eval
    // review artifacts without resource digesting or procedure planning.
    if !dry_plan {
        let Some(file) = file else {
            emit_report(
                false,
                styled_inspect_no_file_lines,
                emit_plain_inspect_no_file_report,
            )?;
            return Ok(CommandOutput::new(()));
        };

        let path = camino::Utf8Path::new(file);
        let encoding = ctx_traits_core::encoding::Encoding::from_path(path)?;
        let text = ctx_traits_io::read::read_text(path)?;
        let (trait_ref, decode_warnings) =
            ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &text)?;
        ctx_traits_io::decode_diagnostics::print_decode_warnings(file, &decode_warnings);
        let (status, trust) = inspect_lifecycle(path, &trait_ref)?;

        emit_report(
            false,
            || styled_inspect_normal_lines(&trait_ref, &status, &trust),
            || emit_plain_inspect_normal_report(&trait_ref, &status, &trust),
        )?;

        return Ok(CommandOutput::new(()));
    }

    let file = file.ok_or_else(|| crate::Error::Command {
        message: "--file is required with --dry-plan".to_string(),
    })?;
    let path = camino::Utf8Path::new(file);
    let encoding = ctx_traits_core::encoding::Encoding::from_path(path)?;
    let text = ctx_traits_io::read::read_text(path)?;
    let (trait_ref, decode_warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &text)?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(file, &decode_warnings);

    emit_report(
        false,
        || styled_inspect_dry_plan_header_lines(&trait_ref),
        || emit_plain_inspect_dry_plan_header(&trait_ref),
    )?;

    let runtime_plan = ctx_traits_core::procedure::run::plan_trait_runtime(
        &trait_ref,
        ctx_traits_core::procedure::run::Id::new("inspect")?,
    )?;

    // Parse render profile (defaults to agent-skills).
    let profile_str = profile.unwrap_or("agent-skills");
    let render_profile = ctx_traits_core::resource_plan::RenderProfile::parse(profile_str)
        .ok_or_else(|| crate::Error::Command {
            message: format!(
                "unsupported profile: {profile_str:?} (expected agent-skills, pi, opencode, claude-code, or codex)"
            ),
        })?;

    // Generated manifests keep package-relative resources one level above.
    let trait_root = ctx_traits_io::layout::package_root_for_manifest(path).ok_or_else(|| {
        crate::Error::Command {
            message: "trait file has no package root".to_string(),
        }
    })?;

    // Digest declared resources via IO. Always called so the manifest
    // digest is printed even for empty resource sets.
    let roots = ctx_traits_io::resource::resolve_resource_roots(trait_root, &trait_ref.resources)?;
    let manifest = ctx_traits_io::resource::digest_resources(
        &roots,
        trait_ref.id.as_str(),
        &trait_ref.resources,
    )?;

    emit_report(
        false,
        || styled_inspect_manifest_lines(&manifest),
        || emit_plain_inspect_manifest(&manifest),
    )?;

    let file_evidence = build_file_evidence_from_io(&manifest);

    let resource_plan =
        ctx_traits_core::resource_plan::plan_resource_inclusion(&trait_ref, &file_evidence);

    emit_report(
        false,
        || styled_inspect_dry_and_resource_plan_lines(&runtime_plan, &resource_plan),
        || emit_plain_inspect_dry_and_resource_plan(&runtime_plan, &resource_plan),
    )?;

    let budget = ctx_traits_core::resource_plan::estimate_context_budget(&resource_plan);

    emit_report(
        false,
        || vec![styled_inspect_budget_line(&budget)],
        || emit_plain_inspect_budget(&budget),
    )?;

    // Surface render compatibility warnings.
    let render_warnings =
        ctx_traits_core::resource_plan::check_render_compatibility(&resource_plan, render_profile);

    emit_report(
        false,
        || styled_inspect_render_warnings_lines(&render_warnings),
        || emit_plain_inspect_render_warnings(&render_warnings),
    )?;

    emit_report(
        false,
        || {
            let mut lines = Vec::new();
            styled_scenarios_lines(&trait_ref, &mut lines);
            styled_evals_lines(&trait_ref, &mut lines);
            lines
        },
        || {
            emit_plain_scenarios(&trait_ref)?;
            emit_plain_evals(&trait_ref)?;
            Ok(())
        },
    )?;

    Ok(CommandOutput::new(()))
}

fn emit_plain_inspect_no_file_report() -> crate::Result<()> {
    w("ctx traits inspect — use --file <trait-file> to inspect a trait")?;
    w("  add --dry-plan for procedure/resource planning")?;
    Ok(())
}

fn styled_inspect_no_file_lines() -> Vec<Line> {
    let mut lines = vec![command_line("ctx traits inspect"), Line::blank()];
    let mut header = Line::blank();
    header.push("use --file <trait-file> to inspect a trait", Tone::Default);
    lines.push(header);
    let mut hint = Line::blank();
    hint.push(
        "  add --dry-plan for procedure/resource planning",
        Tone::Muted,
    );
    lines.push(hint);
    lines
}

/// Resolve `(package status, trust verdict)` for an inspected trait file.
///
/// The canonical trait document carries neither field: status comes from the
/// package's `trait.toml` `[package].status`, trust from the machine trust
/// store keyed by canonical digest. A file with no discoverable package root
/// falls back to the same conservative defaults as an external import
/// (`draft`, `unreviewed`).
fn inspect_lifecycle(
    path: &camino::Utf8Path,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<(
    ctx_traits_core::manifest::PackageStatus,
    ctx_traits_core::r#trait::TrustVerdict,
)> {
    let canonical_digest = ctx_traits_core::digest::canonical_digest(trait_ref)?;
    let status = match ctx_traits_io::layout::package_root_for_manifest(path) {
        Some(trait_root) => ctx_traits_io::lifecycle::resolve_package_status(trait_root)?,
        None => ctx_traits_core::manifest::PackageStatus::Draft,
    };
    let trust = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;
    Ok((status, trust))
}

fn lifecycle_next_text(status: &ctx_traits_core::manifest::PackageStatus) -> &'static str {
    match status {
        ctx_traits_core::manifest::PackageStatus::Draft => "review and activate before use",
        ctx_traits_core::manifest::PackageStatus::Ready => {
            "ready; deactivate (set status back to draft) when no longer needed"
        }
    }
}

fn emit_plain_inspect_normal_report(
    trait_ref: &ctx_traits_core::Trait,
    status: &ctx_traits_core::manifest::PackageStatus,
    trust: &ctx_traits_core::r#trait::TrustVerdict,
) -> crate::Result<()> {
    w("ctx traits inspect")?;
    w(format!("trait: {id}", id = trait_ref.id.as_str()))?;
    w(format!(
        "version: {version}",
        version = trait_ref.version.as_str()
    ))?;
    w(format!(
        "schema-version: {schema_version}",
        schema_version = trait_ref.schema_version.as_str()
    ))?;
    w(format!("name: {name}", name = trait_ref.name.as_str()))?;
    w(format!("status: {status}", status = status.display_name()))?;
    w(format!("trust: {trust}", trust = trust.display_name()))?;
    w(format!("lifecycle: {}", lifecycle_next_text(status)))?;

    emit_plain_scenarios(trait_ref)?;
    emit_plain_evals(trait_ref)?;
    Ok(())
}

fn styled_inspect_normal_lines(
    trait_ref: &ctx_traits_core::Trait,
    status: &ctx_traits_core::manifest::PackageStatus,
    trust: &ctx_traits_core::r#trait::TrustVerdict,
) -> Vec<Line> {
    let mut lines = vec![
        command_line("ctx traits inspect"),
        Line::blank(),
        labeled_line("trait: ", trait_ref.id.as_str()),
        labeled_line("version: ", trait_ref.version.as_str()),
        labeled_line("schema-version: ", trait_ref.schema_version.as_str()),
        labeled_line("name: ", trait_ref.name.as_str()),
        labeled_line("status: ", status.display_name()),
        labeled_line("trust: ", trust.display_name()),
        labeled_line("lifecycle: ", lifecycle_next_text(status)),
    ];

    styled_scenarios_lines(trait_ref, &mut lines);
    styled_evals_lines(trait_ref, &mut lines);
    lines
}

fn emit_plain_inspect_dry_plan_header(trait_ref: &ctx_traits_core::Trait) -> crate::Result<()> {
    w("ctx traits inspect")?;
    w(format!("trait: {id}", id = trait_ref.id.as_str()))?;
    w(format!(
        "version: {version}",
        version = trait_ref.version.as_str()
    ))?;
    w(format!(
        "schema-version: {schema_version}",
        schema_version = trait_ref.schema_version.as_str()
    ))?;
    Ok(())
}

fn styled_inspect_dry_plan_header_lines(trait_ref: &ctx_traits_core::Trait) -> Vec<Line> {
    vec![
        command_line("ctx traits inspect"),
        Line::blank(),
        labeled_line("trait: ", trait_ref.id.as_str()),
        labeled_line("version: ", trait_ref.version.as_str()),
        labeled_line("schema-version: ", trait_ref.schema_version.as_str()),
    ]
}

fn emit_plain_inspect_manifest(
    manifest: &ctx_traits_io::resource::ResourceManifestDigest,
) -> crate::Result<()> {
    w(format!(
        "resource-manifest-digest: {digest}",
        digest = manifest.manifest_digest.as_str()
    ))?;
    emit_plain_resource_read_warnings(&manifest.warnings)?;
    Ok(())
}

fn styled_inspect_manifest_lines(
    manifest: &ctx_traits_io::resource::ResourceManifestDigest,
) -> Vec<Line> {
    let mut lines = vec![labeled_line(
        "resource-manifest-digest: ",
        manifest.manifest_digest.as_str(),
    )];
    styled_resource_read_warnings_lines(&manifest.warnings, &mut lines);
    lines
}

fn emit_plain_inspect_dry_and_resource_plan(
    runtime_plan: &ctx_traits_core::procedure::run::TraitPlan,
    resource_plan: &ctx_traits_core::resource_plan::Plan,
) -> crate::Result<()> {
    emit_plain_dry_plan(runtime_plan)?;
    emit_plain_resource_plan(resource_plan)?;
    Ok(())
}

fn styled_inspect_dry_and_resource_plan_lines(
    runtime_plan: &ctx_traits_core::procedure::run::TraitPlan,
    resource_plan: &ctx_traits_core::resource_plan::Plan,
) -> Vec<Line> {
    let mut lines = Vec::new();
    styled_dry_plan_lines(runtime_plan, &mut lines);
    styled_resource_plan_lines(resource_plan, &mut lines);
    lines
}

fn emit_plain_inspect_budget(
    budget: &ctx_traits_core::resource_plan::ContextBudget,
) -> crate::Result<()> {
    w(format!(
        "resource-budget: {bytes} bytes (~{tokens} tokens, {measured} measured, {unmeasured} unmeasured)",
        bytes = budget.total_bytes,
        tokens = budget.estimated_tokens,
        measured = budget.measured_count,
        unmeasured = budget.unmeasured_count,
    ))?;
    Ok(())
}

fn styled_inspect_budget_line(budget: &ctx_traits_core::resource_plan::ContextBudget) -> Line {
    labeled_line(
        "resource-budget: ",
        &format!(
            "{bytes} bytes (~{tokens} tokens, {measured} measured, {unmeasured} unmeasured)",
            bytes = budget.total_bytes,
            tokens = budget.estimated_tokens,
            measured = budget.measured_count,
            unmeasured = budget.unmeasured_count,
        ),
    )
}

fn emit_plain_inspect_render_warnings(
    render_warnings: &[ctx_traits_core::resource_plan::RenderWarning],
) -> crate::Result<()> {
    if render_warnings.is_empty() {
        w("render-resource-warnings: none")?;
    } else {
        w("render-resource-warnings:")?;
        for warning in render_warnings {
            w(format!(
                "  {id} ({profile}): {reason}",
                id = warning.resource_id,
                profile = warning.profile.as_str(),
                reason = warning.reason
            ))?;
        }
    }
    Ok(())
}

fn styled_inspect_render_warnings_lines(
    render_warnings: &[ctx_traits_core::resource_plan::RenderWarning],
) -> Vec<Line> {
    let mut lines = Vec::new();
    if render_warnings.is_empty() {
        let mut line = Line::blank();
        line.push("render-resource-warnings: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
    } else {
        let mut header = Line::blank();
        header.push("render-resource-warnings:", Tone::Muted);
        lines.push(header);
        for warning in render_warnings {
            let mut line = Line::blank();
            line.push(
                format!(
                    "  {id} ({profile}): ",
                    id = warning.resource_id,
                    profile = warning.profile.as_str()
                ),
                Tone::Muted,
            );
            line.push(warning.reason.clone(), Tone::Warn);
            lines.push(line);
        }
    }
    lines
}

pub(crate) fn build_file_evidence_from_io(
    manifest: &ctx_traits_io::resource::ResourceManifestDigest,
) -> Vec<ctx_traits_core::resource_plan::FileEvidence> {
    use ctx_traits_io::resource::ResourceReadWarning;

    // Build per-resource evidence maps from outcomes.
    let digest_map: std::collections::BTreeMap<&str, &ctx_traits_io::resource::ResourceFileDigest> =
        manifest
            .file_digests
            .iter()
            .map(|fd| (fd.resource_id.as_str(), fd))
            .collect();

    // Collect issue flags per resource from warnings.
    let mut missing_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut symlinks: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut special_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut directories: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for warning in &manifest.warnings {
        match warning {
            ResourceReadWarning::MissingFile { resource_id, .. } => {
                missing_files.insert(resource_id.clone());
            }
            ResourceReadWarning::SymlinkDetected { resource_id, .. } => {
                symlinks.insert(resource_id.clone());
            }
            ResourceReadWarning::SpecialFile { resource_id, .. } => {
                special_files.insert(resource_id.clone());
            }
            ResourceReadWarning::Directory { resource_id, .. } => {
                directories.insert(resource_id.clone());
            }
            ResourceReadWarning::BinaryContent { .. } => {}
        }
    }

    // Collect all resource IDs from digests and issue sets.
    let mut all_ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for fd in &manifest.file_digests {
        all_ids.insert(fd.resource_id.as_str());
    }
    for id in &missing_files {
        all_ids.insert(id.as_str());
    }
    for id in &symlinks {
        all_ids.insert(id.as_str());
    }
    for id in &special_files {
        all_ids.insert(id.as_str());
    }
    for id in &directories {
        all_ids.insert(id.as_str());
    }

    all_ids
        .iter()
        .map(|&rid| {
            let fd = digest_map.get(rid);
            ctx_traits_core::resource_plan::file_evidence_from_io(
                rid,
                fd.map(|d| &d.digest),
                fd.map(|d| d.byte_size).unwrap_or(0),
                fd.map(|d| d.is_binary).unwrap_or(false),
                missing_files.contains(rid),
                symlinks.contains(rid),
                directories.contains(rid),
            )
        })
        .collect()
}

fn emit_plain_dry_plan(plan: &ctx_traits_core::procedure::run::TraitPlan) -> crate::Result<()> {
    use ctx_traits_core::procedure::run::TraitPlan;
    match plan {
        TraitPlan::GuidanceOnly { trait_id, reason } => {
            w("ctx traits inspect --dry-plan")?;
            w(format!("trait: {trait_id}"))?;
            w("shape: guidance-only")?;
            w(format!("reason: {reason}"))?;
        }
        TraitPlan::Planned(run) => {
            w("ctx traits inspect --dry-plan")?;
            w(format!("trait: {}", run.trait_id))?;

            if run.sequence_items.is_empty() {
                w("sequence-items: none")?;
            } else {
                w("sequence-items:")?;
                for item in &run.sequence_items {
                    emit_plain_planned_sequence_item(item, 1)?;
                }
            }

            if run.port_requirements.is_empty() {
                w("port-requirements: none")?;
            } else {
                w("port-requirements:")?;
                for req in &run.port_requirements {
                    w(format!(
                        "  {port_ref} (id={port_id}):",
                        port_ref = req.port_ref,
                        port_id = req.port_id
                    ))?;
                    w(format!("    required: {required}", required = req.required))?;
                    w(format!(
                        "    status: {}",
                        format_port_requirement_status(&req.status)
                    ))?;
                    w(format!("    reason: {reason}", reason = req.reason))?;
                }
            }

            if run.output_ports.is_empty() {
                w("output-ports: none")?;
            } else {
                w("output-ports:")?;
                for port in &run.output_ports {
                    w(format!("  {port_ref}:", port_ref = port.port_ref))?;
                    w(format!(
                        "    value-slot: {slot}",
                        slot = port.value_slot_ref
                    ))?;
                    w(format!(
                        "    required: {required}",
                        required = port.required
                    ))?;
                    w(format!(
                        "    status: {}",
                        format_planned_output_port_status(&port.status)
                    ))?;
                    w(format!("    reason: {reason}", reason = port.reason))?;
                }
            }

            if run.slots.is_empty() {
                w("slots: none")?;
            } else {
                w("slots:")?;
                for slot in &run.slots {
                    w(format!(
                        "  {ref}: {state}",
                        ref = slot.slot_ref,
                        state = format_slot_state(&slot.state)
                    ))?;
                }
            }
        }
    }
    Ok(())
}

fn styled_dry_plan_lines(plan: &ctx_traits_core::procedure::run::TraitPlan, lines: &mut Vec<Line>) {
    use ctx_traits_core::procedure::run::TraitPlan;
    match plan {
        TraitPlan::GuidanceOnly { trait_id, reason } => {
            let mut header = Line::blank();
            header.push("ctx traits inspect --dry-plan", Tone::Muted);
            lines.push(header);
            lines.push(labeled_line("trait: ", trait_id));
            lines.push(labeled_line("shape: ", "guidance-only"));
            lines.push(labeled_line("reason: ", reason));
        }
        TraitPlan::Planned(run) => {
            let mut header = Line::blank();
            header.push("ctx traits inspect --dry-plan", Tone::Muted);
            lines.push(header);
            lines.push(labeled_line("trait: ", &run.trait_id));

            if run.sequence_items.is_empty() {
                let mut line = Line::blank();
                line.push("sequence-items: ", Tone::Muted);
                line.push("none", Tone::Default);
                lines.push(line);
            } else {
                let mut header = Line::blank();
                header.push("sequence-items:", Tone::Muted);
                lines.push(header);
                for item in &run.sequence_items {
                    styled_planned_sequence_item_lines(item, 1, lines);
                }
            }

            if run.port_requirements.is_empty() {
                let mut line = Line::blank();
                line.push("port-requirements: ", Tone::Muted);
                line.push("none", Tone::Default);
                lines.push(line);
            } else {
                let mut header = Line::blank();
                header.push("port-requirements:", Tone::Muted);
                lines.push(header);
                for req in &run.port_requirements {
                    let mut line = Line::blank();
                    line.push("  ", Tone::Muted);
                    line.push(req.port_ref.clone(), Tone::Default);
                    line.push(format!(" (id={}):", req.port_id), Tone::Muted);
                    lines.push(line);
                    lines.push(labeled_line("    required: ", &req.required.to_string()));
                    lines.push(labeled_line(
                        "    status: ",
                        format_port_requirement_status(&req.status),
                    ));
                    lines.push(labeled_line("    reason: ", &req.reason));
                }
            }

            if run.output_ports.is_empty() {
                let mut line = Line::blank();
                line.push("output-ports: ", Tone::Muted);
                line.push("none", Tone::Default);
                lines.push(line);
            } else {
                let mut header = Line::blank();
                header.push("output-ports:", Tone::Muted);
                lines.push(header);
                for port in &run.output_ports {
                    let mut line = Line::blank();
                    line.push("  ", Tone::Muted);
                    line.push(port.port_ref.clone(), Tone::Default);
                    line.push(":", Tone::Muted);
                    lines.push(line);
                    lines.push(labeled_line("    value-slot: ", &port.value_slot_ref));
                    lines.push(labeled_line("    required: ", &port.required.to_string()));
                    lines.push(labeled_line(
                        "    status: ",
                        format_planned_output_port_status(&port.status),
                    ));
                    lines.push(labeled_line("    reason: ", &port.reason));
                }
            }

            if run.slots.is_empty() {
                let mut line = Line::blank();
                line.push("slots: ", Tone::Muted);
                line.push("none", Tone::Default);
                lines.push(line);
            } else {
                let mut header = Line::blank();
                header.push("slots:", Tone::Muted);
                lines.push(header);
                for slot in &run.slots {
                    lines.push(labeled_line(
                        &format!("  {}: ", slot.slot_ref),
                        format_slot_state(&slot.state),
                    ));
                }
            }
        }
    }
}

fn emit_plain_planned_sequence_item(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    depth: usize,
) -> crate::Result<()> {
    let indent = "  ".repeat(depth);
    let child_indent = "  ".repeat(depth + 1);
    let id = item.item_id.as_deref().unwrap_or("-");
    let title = if item.title.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", item.title)
    };
    w(format!(
        "{indent}[run {run_index} / source {source_index}] (id={id}){title}",
        run_index = item.run_index,
        source_index = item.sequence_index,
    ))?;
    w(format!(
        "{child_indent}kind: {}",
        format_planned_sequence_kind(&item.kind)
    ))?;
    w(format!(
        "{child_indent}status: {}",
        format_sequence_item_status(&item.status)
    ))?;
    if let Some(agent_ref) = item.agent_ref.as_deref() {
        w(format!("{child_indent}agent: {agent_ref}"))?;
    }
    w(format!(
        "{child_indent}prompt: {}",
        item.prompt_source
            .as_ref()
            .map(format_prompt_source)
            .unwrap_or("none")
    ))?;
    if let Some(sequence_ref) = item.sequence_ref.as_deref() {
        let marker_note = if item.children.is_empty() {
            " (expanded at first reference)"
        } else {
            ""
        };
        w(format!(
            "{child_indent}sequence: {sequence_ref}{marker_note}"
        ))?;
    }
    if let Some(sequence_ref) = item.otherwise_sequence_ref.as_deref() {
        let marker_note = if item.otherwise_children.is_empty() {
            " (expanded at first reference)"
        } else {
            ""
        };
        w(format!(
            "{child_indent}otherwise: {sequence_ref}{marker_note}"
        ))?;
    }
    if !item.input_refs.is_empty() {
        w(format!(
            "{child_indent}input: {}",
            item.input_refs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
    }
    if !item.output_refs.is_empty() {
        w(format!(
            "{child_indent}output: {}",
            item.output_refs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
    }
    if !item.children.is_empty() {
        let label = if item.kind == ctx_traits_core::procedure::run::PlannedSequenceKind::Branch {
            "then-children"
        } else {
            "children"
        };
        w(format!("{child_indent}{label}:"))?;
        for child in &item.children {
            emit_plain_planned_sequence_item(child, depth + 2)?;
        }
    }
    if !item.otherwise_children.is_empty() {
        w(format!("{child_indent}otherwise-children:"))?;
        for child in &item.otherwise_children {
            emit_plain_planned_sequence_item(child, depth + 2)?;
        }
    }
    if let Some(max_branches) = item.max_branches {
        w(format!("{child_indent}max-branches: {max_branches}"))?;
    }
    if item.concurrent {
        w(format!("{child_indent}concurrent: true"))?;
    }
    if !item.parallel_branches.is_empty() {
        w(format!("{child_indent}branches:"))?;
        for branch in &item.parallel_branches {
            w(format!("{child_indent}  - {}", branch.sequence_ref))?;
            for child in &branch.children {
                emit_plain_planned_sequence_item(child, depth + 3)?;
            }
        }
        w(format!(
            "{child_indent}join: {}",
            item.join.as_ref().map_or(
                "collect-in-order",
                ctx_traits_core::r#trait::procedure::JoinPolicy::label
            )
        ))?;
        if !item.branch_failure.is_empty() {
            w(format!("{child_indent}branch-failure:"))?;
            for entry in &item.branch_failure {
                w(format!(
                    "{child_indent}  - {}: {}",
                    entry.branch,
                    branch_failure_policy_label(entry.on_failure)
                ))?;
            }
        }
    }
    Ok(())
}

fn branch_failure_policy_label(
    policy: ctx_traits_core::r#trait::procedure::BranchFailurePolicy,
) -> &'static str {
    match policy {
        ctx_traits_core::r#trait::procedure::BranchFailurePolicy::Skip => "skip",
        ctx_traits_core::r#trait::procedure::BranchFailurePolicy::Park => "park",
        ctx_traits_core::r#trait::procedure::BranchFailurePolicy::PanelFail => "panel-fail",
    }
}

fn styled_planned_sequence_item_lines(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    depth: usize,
    lines: &mut Vec<Line>,
) {
    let indent = "  ".repeat(depth);
    let child_indent = "  ".repeat(depth + 1);
    let id = item.item_id.as_deref().unwrap_or("-");
    let title = if item.title.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", item.title)
    };
    let mut header = Line::blank();
    header.push(
        format!(
            "{indent}[run {run_index} / source {source_index}] (id={id}){title}",
            run_index = item.run_index,
            source_index = item.sequence_index,
        ),
        Tone::Default,
    );
    lines.push(header);
    lines.push(labeled_line(
        &format!("{child_indent}kind: "),
        format_planned_sequence_kind(&item.kind),
    ));
    lines.push(labeled_line(
        &format!("{child_indent}status: "),
        format_sequence_item_status(&item.status),
    ));
    if let Some(agent_ref) = item.agent_ref.as_deref() {
        lines.push(labeled_line(&format!("{child_indent}agent: "), agent_ref));
    }
    lines.push(labeled_line(
        &format!("{child_indent}prompt: "),
        item.prompt_source
            .as_ref()
            .map(format_prompt_source)
            .unwrap_or("none"),
    ));
    if let Some(sequence_ref) = item.sequence_ref.as_deref() {
        let marker_note = if item.children.is_empty() {
            " (expanded at first reference)"
        } else {
            ""
        };
        lines.push(labeled_line(
            &format!("{child_indent}sequence: "),
            &format!("{sequence_ref}{marker_note}"),
        ));
    }
    if let Some(sequence_ref) = item.otherwise_sequence_ref.as_deref() {
        let marker_note = if item.otherwise_children.is_empty() {
            " (expanded at first reference)"
        } else {
            ""
        };
        lines.push(labeled_line(
            &format!("{child_indent}otherwise: "),
            &format!("{sequence_ref}{marker_note}"),
        ));
    }
    if !item.input_refs.is_empty() {
        lines.push(labeled_line(
            &format!("{child_indent}input: "),
            &item
                .input_refs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if !item.output_refs.is_empty() {
        lines.push(labeled_line(
            &format!("{child_indent}output: "),
            &item
                .output_refs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if !item.children.is_empty() {
        let label = if item.kind == ctx_traits_core::procedure::run::PlannedSequenceKind::Branch {
            "then-children"
        } else {
            "children"
        };
        let mut header = Line::blank();
        header.push(format!("{child_indent}{label}:"), Tone::Muted);
        lines.push(header);
        for child in &item.children {
            styled_planned_sequence_item_lines(child, depth + 2, lines);
        }
    }
    if !item.otherwise_children.is_empty() {
        let mut header = Line::blank();
        header.push(format!("{child_indent}otherwise-children:"), Tone::Muted);
        lines.push(header);
        for child in &item.otherwise_children {
            styled_planned_sequence_item_lines(child, depth + 2, lines);
        }
    }
    if let Some(max_branches) = item.max_branches {
        lines.push(labeled_line(
            &format!("{child_indent}max-branches: "),
            &max_branches.to_string(),
        ));
    }
    if item.concurrent {
        lines.push(labeled_line(&format!("{child_indent}concurrent: "), "true"));
    }
    if !item.parallel_branches.is_empty() {
        let mut header = Line::blank();
        header.push(format!("{child_indent}branches:"), Tone::Muted);
        lines.push(header);
        for branch in &item.parallel_branches {
            lines.push(labeled_line(
                &format!("{child_indent}  - "),
                branch.sequence_ref.as_str(),
            ));
            for child in &branch.children {
                styled_planned_sequence_item_lines(child, depth + 3, lines);
            }
        }
        lines.push(labeled_line(
            &format!("{child_indent}join: "),
            item.join.as_ref().map_or(
                "collect-in-order",
                ctx_traits_core::r#trait::procedure::JoinPolicy::label,
            ),
        ));
        if !item.branch_failure.is_empty() {
            let mut header = Line::blank();
            header.push(format!("{child_indent}branch-failure:"), Tone::Muted);
            lines.push(header);
            for entry in &item.branch_failure {
                lines.push(labeled_line(
                    &format!("{child_indent}  - {}: ", entry.branch),
                    branch_failure_policy_label(entry.on_failure),
                ));
            }
        }
    }
}

fn format_planned_sequence_kind(
    kind: &ctx_traits_core::procedure::run::PlannedSequenceKind,
) -> &'static str {
    match kind {
        ctx_traits_core::procedure::run::PlannedSequenceKind::Prompt => "prompt",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Ask => "ask",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Command => "command",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Check => "check",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Project => "project",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Sequence => "sequence",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Branch => "branch",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Loop => "loop",
        ctx_traits_core::procedure::run::PlannedSequenceKind::ForEach => "for-each",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Parallel => "parallel",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Terminal => "terminal",
    }
}

fn format_sequence_item_status(
    status: &ctx_traits_core::procedure::run::SequenceItemStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::run::SequenceItemStatus::Planned => "planned",
        ctx_traits_core::procedure::run::SequenceItemStatus::Blocked => "blocked",
        ctx_traits_core::procedure::run::SequenceItemStatus::DependencyPending => {
            "dependency-pending"
        }
    }
}

fn format_prompt_source(
    source: &ctx_traits_core::procedure::run::PlannedPromptSource,
) -> &'static str {
    match source {
        ctx_traits_core::procedure::run::PlannedPromptSource::Inline => "inline",
        ctx_traits_core::procedure::run::PlannedPromptSource::LocalPromptRef => "local-prompt-ref",
        ctx_traits_core::procedure::run::PlannedPromptSource::DependencyPendingPromptRef => {
            "dependency-pending-prompt-ref"
        }
    }
}

fn format_slot_state(state: &ctx_traits_core::procedure::run::SlotState) -> &'static str {
    match state {
        ctx_traits_core::procedure::run::SlotState::Required => "required",
        ctx_traits_core::procedure::run::SlotState::PlannedProduced => "planned-produced",
        ctx_traits_core::procedure::run::SlotState::Missing => "missing",
    }
}

fn format_port_requirement_status(
    status: &ctx_traits_core::procedure::run::PortRequirementStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::run::PortRequirementStatus::RuntimeProvided => {
            "runtime-provided"
        }
        ctx_traits_core::procedure::run::PortRequirementStatus::BindingRequired => {
            "binding-required"
        }
        ctx_traits_core::procedure::run::PortRequirementStatus::DependencyPending => {
            "dependency-pending"
        }
    }
}

fn format_planned_output_port_status(
    status: &ctx_traits_core::procedure::run::OutputPortStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::run::OutputPortStatus::PlannedProduced => "planned-produced",
        ctx_traits_core::procedure::run::OutputPortStatus::Missing => "missing",
        ctx_traits_core::procedure::run::OutputPortStatus::OptionalMissing => "optional-missing",
    }
}

/// Shared resource-plan report, reused by inspect and render.
pub(crate) fn emit_plain_resource_plan(
    plan: &ctx_traits_core::resource_plan::Plan,
) -> crate::Result<()> {
    use ctx_traits_core::resource_plan::InclusionReason;

    if plan.entries.is_empty() {
        w("resources: none")?;
        return Ok(());
    }

    w("resources:")?;
    for entry in &plan.entries {
        match (entry.path.as_deref(), entry.dependency_availability) {
            (Some(path), _) => {
                w(format!("  {} (path={}):", entry.resource_id, path))?;
            }
            // A qualified dependency resource with a declared path that was
            // resolved but found unsafe to open — never mislabel it as
            // inline content; report the exact unavailable reason instead.
            (None, Some(status)) => {
                w(format!(
                    "  {} (path=unavailable: {}):",
                    entry.resource_id,
                    ctx_traits_core::resource_plan::dependency_unavailable_reason(status)
                ))?;
            }
            (None, None) => {
                w(format!("  {} (source=inline-content):", entry.resource_id))?;
            }
        }
        w(format!("    trigger: {trigger:?}", trigger = entry.trigger))?;
        let reason_text = match &entry.reason {
            InclusionReason::ActivationTriggered => "activation-triggered".to_string(),
            InclusionReason::ProcedureInput { sequence_index } => {
                format!("procedure-input(sequence[{sequence_index}])")
            }
            InclusionReason::ConditionalInput {
                sequence_index,
                guard,
            } => {
                format!(
                    "conditional-input(sequence[{sequence_index}], candidate only — runtime includes this resource only when guard {} matches at readiness)",
                    serde_json::to_string(guard).unwrap_or_default()
                )
            }
            InclusionReason::OutputPortSchema { port_id, schema_id } => {
                format!("output-port-schema(port={port_id}, schema={schema_id})")
            }
            InclusionReason::SchemaBacking { schema_id } => {
                format!("schema-backing({schema_id})")
            }
        };
        w(format!("    reason: {reason_text}"))?;
        match &entry.digest_evidence {
            Some(ev) => {
                w(format!("    digest: {digest}", digest = ev.digest))?;
                w(format!("    byte-size: {size}", size = ev.byte_size))?;
                if ev.is_binary {
                    w("    binary: true")?;
                }
            }
            None => {
                w("    digest: none")?;
            }
        }
    }

    if !plan.warnings.is_empty() {
        w("resource-warnings:")?;
        for warning in &plan.warnings {
            w(format!("  {warning:?}"))?;
        }
    }
    Ok(())
}

pub(crate) fn styled_resource_plan_lines(
    plan: &ctx_traits_core::resource_plan::Plan,
    lines: &mut Vec<Line>,
) {
    use ctx_traits_core::resource_plan::InclusionReason;

    if plan.entries.is_empty() {
        let mut line = Line::blank();
        line.push("resources: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
        return;
    }

    let mut header = Line::blank();
    header.push("resources:", Tone::Muted);
    lines.push(header);
    for entry in &plan.entries {
        let mut entry_line = Line::blank();
        match (entry.path.as_deref(), entry.dependency_availability) {
            (Some(path), _) => {
                entry_line.push("  ", Tone::Muted);
                entry_line.push(entry.resource_id.clone(), Tone::Default);
                entry_line.push(format!(" (path={path}):"), Tone::Muted);
            }
            // A qualified dependency resource with a declared path that was
            // resolved but found unsafe to open — never mislabel it as
            // inline content; report the exact unavailable reason instead.
            (None, Some(status)) => {
                entry_line.push("  ", Tone::Muted);
                entry_line.push(entry.resource_id.clone(), Tone::Default);
                entry_line.push(
                    format!(
                        " (path=unavailable: {}):",
                        ctx_traits_core::resource_plan::dependency_unavailable_reason(status)
                    ),
                    Tone::Muted,
                );
            }
            (None, None) => {
                entry_line.push("  ", Tone::Muted);
                entry_line.push(entry.resource_id.clone(), Tone::Default);
                entry_line.push(" (source=inline-content):", Tone::Muted);
            }
        }
        lines.push(entry_line);
        lines.push(labeled_line(
            "    trigger: ",
            &format!("{:?}", entry.trigger),
        ));
        let reason_text = match &entry.reason {
            InclusionReason::ActivationTriggered => "activation-triggered".to_string(),
            InclusionReason::ProcedureInput { sequence_index } => {
                format!("procedure-input(sequence[{sequence_index}])")
            }
            InclusionReason::ConditionalInput {
                sequence_index,
                guard,
            } => {
                format!(
                    "conditional-input(sequence[{sequence_index}], candidate only — runtime includes this resource only when guard {} matches at readiness)",
                    serde_json::to_string(guard).unwrap_or_default()
                )
            }
            InclusionReason::OutputPortSchema { port_id, schema_id } => {
                format!("output-port-schema(port={port_id}, schema={schema_id})")
            }
            InclusionReason::SchemaBacking { schema_id } => {
                format!("schema-backing({schema_id})")
            }
        };
        lines.push(labeled_line("    reason: ", &reason_text));
        match &entry.digest_evidence {
            Some(ev) => {
                lines.push(labeled_line("    digest: ", &ev.digest.to_string()));
                lines.push(labeled_line("    byte-size: ", &ev.byte_size.to_string()));
                if ev.is_binary {
                    let mut binary_line = Line::blank();
                    binary_line.push("    binary: true", Tone::Muted);
                    lines.push(binary_line);
                }
            }
            None => {
                lines.push(labeled_line("    digest: ", "none"));
            }
        }
    }

    if !plan.warnings.is_empty() {
        let mut header = Line::blank();
        header.push("resource-warnings:", Tone::Muted);
        lines.push(header);
        for warning in &plan.warnings {
            let mut line = Line::blank();
            line.push("  ", Tone::Muted);
            line.push(format!("{warning:?}"), Tone::Warn);
            lines.push(line);
        }
    }
}

fn emit_plain_resource_read_warnings(
    warnings: &[ctx_traits_io::resource::ResourceReadWarning],
) -> crate::Result<()> {
    use ctx_traits_io::resource::ResourceReadWarning;

    if warnings.is_empty() {
        w("resource-read-warnings: none")?;
        return Ok(());
    }

    w("resource-read-warnings:")?;
    for warning in warnings {
        match warning {
            ResourceReadWarning::MissingFile { resource_id, path } => {
                w(format!("  missing-file: {resource_id} (path={path})"))?;
            }
            ResourceReadWarning::SymlinkDetected { resource_id, path } => {
                w(format!("  symlink-detected: {resource_id} (path={path})"))?;
            }
            ResourceReadWarning::SpecialFile { resource_id, path } => {
                w(format!("  special-file: {resource_id} (path={path})"))?;
            }
            ResourceReadWarning::Directory { resource_id, path } => {
                w(format!("  directory: {resource_id} (path={path})"))?;
            }
            ResourceReadWarning::BinaryContent {
                resource_id,
                path,
                byte_size,
            } => {
                w(format!(
                    "  binary-content: {resource_id} (path={path}, {byte_size} bytes)"
                ))?;
            }
        }
    }
    Ok(())
}

fn styled_resource_read_warnings_lines(
    warnings: &[ctx_traits_io::resource::ResourceReadWarning],
    lines: &mut Vec<Line>,
) {
    use ctx_traits_io::resource::ResourceReadWarning;

    if warnings.is_empty() {
        let mut line = Line::blank();
        line.push("resource-read-warnings: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
        return;
    }

    let mut header = Line::blank();
    header.push("resource-read-warnings:", Tone::Muted);
    lines.push(header);
    for warning in warnings {
        let (kind, resource_id, path, extra) = match warning {
            ResourceReadWarning::MissingFile { resource_id, path } => {
                ("missing-file", resource_id, path.clone(), None)
            }
            ResourceReadWarning::SymlinkDetected { resource_id, path } => {
                ("symlink-detected", resource_id, path.clone(), None)
            }
            ResourceReadWarning::SpecialFile { resource_id, path } => {
                ("special-file", resource_id, path.clone(), None)
            }
            ResourceReadWarning::Directory { resource_id, path } => {
                ("directory", resource_id, path.clone(), None)
            }
            ResourceReadWarning::BinaryContent {
                resource_id,
                path,
                byte_size,
            } => (
                "binary-content",
                resource_id,
                path.clone(),
                Some(format!("{byte_size} bytes")),
            ),
        };
        let mut line = Line::blank();
        line.push("  ", Tone::Muted);
        line.push(kind, Tone::Warn);
        line.push(": ", Tone::Muted);
        line.push(resource_id.clone(), Tone::Default);
        match extra {
            Some(extra) => line.push(format!(" (path={path}, {extra})"), Tone::Muted),
            None => line.push(format!(" (path={path})"), Tone::Muted),
        }
        lines.push(line);
    }
}

fn emit_plain_scenarios(trait_ref: &ctx_traits_core::Trait) -> crate::Result<()> {
    use ctx_traits_core::r#trait::ScenarioVariant;

    if trait_ref.scenarios.is_empty() {
        w("scenarios: none")?;
        return Ok(());
    }

    w("scenarios:")?;
    for s in &trait_ref.scenarios {
        let variant_str = match s.variant {
            ScenarioVariant::Positive => "positive",
            ScenarioVariant::Negative => "negative",
            ScenarioVariant::Edge => "edge",
        };
        w(format!(
            "  {id} (variant={variant}):",
            id = s.id,
            variant = variant_str
        ))?;
        if let Some(ref input) = s.input {
            w(format!("    input: {input}"))?;
        }
        if let Some(ref output) = s.output {
            w(format!("    output: {output}"))?;
        }
        if !s.tags.is_empty() {
            let tags: Vec<String> = s.tags.iter().map(ToString::to_string).collect();
            w(format!("    tags: {}", tags.join(", ")))?;
        }
    }

    let audit = ctx_traits_core::r#trait::scenario::audit_scenarios(&trait_ref.scenarios);
    if audit.is_empty() {
        w("scenario-audit: none")?;
    } else {
        w("scenario-audit:")?;
        for warning in &audit {
            let kind = match warning.kind {
                ctx_traits_core::r#trait::ScenarioAuditKind::MissingOutput => "missing-output",
                ctx_traits_core::r#trait::ScenarioAuditKind::MissingInput => "missing-input",
            };
            w(format!(
                "  {id} ({kind}): {msg}",
                id = warning.scenario_id,
                kind = kind,
                msg = warning.message
            ))?;
        }
    }
    Ok(())
}

fn styled_scenarios_lines(trait_ref: &ctx_traits_core::Trait, lines: &mut Vec<Line>) {
    use ctx_traits_core::r#trait::ScenarioVariant;

    if trait_ref.scenarios.is_empty() {
        let mut line = Line::blank();
        line.push("scenarios: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
        return;
    }

    let mut header = Line::blank();
    header.push("scenarios:", Tone::Muted);
    lines.push(header);
    for s in &trait_ref.scenarios {
        let variant_str = match s.variant {
            ScenarioVariant::Positive => "positive",
            ScenarioVariant::Negative => "negative",
            ScenarioVariant::Edge => "edge",
        };
        let mut line = Line::blank();
        line.push("  ", Tone::Muted);
        line.push(s.id.clone(), Tone::Default);
        line.push(format!(" (variant={variant_str}):"), Tone::Muted);
        lines.push(line);
        if let Some(ref input) = s.input {
            lines.push(labeled_line("    input: ", input));
        }
        if let Some(ref output) = s.output {
            lines.push(labeled_line("    output: ", output));
        }
        if !s.tags.is_empty() {
            let tags: Vec<String> = s.tags.iter().map(ToString::to_string).collect();
            lines.push(labeled_line("    tags: ", &tags.join(", ")));
        }
    }

    let audit = ctx_traits_core::r#trait::scenario::audit_scenarios(&trait_ref.scenarios);
    if audit.is_empty() {
        let mut line = Line::blank();
        line.push("scenario-audit: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
    } else {
        let mut header = Line::blank();
        header.push("scenario-audit:", Tone::Muted);
        lines.push(header);
        for warning in &audit {
            let kind = match warning.kind {
                ctx_traits_core::r#trait::ScenarioAuditKind::MissingOutput => "missing-output",
                ctx_traits_core::r#trait::ScenarioAuditKind::MissingInput => "missing-input",
            };
            let mut line = Line::blank();
            line.push("  ", Tone::Muted);
            line.push(warning.scenario_id.clone(), Tone::Default);
            line.push(" (", Tone::Muted);
            line.push(kind, Tone::Warn);
            line.push("): ", Tone::Muted);
            line.push(warning.message.clone(), Tone::Default);
            lines.push(line);
        }
    }
}

fn emit_plain_evals(trait_ref: &ctx_traits_core::Trait) -> crate::Result<()> {
    use ctx_traits_core::r#trait::EvalVariant;

    if trait_ref.evals.is_empty() {
        w("evals: none")?;
        return Ok(());
    }

    w("evals:")?;
    for e in &trait_ref.evals {
        let variant_str = match e.variant {
            EvalVariant::Documentation => "documentation",
            EvalVariant::Lint => "lint",
            EvalVariant::GoldenRender => "golden-render",
            EvalVariant::Behavioral => "behavioral",
            EvalVariant::Runtime => "runtime",
        };
        let mvp = if e.variant.is_mvp_supported() {
            ""
        } else {
            " (unsupported)"
        };
        w(format!(
            "  {id} (variant={variant}{mvp}):",
            id = e.id,
            variant = variant_str,
            mvp = mvp
        ))?;
        if let Some(ref input) = e.input {
            w(format!("    input: {input}"))?;
        }
        if let Some(ref output) = e.output {
            w(format!("    output: {output}"))?;
        }
        if !e.scenarios.is_empty() {
            w(format!("    scenarios: {}", e.scenarios.join(", ")))?;
        }
    }

    let scenario_ids: std::collections::BTreeSet<&str> =
        trait_ref.scenarios.iter().map(|s| s.id.as_str()).collect();
    let audit = ctx_traits_core::r#trait::eval::audit_evals(&trait_ref.evals, &scenario_ids);
    if audit.is_empty() {
        w("eval-audit: none")?;
    } else {
        w("eval-audit:")?;
        for warning in &audit {
            let kind = match warning.kind {
                ctx_traits_core::r#trait::EvalAuditKind::UnsupportedVariant => {
                    "unsupported-variant"
                }
                ctx_traits_core::r#trait::EvalAuditKind::UnresolvedScenarioRef => {
                    "unresolved-scenario-ref"
                }
            };
            w(format!(
                "  {id} ({kind}): {msg}",
                id = warning.eval_id,
                kind = kind,
                msg = warning.message
            ))?;
        }
    }
    Ok(())
}

fn styled_evals_lines(trait_ref: &ctx_traits_core::Trait, lines: &mut Vec<Line>) {
    use ctx_traits_core::r#trait::EvalVariant;

    if trait_ref.evals.is_empty() {
        let mut line = Line::blank();
        line.push("evals: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
        return;
    }

    let mut header = Line::blank();
    header.push("evals:", Tone::Muted);
    lines.push(header);
    for e in &trait_ref.evals {
        let variant_str = match e.variant {
            EvalVariant::Documentation => "documentation",
            EvalVariant::Lint => "lint",
            EvalVariant::GoldenRender => "golden-render",
            EvalVariant::Behavioral => "behavioral",
            EvalVariant::Runtime => "runtime",
        };
        let mvp = if e.variant.is_mvp_supported() {
            ""
        } else {
            " (unsupported)"
        };
        let mut line = Line::blank();
        line.push("  ", Tone::Muted);
        line.push(e.id.clone(), Tone::Default);
        line.push(format!(" (variant={variant_str}{mvp}):"), Tone::Muted);
        lines.push(line);
        if let Some(ref input) = e.input {
            lines.push(labeled_line("    input: ", input));
        }
        if let Some(ref output) = e.output {
            lines.push(labeled_line("    output: ", output));
        }
        if !e.scenarios.is_empty() {
            lines.push(labeled_line("    scenarios: ", &e.scenarios.join(", ")));
        }
    }

    let scenario_ids: std::collections::BTreeSet<&str> =
        trait_ref.scenarios.iter().map(|s| s.id.as_str()).collect();
    let audit = ctx_traits_core::r#trait::eval::audit_evals(&trait_ref.evals, &scenario_ids);
    if audit.is_empty() {
        let mut line = Line::blank();
        line.push("eval-audit: ", Tone::Muted);
        line.push("none", Tone::Default);
        lines.push(line);
    } else {
        let mut header = Line::blank();
        header.push("eval-audit:", Tone::Muted);
        lines.push(header);
        for warning in &audit {
            let kind = match warning.kind {
                ctx_traits_core::r#trait::EvalAuditKind::UnsupportedVariant => {
                    "unsupported-variant"
                }
                ctx_traits_core::r#trait::EvalAuditKind::UnresolvedScenarioRef => {
                    "unresolved-scenario-ref"
                }
            };
            let mut line = Line::blank();
            line.push("  ", Tone::Muted);
            line.push(warning.eval_id.clone(), Tone::Default);
            line.push(" (", Tone::Muted);
            line.push(kind, Tone::Warn);
            line.push("): ", Tone::Muted);
            line.push(warning.message.clone(), Tone::Default);
            lines.push(line);
        }
    }
}
