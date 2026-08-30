//! gpui-free interrupt state machine: tracks in-flight center control
//! requests, keyed by `ledger_path` since several rows can be interrupted
//! independently. Same discipline `spawn_form.rs` documents at the top: no
//! field, method, or dependency here can reach a `Dashboard`, so the type
//! system — not a convention — forbids optimistic row mutation. A
//! `Requested` entry resolves only through [`Interrupts::observe`], driven
//! by the center's own subscription deltas, never from the control
//! response's own acknowledgement.

use std::collections::HashMap;

use ctx_traits_io::center::ControlResult;

use crate::run_row::{RowState, RunRow};

/// Identity fields copied off the center-supplied [`RunRow`] at request
/// time, never derived locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptRequest {
    pub ledger_path: String,
    pub session_id: String,
    pub repo_key: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterruptOutcome {
    Result(ControlResult),
    Failed(String),
}

/// The visible status. `Requested`'s wording is *"stop requested — waiting
/// for the center"*, never *"stopped"*: acknowledgement means the driver
/// accepted the request, not that the run stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptStatus {
    Requesting,
    Requested { session_id: String },
    Refused(String),
}

struct Entry {
    status: InterruptStatus,
    generation: u64,
    session_id: String,
    display_id: String,
}

#[derive(Default)]
pub struct Interrupts {
    entries: HashMap<String, Entry>,
    generation: u64,
}

impl Interrupts {
    pub fn status(&self, ledger_path: &str) -> Option<&InterruptStatus> {
        self.entries.get(ledger_path).map(|entry| &entry.status)
    }

    /// Every `ledger_path` currently carrying a `Requested` entry — the
    /// only status [`Interrupts::observe`] can clear. Reconciliation walks
    /// this set rather than the face's row list, since a `Requested` entry
    /// for a since-removed row must still resolve.
    pub fn pending_ledger_paths(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, entry)| matches!(entry.status, InterruptStatus::Requested { .. }))
            .map(|(ledger_path, _)| ledger_path.clone())
            .collect()
    }

    /// Validate and, if eligible, produce the request the caller must
    /// submit through the center. Returns `None` without touching the wire
    /// when `row` is not `RowState::Live` (the client-side refusal,
    /// `SpawnForm::submit`'s `Invalid` analogue) or when a request for this
    /// `ledger_path` is already `Requesting` or `Requested` (the double-
    /// click / held-key guard, `SpawnForm`'s `Requesting` guard analogue,
    /// extended to cover an acknowledged-but-unobserved entry: only
    /// [`Interrupts::observe`] may clear a `Requested` entry, never a fresh
    /// request). Bumps a monotonic generation shared across every row this
    /// `Interrupts` tracks.
    pub fn request(&mut self, row: &RunRow) -> Option<InterruptRequest> {
        if row.state != RowState::Live {
            return None;
        }
        if matches!(
            self.entries.get(&row.ledger_path),
            Some(entry)
                if matches!(
                    entry.status,
                    InterruptStatus::Requesting | InterruptStatus::Requested { .. }
                )
        ) {
            return None;
        }
        self.generation += 1;
        let generation = self.generation;
        self.entries.insert(
            row.ledger_path.clone(),
            Entry {
                status: InterruptStatus::Requesting,
                generation,
                session_id: row.session_id.clone(),
                display_id: row.run_id.clone(),
            },
        );
        Some(InterruptRequest {
            ledger_path: row.ledger_path.clone(),
            session_id: row.session_id.clone(),
            repo_key: row.repo_key.clone(),
            generation,
        })
    }

    /// Fold a control response or transport failure back in. Generation-
    /// guarded exactly as `SpawnForm::settle` and `RunDetail::apply` are —
    /// a superseded result is dropped. `ControlResult::Acknowledged` moves
    /// to `Requested`; every other variant, and `Failed`, moves to
    /// `Refused`. Returns whether the visible state changed.
    pub fn settle(
        &mut self,
        ledger_path: &str,
        generation: u64,
        outcome: InterruptOutcome,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(ledger_path) else {
            return false;
        };
        if entry.generation != generation || !matches!(entry.status, InterruptStatus::Requesting) {
            return false;
        }
        entry.status = match outcome {
            InterruptOutcome::Result(ControlResult::Acknowledged) => InterruptStatus::Requested {
                session_id: entry.session_id.clone(),
            },
            InterruptOutcome::Result(result) => {
                InterruptStatus::Refused(result.message(&entry.display_id))
            }
            InterruptOutcome::Failed(error) => InterruptStatus::Refused(error),
        };
        true
    }

    /// The only path that clears a `Requested` entry: the row is gone from
    /// the model (`live == None`, i.e. an `Ended` delta removed it) or is
    /// present and no longer live (`live == Some(false)`). Acknowledgement
    /// never clears it — only an observed delta does. Returns whether an
    /// entry was cleared.
    pub fn observe(&mut self, ledger_path: &str, live: Option<bool>) -> bool {
        let Some(entry) = self.entries.get(ledger_path) else {
            return false;
        };
        if !matches!(entry.status, InterruptStatus::Requested { .. }) {
            return false;
        }
        if live == Some(true) {
            return false;
        }
        self.entries.remove(ledger_path);
        true
    }
}

