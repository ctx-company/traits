//! Merge-time done proposals (0063.8) and their 0064 growth into the full
//! reconcile pass: pure derivation shared by the dashboard TASKS screen and
//! `ctx tasks proposals` / `ctx tasks reconcile`. A proposal is never
//! stored — every consumer recomputes it on each look from the run
//! inventory and the task board's own already-in-hand facts.

use std::collections::BTreeMap;

use ctx_traits_core::procedure::session::{MergeStatus, Session};
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{DuplicateKey, ResolvedTask, TaskSummary};
use ctx_traits_core::task::{AutoClosePolicy, Check, CheckOutcome, CheckRecord};
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

/// One session's facts as the reconcile pass needs them — every field
/// pre-extracted by the caller (a live board read, a ledger inventory scan,
/// an ancestry check) so [`derive_reconcile_report`] stays a pure function
/// over already-in-hand facts, unit-testable with literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFact {
    pub run_id: String,
    pub task_key: Option<String>,
    /// `provenance.task_digest`, stringified (`sha256:<hex>`), if this run
    /// resolved a task at dispatch (0061).
    pub task_digest: Option<String>,
    /// The `landed=<sha>` evidence off this session's last terminal merge
    /// frame, when that frame is `Merged` — [`merged_landed_sha`]'s own
    /// contract, precomputed by the caller.
    pub landed_sha: Option<String>,
    /// Whether `sha` verifies as an ancestor of `main` via `git merge-base
    /// --is-ancestor` — `None` when there is no `landed_sha` to check,
    /// `Some(false)` for "not an ancestor" AND "unresolvable" alike (both
    /// are ambiguity, per [`crate::git_process::is_ancestor`]'s own
    /// contract carried through by the caller).
    pub landed_is_ancestor: Option<bool>,
    /// Whether this session's status is `Blocked` and it carries a typed
    /// park report — counter-evidence against a `MarkDone` proposal for the
    /// same task from an earlier run.
    pub blocked_with_park_report: bool,
    /// The persisted terminal timestamp for this session's last drive
    /// outcome, if any — used to order a later park against an earlier
    /// merge for the same task.
    pub terminal_epoch: Option<u64>,
}

/// A task a dependent derives `Blocked` on, whose edge reconcile proposes
/// removing: the target closed (cancelled, or has a standing `MarkDone`
/// proposal of its own) and the dependent's edge onto it no longer reflects
/// anything the owner still needs to wait for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RemoveDependsOnProposal {
    pub from: String,
    pub to: String,
    pub to_status: DerivedStatus,
    pub evidence: String,
}

/// One reconcile proposal (0064): a task a merged run proposes closing
/// (hardened past [`DoneProposal`] with ancestry, digest and counter-park
/// checks), or a dependent whose edge onto a closed/closing target reconcile
/// proposes releasing. Never applied — the owner accepts each individually
/// through the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum ReconcileProposal {
    MarkDone {
        task_key: String,
        evidence: Vec<MergedRunEvidence>,
    },
    RemoveDependsOn(RemoveDependsOnProposal),
}

impl ReconcileProposal {
    pub fn task_key(&self) -> &str {
        match self {
            ReconcileProposal::MarkDone { task_key, .. } => task_key,
            ReconcileProposal::RemoveDependsOn(proposal) => &proposal.from,
        }
    }
}

/// A task whose evidence was ambiguous — the Watch's "say ambiguous, do not
/// guess" (0064), typed: no proposal is made, and `reason` names exactly
/// which check failed so the owner can verify it themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AmbiguousFinding {
    pub task_key: String,
    pub reason: String,
}

/// The reconcile pass's whole derived output (0064): every proposal it is
/// confident enough to make, and every task it found evidence for but
/// could not clear to a proposal. Never persisted — recomputed on every
/// pass and dropped on exit, exactly like [`DoneProposal`] before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReconcileReport {
    pub proposals: Vec<ReconcileProposal>,
    pub ambiguous: Vec<AmbiguousFinding>,
}

