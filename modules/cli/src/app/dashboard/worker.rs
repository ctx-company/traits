//! The dashboard's single IO owner.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::{
    AttachedView, DashboardSnapshot, Screen, SessionPreviewRequest, State, build_attached_view,
    refresh_attached_view,
};

use ctx_traits_io::center::{ControlAction, StartResult};

pub(super) type RefreshResult = Result<Arc<DashboardSnapshot>, String>;
pub(super) type PreviewResult = AttachedView;
pub(super) struct TraitDetailResult {
    pub(super) selector: ctx_traits_io::library::LibraryDetailSelector,
    pub(super) result: Result<ctx_traits_io::library::LibraryDetailResolution, String>,
}

pub(super) struct Handle {
    commands: mpsc::Sender<Command>,
    snapshots: mpsc::Receiver<RefreshResult>,
    previews: mpsc::Receiver<PreviewResult>,
    trait_details: mpsc::Receiver<TraitDetailResult>,
    explanations: mpsc::Receiver<ExplanationResult>,
    actions: mpsc::Receiver<ActionResult>,
    action_sender: mpsc::Sender<ActionResult>,
}

#[derive(Clone)]
pub(super) struct ExplanationRequest {
    pub(super) trait_id: String,
    pub(super) canonical_digest: String,
    pub(super) canonical_path: String,
}

pub(super) struct ExplanationResult {
    pub(super) trait_id: String,
    pub(super) canonical_digest: String,
    pub(super) result: Result<String, String>,
}

pub(super) struct ActionResult {
    pub(super) message: String,
    pub(super) session_id: Option<String>,
    pub(super) task_key: Option<String>,
}

#[derive(Clone)]
enum Command {
    /// Reproject the worker's subscription-owned rows after an explicit view
    /// change. This never asks the center for another session.
    Render {
        all_repos: bool,
        screen: Screen,
    },
    Preview(SessionPreviewRequest),
    TraitDetail(ctx_traits_io::library::LibraryDetailSelector),
    /// Kept for test-only command projections; it never reads library files.
    #[cfg(test)]
    Refresh,
    Explain(ExplanationRequest),
}

impl Handle {
    pub(super) fn new() -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let (preview_tx, previews) = mpsc::channel();
        let (trait_detail_tx, trait_details) = mpsc::channel();
        let (explanation_tx, explanations) = mpsc::channel();
        let (action_sender, actions) = mpsc::channel();
        std::thread::spawn(move || {
            run(
                command_rx,
                snapshot_tx,
                preview_tx,
                trait_detail_tx,
                explanation_tx,
            )
        });
        Self {
            commands,
            snapshots,
            previews,
            trait_details,
            explanations,
            actions,
            action_sender,
        }
    }

    #[cfg(test)]
    pub(super) fn for_tests() -> Self {
        let (commands, _command_rx) = mpsc::channel();
        let (_snapshot_tx, snapshots) = mpsc::channel();
        let (_preview_tx, previews) = mpsc::channel();
        let (_trait_detail_tx, trait_details) = mpsc::channel();
        let (_explanation_tx, explanations) = mpsc::channel();
        let (action_sender, actions) = mpsc::channel();
        Self {
            commands,
            snapshots,
            previews,
            trait_details,
            explanations,
            actions,
            action_sender,
        }
    }

    pub(super) fn explain(&self, request: ExplanationRequest) {
        let _ = self.commands.send(Command::Explain(request));
    }

    pub(super) fn explanation_results(&self) -> Vec<ExplanationResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.explanations.try_recv() {
            results.push(result);
        }
        results
    }

    pub(super) fn action_results(&self) -> Vec<ActionResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.actions.try_recv() {
            results.push(result);
        }
        results
    }

    #[cfg(test)]
    pub(super) fn test_action_sender(&self) -> mpsc::Sender<ActionResult> {
        self.action_sender.clone()
    }

    pub(super) fn start_trait(&self, args: Vec<String>, cwd: String, task_key: Option<String>) {
        let sender = self.action_sender.clone();
        std::thread::spawn(move || {
            let result = ctx_traits_io::center::start_trait(&args, camino::Utf8Path::new(&cwd));
            let _ = sender.send(start_action_result(result, task_key, None));
        });
    }

    pub(super) fn start_session(
        &self,
        session_id: String,
        display_id: String,
        repo_key: Option<String>,
    ) {
        let sender = self.action_sender.clone();
        std::thread::spawn(move || {
            let result = ctx_traits_io::center::start_session(&session_id, repo_key.as_deref());
            let _ = sender.send(start_action_result(result, None, Some(display_id)));
        });
    }

    pub(super) fn control(
        &self,
        session_id: String,
        display_id: String,
        repo_key: Option<String>,
        action: ControlAction,
    ) {
        let sender = self.action_sender.clone();
        std::thread::spawn(move || {
            let verb = match action {
                ControlAction::Interrupt => "stop",
                ControlAction::Pause => "pause",
            };
            let message =
                match ctx_traits_io::center::control(&session_id, repo_key.as_deref(), action) {
                    Ok(result) => result.message(action, &display_id),
                    Err(error) => format!("{verb} failed for {display_id}: {error}"),
                };
            let _ = sender.send(ActionResult {
                message,
                session_id: None,
                task_key: None,
            });
        });
    }

    pub(super) fn refresh(&self, all_repos: bool, screen: Screen) {
        // Refreshes are read-only and may be superseded. A disconnected worker
        // is treated as an unavailable background task, never a TUI failure.
        let _ = self.commands.send(Command::Render { all_repos, screen });
    }

    pub(super) fn preview(&self, request: SessionPreviewRequest) {
        let _ = self.commands.send(Command::Preview(request));
    }

    pub(super) fn trait_detail(&self, selector: ctx_traits_io::library::LibraryDetailSelector) {
        let _ = self.commands.send(Command::TraitDetail(selector));
    }

    pub(super) fn notify_library_changed(&self) {
        std::thread::spawn(|| {
            if let Ok(repo_key) = ctx_traits_io::state::current_repo_key() {
                let _ = ctx_traits_io::center::notify_library_changed(&repo_key);
            }
        });
    }

    pub(super) fn trait_detail_results(&self) -> Vec<TraitDetailResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.trait_details.try_recv() {
            results.push(result);
        }
        results
    }

    pub(super) fn preview_results(&self) -> Vec<PreviewResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.previews.try_recv() {
            results.push(result);
        }
        results
    }

    pub(super) fn refresh_results(&self) -> Vec<RefreshResult> {
        let mut latest_snapshot = None;
        let mut outage_error = None;
        let mut recovery_snapshot = None;
        while let Ok(result) = self.snapshots.try_recv() {
            match result {
                Ok(snapshot) => {
                    if snapshot.clear_refresh_error {
                        recovery_snapshot = Some(Arc::clone(&snapshot));
                    }
                    latest_snapshot = Some(snapshot);
                    // Only a completed subscription snapshot recovers from an
                    // outage. Command refreshes retain the warning.
                    if latest_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.clear_refresh_error)
                    {
                        outage_error = None;
                    }
                }
                Err(error) => outage_error = Some(error),
            }
        }
        // Renderer coalescing may retain a command refresh received after
        // SnapshotEnd. Deliver the completed subscription first so it still
        // authoritatively clears the sticky outage state.
        let mut results = Vec::new();
        if let Some(recovery) = recovery_snapshot {
            let recovery_is_latest = latest_snapshot
                .as_ref()
                .is_some_and(|latest| Arc::ptr_eq(&recovery, latest));
            results.push(Ok(recovery));
            if recovery_is_latest {
                latest_snapshot = None;
            }
        }
        results.extend(latest_snapshot.into_iter().map(Ok));
        results.extend(outage_error.into_iter().map(Err));
        results
    }
}

