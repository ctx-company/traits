//! Single translation source for trust wording (P473 §4.3, §4.9), mirroring
//! [`super::merge_story`]'s precedent: turns a trust classification into a
//! plain state sentence, a next action, and (for a run-ledger sighting) an
//! evidence sentence that never overclaims who moved a digest. Pure — no IO,
//! no terminal — consumed by both the CLI human-output panels
//! (`lifecycle_reporting::handle_trust_status`/`handle_trust_list`) and the
//! dashboard TRUST screen's list/detail rendering, so the wording is written
//! exactly once.
//!
//! Classification delegates the `MovedApproval`/`MovedBlock` split to
//! [`ctx_traits_io::trust::TrustReportRow::is_stale_approval`] — P419's
//! specific, already-landed rule that a moved BLOCKED record is never a
//! stale approval — rather than re-deriving it here.

use ctx_traits_io::trust::{TrustFreshness, TrustReportRow, TrustState};

/// The classified trust state of one trait/orphan row. Exhaustive matches
/// throughout this module, so a future class added here cannot silently
/// render blank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustClass {
    Verified,
    Blocked,
    Unreviewed,
    /// An identity-bound VERIFIED record whose trait rebuilt to a different
    /// digest — P419's "stale approval": a human who thinks they already
    /// approved the current bytes has not.
    MovedApproval,
    /// An identity-bound BLOCKED record whose trait rebuilt to a different
    /// digest — the new digest already reads unreviewed (the fail-safe
    /// default), so there is nothing stale to warn about re-approving.
    MovedBlock,
    /// A recorded decision that names no trait currently visible here, by
    /// identity or by exact digest.
    Orphaned,
}

impl TrustClass {
    #[cfg(test)]
    fn all() -> &'static [TrustClass] {
        &[
            TrustClass::Verified,
            TrustClass::Blocked,
            TrustClass::Unreviewed,
            TrustClass::MovedApproval,
            TrustClass::MovedBlock,
            TrustClass::Orphaned,
        ]
    }

    /// A short list-row label — never the bare `{:?}` debug form.
    pub(crate) fn label(self) -> &'static str {
        match self {
            TrustClass::Verified => "verified",
            TrustClass::Blocked => "blocked",
            TrustClass::Unreviewed => "unreviewed",
            TrustClass::MovedApproval => "moved (approval stale)",
            TrustClass::MovedBlock => "moved (block stale)",
            TrustClass::Orphaned => "orphaned",
        }
    }
}

/// Classifies a trait against its (optional) identity-bound-or-legacy trust
/// record, already joined via [`ctx_traits_io::trust::classify_records`] —
/// this function never re-implements that current/stale/orphan join itself.
/// `None` means no record was ever found for the trait (a fresh,
/// never-reviewed trait) — distinct from `Orphaned`, which is a record that
/// names no visible trait.
pub(crate) fn classify_trust(record: Option<&TrustReportRow>) -> TrustClass {
    match record {
        None => TrustClass::Unreviewed,
        Some(row) => match row.freshness {
            TrustFreshness::Orphaned => TrustClass::Orphaned,
            TrustFreshness::Current => match row.state {
                TrustState::Verified => TrustClass::Verified,
                TrustState::Blocked => TrustClass::Blocked,
            },
            TrustFreshness::Stale => {
                if row.is_stale_approval() {
                    TrustClass::MovedApproval
                } else {
                    TrustClass::MovedBlock
                }
            }
        },
    }
}

/// One plain-language sentence describing where a row stands — never a bare
/// class name or a `{:?}` dump.
pub(crate) fn state_sentence(class: TrustClass) -> &'static str {
    match class {
        TrustClass::Verified => "This trait's current bytes are approved.",
        TrustClass::Blocked => "This trait's current bytes are blocked.",
        TrustClass::Unreviewed => "This trait has never been reviewed on this machine.",
        TrustClass::MovedApproval => {
            "You approved this trait, but it has been rebuilt since — the bytes running today are unreviewed."
        }
        TrustClass::MovedBlock => {
            "This trait was blocked, then rebuilt — the new bytes already read as unreviewed, not blocked."
        }
        TrustClass::Orphaned => {
            "This recorded decision names no trait visible here; nothing on this machine resolves to it."
        }
    }
}