/// The full reconcile derivation (0064): [`derive_proposals`]'s merge-time
/// `MarkDone` candidates, hardened with ancestry, digest, and counter-park
/// checks (any single failure demotes the whole task to [`AmbiguousFinding`]
/// rather than a partial proposal), plus `RemoveDependsOn` proposals for
/// every dependent blocked on a target that is cancelled or itself a
/// standing `MarkDone` candidate. Pure over already-in-hand facts — every
/// I/O answer (`sessions`, `ancestry`, `resolved`) is the caller's to
/// gather.
pub fn derive_reconcile_report(
    sessions: &[SessionFact],
    all_summaries: &[TaskSummary],
    resolved: &BTreeMap<String, ResolvedTask>,
    duplicate_keys: &[DuplicateKey],
) -> ReconcileReport {
    let live_summaries: Vec<TaskSummary> = all_summaries
        .iter()
        .filter(|summary| !summary.archived)
        .cloned()
        .collect();
    let triples: Vec<(Option<&str>, &str, Option<&str>)> = sessions
        .iter()
        .map(|fact| {
            (
                fact.task_key.as_deref(),
                fact.run_id.as_str(),
                fact.landed_sha.as_deref(),
            )
        })
        .collect();
    let candidates = derive_proposals(&triples, &live_summaries, duplicate_keys);

    let mut proposals = Vec::new();
    let mut ambiguous = Vec::new();
    let mut mark_done_keys: Vec<String> = Vec::new();

    for candidate in candidates {
        match harden_mark_done(&candidate, sessions, resolved) {
            Ok(()) => {
                mark_done_keys.push(candidate.task_key.clone());
                proposals.push(ReconcileProposal::MarkDone {
                    task_key: candidate.task_key,
                    evidence: candidate.evidence,
                });
            }
            Err(reason) => ambiguous.push(AmbiguousFinding {
                task_key: candidate.task_key,
                reason,
            }),
        }
    }

    // Digest drift: a task the ledger bound to a run at dispatch, but whose
    // stored document has since changed, is ambiguous even when it never
    // produced a MarkDone candidate at all — the owner should know the text
    // moved out from under a bound run.
    for fact in sessions {
        let Some(task_key) = &fact.task_key else {
            continue;
        };
        if mark_done_keys.contains(task_key) {
            continue;
        }
        let Some(task_digest) = &fact.task_digest else {
            continue;
        };
        let Some(current) = resolved.get(task_key) else {
            continue;
        };
        if task_digest != &current.digest
            && !ambiguous
                .iter()
                .any(|finding| &finding.task_key == task_key)
        {
            ambiguous.push(AmbiguousFinding {
                task_key: task_key.clone(),
                reason: format!(
                    "run {} bound this task at a digest that no longer matches the stored document",
                    fact.run_id
                ),
            });
        }
    }

    let status_by_key: BTreeMap<&str, DerivedStatus> = all_summaries
        .iter()
        .map(|summary| (summary.key.as_str(), summary.derived_status))
        .collect();
    for summary in &live_summaries {
        let Some(resolved_dependent) = resolved.get(&summary.key) else {
            continue;
        };
        for dep_key in &resolved_dependent.document.relations.depends_on {
            let Some(&to_status) = status_by_key.get(dep_key.as_str()) else {
                continue;
            };
            let target_cancelled = to_status == DerivedStatus::Cancelled;
            let target_mark_done = mark_done_keys.iter().any(|key| key == dep_key);
            if !target_cancelled && !target_mark_done {
                continue;
            }
            let evidence = if target_cancelled {
                format!("{dep_key} is cancelled")
            } else {
                format!("{dep_key} has a standing mark-done proposal")
            };
            proposals.push(ReconcileProposal::RemoveDependsOn(
                RemoveDependsOnProposal {
                    from: summary.key.clone(),
                    to: dep_key.clone(),
                    to_status,
                    evidence,
                },
            ));
        }
    }

    ReconcileReport {
        proposals,
        ambiguous,
    }
}

