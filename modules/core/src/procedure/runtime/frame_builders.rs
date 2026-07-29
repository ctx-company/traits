// Procedure runtime frame construction.
// Procedure runtime frame builders.

/// Pure, read-only projection of a trait's *executable* prompt/command
/// frames outside live control-flow advancement, for `ctx traits preview`.
/// Reuses the same frame-content builder `next_sequence_frame` calls for the
/// live position (`build_sequence_frame`), so preview frame content,
/// ordering, and resolution behavior never diverge from a live frame for the
/// same item and state.
///
/// Only leaf items live dispatch exposes — `Prompt` and `Command` kinds, per
/// `is_executable_item` — produce a frame here. Runtime-owned `Project` leaves
/// are intentionally absent.
/// Container kinds (`Sequence`, `Branch`, `Loop`, `ForEach`) are never
/// dispatch targets themselves; this walks into their declared `sequence`/
/// `otherwise` named-sequence refs instead, so nested items such as a loop
/// body's steps are projected too, matching the executable surface live
/// control-flow can reach. Declaration order stands in for control-flow
/// position: guard evaluation, active loop/for-each iteration, and branch
/// selection are not replayed (both a branch's arms are projected, and a
/// loop/for-each body is projected once), so a projected frame's
/// `position_path` carries only structural (kind/id/index) segments —
/// `iteration`/`item_index` are always absent, never a replayed live value.
pub fn preview_sequence_frames(
    trait_ref: &Trait,
    state: &State,
    step_id: Option<&str>,
) -> crate::Result<Vec<SequenceFrame>> {
    let proc = procedure(trait_ref)?;
    let sequence = effective_sequence_items(proc)?;
    let ctx = PreviewContext {
        trait_ref,
        state,
        step_id,
    };
    let mut frames = Vec::new();
    for effective in &sequence {
        collect_preview_frames(
            &ctx,
            effective.item,
            effective.declaration_index,
            effective.run_index,
            Vec::new(),
            0,
            &mut frames,
        )?;
    }
    Ok(frames)
}

/// The read-only inputs shared by every call in a single [`preview_sequence_frames`]
/// walk, bundled so the recursive helpers stay under clippy's argument-count
/// limit without duplicating `trait_ref`/`state`/`step_id` at every call site.
struct PreviewContext<'a> {
    trait_ref: &'a Trait,
    state: &'a State,
    step_id: Option<&'a str>,
}

/// Recursion budget for [`collect_preview_frames`]. Cyclic named-sequence
/// refs (rejected by the dry-planner) still terminate here because depth
/// strictly increases at every nested expansion, without needing a separate
/// cycle-tracking stack.
fn collect_preview_frames(
    ctx: &PreviewContext<'_>,
    item: &crate::r#trait::procedure::SequenceItem,
    sequence_index: usize,
    run_index: usize,
    position_path: Vec<PathSegment>,
    depth: usize,
    frames: &mut Vec<SequenceFrame>,
) -> crate::Result<()> {
    if is_executable_item(item) {
        let matches = ctx.step_id.is_none_or(|step_id| {
            item.id.as_deref() == Some(step_id)
                || (position_path.is_empty()
                    && (sequence_index.to_string() == step_id || run_index.to_string() == step_id))
        });
        if matches {
            let ready = ReadyItem {
                sequence_index,
                run_index,
                item,
                position_path,
                loop_context: None,
                for_each_context: None,
                guard_explanations: Vec::new(),
            };
            frames.push(build_sequence_frame(ctx.trait_ref, ctx.state, &ready)?);
        }
        return Ok(());
    }
    if depth >= MAX_SEQUENCE_NESTING_DEPTH {
        return Ok(());
    }
    let kind_label = match item.effective_kind() {
        SequenceKind::Branch => "branch",
        SequenceKind::Loop => "loop",
        SequenceKind::ForEach => "for-each",
        SequenceKind::Parallel => "parallel",
        SequenceKind::Sequence
        | SequenceKind::Prompt
        | SequenceKind::Ask
        | SequenceKind::Command
        | SequenceKind::Check
        | SequenceKind::Project => "sequence",
    };
    // A `parallel` item is itself never a dispatch target; traverse its
    // branches in authored
    // order for static preview only.
    if item.effective_kind() == SequenceKind::Project {
        return Ok(());
    }
    let nested_refs: Vec<&str> = if item.effective_kind() == SequenceKind::Parallel {
        item.branches.iter().map(String::as_str).collect()
    } else {
        [item.sequence.as_deref(), item.otherwise.as_deref()]
            .into_iter()
            .flatten()
            .collect()
    };
    for nested_ref in nested_refs {
        expand_nested_preview(
            ctx,
            nested_ref,
            kind_label,
            run_index,
            &position_path,
            depth,
            frames,
        )?;
    }
    Ok(())
}

fn expand_nested_preview(
    ctx: &PreviewContext<'_>,
    sequence_ref: &str,
    kind_label: &str,
    run_index: usize,
    parent_path: &[PathSegment],
    depth: usize,
    frames: &mut Vec<SequenceFrame>,
) -> crate::Result<()> {
    let Ok(parsed) = Reference::parse(sequence_ref) else {
        return Ok(());
    };
    if parsed.kind() != Kind::Sequence || parsed.is_qualified() {
        return Ok(());
    }
    let sequence_id = parsed.id();
    let Some(named) = ctx.trait_ref.sequences.get(sequence_id) else {
        return Ok(());
    };
    for (index, nested_item) in named.sequence.iter().enumerate() {
        let mut path = parent_path.to_vec();
        path.push(PathSegment {
            kind: kind_label.to_string(),
            id: Some(sequence_id.to_string()),
            index,
            iteration: None,
            item_index: None,
        });
        collect_preview_frames(ctx, nested_item, index, run_index, path, depth + 1, frames)?;
    }
    Ok(())
}

