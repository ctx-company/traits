//! Background link to a machine-local run center: connects to an already-
//! serving center (never launches one — see `center::subscribe_existing`),
//! assembles one coherent initial snapshot, and forwards it to the gpui UI
//! thread over an async channel. Later deltas (0256.4) are not applied yet.

use ctx_traits_io::center::{self, CenterEvent, CenterPublicRow};

/// One update the UI thread may act on.
pub enum LinkUpdate {
    Snapshot(Vec<CenterPublicRow>),
    Unavailable(String),
}

/// Stages snapshot rows between `SnapshotStart` and `SnapshotEnd`, mirroring
/// the TUI dashboard worker's staging rule: never publish a partial set.
#[derive(Default)]
pub struct SnapshotAssembler {
    staged: Vec<CenterPublicRow>,
    receiving: bool,
}

impl SnapshotAssembler {
    /// Returns `Some(rows)` only at a complete `SnapshotStart..SnapshotEnd`
    /// boundary.
    pub fn accept(&mut self, event: CenterEvent) -> Option<Vec<CenterPublicRow>> {
        match event {
            CenterEvent::SnapshotStart => {
                self.staged.clear();
                self.receiving = true;
                None
            }
            CenterEvent::SnapshotRow(row) => {
                if self.receiving {
                    self.staged.push(*row);
                }
                None
            }
            CenterEvent::SnapshotEnd => {
                if self.receiving {
                    self.receiving = false;
                    Some(std::mem::take(&mut self.staged))
                } else {
                    None
                }
            }
            // Delta application is 0256.4's; it must not end or corrupt a
            // snapshot in flight, so it is simply ignored here.
            CenterEvent::Delta(_) => None,
        }
    }
}

/// Start the background link thread and return the channel its updates
/// arrive on. Connecting and forwarding both happen off the caller's thread.
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
            if let Some(rows) = assembler.accept(event)
                && tx.send_blocking(LinkUpdate::Snapshot(rows)).is_err()
            {
                break;
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

    #[test]
    fn complete_snapshot_yields_exactly_its_rows() {
        let mut assembler = SnapshotAssembler::default();
        assert_eq!(assembler.accept(CenterEvent::SnapshotStart), None);
        assert_eq!(assembler.accept(CenterEvent::SnapshotRow(row("a"))), None);
        assert_eq!(assembler.accept(CenterEvent::SnapshotRow(row("b"))), None);
        let rows = assembler
            .accept(CenterEvent::SnapshotEnd)
            .expect("complete snapshot");
        assert_eq!(
            rows.iter()
                .map(|r| r.summary.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn rows_before_snapshot_start_are_ignored() {
        let mut assembler = SnapshotAssembler::default();
        assert_eq!(
            assembler.accept(CenterEvent::SnapshotRow(row("stray"))),
            None
        );
        assert_eq!(assembler.accept(CenterEvent::SnapshotStart), None);
        let rows = assembler
            .accept(CenterEvent::SnapshotEnd)
            .expect("complete snapshot");
        assert!(rows.is_empty());
    }

    #[test]
    fn a_second_snapshot_start_discards_the_partial_set() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("discarded")));
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("kept")));
        let rows = assembler
            .accept(CenterEvent::SnapshotEnd)
            .expect("complete snapshot");
        assert_eq!(
            rows.iter()
                .map(|r| r.summary.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["kept"]
        );
    }

    #[test]
    fn a_delta_mid_snapshot_neither_emits_nor_corrupts() {
        let mut assembler = SnapshotAssembler::default();
        assembler.accept(CenterEvent::SnapshotStart);
        assembler.accept(CenterEvent::SnapshotRow(row("a")));
        assert_eq!(
            assembler.accept(CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::Ended { row: row("a") }
            )),
            None
        );
        let rows = assembler
            .accept(CenterEvent::SnapshotEnd)
            .expect("complete snapshot");
        assert_eq!(
            rows.iter()
                .map(|r| r.summary.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn snapshot_end_without_a_start_emits_nothing() {
        let mut assembler = SnapshotAssembler::default();
        assert_eq!(assembler.accept(CenterEvent::SnapshotEnd), None);
    }
}
