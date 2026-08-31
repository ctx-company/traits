//! gpui-free row-control state machine: tracks in-flight center requests
//! against a row, keyed by `ledger_path` since several rows can be
//! controlled independently. Same discipline `spawn_form.rs` documents at
//! the top: no field, method, or dependency here can reach a `Dashboard`, so
//! the type system — not a convention — forbids optimistic row mutation. A
//! `Requested` entry resolves only through [`RowControls::observe`], driven
//! by the center's own subscription deltas, never from the request
//! response's own acknowledgement.
//!
//! One machine for three verbs — stop, pause, resume — rather than three
//! near-identical modules: eligibility, the double-request guard, and the
//! generation-guarded settle are shared; only the outcome type
//! ([`ControlResult`] vs. [`StartResult`]) and the `observe` target differ,
//! and both of those differences are expressed per-verb below.

use std::collections::HashMap;

use ctx_traits_io::center::{ControlAction, ControlResult, StartResult};

use crate::run_row::{RowState, RunRow};

/// The verb behind a row-control request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowVerb {
    Interrupt,
    Pause,
    Resume,
}

/// Identity fields copied off the center-supplied [`RunRow`] at request
/// time, never derived locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowRequest {
    pub ledger_path: String,
    pub session_id: String,
    pub repo_key: String,
    pub generation: u64,
    pub verb: RowVerb,
}

/// The response an in-flight request settles on. Stop/pause settle on a
/// [`ControlResult`]; resume settles on a [`StartResult`] — start-by-
/// session-id, not a control verb.
#[derive(Debug, Clone, PartialEq)]
pub enum RowOutcome {
    Control(ControlResult),
    Start(StartResult),
    Failed(String),
}

/// The visible status. `Requested`'s wording is *"stop/pause/resume
/// requested — waiting for the center"*, never *"stopped"*/*"paused"*/
/// *"running"*: acknowledgement means the driver accepted the request, not
/// that the effect happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowStatus {
    Requesting,
    Requested { verb: RowVerb, session_id: String },
    Refused(String),
}

struct Entry {
    status: RowStatus,
    generation: u64,
    session_id: String,
    display_id: String,
    verb: RowVerb,
}

#[derive(Default)]
pub struct RowControls {
    entries: HashMap<String, Entry>,
    generation: u64,
}

impl RowControls {
    pub fn status(&self, ledger_path: &str) -> Option<&RowStatus> {
        self.entries.get(ledger_path).map(|entry| &entry.status)
    }

