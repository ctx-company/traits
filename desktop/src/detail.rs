//! Run-detail state for a selected center row: plain Rust, no gpui types, so
//! it is unit-testable without an `App` — the same pattern `dashboard.rs`
//! and `run_row.rs` document.
//!
//! # Resolution
//!
//! A selection resolves to `RunRow::ledger_path` — the center's own primary
//! key for a run, the absolute path the center already resolved when it
//! indexed that ledger. This is deliberately **not** re-derived from
//! `ctx_traits_io::state::global_runs_root(&row.repo_key)` plus
//! `run_session::session_store_path(..)`: the center's runs root is
//! env-overridable (`CTX_CENTER_RUNS_ROOT`), so that derivation diverges from
//! the row's real `ledger_path` whenever the center is not serving out of the
//! global runs root — including in this crate's own integration tests. It
//! would also re-perform, from a second source, a resolution the center
//! already performed and shipped on the wire, which is exactly the kind of
//! GUI-private index this task forbids. `repo_key` still travels alongside
//! the ledger path, as part of the selection key and of failure text, so two
//! same-named runs in different repositories stay distinct at this layer too.
//!
//! The one read this module performs goes through
//! `ctx_traits_io::run_session::read_run_session`, the existing IO store
//! boundary (symlink rejection, typed parse errors) — never a hand-rolled
//! `std::fs::read_to_string` + `serde_json` in this crate.

use camino::Utf8PathBuf;
use ctx_traits_core::procedure::session::Session;

use crate::run_row::RunRow;

/// A background read request for one selection. Carries the generation it
/// was issued under, so a caller can tell a stale outcome from the current
/// one without any extra bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    pub generation: u64,
    pub repo_key: String,
    pub ledger_path: Utf8PathBuf,
}

/// The one filesystem read this module performs. Read-only; the io error is
/// converted to `String` here, inside the (future) background task, so the
/// value crossing back to the UI thread is plainly `Send`.
pub fn load(request: &LoadRequest) -> Result<Session, String> {
    ctx_traits_io::run_session::read_run_session(&request.ledger_path)
        .map_err(|error| error.to_string())
}

/// A selected run's load state. `Session` is boxed because it is large
/// enough to trip `clippy::large_enum_variant` otherwise.
#[derive(Debug, Clone, PartialEq)]
pub enum DetailLoad {
    Loading,
    Loaded(Box<Session>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Selection {
    key: String,
    repo_key: String,
    generation: u64,
    load: DetailLoad,
}

/// The desktop's run-detail state. A sibling of `CenterFace`/`Dashboard`, not
/// a member of either: a center reconnect installs a fresh `Dashboard`
/// snapshot but leaves detail untouched (no re-read — "exactly once" holds
/// across reconnects), and a delta cannot reach detail at all in this slice.
#[derive(Default)]
pub struct RunDetail {
    selected: Option<Selection>,
    generation: u64,
}

impl RunDetail {
    /// Select `row`, returning a [`LoadRequest`] the caller must run on a
    /// background executor, or `None` when no read is needed.
    ///
    /// Re-selecting the run that is already the current selection is a
    /// no-op in every state (`Loading`, `Loaded`, `Failed`) — this is the
    /// "exactly once" rule. There is no retry-on-reclick: retry/refresh is
    /// 0257.3's live-follow concern, and a silent second full-ledger read is
    /// precisely what this task's contract forbids.
    ///
    /// A row with an empty `ledger_path` is defensive-only (the center
    /// always populates it): it installs a `Failed` selection and performs
    /// no read, since there is no path to read from.
    pub fn select(&mut self, row: &RunRow) -> Option<LoadRequest> {
        if row.ledger_path.is_empty() {
            self.selected = Some(Selection {
                key: String::new(),
                repo_key: row.repo_key.clone(),
                generation: self.generation,
                load: DetailLoad::Failed("run row carries no ledger path".to_string()),
            });
            return None;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|selection| selection.key == row.ledger_path)
        {
            return None;
        }
        self.generation += 1;
        let generation = self.generation;
        self.selected = Some(Selection {
            key: row.ledger_path.clone(),
            repo_key: row.repo_key.clone(),
            generation,
            load: DetailLoad::Loading,
        });
        Some(LoadRequest {
            generation,
            repo_key: row.repo_key.clone(),
            ledger_path: Utf8PathBuf::from(row.ledger_path.clone()),
        })
    }

    /// Apply a background load's outcome. Returns whether the visible state
    /// actually changed, so the caller can `cx.notify()` only on real
    /// change — the same discipline `Dashboard::apply` established in
    /// 0256.4.
    ///
    /// An outcome tagged with a superseded generation (a stale selection's
    /// result landing late) is ignored.
    pub fn apply(&mut self, generation: u64, outcome: Result<Session, String>) -> bool {
        let Some(selection) = self.selected.as_mut() else {
            return false;
        };
        if selection.generation != generation {
            return false;
        }
        selection.load = match outcome {
            Ok(session) => DetailLoad::Loaded(Box::new(session)),
            Err(reason) => DetailLoad::Failed(reason),
        };
        true
    }

    /// The current selection's key (`ledger_path`), if any.
    pub fn selected_key(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|selection| selection.key.as_str())
    }

    /// The current selection's repository key, if any.
    pub fn repo_key(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|selection| selection.repo_key.as_str())
    }

