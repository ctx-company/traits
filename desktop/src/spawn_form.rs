//! gpui-free spawn state machine: parses text-entry input, validates it
//! through the shared `ctx_traits_io::spawn_request` rule, and tracks one
//! in-flight center spawn request end to end. No gpui types — the same
//! pattern `dashboard.rs`/`detail.rs` document — so it is unit-testable
//! without an `App`.
//!
//! `settle` never touches a `Dashboard`: `SpawnForm` has no field, method,
//! or dependency that could reach one. The type system, not a convention,
//! is what forbids optimistic row insertion — a spawned run becomes visible
//! only through the same subscription-delta path every other row does.

/// A repository the center currently knows about, offered as a spawn
/// target. `repo_path` is always non-empty and absolute —
/// `Dashboard::repositories` filters out any row the center could not
/// resolve one for, since `run_start` rejects an empty or relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRepo {
    pub repo_key: String,
    pub repo_path: String,
    pub label: String,
}

/// The form's visible status. `Requested` is worded as *requested*, not
/// *running*: the row is authoritative only once a delta carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnStatus {
    Idle,
    /// A client-side rejection, before anything reached the wire.
    Invalid(String),
    /// A request is in flight; submit is disabled but close is not.
    Requesting,
    Requested {
        session_id: String,
    },
    /// The center rejected the request, or the child exited before
    /// registering.
    Rejected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitRequest {
    pub args: Vec<String>,
    pub repo_path: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Started { session_id: String },
    Exited { code: Option<i32>, stderr: String },
    Failed(String),
}

pub struct SpawnForm {
    text: String,
    cursor: usize,
    repositories: Vec<SpawnRepo>,
    selected_repo: Option<String>,
    status: SpawnStatus,
    open: bool,
    generation: u64,
}

impl Default for SpawnForm {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            repositories: Vec::new(),
            selected_repo: None,
            status: SpawnStatus::Idle,
            open: false,
            generation: 0,
        }
    }
}

impl SpawnForm {
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn status(&self) -> &SpawnStatus {
        &self.status
    }

    pub fn repositories(&self) -> &[SpawnRepo] {
        &self.repositories
    }

    pub fn selected_repo(&self) -> Option<&str> {
        self.selected_repo.as_deref()
    }

    /// Replace the offered repositories, e.g. from every applied snapshot.
    /// A current selection whose `repo_key` is no longer present is
    /// cleared — a repository that vanished from center state is not a
    /// valid spawn target.
    pub fn set_repositories(&mut self, repositories: Vec<SpawnRepo>) {
        if let Some(selected) = &self.selected_repo
            && !repositories.iter().any(|repo| &repo.repo_key == selected)
        {
            self.selected_repo = None;
        }
        self.repositories = repositories;
    }