fn start_action_result(
    result: ctx_traits_io::Result<StartResult>,
    task_key: Option<String>,
    display_id: Option<String>,
) -> ActionResult {
    let label = display_id.unwrap_or_else(|| "run".to_string());
    match result {
        Ok(StartResult::Started { session_id }) => ActionResult {
            message: format!("started {label} as {session_id}"),
            session_id: Some(session_id),
            task_key,
        },
        Ok(StartResult::Exited { code, stderr }) => ActionResult {
            message: format!(
                "start failed for {label} (exit {}): {stderr}",
                code.map_or_else(|| "unknown".to_string(), |code| code.to_string())
            ),
            session_id: None,
            task_key,
        },
        Err(error) => ActionResult {
            message: format!("start failed for {label}: {error}"),
            session_id: None,
            task_key,
        },
    }
}

fn run(
    commands: mpsc::Receiver<Command>,
    snapshots: mpsc::Sender<RefreshResult>,
    previews: mpsc::Sender<PreviewResult>,
    trait_details: mpsc::Sender<TraitDetailResult>,
    explanations: mpsc::Sender<ExplanationResult>,
) {
    let mut state = State::new_without_worker();
    let mut rows = HashMap::new();
    let mut last_snapshot_at = None;
    loop {
        let subscription = match ctx_traits_io::center::subscribe(None) {
            Ok(subscription) => subscription,
            Err(error) => {
                if snapshots
                    .send(Err(center_unreachable(
                        last_snapshot_at,
                        &error.to_string(),
                    )))
                    .is_err()
                {
                    return;
                }
                if wait_for_retry(
                    &commands,
                    &snapshots,
                    &previews,
                    &trait_details,
                    &explanations,
                    &mut state,
                    &rows,
                    false,
                )
                .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let mut staged_rows = HashMap::new();
        let mut snapshotting = false;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => {
                        if handle_one_command(
                            command,
                            &snapshots,
                            &previews,
                            &trait_details,
                            &explanations,
                            &mut state,
                            &rows,
                            false,
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    // The subscription can remain healthy after the dashboard
                    // has gone away. Do not strand this IO thread in that case.
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            match subscription.recv_timeout(Duration::from_millis(100)) {
                Ok(ctx_traits_io::center::CenterEvent::SnapshotStart) => {
                    staged_rows.clear();
                    snapshotting = true;
                }
                Ok(ctx_traits_io::center::CenterEvent::SnapshotRow(row)) => {
                    staged_rows.insert(row.ledger_path.clone(), *row);
                }
                Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd) => {
                    let candidate_rows = std::mem::take(&mut staged_rows);
                    snapshotting = false;
                    let accepted = match emit_subscription_snapshot(
                        &snapshots,
                        &mut state,
                        &candidate_rows,
                        None,
                    ) {
                        Ok(accepted) => accepted,
                        Err(()) => return,
                    };
                    if accepted {
                        rows = candidate_rows;
                    } else {
                        // A rejected complete candidate must not wait for a
                        // later delta: dropping this stream makes the center
                        // send a fresh complete snapshot after reconnect.
                        break;
                    }
                    // The outage footer describes rendered state, not a merely
                    // received center snapshot. Keep its timestamp anchored to
                    // the last projection the renderer could actually accept.
                    record_accepted_snapshot_time(
                        &mut last_snapshot_at,
                        accepted,
                        std::time::SystemTime::now(),
                    );
                    // Do not delay the authoritative session snapshot. The
                    // served library is requested immediately afterwards.
                    if !cfg!(test)
                        && let Err(error) = refresh_library(&mut state)
                    {
                        let _ = snapshots.send(Err(format!("library unavailable: {error}")));
                    }
                }
                Ok(ctx_traits_io::center::CenterEvent::Delta(delta)) => {
                    if let ctx_traits_io::center::CenterDelta::LibraryChanged { repo_keys } = &delta
                    {
                        let scoped = ctx_traits_io::state::current_repo_key()
                            .map(|key| repo_keys.contains(&key))
                            .unwrap_or(false);
                        if scoped {
                            match refresh_library(&mut state) {
                                Ok(()) => {
                                    if emit_cached_rows_snapshot(
                                        &snapshots, &mut state, &rows, false,
                                    )
                                    .is_err()
                                    {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    let _ = snapshots
                                        .send(Err(format!("library unavailable: {error}")));
                                }
                            }
                        }
                        continue;
                    }
                    if snapshotting {
                        let _ = apply_delta(&mut staged_rows, delta);
                        continue;
                    }
                    let mut candidate_rows = rows.clone();
                    let changed_ledger_path = apply_delta(&mut candidate_rows, delta);
                    if !snapshotting {
                        let accepted = match emit_subscription_snapshot(
                            &snapshots,
                            &mut state,
                            &candidate_rows,
                            changed_ledger_path,
                        ) {
                            Ok(accepted) => accepted,
                            Err(()) => return,
                        };
                        if accepted {
                            rows = candidate_rows;
                        } else {
                            // Keep the accepted map visible and reconnect for
                            // a complete retry of the rejected delta.
                            break;
                        }
                    }
                }
                // Board changes are a separate subscription payload, never a
                // run-row delta. The existing task-board reader remains until
                // its dedicated migration consumes this event.
                Ok(ctx_traits_io::center::CenterEvent::BoardChanged { .. }) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if snapshots
                        .send(Err(center_unreachable(
                            last_snapshot_at,
                            "subscription closed",
                        )))
                        .is_err()
                    {
                        return;
                    }
                    // A peer can accept a subscription and immediately close
                    // it. Apply the same delay as connect/projection failures
                    // so that cannot turn into a hot reconnect loop.
                    break;
                }
            }
        }
        // The inner loop only exits after a rejected projection or a closed
        // stream. In both cases commands remain serviceable while a bounded
        // delay prevents a persistent peer/local failure from spinning.
        if wait_for_retry(
            &commands,
            &snapshots,
            &previews,
            &trait_details,
            &explanations,
            &mut state,
            &rows,
            false,
        )
        .is_err()
        {
            return;
        }
    }
}

/// A terminal drive is a changed row, not a disappearance. Only the center's
/// explicit `Ended` delta removes an entry from the dashboard model. The rule
/// itself lives in `ctx_traits_io::center::CenterDelta::apply_to`, shared with
/// the desktop; this wrapper only keeps this call site's `Option<String>`
/// shape.
fn apply_delta(
    rows: &mut HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    delta: ctx_traits_io::center::CenterDelta,
) -> Option<String> {
    Some(delta.apply_to(rows))
}

fn center_unreachable(last_snapshot_at: Option<std::time::SystemTime>, detail: &str) -> String {
    let Some(snapshot_at) = last_snapshot_at else {
        return format!("center unreachable — no session snapshot available ({detail})");
    };
    let seconds = snapshot_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    format!(
        "center unreachable — showing state as of {:02}:{:02}:{:02} ({detail})",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
    )
}

fn record_accepted_snapshot_time(
    last_snapshot_at: &mut Option<std::time::SystemTime>,
    accepted: bool,
    completed_at: std::time::SystemTime,
) {
    if accepted {
        *last_snapshot_at = Some(completed_at);
    }
}

/// Wait through the reconnect backoff without making queued dashboard actions
/// wait behind it. Commands remain local to the accepted row model.
#[allow(clippy::too_many_arguments)] // one channel per result stream; a struct would only rename the eight
fn wait_for_retry(
    commands: &mpsc::Receiver<Command>,
    snapshots: &mpsc::Sender<RefreshResult>,
    previews: &mpsc::Sender<PreviewResult>,
    trait_details: &mpsc::Sender<TraitDetailResult>,
    explanations: &mpsc::Sender<ExplanationResult>,
    state: &mut State,
    rows: &HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    clear_refresh_error: bool,
) -> Result<(), ()> {
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        match commands.recv_timeout(remaining) {
            Ok(command) => handle_one_command(
                command,
                snapshots,
                previews,
                trait_details,
                explanations,
                state,
                rows,
                clear_refresh_error,
            )?,
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(()),
        }
    }
}

#[allow(clippy::too_many_arguments)] // one channel per result stream; a struct would only rename the eight
fn handle_one_command(
    command: Command,
    snapshots: &mpsc::Sender<RefreshResult>,
    previews: &mpsc::Sender<PreviewResult>,
    trait_details: &mpsc::Sender<TraitDetailResult>,
    explanations: &mpsc::Sender<ExplanationResult>,
    state: &mut State,
    rows: &HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    clear_refresh_error: bool,
) -> Result<(), ()> {
    match command {
        Command::Explain(request) => explanations.send(explain(request)).map_err(|_| ()),
        Command::Preview(request) => previews.send(preview(state, request)).map_err(|_| ()),
        Command::TraitDetail(selector) => {
            let sender = trait_details.clone();
            std::thread::spawn(move || {
                let result = ctx_traits_io::state::current_repo_key()
                    .and_then(|repo_key| {
                        ctx_traits_io::center::library_detail_existing(&repo_key, selector.clone())
                    })
                    .map_err(|error| error.to_string());
                let _ = sender.send(TraitDetailResult { selector, result });
            });
            Ok(())
        }
        #[cfg(test)]
        Command::Refresh => {
            emit_cached_rows_snapshot(snapshots, state, rows, clear_refresh_error)?;
            Ok(())
        }
        Command::Render { all_repos, screen } => {
            let previous_scope = state.all_repos;
            let previous_screen = state.screen;
            state.all_repos = all_repos;
            state.screen = screen;
            // Rendering a scope toggle projects the accepted subscription map
            // with the same bounded presentation enrichment as a subscription.
            let accepted = emit_cached_rows_snapshot(snapshots, state, rows, clear_refresh_error)?;
            if !accepted {
                state.all_repos = previous_scope;
                state.screen = previous_screen;
            }
            Ok(())
        }
    }
}

fn emit_subscription_snapshot(
    snapshots: &mpsc::Sender<RefreshResult>,
    state: &mut State,
    rows: &HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    changed_ledger_path: Option<String>,
) -> Result<bool, ()> {
    emit_snapshot(snapshots, state, rows, true, true, changed_ledger_path)
}

fn refresh_library(state: &mut State) -> crate::Result<()> {
    let repo_key = ctx_traits_io::state::current_repo_key()?;
    let answer = ctx_traits_io::center::library_existing(&repo_key)?;
    state.apply_library(&answer);
    Ok(())
}

fn emit_cached_rows_snapshot(
    snapshots: &mpsc::Sender<RefreshResult>,
    state: &mut State,
    rows: &HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    clear_refresh_error: bool,
) -> Result<bool, ()> {
    // An explicit render may rebuild a scoped projection, so retain the same
    // enriched parked-Ask and trait-drift presentation as subscription renders.
    emit_snapshot(snapshots, state, rows, clear_refresh_error, true, None)
}

fn emit_snapshot(
    snapshots: &mpsc::Sender<RefreshResult>,
    state: &mut State,
    rows: &HashMap<String, ctx_traits_io::center::CenterPublicRow>,
    clear_refresh_error: bool,
    enrich_session_presentations: bool,
    changed_ledger_path: Option<String>,
) -> Result<bool, ()> {
    let rows: Vec<_> = rows.values().cloned().collect();
    let result = state
        .reload_from_center_rows(&rows, enrich_session_presentations)
        .map(|_| {
            let mut snapshot = DashboardSnapshot::from_state(state);
            snapshot.clear_refresh_error = clear_refresh_error;
            snapshot.changed_ledger_path = changed_ledger_path;
            Arc::new(snapshot)
        })
        .map_err(|error| error.to_string());
    let accepted = result.is_ok();
    snapshots.send(result).map_err(|_| ())?;
    Ok(accepted)
}

fn preview(state: &mut State, request: SessionPreviewRequest) -> AttachedView {
    if state.session_preview.as_ref().is_some_and(|view| {
        view.session_id == request.session_id
            && view.ledger_path == request.ledger_path
            && view.run_id == request.run_id
    }) {
        let view = state.session_preview.as_mut().expect("matching preview");
        refresh_attached_view(view);
        return view.clone();
    }
    let view = build_attached_view(&request.session_id, &request.ledger_path, &request.run_id);
    state.session_preview = Some(view.clone());
    view
}

fn explain(request: ExplanationRequest) -> ExplanationResult {
    let result = (|| -> crate::Result<String> {
        let repo_root = ctx_traits_io::state::state_repo_root()?;
        let (record, _) = ctx_traits_io::cache::get_or_generate_trait_explanation(
            &repo_root,
            &request.trait_id,
            &request.canonical_digest,
            || -> Result<ctx_traits_io::cache::TraitExplanation, String> {
                let evidence = crate::app::explain_inspect::build_explain_evidence(
                    &request.canonical_path,
                    None,
                )
                .map_err(|error| error.to_string())?;
                if evidence.trait_ref.id.as_str() != request.trait_id
                    || evidence.source_digest.as_str() != request.canonical_digest
                {
                    return Err("selected trait changed before explanation generation".to_string());
                }
                let scaffold_text = serde_json::to_string(&evidence.scaffold)
                    .map_err(|error| format!("serialize explain scaffold: {error}"))?;
                let raw = crate::app::generate::run_builtin_trait(
                    "explain",
                    vec![
                        crate::app::generate::runtime_input(
                            "source-trait-id",
                            request.trait_id.as_str(),
                        ),
                        crate::app::generate::runtime_input(
                            "receipt-digest",
                            evidence.scaffold.receipt_digest.as_str(),
                        ),
                        crate::app::generate::runtime_input("scaffold", scaffold_text),
                    ],
                    &[],
                    None,
                    None,
                )
                .map_err(|error| error.to_string())?
                .output;
                let explanation =
                    ctx_traits_core::assist::decode_explain_narration(&raw, &evidence.scaffold)?;
                Ok(ctx_traits_io::cache::TraitExplanation {
                    version: 1,
                    trait_id: request.trait_id.clone(),
                    canonical_digest: request.canonical_digest.clone(),
                    explanation,
                })
            },
        )?;
        Ok(record.explanation)
    })();
    ExplanationResult {
        trait_id: request.trait_id,
        canonical_digest: request.canonical_digest,
        result: result.map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fail_center_projection_for_row;
    use super::*;
    use crate::app::test_support::{
        CenterPeer, accept_center_client, accept_center_subscription, read_center_request,
        write_center_response,
    };
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    #[test]
    fn start_action_result_reports_pre_registration_stderr() {
        let result = start_action_result(
            Ok(StartResult::Exited {
                code: Some(17),
                stderr: "first line\nsecond line\n".to_string(),
            }),
            Some("task".to_string()),
            None,
        );
        assert!(result.message.contains("exit 17"));
        assert!(result.message.contains("first line\nsecond line\n"));
        assert!(result.session_id.is_none());
        assert_eq!(result.task_key.as_deref(), Some("task"));

        let result = start_action_result(
            Err(ctx_traits_io::Error::Usage {
                message: "unavailable".to_string(),
            }),
            Some("task".to_string()),
            None,
        );
        assert!(result.session_id.is_none());
        assert_eq!(result.task_key.as_deref(), Some("task"));
    }

    /// Own the worker channels and join its thread after dropping the command
    /// sender, so live protocol tests cannot leave a reconnecting worker behind.
    struct WorkerLoop {
        commands: Option<mpsc::Sender<Command>>,
        snapshots: mpsc::Receiver<RefreshResult>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl WorkerLoop {
        fn start() -> Self {
            Self::start_named("dashboard-worker-test")
        }

        fn start_named(name: &str) -> Self {
            let (commands, command_rx) = mpsc::channel();
            let (snapshot_tx, snapshots) = mpsc::channel();
            let (preview_tx, _preview_rx) = mpsc::channel();
            let (trait_detail_tx, _trait_detail_rx) = mpsc::channel();
            let (explanation_tx, _explanation_rx) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    run(
                        command_rx,
                        snapshot_tx,
                        preview_tx,
                        trait_detail_tx,
                        explanation_tx,
                    )
                })
                .expect("spawn worker");
            Self {
                commands: Some(commands),
                snapshots,
                worker: Some(worker),
            }
        }

        fn send(&self, command: Command) {
            self.commands
                .as_ref()
                .expect("worker is running")
                .send(command)
                .expect("send worker command");
        }

        fn recv_timeout(&self, timeout: Duration) -> Result<RefreshResult, mpsc::RecvTimeoutError> {
            self.snapshots.recv_timeout(timeout)
        }
    }

    impl Drop for WorkerLoop {
        fn drop(&mut self) {
            self.commands.take();
            if let Some(worker) = self.worker.take() {
                worker.join().expect("join worker");
            }
        }
    }

    fn finish_snapshot(stream: &mut UnixStream) {
        writeln!(stream, "{{\"kind\":\"snapshot-end\"}}").expect("write snapshot end");
    }

    fn row(status: &str, title: &str) -> ctx_traits_io::center::CenterPublicRow {
        serde_json::from_value(serde_json::json!({
            "summary": {
                "session_id": "session",
                "run_id": "run",
                "trait_id": "trait",
                "status": status,
                "title": title,
                "has_merge_frames": false,
            },
            "repo_key": "repo",
            "repo_path": "/repo",
            "ledger_path": "/runs/repo/session.json",
            "live": false,
            "modified_epoch_secs": 0,
        }))
        .expect("center row")
    }

    fn current_repo_row(status: &str, title: &str) -> ctx_traits_io::center::CenterPublicRow {
        let mut row = row(status, title);
        row.repo_key = ctx_traits_io::state::current_repo_key().expect("repository key");
        row.repo_path = std::env::current_dir()
            .expect("current directory")
            .display()
            .to_string();
        row
    }

    fn parked_ask_row(title: &str) -> ctx_traits_io::center::CenterPublicRow {
        let mut value = serde_json::to_value(current_repo_row("waiting-on-human", title))
            .expect("encode center row");
        let summary = value["summary"].as_object_mut().expect("row summary");
        summary.insert("next_frame_kind".to_string(), serde_json::json!("ask"));
        summary.insert("interrupted".to_string(), serde_json::json!(false));
        serde_json::from_value(value).expect("parked Ask row")
    }

    #[test]
    fn refresh_emits_an_empty_snapshot() {
        let (snapshots, results) = mpsc::channel();
        let (previews, _preview_results) = mpsc::channel();
        let (trait_details, _trait_detail_results) = mpsc::channel();
        let (explanations, _explanation_results) = mpsc::channel();
        let mut state = State::new_without_worker();
        let rows = HashMap::new();

        handle_one_command(
            Command::Refresh,
            &snapshots,
            &previews,
            &trait_details,
            &explanations,
            &mut state,
            &rows,
            false,
        )
        .expect("emit empty snapshot");

        let snapshot = results
            .recv()
            .expect("snapshot result")
            .expect("successful empty snapshot");
        assert!(snapshot.sessions.is_empty());
        assert!(
            !snapshot.clear_refresh_error,
            "only a completed subscription snapshot clears an outage warning"
        );
    }

    #[test]
    fn command_refresh_preserves_the_subscription_owned_session_model() {
        let _lock = crate::app::test_support::center_environment_lock();
        let (snapshots, results) = mpsc::channel();
        let (previews, _preview_results) = mpsc::channel();
        let (trait_details, _trait_detail_results) = mpsc::channel();
        let (explanations, _explanation_results) = mpsc::channel();
        let mut state = State::new_without_worker();
        state.all_repos = true;
        let center_row = row("awaiting-agent-output", "parked title");
        let rows = HashMap::from([(center_row.ledger_path.clone(), center_row)]);

        emit_subscription_snapshot(&snapshots, &mut state, &rows, None)
            .expect("seed subscription snapshot");
        let _ = results
            .recv()
            .expect("seed snapshot")
            .expect("seed success");

        handle_one_command(
            Command::Refresh,
            &snapshots,
            &previews,
            &trait_details,
            &explanations,
            &mut state,
            &rows,
            false,
        )
        .expect("emit command snapshot");

        let snapshot = results
            .recv()
            .expect("command snapshot")
            .expect("command success");
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].session_id, "session");
    }

    #[test]
    fn reconnect_backoff_services_every_queued_command() {
        let (commands, command_rx) = mpsc::channel();
        let (snapshots, results) = mpsc::channel();
        let (previews, _preview_results) = mpsc::channel();
        let (trait_details, _trait_detail_results) = mpsc::channel();
        let (explanations, _explanation_results) = mpsc::channel();
        let mut state = State::new_without_worker();
        let rows = HashMap::new();
        commands
            .send(Command::Refresh)
            .expect("queue first command");
        commands
            .send(Command::Refresh)
            .expect("queue second command");

        wait_for_retry(
            &command_rx,
            &snapshots,
            &previews,
            &trait_details,
            &explanations,
            &mut state,
            &rows,
            false,
        )
        .expect("commands remain serviceable during retry");

        for _ in 0..2 {
            results
                .recv()
                .expect("queued command snapshot")
                .expect("queued command succeeds");
        }
    }

    #[test]
    fn recovery_snapshot_is_not_hidden_by_a_later_command_refresh() {
        let (commands, _command_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let (_preview_tx, previews) = mpsc::channel();
        let (_trait_detail_tx, trait_details) = mpsc::channel();
        let (_explanation_tx, explanations) = mpsc::channel();
        let (action_sender, actions) = mpsc::channel();
        let handle = Handle {
            commands,
            snapshots,
            previews,
            trait_details,
            explanations,
            actions,
            action_sender,
        };
        let state = State::new_without_worker();
        let mut recovered = DashboardSnapshot::from_state(&state);
        recovered.clear_refresh_error = true;
        let mut command = DashboardSnapshot::from_state(&state);
        command.clear_refresh_error = false;
        snapshot_tx
            .send(Ok(Arc::new(recovered)))
            .expect("send recovery");
        snapshot_tx
            .send(Ok(Arc::new(command)))
            .expect("send command refresh");

        let results = handle.refresh_results();
        assert_eq!(results.len(), 2);
        assert!(results[0].as_ref().expect("recovery").clear_refresh_error);
        assert!(!results[1].as_ref().expect("command").clear_refresh_error);
    }

    #[test]
    fn outage_message_keeps_the_last_completed_snapshot_time() {
        let snapshot_at = std::time::UNIX_EPOCH + Duration::from_secs(3_723);
        assert_eq!(
            center_unreachable(Some(snapshot_at), "retry failed"),
            "center unreachable — showing state as of 01:02:03 (retry failed)"
        );
        assert_eq!(
            center_unreachable(None, "connect failed"),
            "center unreachable — no session snapshot available (connect failed)"
        );
    }

    #[test]
    fn failed_subscription_projection_does_not_advance_outage_timestamp() {
        let previous = std::time::UNIX_EPOCH + Duration::from_secs(3_723);
        let failed_at = std::time::UNIX_EPOCH + Duration::from_secs(7_446);
        let mut last_snapshot_at = Some(previous);

        record_accepted_snapshot_time(&mut last_snapshot_at, false, failed_at);

        assert_eq!(
            center_unreachable(last_snapshot_at, "subscription closed"),
            "center unreachable — showing state as of 01:02:03 (subscription closed)"
        );
    }

    #[test]
    fn failed_complete_snapshot_projection_keeps_the_accepted_rows_and_reconnects() {
        let peer_server = CenterPeer::install("failed-complete-projection");
        let mut initial = current_repo_row("awaiting-agent-output", "rejected");
        initial.ledger_path = "/runs/failed-complete-projection/session.json".to_string();
        let mut recovered = current_repo_row("awaiting-agent-output", "recovered");
        recovered.ledger_path = initial.ledger_path.clone();
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut first = accept_center_subscription(&listener);
            let row = serde_json::to_string(&initial).expect("encode rejected row");
            writeln!(first, "{{\"kind\":\"snapshot-row\",\"row\":{row}}}")
                .expect("write rejected snapshot row");
            finish_snapshot(&mut first);
            drop(first);

            let mut second = accept_center_subscription(&listener);
            let row = serde_json::to_string(&recovered).expect("encode recovered row");
            writeln!(second, "{{\"kind\":\"snapshot-row\",\"row\":{row}}}")
                .expect("write recovered snapshot row");
            finish_snapshot(&mut second);
        });

        fail_center_projection_for_row(
            "failed-complete-projection",
            "/runs/failed-complete-projection/session.json",
            "rejected",
        );
        let worker = WorkerLoop::start_named("failed-complete-projection");
        let error = match worker
            .recv_timeout(Duration::from_secs(3))
            .expect("rejected projection result")
        {
            Ok(_) => panic!("injected projection must fail"),
            Err(error) => error,
        };
        assert!(error.contains("injected center projection failure"));

        worker.send(Command::Render {
            all_repos: true,
            screen: Screen::Sessions,
        });
        let retained = worker
            .recv_timeout(Duration::from_secs(3))
            .expect("render accepted rows")
            .expect("render succeeds");
        assert!(
            retained.sessions.is_empty(),
            "rejected snapshot was not accepted"
        );
        let recovered = worker
            .recv_timeout(Duration::from_secs(8))
            .expect("recovered snapshot")
            .expect("recovered projection succeeds");
        assert_eq!(recovered.sessions[0].title.as_deref(), Some("recovered"));
        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn failed_delta_projection_keeps_the_accepted_rows_and_reconnects() {
        let peer_server = CenterPeer::install("failed-delta-projection");
        let mut before = current_repo_row("awaiting-agent-output", "before");
        before.ledger_path = "/runs/failed-delta-projection/session.json".to_string();
        let mut after = current_repo_row("awaiting-agent-output", "after");
        after.ledger_path = before.ledger_path.clone();
        let (release_delta, delta_released) = mpsc::channel();
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut first = accept_center_subscription(&listener);
            let row = serde_json::to_string(&before).expect("encode before row");
            writeln!(first, "{{\"kind\":\"snapshot-row\",\"row\":{row}}}")
                .expect("write initial snapshot row");
            finish_snapshot(&mut first);
            delta_released.recv().expect("release delta");
            let row = serde_json::to_string(&after).expect("encode after row");
            writeln!(
                first,
                "{{\"kind\":\"delta\",\"delta\":{{\"type\":\"row-changed\",\"row\":{row}}}}}"
            )
            .expect("write rejected delta");
            drop(first);

            let mut second = accept_center_subscription(&listener);
            let row = serde_json::to_string(&after).expect("encode recovered row");
            writeln!(second, "{{\"kind\":\"snapshot-row\",\"row\":{row}}}")
                .expect("write recovered snapshot row");
            finish_snapshot(&mut second);
        });

        let worker = WorkerLoop::start_named("failed-delta-projection");
        let initial = worker
            .recv_timeout(Duration::from_secs(3))
            .expect("initial snapshot")
            .expect("initial projection succeeds");
        assert_eq!(initial.sessions[0].title.as_deref(), Some("before"));
        fail_center_projection_for_row(
            "failed-delta-projection",
            "/runs/failed-delta-projection/session.json",
            "after",
        );
        release_delta.send(()).expect("release peer delta");
        let error = match worker
            .recv_timeout(Duration::from_secs(3))
            .expect("rejected delta result")
        {
            Ok(_) => panic!("injected delta projection must fail"),
            Err(error) => error,
        };
        assert!(error.contains("injected center projection failure"));

        worker.send(Command::Render {
            all_repos: true,
            screen: Screen::Sessions,
        });
        let retained = worker
            .recv_timeout(Duration::from_secs(3))
            .expect("render accepted rows")
            .expect("render succeeds");
        assert_eq!(retained.sessions[0].title.as_deref(), Some("before"));
        let recovered = worker
            .recv_timeout(Duration::from_secs(8))
            .expect("recovered snapshot")
            .expect("recovered projection succeeds");
        assert_eq!(recovered.sessions[0].title.as_deref(), Some("after"));
        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn parked_ask_presentation_survives_a_render_with_an_unavailable_center_get() {
        let peer_server = CenterPeer::install("parked-ask-render");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_client(&listener);
            let request = read_center_request(&stream);
            assert_eq!(request["kind"], "get");
            write_center_response(
                &mut stream,
                &request,
                serde_json::json!({"type": "error", "data": {"message": "center unavailable"}}),
            );
        });
        let (snapshots, results) = mpsc::channel();
        let (previews, _preview_results) = mpsc::channel();
        let (trait_details, _trait_detail_results) = mpsc::channel();
        let (explanations, _explanation_results) = mpsc::channel();
        let mut state = State::new_without_worker();
        state.all_repos = true;
        let center_row = parked_ask_row("waiting for approval");
        let rows = HashMap::from([(center_row.ledger_path.clone(), center_row)]);
        state
            .reload_from_center_rows(&rows.values().cloned().collect::<Vec<_>>(), false)
            .expect("seed accepted projection");
        state.sessions[0].phase = "ask: approve the change (wait 1m)".to_string();

        handle_one_command(
            Command::Render {
                all_repos: true,
                screen: Screen::Sessions,
            },
            &snapshots,
            &previews,
            &trait_details,
            &explanations,
            &mut state,
            &rows,
            false,
        )
        .expect("render accepted rows");
        let snapshot = results
            .recv()
            .expect("render result")
            .expect("render snapshot");
        let phase = &snapshot.sessions[0].phase;
        assert!(phase.starts_with("ask: approve the change (wait 1m); "));
        assert!(phase.contains("has no pinned document"));
        assert_eq!(phase.matches("has no pinned document").count(), 1);
        peer.join().expect("join center peer");
    }

    #[test]
    fn accepted_delta_does_not_advance_completed_snapshot_time() {
        let completed_snapshot = std::time::UNIX_EPOCH + Duration::from_secs(3_723);
        let last_snapshot_at = Some(completed_snapshot);

        // Deltas update the accepted row map, but the outage footer promises
        // the time of the last complete snapshot, not a partial update.
        assert_eq!(
            center_unreachable(last_snapshot_at, "subscription closed"),
            "center unreachable — showing state as of 01:02:03 (subscription closed)"
        );
    }

    #[test]
    fn terminal_updates_and_title_changes_replace_the_retained_row() {
        let running = row("awaiting-agent-output", "before title");
        let completed = row("completed", "after title");
        let mut rows = HashMap::from([(running.ledger_path.clone(), running)]);

        apply_delta(
            &mut rows,
            ctx_traits_io::center::CenterDelta::RowChanged {
                row: Box::new(completed),
            },
        );

        let retained = rows.values().next().expect("terminal row retained");
        assert_eq!(
            retained.summary.status,
            ctx_traits_core::procedure::session::Status::Completed
        );
        assert_eq!(retained.summary.title.as_deref(), Some("after title"));
    }

    #[test]
    fn ended_delta_removes_the_row() {
        let row = row("completed", "title");
        let mut rows = HashMap::from([(row.ledger_path.clone(), row.clone())]);

        apply_delta(
            &mut rows,
            ctx_traits_io::center::CenterDelta::Ended { row: Box::new(row) },
        );

        assert!(rows.is_empty());
    }

    #[test]
    fn activity_delta_requests_an_unchanged_selected_preview_refresh() {
        let _lock = crate::app::test_support::center_environment_lock();
        let mut rows = HashMap::new();
        let center_row = row("awaiting-agent-output", "activity title");

        let changed_ledger_path = apply_delta(
            &mut rows,
            ctx_traits_io::center::CenterDelta::ActivityLine {
                row: Box::new(center_row.clone()),
                activity: ctx_traits_io::activity_sidecar::ActivityRecord::SessionTitle {
                    at_epoch_ms: 0,
                    title: "activity title".to_string(),
                },
            },
        );
        assert_eq!(
            changed_ledger_path.as_deref(),
            Some(center_row.ledger_path.as_str())
        );
        assert!(rows.is_empty(), "activity does not alter the row model");

        let (snapshots, results) = mpsc::channel();
        let mut state = State::new_without_worker();
        emit_subscription_snapshot(&snapshots, &mut state, &rows, changed_ledger_path)
            .expect("emit activity snapshot");
        assert_eq!(
            results
                .recv()
                .expect("activity result")
                .expect("activity snapshot")
                .changed_ledger_path
                .as_deref(),
            Some(center_row.ledger_path.as_str()),
            "activity reports the changed ledger path to the renderer"
        );
    }

    #[test]
    fn worker_delta_snapshots_update_and_remove_renderer_state() {
        let _lock = crate::app::test_support::center_environment_lock();
        let (snapshots, results) = mpsc::channel();
        let mut worker_state = State::new_without_worker();
        // ALL scope keeps this worker/state boundary focused on subscription
        // deltas rather than the test process's repository identity.
        worker_state.all_repos = true;
        let running = row("awaiting-agent-output", "before completion");
        let completed = row("completed", "completed title");
        let mut rows = HashMap::from([(running.ledger_path.clone(), running)]);

        apply_delta(
            &mut rows,
            ctx_traits_io::center::CenterDelta::RowChanged {
                row: Box::new(completed.clone()),
            },
        );
        emit_subscription_snapshot(&snapshots, &mut worker_state, &rows, None)
            .expect("emit terminal snapshot");
        let terminal = results
            .recv()
            .expect("terminal result")
            .expect("terminal snapshot");
        let mut renderer_state = State::new_without_worker();
        renderer_state.apply_snapshot(&terminal);
        assert_eq!(
            renderer_state.sessions.len(),
            1,
            "terminal update retains its row"
        );
        assert_eq!(renderer_state.sessions[0].session_id, "session");

        apply_delta(
            &mut rows,
            ctx_traits_io::center::CenterDelta::Ended {
                row: Box::new(completed),
            },
        );
        emit_subscription_snapshot(&snapshots, &mut worker_state, &rows, None)
            .expect("emit removal snapshot");
        let removed = results
            .recv()
            .expect("removal result")
            .expect("removal snapshot");
        renderer_state.apply_snapshot(&removed);
        assert!(
            renderer_state.sessions.is_empty(),
            "only Ended removes the row"
        );
    }

    #[test]
    fn worker_run_applies_an_external_delta_without_a_refresh_command() {
        let peer_server = CenterPeer::install("external-delta");

        let mut appeared = row("awaiting-agent-output", "external run");
        appeared.repo_key = ctx_traits_io::state::current_repo_key().expect("repository key");
        appeared.repo_path = std::env::current_dir()
            .expect("current directory")
            .display()
            .to_string();
        appeared.ledger_path = "/runs/external/session.json".to_string();
        let peer_row = appeared.clone();
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            finish_snapshot(&mut stream);
            let row = serde_json::to_string(&peer_row).expect("encode center row");
            writeln!(
                stream,
                "{{\"kind\":\"delta\",\"delta\":{{\"type\":\"appeared\",\"row\":{row}}}}}"
            )
            .expect("write appeared delta");
        });

        let worker = WorkerLoop::start();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut saw_external_row = false;
        let mut worker_errors = Vec::new();
        while std::time::Instant::now() < deadline {
            if let Ok(result) = worker.recv_timeout(Duration::from_millis(100)) {
                match result {
                    Ok(snapshot)
                        if snapshot
                            .sessions
                            .iter()
                            .any(|session| session.session_id == "session") =>
                    {
                        saw_external_row = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => worker_errors.push(error),
                }
            }
        }
        assert!(
            saw_external_row,
            "subscription delta must update the worker model without Command::Refresh: {worker_errors:?}"
        );

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_applies_a_title_row_change_before_frame_completion() {
        let peer_server = CenterPeer::install("title-row-change");
        let mut initial = current_repo_row("awaiting-agent-output", "initial title");
        initial.ledger_path = "/runs/title/session.json".to_string();
        let mut titled = initial.clone();
        titled.summary.title = Some("title from activity".to_string());
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            let initial = serde_json::to_string(&initial).expect("encode initial row");
            writeln!(stream, "{{\"kind\":\"snapshot-row\",\"row\":{initial}}}")
                .expect("write initial row");
            finish_snapshot(&mut stream);
            let titled = serde_json::to_string(&titled).expect("encode titled row");
            writeln!(
                stream,
                "{{\"kind\":\"delta\",\"delta\":{{\"type\":\"row-changed\",\"row\":{titled}}}}}"
            )
            .expect("write title row change");
        });

        let worker = WorkerLoop::start();
        let initial = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("initial snapshot")
            .expect("initial snapshot success");
        assert_eq!(initial.sessions[0].title.as_deref(), Some("initial title"));
        let updated = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("title update")
            .expect("title update success");
        assert_eq!(
            updated.sessions[0].title.as_deref(),
            Some("title from activity")
        );
        assert_eq!(
            updated.sessions[0].status,
            Some(ctx_traits_core::procedure::session::Status::AwaitingAgentOutput),
            "the title update arrives while the frame is still active"
        );

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_retains_terminal_rows_and_removes_only_ended_rows() {
        let peer_server = CenterPeer::install("terminal-and-ended");

        let mut running = row("awaiting-agent-output", "running");
        running.repo_key = ctx_traits_io::state::current_repo_key().expect("repository key");
        running.repo_path = std::env::current_dir()
            .expect("current directory")
            .display()
            .to_string();
        running.ledger_path = "/runs/terminal/session.json".to_string();
        let mut completed = running.clone();
        completed.summary.status = ctx_traits_core::procedure::session::Status::Completed;
        completed.summary.title = Some("completed".to_string());
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            let running = serde_json::to_string(&running).expect("encode running row");
            writeln!(stream, "{{\"kind\":\"snapshot-row\",\"row\":{running}}}")
                .expect("write snapshot row");
            finish_snapshot(&mut stream);
            let completed = serde_json::to_string(&completed).expect("encode completed row");
            writeln!(
                stream,
                "{{\"kind\":\"delta\",\"delta\":{{\"type\":\"row-changed\",\"row\":{completed}}}}}"
            )
            .expect("write terminal delta");
            writeln!(
                stream,
                "{{\"kind\":\"delta\",\"delta\":{{\"type\":\"ended\",\"row\":{completed}}}}}"
            )
            .expect("write ended delta");
        });

        let worker = WorkerLoop::start();
        let initial = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("initial snapshot")
            .expect("initial snapshot success");
        assert_eq!(initial.sessions.len(), 1);
        let terminal = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("terminal update")
            .expect("terminal update success");
        assert_eq!(terminal.sessions.len(), 1, "terminal rows remain visible");
        assert_eq!(
            terminal.sessions[0].status,
            Some(ctx_traits_core::procedure::session::Status::Completed)
        );
        let removed = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("ended update")
            .expect("ended update success");
        assert!(removed.sessions.is_empty(), "only Ended deletes the row");

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_retains_state_during_disconnect_and_clears_it_after_reconnect() {
        let peer_server = CenterPeer::install("reconnect");

        let mut initial = row("awaiting-agent-output", "retained while offline");
        initial.repo_key = ctx_traits_io::state::current_repo_key().expect("repository key");
        initial.repo_path = std::env::current_dir()
            .expect("current directory")
            .display()
            .to_string();
        initial.ledger_path = "/runs/reconnect/session.json".to_string();
        let recovered = initial.clone();
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            for row in [initial, recovered] {
                let mut stream = accept_center_subscription(&listener);
                let encoded_row = serde_json::to_string(&row).expect("encode center row");
                writeln!(
                    stream,
                    "{{\"kind\":\"snapshot-row\",\"row\":{encoded_row}}}"
                )
                .expect("write snapshot row");
                finish_snapshot(&mut stream);
                // Closing the first stream forces the worker to demonstrate its
                // stale-state error and its next subscription recovery.
            }
        });

        let worker = WorkerLoop::start();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_initial = false;
        let mut saw_outage = false;
        let mut saw_recovery = false;
        while std::time::Instant::now() < deadline && !saw_recovery {
            match worker.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(snapshot)) if !saw_initial => {
                    saw_initial = snapshot
                        .sessions
                        .iter()
                        .any(|session| session.session_id == "session");
                }
                Ok(Err(error)) => {
                    saw_outage = error.contains("showing state as of");
                }
                Ok(Ok(snapshot)) => {
                    saw_recovery = snapshot.clear_refresh_error
                        && snapshot
                            .sessions
                            .iter()
                            .any(|session| session.session_id == "session");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(saw_initial, "first snapshot must be rendered before outage");
        assert!(
            saw_outage,
            "disconnect must retain and describe stale state"
        );
        assert!(
            saw_recovery,
            "completed reconnect snapshot must clear outage state"
        );

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_backs_off_after_repeated_subscription_disconnects() {
        let peer_server = CenterPeer::install("disconnect-backoff");
        let listener = peer_server.listener();
        listener
            .set_nonblocking(true)
            .expect("make center listener nonblocking");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peer_attempts = Arc::clone(&attempts);
        let peer = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(1_250);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        peer_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted stream blocking");
                        crate::app::test_support::complete_center_hello(&mut stream);
                        let _request = crate::app::test_support::read_center_request(&stream);
                        // Dropping the stream immediately after the request
                        // simulates a center that repeatedly closes streams.
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept worker: {error}"),
                }
            }
        });

        let worker = WorkerLoop::start();
        let outage = match worker
            .recv_timeout(Duration::from_secs(2))
            .expect("subscription closure reports an outage")
        {
            Err(error) => error,
            Ok(_) => panic!("closed subscription must report an outage"),
        };
        assert!(outage.contains("subscription closed"));
        worker.send(Command::Refresh);
        worker
            .recv_timeout(Duration::from_secs(1))
            .expect("queued refresh remains serviceable during backoff")
            .expect("non-session refresh succeeds");
        drop(worker);
        peer.join().expect("join center peer");
        assert!(
            attempts.load(std::sync::atomic::Ordering::Relaxed) <= 3,
            "reconnects must be rate-limited after established-stream closures"
        );
    }

    #[test]
    fn worker_run_refresh_command_does_not_query_the_center() {
        let peer_server = CenterPeer::install("refresh-isolation");

        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            finish_snapshot(&mut stream);

            // Once subscribed, `Refresh` is local non-session work. A request
            // here would prove it had reintroduced center polling.
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .expect("set peer read timeout");
            let mut byte = [0_u8; 1];
            match stream.read(&mut byte) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Ok(0) => {}
                Ok(_) => panic!("refresh sent an unexpected center request"),
                Err(error) => panic!("read peer request: {error}"),
            }
        });

        let worker = WorkerLoop::start();
        worker
            .recv_timeout(Duration::from_secs(2))
            .expect("initial subscription snapshot")
            .expect("initial snapshot success");
        worker.send(Command::Refresh);
        worker
            .recv_timeout(Duration::from_secs(2))
            .expect("command snapshot")
            .expect("command snapshot success");

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_render_reprojects_the_accepted_rows_without_resubscribing() {
        let peer_server = CenterPeer::install("render-isolation");
        let mut scoped = current_repo_row("awaiting-agent-output", "scoped");
        scoped.ledger_path = "/runs/scoped/session.json".to_string();
        let mut other = row("awaiting-agent-output", "other");
        other.repo_key = "other-repository".to_string();
        other.ledger_path = "/runs/other/session.json".to_string();
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            for row in [&scoped, &other] {
                let row = serde_json::to_string(row).expect("encode center row");
                writeln!(stream, "{{\"kind\":\"snapshot-row\",\"row\":{row}}}")
                    .expect("write snapshot row");
            }
            finish_snapshot(&mut stream);
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .expect("set peer read timeout");
            let mut byte = [0_u8; 1];
            match stream.read(&mut byte) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Ok(0) => {}
                Ok(_) => panic!("render sent an unexpected center request"),
                Err(error) => panic!("read peer request: {error}"),
            }
        });

        let worker = WorkerLoop::start();
        let scoped_snapshot = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("initial snapshot")
            .expect("initial snapshot success");
        assert_eq!(scoped_snapshot.sessions.len(), 1);
        worker.send(Command::Render {
            all_repos: true,
            screen: Screen::Sessions,
        });
        let all_repos_snapshot = worker
            .recv_timeout(Duration::from_secs(2))
            .expect("render snapshot")
            .expect("render snapshot success");
        assert_eq!(all_repos_snapshot.sessions.len(), 2);

        drop(worker);
        peer.join().expect("join center peer");
    }

    #[test]
    fn worker_run_exits_when_command_channel_closes_with_live_subscription() {
        let peer_server = CenterPeer::install("command-channel-close");
        let listener = peer_server.listener();
        let peer = std::thread::spawn(move || {
            let mut stream = accept_center_subscription(&listener);
            finish_snapshot(&mut stream);
            // Keep the protocol peer open. Worker shutdown must be driven by
            // command-channel disconnect, not subscription disconnect.
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set peer read timeout");
            let mut byte = [0_u8; 1];
            assert_eq!(stream.read(&mut byte).expect("read worker closure"), 0);
        });

        let (commands, command_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let (preview_tx, _preview_rx) = mpsc::channel();
        let (explanation_tx, _explanation_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (trait_detail_tx, _trait_detail_rx) = mpsc::channel();
            run(
                command_rx,
                snapshot_tx,
                preview_tx,
                trait_detail_tx,
                explanation_tx,
            );
            let _ = exited_tx.send(());
        });

        snapshots
            .recv_timeout(Duration::from_secs(2))
            .expect("initial subscription snapshot")
            .expect("initial snapshot success");
        drop(commands);
        exited_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker exits after command channel disconnect");
        worker.join().expect("join worker");
        peer.join().expect("join center peer");
    }
}