    /// The current selection's load state, if any.
    pub fn load_state(&self) -> Option<&DetailLoad> {
        self.selected.as_ref().map(|selection| &selection.load)
    }

    /// A single-line acknowledgement of the current selection's state, for
    /// the thinnest possible on-screen proof of this task's behaviour. This
    /// is a **0257.2 placeholder** — real detail presentation is that
    /// task's contract, not this one's.
    pub fn summary_line(&self) -> Option<String> {
        match self.load_state()? {
            DetailLoad::Loading => Some("loading…".to_string()),
            DetailLoad::Failed(reason) => Some(format!("unreadable: {reason}")),
            DetailLoad::Loaded(session) => Some(format!(
                "{} — {} — {}",
                session.run_id.as_str(),
                session.trait_id,
                format!("{:?}", session.status).to_ascii_lowercase(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(repo_key: &str, ledger_path: &str, run_id: &str) -> RunRow {
        RunRow {
            ledger_path: ledger_path.to_string(),
            session_id: format!("{run_id}-session"),
            run_id: run_id.to_string(),
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            repo_label: repo_key.to_string(),
            title: run_id.to_string(),
            trait_id: "fixture-trait".to_string(),
            state: crate::run_row::RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
        }
    }

    /// Built from the same minimal JSON shape `tests/live_deltas.rs` uses,
    /// via `serde_json`, rather than a hand-written `Session` literal: the
    /// struct carries far more fields than this test needs to name, and
    /// `#[serde(default, ..)]` already covers all of them.
    fn session(run_id: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": format!("{run_id}-session"),
            "run-id": run_id,
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "detail-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": run_id,
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "completed",
            },
            "state-digest": format!("sha256:fixture-{run_id}"),
        }))
        .expect("fixture session")
    }

    #[test]
    fn select_yields_a_request_and_performs_no_io() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .expect("first selection issues a request");
        assert_eq!(request.repo_key, "repo-a");
        assert_eq!(request.ledger_path, Utf8PathBuf::from("/repo-a/run.json"));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loading)));
    }

    #[test]
    fn reselecting_the_same_ledger_path_is_a_no_op_in_every_state() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(
            detail
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );

        assert!(detail.apply(request.generation, Ok(session("run-a"))));
        assert!(
            detail
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );

        let mut failed = RunDetail::default();
        let failed_request = failed
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(failed.apply(failed_request.generation, Err("broken".to_string())));
        assert!(
            failed
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );
    }

    #[test]
    fn distinct_repositories_with_identical_run_ids_are_distinct_selections() {
        let mut detail = RunDetail::default();
        let first = detail
            .select(&row("repo-a", "/repo-a/run.json", "shared-run"))
            .unwrap();
        let second = detail
            .select(&row("repo-b", "/repo-b/run.json", "shared-run"))
            .unwrap();
        assert_ne!(first.ledger_path, second.ledger_path);
        assert_ne!(first.generation, second.generation);
    }

    #[test]
    fn a_superseded_generation_outcome_is_ignored() {
        let mut detail = RunDetail::default();
        let first = detail
            .select(&row("repo-a", "/repo-a/run-a.json", "run-a"))
            .unwrap();
        let second = detail
            .select(&row("repo-a", "/repo-a/run-b.json", "run-b"))
            .unwrap();

        assert!(!detail.apply(first.generation, Ok(session("run-a"))));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loading)));

        assert!(detail.apply(second.generation, Ok(session("run-b"))));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loaded(_))));
    }

    #[test]
    fn an_error_outcome_lands_as_failed_never_loaded() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Err("bad json".to_string())));
        assert!(
            matches!(detail.load_state(), Some(DetailLoad::Failed(reason)) if reason == "bad json")
        );
    }

    #[test]
    fn an_empty_ledger_path_selects_into_failed_with_no_request() {
        let mut detail = RunDetail::default();
        let request = detail.select(&row("repo-a", "", "run-a"));
        assert!(request.is_none());
        assert!(matches!(detail.load_state(), Some(DetailLoad::Failed(_))));
    }
}
