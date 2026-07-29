//! Launch readiness and compatibility reports.

use ctx_traits_core::response::CommandOutput;

use crate::app::command_handlers::print_json_report;
use crate::app::tui::{Line, Tone, labeled_line, write_plain_line as w};

fn parse_extended_profile(
    profile: &str,
) -> crate::Result<ctx_traits_core::render::ExtendedRenderProfile> {
    ctx_traits_core::render::ExtendedRenderProfile::parse(profile).ok_or_else(|| {
        crate::Error::Command {
            message: format!(
                "unsupported profile: {profile:?} (expected agent-skills, pi, opencode, claude-code, codex, copilot, or markdown-only)"
            ),
        }
    })
}

fn load_trait_with_source(file: &str) -> crate::Result<(ctx_traits_core::Trait, String)> {
    let path = camino::Utf8Path::new(file);
    let encoding = ctx_traits_core::encoding::Encoding::from_path(path)?;
    let text = ctx_traits_io::read::read_text(path)?;
    let (trait_ref, decode_warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &text)?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(file, &decode_warnings);
    Ok((trait_ref, text))
}

/// Resolve `(package status, trust verdict)` for a bare loaded trait file.
///
/// The canonical document carries neither field: status comes from the
/// package's `trait.toml` `[package].status`, trust from the machine trust
/// store keyed by canonical digest. A file with no discoverable package
/// root falls back to the same conservative defaults as an external import
/// (`draft`, `unreviewed`).
fn resolve_lifecycle_for_file(
    file: &str,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<(
    ctx_traits_core::manifest::PackageStatus,
    ctx_traits_core::r#trait::TrustVerdict,
)> {
    let path = camino::Utf8Path::new(file);
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

pub(crate) fn handle_hygiene(
    trait_files: &[String],
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let traits = crate::app::report_handlers::load_trait_files(trait_files)?;
    let lifecycle = trait_files
        .iter()
        .zip(traits.iter())
        .map(|(file, trait_ref)| resolve_lifecycle_for_file(file, trait_ref))
        .collect::<crate::Result<Vec<_>>>()?;
    let report = ctx_traits_core::launch::hygiene_report(&traits, &lifecycle);
    if json {
        print_json_report(&report, "hygiene report")?;
    } else {
        crate::app::tui::emit_report(
            false,
            || styled_hygiene_lines(&report, trait_files),
            || emit_plain_hygiene(&report),
        )?;
    }
    Ok(CommandOutput::new(()))
}

fn emit_plain_hygiene(report: &ctx_traits_core::launch::TraitHygieneReport) -> crate::Result<()> {
    w("ctx traits hygiene")?;
    for entry in &report.traits {
        w(format!("  - {}: {}", entry.trait_id, entry.action))?;
        for finding in &entry.findings {
            w(format!(
                "      {} [{}]: {}",
                finding.code, finding.severity, finding.message
            ))?;
        }
    }
    w("  inventory:")?;
    for item in &report.inventory {
        w(format!("    - {}", item.trait_id))?;
        w(format!("        why-this-exists: {}", item.why_this_exists))?;
        w(format!(
            "        when-it-should-trigger: {}",
            item.when_it_should_trigger
        ))?;
        w(format!(
            "        conflicts-with: {}",
            display_list(&item.conflicts_with)
        ))?;
        w(format!("        last-reviewed: {}", item.last_reviewed))?;
        w(format!(
            "        render-targets: {}",
            display_list(&item.render_targets)
        ))?;
    }
    w("  prune-plan:")?;
    if report.prune_plan.is_empty() {
        w("    none")?;
    } else {
        for item in &report.prune_plan {
            w(format!("    - {}: {}", item.trait_id, item.recommendation))?;
            w(format!("        reason: {}", item.reason))?;
            w(format!(
                "        replacement-suggestions: {}",
                display_list(&item.replacement_suggestions)
            ))?;
            w(format!(
                "        render-export-impact: {}",
                item.render_export_impact
            ))?;
            w(format!("        requires-review: {}", item.requires_review))?;
        }
    }
    Ok(())
}

fn styled_hygiene_lines(
    report: &ctx_traits_core::launch::TraitHygieneReport,
    trait_files: &[String],
) -> Vec<Line> {
    let mut lines = Vec::new();
    let command = trait_files
        .iter()
        .map(|file| format!("--file {file}"))
        .fold("ctx traits hygiene".to_string(), |mut command, arg| {
            command.push(' ');
            command.push_str(&arg);
            command
        });
    lines.push(crate::app::tui::command_line(command));
    lines.push(Line::blank());

    for entry in &report.traits {
        let mut line = Line::blank();
        line.push("- ", Tone::Muted);
        line.push(entry.trait_id.clone(), Tone::Default);
        line.push(": ", Tone::Muted);
        line.push(entry.action.clone(), Tone::Default);
        lines.push(line);
        for finding in &entry.findings {
            let severity_tone = if finding.severity == "warning" {
                Tone::Warn
            } else {
                Tone::Default
            };
            let mut line = Line::blank();
            line.push("    ", Tone::Default);
            line.push(finding.code.clone(), Tone::Muted);
            line.push(" [", Tone::Muted);
            line.push(finding.severity.clone(), severity_tone);
            line.push("]: ", Tone::Muted);
            line.push(finding.message.clone(), Tone::Default);
            lines.push(line);
        }
    }

    lines.push(Line::blank());
    let mut inventory_header = Line::blank();
    inventory_header.push("inventory:", Tone::Muted);
    lines.push(inventory_header);
    for item in &report.inventory {
        let mut line = Line::blank();
        line.push("  - ", Tone::Muted);
        line.push(item.trait_id.clone(), Tone::Default);
        lines.push(line);
        lines.push(labeled_line(
            "      why-this-exists: ",
            &item.why_this_exists,
        ));
        lines.push(labeled_line(
            "      when-it-should-trigger: ",
            &item.when_it_should_trigger,
        ));
        lines.push(labeled_line(
            "      conflicts-with: ",
            &display_list(&item.conflicts_with),
        ));
        lines.push(labeled_line("      last-reviewed: ", &item.last_reviewed));
        lines.push(labeled_line(
            "      render-targets: ",
            &display_list(&item.render_targets),
        ));
    }

    lines.push(Line::blank());
    let mut prune_header = Line::blank();
    prune_header.push("prune-plan:", Tone::Muted);
    lines.push(prune_header);
    if report.prune_plan.is_empty() {
        let mut line = Line::blank();
        line.push("  none", Tone::Muted);
        lines.push(line);
    } else {
        for item in &report.prune_plan {
            let mut line = Line::blank();
            line.push("  - ", Tone::Muted);
            line.push(item.trait_id.clone(), Tone::Default);
            line.push(": ", Tone::Muted);
            line.push(item.recommendation.clone(), Tone::Default);
            lines.push(line);
            lines.push(labeled_line("      reason: ", &item.reason));
            lines.push(labeled_line(
                "      replacement-suggestions: ",
                &display_list(&item.replacement_suggestions),
            ));
            lines.push(labeled_line(
                "      render-export-impact: ",
                &item.render_export_impact,
            ));
            lines.push(labeled_line(
                "      requires-review: ",
                &item.requires_review.to_string(),
            ));
        }
    }
    lines
}

pub(crate) fn handle_cost(
    file: &str,
    budget: Option<u64>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, _) = load_trait_with_source(file)?;
    let report = ctx_traits_core::launch::context_cost_report(&trait_ref, budget);
    if json {
        print_json_report(&report, "context cost report")?;
    } else {
        crate::app::tui::emit_report(
            false,
            || styled_cost_lines(&report, file, budget),
            || emit_plain_cost(&report),
        )?;
    }
    Ok(CommandOutput::new(()))
}

fn opt_display<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or("none".to_string(), |v| v.to_string())
}

