//! Shared, side-effect-free import analysis helpers.
//!
//! `import` and `doctor` both need to turn a decoded [`ctx_traits_core::Trait`]
//! plus its import evidence into a source-anchored
//! [`ctx_traits_core::scaffold::TraitScaffold`]. This module holds that
//! construction once so neither command duplicates it.
//!
//! [`analyze_import_source`] goes one step further: it is the single
//! deterministic, side-effect-free (no writes, no trust/lifecycle decisions,
//! no network, no model calls) pipeline from a `--source` path to a planned
//! trait, import report, and scaffold. `ctx traits import` calls it and then
//! continues into its own write/lifecycle/candidate logic; `ctx traits
//! doctor` calls it and only ever reads the result. Both commands see
//! byte-identical trait IDs, digests, declarations, and warnings for the
//! same source because they run the same code, not two hand-maintained
//! copies of it.

/// Coarse whole-file, frontmatter, and body source anchors for a raw imported
/// text. Per-declaration semantic anchors are deferred to Group 46.
pub(crate) struct ImportSourceAnchors {
    pub(crate) whole_file: ctx_traits_core::source_map::SourceAnchor,
    pub(crate) frontmatter: ctx_traits_core::source_map::SourceAnchor,
    pub(crate) body: ctx_traits_core::source_map::SourceAnchor,
}

/// Derive coarse whole-file, frontmatter, and body spans for deterministic
/// imports. Per-declaration semantic anchors are deferred to Group 46.
pub(crate) fn source_anchors_for_text(path: &camino::Utf8Path, text: &str) -> ImportSourceAnchors {
    let line_count = text.lines().count().max(1);
    let frontmatter_end = text
        .lines()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim_end_matches('\r') == "---").then_some(index + 1))
        .filter(|_| {
            text.lines()
                .next()
                .is_some_and(|line| line.trim_end_matches('\r') == "---")
        })
        .unwrap_or(1);
    let body_start = (frontmatter_end + 1).min(line_count);
    let whole_file = ctx_traits_core::source_map::SourceAnchor {
        file: path.to_string(),
        start: 1,
        end: line_count,
    };
    ImportSourceAnchors {
        frontmatter: ctx_traits_core::source_map::SourceAnchor {
            file: path.to_string(),
            start: 1,
            end: frontmatter_end,
        },
        body: ctx_traits_core::source_map::SourceAnchor {
            file: path.to_string(),
            start: body_start,
            end: line_count,
        },
        whole_file,
    }
}