/// Reconstruct and build the frame for a step identified by `step_id` as it
/// stood in the run ledger — for a step that has already executed, using
/// the value each of its declared inputs held *before* that step's own
/// activation wrote anything, not the ledger's current aggregate. This is
/// what makes `--session --step <id>` safe for a step behind a rewritten
/// slot, a branch, or a prior loop iteration: the live aggregate in `state`
/// may already reflect writes made *after* the requested activation.
///
/// Returns `None` when no ledger status matches `step_id`. Reuses
/// `build_sequence_frame` for frame content — this adds no second frame
/// compiler, only the historical accepted-slot-value reconstruction that
/// feeds it.
pub fn preview_historical_frame(
    trait_ref: &Trait,
    state: &State,
    step_id: &str,
) -> crate::Result<Option<(SequenceFrame, Vec<Value>)>> {
    // `sequence_statuses` is append-only per distinct `position_path`
    // (`set_path_sequence_status` pushes a new entry only the first time a
    // path is seen, and updates in place thereafter), so vec order IS
    // activation order across every distinct path. Taking the last matching
    // entry therefore selects the most-recently-activated match; a scalar
    // `max_by_key` over each path's own iteration/item-index component (the
    // prior approach) compared apples to oranges across nesting depths — an
    // early outer iteration with a deep inner iteration (e.g. outer=1,
    // inner=10) could outrank a later outer iteration with a shallow one
    // (outer=2, inner=1), even though the latter activated after the former.
    let Some(status) = state.sequence_statuses.iter().rfind(|status| {
        status.item_id.as_deref() == Some(step_id)
            || (status.position_path.is_empty()
                && (status.sequence_index.to_string() == step_id
                    || status.run_index.to_string() == step_id))
    }) else {
        return Ok(None);
    };
    let Some(item) = item_for_status(trait_ref, status) else {
        return Ok(None);
    };
    // Both repeated-control contexts are reconstructed after the historical
    // value snapshot. Dynamic bounds must not resolve through a later value
    // still present in an active parallel buffer.
    let mut ready = ReadyItem {
        sequence_index: status.sequence_index,
        run_index: status.run_index,
        item,
        for_each_context: None,
        loop_context: None,
        position_path: status.position_path.clone(),
        guard_explanations: Vec::new(),
    };
    let cutoff = activation_cutoff(state, &ready);
    let activation_path = producer_path_for_ready(&ready);
    let historical_slot_values = accepted_slot_values_before(state, cutoff, &activation_path);
    // Build the frame from a state whose accepted-slot aggregate already IS
    // the reconstruction, not the ledger's current aggregate: `available_inputs`
    // (and every digest in it) must come from the same snapshot the caller
    // will use to inline values, or the frame's own digests disagree with the
    // reconstruction that is supposed to satisfy them. Clear every live
    // parallel value overlay so `accepted_value` cannot shadow the snapshot
    // with a post-cutoff value from the current runtime position.
    let mut historical_state = state.clone();
    historical_state.accepted_slot_values = historical_slot_values.clone();
    for frame in &mut historical_state.control_stack {
        frame.parallel_buffer.accepted_slot_values.clear();
        frame.parallel_buffer.accepted_output_port_values.clear();
    }
    ready.loop_context = reconstruct_loop_context(
        trait_ref,
        &historical_state,
        &status.position_path,
    );
    // Reuses the same helper live dispatch calls (`current_ready_item`) with
    // the reconstructed `loop_context` in place of the live control stack's.
    ready.guard_explanations =
        guard_explanations_for_context(state, ready.loop_context.as_ref());
    ready.for_each_context = reconstruct_for_each_context(
        trait_ref,
        &status.position_path,
        &historical_state,
        &historical_slot_values,
    );
    let frame = build_sequence_frame(trait_ref, &historical_state, &ready)?;
    Ok(Some((frame, historical_slot_values)))
}

/// Reconstruct the [`LoopContext`] a live activation at `path` would have
/// carried, from the innermost `kind == "loop"` segment (if any). The
/// segment's `id` is the loop body's named-sequence id (what
/// `path_for_nested_item` stamps for every control-stack frame); the loop
/// item's own id (`loop_id`) and `max-iterations` bound are recovered by
/// finding the declared loop item whose `sequence` ref points at that body,
/// since neither survives on the path segment itself. Falls back to the
/// sequence id / observed iteration when no such declaration is found (e.g.
/// a stale ledger from a since-edited trait), so reconstruction degrades
/// gracefully instead of silently dropping the context.
fn reconstruct_loop_context(
    trait_ref: &Trait,
    state: &State,
    path: &[PathSegment],
) -> Option<LoopContext> {
    let segment = path.iter().rev().find(|segment| segment.kind == "loop")?;
    let sequence_id = segment.id.clone()?;
    let control_item = control_item_for_sequence(trait_ref, &sequence_id, SequenceKind::Loop);
    let iteration_index = segment.iteration.unwrap_or(0);
    Some(LoopContext {
        loop_id: control_item
            .and_then(|item| item.id.clone())
            .unwrap_or_else(|| sequence_id.clone()),
        sequence_id: Some(sequence_id),
        iteration_index,
        max_iterations: control_item
            .and_then(|item| resolved_loop_bound(item, state))
            .unwrap_or(iteration_index + 1),
    })
}

fn resolved_loop_bound(
    item: &crate::r#trait::procedure::SequenceItem,
    state: &State,
) -> Option<usize> {
    item.max_iterations.or_else(|| {
        let ref_text = item.max_iterations_from.as_deref()?;
        let value = accepted_value(state, ref_text)?.value.as_u64()?;
        usize::try_from(value).ok().filter(|value| *value > 0)
    })
}

