//! Merge-time done proposals (0063.8): pure derivation shared by the
//! dashboard TASKS screen and `ctx tasks proposals`. A proposal is never
//! stored — both consumers recompute it on each look from the run
//! inventory and the task board's own already-in-hand facts.

use ctx_traits_core::procedure::session::{MergeStatus, Session};
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{DuplicateKey, TaskSummary};
use serde::Serialize;

/// One merged bound run cited as evidence for a [`DoneProposal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct MergedRunEvidence {
    pub run_id: String,
    pub sha: String,
}

/// A task a merged run proposes closing — the ledger's `task_key`, plus
/// every merged bound run that cites it, in the order [`derive_proposals`]
/// was given them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DoneProposal {
    pub task_key: String,
    pub evidence: Vec<MergedRunEvidence>,
}

/// The `landed=<sha>` evidence off a session's last *terminal* merge frame
/// (`MergeStatus::is_terminal`, `merge.rs`'s own classifier), when that
/// frame's status is `Merged`. `None` when there is no terminal frame yet,
/// the last terminal frame parked or failed instead of landing, or (should
/// never happen, but) a `Merged` frame carries no parseable `landed=`
/// entry — a proposal that cannot cite its sha is not made.
pub fn merged_landed_sha(session: &Session) -> Option<String> {
    let last_terminal = session
        .provenance
        .merge_frames
        .iter()
        .rev()
        .find(|frame| frame.status.is_terminal())?;
    if last_terminal.status != MergeStatus::Merged {
        return None;
    }
    last_terminal
        .evidence
        .iter()
        .find_map(|entry| entry.strip_prefix("landed=").map(str::to_string))
}

