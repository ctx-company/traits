//! `ctx traits story <run-id>` (P383): renders a persisted run-session ledger
//! as a chronological narrative. Read-only — resolves the ledger and,
//! best-effort, the trait's dry plan for producer-role enrichment, then hands
//! both to the pure `ctx_traits_core::procedure::story` builder. Never
//! mutates the ledger, never calls a provider, never reads the clock.

mod assisted;

use camino::Utf8PathBuf;

use ctx_traits_core::procedure::runtime::FinalState;
use ctx_traits_core::procedure::session::{MergeFrame, MergeStatus, Session};
use ctx_traits_core::procedure::story::{
    self, StoryBeat, StoryLevel, StoryReport, StorySpine, ValueGloss,
};
use ctx_traits_core::response::CommandOutput;

use crate::app::command_handlers::print_json_report;
use crate::app::merge_story::{Explanation, GateRow, explain_frame, gate_rows, stage_sentence};
use crate::app::run_format::render_runtime_path;
use crate::app::run_view::{session_status, stop_reason_summary};
use crate::app::tui::{self, Line, Tone, labeled_line, write_plain_line as w};

pub(crate) fn handle_story(
    run: &str,
    session_store: Option<&str>,
    json: bool,
    markdown: bool,
    level: Option<StoryLevel>,
) -> crate::Result<CommandOutput<()>> {
    if json && markdown {
        return Err(crate::Error::Command {
            message: "ctx traits story: --json and --markdown are mutually exclusive".to_string(),
        });
    }
    let level = level.unwrap_or(StoryLevel::Default);

    let (ledger_path, session) = resolve_run(run, session_store)?;
    let plan = load_plan(&session);
    let activity = load_activity(&ledger_path);
    let report = if level.spends_model_call() {
        assisted::narrate(
            story::build(&session, plan.as_ref(), activity.as_ref()),
            &session,
        )
    } else {
        story::build(&session, plan.as_ref(), activity.as_ref())
    };

    if json {
        print_json_report(&report, "story report")?;
        return Ok(CommandOutput::new(()));
    }
    if markdown {
        for line in render_markdown(&session, &report, level) {
            w(line)?;
        }
        return Ok(CommandOutput::new(()));
    }
    tui::emit_report(
        false,
        || story_document(&session, &report, level),
        || render_plain(&session, &report, level),
    )?;
    Ok(CommandOutput::new(()))
}

/// Plain-language sentence naming this run's disposition — the FIRST content
/// line in every render path (styled/plain/markdown/viewer), P550. Built
/// from [`final_state_text`]/[`stop_reason_summary`] and, when the run's last
/// action was landing, the terminal merge frame's own explanation, so a
/// parked or failed landing never reads like an unqualified success just
/// because the arc of accepted beats stopped cleanly.
pub(crate) fn disposition_sentence(session: &Session, report: &StoryReport) -> String {
    if let Some(landing) = landing_frames(&report.merge_frames)
        .iter()
        .rev()
        .find(|landing| landing.outcome.is_some())
    {
        let explanation = landing
            .outcome
            .as_ref()
            .expect("filtered to Some outcome above");
        return match landing.frame.status {
            MergeStatus::Merged => "This run completed and merged to main.".to_string(),
            MergeStatus::Parked => {
                format!("This run parked while landing: {}", explanation.sentence)
            }
            MergeStatus::PostMergeCleanupFailure => format!(
                "This run's merge landed, but cleanup afterward failed: {}",
                explanation.sentence
            ),
            MergeStatus::RecoveryFailure => format!(
                "This run's landing could not be confirmed: {}",
                explanation.sentence
            ),
            MergeStatus::LockAcquired | MergeStatus::GatesPassed | MergeStatus::Reconciled => {
                unreachable!(
                    "landing.outcome is Some only at the last terminal frame (is_terminal); \
                     these variants are never terminal"
                )
            }
        };
    }

    let stop = stop_reason_summary(session);
    match (&report.final_state, stop) {
        (FinalState::Completed, _) => "This run completed.".to_string(),
        (FinalState::Blocked, Some(summary)) => format!("This run is blocked: {summary}."),
        (FinalState::Blocked, None) => "This run is blocked.".to_string(),
        (FinalState::Failed, Some(summary)) => format!("This run failed: {summary}."),
        (FinalState::Failed, None) => "This run failed.".to_string(),
        (FinalState::Rejected, Some(summary)) => format!("This run was rejected: {summary}."),
        (FinalState::Rejected, None) => "This run was rejected.".to_string(),
        (FinalState::Running, Some(summary)) => format!("This run stopped in progress: {summary}."),
        (FinalState::Running, None) => "This run is still in progress.".to_string(),
    }
}