/// The demotion rules a `MarkDone` candidate must clear every one of to
/// stand as a proposal: each cited sha verifies as an ancestor of main, and
/// no later `Blocked` session of the same task carries a park report (park
/// report timing is counter-evidence against done — checked without regard
/// to which run cites which evidence, since any later park at all
/// contradicts "done"). Returns the first failing reason; a candidate
/// failing more than one check reports only the first found.
fn harden_mark_done(
    candidate: &DoneProposal,
    sessions: &[SessionFact],
    resolved: &BTreeMap<String, ResolvedTask>,
) -> Result<(), String> {
    for evidence in &candidate.evidence {
        let fact = sessions.iter().find(|fact| {
            fact.run_id == evidence.run_id
                && fact.task_key.as_deref() == Some(candidate.task_key.as_str())
        });
        let is_ancestor = fact.and_then(|fact| fact.landed_is_ancestor);
        if is_ancestor != Some(true) {
            return Err(format!(
                "run {}'s landed sha {} does not verify as an ancestor of main",
                evidence.run_id, evidence.sha
            ));
        }
    }
    if let Some(current) = resolved.get(&candidate.task_key) {
        for fact in sessions
            .iter()
            .filter(|fact| fact.task_key.as_deref() == Some(candidate.task_key.as_str()))
        {
            if let Some(task_digest) = &fact.task_digest
                && task_digest != &current.digest
            {
                return Err(
                    "a bound run's task digest no longer matches the stored document".to_string(),
                );
            }
        }
    }
    let latest_merge_epoch = candidate
        .evidence
        .iter()
        .filter_map(|evidence| {
            sessions
                .iter()
                .find(|fact| fact.run_id == evidence.run_id)
                .and_then(|fact| fact.terminal_epoch)
        })
        .max();
    let later_park = sessions.iter().any(|fact| {
        fact.task_key.as_deref() == Some(candidate.task_key.as_str())
            && fact.blocked_with_park_report
            && match (fact.terminal_epoch, latest_merge_epoch) {
                (Some(park_epoch), Some(merge_epoch)) => park_epoch > merge_epoch,
                // An unordered park (no timestamp on either side) is still
                // counter-evidence — confidence is the failure mode, so an
                // unresolvable ordering never defaults to "safe".
                _ => true,
            }
    });
    if later_park {
        return Err(format!(
            "{} has a later blocked session with a park report",
            candidate.task_key
        ));
    }
    Ok(())
}

/// A hardened `MarkDone` candidate's declared checks resolve to this: either
/// the write closes itself (0144), or the candidate stays a proposal — the
/// existing confirm flow, unchanged or strengthened. Check execution
/// happens in the caller (the dashboard/CLI reconcile path); this stays
/// pure over already-in-hand results, unit-testable with literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseDisposition {
    /// The write should carry `set_closure` with these check records (empty
    /// under `AutoClosePolicy::Merge` with no declared checks — the
    /// `unchecked` case).
    AutoClose { checks: Vec<CheckRecord> },
    /// Stays a proposal. `reason` is `None` for the plain today's-flow
    /// proposal (no checks declared, or checks have not run yet under
    /// `checked`), `Some` when checks strengthen or block it.
    Proposal { reason: Option<String> },
}

fn first_non_pass(results: &[CheckRecord]) -> Option<&CheckRecord> {
    results.iter().find(|r| r.outcome != CheckOutcome::Passed)
}

fn failure_reason(check: &CheckRecord) -> String {
    format!(
        "check {:?} did not pass ({:?}): {}",
        check.name, check.outcome, check.detail
    )
}

/// The whole 0144 Done-when table for one hardened `MarkDone` candidate.
/// `results` is `None` when the checks have not been executed for this
/// look (e.g. `checked` mode before the caller has run them); `Some` once
/// the caller has a verdict for every declared check.
pub fn close_disposition(
    policy: AutoClosePolicy,
    checks: &[Check],
    results: Option<&[CheckRecord]>,
) -> CloseDisposition {
    match policy {
        AutoClosePolicy::Merge => CloseDisposition::AutoClose {
            checks: results.map(<[CheckRecord]>::to_vec).unwrap_or_default(),
        },
        AutoClosePolicy::Confirm => {
            if checks.is_empty() {
                return CloseDisposition::Proposal { reason: None };
            }
            match results {
                None => CloseDisposition::Proposal { reason: None },
                Some(results) => CloseDisposition::Proposal {
                    reason: Some(match first_non_pass(results) {
                        Some(failing) => failure_reason(failing),
                        None => format!("all {} checks passed", results.len()),
                    }),
                },
            }
        }
        AutoClosePolicy::Checked => {
            if checks.is_empty() {
                return CloseDisposition::Proposal { reason: None };
            }
            match results {
                None => CloseDisposition::Proposal {
                    reason: Some("declared checks have not run yet".to_string()),
                },
                Some(results) => match first_non_pass(results) {
                    Some(failing) => CloseDisposition::Proposal {
                        reason: Some(failure_reason(failing)),
                    },
                    None => CloseDisposition::AutoClose {
                        checks: results.to_vec(),
                    },
                },
            }
        }
    }
}