/// Derive one proposal per task from `runs` — `(task_key, run_id, merged
/// sha)` triples, one per readable session in the inventory, in caller
/// order (evidence within a proposal preserves that order). Filters: the
/// key must be present, unambiguous (not in `duplicate_keys`), resolve to a
/// task in `summaries`, and that task's `derived_status` must be neither
/// `Done` nor `Cancelled`, and it must not be archived — a merge with a
/// missing or ambiguous binding proposes nothing (0064's Watch: confidence
/// is the failure mode).
pub fn derive_proposals(
    runs: &[(Option<&str>, &str, Option<&str>)],
    summaries: &[TaskSummary],
    duplicate_keys: &[DuplicateKey],
) -> Vec<DoneProposal> {
    let mut proposals: Vec<DoneProposal> = Vec::new();
    for (task_key, run_id, sha) in runs {
        let Some(task_key) = task_key else { continue };
        let Some(sha) = sha else { continue };
        if duplicate_keys
            .iter()
            .any(|duplicate| &duplicate.key == task_key)
        {
            continue;
        }
        let Some(summary) = summaries.iter().find(|summary| &summary.key == task_key) else {
            continue;
        };
        if summary.archived
            || matches!(
                summary.derived_status,
                DerivedStatus::Done | DerivedStatus::Cancelled
            )
        {
            continue;
        }
        let evidence = MergedRunEvidence {
            run_id: (*run_id).to_string(),
            sha: (*sha).to_string(),
        };
        match proposals
            .iter_mut()
            .find(|proposal| &proposal.task_key == task_key)
        {
            Some(proposal) => proposal.evidence.push(evidence),
            None => proposals.push(DoneProposal {
                task_key: (*task_key).to_string(),
                evidence: vec![evidence],
            }),
        }
    }
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::digest::Digest;
    use ctx_traits_core::procedure::runtime::FinalState;
    use ctx_traits_core::procedure::session::{MergeFrame, MergeStage};

    fn session_fixture(run_id: &str) -> Session {
        Session {
            schema_version: "1".to_string(),
            session_id: ctx_traits_core::procedure::session::SessionId::new(format!(
                "session-{run_id}"
            ))
            .expect("session id"),
            run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string()).expect("run id"),
            trait_id: "test-trait".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: ctx_traits_core::procedure::session::Status::Completed,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: None,
            last_validation_report: None,
            completion: None,
            last_drive_outcome: None,
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "test".to_string(),
                    caller: "task-proposals-test".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
            ledger: ctx_traits_core::procedure::runtime::State {
                run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string())
                    .expect("run id"),
                trait_id: "test-trait".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                active_path: Vec::new(),
                control_stack: Vec::new(),
                branch_decisions: Vec::new(),
                conditional_input_decisions: Vec::new(),
                ask_decisions: Vec::new(),
                failure_routes: Vec::new(),
                guard_evaluations: Vec::new(),
                parallel_panel_records: Vec::new(),
                stop_reason: None,
                elapsed_seconds: 0,
                final_state: FinalState::Running,
            },
            state_digest: Digest::source("test"),
        }
    }

    fn frame(status: MergeStatus, evidence: Vec<String>) -> MergeFrame {
        MergeFrame {
            stage: MergeStage::Landing,
            status,
            reason: None,
            evidence,
            park_reason: None,
            deep_decisions: Vec::new(),
        }
    }

    #[test]
    fn terminal_merged_frame_with_landed_evidence_yields_the_sha() {
        let mut session = session_fixture("run-a");
        session.provenance.merge_frames.push(frame(
            MergeStatus::Merged,
            vec!["landed=abc123".to_string()],
        ));
        assert_eq!(merged_landed_sha(&session), Some("abc123".to_string()));
    }

    #[test]
    fn terminal_parked_frame_yields_nothing() {
        let mut session = session_fixture("run-b");
        session
            .provenance
            .merge_frames
            .push(frame(MergeStatus::Parked, Vec::new()));
        assert_eq!(merged_landed_sha(&session), None);
    }

    #[test]
    fn only_nonterminal_frames_yield_nothing() {
        let mut session = session_fixture("run-c");
        session
            .provenance
            .merge_frames
            .push(frame(MergeStatus::GatesPassed, Vec::new()));
        assert_eq!(merged_landed_sha(&session), None);
    }

    #[test]
    fn merged_frame_without_landed_evidence_yields_nothing() {
        let mut session = session_fixture("run-d");
        session.provenance.merge_frames.push(frame(
            MergeStatus::Merged,
            vec!["merger=fast-forward".to_string()],
        ));
        assert_eq!(merged_landed_sha(&session), None);
    }

    #[test]
    fn a_later_nonterminal_frame_never_hides_an_earlier_merged_landing() {
        let mut session = session_fixture("run-e");
        session.provenance.merge_frames.push(frame(
            MergeStatus::Merged,
            vec!["landed=deadbeef".to_string()],
        ));
        // The prior test's own comment on merges_from_inventory notes a
        // terminal frame is always the LAST one; a merge attempt never
        // appends a nonterminal frame after landing, but the lookup itself
        // must still walk from the end rather than take the first frame.
        assert_eq!(merged_landed_sha(&session), Some("deadbeef".to_string()));
    }

    fn summary(key: &str, derived_status: DerivedStatus, archived: bool) -> TaskSummary {
        TaskSummary {
            key: key.to_string(),
            title: format!("task {key}"),
            stored_status: None,
            derived_status,
            archived,
        }
    }

    #[test]
    fn a_bound_merged_run_proposes_its_task_once() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        let proposals = derive_proposals(&runs, &summaries, &[]);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].task_key, "0100");
        assert_eq!(proposals[0].evidence.len(), 1);
        assert_eq!(proposals[0].evidence[0].run_id, "run-1");
        assert_eq!(proposals[0].evidence[0].sha, "abc");
    }

    #[test]
    fn an_unbound_run_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let runs = vec![(None, "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &summaries, &[]).is_empty());
    }

    #[test]
    fn a_run_with_no_landed_sha_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let runs = vec![(Some("0100"), "run-1", None)];
        assert!(derive_proposals(&runs, &summaries, &[]).is_empty());
    }

    #[test]
    fn a_duplicate_key_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let duplicates = vec![DuplicateKey {
            key: "0100".to_string(),
            locations: vec!["a.toml".to_string(), "b.toml".to_string()],
        }];
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &summaries, &duplicates).is_empty());
    }

    #[test]
    fn a_missing_task_proposes_nothing() {
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &[], &[]).is_empty());
    }

    #[test]
    fn an_already_done_task_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Done, true)];
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &summaries, &[]).is_empty());
    }

    #[test]
    fn an_already_cancelled_task_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Cancelled, false)];
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &summaries, &[]).is_empty());
    }

    #[test]
    fn an_archived_task_proposes_nothing() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, true)];
        let runs = vec![(Some("0100"), "run-1", Some("abc"))];
        assert!(derive_proposals(&runs, &summaries, &[]).is_empty());
    }

    #[test]
    fn two_merged_runs_on_one_task_yield_one_proposal_with_two_evidence_entries() {
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let runs = vec![
            (Some("0100"), "run-1", Some("abc")),
            (Some("0100"), "run-2", Some("def")),
        ];
        let proposals = derive_proposals(&runs, &summaries, &[]);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].evidence.len(), 2);
        assert_eq!(proposals[0].evidence[0].run_id, "run-1");
        assert_eq!(proposals[0].evidence[1].run_id, "run-2");
    }
}
