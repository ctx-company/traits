//! Background link to a machine-local run center: connects to an already-
//! serving center (never launches one — see `center::subscribe_existing`),
//! assembles one coherent initial snapshot, and forwards it — followed by
//! every later delta, in subscription order — to the gpui UI thread over a
//! single async channel. The snapshot-plus-delta stream is the exclusive
//! source of dashboard change; nothing here scans a run directory or polls
//! the center.

use ctx_traits_io::center::{self, CenterDelta, CenterEvent, CenterPublicRow};

/// One update the UI thread may act on, in the exact order the center
/// produced it.
pub enum LinkUpdate {
    Snapshot(Vec<CenterPublicRow>),
    Delta(CenterDelta),
    Unavailable(String),
}

/// Stages snapshot rows between `SnapshotStart` and `SnapshotEnd`, mirroring
/// the TUI dashboard worker's staging rule: never publish a partial set. A
/// delta that races a snapshot in flight is buffered rather than dropped or
/// reordered, and is replayed immediately after the snapshot it raced.
#[derive(Default)]
pub struct SnapshotAssembler {
    staged: Vec<CenterPublicRow>,
    buffered_deltas: Vec<CenterDelta>,
    receiving: bool,
}

impl SnapshotAssembler {
    /// Returns the updates this event yields, in arrival order. Normally
    /// empty or a single element; a `SnapshotEnd` that closes a boundary with
    /// races buffered behind it returns the snapshot followed by each raced
    /// delta, oldest first.
    pub fn accept(&mut self, event: CenterEvent) -> Vec<LinkUpdate> {
        match event {
            CenterEvent::SnapshotStart => {
                self.staged.clear();
                self.buffered_deltas.clear();
                self.receiving = true;
                Vec::new()
            }
            CenterEvent::SnapshotRow(row) => {
                if self.receiving {
                    self.staged.push(*row);
                }
                Vec::new()
            }
            CenterEvent::SnapshotEnd => {
                if !self.receiving {
                    return Vec::new();
                }
                self.receiving = false;
                let rows = std::mem::take(&mut self.staged);
                let deltas = std::mem::take(&mut self.buffered_deltas);
                let mut updates = Vec::with_capacity(1 + deltas.len());
                updates.push(LinkUpdate::Snapshot(rows));
                updates.extend(deltas.into_iter().map(LinkUpdate::Delta));
                updates
            }
            CenterEvent::Delta(delta) => {
                if self.receiving {
                    self.buffered_deltas.push(delta);
                    Vec::new()
                } else {
                    vec![LinkUpdate::Delta(delta)]
                }
            }
        }
    }
}

/// Start the background link thread and return the channel its updates
/// arrive on. Connecting and forwarding both happen off the caller's thread.
/// The blocking `recv()` loop stays on this dedicated thread — never on
/// gpui's UI thread — and feeds one unbounded channel, which is what
/// preserves subscription order end to end.
pub fn start(repo_key: Option<String>) -> async_channel::Receiver<LinkUpdate> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let subscription = match center::subscribe_existing(repo_key.as_deref()) {
            Ok(subscription) => subscription,
            Err(error) => {
                let _ = tx.send_blocking(LinkUpdate::Unavailable(error.to_string()));
                return;
            }
        };
        let mut assembler = SnapshotAssembler::default();
        while let Ok(event) = subscription.recv() {
            for update in assembler.accept(event) {
                if tx.send_blocking(update).is_err() {
                    return;
                }
            }
        }
        // `recv()` returning Err means the center closed; deciding what the
        // UI does about that is 0256.5's.
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::run_summary::RunSummary;

    fn row(run_id: &str) -> Box<CenterPublicRow> {
        Box::new(CenterPublicRow {
            summary: RunSummary {
                run_id: run_id.to_string(),
                ..RunSummary::unreadable(run_id.to_string(), "fixture".to_string())
            },
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            ledger_path: "/repo/session.json".to_string(),
            live: true,
            modified_epoch_secs: 0,
        })
    }

    fn ended_delta(run_id: &str) -> CenterDelta {
        CenterDelta::Ended { row: row(run_id) }
    }

    fn snapshot_run_ids(updates: &[LinkUpdate]) -> Vec<&str> {
        match updates.first() {
            Some(LinkUpdate::Snapshot(rows)) => {
                rows.iter().map(|r| r.summary.run_id.as_str()).collect()
            }
            _ => panic!(
                "expected a leading snapshot update, got {} updates",
                updates.len()
            ),
        }
    }

    fn delta_run_ids(updates: &[LinkUpdate]) -> Vec<&str> {
        updates
            .iter()
            .filter_map(|update| match update {
                LinkUpdate::Delta(delta) => Some(delta.row().summary.run_id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn complete_snapshot_yields_exactly_its_rows() {
        let mut assembler = SnapshotAssembler::default();
        assert!(assembler.accept(CenterEvent::SnapshotStart).is_empty());
        assert!(
            assembler
                .accept(CenterEvent::SnapshotRow(row("a")))
                .is_empty()
        );
        assert!(
            assembler
                .accept(CenterEvent::SnapshotRow(row("b")))
                .is_empty()
        );
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert_eq!(snapshot_run_ids(&updates), vec!["a", "b"]);
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn rows_before_snapshot_start_are_ignored() {
        let mut assembler = SnapshotAssembler::default();
        assert!(
            assembler
                .accept(CenterEvent::SnapshotRow(row("stray")))
                .is_empty()
        );
        assert!(assembler.accept(CenterEvent::SnapshotStart).is_empty());
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert!(snapshot_run_ids(&updates).is_empty());
    }

    #[test]
    fn a_second_snapshot_start_discards_the_partial_set() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("discarded")));
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("kept")));
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert_eq!(snapshot_run_ids(&updates), vec!["kept"]);
    }

    #[test]
    fn a_second_snapshot_start_discards_buffered_deltas_too() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("a")));
        assert!(
            assembler
                .accept(CenterEvent::Delta(ended_delta("stale")))
                .is_empty()
        );
        // The stream aborts and restarts before this delta was ever emitted.
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("b")));
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert_eq!(snapshot_run_ids(&updates), vec!["b"]);
        assert!(delta_run_ids(&updates).is_empty());
    }

    #[test]
    fn a_delta_mid_snapshot_is_emitted_after_the_snapshot_in_arrival_order() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("a")));
        assert!(
            assembler
                .accept(CenterEvent::Delta(ended_delta("first")))
                .is_empty()
        );
        assert!(
            assembler
                .accept(CenterEvent::Delta(ended_delta("second")))
                .is_empty()
        );
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert_eq!(snapshot_run_ids(&updates), vec!["a"]);
        assert_eq!(delta_run_ids(&updates), vec!["first", "second"]);
        // The snapshot update precedes both deltas.
        assert!(matches!(updates[0], LinkUpdate::Snapshot(_)));
        assert!(matches!(updates[1], LinkUpdate::Delta(_)));
        assert!(matches!(updates[2], LinkUpdate::Delta(_)));
    }

    #[test]
    fn a_post_snapshot_delta_is_emitted_immediately_alone() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        let updates = assembler.accept(CenterEvent::SnapshotEnd);
        assert_eq!(updates.len(), 1);

        let updates = assembler.accept(CenterEvent::Delta(ended_delta("later")));
        assert_eq!(updates.len(), 1);
        assert_eq!(delta_run_ids(&updates), vec!["later"]);
    }

    #[test]
    fn snapshot_end_without_a_start_emits_nothing() {
        let mut assembler = SnapshotAssembler::default();
        assert!(assembler.accept(CenterEvent::SnapshotEnd).is_empty());
    }
}