fn emit_plain_cost(report: &ctx_traits_core::launch::ContextCostReport) -> crate::Result<()> {
    w("ctx traits cost")?;
    w(format!("  trait: {}", report.trait_id))?;
    w(format!("  tokenizer: {}", report.tokenizer))?;
    w(format!(
        "  total-estimated-tokens: {}",
        report.total_estimated_tokens
    ))?;
    w(format!("  budget: {}", opt_display(report.budget)))?;
    w(format!(
        "  budget-remaining: {}",
        opt_display(report.budget_remaining)
    ))?;
    w(format!(
        "  over-budget-by: {}",
        opt_display(report.over_budget_by)
    ))?;
    w(format!("  budget-status: {}", report.budget_status))?;
    for warning in &report.warnings {
        w(format!(
            "  warning {}: {} [{}]",
            warning.code,
            warning.message,
            display_list(&warning.items)
        ))?;
    }
    for layer in &report.layers {
        let mut line = format!(
            "  - {} {}: {} token(s), selected={}",
            layer.layer, layer.item, layer.estimated_tokens, layer.selected,
        );
        if let Some(required) = layer.required {
            line.push_str(&format!(", required={required}"));
        }
        w(line)?;
        if let Some(reason) = &layer.skip_reason {
            w(format!("      skip-reason: {reason}"))?;
        }
    }
    Ok(())
}