/// Reconstruct the [`ForEachContext`] a live activation at `path` would have
/// carried, mirroring [`reconstruct_loop_context`] for `kind == "for-each"`
/// segments. `item_total` is derived exactly, the same way live dispatch
/// derives it (`control_flow.rs`'s for-each entry): the length of the
/// accepted list held by the control item's declared `over` slot, evaluated
/// at `cutoff` — the same historical snapshot (`historical_slot_values`,
/// merged with `state`'s port values, since ports don't change during a run)
/// this activation's own inputs are reconstructed from. `over` is fixed for
/// the lifetime of a for-each (rewriting an active for-each's `over` slot is
/// rejected elsewhere), so this activation's own cutoff — strictly after the
/// for-each's entry — always sees the same list. When that evidence is
/// unavailable (no declared control item, no resolvable `over` value, or a
/// non-list value — e.g. a stale ledger from a since-edited trait), this
/// fails closed with `None` rather than emit an approximated `item_total` as
/// if it were exact.
fn reconstruct_for_each_context(
    trait_ref: &Trait,
    path: &[PathSegment],
    state: &State,
    historical_slot_values: &[Value],
) -> Option<ForEachContext> {
    let segment = path.iter().rev().find(|segment| segment.kind == "for-each")?;
    let sequence_id = segment.id.clone()?;
    let control_item = control_item_for_sequence(trait_ref, &sequence_id, SequenceKind::ForEach)?;
    let item_index = segment.item_index.unwrap_or(0);
    let over = control_item.over.as_deref()?;
    let item_total = state
        .accepted_port_values
        .iter()
        .chain(historical_slot_values.iter())
        .find(|value| value.ref_text == over && value.acceptance == AcceptanceStatus::Accepted)
        .and_then(|value| value.value.as_array())
        .map(|items| items.len())?;
    Some(ForEachContext {
        for_each_id: control_item
            .id
            .clone()
            .unwrap_or_else(|| sequence_id.clone()),
        item_index,
        item_total,
        max_items: control_item.max_items.unwrap_or(item_total),
    })
}

/// The declared sequence item (a `loop`/`for-each` control item, per `kind`)
/// whose `sequence` ref names `sequence_id` — searched across the top-level
/// procedure and every named sequence, since a control item can live at any
/// nesting depth.
fn control_item_for_sequence<'a>(
    trait_ref: &'a Trait,
    sequence_id: &str,
    kind: SequenceKind,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    let matches = |item: &crate::r#trait::procedure::SequenceItem| {
        item.effective_kind() == kind
            && item
                .sequence
                .as_deref()
                .and_then(|reference| Reference::parse(reference).ok())
                .is_some_and(|parsed| parsed.id() == sequence_id)
    };
    if let Some(proc) = trait_ref.procedure.as_ref()
        && let Some(item) = proc.sequence.iter().find(|item| matches(item)) {
            return Some(item);
        }
    trait_ref
        .sequences
        .iter()
        .find_map(|(_, sequence)| sequence.sequence.iter().find(|item| matches(item)))
}

fn item_for_status<'a>(
    trait_ref: &'a Trait,
    status: &SequenceStatus,
) -> Option<&'a crate::r#trait::procedure::SequenceItem> {
    if status.position_path.is_empty() {
        return trait_ref.procedure.as_ref()?.sequence.get(status.sequence_index);
    }
    let item_id = status.item_id.as_deref()?;
    trait_ref
        .sequences
        .iter()
        .find_map(|(_, sequence)| sequence.sequence.iter().find(|item| item.id.as_deref() == Some(item_id)))
}

/// The exclusive acceptance-order boundary of `ready`'s own activation: the
/// earliest slot write that activation itself made. When it made none (e.g. a
/// signal-only step with no slot output), fall back to the earliest write
/// made by the nearest later sibling at ANY ancestor scope — not just the
/// immediate parent — so a write from a different branch/loop/iteration
/// entirely still cannot leak into this activation's "before" view. Only
/// when no such evidence exists anywhere in the ledger (this activation is
/// the very last one reachable from the root) does this fall back to
/// [`usize::MAX`], treating the whole ledger as "before".
fn activation_cutoff(state: &State, ready: &ReadyItem<'_>) -> usize {
    let activation_path = producer_path_for_ready(ready);
    recorded_slot_revisions(state)
        .into_iter()
        .filter(|revision| revision.position_path == activation_path)
        .map(|revision| revision.acceptance_order)
        .min()
        .or_else(|| next_sibling_cutoff(state, &activation_path))
        .unwrap_or(usize::MAX)
}

/// The earliest acceptance order among writes from the nearest later sibling
/// of `activation_path`, checked at every ancestor scope depth (deepest
/// first) and reduced to the minimum found — not only the immediate parent.
///
/// A same-immediate-scope check alone misses a real leak: an activation that
/// is the last leaf of its own scope (e.g. the last step of a loop
/// iteration) has no immediate-scope sibling, but a later iteration of the
/// enclosing loop, or a later top-level step after the whole loop, is still
/// a write that happened after this activation and must not leak into its
/// "before" view. Scanning every ancestor level closes that gap: whichever
/// level actually holds a later sibling supplies the bound.
fn next_sibling_cutoff(state: &State, activation_path: &[PathSegment]) -> Option<usize> {
    (1..=activation_path.len())
        .rev()
        .filter_map(|depth| next_sibling_cutoff_at_depth(state, activation_path, depth))
        .min()
}