/// The one honest-degradation notice line (P550/P521): `detailed` degrades
/// only when no activity sidecar was found for this ledger at all (a ledger
/// predating P521, or a concurrent-wave frame — P504's wave dispatch does
/// not attach activity observers today); `assisted` additionally degrades
/// when no narrator seat resolved offline, or a per-run request could not
/// spend at all. Never silently renders `Default` content as though the
/// request had been honored.
pub(crate) fn degrade_notice(level: StoryLevel, report: &StoryReport) -> Option<String> {
    match level {
        StoryLevel::Default => None,
        StoryLevel::Detailed => match report.activity_provenance {
            story::ActivityProvenance::Absent => {
                Some("no activity was recorded for this run".to_string())
            }
            story::ActivityProvenance::Partial { skipped_lines } => Some(format!(
                "activity partially recorded for this run ({skipped_lines} unparseable line(s) skipped)"
            )),
            story::ActivityProvenance::Recorded => None,
        },
        StoryLevel::Assisted => {
            if let story::ActivityProvenance::Absent = report.activity_provenance {
                return Some("no activity was recorded for this run".to_string());
            }
            report.assisted_unavailable.clone()
        }
    }
}

/// Resolve `run` the same way `ctx traits merge <run-id>` does: first as an
/// internal run-id (scanning the session store), then — since `story` is
/// useful against any persisted ledger, not only completed `--worktree`
/// runs — as a full session ID, an unambiguous session-ID prefix, or an
/// explicit ledger path via `resolve_session_path`.
pub(crate) fn resolve_run(
    run: &str,
    session_store: Option<&str>,
) -> crate::Result<(Utf8PathBuf, Session)> {
    if let Some(resolved) = ctx_traits_io::run_session::find_session_by_run_id(session_store, run)?
    {
        return Ok(resolved);
    }
    let path = ctx_traits_io::run_session::resolve_session_path(run, session_store)?;
    let session = ctx_traits_io::run_session::read_run_session(&path)?;
    Ok((path, session))
}

/// Load and tolerantly parse `ledger_path`'s activity sidecar (P521),
/// converting `ctx_traits_io::activity_sidecar::ActivityRecord`s into the
/// core-typed `ActivityInput` `story::build` accepts — core does no IO, so
/// this conversion is the CLI/IO boundary's own job. `None` when no sidecar
/// exists at all (a ledger predating P521, or one that never dispatched
/// through the CLI's non-wave path); an existing-but-partially-corrupt
/// sidecar still returns `Some`, with `skipped_lines` set.
pub(crate) fn load_activity(
    ledger_path: &camino::Utf8Path,
) -> Option<ctx_traits_core::procedure::story::ActivityInput> {
    if !ctx_traits_io::activity_sidecar::activity_exists(ledger_path) {
        return None;
    }
    let (records, skipped_lines) = ctx_traits_io::activity_sidecar::read_activity(ledger_path);
    let mut events = Vec::new();
    let mut step_summaries = Vec::new();
    for record in records {
        match record {
            ctx_traits_io::activity_sidecar::ActivityRecord::Activity { at_epoch_ms, event } => {
                events.push(ctx_traits_core::procedure::story::TimedActivityEvent {
                    at_epoch_ms,
                    event,
                });
            }
            ctx_traits_io::activity_sidecar::ActivityRecord::StepSummary {
                at_epoch_ms,
                key,
                role,
                text,
            } => {
                step_summaries.push(ctx_traits_core::procedure::story::TimedStepSummary {
                    at_epoch_ms,
                    key,
                    role,
                    text,
                });
            }
        }
    }
    Some(ctx_traits_core::procedure::story::ActivityInput {
        events,
        step_summaries,
        skipped_lines,
    })
}