pub(crate) fn trait_scaffold_from_trait(
    trait_ref: &ctx_traits_core::Trait,
    source_kind: &str,
    source_path: &str,
    source_digest: &str,
    anchors: ImportSourceAnchors,
    report: &ctx_traits_core::import::plan::ImportReport,
) -> crate::Result<ctx_traits_core::scaffold::TraitScaffold> {
    let mut declarations = Vec::new();
    push_scaffold_declaration(
        &mut declarations,
        format!("trait:{}", trait_ref.id.as_str()),
        "trait",
        &anchors,
    );
    if trait_ref.behavior.is_some() {
        push_scaffold_declaration(
            &mut declarations,
            format!("behavior:{}", trait_ref.id.as_str()),
            "behavior",
            &anchors,
        );
    }
    for agent in &trait_ref.agents {
        push_scaffold_declaration(
            &mut declarations,
            format!("agent:{}", agent.id),
            "agent",
            &anchors,
        );
    }
    for port in &trait_ref.ports {
        push_scaffold_declaration(
            &mut declarations,
            format!("port:{}", port.id),
            "port",
            &anchors,
        );
    }
    for slot in &trait_ref.slots {
        push_scaffold_declaration(
            &mut declarations,
            format!("slot:{}", slot.id),
            "slot",
            &anchors,
        );
    }
    for resource in &trait_ref.resources {
        push_scaffold_declaration(
            &mut declarations,
            format!("resource:{}", resource.id),
            "resource",
            &anchors,
        );
    }
    for schema in &trait_ref.schemas {
        push_scaffold_declaration(
            &mut declarations,
            format!("schema:{}", schema.id),
            "schema",
            &anchors,
        );
    }
    for signal in &trait_ref.signals {
        push_scaffold_declaration(
            &mut declarations,
            format!("signal:{}", signal.id),
            "signal",
            &anchors,
        );
    }
    for (prompt_id, _) in trait_ref.prompts.iter() {
        push_scaffold_declaration(
            &mut declarations,
            format!("prompt:{prompt_id}"),
            "prompt",
            &anchors,
        );
    }
    for (sequence_id, _) in trait_ref.sequences.iter() {
        push_scaffold_declaration(
            &mut declarations,
            format!("sequence:{sequence_id}"),
            "sequence",
            &anchors,
        );
    }
    if let Some(procedure) = trait_ref.procedure.as_ref() {
        for (index, item) in procedure.sequence.iter().enumerate() {
            let step_id = item
                .id
                .clone()
                .unwrap_or_else(|| format!("step-{}", index + 1));
            push_scaffold_declaration(
                &mut declarations,
                format!("sequence:procedure/{step_id}"),
                "procedure-step",
                &anchors,
            );
        }
    }
    declarations.sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    declarations.dedup_by(|a, b| a.ref_text == b.ref_text);

    let dependency_edges = trait_ref
        .dependencies
        .iter()
        .map(
            |dependency| ctx_traits_core::scaffold::ScaffoldDependencyEdge {
                alias: dependency.alias.clone(),
                trait_id: dependency.id.clone(),
                version: dependency.version.clone(),
            },
        )
        .collect();
    let mut review_warnings = report.conversion_warnings.clone();
    review_warnings.extend(report.warnings.iter().map(|warning| format!("{warning:?}")));
    review_warnings.sort();
    review_warnings.dedup();

    let scaffold = ctx_traits_core::scaffold::TraitScaffold {
        trait_id: trait_ref.id.as_str().to_string(),
        source: ctx_traits_core::scaffold::ScaffoldSource {
            kind: source_kind.to_string(),
            path: source_path.to_string(),
            digest: source_digest.to_string(),
        },
        declarations,
        dependency_edges,
        review_warnings,
        check: ctx_traits_core::scaffold::ScaffoldCheckState {
            required: true,
            passed: None,
            summary: "scaffold remains untrusted until deterministic ctx traits check passes"
                .to_string(),
        },
    };
    scaffold.validate()?;
    Ok(scaffold)
}