/// The earliest acceptance order among writes from a later sibling of
/// `activation_path[depth - 1]` — same ancestor prefix (`activation_path[..depth
/// - 1]`), same segment kind, a strictly greater index/iteration/item-index —
/// per [`next_sibling_cutoff`].
///
/// `id` is compared only at a depth whose segment names a container actually
/// shared by every child at that level — a `loop`/`branch`/`for-each`/
/// `sequence` frame pushed by [`path_for_nested_item`], where `id` is the
/// body's named-sequence id and is therefore identical for every sibling
/// inside it, so requiring a match there is what keeps this scan from
/// crossing into an unrelated container at the same depth.
///
/// Two segment kinds are *not* such a container and are excluded from the
/// `id` check even when they occur at an ancestor depth: `"item"` (the
/// leaf `path_for_nested_item` pushes, `depth == activation_path.len()`) and
/// `"procedure"` (either the sole segment `producer_path_for_ready` synthesizes
/// for a top-level activation, or the first segment `path_for_nested_item`
/// pushes for a nested one — in both cases this names the activated item's
/// *own* id, or a nested activation's enclosing top-level control item's own
/// id, never a ancestor container id, since a top-level activation's siblings
/// are other top-level items with their own distinct ids by construction).
/// Requiring `id` equality on a `"procedure"` segment at depth 1 meant a
/// nested activation's later top-level sibling — e.g. a plain step after the
/// loop the activation lives in — could never match, since the loop's own id
/// (the nested activation's depth-1 id) differs from that later step's id;
/// the scan silently fell through to `usize::MAX`, leaking that later write
/// into the "before" view. `kind` plus the strictly-greater ordering tuple is
/// sufficient for both excluded kinds, since only one item occupies a given
/// index within a scope regardless of that item's own id.
fn next_sibling_cutoff_at_depth(
    state: &State,
    activation_path: &[PathSegment],
    depth: usize,
) -> Option<usize> {
    let prefix = &activation_path[..depth - 1];
    let target = &activation_path[depth - 1];
    let is_container_segment = !matches!(target.kind.as_str(), "procedure" | "item");
    let target_order = (target.index, target.iteration.unwrap_or(0), target.item_index.unwrap_or(0));
    recorded_slot_revisions(state)
        .into_iter()
        .filter(|revision| {
            if revision.position_path.len() < depth {
                return false;
            }
            let candidate = &revision.position_path[depth - 1];
            revision.position_path[..depth - 1] == *prefix
                && candidate.kind == target.kind
                && (!is_container_segment || candidate.id == target.id)
                && (
                    candidate.index,
                    candidate.iteration.unwrap_or(0),
                    candidate.item_index.unwrap_or(0),
                ) > target_order
        })
        .map(|revision| revision.acceptance_order)
        .min()
}

/// The accepted value of every slot that had one before `cutoff` and was
/// visible from `position_path`, replaying each slot's latest qualifying
/// revision through the same write-operation semantics live dispatch applies.
/// This includes the activation's own outer/inner parallel buffers but never
/// held sibling branches or post-cutoff values from the active buffer.
fn accepted_slot_values_before(
    state: &State,
    cutoff: usize,
    position_path: &[PathSegment],
) -> Vec<Value> {
    let mut latest: BTreeMap<String, &SlotRevision> = BTreeMap::new();
    for revision in recorded_slot_revisions(state) {
        if revision.acceptance_order >= cutoff
            || !revision_visible_at_decision(&revision.position_path, position_path)
        {
            continue;
        }
        latest
            .entry(revision.slot_ref.to_string())
            .and_modify(|current| {
                if revision.acceptance_order > current.acceptance_order {
                    *current = revision;
                }
            })
            .or_insert(revision);
    }
    latest
        .into_iter()
        .filter_map(|(ref_text, revision)| {
            let operation = revision.operation.clone().unwrap_or(WriteOperation::Replace);
            let submitted = revision.submitted_payload.as_ref()?.value.clone();
            let prior = revision.prior_value.as_ref().map(|value| &value.value);
            let value = apply_write_operation_value(&operation, prior, &submitted).ok()?;
            let digest = value_digest(&value).ok()?;
            Some(Value {
                ref_text,
                value,
                value_digest: digest,
                schema_ref: None,
                // Preserve the write's true original source (e.g.
                // `model-output`, `manual-output`) instead of fabricating
                // `Ledger` — only ledgers written before `SlotRevision.source`
                // existed fall back to the historical placeholder.
                source: revision.source.clone().unwrap_or(ValueSource::Ledger),
                producer_evidence: None,
                command_execution: revision.command_execution.clone(),
                producer_agent: None,
                producer_harness: None,
                producer_check_verdict: false,
                acceptance: AcceptanceStatus::Accepted,
                schema_validation: Vec::new(),
            })
        })
        .collect()
}

fn missing_required_procedure_ports(trait_ref: &Trait, state: &State) -> Vec<String> {
    let Some(proc) = trait_ref.procedure.as_ref() else {
        return Vec::new();
    };
    let input_ports: BTreeMap<&str, &crate::r#trait::Port> = trait_ref
        .ports
        .iter()
        .filter(|port| matches!(port.direction, PortDirection::Input))
        .map(|port| (port.id.as_str(), port))
        .collect();
    let mut missing = Vec::new();
    for ref_text in proc.input.iter() {
        let Ok(parsed) = Reference::parse(ref_text) else {
            continue;
        };
        let Some(port) = input_ports.get(parsed.id()) else {
            continue;
        };
        if port.optional {
            continue;
        }
        if accepted_value(state, ref_text).is_none() {
            missing.push(ref_text.clone());
        }
    }
    missing.sort();
    missing
}

