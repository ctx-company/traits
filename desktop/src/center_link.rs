//! Background link to a machine-local run center: connects to an already-
//! serving center (never launches one — see `center::subscribe_existing`),
//! assembles one coherent initial snapshot, and forwards it — followed by
//! every later delta, in subscription order — to the gpui UI thread over a
//! single async channel. The snapshot-plus-delta stream is the exclusive
//! source of dashboard change; nothing here scans a run directory or polls
//! the center.

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use ctx_traits_io::center::{self, CenterDelta, CenterEvent, CenterPublicRow};

/// Fixed reconnect backoff, parity with the CLI dashboard worker's
/// `wait_for_retry` (`modules/cli/src/app/dashboard/worker.rs`).
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// How often an idle pump wakes up to check whether the UI receiver was
/// dropped. Bounds how long a subscription can be leaked (thread, socket,
/// server-side subscriber) after the consumer is gone but the center itself
/// stays silent — see `pump`.
const CONSUMER_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// One update the UI thread may act on, in the exact order the center
/// produced it.
pub enum LinkUpdate {
    Snapshot(Vec<CenterPublicRow>),
    Delta(CenterDelta),
    /// The subscription is down, with a reason. Absent-at-first-contact vs.
    /// mid-stream loss vs. eviction are indistinguishable on this wire and
    /// must stay that way — the face classifies the transition, not the
    /// link (see `shell::CenterFace`).
    Down(String),
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
            // Board changes are consumed by the board request path; they do
            // not alter the run snapshot assembled by this link.
            CenterEvent::BoardChanged { .. } => Vec::new(),
        }
    }
}

/// What one bounded wait on the subscription yielded.
enum PumpEvent {
    Event(CenterEvent),
    /// No event arrived within `CONSUMER_POLL_INTERVAL`; the subscription is
    /// still open. The caller re-checks `tx.is_closed()` and waits again.
    Timeout,
    /// The stream ended (center exit, disconnect, or backpressure eviction —
    /// all the same signal on this wire).
    Closed,
}

/// How `pump` ended.
enum PumpOutcome {
    /// The subscription's stream ended on its own. Carries whether at least
    /// one `Snapshot` was delivered this run, which is what lets the caller
    /// clear its outage memo — a stream that dies mid-snapshot publishes
    /// nothing, so no partial snapshot can ever reach the UI.
    StreamEnded { snapshot_sent: bool },
    /// The UI consumer is gone. The caller must drop the subscription and
    /// stop retrying without announcing a `Down` — there is no one left to
    /// show it to.
    ConsumerGone,
}

/// Forward one subscription's events until it ends or the UI consumer is
/// gone. `next` is polled on a bounded timeout rather than a blocking
/// `recv()`, so an idle subscription (connected, but with no events in
/// flight) still notices a dropped `tx` within `CONSUMER_POLL_INTERVAL`
/// instead of leaking the thread, socket and server-side subscriber for the
/// life of the process.
fn pump(
    mut next: impl FnMut(Duration) -> PumpEvent,
    assembler: &mut SnapshotAssembler,
    tx: &async_channel::Sender<LinkUpdate>,
) -> PumpOutcome {
    let mut snapshot_sent = false;
    loop {
        if tx.is_closed() {
            return PumpOutcome::ConsumerGone;
        }
        match next(CONSUMER_POLL_INTERVAL) {
            PumpEvent::Event(event) => {
                for update in assembler.accept(event) {
                    if matches!(update, LinkUpdate::Snapshot(_)) {
                        snapshot_sent = true;
                    }
                    if tx.send_blocking(update).is_err() {
                        return PumpOutcome::ConsumerGone;
                    }
                }
            }
            PumpEvent::Timeout => {}
            PumpEvent::Closed => return PumpOutcome::StreamEnded { snapshot_sent },
        }
    }
}