/// Resource ID mappings for a multi-file skill source's linked files, with a
/// collision check: two different source paths must never derive the same
/// canonical resource ID.
///
/// Runs each included file's content through the same deterministic checklist
/// reconciler as the entry document (P405): a file recognized as a checklist
/// with no residual non-checklist content is mapped with `variant:
/// "checklist"` and its derived typed resource is returned alongside, keyed
/// by resource id, so the caller can splice a checklist resource into the
/// draft instead of a path-backed one. A checklist resource has no field for
/// prose (`path`/`content` are rejected alongside `item`), so a linked file
/// whose checklist structure is only part of its content — leading/trailing
/// prose, unrelated sections — is conservatively left with its existing
/// `variant: "markdown"` path-backed mapping instead of silently dropping
/// that residual text.
pub(crate) fn multi_file_resource_mappings(
    mf: &ctx_traits_io::import::MultiFileSkillSource,
    prior_checklists: &std::collections::BTreeMap<
        String,
        Vec<ctx_traits_core::r#trait::ChecklistItem>,
    >,
) -> crate::Result<(
    Vec<ctx_traits_core::import::plan::ResourceIdMapping>,
    std::collections::BTreeMap<String, ctx_traits_core::r#trait::Resource>,
)> {
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut mappings = Vec::new();
    let mut checklist_resources = std::collections::BTreeMap::new();
    for (source_path, content) in &mf.included_files {
        let resource_id = ctx_traits_core::import::plan::resource_id_from_path(source_path);
        if let Some(previous) = seen.get(&resource_id) {
            return Err(crate::Error::Command {
                message: format!(
                    "resource ID collision: '{resource_id}' derived from '{source_path}' \
                     collides with '{previous}'; rename source files to avoid ambiguity"
                ),
            });
        }
        seen.insert(resource_id.clone(), source_path.clone());

        let prior_items = prior_checklists
            .get(&resource_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let variant = match ctx_traits_core::import::plan::derive_checklist_resource(
            &resource_id,
            content,
            prior_items,
        ) {
            Some(extraction) if extraction.residual_text.trim().is_empty() => {
                checklist_resources.insert(resource_id.clone(), extraction.resource);
                "checklist".to_string()
            }
            _ => "markdown".to_string(),
        };
        mappings.push(ctx_traits_core::import::plan::ResourceIdMapping {
            source_path: source_path.clone(),
            resource_id,
            variant,
        });
    }
    Ok((mappings, checklist_resources))
}

/// Structured multi-file import evidence (entry file, included files,
/// skipped/missing/unsafe links, resource mappings, graph digest) built from
/// an already-read [`ctx_traits_io::import::MultiFileSkillSource`].
pub(crate) fn multi_file_evidence(
    mf: &ctx_traits_io::import::MultiFileSkillSource,
    resource_mappings: Vec<ctx_traits_core::import::plan::ResourceIdMapping>,
) -> ctx_traits_core::import::plan::MultiFileEvidence {
    use ctx_traits_core::import::plan::{LinkEvidence, MultiFileEvidence, SkillLinkStatus};

    let link_evidence = |status: SkillLinkStatus| -> Vec<LinkEvidence> {
        mf.graph
            .edges
            .iter()
            .filter(|edge| edge.status == status)
            .map(|edge| LinkEvidence {
                source_file: edge.source_path.clone(),
                target: edge.target_path.clone(),
                link_text: edge.link_text.clone(),
            })
            .collect()
    };

    MultiFileEvidence {
        entry_file: "SKILL.md".to_string(),
        included_files: mf
            .included_files
            .iter()
            .map(|(path, _)| path.clone())
            .collect(),
        skipped_external: link_evidence(SkillLinkStatus::SkippedExternal),
        missing_links: link_evidence(SkillLinkStatus::Missing),
        unsafe_links: link_evidence(SkillLinkStatus::Unsafe),
        resource_mappings,
        graph_digest: Some(mf.graph.graph_digest.clone()),
    }
}

/// Splice discovered multi-file resource mappings into a draft trait's
/// `resource` array before synthesis.
///
/// Merges by resource id rather than replacing the array outright: the entry
/// document's own planning (e.g. a checklist derived from `SKILL.md` itself)
/// may already have declared resources before this runs, and those
/// declarations must survive a directory import untouched. A multi-file
/// mapping whose resource id collides with one already present fails the
/// import explicitly instead of silently dropping the linked file's content
/// — an entry-document checklist (fixed id `checklist`) and a linked
/// `checklist.md` (path-derived id `checklist`) are the common case.
pub(crate) fn augment_draft_with_multi_file_resources(
    draft_json: &mut serde_json::Value,
    resource_mappings: &[ctx_traits_core::import::plan::ResourceIdMapping],
    checklist_resources: &std::collections::BTreeMap<String, ctx_traits_core::r#trait::Resource>,
) -> crate::Result<()> {
    if resource_mappings.is_empty() {
        return Ok(());
    }
    let Some(obj) = draft_json.as_object_mut() else {
        return Ok(());
    };
    let mut resources: Vec<serde_json::Value> = obj
        .get("resource")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut existing_ids: std::collections::BTreeSet<String> = resources
        .iter()
        .filter_map(|resource| resource.get("id").and_then(|id| id.as_str()))
        .map(str::to_string)
        .collect();

    for mapping in resource_mappings {
        if existing_ids.contains(&mapping.resource_id) {
            return Err(crate::Error::Command {
                message: format!(
                    "resource ID collision: linked file '{}' derives resource id '{}', which the entry \
                     document already declares (e.g. an entry checklist derived from SKILL.md itself); \
                     rename the linked file to avoid ambiguity",
                    mapping.source_path, mapping.resource_id
                ),
            });
        }
        let resource_json = if let Some(resource) = checklist_resources.get(&mapping.resource_id) {
            serde_json::to_value(resource).map_err(|e| crate::Error::Command {
                message: format!(
                    "failed to serialize derived checklist resource '{}': {e}",
                    mapping.resource_id
                ),
            })?
        } else {
            serde_json::json!({
                "id": mapping.resource_id,
                "path": format!("resources/{}.md", mapping.resource_id),
            })
        };
        existing_ids.insert(mapping.resource_id.clone());
        resources.push(resource_json);
    }

    if !resources.is_empty() {
        obj.insert("resource".to_string(), serde_json::Value::Array(resources));
    }
    Ok(())
}

/// Reconstruct `behavior`/`resource`/`procedure`/`dependency` draft sections
/// from a ctx.traits-exported `SKILL.md`'s own sections, when the source
/// markdown claims the ctx.traits skill-export format.
pub(crate) fn augment_draft_with_skill_format_sections(
    draft_json: &mut serde_json::Value,
    markdown: &str,
    warnings: &mut Vec<String>,
) {
    if !ctx_traits_core::export::claims_skill_format(markdown) {
        return;
    }
    let Some(object) = draft_json.as_object_mut() else {
        return;
    };

    if section_body(markdown, "How To Apply").is_some_and(|body| {
        body.lines()
            .any(|line| line.trim_start().starts_with("- [ ]"))
    }) && !object.contains_key("behavior")
    {
        object.insert(
            "behavior".to_string(),
            serde_json::json!({ "format": ["imported-skill-checklist"] }),
        );
        warnings.push("skill-format import reconstructed behavior kind".to_string());
    }

    if let Some(body) = section_body(markdown, "Knowledge Resources") {
        let resources = resources_from_skill_section(body);
        if !resources.is_empty() && !object.contains_key("resource") {
            object.insert("resource".to_string(), serde_json::Value::Array(resources));
            warnings.push("skill-format import reconstructed knowledge resources".to_string());
        }
    }

    if let Some(body) = section_body(markdown, "Procedure Steps")
        && !body.contains("No deterministic procedure is declared")
        && !object.contains_key("procedure")
    {
        let sequence = procedure_steps_from_skill_section(body);
        if !sequence.is_empty() {
            object.insert(
                "procedure".to_string(),
                serde_json::json!({
                    "description": "Imported procedure reconstructed from ctx.traits skill export.",
                    "sequence": sequence,
                }),
            );
            warnings.push("skill-format import reconstructed procedure kind".to_string());
        }
    }

    if let Some(body) = section_body(markdown, "Dependency Contract") {
        let dependencies = dependencies_from_skill_section(body);
        if !dependencies.is_empty() && !object.contains_key("dependency") {
            object.insert(
                "dependency".to_string(),
                serde_json::Value::Array(dependencies),
            );
            warnings.push("skill-format import reconstructed dependency contract".to_string());
        }
    }
}

fn section_body<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}");
    let mut body_start = None;
    let mut body_end = markdown.len();
    let mut offset = 0usize;
    for line in markdown.split_inclusive('\n') {
        let normalized = line.trim_end_matches(['\r', '\n']);
        if body_start.is_none() {
            if normalized.trim() == marker {
                body_start = Some(offset + line.len());
            }
        } else if normalized.starts_with("## ") {
            body_end = offset;
            break;
        }
        offset += line.len();
    }
    let body_start = body_start?;
    markdown.get(body_start..body_end).map(str::trim)
}

