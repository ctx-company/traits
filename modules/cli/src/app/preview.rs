//! Read-only mirror of a trait's prompt frames (`ctx traits preview`).
//!
//! No harness dispatch, no session start/advance, no runtime state written.
//! Every prompt is composed through the [`frame_prompt`] chain, the same
//! shared resolver/composer live drive dispatch uses — this module adds no
//! parallel prompt or frame compiler of its own.

use serde::Serialize;

use crate::app::command_handlers::print_json_report;
use crate::app::frame_prompt::{
    PendingInput, frame_prompt, human_frame_prompt, mcp_frame_prompt, requested_output_schema,
    requested_outputs, resolved_frame_prompt,
};
use ctx_traits_core::response::CommandOutput;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PreviewReport {
    pub(crate) trait_id: String,
    pub(crate) session: Option<String>,
    pub(crate) frames: Vec<PreviewFrame>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PreviewFrame {
    pub(crate) step: String,
    pub(crate) title: String,
    /// "cli", "mcp", or "static" (no `--session`: no transport is assigned yet).
    pub(crate) transport: String,
    /// The composed agent prompt for a `Prompt`-kind frame. `None` for a
    /// command frame — no agent prompt is ever composed for one; see
    /// `command` instead. Any pending (declared-but-unaccepted) inputs are
    /// already folded into this text by the shared `frame_prompt` resolver,
    /// not reported as a separate side-channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prompt: Option<String>,
    /// Disclosed command evidence for a `Command`-kind frame. `None` for an
    /// agent-prompt frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// Deterministic run/session id for the synthetic no-session preview state.
/// Never persisted or reused; it only labels the pure projected view.
const PREVIEW_ID: &str = "preview";

pub(crate) fn handle_preview(
    file: &str,
    step: Option<&str>,
    session: Option<&str>,
    session_store: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let report = match session {
        Some(session_path) => session_preview(file, step, session_path, session_store)?,
        None => static_preview(file, step)?,
    };
    if json {
        print_json_report(&report, "preview")?;
    } else {
        print_report(&report);
    }
    Ok(CommandOutput::new(()))
}

fn preview_session(
    trait_ref: &ctx_traits_core::Trait,
    state: &ctx_traits_core::procedure::runtime::State,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    use ctx_traits_core::procedure::session::{CallerProvenance, Provenance, Session, SessionId};
    Ok(Session {
        schema_version: ctx_traits_core::procedure::session::SCHEMA_VERSION.to_string(),
        session_id: SessionId::new(PREVIEW_ID)?,
        run_id: state.run_id.clone(),
        trait_id: state.trait_id.clone(),
        source_digest: state.source_digest.clone(),
        canonical_digest: state.canonical_digest.clone(),
        current_run_index: state.current_run_index,
        current_source_index: None,
        current_sequence_item_id: None,
        current_sequence_title: None,
        current_agent: None,
        status: ctx_traits_core::procedure::session::Status::AwaitingAgentOutput,
        warnings: Vec::new(),
        accepted_port_values: state.accepted_port_values.clone(),
        accepted_slot_values: state.accepted_slot_values.clone(),
        accepted_output_port_values: state.accepted_output_port_values.clone(),
        slot_revisions: state.slot_revisions.clone(),
        emitted_signals: state.emitted_signals.clone(),
        rejected_submissions: state.rejected_attempts.clone(),
        unresolved_inputs: Vec::new(),
        resource_evidence: state.resource_evidence.clone(),
        provider_capability_reports: Vec::new(),
        output_ports: state.output_ports.clone(),
        active_path: state.active_path.clone(),
        control_stack: state.control_stack.clone(),
        stop_reason: state.stop_reason.clone(),
        final_output_summary: Vec::new(),
        next_frame: None,
        last_validation_report: None,
        completion: None,
        last_drive_outcome: None,
        provenance: Provenance {
            started_by: CallerProvenance::cli(),
            state_source: "preview".to_string(),
            agent_assignments: None,
            harness_probes: Vec::new(),
            warnings: Vec::new(),
            trait_source: None,
            query_selection: None,
            worktree: None,
            merge_frames: Vec::new(),
            merge_intent: None,
            out_of_tree_mutations: Vec::new(),
            started_at_epoch: None,
            trust_approval: None,
            session_title: None,
        },
        ledger: state.clone(),
        state_digest: ctx_traits_core::digest::Digest::source(trait_ref.id.as_str()),
    })
}

fn static_preview(file: &str, step: Option<&str>) -> crate::Result<PreviewReport> {
    let loaded = ctx_traits_io::run::load_trait_source(Some(file), None, "preview")?;
    let run_id = ctx_traits_core::procedure::run::Id::new(PREVIEW_ID)?;
    let state = ctx_traits_core::procedure::runtime::start_procedure_run(
        &loaded.trait_ref,
        run_id,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
    )?;
    let frames = ctx_traits_core::procedure::runtime::preview_sequence_frames(
        &loaded.trait_ref,
        &state,
        step,
    )?;
    let session = preview_session(&loaded.trait_ref, &state)?;
    let preview_frames = frames
        .iter()
        .map(|frame| build_static_frame(&loaded, &session, frame))
        .collect::<crate::Result<Vec<_>>>()?;
    Ok(PreviewReport {
        trait_id: loaded.trait_ref.id.as_str().to_string(),
        session: None,
        frames: preview_frames,
    })
}

fn frame_step_label(frame: &ctx_traits_core::procedure::runtime::SequenceFrame) -> String {
    frame.item_id.clone().unwrap_or_else(|| {
        frame
            .sequence_index
            .map_or_else(String::new, |i| i.to_string())
    })
}

/// Producer step(s) for a declared-but-unaccepted input ref, from the
/// dry-plan's own planned-item tree (`plan_procedure_run`) rather than a
/// hand-rolled first-declared-output scan — the same plan `ctx traits plan`
/// uses, so a slot produced by more than one candidate step (e.g. either arm
/// of a branch) reports every candidate instead of silently picking one.
///
/// Walks `plan.sequence_items` (and each item's `children`/`otherwise_children`)
/// directly rather than `plan.producer_edges`: an edge's `sequence_index`
/// identifies only the top-level container a nested producer lives inside
/// (e.g. a loop), so resolving through it mislabels a nested step's own
/// output with its container's id.
fn producer_steps(loaded: &ctx_traits_io::run::LoadedTrait, ref_text: &str) -> Vec<String> {
    let Ok(run_id) = ctx_traits_core::procedure::run::Id::new(PREVIEW_ID) else {
        return Vec::new();
    };
    let Ok(plan) = ctx_traits_core::procedure::run::plan_procedure_run(&loaded.trait_ref, run_id)
    else {
        return Vec::new();
    };
    let mut labels = Vec::new();
    collect_producer_labels(&plan.sequence_items, ref_text, &mut labels);
    labels.sort();
    labels.dedup();
    labels
}

fn collect_producer_labels(
    items: &[ctx_traits_core::procedure::run::PlannedSequenceItem],
    ref_text: &str,
    labels: &mut Vec<String>,
) {
    for item in items {
        if item
            .output_refs
            .iter()
            .any(|output| output.as_str() == ref_text)
        {
            labels.push(item.item_id.clone().unwrap_or_else(|| item.title.clone()));
        }
        collect_producer_labels(&item.children, ref_text, labels);
        collect_producer_labels(&item.otherwise_children, ref_text, labels);
    }
}

/// Resolves the exact declared [`SequenceItem`](ctx_traits_core::r#trait::procedure::SequenceItem)
/// a frame was produced from, by structural position rather than `item_id`
/// string match. `item_id` is optional (id-less items) and, when present, is
/// only unique *within* the sequence that declares it — matching it globally
/// across `loaded.trait_ref.sequences` can land on a same-named id in the
/// wrong sequence.
///
/// `frame.position_path`'s trailing segments identify the item's owning
/// named sequence and its index within it, but the two frame builders don't
/// agree on shape: the live path (`path_for_nested_item`, used by session
/// drive) appends a final `kind: "item"` segment after the owning
/// control-frame segment, `[.., <owning sequence, id + index>, <item>]`,
/// while the no-session static-preview path (`expand_nested_preview`) has no
/// trailing item segment at all — the last segment itself is `<owning
/// sequence, id + index>`. Both agree that the rightmost non-`"item"`
/// segment carries the owning sequence's id *and* the item's index within
/// it (duplicated onto the trailing item segment when one exists), so
/// skipping a trailing `"item"` segment before reading `id`/`index`
/// resolves either shape identically. An empty `position_path` means a
/// top-level item, addressed instead by `frame.sequence_index` into
/// `procedure.sequence`.
fn resolve_declared_item<'a>(
    loaded: &'a ctx_traits_io::run::LoadedTrait,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> Option<&'a ctx_traits_core::r#trait::procedure::SequenceItem> {
    let path = frame.position_path.as_slice();
    let owner = match path {
        [.., last] if last.kind == "item" => path.get(path.len().wrapping_sub(2))?,
        [.., last] => last,
        [] => {
            let sequence_index = frame.sequence_index?;
            return loaded
                .trait_ref
                .procedure
                .as_ref()?
                .sequence
                .get(sequence_index);
        }
    };
    let sequence_id = owner.id.as_deref()?;
    loaded
        .trait_ref
        .sequences
        .get(sequence_id)?
        .sequence
        .get(owner.index)
}

fn pending_inputs_for(
    loaded: &ctx_traits_io::run::LoadedTrait,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> Vec<PendingInput> {
    let Some(item) = resolve_declared_item(loaded, frame) else {
        return Vec::new();
    };
    let accepted: std::collections::BTreeSet<&str> = frame
        .available_inputs
        .iter()
        .map(|input| input.ref_text.as_str())
        .collect();
    item.input
        .iter()
        .filter(|input| !input.is_optional())
        .map(|input| input.ref_text())
        .filter(|ref_text| !accepted.contains(ref_text))
        .map(|ref_text| {
            let reason = match producer_steps(loaded, ref_text) {
                producers if producers.is_empty() => "external input, not yet accepted".to_string(),
                producers => format!("produced by step {}", producers.join(" or ")),
            };
            PendingInput {
                ref_text: ref_text.to_string(),
                reason,
            }
        })
        .collect()
}

/// Command and check steps dispatch through the runtime IO edge, never
/// through the agent-prompt composer — live drive never hands either frame
/// kind to `frame_prompt`. Rendering one there would fabricate a
/// JSON-response agent prompt for a step no agent ever receives, so preview
/// surfaces the disclosed
/// [`CommandFrame`](ctx_traits_core::procedure::runtime::CommandFrame)
/// evidence instead of composing a prompt for it.
fn command_preview_frame(
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> PreviewFrame {
    let is_check = frame.kind == ctx_traits_core::procedure::runtime::SequenceFrameKind::Check;
    PreviewFrame {
        step: frame_step_label(frame),
        title: frame.title.clone(),
        transport: if is_check { "check" } else { "command" }.to_string(),
        prompt: None,
        command: Some(frame.frame_text.clone()),
        note: Some(if is_check {
            "check step: dispatched via the runtime IO edge, not an agent — no agent prompt is composed for this frame"
                .to_string()
        } else {
            "command step: dispatched via the runtime IO edge, not an agent — no agent prompt is composed for this frame"
                .to_string()
        }),
    }
}

fn is_command_backed_frame(frame: &ctx_traits_core::procedure::runtime::SequenceFrame) -> bool {
    matches!(
        frame.kind,
        ctx_traits_core::procedure::runtime::SequenceFrameKind::Command
            | ctx_traits_core::procedure::runtime::SequenceFrameKind::Check
    )
}

fn build_static_frame(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> crate::Result<PreviewFrame> {
    if is_command_backed_frame(frame) {
        return Ok(command_preview_frame(frame));
    }
    let pending_inputs = pending_inputs_for(loaded, frame);
    let context = resolved_frame_prompt(loaded, session, frame, &pending_inputs)?;
    let requested = requested_outputs(frame)?;
    let schema = requested_output_schema(&requested, loaded);
    let prompt = frame_prompt(&context, &schema, None);
    Ok(PreviewFrame {
        step: frame_step_label(frame),
        title: frame.title.clone(),
        transport: "static".to_string(),
        prompt: Some(prompt),
        command: None,
        note: Some(
            "no session: only accepted values are inlined; pending inputs are declared inline in the resolved prompt below"
                .to_string(),
        ),
    })
}

fn session_preview(
    file: &str,
    step: Option<&str>,
    session_path: &str,
    session_store: Option<&str>,
) -> crate::Result<PreviewReport> {
    let outcome = ctx_traits_io::run::status(ctx_traits_io::run::InspectRequest {
        trait_file: Some(file),
        trait_id: None,
        session: session_path,
        session_store,
        elapsed_seconds: None,
    })?;
    let session = outcome.session;
    let loaded = ctx_traits_io::run::load_trait_for_session(Some(file), None, &session, "preview")?;

    let active_matches = |frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
                          step: &str| {
        frame.item_id.as_deref() == Some(step)
            || frame.sequence_index.map(|i| i.to_string()).as_deref() == Some(step)
            || frame.run_index.map(|i| i.to_string()).as_deref() == Some(step)
    };

    let preview_frame = match (session.next_frame.as_deref(), step) {
        (Some(frame), Some(step)) if active_matches(frame, step) => {
            build_session_frame(&loaded, &session, frame)?
        }
        (Some(frame), None) => build_session_frame(&loaded, &session, frame)?,
        (_, Some(step)) => {
            let Some((historical_frame, historical_slot_values)) =
                ctx_traits_core::procedure::runtime::preview_historical_frame(
                    &loaded.trait_ref,
                    &session.ledger,
                    step,
                )?
            else {
                return Err(crate::Error::Command {
                    message: format!(
                        "{step} matches no step in this session's ledger (active: {})",
                        session
                            .next_frame
                            .as_ref()
                            .and_then(|frame| frame.item_id.as_deref())
                            .unwrap_or("none — run is not awaiting a step")
                    ),
                });
            };
            // Historical values inline against a session whose accepted-slot
            // aggregate is overridden to the pre-step reconstruction, not the
            // ledger's current aggregate — `build_session_frame`/
            // `resolved_frame_prompt` are otherwise unchanged and unaware
            // this is a historical view.
            let mut historical_session = session.clone();
            historical_session.accepted_slot_values = historical_slot_values;
            let mut frame = build_session_frame(&loaded, &historical_session, &historical_frame)?;
            frame.note = Some(match frame.note.take() {
                Some(existing) => format!(
                    "historical: reconstructed as of this step's own activation, not the ledger's current aggregate; {existing}"
                ),
                None => "historical: reconstructed as of this step's own activation, not the ledger's current aggregate".to_string(),
            });
            frame
        }
        (None, None) => {
            return Ok(PreviewReport {
                trait_id: loaded.trait_ref.id.as_str().to_string(),
                session: Some(session_path.to_string()),
                frames: Vec::new(),
            });
        }
    };
    Ok(PreviewReport {
        trait_id: loaded.trait_ref.id.as_str().to_string(),
        session: Some(session_path.to_string()),
        frames: vec![preview_frame],
    })
}

fn build_session_frame(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> crate::Result<PreviewFrame> {
    if is_command_backed_frame(frame) {
        return Ok(command_preview_frame(frame));
    }
    let context = resolved_frame_prompt(loaded, session, frame, &[])?;
    if frame.kind == ctx_traits_core::procedure::runtime::SequenceFrameKind::Ask {
        return Ok(PreviewFrame {
            step: frame_step_label(frame),
            title: frame.title.clone(),
            transport: "human".to_string(),
            prompt: Some(human_frame_prompt(
                session.session_id.as_str(),
                frame,
                &context,
            )),
            command: None,
            note: Some("human-owned frame; no agent or harness dispatch".to_string()),
        });
    }
    let role = frame
        .assigned_agent
        .as_ref()
        .map_or(ctx_traits_io::harness_config::DEFAULT_SEAT, |agent| {
            agent.role.as_str()
        });
    let structural_seat = frame
        .assigned_agent
        .as_ref()
        .and_then(|agent| agent.structural_seat);
    let assignment = session
        .provenance
        .agent_assignments
        .as_ref()
        .and_then(|assignments| {
            ctx_traits_core::procedure::session::select_agent_assignment(
                assignments,
                role,
                structural_seat,
            )
        });
    let step = frame_step_label(frame);
    match assignment {
        Some(assignment) if assignment.transport == "mcp" => {
            let prompt = mcp_frame_prompt(
                session.session_id.as_str(),
                None,
                frame,
                &context,
                role,
                &assignment.harness,
            );
            Ok(PreviewFrame {
                step,
                title: frame.title.clone(),
                transport: "mcp".to_string(),
                prompt: Some(prompt),
                command: None,
                note: Some(
                    "transport: mcp — CLI-composed, not byte-verified against a live MCP dispatch"
                        .to_string(),
                ),
            })
        }
        _ => {
            let requested = requested_outputs(frame)?;
            let schema = requested_output_schema(&requested, loaded);
            let prompt = frame_prompt(&context, &schema, None);
            Ok(PreviewFrame {
                step,
                title: frame.title.clone(),
                transport: "cli".to_string(),
                prompt: Some(prompt),
                command: None,
                note: None,
            })
        }
    }
}

fn print_report(report: &PreviewReport) {
    println!("ctx traits preview");
    println!("  trait: {}", report.trait_id);
    println!("  session: {}", report.session.as_deref().unwrap_or("none"));
    for frame in &report.frames {
        println!(
            "--- step {} [{}] transport={} ---",
            frame.step, frame.title, frame.transport
        );
        if let Some(note) = frame.note.as_deref() {
            println!("  note: {note}");
        }
        if let Some(command) = frame.command.as_deref() {
            println!("{command}");
        }
        if let Some(prompt) = frame.prompt.as_deref() {
            println!("{prompt}");
        }
    }
}