/// Start the background link thread and return the channel its updates
/// arrive on. Connecting and forwarding both happen off the caller's thread.
/// The bounded-wait pump loop stays on this dedicated thread — never on
/// gpui's UI thread — and feeds one unbounded channel, which is what
/// preserves subscription order end to end.
///
/// The thread is a supervisor: a connect failure or a mid-stream stream end
/// (center exit, disconnect, or backpressure eviction — all the same signal
/// on this wire, see `ctx_traits_io::center`) is announced as a `Down` and
/// retried after a fixed backoff, forever, until the receiver is dropped.
/// A lost consumer ends the thread immediately, without announcing a
/// `Down` — there is no one left to show it to — whether it is noticed
/// before subscribing or, via `pump`'s periodic check, while an otherwise
/// idle subscription is open.
pub fn start(repo_key: Option<String>) -> async_channel::Receiver<LinkUpdate> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        // De-duplicates repeated identical outages so a permanently absent
        // center does not produce (and repaint) a `Down` every backoff tick
        // for the life of the process. Cleared whenever a snapshot is
        // delivered, so the next distinct outage is announced again.
        let mut last_down: Option<String> = None;
        loop {
            if tx.is_closed() {
                return;
            }
            match center::subscribe_existing(repo_key.as_deref()) {
                Ok(subscription) => {
                    let mut assembler = SnapshotAssembler::default();
                    let outcome = pump(
                        |timeout| match subscription.recv_timeout(timeout) {
                            Ok(event) => PumpEvent::Event(event),
                            Err(RecvTimeoutError::Timeout) => PumpEvent::Timeout,
                            Err(RecvTimeoutError::Disconnected) => PumpEvent::Closed,
                        },
                        &mut assembler,
                        &tx,
                    );
                    // The subscription's `Drop` shuts the socket down,
                    // freeing the server-side subscriber immediately rather
                    // than waiting for its idle timeout. This runs whether
                    // the stream ended on its own or the consumer went away.
                    drop(subscription);
                    match outcome {
                        PumpOutcome::StreamEnded { snapshot_sent } => {
                            if snapshot_sent {
                                last_down = None;
                            }
                            if announce(&tx, &mut last_down, "subscription closed".to_string())
                                .is_err()
                            {
                                return;
                            }
                        }
                        // No Down to announce: there is no consumer left to
                        // show it to.
                        PumpOutcome::ConsumerGone => return,
                    }
                }
                Err(error) => {
                    if announce(&tx, &mut last_down, error.to_string()).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(RECONNECT_DELAY);
        }
    });
    rx
}

