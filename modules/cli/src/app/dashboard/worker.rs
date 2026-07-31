//! The dashboard's single IO owner.

use std::sync::{Arc, mpsc};

use super::{
    DashboardSnapshot, Screen, SessionPreviewRequest, State, build_attached_view,
    refresh_attached_view,
};

pub(super) type RefreshResult = Result<Arc<DashboardSnapshot>, String>;

pub(super) struct Handle {
    commands: mpsc::Sender<Command>,
    snapshots: mpsc::Receiver<RefreshResult>,
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
    Refresh {
        all_repos: bool,
        screen: Screen,
        preview: Option<SessionPreviewRequest>,
    },
    Explain(ExplanationRequest),
}

impl Handle {
    pub(super) fn new() -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let (explanation_tx, explanations) = mpsc::channel();
        std::thread::spawn(move || run(command_rx, snapshot_tx, explanation_tx));
        Self {
            commands,
            snapshots,
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

    pub(super) fn refresh(
        &self,
        all_repos: bool,
        screen: Screen,
        preview: Option<SessionPreviewRequest>,
    ) {
        // Refreshes are read-only and may be superseded. A disconnected worker
        // is treated as an unavailable background task, never a TUI failure.
        let _ = self.commands.send(Command::Refresh {
            all_repos,
            screen,
            preview,
        });
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
    explanations: mpsc::Sender<ExplanationResult>,
) {
    let mut state = State::new_without_worker();
    while let Ok(mut command) = commands.recv() {
        if let Command::Explain(request) = command {
            if explanations.send(explain(request)).is_err() {
                return;
            }
            continue;
        }
        // Keep only the newest read-only request received before work starts.
        let mut pending_explanations = Vec::new();
        while let Ok(next) = commands.try_recv() {
            match next {
                Command::Refresh { .. } => command = next,
                Command::Explain(request) => pending_explanations.push(request),
            }
        }
        let Command::Refresh {
            all_repos,
            screen,
            preview,
        } = command
        else {
            unreachable!("explanation commands are handled before refresh coalescing")
        };
        state.all_repos = all_repos;
        state.screen = screen;
        let result: RefreshResult = (|| -> crate::Result<Arc<DashboardSnapshot>> {
            state.reload_sync()?;
            match preview {
                Some(request)
                    if state.session_preview.as_ref().is_some_and(|view| {
                        view.session_id == request.session_id
                            && view.ledger_path == request.ledger_path
                    }) =>
                {
                    if let Some(view) = &mut state.session_preview {
                        refresh_attached_view(view);
                    }
                }
                Some(request) => {
                    state.session_preview = Some(build_attached_view(
                        &request.session_id,
                        &request.ledger_path,
                        &request.run_id,
                    ));
                }
                None => state.session_preview = None,
            }
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

fn explain(request: ExplanationRequest) -> ExplanationResult {
    let result = (|| -> crate::Result<String> {
        let repo_root = ctx_traits_io::state::state_repo_root()?;
        let (record, _) = ctx_traits_io::cache::get_or_generate_trait_explanation(
            &repo_root,
            &request.trait_id,
            &request.canonical_digest,
            || -> Result<ctx_traits_io::cache::TraitExplanation, String> {
                let (trait_ref, _, _, canonical_digest) =
                    ctx_traits_io::run::load_trait(&request.canonical_path)
                        .map_err(|error| error.to_string())?;
                if canonical_digest.as_str() != request.canonical_digest {
                    return Err("selected trait changed before explanation generation".to_string());
                }
                let canonical = serde_json::to_string(&trait_ref)
                    .map_err(|error| format!("serialize canonical trait: {error}"))?;
                let raw = crate::app::generate::run_builtin_trait(
                    "explain-trait",
                    vec![
                        crate::app::generate::runtime_input(
                            "source-trait-id",
                            request.trait_id.as_str(),
                        ),
                        crate::app::generate::runtime_input(
                            "canonical-digest",
                            request.canonical_digest.as_str(),
                        ),
                        crate::app::generate::runtime_input("canonical-trait", canonical),
                    ],
                    &[],
                    None,
                    None,
                )
                .map_err(|error| error.to_string())?;
                let narration = ctx_traits_core::assist::validate_trait_narration(
                    &raw,
                    &request.trait_id,
                    request.canonical_digest.as_str(),
                )?;
                Ok(ctx_traits_io::cache::TraitExplanation {
                    version: 1,
                    trait_id: request.trait_id.clone(),
                    canonical_digest: request.canonical_digest.clone(),
                    explanation: narration.explanation.clone(),
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