fn missing_inputs_for_item(
    trait_ref: &Trait,
    ready: &ReadyItem<'_>,
    state: &State,
) -> crate::Result<Vec<String>> {
    let active: BTreeSet<&str> = active_input_refs(trait_ref, state, ready)?
        .into_iter()
        .collect();
    let (refs, hard_slots) = hard_input_refs_for_item(ready.item, &active);
    let mut missing = missing_inputs_for_refs(trait_ref, &refs, state);
    missing.extend(missing_hard_slot_refs(&hard_slots, state));
    missing.sort();
    missing.dedup();
    Ok(missing)
}

fn dependency_capabilities_for_item(
    item: &crate::r#trait::procedure::SequenceItem,
) -> Vec<CapabilityReport> {
    let mut reports = Vec::new();
    for input in item.input.iter() {
        if input.is_optional() {
            continue;
        }
        let ref_text = input.ref_text();
        if Reference::parse(ref_text)
            .is_ok_and(|parsed| parsed.is_qualified() && parsed.kind() != Kind::Resource)
        {
            reports.push(CapabilityReport::unsupported(
                "runtime.dependency-qualified-ref",
                format!(
                    "dependency-qualified ref {ref_text:?} requires dependency loading evidence"
                ),
            ));
        }
    }
    for ref_text in item.emits.iter().map(|emit| emit.signal_ref()) {
        if Reference::parse(ref_text)
            .is_ok_and(|parsed| parsed.is_qualified() && parsed.kind() != Kind::Resource)
        {
            reports.push(CapabilityReport::unsupported(
                "runtime.dependency-qualified-ref",
                format!(
                    "dependency-qualified ref {ref_text:?} requires dependency loading evidence"
                ),
            ));
        }
    }
    if item.cmd.is_none()
        && item.command.is_none()
        && classify_prompt(&item.prompt).is_ok_and(|classification| {
            matches!(classification, PromptClassification::DependencyPromptRef(_))
        })
    {
        reports.push(CapabilityReport::unsupported(
            "runtime.dependency-prompt",
            "dependency-qualified prompt refs require dependency loading evidence",
        ));
    }
    reports.sort();
    reports.dedup();
    reports
}

fn command_frame(
    item: &crate::r#trait::procedure::SequenceItem,
    plan: &CommandPlan,
    state: &State,
) -> crate::Result<CommandFrame> {
    let (argv, resource_argv) = if let Some(argv_from) = plan.argv_from.as_deref() {
        let value = accepted_value(state, argv_from).ok_or_else(|| {
            crate::procedure::invalid_field(
                "runtime.command.argv-from",
                format!("accepted argv input {argv_from:?} is missing"),
            )
        })?;
        let items = value.value.as_array().ok_or_else(|| {
            crate::procedure::invalid_field(
                "runtime.command.argv-from",
                format!("accepted argv input {argv_from:?} is not a list"),
            )
        })?;
        // Retired (P516, 2026-07-26): a dynamic argv-from value that happens
        // to read `{resource:x}` is caller data, not authored code, and is
        // refused rather than left as silent inert text — see the retirement
        // note on `command_contract::unpinned_command_resource_argv`.
        let argv = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    crate::procedure::invalid_field(
                        format!("runtime.command.argv-from[{index}]"),
                        "dynamic argv items must be strings",
                    )
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;
        if let Some((index, resource_id)) =
            crate::r#trait::procedure::whole_token_resource_argv_refs(&argv).into_iter().next()
        {
            return Err(crate::procedure::invalid_field(
                format!("runtime.command.argv-from[{index}]"),
                format!(
                    "dynamic argv value {{resource:{resource_id}}} is never a resource reference; \
                     use a literal `argv` with `{{resource:{resource_id}}}` in position 0 alongside \
                     `{{port:...}}`/`{{slot:...}}` arguments instead"
                ),
            ));
        }
        (argv, Vec::new())
    } else {
        (resolve_command_argv(&plan.argv, state), collect_resource_argv(&plan.argv))
    };
    crate::r#trait::procedure::validate_command_argv(&argv, "runtime.command.argv")?;
    let executable_digest = plan
        .executable_digest_from
        .as_deref()
        .map(|ref_text| {
            let value = accepted_value(state, ref_text).ok_or_else(|| {
                crate::procedure::invalid_field(
                    "runtime.command.executable-digest-from",
                    format!("accepted executable digest input {ref_text:?} is missing"),
                )
            })?;
            let digest = value.value.as_str().ok_or_else(|| {
                crate::procedure::invalid_field(
                    "runtime.command.executable-digest-from",
                    format!("accepted executable digest input {ref_text:?} is not text"),
                )
            })?;
            Digest::parse(digest)
        })
        .transpose()?;
    if executable_digest.is_some()
        && !std::path::Path::new(argv.first().map_or("", String::as_str)).is_absolute()
    {
        return Err(crate::procedure::invalid_field(
            "runtime.command.argv[0]",
            "digest-verified command executable must resolve to an absolute path",
        ));
    }
    Ok(CommandFrame {
        cmd: item.cmd.clone(),
        argv,
        executable_digest,
        resource_argv,
        cwd: plan.cwd.clone(),
        timeout_ms: plan.timeout_ms,
        capture_bytes: plan.capture_bytes,
        success_exit_code: plan.success_exit_code.clone(),
        output_slot: item
            .output
            .iter()
            .next()
            .map(|sink| sink.ref_text().to_string())
            .unwrap_or_else(|| "slot:command-output".to_string()),
        permission_code: "blocked-command-permission-required".to_string(),
        reason: "command steps run only when the controlled runtime reaches this frame and command permission explicitly allows the argv".to_string(),
    })
}