/// Resolve the effective policy for one task: its own `auto_close`
/// override wins over the `[tasks] auto-close` config leaf in either
/// direction; `None` when neither is set (today's confirm-only flow).
pub fn resolve_auto_close_policy(
    document_override: Option<AutoClosePolicy>,
    config_default: Option<AutoClosePolicy>,
) -> Option<AutoClosePolicy> {
    document_override.or(config_default)
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

    // --- 0064 reconcile derivation -----------------------------------

    fn task_document(key: &str, depends_on: Vec<&str>) -> ctx_traits_core::task::TaskDocument {
        ctx_traits_core::task::TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: key.to_string(),
            title: format!("title {key}"),
            status: Some(ctx_traits_core::task::TaskStatus::Ready),
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: String::new(),
            scope: String::new(),
            validation: String::new(),
            relations: ctx_traits_core::task::Relations {
                depends_on: depends_on.into_iter().map(str::to_string).collect(),
                parent: None,
            },
            steps: Vec::new(),
            checks: Vec::new(),
            auto_close: None,
            closure: None,
        }
    }

    fn resolved_task(
        documents: &BTreeMap<String, ctx_traits_core::task::TaskDocument>,
        key: &str,
        digest: &str,
    ) -> ResolvedTask {
        ctx_traits_core::task::provider::resolve_task(documents, key, false, digest.to_string())
    }

    fn fact(run_id: &str, task_key: &str, landed_sha: Option<&str>) -> SessionFact {
        SessionFact {
            run_id: run_id.to_string(),
            task_key: Some(task_key.to_string()),
            task_digest: None,
            landed_sha: landed_sha.map(str::to_string),
            landed_is_ancestor: landed_sha.map(|_| true),
            blocked_with_park_report: false,
            terminal_epoch: Some(100),
        }
    }

    #[test]
    fn a_landed_sha_that_is_not_an_ancestor_of_main_is_ambiguous_not_a_proposal() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-a"),
        )]
        .into_iter()
        .collect();
        let mut session_fact = fact("run-1", "0100", Some("deadbeef"));
        session_fact.landed_is_ancestor = Some(false);
        let report = derive_reconcile_report(&[session_fact], &summaries, &resolved, &[]);
        assert!(report.proposals.is_empty());
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(report.ambiguous[0].task_key, "0100");
    }

    #[test]
    fn a_later_park_report_demotes_a_mark_done_candidate_to_ambiguous() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-a"),
        )]
        .into_iter()
        .collect();
        let mut merged = fact("run-1", "0100", Some("deadbeef"));
        merged.terminal_epoch = Some(100);
        let mut later_park = fact("run-2", "0100", None);
        later_park.blocked_with_park_report = true;
        later_park.terminal_epoch = Some(200);
        let report = derive_reconcile_report(&[merged, later_park], &summaries, &resolved, &[]);
        assert!(report.proposals.is_empty());
        assert_eq!(report.ambiguous.len(), 1);
    }

    #[test]
    fn an_earlier_park_report_never_blocks_a_later_merge() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-a"),
        )]
        .into_iter()
        .collect();
        let mut earlier_park = fact("run-1", "0100", None);
        earlier_park.blocked_with_park_report = true;
        earlier_park.terminal_epoch = Some(50);
        let mut merged = fact("run-2", "0100", Some("deadbeef"));
        merged.terminal_epoch = Some(200);
        let report = derive_reconcile_report(&[earlier_park, merged], &summaries, &resolved, &[]);
        assert_eq!(report.proposals.len(), 1);
        assert!(report.ambiguous.is_empty());
    }

    #[test]
    fn a_clean_mark_done_candidate_proposes() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-a"),
        )]
        .into_iter()
        .collect();
        let report = derive_reconcile_report(
            &[fact("run-1", "0100", Some("deadbeef"))],
            &summaries,
            &resolved,
            &[],
        );
        assert_eq!(report.proposals.len(), 1);
        assert!(matches!(
            &report.proposals[0],
            ReconcileProposal::MarkDone { task_key, .. } if task_key == "0100"
        ));
        assert!(report.ambiguous.is_empty());
    }

    #[test]
    fn a_cancelled_dependency_yields_a_remove_depends_on_proposal() {
        let documents: BTreeMap<_, _> = [
            task_document("0100", vec![]),
            task_document("0101", vec!["0100"]),
        ]
        .into_iter()
        .map(|d| (d.key.clone(), d))
        .collect();
        let summaries = vec![
            summary("0100", DerivedStatus::Cancelled, true),
            summary("0101", DerivedStatus::Blocked, false),
        ];
        let resolved: BTreeMap<_, _> = [
            (
                "0100".to_string(),
                resolved_task(&documents, "0100", "digest-a"),
            ),
            (
                "0101".to_string(),
                resolved_task(&documents, "0101", "digest-b"),
            ),
        ]
        .into_iter()
        .collect();
        let report = derive_reconcile_report(&[], &summaries, &resolved, &[]);
        assert_eq!(report.proposals.len(), 1);
        match &report.proposals[0] {
            ReconcileProposal::RemoveDependsOn(proposal) => {
                assert_eq!(proposal.from, "0101");
                assert_eq!(proposal.to, "0100");
                assert_eq!(proposal.to_status, DerivedStatus::Cancelled);
            }
            other => panic!("expected RemoveDependsOn, got {other:?}"),
        }
    }

    #[test]
    fn a_done_dependency_yields_no_remove_depends_on_proposal() {
        let documents: BTreeMap<_, _> = [
            task_document("0100", vec![]),
            task_document("0101", vec!["0100"]),
        ]
        .into_iter()
        .map(|d| (d.key.clone(), d))
        .collect();
        let summaries = vec![
            summary("0100", DerivedStatus::Done, true),
            summary("0101", DerivedStatus::Ready, false),
        ];
        let resolved: BTreeMap<_, _> = [
            (
                "0100".to_string(),
                resolved_task(&documents, "0100", "digest-a"),
            ),
            (
                "0101".to_string(),
                resolved_task(&documents, "0101", "digest-b"),
            ),
        ]
        .into_iter()
        .collect();
        let report = derive_reconcile_report(&[], &summaries, &resolved, &[]);
        assert!(report.proposals.is_empty());
    }

    #[test]
    fn a_digest_mismatch_with_no_mark_done_candidate_is_still_ambiguous() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-now"),
        )]
        .into_iter()
        .collect();
        let mut drifted = fact("run-1", "0100", None);
        drifted.task_digest = Some("digest-then".to_string());
        let report = derive_reconcile_report(&[drifted], &summaries, &resolved, &[]);
        assert!(report.proposals.is_empty());
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(report.ambiguous[0].task_key, "0100");
    }

    #[test]
    fn a_digest_mismatch_on_a_mark_done_candidate_demotes_it_to_ambiguous() {
        let documents: BTreeMap<_, _> = [task_document("0100", vec![])]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "digest-now"),
        )]
        .into_iter()
        .collect();
        // A candidate with landed evidence AND a digest that has drifted
        // since the run bound this task must demote — the digest check is
        // not skipped just because ancestry already passed.
        let mut drifted = fact("run-1", "0100", Some("deadbeef"));
        drifted.task_digest = Some("digest-then".to_string());
        let report = derive_reconcile_report(&[drifted], &summaries, &resolved, &[]);
        assert!(report.proposals.is_empty());
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(report.ambiguous[0].task_key, "0100");
    }

    /// 0144 tamper protection: a run whose merged branch edited its own
    /// task's `checks` post-dispatch drifts the digest exactly like any
    /// other post-dispatch edit — `harden_mark_done` demotes the candidate
    /// to ambiguous, so no disposition (auto-close or otherwise) is ever
    /// computed for it. No new mechanism was needed; this pins the
    /// scenario the draft calls out explicitly.
    #[test]
    fn editing_checks_post_dispatch_demotes_the_candidate_never_reaching_auto_close() {
        let mut original = task_document("0100", vec![]);
        original.checks = vec![Check {
            name: "unit tests".to_string(),
            command: "cargo test".to_string(),
            timeout_ms: None,
            expect: None,
        }];
        let dispatch_time_digest = ctx_traits_core::digest::Digest::source(
            &ctx_traits_core::task::serialize(&original).unwrap(),
        )
        .to_string();

        // The run's merged branch replaced the declared checks with a
        // no-op before proposing done — the stored document's digest no
        // longer matches what the run was dispatched against.
        let mut tampered = original.clone();
        tampered.checks = vec![Check {
            name: "always true".to_string(),
            command: "true".to_string(),
            timeout_ms: None,
            expect: None,
        }];
        let documents: BTreeMap<_, _> = [tampered.clone()]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();
        let summaries = vec![summary("0100", DerivedStatus::Ready, false)];
        let resolved = [(
            "0100".to_string(),
            resolved_task(&documents, "0100", "irrelevant-current-digest"),
        )]
        .into_iter()
        .collect();

        let mut run = fact("run-1", "0100", Some("deadbeef"));
        run.task_digest = Some(dispatch_time_digest);
        let report = derive_reconcile_report(&[run], &summaries, &resolved, &[]);

        assert!(
            report.proposals.is_empty(),
            "a candidate whose checks were edited post-dispatch must never reach a disposition"
        );
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(report.ambiguous[0].task_key, "0100");
    }

    // --- close_disposition ------------------------------------------

    fn passed(name: &str) -> CheckRecord {
        CheckRecord {
            name: name.to_string(),
            command: "true".to_string(),
            outcome: CheckOutcome::Passed,
            detail: "exit 0".to_string(),
        }
    }

    fn failed(name: &str) -> CheckRecord {
        CheckRecord {
            name: name.to_string(),
            command: "false".to_string(),
            outcome: CheckOutcome::Failed,
            detail: "exited 1".to_string(),
        }
    }

    fn one_check() -> Vec<Check> {
        vec![Check {
            name: "c".to_string(),
            command: "true".to_string(),
            timeout_ms: None,
            expect: None,
        }]
    }

    #[test]
    fn checked_with_no_declared_checks_is_todays_plain_proposal() {
        let disposition = close_disposition(AutoClosePolicy::Checked, &[], None);
        assert_eq!(disposition, CloseDisposition::Proposal { reason: None });
    }

    #[test]
    fn checked_with_all_passing_checks_auto_closes() {
        let results = vec![passed("c")];
        let disposition = close_disposition(AutoClosePolicy::Checked, &one_check(), Some(&results));
        assert_eq!(
            disposition,
            CloseDisposition::AutoClose {
                checks: results.clone()
            }
        );
    }

    #[test]
    fn checked_with_one_failing_check_stays_a_proposal_naming_it() {
        let results = vec![passed("a"), failed("b")];
        let checks = vec![
            Check {
                name: "a".to_string(),
                command: "true".to_string(),
                timeout_ms: None,
                expect: None,
            },
            Check {
                name: "b".to_string(),
                command: "false".to_string(),
                timeout_ms: None,
                expect: None,
            },
        ];
        let disposition = close_disposition(AutoClosePolicy::Checked, &checks, Some(&results));
        match disposition {
            CloseDisposition::Proposal { reason: Some(r) } => assert!(r.contains("\"b\"")),
            other => panic!("expected a Proposal naming check b, got {other:?}"),
        }
    }

    #[test]
    fn checked_with_declared_checks_not_yet_run_stays_a_proposal() {
        let disposition = close_disposition(AutoClosePolicy::Checked, &one_check(), None);
        assert!(matches!(
            disposition,
            CloseDisposition::Proposal { reason: Some(_) }
        ));
    }

    #[test]
    fn confirm_never_auto_closes_even_with_all_checks_passing() {
        let results = vec![passed("c")];
        let disposition = close_disposition(AutoClosePolicy::Confirm, &one_check(), Some(&results));
        assert_eq!(
            disposition,
            CloseDisposition::Proposal {
                reason: Some("all 1 checks passed".to_string())
            }
        );
    }

    #[test]
    fn confirm_with_no_checks_is_todays_plain_proposal() {
        let disposition = close_disposition(AutoClosePolicy::Confirm, &[], None);
        assert_eq!(disposition, CloseDisposition::Proposal { reason: None });
    }

    #[test]
    fn merge_auto_closes_with_no_declared_checks_recording_unchecked() {
        let disposition = close_disposition(AutoClosePolicy::Merge, &[], None);
        assert_eq!(
            disposition,
            CloseDisposition::AutoClose { checks: Vec::new() }
        );
    }

    #[test]
    fn merge_auto_closes_even_when_a_check_failed() {
        let results = vec![failed("c")];
        let disposition = close_disposition(AutoClosePolicy::Merge, &one_check(), Some(&results));
        assert_eq!(
            disposition,
            CloseDisposition::AutoClose {
                checks: results.clone()
            }
        );
    }

    #[test]
    fn document_override_beats_config_default_in_either_direction() {
        assert_eq!(
            resolve_auto_close_policy(Some(AutoClosePolicy::Merge), Some(AutoClosePolicy::Confirm)),
            Some(AutoClosePolicy::Merge)
        );
        assert_eq!(
            resolve_auto_close_policy(None, Some(AutoClosePolicy::Checked)),
            Some(AutoClosePolicy::Checked)
        );
        assert_eq!(
            resolve_auto_close_policy(Some(AutoClosePolicy::Confirm), None),
            Some(AutoClosePolicy::Confirm)
        );
        assert_eq!(resolve_auto_close_policy(None, None), None);
    }
}