/// Emit a `Down` only when its reason differs from the last one emitted.
fn announce(
    tx: &async_channel::Sender<LinkUpdate>,
    last_down: &mut Option<String>,
    reason: String,
) -> Result<(), ()> {
    if last_down.as_deref() == Some(reason.as_str()) {
        return Ok(());
    }
    *last_down = Some(reason.clone());
    tx.send_blocking(LinkUpdate::Down(reason)).map_err(|_| ())
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

    fn drain(
        tx_pair: &(
            async_channel::Sender<LinkUpdate>,
            async_channel::Receiver<LinkUpdate>,
        ),
    ) -> Vec<LinkUpdate> {
        let (_, rx) = tx_pair;
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        updates
    }

    /// Adapts a fixed event list to `pump`'s bounded-wait signature: each
    /// call returns the next event immediately (never `Timeout`), and
    /// `Closed` once the list is exhausted — i.e. a stream that ends on its
    /// own, not a dropped consumer.
    fn from_events(events: Vec<CenterEvent>) -> impl FnMut(Duration) -> PumpEvent {
        let mut events = events.into_iter();
        move |_timeout| match events.next() {
            Some(event) => PumpEvent::Event(event),
            None => PumpEvent::Closed,
        }
    }

    fn expect_stream_ended(outcome: PumpOutcome) -> bool {
        match outcome {
            PumpOutcome::StreamEnded { snapshot_sent } => snapshot_sent,
            PumpOutcome::ConsumerGone => panic!("expected the stream to end, not the consumer"),
        }
    }

    #[test]
    fn pump_forwards_snapshot_then_delta_in_order() {
        let mut assembler = SnapshotAssembler::default();
        let channel = async_channel::unbounded();
        let outcome = pump(
            from_events(vec![
                CenterEvent::SnapshotStart,
                CenterEvent::SnapshotRow(row("a")),
                CenterEvent::SnapshotEnd,
                CenterEvent::Delta(ended_delta("later")),
            ]),
            &mut assembler,
            &channel.0,
        );
        assert!(expect_stream_ended(outcome), "a snapshot was delivered");
        let updates = drain(&channel);
        assert_eq!(snapshot_run_ids(&updates), vec!["a"]);
        assert!(matches!(updates[0], LinkUpdate::Snapshot(_)));
        assert!(matches!(updates[1], LinkUpdate::Delta(_)));
    }

    #[test]
    fn pump_publishes_nothing_when_the_stream_dies_mid_snapshot() {
        let mut assembler = SnapshotAssembler::default();
        let channel = async_channel::unbounded();
        let outcome = pump(
            from_events(vec![
                CenterEvent::SnapshotStart,
                CenterEvent::SnapshotRow(row("orphaned")),
            ]),
            &mut assembler,
            &channel.0,
        );
        assert!(
            !expect_stream_ended(outcome),
            "no snapshot should be published mid-boundary"
        );
        assert!(drain(&channel).is_empty());
    }

    #[test]
    fn a_second_subscriptions_snapshot_after_a_dead_one_carries_only_new_rows() {
        let channel = async_channel::unbounded();

        let mut assembler = SnapshotAssembler::default();
        pump(
            from_events(vec![
                CenterEvent::SnapshotStart,
                CenterEvent::SnapshotRow(row("orphaned")),
            ]),
            &mut assembler,
            &channel.0,
        );

        // A fresh assembler per attempt, exactly as `start` builds one per
        // reconnect: nothing from the dead attempt can leak in.
        let mut assembler = SnapshotAssembler::default();
        let outcome = pump(
            from_events(vec![
                CenterEvent::SnapshotStart,
                CenterEvent::SnapshotRow(row("kept")),
                CenterEvent::SnapshotEnd,
            ]),
            &mut assembler,
            &channel.0,
        );
        assert!(expect_stream_ended(outcome));
        let updates = drain(&channel);
        assert_eq!(snapshot_run_ids(&updates), vec!["kept"]);
    }

    #[test]
    fn pump_stops_when_the_consumer_is_gone_immediately() {
        let (tx, rx) = async_channel::unbounded();
        drop(rx);
        let mut assembler = SnapshotAssembler::default();
        let outcome = pump(
            from_events(vec![CenterEvent::SnapshotStart, CenterEvent::SnapshotEnd]),
            &mut assembler,
            &tx,
        );
        assert!(matches!(outcome, PumpOutcome::ConsumerGone));
    }

    #[test]
    fn pump_stops_when_the_consumer_is_gone_while_idle() {
        // The subscription itself never ends (always `Timeout`, never
        // `Closed`); only the periodic `tx.is_closed()` check between waits
        // can end this pump. Proves the idle-consumer fix: a dropped
        // receiver is noticed even with no events in flight, after waiting
        // through a few timeouts rather than on the very first check.
        let (tx, rx) = async_channel::unbounded();
        let mut rx = Some(rx);
        let mut assembler = SnapshotAssembler::default();
        let mut waits = 0;
        let outcome = pump(
            |_timeout| {
                waits += 1;
                if waits == 3 {
                    rx.take();
                }
                PumpEvent::Timeout
            },
            &mut assembler,
            &tx,
        );
        assert!(matches!(outcome, PumpOutcome::ConsumerGone));
        assert!(
            waits >= 3,
            "pump must wait through idle timeouts before noticing: {waits}"
        );
    }
}
