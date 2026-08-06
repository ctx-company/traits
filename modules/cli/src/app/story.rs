//! `ctx traits story <run-id>` (P383): renders a persisted run-session ledger
//! as a chronological narrative. Read-only — resolves the ledger and,
//! best-effort, the trait's dry plan for producer-role enrichment, then hands
//! both to the pure `ctx_traits_core::procedure::story` builder. Never
//! mutates the ledger, never calls a provider, never reads the clock.

mod assisted;

use camino::Utf8PathBuf;

use std::collections::BTreeMap;

use ctx_traits_core::procedure::activity::ActivityKind;
use ctx_traits_core::procedure::runtime::FinalState;
use ctx_traits_core::procedure::runtime::PathSegment;
use ctx_traits_core::procedure::session::{MergeFrame, MergeStatus, Session};
use ctx_traits_core::procedure::story::{self, StoryBeat, StoryLevel, StoryReport};
use ctx_traits_core::response::CommandOutput;

use crate::app::command_handlers::print_json_report;
use crate::app::merge_story::{Explanation, GateRow, explain_frame, gate_rows, stage_sentence};
use crate::app::run_format::render_runtime_path;
use crate::app::run_view::{session_status, stop_reason_summary};
use crate::app::tui::{self, Line, Tone, write_plain_line as w};

/// Formats a persisted step title for human-facing output. Only loop control
/// segments own round numbers; item and other control segments may repeat an
/// iteration field for unrelated bookkeeping.
pub(crate) fn format_step_title(title: &str, position_path: &[PathSegment]) -> String {
    let rounds = position_path
        .iter()
        .filter(|segment| segment.kind == "loop")
        .filter_map(|segment| segment.iteration)
        .map(|iteration| iteration.saturating_add(1).to_string())
        .collect::<Vec<_>>();
    if rounds.is_empty() {
        title.to_string()
    } else {
        format!("{title} ({})", rounds.join("/"))
    }
}

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
                format!("This run parked during post-run: {}", explanation.sentence)
            }
            MergeStatus::PostMergeCleanupFailure => format!(
                "This run's merge landed, but cleanup afterward failed: {}",
                explanation.sentence
            ),
            MergeStatus::RecoveryFailure => format!(
                "This run's post-run could not be confirmed: {}",
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
            // The parked session title is a dashboard presentation record;
            // the story reads titles from the ledger's own provenance.
            ctx_traits_io::activity_sidecar::ActivityRecord::SessionTitle { .. } => {}
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

/// The landing evidence is projected once with the rest of the story. The
/// terminal frame gets its own explanation; progress frames remain facts.
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

#[derive(Debug, Clone)]
struct HumanSection {
    heading: &'static str,
    rows: Vec<String>,
}

#[derive(Debug, Clone)]
struct HumanStory {
    header: Vec<String>,
    sections: Vec<HumanSection>,
    diagnostics: Vec<HumanSection>,
}

fn human_story(session: &Session, report: &StoryReport, level: StoryLevel) -> HumanStory {
    project_human_story(report, level, disposition_sentence(session, report))
}

fn project_human_story(report: &StoryReport, level: StoryLevel, disposition: String) -> HumanStory {
    let mut header = vec![
        disposition.clone(),
        format!(
            "{} | elapsed {}",
            report.trait_id,
            tui::elapsed_text(std::time::Duration::from_secs(report.elapsed_seconds))
        ),
    ];
    if let Some(notice) = degrade_notice(level, report) {
        header.push(format!("({notice})"));
    }
    let sections = vec![
        HumanSection {
            heading: "JOURNEY",
            rows: journey_rows(report, level),
        },
        HumanSection {
            heading: "BLOCKERS RAISED",
            rows: blocker_rows(report),
        },
        HumanSection {
            heading: "COMMANDS",
            rows: command_rows(report),
        },
        HumanSection {
            heading: "OUTCOME",
            rows: outcome_rows(report, &disposition),
        },
    ];
    HumanStory {
        header,
        sections,
        diagnostics: if level == StoryLevel::Detailed {
            diagnostic_sections(report)
        } else {
            Vec::new()
        },
    }
}

fn loop_key(beat: &StoryBeat) -> Vec<(String, usize)> {
    beat.position_path
        .iter()
        .filter(|part| part.kind == "loop")
        .filter_map(|part| {
            part.iteration.map(|iteration| {
                (
                    part.id.clone().unwrap_or_else(|| "loop".to_string()),
                    iteration,
                )
            })
        })
        .collect()
}

fn round_label(key: &[(String, usize)]) -> String {
    let rounds = key
        .iter()
        .map(|(id, iteration)| format!("{} {}", humanize_loop_id(id), iteration + 1))
        .collect::<Vec<_>>()
        .join(" / ");
    format!("Round {rounds}")
}

fn humanize_loop_id(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => "Loop".to_string(),
    }
}