/// Best-effort trait/plan resolution for producer-role enrichment (§3.3):
/// degrades to `None` on any failure (moved source, drifted digest, missing
/// agents) rather than failing the whole report — the story still renders in
/// full from the ledger alone, just with `actor: "unrecorded"`.
pub(crate) fn load_plan(session: &Session) -> Option<ctx_traits_core::procedure::run::Plan> {
    let loaded = ctx_traits_io::run::load_trait_for_session(None, None, session, "story").ok()?;
    ctx_traits_core::procedure::run::plan_procedure_run(&loaded.trait_ref, session.run_id.clone())
        .ok()
}

fn spine_text(spine: StorySpine) -> &'static str {
    match spine {
        StorySpine::SlotRevisions => "slot-revisions",
        // Deliberately does not claim "legacy ledger": an empty
        // `slot-revisions` also occurs on a current-schema ledger that has
        // simply accepted nothing yet, and that is not a legacy fact (P383
        // review round 1, blocker
        // `status-fallback-fabricates-unexecuted-beats`).
        StorySpine::SequenceStatuses => "sequence-statuses (no slot-revision evidence)",
    }
}

fn final_state_text(state: &ctx_traits_core::procedure::runtime::FinalState) -> &'static str {
    use ctx_traits_core::procedure::runtime::FinalState;
    match state {
        FinalState::Running => "running",
        FinalState::Blocked => "blocked",
        FinalState::Completed => "completed",
        FinalState::Failed => "failed",
        FinalState::Rejected => "rejected",
    }
}

fn summary_source_text(source: Option<story::SummaryLineSource>) -> &'static str {
    match source {
        Some(story::SummaryLineSource::StepSummary) => "step-summary",
        Some(story::SummaryLineSource::TypedOutput) => "typed-output",
        Some(story::SummaryLineSource::Derived) => "derived",
        None => "unrecorded",
    }
}

fn gloss_text(gloss: &ValueGloss) -> String {
    let mut parts = Vec::new();
    if let Some(status) = &gloss.status {
        parts.push(format!("status={status}"));
    }
    if !gloss.blockers.is_empty() {
        parts.push(format!("blockers=[{}]", gloss.blockers.join(", ")));
    }
    if let Some(escalation) = &gloss.escalation {
        parts.push(format!("escalation={escalation}"));
    }
    if let Some(advisory) = &gloss.advisory {
        parts.push(format!("advisory={advisory:?}"));
    }
    if let Some(generic) = &gloss.generic {
        if let Some(preview) = &generic.preview {
            parts.push(format!("preview={preview:?}"));
        }
        parts.push(format!("bytes={}", generic.byte_len));
        if let Some(count) = generic.element_count {
            parts.push(format!("elements={count}"));
        }
    }
    if parts.is_empty() {
        "(empty)".to_string()
    } else {
        parts.join(" ")
    }
}

/// One line of `$ argv — exit evidence`. `argv` is bounded through the same
/// [`story::bounded`]/[`story::GLOSS_CHAR_BOUND`] rule every value gloss
/// already uses — a `git commit -m <message>` beat's argv can otherwise
/// carry an entire multi-paragraph commit body, wrapping one "line" across
/// the terminal (styled) or breaking a `|`-delimited table row (markdown)
/// (P383 review round 1, blocker `unbounded-command-argv-in-render`).
/// `--json` is unaffected — it serializes `beat.command` directly, never
/// this rendered text.
fn beat_command_text(beat: &StoryBeat) -> Option<String> {
    let command = beat.command.as_ref()?;
    let result = match command.exit_code {
        Some(0) => "pass".to_string(),
        Some(code) => format!("fail exit={code}"),
        None => "fail (no exit code)".to_string(),
    };
    let timed_out = if command.timed_out { " timed-out" } else { "" };
    let argv = command.argv.join(" ").replace(['\n', '\r'], " ");
    Some(format!("$ {} — {result}{timed_out}", story::bounded(&argv)))
}