fn resources_from_skill_section(body: &str) -> Vec<serde_json::Value> {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("### resource:"))
        .filter_map(|id| {
            let id = id.trim();
            (!id.is_empty()).then(|| {
                serde_json::json!({
                    "id": id,
                    "path": "imported/SKILL.md",
                    "hint": "Reconstructed from ctx.traits skill export section",
                })
            })
        })
        .collect()
}

fn procedure_steps_from_skill_section(body: &str) -> Vec<serde_json::Value> {
    body.lines()
        .filter_map(|line| exported_step(line.trim()))
        .enumerate()
        .map(|(index, step)| {
            let prompt = step
                .prompt_ref
                .unwrap_or_else(|| format!("Apply imported skill step {}.", index + 1));
            serde_json::json!({
                "id": slugify_step_id(&step.title, index),
                "title": step.title,
                "prompt": prompt,
            })
        })
        .collect()
}

struct ExportedStep {
    title: String,
    prompt_ref: Option<String>,
}

fn exported_step(line: &str) -> Option<ExportedStep> {
    let (number, rest) = line.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let (title, construct) = rest.split_once(" (`")?;
    let construct = construct.split("`)").next()?.trim();
    let prompt_ref = ctx_traits_core::reference::Reference::parse(construct)
        .ok()
        .filter(|reference| reference.kind() == ctx_traits_core::reference::Kind::Prompt)
        .map(|reference| reference.as_str().to_string());
    let title = title.trim().trim_end_matches('.');
    (!title.is_empty()).then(|| ExportedStep {
        title: title.to_string(),
        prompt_ref,
    })
}