/// The surface a [`next_action`] sentence is rendered for — a shared wording
/// module must never print one surface's input model (a TUI keypress) on
/// another surface (a non-interactive CLI command) that has no such key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Surface {
    /// The interactive dashboard's TRUST screen: names keypresses (`a`/`b`/`A`).
    Tui,
    /// `ctx traits trust <id>` and friends: names commands
    /// (`ctx traits trust approve/block <id>`), never a keypress.
    Cli { trait_id: String },
}

/// The exact next action for a row, family-aware for the two classes where a
/// standing re-approval is the point (`MovedApproval`) — `family` is the
/// row's `metadata.family`, when it has one. Parameterized by [`Surface`] so
/// the TUI names its keys and the CLI names its commands from the same
/// class→intent mapping (P473 §4.9 shares a voice, not a keymap).
pub(crate) fn next_action(class: TrustClass, family: Option<&str>, surface: &Surface) -> String {
    match surface {
        Surface::Tui => next_action_tui(class, family),
        Surface::Cli { trait_id } => next_action_cli(class, trait_id),
    }
}

fn next_action_tui(class: TrustClass, family: Option<&str>) -> String {
    match class {
        TrustClass::Verified => "already approved — `b` to block it instead".to_string(),
        TrustClass::Blocked => "already blocked — `a` to approve it instead".to_string(),
        TrustClass::Unreviewed => "approve with `a` or block with `b`".to_string(),
        TrustClass::MovedApproval => match family {
            Some(family) => {
                format!("approve again with `a` (or `A` for the whole {family} family)")
            }
            None => "approve again with `a`".to_string(),
        },
        TrustClass::MovedBlock => {
            "the new bytes already read as unreviewed — `a`/`b` to decide them fresh".to_string()
        }
        TrustClass::Orphaned => {
            "nothing to approve — `ctx traits trust list` reports it as orphaned".to_string()
        }
    }
}

fn next_action_cli(class: TrustClass, trait_id: &str) -> String {
    match class {
        TrustClass::Verified => {
            format!("already approved — `ctx traits trust block {trait_id}` to block it instead")
        }
        TrustClass::Blocked => {
            format!("already blocked — `ctx traits trust approve {trait_id}` to approve it instead")
        }
        TrustClass::Unreviewed | TrustClass::MovedApproval | TrustClass::MovedBlock => format!(
            "re-run `ctx traits trust approve {trait_id}` to approve, or `ctx traits trust block {trait_id}` to block"
        ),
        TrustClass::Orphaned => {
            "nothing to approve — `ctx traits trust list` reports it as orphaned".to_string()
        }
    }
}

/// The fixed three-line explanation of what approving a trait actually
/// means, rendered verbatim in the TRUST detail pane (§4.5) and quoted from
/// PRODUCT's own phrasing on trust scope.
pub(crate) fn approval_meaning() -> [&'static str; 3] {
    [
        "this machine only — approval is never shared or synced anywhere else",
        "keyed to these exact bytes — any rebuild produces a new digest and reverts to unreviewed",
        "trust records that the bytes were reviewed once — it proves nothing about future model behavior",
    ]
}

/// The most recent readable run-ledger session on this machine whose
/// `trait_id` and canonical digest matched the row being explained — pure
/// evidence of "these bytes ran", never "who changed them" (§4.4).
pub(crate) struct RunSighting {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) when: String,
}