struct ActiveLoop {
    key: Vec<(String, usize)>,
    rows: Vec<String>,
}

fn journey_rows(report: &StoryReport, level: StoryLevel) -> Vec<String> {
    if report.beats.is_empty() {
        return vec!["No accepted outputs recorded.".to_string()];
    }
    let mut rows = Vec::new();
    let mut active_loop: Option<ActiveLoop> = None;
    for beat in &report.beats {
        let next_key = loop_key(beat);
        if next_key.is_empty() {
            if let Some(ActiveLoop {
                key,
                rows: loop_rows,
            }) = active_loop.take()
            {
                rows.push(round_label(&key));
                rows.extend(loop_rows.into_iter().map(|row| format!("  - {row}")));
            }
            rows.push(format!("- {}", journey_row(beat, report, level)));
        } else if active_loop
            .as_ref()
            .is_some_and(|active| active.key == next_key)
        {
            active_loop
                .as_mut()
                .expect("active loop just matched")
                .rows
                .push(journey_row(beat, report, level));
        } else {
            if let Some(ActiveLoop {
                key,
                rows: loop_rows,
            }) = active_loop.take()
            {
                rows.push(round_label(&key));
                rows.extend(loop_rows.into_iter().map(|row| format!("  - {row}")));
            }
            active_loop = Some(ActiveLoop {
                key: next_key,
                rows: vec![journey_row(beat, report, level)],
            });
        }
    }
    if let Some(ActiveLoop {
        key,
        rows: loop_rows,
    }) = active_loop
    {
        rows.push(round_label(&key));
        rows.extend(loop_rows.into_iter().map(|row| format!("  - {row}")));
    }
    rows
}

fn journey_row(beat: &StoryBeat, report: &StoryReport, level: StoryLevel) -> String {
    let title = beat.title.as_deref().unwrap_or("Untitled step");
    let mut parts = vec![title.to_string()];
    if beat.actor != "unrecorded" && !beat.actor.is_empty() {
        parts.push(format!("by {}", beat.actor));
    }
    if let Some(duration) = beat_duration(beat, report) {
        parts.push(duration);
    }
    let summary = if level == StoryLevel::Assisted {
        beat.assisted_prose.as_ref().or(beat.summary_line.as_ref())
    } else {
        beat.summary_line.as_ref()
    };
    if let Some(summary) = summary.filter(|summary| !summary.is_empty()) {
        parts.push(summary.clone());
    } else if let Some(status) = beat
        .gloss
        .status
        .as_deref()
        .filter(|status| !matches!(*status, "approved" | "accepted" | "completed"))
    {
        parts.push(status.to_string());
    } else if let Some(advisory) = &beat.gloss.advisory {
        parts.push(advisory.clone());
    } else if beat.command.is_some() {
        parts.push("command recorded".to_string());
    }
    format!("{} {}", beat_mark(beat), parts.join(" | "))
}

fn beat_mark(beat: &StoryBeat) -> &'static str {
    match beat.gloss.status.as_deref() {
        Some("approved" | "accepted" | "completed" | "passed" | "pass" | "ok") => "[ok]",
        Some("revise" | "blocked" | "rejected" | "failed" | "fail") => "[!]",
        Some(_) => "[-]",
        None => match beat.command.as_ref() {
            Some(command) if command.timed_out => "[!]",
            Some(command) => match command.exit_code {
                Some(0) => "[ok]",
                Some(_) => "[!]",
                None => "[-]",
            },
            None => "[-]",
        },
    }
}