fn budget_status_tone(status: &str) -> Tone {
    match status {
        "exceeded" => Tone::Fail,
        "within-budget" => Tone::Pass,
        _ => Tone::Default,
    }
}

fn styled_cost_lines(
    report: &ctx_traits_core::launch::ContextCostReport,
    file: &str,
    budget: Option<u64>,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut command = format!("ctx traits cost --file {file}");
    if let Some(budget) = budget {
        command.push_str(&format!(" --budget {budget}"));
    }
    lines.push(crate::app::tui::command_line(command));
    lines.push(Line::blank());

    lines.push(labeled_line("trait: ", &report.trait_id));
    lines.push(labeled_line("tokenizer: ", &report.tokenizer));
    lines.push(labeled_line(
        "total-estimated-tokens: ",
        &report.total_estimated_tokens.to_string(),
    ));
    lines.push(labeled_line("budget: ", &opt_display(report.budget)));
    lines.push(labeled_line(
        "budget-remaining: ",
        &opt_display(report.budget_remaining),
    ));
    lines.push(labeled_line(
        "over-budget-by: ",
        &opt_display(report.over_budget_by),
    ));
    let mut status_line = Line::blank();
    status_line.push("budget-status: ", Tone::Muted);
    status_line.push(
        report.budget_status.clone(),
        budget_status_tone(&report.budget_status),
    );
    lines.push(status_line);

    for warning in &report.warnings {
        lines.push(labeled_line(
            &format!("warning {}: ", warning.code),
            &format!("{} [{}]", warning.message, display_list(&warning.items)),
        ));
    }
    for layer in &report.layers {
        let mut line = Line::blank();
        line.push("- ", Tone::Muted);
        line.push(format!("{} {}", layer.layer, layer.item), Tone::Default);
        line.push(": ", Tone::Muted);
        line.push(
            format!("{} token(s)", layer.estimated_tokens),
            Tone::Default,
        );
        line.push(", selected=", Tone::Muted);
        line.push(layer.selected.to_string(), Tone::Default);
        if let Some(required) = layer.required {
            line.push(", required=", Tone::Muted);
            line.push(required.to_string(), Tone::Default);
        }
        lines.push(line);
        if let Some(reason) = &layer.skip_reason {
            lines.push(labeled_line("    skip-reason: ", reason));
        }
    }
    lines
}