/// Renders a [`RunSighting`] as a sentence — deliberately never "moved by":
/// a ledger sighting is evidence bytes ran, not evidence of who changed
/// them.
pub(crate) fn sighting_sentence(sighting: Option<&RunSighting>) -> String {
    match sighting {
        Some(sighting) => format!(
            "these bytes are recorded in run `{}` (ledger `{}`, {})",
            sighting.run_id, sighting.session_id, sighting.when
        ),
        None => "no run ledger on this machine recorded these bytes".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::trust::TrustFreshness;

    fn row(trait_id: Option<&str>, state: TrustState, freshness: TrustFreshness) -> TrustReportRow {
        TrustReportRow {
            trait_id: trait_id.map(str::to_string),
            digest: "sha256:aaa".to_string(),
            current_digest: Some("sha256:bbb".to_string()),
            state,
            freshness,
            updated_at: None,
            reason: None,
            seq: None,
            superseded: false,
        }
    }

    // Every class yields a non-empty sentence and a non-empty next action on
    // both surfaces — a new class added later cannot silently ship
    // untranslated for either.
    #[test]
    fn every_trust_class_has_sentence_and_next_action() {
        let cli = Surface::Cli {
            trait_id: "t1".to_string(),
        };
        for class in TrustClass::all() {
            assert!(!state_sentence(*class).is_empty());
            assert!(!next_action(*class, None, &Surface::Tui).is_empty());
            assert!(!next_action(*class, None, &cli).is_empty());
            assert!(!class.label().is_empty());
        }
    }

    // The CLI surface names a command it can actually run and never a TUI
    // keypress it has no key for; the TUI surface is the mirror image.
    #[test]
    fn next_action_names_the_calling_surfaces_own_affordances() {
        let cli = Surface::Cli {
            trait_id: "my-trait".to_string(),
        };
        for class in TrustClass::all() {
            let cli_text = next_action(*class, None, &cli);
            assert!(
                cli_text.contains("ctx traits trust") || cli_text.contains("nothing to approve"),
                "cli next action for {class:?} named no command: {cli_text}"
            );
            assert!(
                !cli_text.contains("`a`") && !cli_text.contains("`b`") && !cli_text.contains("`A`"),
                "cli next action for {class:?} leaked a TUI keypress: {cli_text}"
            );

            // Orphaned is the one class with nothing surface-specific to say
            // on either surface — both point at the read-only `trust list`
            // report, not at an approve/block command or keypress.
            if *class == TrustClass::Orphaned {
                continue;
            }
            let tui_text = next_action(*class, None, &Surface::Tui);
            assert!(
                !tui_text.contains("ctx traits trust"),
                "tui next action for {class:?} leaked a CLI command: {tui_text}"
            );
        }
    }

    #[test]
    fn classify_trust_distinguishes_moved_approval_from_moved_block() {
        let approval = row(Some("t"), TrustState::Verified, TrustFreshness::Stale);
        assert_eq!(classify_trust(Some(&approval)), TrustClass::MovedApproval);

        let block = row(Some("t"), TrustState::Blocked, TrustFreshness::Stale);
        assert_eq!(classify_trust(Some(&block)), TrustClass::MovedBlock);
    }

    #[test]
    fn classify_trust_current_and_orphaned_and_absent() {
        let current = row(Some("t"), TrustState::Verified, TrustFreshness::Current);
        assert_eq!(classify_trust(Some(&current)), TrustClass::Verified);

        let orphaned = row(None, TrustState::Verified, TrustFreshness::Orphaned);
        assert_eq!(classify_trust(Some(&orphaned)), TrustClass::Orphaned);

        assert_eq!(classify_trust(None), TrustClass::Unreviewed);
    }

    #[test]
    fn moved_approval_next_action_names_the_family_when_present() {
        let text = next_action(TrustClass::MovedApproval, Some("widgets"), &Surface::Tui);
        assert!(text.contains("widgets"));
        assert!(text.contains('A'));
    }

    // The sighting sentence must never make a causal claim about who moved
    // a digest — only that the bytes were recorded as having run.
    #[test]
    fn sighting_sentence_makes_no_causal_claim() {
        let sighting = RunSighting {
            run_id: "run-1".to_string(),
            session_id: "sess-1".to_string(),
            when: "3m ago".to_string(),
        };
        let text = sighting_sentence(Some(&sighting));
        assert!(!text.contains("moved by"));
        assert!(!text.contains("changed by"));
        assert!(text.contains("run-1"));

        let absent = sighting_sentence(None);
        assert!(!absent.contains("moved by"));
        assert!(!absent.contains("changed by"));
    }
}