    /// Every entry currently `Refused`, keyed by `ledger_path`, carrying the
    /// `display_id` captured at request time. `RowControls` has no notion of
    /// whether the row is still present anywhere — it is gpui-free and
    /// carries no dashboard reference — so a caller (`Shell::render`) uses
    /// this to find refusals whose row has since left the current row list
    /// (an `Ended` delta during a pending resume, per
    /// [`RowControls::observe`]) and render them without a `RunRow` to hang
    /// them off of.
    pub fn refused_entries(&self) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .filter_map(|(ledger_path, entry)| match &entry.status {
                RowStatus::Refused(message) => Some((
                    ledger_path.clone(),
                    entry.display_id.clone(),
                    message.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Every `ledger_path` currently carrying a `Requested` entry — the
    /// only status [`RowControls::observe`] can clear. Reconciliation walks
    /// this set rather than the face's row list, since a `Requested` entry
    /// for a since-removed row must still resolve.
    pub fn pending_ledger_paths(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, entry)| matches!(entry.status, RowStatus::Requested { .. }))
            .map(|(ledger_path, _)| ledger_path.clone())
            .collect()
    }

    /// Validate and, if eligible, produce the request the caller must
    /// submit through the center. Returns `None` without touching the wire
    /// when `row` is not eligible for `verb` (`RowState::Live` for
    /// `Interrupt`/`Pause`, `row.can_resume()` for `Resume` — the client-
    /// side refusal, `SpawnForm::submit`'s `Invalid` analogue) or when a
    /// request for this `ledger_path` is already `Requesting` or
    /// `Requested` for *any* verb (the double-click / held-key guard,
    /// `SpawnForm`'s `Requesting` guard analogue, extended to cover an
    /// acknowledged-but-unobserved entry: only [`RowControls::observe`] may
    /// clear a `Requested` entry, never a fresh request — this also
    /// prevents pausing a row with a stop already in flight). Bumps a
    /// monotonic generation shared across every row this `RowControls`
    /// tracks.
    pub fn request(&mut self, row: &RunRow, verb: RowVerb) -> Option<RowRequest> {
        let eligible = match verb {
            RowVerb::Interrupt | RowVerb::Pause => row.state == RowState::Live,
            RowVerb::Resume => row.can_resume(),
        };
        if !eligible {
            return None;
        }
        if matches!(
            self.entries.get(&row.ledger_path),
            Some(entry)
                if matches!(entry.status, RowStatus::Requesting | RowStatus::Requested { .. })
        ) {
            return None;
        }
        self.generation += 1;
        let generation = self.generation;
        self.entries.insert(
            row.ledger_path.clone(),
            Entry {
                status: RowStatus::Requesting,
                generation,
                session_id: row.session_id.clone(),
                display_id: row.run_id.clone(),
                verb,
            },
        );
        Some(RowRequest {
            ledger_path: row.ledger_path.clone(),
            session_id: row.session_id.clone(),
            repo_key: row.repo_key.clone(),
            generation,
            verb,
        })
    }

    /// Fold a control response, a start response, or a transport failure
    /// back in. Generation-guarded exactly as `SpawnForm::settle` and
    /// `RunDetail::apply` are — a superseded result is dropped.
    /// `ControlResult::Acknowledged` and `StartResult::Started` move to
    /// `Requested`; every other variant, and `Failed`, moves to `Refused`.
    /// Returns whether the visible state changed.
    pub fn settle(&mut self, ledger_path: &str, generation: u64, outcome: RowOutcome) -> bool {
        let Some(entry) = self.entries.get_mut(ledger_path) else {
            return false;
        };
        if entry.generation != generation || !matches!(entry.status, RowStatus::Requesting) {
            return false;
        }
        entry.status = match outcome {
            RowOutcome::Control(ControlResult::Acknowledged) => RowStatus::Requested {
                verb: entry.verb,
                session_id: entry.session_id.clone(),
            },
            RowOutcome::Control(result) => {
                RowStatus::Refused(result.message(control_action(entry.verb), &entry.display_id))
            }
            RowOutcome::Start(StartResult::Started { session_id }) => RowStatus::Requested {
                verb: entry.verb,
                session_id,
            },
            RowOutcome::Start(StartResult::Exited { code, stderr }) => {
                RowStatus::Refused(format!("exited ({code:?}): {stderr}"))
            }
            RowOutcome::Failed(error) => RowStatus::Refused(error),
        };
        true
    }

    /// The only path that clears a `Requested` entry. For `Interrupt`/
    /// `Pause`, the target is unchanged from the landed interrupt machine:
    /// the row is gone from the model (`live == None`) or is present and no
    /// longer live (`live == Some(false)`) — acknowledgement never clears
    /// it, only an observed delta does. For `Resume` the target is
    /// inverted: it clears on `live == Some(true)` (the resumed run is
    /// visible and live again); on `live == None` (an `Ended` delta removed
    /// the row while the resume was pending) the observation it was waiting
    /// for can never arrive, so the entry moves to `Refused` rather than
    /// silently clearing — a silent clear there would read as success.
    /// Returns whether an entry was cleared or moved to `Refused`.
    pub fn observe(&mut self, ledger_path: &str, live: Option<bool>) -> bool {
        let Some(entry) = self.entries.get(ledger_path) else {
            return false;
        };
        let RowStatus::Requested { verb, .. } = &entry.status else {
            return false;
        };
        match *verb {
            RowVerb::Interrupt | RowVerb::Pause => {
                if live == Some(true) {
                    return false;
                }
                self.entries.remove(ledger_path);
                true
            }
            RowVerb::Resume => {
                if live == Some(true) {
                    self.entries.remove(ledger_path);
                    true
                } else if live.is_none() {
                    let display_id = entry.display_id.clone();
                    if let Some(entry) = self.entries.get_mut(ledger_path) {
                        entry.status =
                            RowStatus::Refused(format!("{display_id} is no longer listed"));
                    }
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// The one production seam that maps a verb to the center call it makes:
/// `Interrupt`/`Pause` go through `control_existing`, `Resume` goes through
/// `start_session_existing` (start-by-session-id, never a control verb).
/// gpui-free and blocking — callers (the desktop's `cx.background_spawn`
/// closure, and the fake-peer integration tests) both run it off the UI
/// thread. Kept here, not duplicated in `shell.rs` or a test, so a change to
/// the verb-to-call mapping cannot silently diverge between production and
/// its own proof.
pub fn dispatch(request: &RowRequest) -> RowOutcome {
    match request.verb {
        RowVerb::Interrupt | RowVerb::Pause => {
            match ctx_traits_io::center::control_existing(
                &request.session_id,
                Some(&request.repo_key),
                control_action(request.verb),
            ) {
                Ok(result) => RowOutcome::Control(result),
                Err(error) => RowOutcome::Failed(error.to_string()),
            }
        }
        RowVerb::Resume => match ctx_traits_io::center::start_session_existing(
            &request.session_id,
            Some(&request.repo_key),
        ) {
            Ok(result) => RowOutcome::Start(result),
            Err(error) => RowOutcome::Failed(error.to_string()),
        },
    }
}

fn control_action(verb: RowVerb) -> ControlAction {
    match verb {
        RowVerb::Interrupt => ControlAction::Interrupt,
        RowVerb::Pause => ControlAction::Pause,
        // Resume never settles on a `ControlResult` — it settles on a
        // `StartResult` (start-by-session-id) — so this arm is unreachable
        // in practice; the fallback keeps `control_action` total rather
        // than partial.
        RowVerb::Resume => ControlAction::Interrupt,
    }
}

/// The wording lives here (gpui-free, unit-testable), not in the render
/// path, mirroring `spawn_view::status_text`.
pub fn status_text(status: &RowStatus) -> String {
    match status {
        RowStatus::Requesting => "requesting…".to_string(),
        RowStatus::Requested { verb, session_id } => {
            let verb_word = match verb {
                RowVerb::Interrupt => "stop",
                RowVerb::Pause => "pause",
                RowVerb::Resume => "resume",
            };
            format!("{verb_word} requested ({session_id}) — waiting for the center")
        }
        RowStatus::Refused(reason) => format!("refused: {reason}"),
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
            session_title: None,
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 0,
            started_at_epoch: None,
        }
    }

    fn terminal_row(ledger_path: &str, session_id: &str) -> RunRow {
        let mut row = live_row(ledger_path, session_id);
        row.state = RowState::Terminal(SessionState::Completed);
        row.live = false;
        row
    }

    fn resumable_row(ledger_path: &str, session_id: &str) -> RunRow {
        let mut row = live_row(ledger_path, session_id);
        row.state = RowState::Resumable(SessionState::WaitingOnHuman);
        row.live = false;
        row
    }

    fn paused_row(ledger_path: &str, session_id: &str) -> RunRow {
        let mut row = live_row(ledger_path, session_id);
        row.state = RowState::Paused;
        row.live = false;
        row
    }

    #[test]
    fn request_refused_for_a_non_live_row_without_reaching_the_wire() {
        let mut controls = RowControls::default();
        let row = terminal_row("/a.json", "session-a");
        assert!(controls.request(&row, RowVerb::Interrupt).is_none());
        assert!(controls.request(&row, RowVerb::Pause).is_none());
        assert!(controls.status("/a.json").is_none());
    }

    #[test]
    fn a_second_request_while_requesting_is_refused_for_any_verb() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let first = controls
            .request(&row, RowVerb::Interrupt)
            .expect("first request is valid");
        assert!(
            controls.request(&row, RowVerb::Pause).is_none(),
            "a pause must not be issued while a stop is in flight for the same row"
        );
        assert_eq!(first.session_id, "session-a");
        assert_eq!(first.repo_key, "repo-a");
    }

    #[test]
    fn settle_ignores_a_superseded_generation() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let stale = controls
            .request(&row, RowVerb::Interrupt)
            .expect("first request");

        // Simulate the entry being cleared and re-requested before the
        // stale result lands, producing a fresh generation.
        controls.entries.remove("/a.json");
        let fresh = controls
            .request(&row, RowVerb::Interrupt)
            .expect("second request");
        assert_ne!(stale.generation, fresh.generation);

        let changed = controls.settle(
            "/a.json",
            stale.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        assert!(!changed, "a superseded generation must not settle");
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requesting)
        ));
    }

    #[test]
    fn acknowledged_moves_to_requested_worded_as_requested_not_stopped() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Interrupt)
            .expect("valid request");
        let changed = controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        assert!(changed);
        let status = controls.status("/a.json").expect("entry present");
        assert!(matches!(
            status,
            RowStatus::Requested { session_id, verb: RowVerb::Interrupt } if session_id == "session-a"
        ));
        let text = status_text(status);
        assert!(text.contains("requested"));
        assert!(!text.contains("stopped"));
    }