pub(crate) fn command_execution_succeeded(
    evidence: &CommandExecutionEvidence,
    command: &CommandFrame,
) -> bool {
    !evidence.timed_out
        && evidence.exit_code.is_some_and(|exit_code| {
            if command.success_exit_code.is_empty() {
                exit_code == 0
            } else {
                command.success_exit_code.contains(&exit_code)
            }
        })
}

/// Resolve `{slot:x}`/`{port:x}` interpolation tokens in each argv item against
/// accepted runtime values, producing an already-literal argv the IO edge runs
/// verbatim.
///
/// Resolution is a pure function of `state`: a token is replaced only when the
/// referenced local slot/port has an accepted value, and the rendered value is
/// inserted as part of a single argv element (never re-scanned). Text stays
/// text; numbers and booleans use their canonical scalar form; object/list/null
/// values use compact JSON. A substituted value such as `; rm -rf /` therefore
/// stays one literal argument. Tokens without an accepted value, and braces
/// that are not `{slot:x}`/`{port:x}` refs, are copied through unchanged —
/// token-free argv is returned byte-for-byte identical.
fn resolve_command_argv(argv: &[String], state: &State) -> Vec<String> {
    argv.iter()
        .map(|arg| resolve_command_argv_item(arg, state))
        .collect()
}

/// Extract authored `{resource:<id>}` argv positions from literal command
/// argv, reusing the same whole-token/local/unqualified definition
/// `command_contract` statically validates against
/// ([`crate::r#trait::procedure::whole_token_resource_argv_refs`]), so frame
/// building can never disagree with what validation actually accepted. It
/// never inspects `argv-from` or a resolved argv value, so a dynamic string
/// cannot be reinterpreted as a resource ref.
fn collect_resource_argv(argv: &[String]) -> Vec<ResourceArgvRef> {
    crate::r#trait::procedure::whole_token_resource_argv_refs(argv)
        .into_iter()
        .map(|(index, resource_id)| ResourceArgvRef {
            index,
            resource_ref: format!("resource:{resource_id}"),
        })
        .collect()
}

/// Substitute the interpolation spans [`scan_interpolations`] recognizes in a
/// single argv element.
///
/// Reuses the shared scanner rather than re-parsing the brace grammar here,
/// so runtime substitution and `command_contract` validation always agree on
/// which spans are interpolations: shell-like forms the scanner treats as
/// diagnostics-only (`${...}`, `` `...` ``, `{{...}}`) stay untouched at
/// runtime too, instead of a divergent hand-rolled parser substituting text
/// validation never required as input.
fn resolve_command_argv_item(arg: &str, state: &State) -> String {
    let (interpolations, _diagnostics) = scan_interpolations(arg);
    if interpolations.is_empty() {
        return arg.to_string();
    }
    let chars: Vec<char> = arg.chars().collect();
    let mut out = String::new();
    let mut cursor = 0;
    for interp in &interpolations {
        out.extend(chars[cursor..interp.start].iter());
        match resolve_argv_ref_value(&interp.ref_text, state) {
            Some(value) => out.push_str(&value),
            None => out.extend(chars[interp.start..interp.end].iter()),
        }
        cursor = interp.end;
    }
    out.extend(chars[cursor..].iter());
    out
}

/// Render the accepted value for a local `slot:`/`port:` interpolation body,
/// or `None` when the body is not such a ref or has no accepted value.
fn resolve_argv_ref_value(body: &str, state: &State) -> Option<String> {
    let parsed = Reference::parse(body).ok()?;
    if parsed.is_qualified() || !matches!(parsed.kind(), Kind::Slot | Kind::Port) {
        return None;
    }
    let value = accepted_value(state, body)?;
    render_argv_value(&value.value)
}

/// Canonical single-token rendering: text as-is, number/boolean canonical, and
/// every non-scalar as compact JSON.
fn render_argv_value(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Number(number) => Some(number.to_string()),
        JsonValue::Bool(boolean) => Some(boolean.to_string()),
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => {
            serde_json::to_string(value).ok()
        }
    }
}

fn prompt_evidence(
    trait_ref: &Trait,
    item: &crate::r#trait::procedure::SequenceItem,
) -> crate::Result<Option<PromptEvidence>> {
    let classification = classify_prompt(&item.prompt)
        .map_err(|message| crate::procedure::invalid_field("procedure.sequence.prompt", message))?;
    match classification {
        PromptClassification::Inline => Ok(Some(prompt_evidence_from_text(
            "inline",
            None,
            &item.prompt,
        ))),
        PromptClassification::LocalPromptRef(parsed) => {
            let Some(prompt) = trait_ref.prompts.get(parsed.id()) else {
                return Ok(Some(PromptEvidence {
                    source: "local-prompt-ref-missing".to_string(),
                    prompt_ref: Some(parsed.clone()),
                    digest: Digest::source(parsed.as_ref()),
                    interpolations: Vec::new(),
                }));
            };
            if let Some(text) = prompt.text.as_deref() {
                Ok(Some(prompt_evidence_from_text(
                    "local-prompt-ref",
                    Some(parsed.clone()),
                    text,
                )))
            } else if let Some(source) = prompt.source.as_deref() {
                Ok(Some(PromptEvidence {
                    source: "resource-backed-prompt".to_string(),
                    prompt_ref: Some(parsed.clone()),
                    digest: Digest::source(source),
                    interpolations: vec![source.to_string()],
                }))
            } else {
                Ok(None)
            }
        }
        PromptClassification::DependencyPromptRef(parsed) => Ok(Some(PromptEvidence {
            source: "dependency-pending".to_string(),
            prompt_ref: Some(parsed.clone()),
            digest: Digest::source(parsed.as_ref()),
            interpolations: Vec::new(),
        })),
    }
}