/// The wording lives here (gpui-free, unit-testable), not in the render
/// path, mirroring `spawn_view::status_text`.
pub fn status_text(status: &InterruptStatus) -> String {
    match status {
        InterruptStatus::Requesting => "requesting…".to_string(),
        InterruptStatus::Requested { session_id } => {
            format!("stop requested ({session_id}) — waiting for the center")
        }
        InterruptStatus::Refused(reason) => format!("refused: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::SessionState;

    fn live_row(ledger_path: &str, session_id: &str) -> RunRow {
        RunRow {
            ledger_path: ledger_path.to_string(),
            session_id: session_id.to_string(),
            run_id: session_id.to_string(),
            repo_key: "repo-a".to_string(),
            repo_path: "/repo-a".to_string(),
            repo_label: "repo-a".to_string(),
            title: "title".to_string(),
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
        }
    }

    fn terminal_row(ledger_path: &str, session_id: &str) -> RunRow {
        let mut row = live_row(ledger_path, session_id);
        row.state = RowState::Terminal(SessionState::Completed);
        row.live = false;
        row
    }

    #[test]
    fn request_refused_for_a_non_live_row_without_reaching_the_wire() {
        let mut interrupts = Interrupts::default();
        let row = terminal_row("/a.json", "session-a");
        assert!(interrupts.request(&row).is_none());
        assert!(interrupts.status("/a.json").is_none());
    }

    #[test]
    fn a_second_request_while_requesting_is_refused() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let first = interrupts.request(&row).expect("first request is valid");
        assert!(
            interrupts.request(&row).is_none(),
            "a second request while requesting must not produce a request"
        );
        assert_eq!(first.session_id, "session-a");
        assert_eq!(first.repo_key, "repo-a");
    }

    #[test]
    fn settle_ignores_a_superseded_generation() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let stale = interrupts.request(&row).expect("first request");

        // Simulate the entry being cleared and re-requested before the
        // stale result lands, producing a fresh generation.
        interrupts.entries.remove("/a.json");
        let fresh = interrupts.request(&row).expect("second request");
        assert_ne!(stale.generation, fresh.generation);

        let changed = interrupts.settle(
            "/a.json",
            stale.generation,
            InterruptOutcome::Result(ControlResult::Acknowledged),
        );
        assert!(!changed, "a superseded generation must not settle");
        assert!(matches!(
            interrupts.status("/a.json"),
            Some(InterruptStatus::Requesting)
        ));
    }

    #[test]
    fn acknowledged_moves_to_requested_worded_as_requested_not_stopped() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let request = interrupts.request(&row).expect("valid request");
        let changed = interrupts.settle(
            "/a.json",
            request.generation,
            InterruptOutcome::Result(ControlResult::Acknowledged),
        );
        assert!(changed);
        let status = interrupts.status("/a.json").expect("entry present");
        assert!(matches!(
            status,
            InterruptStatus::Requested { session_id } if session_id == "session-a"
        ));
        let text = status_text(status);
        assert!(text.contains("requested"));
        assert!(!text.contains("stopped"));
    }

    #[test]
    fn every_non_acknowledged_variant_and_failed_moves_to_a_distinct_refused_message() {
        let variants = [
            InterruptOutcome::Result(ControlResult::Missing),
            InterruptOutcome::Result(ControlResult::Ambiguous(vec!["one".to_string()])),
            InterruptOutcome::Result(ControlResult::NotLive),
            InterruptOutcome::Result(ControlResult::Unverifiable),
            InterruptOutcome::Result(ControlResult::Refused),
            InterruptOutcome::Failed("connection refused".to_string()),
        ];
        let mut messages = std::collections::HashSet::new();
        for (index, outcome) in variants.into_iter().enumerate() {
            let mut interrupts = Interrupts::default();
            let ledger_path = format!("/row-{index}.json");
            let row = live_row(&ledger_path, "session-a");
            let request = interrupts.request(&row).expect("valid request");
            assert!(interrupts.settle(&ledger_path, request.generation, outcome));
            let status = interrupts.status(&ledger_path).expect("entry present");
            let InterruptStatus::Refused(message) = status else {
                panic!("expected Refused, got {status:?}");
            };
            assert!(!message.is_empty());
            messages.insert(message.clone());
        }
        assert_eq!(
            messages.len(),
            6,
            "each variant must render a distinct message"
        );
    }

    #[test]
    fn observe_clears_only_on_row_gone_or_no_longer_live_never_on_acknowledgement() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let request = interrupts.request(&row).expect("valid request");
        interrupts.settle(
            "/a.json",
            request.generation,
            InterruptOutcome::Result(ControlResult::Acknowledged),
        );
        assert!(matches!(
            interrupts.status("/a.json"),
            Some(InterruptStatus::Requested { .. })
        ));

        assert!(
            !interrupts.observe("/a.json", Some(true)),
            "a still-live row must not clear a pending Requested entry"
        );
        assert!(matches!(
            interrupts.status("/a.json"),
            Some(InterruptStatus::Requested { .. })
        ));

        assert!(
            interrupts.observe("/a.json", Some(false)),
            "a no-longer-live row must clear the pending entry"
        );
        assert!(interrupts.status("/a.json").is_none());
    }

    #[test]
    fn observe_clears_on_row_gone() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let request = interrupts.request(&row).expect("valid request");
        interrupts.settle(
            "/a.json",
            request.generation,
            InterruptOutcome::Result(ControlResult::Acknowledged),
        );
        assert!(interrupts.observe("/a.json", None));
        assert!(interrupts.status("/a.json").is_none());
    }

    #[test]
    fn requested_entry_rejects_repeated_request_until_observed() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        let request = interrupts.request(&row).expect("first request is valid");
        interrupts.settle(
            "/a.json",
            request.generation,
            InterruptOutcome::Result(ControlResult::Acknowledged),
        );
        assert!(matches!(
            interrupts.status("/a.json"),
            Some(InterruptStatus::Requested { .. })
        ));

        assert!(
            interrupts.request(&row).is_none(),
            "a request against an acknowledged-but-unobserved entry must not \
             produce a fresh request"
        );
        assert!(
            matches!(
                interrupts.status("/a.json"),
                Some(InterruptStatus::Requested { .. })
            ),
            "the Requested entry must survive the rejected re-request"
        );

        assert!(interrupts.observe("/a.json", Some(false)));
        assert!(interrupts.status("/a.json").is_none());
    }

    #[test]
    fn observe_does_not_clear_a_merely_requesting_entry() {
        let mut interrupts = Interrupts::default();
        let row = live_row("/a.json", "session-a");
        interrupts.request(&row).expect("valid request");
        assert!(!interrupts.observe("/a.json", Some(false)));
        assert!(matches!(
            interrupts.status("/a.json"),
            Some(InterruptStatus::Requesting)
        ));
    }
}