fn slugify_step_id(title: &str, index: usize) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        format!("step-{}", index + 1)
    } else {
        slug
    }
}

fn dependencies_from_skill_section(body: &str) -> Vec<serde_json::Value> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("- `")?;
            let (alias, rest) = line.split_once("` -> `")?;
            let (trait_id, rest) = rest.split_once("` version `")?;
            let version = rest.strip_suffix('`')?;
            Some(serde_json::json!({
                "alias": alias,
                "id": trait_id,
                "version": version,
            }))
        })
        .collect()
}

/// The full deterministic, side-effect-free result of analyzing one import
/// `--source` path: everything `ctx traits import` computes before it makes
/// any write/trust/lifecycle decision.
pub(crate) struct ImportSourceAnalysis {
    pub(crate) trait_id: String,
    pub(crate) trait_name: String,
    pub(crate) summary: String,
    pub(crate) raw_source_digest: ctx_traits_core::digest::Digest,
    pub(crate) loaded_source: ctx_traits_io::import::LoadedAgentSkillSource,
    pub(crate) multi_file_source: Option<ctx_traits_io::import::MultiFileSkillSource>,
    pub(crate) multi_file_resource_mappings:
        Option<Vec<ctx_traits_core::import::plan::ResourceIdMapping>>,
    pub(crate) draft_json: serde_json::Value,
    pub(crate) report: ctx_traits_core::import::plan::ImportReport,
    pub(crate) synth_output_text: String,
    pub(crate) scaffold: ctx_traits_core::scaffold::TraitScaffold,
    /// Non-fatal decode warnings from turning the synthesized TOML back into
    /// a `Trait` for scaffold construction. Callers decide whether/how to
    /// surface them (`import` prints them; `doctor` stays silent).
    pub(crate) decode_warnings: Vec<String>,
}