pub(crate) fn handle_prepare_public(file: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, _) = load_trait_with_source(file)?;
    let report = ctx_traits_core::launch::publish_prep_report(&trait_ref);
    if json {
        print_json_report(&report, "publish prep report")?;
    } else {
        println!("ctx traits prepare-public");
        println!("  trait: {}", report.trait_id);
        println!("  requires-human-review: {}", report.requires_human_review);
        for finding in &report.findings {
            println!(
                "  - {} {}: {}",
                finding.field, finding.kind, finding.message
            );
        }
        for entry in &report.packaging_plan {
            println!("  package {}: {}", entry.part, entry.recommendation);
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_context_contracts(file: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, _) = load_trait_with_source(file)?;
    let report = ctx_traits_core::launch::context_contract_report(&trait_ref);
    if json {
        print_json_report(&report, "context contract report")?;
    } else {
        println!("ctx traits context-contracts");
        println!("  trait: {}", report.trait_id);
        for layer in &report.layers {
            println!(
                "  - {} -> {}: {}",
                layer.layer, layer.belongs_in, layer.evidence
            );
        }
        for warning in &report.warnings {
            println!("  warning: {warning}");
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_policy(
    file: &str,
    profile: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, _) = load_trait_with_source(file)?;
    let render_profile = parse_extended_profile(profile)?;
    let report = ctx_traits_core::launch::policy_report(&trait_ref, render_profile);
    if json {
        print_json_report(&report, "policy report")?;
    } else {
        crate::app::tui::emit_report(
            false,
            || styled_policy_lines(&report, file, profile),
            || emit_plain_policy(&report),
        )?;
    }
    Ok(CommandOutput::new(()))
}

fn emit_plain_policy(report: &ctx_traits_core::launch::PolicyReport) -> crate::Result<()> {
    w("ctx traits policy")?;
    w(format!("  trait: {}", report.trait_id))?;
    w(format!("  profile: {}", report.profile))?;
    for item in &report.items {
        w(format!(
            "  - {} [{}]: {}",
            item.field, item.class, item.message
        ))?;
    }
    for hook in &report.hook_plan {
        w(format!(
            "  hook {}: capability={}, supported={}, capability-status={}, fallback={}",
            hook.hook,
            hook.required_capability,
            hook.supported,
            hook.capability_status,
            hook.fallback
        ))?;
    }
    Ok(())
}

fn styled_policy_lines(
    report: &ctx_traits_core::launch::PolicyReport,
    file: &str,
    profile: &str,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let command_line = crate::app::tui::command_line(format!(
        "ctx traits policy --file {file} --profile {profile}"
    ));
    lines.push(command_line);
    lines.push(Line::blank());

    lines.push(labeled_line("trait: ", &report.trait_id));
    lines.push(labeled_line("profile: ", &report.profile));
    for item in &report.items {
        let mut line = Line::blank();
        line.push("- ", Tone::Muted);
        line.push(item.field.clone(), Tone::Default);
        line.push(" [", Tone::Muted);
        line.push(item.class.clone(), Tone::Default);
        line.push("]: ", Tone::Muted);
        line.push(item.message.clone(), Tone::Default);
        lines.push(line);
    }
    for hook in &report.hook_plan {
        let mut line = Line::blank();
        line.push("hook ", Tone::Muted);
        line.push(hook.hook.clone(), Tone::Default);
        line.push(": capability=", Tone::Muted);
        line.push(hook.required_capability.clone(), Tone::Default);
        line.push(", supported=", Tone::Muted);
        line.push(
            hook.supported.to_string(),
            if hook.supported {
                Tone::Pass
            } else {
                Tone::Warn
            },
        );
        line.push(", capability-status=", Tone::Muted);
        line.push(hook.capability_status.clone(), Tone::Default);
        line.push(", fallback=", Tone::Muted);
        line.push(hook.fallback.clone(), Tone::Default);
        lines.push(line);
    }
    lines
}

pub(crate) fn handle_evidence(
    file: &str,
    profile: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, source_text) = load_trait_with_source(file)?;
    let (status, trust) = resolve_lifecycle_for_file(file, &trait_ref)?;
    let render_profile = parse_extended_profile(profile)?;
    let report = ctx_traits_core::launch::evidence_bundle(
        &trait_ref,
        &status,
        &trust,
        &source_text,
        render_profile,
    );
    if json {
        print_json_report(&report, "evidence bundle")?;
    } else {
        crate::app::tui::emit_report(
            false,
            || styled_evidence_lines(&report, file, profile),
            || emit_plain_evidence(&report),
        )?;
    }
    Ok(CommandOutput::new(()))
}

fn emit_plain_evidence(report: &ctx_traits_core::launch::EvidenceBundle) -> crate::Result<()> {
    w("ctx traits evidence")?;
    w(format!("  trait: {} {}", report.trait_id, report.version))?;
    w(format!(
        "  lifecycle/trust: {}/{}",
        report.lifecycle, report.trust
    ))?;
    w(format!("  source-digest: {}", report.digests.source))?;
    w(format!("  canonical-digest: {}", report.digests.canonical))?;
    w(format!(
        "  model-view-digest: {}",
        report.digests.model_view
    ))?;
    w(format!("  non-claim: {}", report.non_claim))?;
    w(format!(
        "  scenarios: {}, evals: {}",
        report.scenarios.len(),
        report.evals.len()
    ))?;
    Ok(())
}

fn styled_evidence_lines(
    report: &ctx_traits_core::launch::EvidenceBundle,
    file: &str,
    profile: &str,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let command_line = crate::app::tui::command_line(format!(
        "ctx traits evidence --file {file} --profile {profile}"
    ));
    lines.push(command_line);
    lines.push(Line::blank());

    lines.push(labeled_line(
        "trait: ",
        &format!("{} {}", report.trait_id, report.version),
    ));
    lines.push(labeled_line(
        "lifecycle/trust: ",
        &format!("{}/{}", report.lifecycle, report.trust),
    ));
    lines.push(labeled_line(
        "source-digest: ",
        &report.digests.source.to_string(),
    ));
    lines.push(labeled_line(
        "canonical-digest: ",
        &report.digests.canonical.to_string(),
    ));
    lines.push(labeled_line(
        "model-view-digest: ",
        &report.digests.model_view.to_string(),
    ));
    lines.push(labeled_line("non-claim: ", &report.non_claim));
    lines.push(labeled_line(
        "scenarios/evals: ",
        &format!("{}, {}", report.scenarios.len(), report.evals.len()),
    ));
    lines
}

pub(crate) fn handle_compatibility(json: bool) -> crate::Result<CommandOutput<()>> {
    let report = ctx_traits_core::launch::compatibility_matrix();
    if json {
        print_json_report(&report, "compatibility matrix")?;
    } else {
        println!("ctx traits compatibility");
        for profile in &report.profiles {
            println!(
                "  - {}: {}",
                profile.profile, profile.activation_approximation
            );
            println!("      policy: {}", profile.policy_enforceability);
            if !profile.unsupported_fields.is_empty() {
                println!(
                    "      unsupported: {}",
                    profile.unsupported_fields.join(", ")
                );
            }
        }
    }
    Ok(CommandOutput::new(()))
}

pub(crate) fn handle_subagent(
    file: &str,
    profile: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (trait_ref, _) = load_trait_with_source(file)?;
    let (status, trust) = resolve_lifecycle_for_file(file, &trait_ref)?;
    let render_profile = parse_extended_profile(profile)?;
    let report =
        ctx_traits_core::launch::subagent_report(&trait_ref, &status, &trust, render_profile);
    if json {
        print_json_report(&report, "subagent report")?;
    } else {
        println!("ctx traits subagent");
        println!("  trait: {}", report.trait_id);
        println!("  profile: {}", report.profile);
        println!("  non-claim: {}", report.non_claim);
        for item in &report.declared_intent {
            println!(
                "  declared {} [{}]: {}",
                item.field,
                item.enforceability,
                display_list(&item.values)
            );
        }
        for item in &report.propagation {
            println!(
                "  - {}: propagate={}, enforceability={}, evidence={}",
                item.item, item.should_propagate, item.enforceability, item.evidence
            );
        }
    }
    Ok(CommandOutput::new(()))
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

pub(crate) fn print_lock_update(update: &ctx_traits_io::lockfile::LockUpdateResult) {
    match update {
        ctx_traits_io::lockfile::LockUpdateResult::Updated { path } => {
            println!("  lock-update: updated {path}");
        }
        ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingLock { path } => {
            println!("  lock-update: skipped (missing lockfile {path})");
        }
        ctx_traits_io::lockfile::LockUpdateResult::SkippedMissingEntry { path, trait_id } => {
            println!("  lock-update: skipped (missing trait {trait_id} in {path})");
        }
    }
}
