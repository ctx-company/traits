//! The dashboard's single IO owner.

use std::sync::{Arc, mpsc};

use super::{
    AttachedView, DashboardSnapshot, Screen, SessionPreviewRequest, State, build_attached_view,
    refresh_attached_view,
};

pub(super) type RefreshResult = Result<Arc<DashboardSnapshot>, String>;
pub(super) type PreviewResult = AttachedView;

pub(super) struct Handle {
    commands: mpsc::Sender<Command>,
    snapshots: mpsc::Receiver<RefreshResult>,
    previews: mpsc::Receiver<PreviewResult>,
    explanations: mpsc::Receiver<ExplanationResult>,
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

#[derive(Clone)]
enum Command {
    Refresh { all_repos: bool, screen: Screen },
    Preview(SessionPreviewRequest),
    Explain(ExplanationRequest),
}

impl Handle {
    pub(super) fn new() -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let (preview_tx, previews) = mpsc::channel();
        let (explanation_tx, explanations) = mpsc::channel();
        std::thread::spawn(move || run(command_rx, snapshot_tx, preview_tx, explanation_tx));
        Self {
            commands,
            snapshots,
            previews,
            explanations,
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

    pub(super) fn refresh(&self, all_repos: bool, screen: Screen) {
        // Refreshes are read-only and may be superseded. A disconnected worker
        // is treated as an unavailable background task, never a TUI failure.
        let _ = self.commands.send(Command::Refresh { all_repos, screen });
    }

    pub(super) fn preview(&self, request: SessionPreviewRequest) {
        let _ = self.commands.send(Command::Preview(request));
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
        let mut trailing_error = None;
        while let Ok(result) = self.snapshots.try_recv() {
            match result {
                Ok(snapshot) => {
                    latest_snapshot = Some(snapshot);
                    // A newer complete snapshot recovers from older errors.
                    trailing_error = None;
                }
                Err(error) => trailing_error = Some(error),
            }
        }
        latest_snapshot
            .into_iter()
            .map(Ok)
            .chain(trailing_error.into_iter().map(Err))
            .collect()
    }
}

fn run(
    commands: mpsc::Receiver<Command>,
    snapshots: mpsc::Sender<RefreshResult>,
    previews: mpsc::Sender<PreviewResult>,
    explanations: mpsc::Sender<ExplanationResult>,
) {
    let mut state = State::new_without_worker();
    while let Ok(mut command) = commands.recv() {
        match command {
            Command::Explain(request) => {
                if explanations.send(explain(request)).is_err() {
                    return;
                }
                continue;
            }
            Command::Preview(request) => {
                if previews.send(preview(&mut state, request)).is_err() {
                    return;
                }
                continue;
            }
            Command::Refresh { .. } => {}
        }
        // Keep only the newest read-only request received before work starts.
        let (coalesced, pending_previews, pending_explanations) =
            coalesce_refresh_commands(&commands, command);
        command = coalesced;
        let Command::Refresh { all_repos, screen } = command else {
            unreachable!("non-refresh commands are handled before refresh coalescing")
        };
        // Preview work is independent of inventory scanning and must not be
        // discarded when adjacent refreshes coalesce.
        for request in pending_previews {
            if previews.send(preview(&mut state, request)).is_err() {
                return;
            }
        }
        state.all_repos = all_repos;
        state.screen = screen;
        let result: RefreshResult = (|| -> crate::Result<Arc<DashboardSnapshot>> {
            state.reload_sync()?;
            Ok(Arc::new(DashboardSnapshot::from_state(&state)))
        })()
        .map_err(|error| error.to_string());
        if snapshots.send(result).is_err() {
            return;
        }
        for request in pending_explanations {
            if explanations.send(explain(request)).is_err() {
                return;
            }
        }
    }
}

fn coalesce_refresh_commands(
    commands: &mpsc::Receiver<Command>,
    mut command: Command,
) -> (Command, Vec<SessionPreviewRequest>, Vec<ExplanationRequest>) {
    let mut previews = Vec::new();
    let mut explanations = Vec::new();
    while let Ok(next) = commands.try_recv() {
        match next {
            Command::Refresh { .. } => command = next,
            Command::Preview(request) => previews.push(request),
            Command::Explain(request) => explanations.push(request),
        }
    }
    (command, previews, explanations)
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
    use super::*;

    #[test]
    fn preview_between_refreshes_survives_coalescing() {
        let (tx, rx) = mpsc::channel();
        tx.send(Command::Preview(SessionPreviewRequest {
            session_id: "session".to_string(),
            ledger_path: camino::Utf8PathBuf::from("/tmp/session.json"),
            run_id: "run".to_string(),
        }))
        .expect("queue preview");
        tx.send(Command::Refresh {
            all_repos: true,
            screen: Screen::Traits,
        })
        .expect("queue refresh");

        let (command, previews, _) = coalesce_refresh_commands(
            &rx,
            Command::Refresh {
                all_repos: false,
                screen: Screen::Sessions,
            },
        );

        assert!(matches!(
            command,
            Command::Refresh {
                all_repos: true,
                screen: Screen::Traits,
            }
        ));
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].session_id, "session");
    }
}