/// Run the full deterministic pre-write import analysis for `source_path`:
/// load the source (single `SKILL.md` file or a directory, discovering
/// linked resources in the directory case), plan the Agent-Skills import,
/// splice in multi-file resource/evidence and ctx.traits skill-format
/// sections, synthesize the canonical trait text, decode it, and build its
/// source-anchored [`ctx_traits_core::scaffold::TraitScaffold`].
///
/// Performs no writes, no trust/lifecycle decisions, no network access, and
/// no model calls — only the reads `ctx_traits_io::import::read_agent_skill_source`
/// / `read_multi_file_skill_source` already perform. `ctx traits import` and
/// `ctx traits doctor` both call this so their evidence for the same source
/// never diverges.
pub(crate) fn analyze_import_source(
    source_path: &camino::Utf8Path,
    source_profile: ctx_traits_core::import::plan::ImportProfile,
    generator_package: &str,
    scaffold_source_kind: &str,
    prior_checklists: &std::collections::BTreeMap<
        String,
        Vec<ctx_traits_core::r#trait::ChecklistItem>,
    >,
) -> crate::Result<ImportSourceAnalysis> {
    let multi_file_source = if source_path.is_dir() {
        Some(ctx_traits_io::import::read_multi_file_skill_source(
            source_path,
        )?)
    } else {
        None
    };

    let loaded_source = ctx_traits_io::import::read_agent_skill_source(source_path)?;
    let raw_source_digest = ctx_traits_io::import::digest_import_source(source_path)?;

    analyze_loaded_source(LoadedSourceAnalysisRequest {
        loaded_source,
        source_display_path: source_path.to_string(),
        raw_source_digest,
        multi_file_source,
        source_profile,
        generator_package,
        scaffold_source_kind,
        prior_checklists,
    })
}

/// Inputs for [`analyze_loaded_source`], grouped to keep the function under
/// clippy's argument-count lint without an `#[allow]`.
pub(crate) struct LoadedSourceAnalysisRequest<'a> {
    pub(crate) loaded_source: ctx_traits_io::import::LoadedAgentSkillSource,
    pub(crate) source_display_path: String,
    pub(crate) raw_source_digest: ctx_traits_core::digest::Digest,
    pub(crate) multi_file_source: Option<ctx_traits_io::import::MultiFileSkillSource>,
    pub(crate) source_profile: ctx_traits_core::import::plan::ImportProfile,
    pub(crate) generator_package: &'a str,
    pub(crate) scaffold_source_kind: &'a str,
    pub(crate) prior_checklists:
        &'a std::collections::BTreeMap<String, Vec<ctx_traits_core::r#trait::ChecklistItem>>,
}