fn beat_duration(beat: &StoryBeat, report: &StoryReport) -> Option<String> {
    let key = beat.frame_key.as_deref()?;
    if report
        .beats
        .iter()
        .filter(|other| other.frame_key.as_deref() == Some(key))
        .nth(1)
        .is_some()
    {
        return None;
    }
    let mut times = report
        .detailed_timeline
        .iter()
        .filter(|event| event.event.frame_id == key)
        .map(|event| event.at_epoch_ms);
    let first = times.next()?;
    let last = times.next_back().unwrap_or(first);
    (last > first).then(|| tui::elapsed_text(std::time::Duration::from_millis(last - first)))
}

fn blocker_rows(report: &StoryReport) -> Vec<String> {
    blocker_rows_for_beats(&report.beats)
}

fn blocker_rows_for_beats(beats: &[StoryBeat]) -> Vec<String> {
    #[derive(Clone)]
    struct Blocker {
        id: String,
        first_round: Option<String>,
        active: bool,
    }
    let mut by_ref: BTreeMap<&str, Vec<Blocker>> = BTreeMap::new();
    for beat in beats {
        if beat.ref_text.is_empty() {
            continue;
        }
        let entries = by_ref.entry(&beat.ref_text).or_default();
        for blocker in entries.iter_mut() {
            if !beat.gloss.blockers.contains(&blocker.id) {
                blocker.active = false;
            }
        }
        for id in &beat.gloss.blockers {
            if let Some(blocker) = entries.iter_mut().find(|blocker| blocker.id == *id) {
                blocker.active = true;
            } else {
                entries.push(Blocker {
                    id: id.clone(),
                    first_round: (!loop_key(beat).is_empty()).then(|| round_label(&loop_key(beat))),
                    active: true,
                });
            }
        }
    }
    let rows = by_ref
        .into_values()
        .flatten()
        .map(|blocker| {
            format!(
                "{}: {}{}",
                blocker.id,
                if blocker.active {
                    "never cleared"
                } else {
                    "cleared"
                },
                blocker
                    .first_round
                    .map(|round| format!(" (first raised {round})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        vec!["No blockers raised.".to_string()]
    } else {
        rows
    }
}

#[derive(Default)]
struct CommandSummary {
    count: usize,
    pass: usize,
    fail: usize,
    unknown: usize,
    activity: usize,
}

fn command_rows(report: &StoryReport) -> Vec<String> {
    enum CommandKey {
        Structured(Vec<String>),
        Activity(String),
    }
    impl Ord for CommandKey {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            match (self, other) {
                (Self::Structured(left), Self::Structured(right)) => left.cmp(right),
                (Self::Activity(left), Self::Activity(right)) => left.cmp(right),
                (Self::Structured(_), Self::Activity(_)) => std::cmp::Ordering::Less,
                (Self::Activity(_), Self::Structured(_)) => std::cmp::Ordering::Greater,
            }
        }
    }
    impl PartialOrd for CommandKey {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl PartialEq for CommandKey {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other).is_eq()
        }
    }
    impl Eq for CommandKey {}

    let mut commands: BTreeMap<CommandKey, CommandSummary> = BTreeMap::new();
    for beat in &report.beats {
        if let Some(command) = &beat.command {
            let entry = commands
                .entry(CommandKey::Structured(command.argv.clone()))
                .or_default();
            entry.count += 1;
            match (command.timed_out, command.exit_code) {
                (true, _) => entry.fail += 1,
                (false, Some(0)) => entry.pass += 1,
                (false, Some(_)) => entry.fail += 1,
                (false, None) => entry.unknown += 1,
            }
        }
    }
    for timed in &report.detailed_timeline {
        if timed.event.kind == ActivityKind::RunningTool
            && timed.event.tool.as_deref().is_some_and(is_shell_tool)
            && let Some(command) = timed.event.text.as_deref().and_then(shell_command)
        {
            commands
                .entry(CommandKey::Activity(command))
                .or_default()
                .activity += 1;
        }
    }
    let rows = commands
        .into_iter()
        .map(|(key, summary)| {
            let command = match key {
                CommandKey::Structured(argv) => argv_display(&argv),
                CommandKey::Activity(command) => command,
            };
            let mut facts = vec![format!("{} recorded", summary.count + summary.activity)];
            if summary.pass > 0 {
                facts.push(format!("{} passed", summary.pass));
            }
            if summary.fail > 0 {
                facts.push(format!("{} failed", summary.fail));
            }
            if summary.unknown > 0 {
                facts.push(format!("{} unknown", summary.unknown));
            }
            if summary.activity > 0 {
                facts.push(format!("{} activity only", summary.activity));
            }
            format!("$ {} ({})", story::bounded(&command), facts.join(", "))
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        vec!["No commands recorded.".to_string()]
    } else {
        rows
    }
}

fn is_shell_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "bash" | "sh" | "zsh" | "shell" | "command" | "run_command" | "shell_command"
    )
}

fn argv_display(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if argument.contains(char::is_whitespace) {
                format!("{:?}", argument)
            } else {
                argument.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sidecar tool text is provider-normalized and often lossy. Accept only an
/// exact single command field, not prose that merely happens to contain one.
fn shell_command(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    (object.len() == 1)
        .then(|| object.get("command"))
        .flatten()?
        .as_str()
        .filter(|command| !command.is_empty() && !command.contains(['\n', '\r']))
        .map(str::to_string)
}

fn outcome_rows(report: &StoryReport, disposition: &str) -> Vec<String> {
    let mut rows = Vec::new();
    for landing in landing_frames(&report.merge_frames) {
        rows.push(match landing.outcome {
            Some(explanation) => explanation.sentence,
            None => format!(
                "{}: {}",
                stage_sentence(landing.frame.stage),
                merge_status_word(landing.frame.status)
            ),
        });
        rows.extend(
            landing
                .rows
                .into_iter()
                .map(|row| format!("{}: {}", row.label, row.value)),
        );
    }
    if rows.is_empty()
        && let Some(completion) = &report.completion
    {
        rows.push(format!(
            "Completion: {} ({}, {} final output(s))",
            session_status(&completion.status),
            completion.event_code,
            completion.final_outputs.len()
        ));
    }
    if rows.is_empty() {
        rows.push(disposition.to_string());
    }
    rows
}

fn diagnostic_sections(report: &StoryReport) -> Vec<HumanSection> {
    let mut rows = report
        .beats
        .iter()
        .map(|beat| {
            format!(
                "{}: {}",
                beat.title.as_deref().unwrap_or("(untitled)"),
                render_runtime_path(&beat.position_path)
            )
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        rows.push("No beat paths recorded.".to_string());
    }
    let timeline = report
        .detailed_timeline
        .iter()
        .map(|timed| {
            format!(
                "[{}] {} {:?}{}",
                timed.at_epoch_ms,
                timed.event.frame_id,
                timed.event.kind,
                timed
                    .event
                    .text
                    .as_deref()
                    .map(|text| format!(": {}", story::bounded(text)))
                    .unwrap_or_default()
            )
        })
        .collect();
    let mut control_rows = Vec::new();
    control_rows.extend(report.branch_decisions.iter().map(|decision| {
        format!(
            "branch {} matched={} arm={} at {}",
            decision.branch_id,
            decision.matched,
            decision.selected_arm,
            render_runtime_path(&decision.position_path)
        )
    }));
    control_rows.extend(report.conditional_input_decisions.iter().map(|decision| {
        format!(
            "input {} matched={} at {}",
            decision.ref_text,
            decision.matched,
            render_runtime_path(&decision.position_path)
        )
    }));
    control_rows.extend(report.failure_routes.iter().map(|route| {
        format!(
            "route {} -> {} signal={} at {}",
            route.source_step_id,
            route.target_step_id,
            route.signal.as_deref().unwrap_or("none"),
            render_runtime_path(&route.position_path)
        )
    }));
    control_rows.extend(report.guard_evaluations.iter().map(|guard| {
        format!(
            "guard {} matched={}: {}",
            guard.predicate, guard.matched, guard.reason
        )
    }));
    control_rows.extend(report.parallel_panels.iter().map(|panel| {
        format!(
            "parallel {} join={} disposition={:?} at {}",
            panel.control_item_id.as_deref().unwrap_or("<unnamed>"),
            panel.join_policy,
            panel.disposition,
            render_runtime_path(&panel.position_path)
        )
    }));
    control_rows.push(format!("emitted signals: {}", report.emitted_signals));
    control_rows.push(format!(
        "rejected submissions: {}",
        report.rejected_submissions
    ));
    if let Some(drive) = &report.last_drive {
        control_rows.push(format!(
            "last drive: {} at {}",
            drive.outcome, drive.recorded_at_epoch
        ));
    }
    if let Some(completion) = &report.completion {
        control_rows.push(format!(
            "completion: {} ({}, {} final output(s))",
            session_status(&completion.status),
            completion.event_code,
            completion.final_outputs.len()
        ));
    }
    vec![
        HumanSection {
            heading: "DIAGNOSTIC PATHS",
            rows,
        },
        HumanSection {
            heading: "DETAILED TIMELINE",
            rows: timeline,
        },
        HumanSection {
            heading: "CONTROL EVIDENCE",
            rows: control_rows,
        },
    ]
}

pub(crate) fn print_plain_story(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
) -> crate::Result<()> {
    render_plain(session, report, level)
}

fn render_plain(session: &Session, report: &StoryReport, level: StoryLevel) -> crate::Result<()> {
    for line in human_lines(&human_story(session, report, level)) {
        w(line)?;
    }
    Ok(())
}

fn human_lines(story: &HumanStory) -> Vec<String> {
    let mut lines = story.header.clone();
    for section in story.sections.iter().chain(&story.diagnostics) {
        lines.push(String::new());
        lines.push(section.heading.to_string());
        lines.extend(section.rows.iter().cloned());
    }
    lines
}

pub(crate) fn story_document(
    session: &Session,
    report: &StoryReport,
    level: StoryLevel,
) -> Vec<Line> {
    human_lines(&human_story(session, report, level))
        .into_iter()
        .map(|text| {
            let mut line = Line::blank();
            let tone = if matches!(
                text.as_str(),
                "JOURNEY" | "BLOCKERS RAISED" | "COMMANDS" | "OUTCOME"
            ) {
                Tone::Muted
            } else {
                Tone::Default
            };
            line.push(text, tone);
            line
        })
        .collect()
}

fn render_markdown(session: &Session, report: &StoryReport, level: StoryLevel) -> Vec<String> {
    markdown_lines(&human_story(session, report, level))
}

fn markdown_lines(story: &HumanStory) -> Vec<String> {
    let mut lines = story
        .header
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("**{line}**")
            } else {
                line.clone()
            }
        })
        .collect::<Vec<_>>();
    for section in story.sections.iter().chain(&story.diagnostics) {
        lines.push(String::new());
        lines.push(format!("## {}", section.heading));
        lines.extend(section.rows.iter().map(|row| markdown_row(row)));
    }
    lines
}

fn markdown_row(row: &str) -> String {
    if let Some(row) = row.strip_prefix("  - ") {
        format!("  - {}", markdown_text(row))
    } else if let Some(row) = row.strip_prefix("- ") {
        format!("- {}", markdown_text(row))
    } else {
        format!("- {}", markdown_text(row))
    }
}

fn markdown_text(text: &str) -> String {
    text.split(['\r', '\n'])
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', "\\\\")
        .replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::ActivityEvent;
    use ctx_traits_core::procedure::runtime::{BranchDecision, CommandExecutionEvidence};
    use ctx_traits_core::procedure::session::{CompletionNotification, MergeStage, Status};
    use ctx_traits_core::procedure::story::{
        ActivityProvenance, StoryEnrichment, StorySpine, TimedActivityEvent,
    };

    fn report(beats: Vec<StoryBeat>) -> StoryReport {
        StoryReport {
            schema_version: "1".to_string(),
            run_id: "run".to_string(),
            trait_id: "trait".to_string(),
            spine: StorySpine::SlotRevisions,
            enrichment: StoryEnrichment::LedgerOnly,
            status: Status::Completed,
            final_state: FinalState::Completed,
            elapsed_seconds: 0,
            stop_reason: None,
            beats,
            branch_decisions: Vec::new(),
            conditional_input_decisions: Vec::new(),
            failure_routes: Vec::new(),
            guard_evaluations: Vec::new(),
            parallel_panels: Vec::new(),
            emitted_signals: 0,
            rejected_submissions: 0,
            last_drive: None,
            completion: None,
            merge_frames: Vec::new(),
            activity_provenance: ActivityProvenance::Recorded,
            detailed_timeline: Vec::new(),
            assisted_narrator_tokens: None,
            assisted_unavailable: None,
        }
    }

    fn command(argv: &[&str], exit_code: Option<i32>, timed_out: bool) -> CommandExecutionEvidence {
        CommandExecutionEvidence {
            argv: argv.iter().map(|value| (*value).to_string()).collect(),
            output_slot: "slot:result".to_string(),
            executable_digest: None,
            exit_code,
            timed_out,
            output_tail: None,
        }
    }

    fn activity(
        frame_id: &str,
        at_epoch_ms: u64,
        tool: Option<&str>,
        text: Option<&str>,
    ) -> TimedActivityEvent {
        TimedActivityEvent {
            at_epoch_ms,
            event: ActivityEvent {
                sequence: at_epoch_ms,
                frame_id: frame_id.to_string(),
                kind: ActivityKind::RunningTool,
                text: text.map(str::to_string),
                tool: tool.map(str::to_string),
                tokens: None,
            },
        }
    }

    fn segment(kind: &str, iteration: Option<usize>) -> PathSegment {
        PathSegment {
            kind: kind.to_string(),
            id: None,
            index: 0,
            iteration,
            item_index: None,
        }
    }

    fn beat(ref_text: &str, path: Vec<PathSegment>, blockers: &[&str]) -> StoryBeat {
        StoryBeat {
            acceptance_order: 0,
            position_path: path,
            title: Some("step".to_string()),
            reason: "all declared outputs accepted".to_string(),
            actor: "unrecorded".to_string(),
            ref_text: ref_text.to_string(),
            value_digest: None,
            operation: None,
            source: None,
            gloss: ctx_traits_core::procedure::story::ValueGloss {
                blockers: blockers.iter().map(|id| (*id).to_string()).collect(),
                ..Default::default()
            },
            command: None,
            summary_line: None,
            summary_source: None,
            frame_key: None,
            bullets: Vec::new(),
            assisted_prose: None,
        }
    }

    #[test]
    fn step_title_rounds_only_use_enclosing_loop_segments() {
        assert_eq!(format_step_title("build", &[]), "build");
        assert_eq!(
            format_step_title("build", &[segment("loop", Some(0))]),
            "build (1)"
        );
        assert_eq!(
            format_step_title("build", &[segment("loop", Some(4))]),
            "build (5)"
        );
        assert_eq!(
            format_step_title(
                "build",
                &[
                    segment("loop", Some(1)),
                    segment("parallel", Some(9)),
                    segment("loop", Some(2)),
                    segment("item", Some(2)),
                ]
            ),
            "build (2/3)"
        );
    }

    #[test]
    fn loop_round_labels_use_recorded_non_contiguous_nested_iterations() {
        let mut outer = segment("loop", Some(2));
        outer.id = Some("outer-review".to_string());
        let mut inner = segment("loop", Some(0));
        inner.id = Some("inner-fix".to_string());
        let first = beat("output", vec![outer, inner], &[]);
        let mut other = segment("loop", Some(2));
        other.id = Some("other-review".to_string());
        assert_eq!(
            round_label(&loop_key(&first)),
            "Round Outer review 3 / Inner fix 1"
        );
        assert_eq!(
            round_label(&loop_key(&beat("output", vec![other], &[]))),
            "Round Other review 3"
        );
    }

    #[test]
    fn journey_preserves_acceptance_order_around_loop_groups() {
        let mut setup = beat("setup", Vec::new(), &[]);
        setup.title = Some("setup".to_string());
        let mut loop_beat = beat("loop", vec![segment("loop", Some(2))], &[]);
        loop_beat.title = Some("loop work".to_string());
        let mut nested = beat(
            "nested",
            vec![segment("loop", Some(2)), segment("loop", Some(1))],
            &[],
        );
        nested.title = Some("nested work".to_string());
        let mut finish = beat("finish", Vec::new(), &[]);
        finish.title = Some("finish".to_string());
        let rows = journey_rows(
            &report(vec![setup, loop_beat, nested, finish]),
            StoryLevel::Default,
        );
        assert_eq!(rows[0], "- [-] setup");
        assert!(rows[1].starts_with("Round Loop 3"));
        assert!(rows[3].starts_with("Round Loop 3 / Loop 2"));
        assert_eq!(rows[5], "- [-] finish");
    }

    #[test]
    fn blockers_clear_only_on_a_later_revision_of_the_same_ref() {
        let rows = blocker_rows_for_beats(&[
            beat("review", vec![segment("loop", Some(0))], &["missing-test"]),
            beat("other-review", vec![segment("loop", Some(1))], &[]),
            beat("review", vec![segment("loop", Some(2))], &[]),
            beat("never", Vec::new(), &["still-open"]),
        ]);
        assert!(rows.iter().any(|row| row.contains("missing-test: cleared")));
        assert!(
            rows.iter()
                .any(|row| row.contains("still-open: never cleared"))
        );
    }

    #[test]
    fn shell_activity_requires_an_unambiguous_command_object() {
        assert_eq!(
            shell_command(r#"{"command":"cargo test"}"#).as_deref(),
            Some("cargo test")
        );
        assert!(shell_command(r#"{"command":"cargo test","cwd":"/tmp"}"#).is_none());
        assert!(shell_command("command=cargo test").is_none());
    }

    #[test]
    fn shared_frame_duration_is_omitted_but_unique_frame_duration_is_recorded() {
        let mut first = beat("one", vec![segment("loop", Some(0))], &[]);
        first.frame_key = Some("shared".to_string());
        let mut second = beat("two", vec![segment("loop", Some(1))], &[]);
        second.frame_key = Some("shared".to_string());
        let mut unique = beat("three", Vec::new(), &[]);
        unique.frame_key = Some("unique".to_string());
        let mut story = report(vec![first.clone(), second, unique.clone()]);
        story.detailed_timeline = vec![
            activity("shared", 1, None, None),
            activity("shared", 9_001, None, None),
            activity("unique", 1, None, None),
            activity("unique", 2_001, None, None),
        ];
        assert!(beat_duration(&first, &story).is_none());
        assert_eq!(beat_duration(&unique, &story).as_deref(), Some("00:00:02"));
    }

    #[test]
    fn command_rows_keep_exact_argv_and_timeout_is_failure() {
        let mut first = beat("one", Vec::new(), &[]);
        first.command = Some(command(&["a b", "c"], Some(0), false));
        let mut second = beat("two", Vec::new(), &[]);
        second.command = Some(command(&["a", "b c"], None, true));
        let mut story = report(vec![first, second]);
        story.detailed_timeline = vec![
            activity("tool", 1, Some("Bash"), Some(r#"{"command":"cargo test"}"#)),
            activity(
                "tool",
                2,
                Some("bash"),
                Some(r#"{"command":"cargo test","cwd":"."}"#),
            ),
        ];
        let rows = command_rows(&story);
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .any(|row| row.contains("\"a b\" c") && row.contains("1 passed"))
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("a \"b c\"") && row.contains("1 failed"))
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("cargo test") && row.contains("activity only"))
        );
    }

    #[test]
    fn unknown_and_nonterminal_statuses_are_neutral() {
        let mut beat = beat("one", Vec::new(), &[]);
        for status in ["needs-discussion", "pending"] {
            beat.gloss.status = Some(status.to_string());
            assert_eq!(beat_mark(&beat), "[-]");
        }
        beat.gloss.status = Some("approved".to_string());
        assert_eq!(beat_mark(&beat), "[ok]");
        beat.gloss.status = Some("failed".to_string());
        assert_eq!(beat_mark(&beat), "[!]");
    }

    #[test]
    fn detailed_diagnostics_retain_control_evidence_only_in_diagnostic_sections() {
        let mut story = report(Vec::new());
        story.branch_decisions.push(BranchDecision {
            parent_run_index: 0,
            branch_id: "choose-path".to_string(),
            position_path: vec![segment("procedure", None)],
            matched: true,
            when: None,
            guard_evaluation_start_index: None,
            slot_revision_watermark: None,
            guard_evaluation_index: 0,
            selected_arm: "then".to_string(),
            sequence_id: None,
        });
        story.emitted_signals = 2;
        story.rejected_submissions = 1;
        let detailed = human_lines(&project_human_story(
            &story,
            StoryLevel::Detailed,
            "disposition".to_string(),
        ))
        .join("\n");
        let default = human_lines(&project_human_story(
            &story,
            StoryLevel::Default,
            "disposition".to_string(),
        ))
        .join("\n");
        let assisted = human_lines(&project_human_story(
            &story,
            StoryLevel::Assisted,
            "disposition".to_string(),
        ))
        .join("\n");
        assert!(detailed.contains("branch choose-path matched=true arm=then"));
        assert!(detailed.contains("emitted signals: 2"));
        assert!(detailed.contains("procedure[0]"));
        assert!(!default.contains("choose-path"));
        assert!(!assisted.contains("choose-path"));
    }

    #[test]
    fn outcome_prefers_specific_evidence_and_falls_back_to_disposition() {
        let mut story = report(Vec::new());
        assert_eq!(
            outcome_rows(&story, "This run is blocked."),
            vec!["This run is blocked."]
        );
        story.completion = Some(CompletionNotification {
            status: Status::Completed,
            event_code: "completed".to_string(),
            final_outputs: Vec::new(),
            final_session_digest: ctx_traits_core::digest::Digest::from_bytes(b"session"),
        });
        assert_eq!(
            outcome_rows(&story, "unused"),
            vec!["Completion: completed (completed, 0 final output(s))"]
        );
        story.completion = None;
        story.merge_frames = vec![MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Parked,
            reason: Some("branch mismatch".to_string()),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        }];
        assert_ne!(
            outcome_rows(&story, "This run is blocked."),
            vec!["This run is blocked."]
        );
        story.merge_frames[0].status = MergeStatus::Merged;
        assert!(
            outcome_rows(&story, "This run is blocked.")
                .iter()
                .any(|row| row.contains("landed"))
        );
    }

    #[test]
    fn markdown_projection_is_line_safe_and_equivalent_for_default_and_assisted() {
        let mut beat = beat("output", Vec::new(), &[]);
        beat.summary_line = Some("default\rsummary | value".to_string());
        beat.assisted_prose = Some("assisted\nsummary | value".to_string());
        let report = report(vec![beat]);
        let default = project_human_story(&report, StoryLevel::Default, "disposition".to_string());
        let assisted =
            project_human_story(&report, StoryLevel::Assisted, "disposition".to_string());
        let default_markdown = markdown_lines(&default);
        let assisted_markdown = markdown_lines(&assisted);
        let headings = |lines: &[String]| {
            lines
                .iter()
                .filter(|line| line.starts_with("## "))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(headings(&default_markdown), headings(&assisted_markdown));
        assert_eq!(
            headings(&default_markdown),
            [
                "## JOURNEY",
                "## BLOCKERS RAISED",
                "## COMMANDS",
                "## OUTCOME"
            ]
        );
        assert!(
            default_markdown
                .iter()
                .any(|line| line.contains("default summary \\| value"))
        );
        assert!(
            assisted_markdown
                .iter()
                .any(|line| line.contains("assisted summary \\| value"))
        );
        assert!(default_markdown.iter().all(|line| !line.contains('\r')));
        assert_eq!(
            markdown_row("  - [ok] step | prose\rnext"),
            "  - [ok] step \\| prose next"
        );
        assert_eq!(markdown_row("Round Loop 1"), "- Round Loop 1");
    }
}