/// One merge frame as it should render in every mode: the terminal
/// (`Merged`/`Parked`/the two non-park terminal failures) frame gets its
/// full [`Explanation`]; every other, necessarily non-terminal, frame gets a
/// plain progression row instead — `explain_frame` is only ever meant to be
/// called on the last terminal frame (see its own doc), so calling it on
/// `LockAcquired`/`GatesPassed`/`Reconciled` made every landed run's story
/// falsely claim "this merge attempt is still running" for stages that had
/// long since completed (P383 review round 1, blocker
/// `merge-explainer-applied-to-nonterminal-frames`). Built once and shared
/// by all three renderers so a field present in one mode cannot silently be
/// absent from another (P383 review round 1, blocker
/// `landing-commit-and-commands-missing-from-styled-and-markdown`).
struct LandingFrame<'a> {
    frame: &'a MergeFrame,
    outcome: Option<Explanation>,
    rows: Vec<GateRow>,
}

fn landing_frames(frames: &[MergeFrame]) -> Vec<LandingFrame<'_>> {
    let outcome_index = frames.iter().rposition(|frame| frame.status.is_terminal());
    frames
        .iter()
        .enumerate()
        .map(|(index, frame)| LandingFrame {
            frame,
            outcome: (Some(index) == outcome_index).then(|| explain_frame(frame)),
            rows: gate_rows(&frame.evidence),
        })
        .collect()
}

fn merge_status_word(status: MergeStatus) -> &'static str {
    match status {
        MergeStatus::LockAcquired => "lock-acquired",
        MergeStatus::Parked => "parked",
        MergeStatus::GatesPassed => "gates-passed",
        MergeStatus::Reconciled => "reconciled",
        MergeStatus::Merged => "merged",
        MergeStatus::PostMergeCleanupFailure => "post-merge-cleanup-failure",
        MergeStatus::RecoveryFailure => "recovery-failure",
    }
}

// ---------------------------------------------------------------------------
// Plain (non-styled) rendering
// ---------------------------------------------------------------------------

/// Non-TTY sibling of [`story_view::run`](super::story_view::run): prints the
/// plain story block after a driven run's final output, so `--story` still
/// means something in CI logs without any terminal takeover (P550).
pub(crate) fn print_plain_story(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
) -> crate::Result<()> {
    render_plain(session, report, level)
}