    #[test]
    fn pause_acknowledged_renders_pause_wording_distinct_from_stop() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Pause)
            .expect("valid request");
        controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        let status = controls.status("/a.json").expect("entry present");
        let text = status_text(status);
        assert!(text.contains("pause requested"));
        assert!(!text.contains("stop requested"));
        assert!(
            !text.contains("paused"),
            "acknowledgement is not an outcome"
        );
    }

    #[test]
    fn every_non_acknowledged_variant_and_failed_moves_to_a_distinct_refused_message() {
        let variants = [
            RowOutcome::Control(ControlResult::Missing),
            RowOutcome::Control(ControlResult::Ambiguous(vec!["one".to_string()])),
            RowOutcome::Control(ControlResult::NotLive),
            RowOutcome::Control(ControlResult::Unverifiable),
            RowOutcome::Control(ControlResult::Refused),
            RowOutcome::Failed("connection refused".to_string()),
        ];
        let mut messages = std::collections::HashSet::new();
        for (index, outcome) in variants.into_iter().enumerate() {
            let mut controls = RowControls::default();
            let ledger_path = format!("/row-{index}.json");
            let row = live_row(&ledger_path, "session-a");
            let request = controls
                .request(&row, RowVerb::Interrupt)
                .expect("valid request");
            assert!(controls.settle(&ledger_path, request.generation, outcome));
            let status = controls.status(&ledger_path).expect("entry present");
            let RowStatus::Refused(message) = status else {
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
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Interrupt)
            .expect("valid request");
        controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requested { .. })
        ));

        assert!(
            !controls.observe("/a.json", Some(true)),
            "a still-live row must not clear a pending Requested entry"
        );
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requested { .. })
        ));

        assert!(
            controls.observe("/a.json", Some(false)),
            "a no-longer-live row must clear the pending entry"
        );
        assert!(controls.status("/a.json").is_none());
    }

    #[test]
    fn observe_clears_on_row_gone() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Interrupt)
            .expect("valid request");
        controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        assert!(controls.observe("/a.json", None));
        assert!(controls.status("/a.json").is_none());
    }

    #[test]
    fn requested_entry_rejects_repeated_request_until_observed() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Interrupt)
            .expect("first request is valid");
        controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Control(ControlResult::Acknowledged),
        );
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requested { .. })
        ));

        assert!(
            controls.request(&row, RowVerb::Interrupt).is_none(),
            "a request against an acknowledged-but-unobserved entry must not \
             produce a fresh request"
        );
        assert!(
            matches!(
                controls.status("/a.json"),
                Some(RowStatus::Requested { .. })
            ),
            "the Requested entry must survive the rejected re-request"
        );

        assert!(controls.observe("/a.json", Some(false)));
        assert!(controls.status("/a.json").is_none());
    }

    #[test]
    fn observe_does_not_clear_a_merely_requesting_entry() {
        let mut controls = RowControls::default();
        let row = live_row("/a.json", "session-a");
        controls
            .request(&row, RowVerb::Interrupt)
            .expect("valid request");
        assert!(!controls.observe("/a.json", Some(false)));
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requesting)
        ));
    }

    #[test]
    fn resume_eligible_for_resumable_and_paused_ineligible_for_live_and_terminal() {
        let mut controls = RowControls::default();
        assert!(
            controls
                .request(&resumable_row("/a.json", "session-a"), RowVerb::Resume)
                .is_some()
        );
        controls = RowControls::default();
        assert!(
            controls
                .request(&paused_row("/b.json", "session-b"), RowVerb::Resume)
                .is_some()
        );
        controls = RowControls::default();
        assert!(
            controls
                .request(&live_row("/c.json", "session-c"), RowVerb::Resume)
                .is_none()
        );
        controls = RowControls::default();
        assert!(
            controls
                .request(&terminal_row("/d.json", "session-d"), RowVerb::Resume)
                .is_none()
        );
    }

    #[test]
    fn resume_started_response_moves_to_requested_and_observe_clears_only_when_live() {
        let mut controls = RowControls::default();
        let row = paused_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Resume)
            .expect("valid resume request");
        let changed = controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Start(StartResult::Started {
                session_id: "session-a".to_string(),
            }),
        );
        assert!(changed);
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Requested {
                verb: RowVerb::Resume,
                ..
            })
        ));

        assert!(
            !controls.observe("/a.json", Some(false)),
            "a resume must not resolve while the row is still non-live"
        );
        assert!(
            controls.observe("/a.json", Some(true)),
            "a resume resolves once the row is observed live again"
        );
        assert!(controls.status("/a.json").is_none());
    }

    #[test]
    fn resume_pending_and_row_gone_moves_to_refused_rather_than_silently_clearing() {
        let mut controls = RowControls::default();
        let row = paused_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Resume)
            .expect("valid resume request");
        controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Start(StartResult::Started {
                session_id: "session-a".to_string(),
            }),
        );

        let changed = controls.observe("/a.json", None);
        assert!(changed, "row-gone must still change visible state");
        assert!(matches!(
            controls.status("/a.json"),
            Some(RowStatus::Refused(_))
        ));
    }

    #[test]
    fn resume_exited_response_moves_to_a_distinct_refused_message() {
        let mut controls = RowControls::default();
        let row = paused_row("/a.json", "session-a");
        let request = controls
            .request(&row, RowVerb::Resume)
            .expect("valid resume request");
        let changed = controls.settle(
            "/a.json",
            request.generation,
            RowOutcome::Start(StartResult::Exited {
                code: Some(1),
                stderr: "boom".to_string(),
            }),
        );
        assert!(changed);
        let status = controls.status("/a.json").expect("entry present");
        let RowStatus::Refused(message) = status else {
            panic!("expected Refused, got {status:?}");
        };
        assert!(message.contains("boom"));
    }
}