fn prompt_evidence_from_text(
    source: &str,
    prompt_ref: Option<Reference>,
    text: &str,
) -> PromptEvidence {
    let (interpolations, diagnostics) = scan_interpolations(text);
    let mut refs: Vec<String> = interpolations
        .into_iter()
        .map(|interp| interp.ref_text)
        .collect();
    refs.extend(diagnostics);
    refs.sort();
    PromptEvidence {
        source: source.to_string(),
        prompt_ref,
        digest: Digest::source(text),
        interpolations: refs,
    }
}

fn slot_schema_ref(trait_ref: &Trait, slot_ref: &str) -> Option<String> {
    let parsed = Reference::parse(slot_ref).ok()?;
    if parsed.kind() != Kind::Slot || parsed.is_qualified() {
        return None;
    }
    trait_ref
        .slots
        .iter()
        .find(|slot| slot.id == parsed.id())
        .and_then(|slot| slot.schema.as_ref().map(ToString::to_string))
}

fn output_schema_ref(trait_ref: &Trait, ref_text: &str) -> Option<String> {
    let parsed = Reference::parse(ref_text).ok()?;
    match parsed.kind() {
        Kind::Slot => slot_schema_ref(trait_ref, ref_text),
        Kind::Port if !parsed.is_qualified() => trait_ref
            .ports
            .iter()
            .find(|port| port.id == parsed.id() && matches!(port.direction, PortDirection::Output))
            .map(|port| port.schema.clone()),
        Kind::Schema if !parsed.is_qualified() => Some(ref_text.to_string()),
        _ => None,
    }
}

fn input_schema_ref(trait_ref: &Trait, ref_text: &str) -> Option<String> {
    let parsed = Reference::parse(ref_text).ok()?;
    match parsed.kind() {
        Kind::Slot => slot_schema_ref(trait_ref, ref_text),
        Kind::Port if !parsed.is_qualified() => trait_ref
            .ports
            .iter()
            .find(|port| port.id == parsed.id())
            .map(|port| port.schema.clone()),
        Kind::Schema if !parsed.is_qualified() => Some(ref_text.to_string()),
        _ => None,
    }
}

/// Resolve `ref_text`'s currently accepted value: a global port value, then —
/// innermost first — the isolation buffer of every active `parallel` frame
/// on the stack, then the committed ledger. A branch's own not-yet-merged
/// writes are visible to its own reads; a sibling (including a completed but
/// not-yet-barriered) branch's buffer never is, since only frames actually on
/// `control_stack` are consulted here.
fn accepted_value<'a>(state: &'a State, ref_text: &str) -> Option<&'a Value> {
    let matches = |value: &&Value| {
        value.ref_text == ref_text && value.acceptance == AcceptanceStatus::Accepted
    };
    if let Some(value) = state.accepted_port_values.iter().find(matches) {
        return Some(value);
    }
    for frame in state.control_stack.iter().rev() {
        if frame.kind == ControlKind::Parallel
            && let Some(value) = frame.parallel_buffer.accepted_slot_values.iter().find(matches) {
                return Some(value);
            }
    }
    state.accepted_slot_values.iter().find(matches)
}

fn accepted_resource<'a>(state: &'a State, ref_text: &str) -> Option<&'a ResourceEvidence> {
    state
        .resource_evidence
        .iter()
        .find(|resource| resource.resource_ref.as_str() == ref_text && resource.available)
}

fn is_soft_local_slot_ref(ref_text: &str) -> bool {
    Reference::parse(ref_text)
        .is_ok_and(|parsed| parsed.kind() == Kind::Slot && !parsed.is_qualified())
}

fn frame_input_from_value(trait_ref: &Trait, value: &Value) -> FrameInput {
    FrameInput {
        ref_text: value.ref_text.clone(),
        value_digest: value.value_digest.clone(),
        schema_ref: input_schema_ref(trait_ref, &value.ref_text)
            .or_else(|| value.schema_ref.as_ref().map(ToString::to_string)),
        source: value.source.clone(),
        // Surfacing producer evidence downstream is a P274 addition for
        // check verdicts specifically — a repair step needs the captured
        // stdout/stderr to diagnose a failing check. Host/manual/ordinary
        // command values already carry non-empty producer_evidence for
        // unrelated reasons (initial input provenance, caller identity);
        // surfacing it for every value here would change existing frames'
        // `--json` bytes for traits that predate P274. `producer_check_verdict`
        // is stamped once at accept time from the runtime item that actually
        // produced the value, so this is correct regardless of how deeply the
        // check is nested inside loop/branch/parallel bodies.
        producer_evidence: if value.producer_check_verdict {
            value.producer_evidence.clone()
        } else {
            None
        },
    }
}

fn upsert_runtime_value(values: &mut Vec<Value>, value: Value) {
    if let Some(existing) = values
        .iter_mut()
        .find(|existing| existing.ref_text == value.ref_text)
    {
        *existing = value;
    } else {
        values.push(value);
    }
}

struct SlotRevisionWrite<'a> {
    operation: WriteOperation,
    submitted_payload: JsonValue,
    prior_value: Option<&'a Value>,
    runtime_binding: bool,
    projection: Option<ProjectionProvenance>,
}

struct SlotRevisionContext<'a> {
    acceptance_order: usize,
    position_path: &'a [PathSegment],
    loop_context: Option<&'a LoopContext>,
    for_each_context: Option<&'a ForEachContext>,
}