/// The shared tail of [`analyze_import_source`]: everything from the
/// deterministic import planner onward, given an already-loaded source.
///
/// Split out so callers that cannot use `ctx_traits_io::import::read_agent_skill_source`'s
/// strict `SKILL.md`-named-file gate (`ctx traits doctor` previewing an
/// `AGENTS.md`/`CLAUDE.md` candidate that import itself cannot load yet)
/// still run the exact same planning/synth/decode/scaffold pipeline as
/// `ctx traits import`, instead of a second hand-maintained copy of it.
pub(crate) fn analyze_loaded_source(
    request: LoadedSourceAnalysisRequest<'_>,
) -> crate::Result<ImportSourceAnalysis> {
    let LoadedSourceAnalysisRequest {
        loaded_source,
        source_display_path,
        raw_source_digest,
        multi_file_source,
        source_profile,
        generator_package,
        scaffold_source_kind,
        prior_checklists,
    } = request;
    let mut import_plan = ctx_traits_core::import::plan::plan_agent_skills_import(
        ctx_traits_core::import::plan::AgentSkillsImportRequest {
            source: ctx_traits_core::import::plan::ImportSource::Local {
                path: source_display_path,
            },
            source_profile,
            raw_source_digest: raw_source_digest.clone(),
            source_path: loaded_source.skill_path.to_string(),
            source_name: loaded_source.source_name.clone(),
            skill_markdown: loaded_source.skill_markdown.clone(),
            prior_checklists: prior_checklists.clone(),
        },
    )?;

    let (multi_file_resource_mappings, multi_file_checklist_resources) =
        match multi_file_source.as_ref() {
            Some(mf) => {
                let (mappings, checklist_resources) =
                    multi_file_resource_mappings(mf, prior_checklists)?;
                (Some(mappings), checklist_resources)
            }
            None => (None, std::collections::BTreeMap::new()),
        };

    if let Some(ref mf) = multi_file_source {
        augment_draft_with_multi_file_resources(
            &mut import_plan.draft_json,
            multi_file_resource_mappings.as_deref().unwrap_or(&[]),
            &multi_file_checklist_resources,
        )?;
        import_plan.report.conversion_warnings.push(format!(
            "multi-file import: {} support file(s) discovered, {} edge(s) in artifact graph",
            mf.included_files.len(),
            mf.graph.edges.len()
        ));
        for w in &mf.graph_warnings {
            import_plan.report.conversion_warnings.push(w.clone());
        }
    }
    augment_draft_with_skill_format_sections(
        &mut import_plan.draft_json,
        &loaded_source.skill_markdown,
        &mut import_plan.report.conversion_warnings,
    );

    let synth_response = ctx_traits_core::synth::synthesize(ctx_traits_core::synth::Request {
        document_kind: ctx_traits_core::synth::DocumentKind::Trait,
        draft_json: import_plan.draft_json.clone(),
        output_format: ctx_traits_core::synth::OutputFormat::Toml,
        provenance: ctx_traits_core::synth::ProvenanceSeed {
            generator_package: Some(generator_package.to_string()),
            generator_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            source_path: Some(loaded_source.skill_path.to_string()),
            warnings: import_plan.report.conversion_warnings.clone(),
        },
    })?;

    let mut report = import_plan.report;
    let trait_digest = ctx_traits_core::digest::Digest::source(&synth_response.output_text);
    report.synth_provenance = Some(synth_response.provenance.clone());
    report.managed_import = Some(ctx_traits_core::import::plan::ManagedImportArtifact::new(
        import_plan.trait_id.clone(),
        report.source_profile.clone(),
        raw_source_digest.clone(),
        trait_digest,
    ));
    if let Some(ref mf) = multi_file_source {
        report.multi_file_evidence = Some(multi_file_evidence(
            mf,
            multi_file_resource_mappings.clone().unwrap_or_default(),
        ));
    }

    let (scaffold_trait, decode_warnings) = ctx_traits_core::encoding::decode_trait_with_warnings(
        ctx_traits_core::encoding::Encoding::Toml,
        &synth_response.output_text,
    )?;

    let anchors = source_anchors_for_text(&loaded_source.skill_path, &loaded_source.skill_markdown);
    let scaffold = trait_scaffold_from_trait(
        &scaffold_trait,
        scaffold_source_kind,
        loaded_source.skill_path.as_str(),
        raw_source_digest.as_str(),
        anchors,
        &report,
    )?;

    Ok(ImportSourceAnalysis {
        trait_id: import_plan.trait_id,
        trait_name: import_plan.trait_name,
        summary: import_plan.summary,
        raw_source_digest,
        loaded_source,
        multi_file_source,
        multi_file_resource_mappings,
        draft_json: import_plan.draft_json,
        report,
        synth_output_text: synth_response.output_text,
        scaffold,
        decode_warnings,
    })
}

fn push_scaffold_declaration(
    declarations: &mut Vec<ctx_traits_core::scaffold::ScaffoldDeclaration>,
    ref_text: String,
    kind: &str,
    anchors: &ImportSourceAnchors,
) {
    let (confidence, rationale, anchor) = if kind == "trait" {
        (
            98,
            "identity mapped directly from generated skill frontmatter",
            anchors.frontmatter.clone(),
        )
    } else if ref_text == "prompt:imported-guidance" {
        (
            75,
            "guidance body derived from imported skill Markdown",
            anchors.body.clone(),
        )
    } else {
        (
            70,
            "provisional declaration reconstructed from generated skill sections; per-declaration semantic anchors are deferred to Group 46 import-trait",
            anchors.whole_file.clone(),
        )
    };
    declarations.push(ctx_traits_core::scaffold::ScaffoldDeclaration {
        ref_text,
        kind: kind.to_string(),
        confidence,
        rationale: rationale.to_string(),
        anchor: Some(anchor),
    });
}