fn render_plain(session: &Session, report: &StoryReport, level: StoryLevel) -> crate::Result<()> {
    w(disposition_sentence(session, report))?;
    if let Some(notice) = degrade_notice(level, report) {
        w(format!("  ({notice})"))?;
    }
    w(format!("ctx traits story {}", report.run_id))?;
    w(format!("  trait: {}", report.trait_id))?;
    w(format!("  spine: {}", spine_text(report.spine)))?;
    w(format!(
        "  enrichment: {}",
        match report.enrichment {
            ctx_traits_core::procedure::story::StoryEnrichment::Trait => "trait",
            ctx_traits_core::procedure::story::StoryEnrichment::LedgerOnly => "ledger-only",
        }
    ))?;
    w(format!("  status: {}", session_status(&report.status)))?;
    w(format!(
        "  final-state: {}",
        final_state_text(&report.final_state)
    ))?;
    w(format!(
        "  elapsed: {}",
        tui::elapsed_text(std::time::Duration::from_secs(report.elapsed_seconds))
    ))?;
    if let Some(summary) = stop_reason_summary(session) {
        w(format!("  stop-reason: {summary}"))?;
    }

    w(format!("  beats: {}", report.beats.len()))?;
    if report.beats.is_empty() {
        w("    (no accepted outputs yet)")?;
    }
    for beat in &report.beats {
        w(format!(
            "    [{}] {} — {}",
            beat.acceptance_order,
            render_runtime_path(&beat.position_path),
            beat.title.as_deref().unwrap_or("(untitled)")
        ))?;
        if !beat.reason.is_empty() {
            w(format!("        reason: {}", beat.reason))?;
        }
        if !beat.ref_text.is_empty() {
            w(format!(
                "        {} by {} ({})",
                beat.ref_text,
                beat.actor,
                beat.operation
                    .clone()
                    .map(|op| format!("{op:?}"))
                    .unwrap_or_default()
            ))?;
            w(format!("        {}", gloss_text(&beat.gloss)))?;
        }
        if let Some(command) = beat_command_text(beat) {
            w(format!("        {command}"))?;
        }
        if let Some(summary) = &beat.summary_line {
            w(format!(
                "        summary ({}): {summary}",
                summary_source_text(beat.summary_source)
            ))?;
        }
        for bullet in &beat.bullets {
            w(format!("        - {bullet}"))?;
        }
        if let Some(prose) = &beat.assisted_prose {
            w(format!("        assisted: {prose}"))?;
        }
    }

    if level == StoryLevel::Detailed || level == StoryLevel::Assisted {
        w(format!(
            "  detailed-timeline: {} event(s)",
            report.detailed_timeline.len()
        ))?;
        for timed in &report.detailed_timeline {
            w(format!(
                "    [{}] {} {:?}{}",
                timed.at_epoch_ms,
                timed.event.frame_id,
                timed.event.kind,
                timed
                    .event
                    .text
                    .as_deref()
                    .map(|text| format!(": {}", story::bounded(text)))
                    .unwrap_or_default()
            ))?;
        }
    }
    if level == StoryLevel::Assisted
        && let Some(tokens) = report.assisted_narrator_tokens
    {
        w(format!("  assisted-narrator-tokens: {tokens}"))?;
    }

    if !report.branch_decisions.is_empty() {
        w("  branch-decisions:")?;
        for decision in &report.branch_decisions {
            w(format!(
                "    {} matched={} arm={}",
                render_runtime_path(&decision.position_path),
                decision.matched,
                decision.selected_arm
            ))?;
        }
    }
    if !report.failure_routes.is_empty() {
        w("  failure-routes:")?;
        for route in &report.failure_routes {
            w(format!(
                "    {} -> {} (signal={})",
                route.source_step_id,
                route.target_step_id,
                route.signal.as_deref().unwrap_or("none")
            ))?;
        }
    }
    if !report.parallel_panels.is_empty() {
        w(format!(
            "  parallel-panels: {}",
            report.parallel_panels.len()
        ))?;
    }
    if !report.guard_evaluations.is_empty() {
        w(format!(
            "  guard-evaluations: {}",
            report.guard_evaluations.len()
        ))?;
    }

    w(format!("  emitted-signals: {}", report.emitted_signals))?;
    w(format!(
        "  rejected-submissions: {}",
        report.rejected_submissions
    ))?;

    if let Some(drive) = &report.last_drive {
        w(format!(
            "  last-drive: outcome={} recorded-at-epoch={}",
            drive.outcome, drive.recorded_at_epoch
        ))?;
    }
    if let Some(completion) = &report.completion {
        w(format!(
            "  completion: status={} outputs={}",
            session_status(&completion.status),
            completion.final_outputs.len()
        ))?;
    }

    if !report.merge_frames.is_empty() {
        w("  landing:")?;
        for landing in landing_frames(&report.merge_frames) {
            match &landing.outcome {
                Some(explanation) => w(format!(
                    "    {} — {}",
                    stage_sentence(landing.frame.stage),
                    explanation.sentence
                ))?,
                None => w(format!(
                    "    {}: {}",
                    stage_sentence(landing.frame.stage),
                    merge_status_word(landing.frame.status)
                ))?,
            }
            for row in landing.rows {
                w(format!("        {}: {}", row.label, row.value))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Styled (TTY) rendering
// ---------------------------------------------------------------------------

/// The shared styled-line document every P550 surface renders from: the
/// standalone `ctx traits story` command (via [`tui::emit_report`]) and the
/// `--story` pane / dashboard `S` viewer (`story_view.rs`). Building it once
/// here means a field present in one surface cannot silently be absent from
/// another.
pub(crate) fn story_document(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
) -> Vec<Line> {
    let mut disposition = Line::blank();
    let disposition_tone = match report.final_state {
        FinalState::Completed => Tone::Pass,
        FinalState::Blocked | FinalState::Failed | FinalState::Rejected => Tone::Fail,
        FinalState::Running => Tone::Default,
    };
    disposition.push(disposition_sentence(session, report), disposition_tone);
    let mut lines = vec![disposition];
    if let Some(notice) = degrade_notice(level, report) {
        let mut notice_line = Line::blank();
        notice_line.push(format!("  ({notice})"), Tone::Muted);
        lines.push(notice_line);
    }
    lines.push(Line::blank());
    lines.extend([
        labeled_line("story ", &report.run_id),
        labeled_line("trait: ", &report.trait_id),
        labeled_line("spine: ", spine_text(report.spine)),
        labeled_line(
            "enrichment: ",
            match report.enrichment {
                ctx_traits_core::procedure::story::StoryEnrichment::Trait => "trait",
                ctx_traits_core::procedure::story::StoryEnrichment::LedgerOnly => "ledger-only",
            },
        ),
    ]);

    let mut status_line = Line::blank();
    status_line.push("status: ", Tone::Muted);
    let status_tone = match report.status {
        ctx_traits_core::procedure::session::Status::Completed => Tone::Pass,
        ctx_traits_core::procedure::session::Status::Blocked
        | ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned
        | ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired
        | ctx_traits_core::procedure::session::Status::Rejected
        | ctx_traits_core::procedure::session::Status::Failed => Tone::Fail,
        _ => Tone::Default,
    };
    status_line.push(session_status(&report.status), status_tone);
    lines.push(status_line);
    lines.push(labeled_line(
        "final-state: ",
        final_state_text(&report.final_state),
    ));
    lines.push(labeled_line(
        "elapsed: ",
        &tui::elapsed_text(std::time::Duration::from_secs(report.elapsed_seconds)),
    ));

    if let Some(summary) = stop_reason_summary(session) {
        lines.push(labeled_line("stop-reason: ", &summary));
    }

    lines.push(Line::blank());
    let mut beats_header = Line::blank();
    beats_header.push(format!("beats ({}):", report.beats.len()), Tone::Muted);
    lines.push(beats_header);
    if report.beats.is_empty() {
        let mut empty_line = Line::blank();
        empty_line.push("  (no accepted outputs yet)", Tone::Muted);
        lines.push(empty_line);
    }

    for beat in &report.beats {
        let mut header = Line::blank();
        header.push(
            format!(
                "  [{}] {} ",
                beat.acceptance_order,
                render_runtime_path(&beat.position_path)
            ),
            Tone::Muted,
        );
        header.push(
            beat.title.as_deref().unwrap_or("(untitled)").to_string(),
            Tone::Default,
        );
        lines.push(header);

        if !beat.reason.is_empty() {
            let mut reason_line = Line::blank();
            reason_line.push("      ", Tone::Muted);
            reason_line.push(beat.reason.clone(), Tone::Muted);
            lines.push(reason_line);
        }

        if !beat.ref_text.is_empty() {
            let mut detail = Line::blank();
            detail.push("      ", Tone::Muted);
            detail.push(beat.ref_text.clone(), Tone::Default);
            detail.push(" by ", Tone::Muted);
            detail.push(beat.actor.clone(), Tone::Default);
            lines.push(detail);

            let mut gloss = Line::blank();
            gloss.push("      ", Tone::Muted);
            let gloss_tone = match beat.gloss.status.as_deref() {
                Some("approved") | Some("accepted") | Some("completed") => Tone::Pass,
                Some("revise") | Some("blocked") | Some("rejected") => Tone::Fail,
                _ => Tone::Default,
            };
            gloss.push(gloss_text(&beat.gloss), gloss_tone);
            lines.push(gloss);
        }

        if let Some(command_text) = beat_command_text(beat) {
            let mut command_line = Line::blank();
            command_line.push("      ", Tone::Muted);
            let tone = if command_text.contains("pass") {
                Tone::Pass
            } else {
                Tone::Fail
            };
            command_line.push(command_text, tone);
            lines.push(command_line);
        }

        if let Some(summary) = &beat.summary_line {
            let mut summary_line = Line::blank();
            summary_line.push("      summary: ", Tone::Muted);
            summary_line.push(summary.clone(), Tone::Default);
            summary_line.push(
                format!(" ({})", summary_source_text(beat.summary_source)),
                Tone::Muted,
            );
            lines.push(summary_line);
        }
        for bullet in &beat.bullets {
            let mut bullet_line = Line::blank();
            bullet_line.push("      - ", Tone::Muted);
            bullet_line.push(bullet.clone(), Tone::Default);
            lines.push(bullet_line);
        }
        if let Some(prose) = &beat.assisted_prose {
            let mut prose_line = Line::blank();
            prose_line.push("      assisted: ", Tone::Muted);
            prose_line.push(prose.clone(), Tone::Default);
            lines.push(prose_line);
        }
    }

    if level == StoryLevel::Detailed || level == StoryLevel::Assisted {
        lines.push(Line::blank());
        let mut timeline_header = Line::blank();
        timeline_header.push(
            format!(
                "detailed timeline ({} event(s)):",
                report.detailed_timeline.len()
            ),
            Tone::Muted,
        );
        lines.push(timeline_header);
        for timed in &report.detailed_timeline {
            let mut line = Line::blank();
            line.push(
                format!(
                    "  [{}] {} {:?}",
                    timed.at_epoch_ms, timed.event.frame_id, timed.event.kind
                ),
                Tone::Muted,
            );
            if let Some(text) = &timed.event.text {
                line.push(format!(": {}", story::bounded(text)), Tone::Default);
            }
            lines.push(line);
        }
    }
    if level == StoryLevel::Assisted
        && let Some(tokens) = report.assisted_narrator_tokens
    {
        lines.push(labeled_line(
            "assisted-narrator-tokens: ",
            &tokens.to_string(),
        ));
    }

    if !report.merge_frames.is_empty() {
        lines.push(Line::blank());
        lines.push(labeled_line("landing:", ""));
        for landing in landing_frames(&report.merge_frames) {
            let mut line = Line::blank();
            line.push("  ", Tone::Muted);
            match &landing.outcome {
                Some(explanation) => {
                    let tone = match landing.frame.status {
                        MergeStatus::Merged => Tone::Pass,
                        MergeStatus::Parked
                        | MergeStatus::PostMergeCleanupFailure
                        | MergeStatus::RecoveryFailure => Tone::Fail,
                        MergeStatus::LockAcquired
                        | MergeStatus::GatesPassed
                        | MergeStatus::Reconciled => Tone::Default,
                    };
                    line.push(explanation.sentence.clone(), tone);
                }
                None => {
                    line.push(
                        format!(
                            "{}: {}",
                            stage_sentence(landing.frame.stage),
                            merge_status_word(landing.frame.status)
                        ),
                        Tone::Muted,
                    );
                }
            }
            lines.push(line);
            for row in &landing.rows {
                let mut row_line = Line::blank();
                row_line.push("    ", Tone::Muted);
                row_line.push(format!("{}: ", row.label), Tone::Muted);
                row_line.push(row.value.clone(), Tone::Default);
                lines.push(row_line);
            }
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Markdown rendering — PR-comment-ready, no ANSI, no terminal-width
// dependence, byte-stable.
// ---------------------------------------------------------------------------

/// Neutralize a value before it enters a `--markdown` table cell: collapse
/// any embedded newline/carriage return to a space (a raw one would render
/// as if it belonged to the next row) and escape the reserved `|` as `\|`,
/// the standard GFM table escape (an unescaped one splits the row into an
/// extra column and misaligns every field after it). Applied to every
/// column a row is built from, not only the ones a given real ledger
/// happened to trip — cell safety is a property of how a row is
/// constructed, not of which field is remembered (P383 review round 2,
/// blocker `markdown-cell-not-escaped`).
fn markdown_cell(text: &str) -> String {
    text.replace(['\n', '\r'], " ").replace('|', "\\|")
}

fn render_markdown(session: &Session, report: &StoryReport, level: StoryLevel) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("**{}**", disposition_sentence(session, report)));
    if let Some(notice) = degrade_notice(level, report) {
        out.push(format!("_({notice})_"));
    }
    out.push(String::new());
    out.push(format!("## Run story: {}", report.run_id));
    out.push(String::new());
    out.push(format!(
        "- **trait**: {}\n- **status**: {}\n- **final-state**: {}\n- **elapsed**: {}",
        report.trait_id,
        session_status(&report.status),
        final_state_text(&report.final_state),
        tui::elapsed_text(std::time::Duration::from_secs(report.elapsed_seconds))
    ));
    if let Some(summary) = stop_reason_summary(session) {
        out.push(format!("- **stop-reason**: {summary}"));
    }
    out.push(String::new());

    out.push("### Arc".to_string());
    out.push(String::new());
    if report.beats.is_empty() {
        out.push("_(no accepted outputs yet)_".to_string());
    } else {
        out.push("| # | position | title | actor | detail | command | summary |".to_string());
        out.push("| --- | --- | --- | --- | --- | --- | --- |".to_string());
        for beat in &report.beats {
            let detail = if beat.ref_text.is_empty() {
                String::new()
            } else {
                format!("`{}` — {}", beat.ref_text, gloss_text(&beat.gloss))
            };
            let command = beat_command_text(beat).unwrap_or_default();
            let mut summary = beat.summary_line.clone().unwrap_or_default();
            if let Some(prose) = &beat.assisted_prose {
                summary = format!("{summary} {prose}").trim().to_string();
            }
            out.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                beat.acceptance_order,
                markdown_cell(&render_runtime_path(&beat.position_path)),
                markdown_cell(beat.title.as_deref().unwrap_or("")),
                markdown_cell(&beat.actor),
                markdown_cell(&detail),
                markdown_cell(&command),
                markdown_cell(&summary)
            ));
            for bullet in &beat.bullets {
                out.push(format!("  - {}", markdown_cell(bullet)));
            }
        }
    }

    if level == StoryLevel::Detailed || level == StoryLevel::Assisted {
        out.push(String::new());
        out.push(format!(
            "### Detailed timeline ({} event(s))",
            report.detailed_timeline.len()
        ));
        out.push(String::new());
        for timed in &report.detailed_timeline {
            let text = timed
                .event
                .text
                .as_deref()
                .map(story::bounded)
                .unwrap_or_default();
            out.push(format!(
                "- `{}` [{}] {:?} {}",
                timed.at_epoch_ms,
                markdown_cell(&timed.event.frame_id),
                timed.event.kind,
                markdown_cell(&text)
            ));
        }
    }
    if level == StoryLevel::Assisted
        && let Some(tokens) = report.assisted_narrator_tokens
    {
        out.push(String::new());
        out.push(format!("_assisted-narrator-tokens: {tokens}_"));
    }

    if !report.merge_frames.is_empty() {
        out.push(String::new());
        out.push("### Landing".to_string());
        out.push(String::new());
        for landing in landing_frames(&report.merge_frames) {
            match &landing.outcome {
                Some(explanation) => out.push(format!("- {}", explanation.sentence)),
                None => out.push(format!(
                    "- {}: {}",
                    stage_sentence(landing.frame.stage),
                    merge_status_word(landing.frame.status)
                )),
            }
            for row in &landing.rows {
                out.push(format!("  - {}: {}", row.label, row.value));
            }
        }
    }

    out
}
