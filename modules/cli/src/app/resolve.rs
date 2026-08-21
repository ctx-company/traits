//! Resolve command handler.

use crate::app::command_handlers::resolve_activation;
use ctx_traits_core::response::CommandOutput;

pub(crate) struct ResolveInputs<'a> {
    pub(crate) task: &'a str,
    pub(crate) trait_files: &'a [String],
    pub(crate) repo_root: Option<&'a str>,
    pub(crate) files: &'a [String],
    pub(crate) mode: Option<&'a str>,
    pub(crate) languages: &'a [String],
    pub(crate) budget: Option<u64>,
    pub(crate) session: Option<&'a str>,
    pub(crate) explicit_invocation: Option<&'a str>,
    pub(crate) trait_id: Option<&'a str>,
    pub(crate) json: bool,
}

pub(crate) fn handle_resolve(input: ResolveInputs<'_>) -> crate::Result<CommandOutput<()>> {
    let request = ctx_traits_core::resolve::Request {
        task: input.task.to_string(),
        files: input.files.to_vec(),
        mode: input.mode.map(|s| s.to_string()),
        language_hints: input.languages.to_vec(),
        budget_tokens: input.budget,
        session_hint: input.session.map(|s| s.to_string()),
        explicit_invocation: input.explicit_invocation.map(|s| s.to_string()),
        trait_id: input.trait_id.map(|s| s.to_string()),
    };

    let (inventory, mut response) = resolve_activation(
        input.trait_files,
        input.repo_root,
        input.mode,
        input.languages,
        input.trait_id,
        &request,
    )?;

    // P498 (f): carry each candidate's source digest along so a non-ledger
    // adapter can skip a round trip through `ctx traits internal prompt` just to
    // learn it. Additive-optional field, populated from the already-loaded
    // inventory — never from a fresh render, which would make this cheap
    // planning verb expensive (see `context plan`, which renders anyway and
    // is the only verb that ever populates `model-view-digest`).
    let source_digests: std::collections::BTreeMap<&str, &str> = inventory
        .loaded
        .iter()
        .map(|l| (l.trait_ref.id.as_str(), l.source_digest.as_str()))
        .collect();
    for candidate in response.selected.iter_mut().chain(&mut response.rejected) {
        candidate.source_digest = source_digests
            .get(candidate.trait_id.as_str())
            .map(|digest| ctx_traits_core::digest::Digest::parse(digest))
            .transpose()?;
    }

    if input.json {
        let json_text = serde_json::to_string_pretty(&response)
            .map_err(|e| crate::Error::json("serialize resolve response", e))?;
        println!("{json_text}");
    } else {
        println!("ctx traits internal resolve");
        println!("  task: {}", input.task);
        if let Some(session) = input.session {
            println!("  session: {session}");
        }
        if let Some(budget) = input.budget {
            println!("  budget: {budget}");
        }
        println!("  selected: {}", response.selected.len());
        for candidate in &response.selected {
            println!(
                "    {} ({}) load={} tokens={} decision={} estimate={}",
                candidate.trait_id,
                candidate.version,
                candidate.load_level,
                candidate.estimated_tokens,
                candidate.budget_decision,
                candidate.estimate_source,
            );
        }
        println!("  rejected: {}", response.rejected.len());
        for candidate in &response.rejected {
            println!(
                "    {} ({}) active={} decision={} estimate={} reason={}",
                candidate.trait_id,
                candidate.version,
                candidate.active,
                candidate.budget_decision,
                candidate.estimate_source,
                candidate.reason_codes.join(", ")
            );
            if !candidate.remedies.is_empty() {
                println!("      remedy: {}", candidate.remedies.join(", "));
            }
        }
        if !response.index_rejections.is_empty() {
            println!("  index-rejections: {}", response.index_rejections.len());
            for rej in &response.index_rejections {
                println!(
                    "    {} reason={}",
                    rej.trait_id,
                    rej.reason_codes.join(", ")
                );
                if !rej.remedies.is_empty() {
                    println!("      remedy: {}", rej.remedies.join(", "));
                }
            }
        }
        println!(
            "  budget: used={}, remaining={}, exceeded={}",
            response.budget.used,
            response.budget.remaining.unwrap_or(0),
            response.budget.exceeded
        );
        let rel = &response.relation_evidence;
        println!(
            "  relations: depth-used={}, depth-allowed={}, required={}, suggested={}, conflicts={}, cycles={}, unresolved={}, changed-status={}",
            rel.expansion_depth_used,
            rel.expansion_depth_allowed,
            rel.required_edges.len(),
            rel.suggested_edges.len(),
            rel.conflict_edges.len(),
            rel.cycles.len(),
            rel.unresolved_targets.len(),
            rel.changed_status,
        );
        if !rel.support_note.is_empty() {
            println!("    support: {}", rel.support_note);
        }
        if !rel.required_edges.is_empty() {
            println!("    required-edges: {}", rel.required_edges.join(", "));
        }
        if !rel.suggested_edges.is_empty() {
            println!("    suggested-edges: {}", rel.suggested_edges.join(", "));
        }
        if !rel.conflict_edges.is_empty() {
            println!("    conflict-edges: {}", rel.conflict_edges.join(", "));
        }
        if !rel.unresolved_targets.is_empty() {
            println!(
                "    unresolved-targets: {}",
                rel.unresolved_targets.join(", ")
            );
        }
    }

    Ok(CommandOutput::new(()))
}

pub(crate) use handle_resolve as handle;