    pub fn select_repository(&mut self, repo_key: String) {
        if self
            .repositories
            .iter()
            .any(|repo| repo.repo_key == repo_key)
        {
            self.selected_repo = Some(repo_key);
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        let Some((previous, _)) = self.text[..self.cursor].char_indices().next_back() else {
            return;
        };
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
    }

    pub fn newline(&mut self) {
        self.insert_char('\n');
    }

    /// Validate and, if valid, produce the request the caller must submit
    /// through the center. Returns `None` (and sets `Invalid`) for empty
    /// text, a forbidden flag, or no repository selected — nothing reaches
    /// the wire in any of those cases. Returns `None` with no state change
    /// while a request is already `Requesting`: one in-flight request per
    /// form, so a double-click or a held Enter key cannot double-spawn.
    pub fn submit(&mut self) -> Option<SubmitRequest> {
        if matches!(self.status, SpawnStatus::Requesting) {
            return None;
        }
        let args = match ctx_traits_io::spawn_request::parse_spawn_args(&self.text) {
            Ok(args) => args,
            Err(error) => {
                self.status = SpawnStatus::Invalid(error);
                return None;
            }
        };
        let Some(repo) = self
            .selected_repo
            .as_ref()
            .and_then(|key| self.repositories.iter().find(|repo| &repo.repo_key == key))
        else {
            self.status = SpawnStatus::Invalid("select a repository before submitting".to_string());
            return None;
        };
        self.generation += 1;
        self.status = SpawnStatus::Requesting;
        Some(SubmitRequest {
            args,
            repo_path: repo.repo_path.clone(),
            generation: self.generation,
        })
    }

    /// Fold a submit outcome back in. Returns whether the visible state
    /// changed, matching the `Dashboard::apply` / `RunDetail::apply`
    /// discipline so a caller notifies only on real change. A superseded
    /// generation — a stale in-flight result landing after the form moved
    /// on — is ignored, the same guard `RunDetail::apply` uses.
    pub fn settle(&mut self, generation: u64, outcome: SubmitOutcome) -> bool {
        if generation != self.generation || !matches!(self.status, SpawnStatus::Requesting) {
            return false;
        }
        self.status = match outcome {
            SubmitOutcome::Started { session_id } => SpawnStatus::Requested { session_id },
            SubmitOutcome::Exited { code, stderr } => {
                SpawnStatus::Rejected(format!("exited ({code:?}): {stderr}"))
            }
            SubmitOutcome::Failed(error) => SpawnStatus::Rejected(error),
        };
        true
    }

    /// Clear a `Requested` status and the submitted text once `session_id`
    /// is confirmed visible. `Shell` calls this once it has established
    /// `session_id` is present in the row list — after either the row's own
    /// `Appeared` delta or this request's `Started` response, whichever of
    /// the two independently-scheduled connections lands second; if the
    /// session never becomes visible the "requested" message simply stays,
    /// which is honest — the center is not lying about having accepted the
    /// request.
    pub fn clear_requested_for(&mut self, session_id: &str) -> bool {
        let matches = matches!(
            &self.status, SpawnStatus::Requested { session_id: pending } if pending == session_id
        );
        if !matches {
            return false;
        }
        self.status = SpawnStatus::Idle;
        self.text.clear();
        self.cursor = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(key: &str) -> SpawnRepo {
        SpawnRepo {
            repo_key: key.to_string(),
            repo_path: format!("/{key}"),
            label: key.to_string(),
        }
    }

    fn seeded(text: &str, repo_key: &str) -> SpawnForm {
        let mut form = SpawnForm::default();
        form.set_repositories(vec![repo(repo_key)]);
        form.select_repository(repo_key.to_string());
        for ch in text.chars() {
            form.insert_char(ch);
        }
        form
    }

    #[test]
    fn submit_rejects_empty_text_without_reaching_the_wire() {
        let mut form = seeded("", "repo-a");
        assert!(form.submit().is_none());
        assert!(matches!(form.status(), SpawnStatus::Invalid(_)));
    }

    #[test]
    fn submit_rejects_a_forbidden_flag_without_reaching_the_wire() {
        let mut form = seeded("fixture\n--json", "repo-a");
        assert!(form.submit().is_none());
        assert!(
            matches!(form.status(), SpawnStatus::Invalid(message) if message.contains("--json"))
        );
    }

    #[test]
    fn submit_rejects_no_repository_selected() {
        let mut form = SpawnForm::default();
        form.insert_char('f');
        assert!(form.submit().is_none());
        assert!(matches!(form.status(), SpawnStatus::Invalid(_)));
    }

    #[test]
    fn submit_while_requesting_returns_none_and_does_not_double_spawn() {
        let mut form = seeded("fixture", "repo-a");
        let first = form.submit().expect("first submit is valid");
        assert!(matches!(form.status(), SpawnStatus::Requesting));
        assert!(
            form.submit().is_none(),
            "a second submit while requesting must not produce a request"
        );
        assert_eq!(first.args, ["fixture"]);
        assert_eq!(first.repo_path, "/repo-a");
    }

    #[test]
    fn settle_ignores_a_superseded_generation() {
        let mut form = seeded("fixture", "repo-a");
        let stale = form.submit().expect("first submit");
        // A later reset issues a fresh generation before the stale result
        // lands.
        form.status = SpawnStatus::Idle;
        let fresh = form.submit().expect("second submit");
        assert_ne!(stale.generation, fresh.generation);

        let changed = form.settle(
            stale.generation,
            SubmitOutcome::Started {
                session_id: "stale-session".to_string(),
            },
        );
        assert!(!changed, "a superseded generation must not settle");
        assert!(matches!(form.status(), SpawnStatus::Requesting));
    }

    #[test]
    fn settle_records_a_requested_session_worded_as_requested_not_running() {
        let mut form = seeded("fixture", "repo-a");
        let request = form.submit().expect("valid submit");
        let changed = form.settle(
            request.generation,
            SubmitOutcome::Started {
                session_id: "session-1".to_string(),
            },
        );
        assert!(changed);
        assert!(matches!(
            form.status(),
            SpawnStatus::Requested { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn settle_records_an_exited_outcome_with_its_code_and_stderr() {
        let mut form = seeded("fixture", "repo-a");
        let request = form.submit().expect("valid submit");
        let changed = form.settle(
            request.generation,
            SubmitOutcome::Exited {
                code: Some(7),
                stderr: "boom".to_string(),
            },
        );
        assert!(changed);
        assert!(
            matches!(form.status(), SpawnStatus::Rejected(reason) if reason.contains('7') && reason.contains("boom"))
        );
    }

    #[test]
    fn settle_records_a_failed_transport_error() {
        let mut form = seeded("fixture", "repo-a");
        let request = form.submit().expect("valid submit");
        let changed = form.settle(
            request.generation,
            SubmitOutcome::Failed("connection refused".to_string()),
        );
        assert!(changed);
        assert!(
            matches!(form.status(), SpawnStatus::Rejected(reason) if reason == "connection refused")
        );
    }

    #[test]
    fn clear_requested_for_matching_session_resets_the_form() {
        let mut form = seeded("fixture", "repo-a");
        let request = form.submit().expect("valid submit");
        form.settle(
            request.generation,
            SubmitOutcome::Started {
                session_id: "session-1".to_string(),
            },
        );
        assert!(!form.clear_requested_for("other-session"));
        assert!(matches!(form.status(), SpawnStatus::Requested { .. }));

        assert!(form.clear_requested_for("session-1"));
        assert!(matches!(form.status(), SpawnStatus::Idle));
        assert_eq!(form.text(), "");
    }

    #[test]
    fn a_repository_disappearing_from_center_state_clears_a_stale_selection() {
        let mut form = SpawnForm::default();
        form.set_repositories(vec![repo("repo-a"), repo("repo-b")]);
        form.select_repository("repo-a".to_string());
        assert_eq!(form.selected_repo(), Some("repo-a"));

        form.set_repositories(vec![repo("repo-b")]);
        assert_eq!(
            form.selected_repo(),
            None,
            "a repository the center no longer knows about is not a valid spawn target"
        );
    }

    #[test]
    fn insert_and_backspace_stay_on_char_boundaries() {
        let mut form = SpawnForm::default();
        for ch in "aé€".chars() {
            form.insert_char(ch);
        }
        assert_eq!(form.text(), "aé€");
        form.backspace();
        assert_eq!(form.text(), "aé");
        form.backspace();
        assert_eq!(form.text(), "a");
    }
}