fn slot_revision_from_value(
    value: &Value,
    write: SlotRevisionWrite<'_>,
    context: SlotRevisionContext<'_>,
) -> crate::Result<SlotRevision> {
    Ok(SlotRevision {
        slot_ref: Reference::parse(&value.ref_text)?,
        value_digest: value.value_digest.clone(),
        acceptance_order: context.acceptance_order,
        operation: Some(write.operation),
        submitted_payload: Some(RevisionValue {
            value: write.submitted_payload,
        }),
        prior_value_digest: write
            .prior_value
            .map(|prior| prior.value_digest.clone()),
        prior_value: write.prior_value.map(|prior| RevisionValue {
            value: prior.value.clone(),
        }),
        source: Some(value.source.clone()),
        command_execution: value.command_execution.clone(),
        runtime_binding: write.runtime_binding,
        projection: write.projection,
        position_path: context.position_path.to_vec(),
        loop_id: context
            .loop_context
            .map(|context| context.loop_id.clone()),
        iteration_index: context
            .loop_context
            .map(|context| context.iteration_index),
        for_each_id: context
            .for_each_context
            .map(|context| context.for_each_id.clone()),
        item_index: context
            .for_each_context
            .map(|context| context.item_index),
    })
}

fn reject_envelope(report: &mut StepValidationReport, sequence_index: usize, reason: String) {
    report.rejected_outputs.push(RejectedAttempt {
        sequence_index,
        position_path: Vec::new(),
        ref_text: None,
        value_digest: None,
        reason,
    });
}

fn set_current_outer_status(
    state: &mut State,
    status_kind: SequenceStatusKind,
    reason: impl Into<String>,
) {
    let current = state.current_run_index;
    if let Some(status) = state
        .sequence_statuses
        .iter_mut()
        .find(|status| status.run_index == current && status.position_path.is_empty())
    {
        status.status = status_kind;
        status.reason = reason.into();
    }
}

fn set_path_sequence_status(state: &mut State, status: SequenceStatus) {
    if status.position_path.is_empty() {
        set_sequence_status(state, status.sequence_index, status.status, status.reason);
        return;
    }
    if let Some(existing) = state
        .sequence_statuses
        .iter_mut()
        .find(|existing| existing.position_path == status.position_path)
    {
        existing.status = status.status;
        existing.reason = status.reason;
        return;
    }
    state.sequence_statuses.push(status);
}

/// Update the top-level status for `sequence_index` (a declaration index into
/// `[[procedure.sequence]]`). Every caller of this function targets a
/// top-level item, so the match must require an empty `position_path` —
/// nested per-branch/per-iteration entries reuse small local indices (a
/// `parallel` branch's own step position, a loop body's step position) that
/// collide with top-level declaration indices, and without this guard the
/// first colliding nested entry silently absorbs the update instead of the
/// intended top-level one, leaving it stuck at its prior status.
fn set_sequence_status(
    state: &mut State,
    sequence_index: usize,
    status_kind: SequenceStatusKind,
    reason: impl Into<String>,
) {
    if let Some(status) = state
        .sequence_statuses
        .iter_mut()
        .find(|status| status.sequence_index == sequence_index && status.position_path.is_empty())
    {
        status.status = status_kind;
        status.reason = reason.into();
    }
}

fn value_digest(value: &JsonValue) -> crate::Result<Digest> {
    let text = serde_json::to_string(value)
        .map_err(|e| crate::procedure::serialization("runtime.value", "runtime value", e))?;
    Ok(Digest::source(&text))
}

fn format_path(path: &[PathSegment]) -> String {
    if path.is_empty() {
        return "procedure".to_string();
    }
    path.iter()
        .map(|segment| {
            let id = segment
                .id
                .as_deref()
                .map(|id| format!(":{id}"))
                .unwrap_or_default();
            let iteration = segment
                .iteration
                .map(|iteration| format!("#iteration:{iteration}"))
                .unwrap_or_default();
            let item_index = segment
                .item_index
                .map(|item_index| format!("#item:{item_index}"))
                .unwrap_or_default();
            format!(
                "{}{}[{}]{}{}",
                segment.kind, id, segment.index, iteration, item_index
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn format_optional_str(value: Option<&str>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "absent".to_string())
}

fn format_optional_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "absent".to_string())
}

fn final_state_label(final_state: &FinalState) -> &'static str {
    match final_state {
        FinalState::Running => "running",
        FinalState::Blocked => "blocked",
        FinalState::Completed => "completed",
        FinalState::Failed => "failed",
        FinalState::Rejected => "rejected",
    }
}

fn bounded(mut text: String) -> String {
    if text.len() <= FRAME_TEXT_LIMIT {
        return text;
    }
    let cut = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= FRAME_TEXT_LIMIT)
        .last()
        .map_or(0, |index| index);
    text.truncate(cut);
    text.push_str("\n[frame truncated]\n");
    text
}

fn sort_state(state: &mut State) {
    state
        .sequence_statuses
        .sort_by_key(|status| status.run_index);
    state
        .accepted_port_values
        .sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    state
        .accepted_slot_values
        .sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    state
        .accepted_output_port_values
        .sort_by(|a, b| a.ref_text.cmp(&b.ref_text));
    state
        .resource_evidence
        .sort_by(|a, b| a.resource_ref.cmp(&b.resource_ref));
    state.emitted_signals.sort_by(|a, b| {
        a.sequence_index
            .cmp(&b.sequence_index)
            .then(a.signal_ref.cmp(&b.signal_ref))
            .then(a.evidence_digest.cmp(&b.evidence_digest))
    });
    state.rejected_attempts.sort_by(|a, b| {
        a.sequence_index
            .cmp(&b.sequence_index)
            .then(a.ref_text.cmp(&b.ref_text))
            .then(a.reason.cmp(&b.reason))
    });
    state.provider_capability_reports.sort();
    state.provider_capability_reports.dedup();
    state
        .output_ports
        .sort_by(|a, b| a.port_ref.cmp(&b.port_ref));
}
