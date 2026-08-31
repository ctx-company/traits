//! `<ledger>.json.activity.jsonl` sidecar: a per-run, append-during-drive log
//! of normalized activity (P504's [`ActivityEvent`]) and finished-step
//! summaries (P455), read back by `ctx traits internal story` (P521) to render
//! detailed/assisted levels without re-deriving anything the drive already
//! observed.
//!
//! Deliberately a `.jsonl` suffix, not `.json`: `run_session.rs`'s
//! `session_ledger_names` filters `*.json` files directly under the store and
//! treats activity records as distinct from session ledgers —
//! `.activity.jsonl` does not end in `.json` at all, so it needs no such
//! clause.
//!
//! Derived evidence only, never authority (same rule as `run_summary`): a
//! write failure here is swallowed, and a read tolerates and counts skipped
//! unparseable/truncated trailing lines rather than failing the whole story.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::procedure::activity::ActivityEvent;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// One line of the sidecar. `at_epoch_ms` is stamped here, at the IO
/// boundary — core's `ActivityEvent` stays clock-free.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
pub enum ActivityRecord {
    Activity {
        at_epoch_ms: u64,
        event: ActivityEvent,
    },
    StepSummary {
        at_epoch_ms: u64,
        key: String,
        role: String,
        text: String,
    },
    /// P552 follow-up: the resolved session title, parked here by the title
    /// worker the moment it exists. The ledger's `SessionTitleState` stays
    /// the authority and is written only at a frame boundary (the drive
    /// thread owns whole-ledger writes); this record exists so a
    /// ledger-observing surface (the dashboard) can show the title without
    /// waiting out the first frame. Same doctrine as every sidecar record:
    /// derived evidence, never authority.
    SessionTitle { at_epoch_ms: u64, title: String },
    /// P146: a resolved narrator summary, parked here the moment it exists
    /// so a ledger-observing surface (the dashboard's attach) can show the
    /// same words the live panel showed instead of re-deriving from raw
    /// stream text. `frame_id` is the recorder's current frame at the
    /// moment the narration resolved (see `record_narration`'s doc for the
    /// async-attribution caveat). Derived evidence, never authority.
    Narration {
        at_epoch_ms: u64,
        frame_id: String,
        text: String,
    },
}

impl ActivityRecord {
    pub fn at_epoch_ms(&self) -> u64 {
        match self {
            ActivityRecord::Activity { at_epoch_ms, .. } => *at_epoch_ms,
            ActivityRecord::StepSummary { at_epoch_ms, .. } => *at_epoch_ms,
            ActivityRecord::SessionTitle { at_epoch_ms, .. } => *at_epoch_ms,
            ActivityRecord::Narration { at_epoch_ms, .. } => *at_epoch_ms,
        }
    }

    /// The one home for "which record is agent activity" — `Some` only for
    /// `Activity`, so a consumer filtering `StepSummary`/`SessionTitle`/
    /// `Narration` out of a rendered line has a single accessor to call
    /// rather than matching the variant itself at each call site.
    pub fn as_activity(&self) -> Option<(u64, &ActivityEvent)> {
        match self {
            ActivityRecord::Activity { at_epoch_ms, event } => Some((*at_epoch_ms, event)),
            _ => None,
        }
    }
}

/// Sidecar path for a ledger: `<ledger>.json` -> `<ledger>.json.activity.jsonl`.
pub fn activity_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    let mut path = ledger_path.as_str().to_string();
    path.push_str(".activity.jsonl");
    Utf8PathBuf::from(path)
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A durable, append-only sink for one drive's activity, opened once per
/// ledger and reused across every record it appends. Best-effort: any open
/// or write failure degrades to a no-op sink (derived evidence, never
/// authority) rather than surfacing an error into the drive loop.
type ActivityObserver = Box<dyn Fn(&ActivityRecord) + Send + Sync>;

pub struct ActivitySidecarWriter {
    file: Option<std::fs::File>,
    observer: Option<ActivityObserver>,
}

impl ActivitySidecarWriter {
    /// Open (creating if absent) the sidecar for `ledger_path`, refusing a
    /// symlinked leaf under the same discipline as the ledger itself.
    pub fn open(ledger_path: &Utf8Path) -> Self {
        let path = activity_path(ledger_path);
        if crate::run_session::reject_symlink_leaf(&path).is_err() {
            return Self {
                file: None,
                observer: None,
            };
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_std_path())
            .ok();
        Self {
            file,
            observer: None,
        }
    }

    /// Installs a best-effort observer invoked only after a complete record is
    /// appended and flushed. The observer never participates in persistence.
    pub fn set_observer(&mut self, observer: impl Fn(&ActivityRecord) + Send + Sync + 'static) {
        self.observer = Some(Box::new(observer));
    }

    fn append_line(&mut self, record: &ActivityRecord) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let Ok(mut line) = serde_json::to_string(record) else {
            return;
        };
        line.push('\n');
        if file.write_all(line.as_bytes()).is_ok()
            && file.flush().is_ok()
            && let Some(observer) = &self.observer
        {
            observer(record);
        }
    }

    pub fn append_activity(&mut self, event: ActivityEvent) {
        self.append_line(&ActivityRecord::Activity {
            at_epoch_ms: current_epoch_ms(),
            event,
        });
    }

    pub fn append_step_summary(&mut self, key: String, role: String, text: String) {
        self.append_line(&ActivityRecord::StepSummary {
            at_epoch_ms: current_epoch_ms(),
            key,
            role,
            text,
        });
    }

    /// Append the resolved session title. Safe alongside the drive thread's
    /// own writer: both hold `O_APPEND` handles and every record is one
    /// whole-line write, so concurrent appends interleave by line, never
    /// mid-record.
    pub fn append_session_title(&mut self, title: String) {
        self.append_line(&ActivityRecord::SessionTitle {
            at_epoch_ms: current_epoch_ms(),
            title,
        });
    }

    /// Append a resolved narration line. Safe alongside the drive thread's
    /// own writer under the same one-whole-line `O_APPEND` discipline as
    /// `append_session_title`.
    pub fn append_narration(&mut self, frame_id: String, text: String) {
        self.append_line(&ActivityRecord::Narration {
            at_epoch_ms: current_epoch_ms(),
            frame_id,
            text,
        });
    }
}

/// Tolerant read of a sidecar: parses one JSON value per line, skipping and
/// counting any line that fails to parse (a truncated last line from a
/// killed process, or a hand-edited file) rather than failing the whole
/// read. Absent file is not an error — it is `(empty, 0)`.
pub fn read_activity(ledger_path: &Utf8Path) -> (Vec<ActivityRecord>, usize) {
    let path = activity_path(ledger_path);
    let Ok(text) = std::fs::read_to_string(path.as_std_path()) else {
        return (Vec::new(), 0);
    };
    let mut records = Vec::new();
    let mut skipped = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ActivityRecord>(line) {
            Ok(record) => records.push(record),
            Err(_) => skipped += 1,
        }
    }
    (records, skipped)
}

/// Whether a sidecar exists at all for `ledger_path` — cheap presence check
/// used to decide `detailed`/`assisted` degradation without a full read.
pub fn activity_exists(ledger_path: &Utf8Path) -> bool {
    activity_path(ledger_path).as_std_path().is_file()
}

/// The last session title parked in the sidecar, if any — the presentation
/// fallback a ledger-observing surface uses while the ledger's own
/// `SessionTitleState` has not resolved yet (it resolves only at a frame
/// boundary). Tolerant like every sidecar read; callers should prefer the
/// ledger title whenever it exists.
pub fn read_session_title(ledger_path: &Utf8Path) -> Option<String> {
    let (records, _) = read_activity(ledger_path);
    records.into_iter().rev().find_map(|record| match record {
        ActivityRecord::SessionTitle { title, .. } => Some(title),
        _ => None,
    })
}

/// Best-effort removal of a ledger's activity sidecar, sibling to
/// ledger deletion — called wherever a ledger
/// itself is deleted (P512).
pub fn remove_activity_for_ledger(ledger_path: &Utf8Path) {
    let _ = std::fs::remove_file(activity_path(ledger_path).as_std_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::ActivityKind;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-activity-sidecar-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    fn fixture_event(sequence: u64) -> ActivityEvent {
        ActivityEvent {
            sequence,
            frame_id: "frame-1".to_string(),
            kind: ActivityKind::RunningTool,
            text: Some("edited file.rs".to_string()),
            tool: Some("edit".to_string()),
            tokens: None,
            rate_limit: None,
        }
    }

    #[test]
    fn round_trips_activity_and_step_summary_records() {
        let dir = scratch_dir("round-trip");
        let ledger_path = dir.join("session-fixture.json");
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        writer.append_step_summary(
            "frame-1".to_string(),
            "agent:worker".to_string(),
            "did the thing".to_string(),
        );
        let (records, skipped) = read_activity(&ledger_path);
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0], ActivityRecord::Activity { .. }));
        assert!(matches!(records[1], ActivityRecord::StepSummary { .. }));
    }

    #[test]
    fn as_activity_returns_the_stamp_and_event_only_for_the_activity_variant() {
        let event = fixture_event(1);
        let activity = ActivityRecord::Activity {
            at_epoch_ms: 42,
            event: event.clone(),
        };
        assert_eq!(activity.as_activity(), Some((42, &event)));

        let step_summary = ActivityRecord::StepSummary {
            at_epoch_ms: 1,
            key: "frame-1".to_string(),
            role: "agent:worker".to_string(),
            text: "did the thing".to_string(),
        };
        assert_eq!(step_summary.as_activity(), None);

        let session_title = ActivityRecord::SessionTitle {
            at_epoch_ms: 1,
            title: "a title".to_string(),
        };
        assert_eq!(session_title.as_activity(), None);

        let narration = ActivityRecord::Narration {
            at_epoch_ms: 1,
            frame_id: "frame-1".to_string(),
            text: "a narration".to_string(),
        };
        assert_eq!(narration.as_activity(), None);
    }

    #[test]
    fn tolerates_truncated_trailing_line() {
        let dir = scratch_dir("truncated");
        let ledger_path = dir.join("session-fixture.json");
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        writer.append_activity(fixture_event(2));
        drop(writer);
        // Simulate a process killed mid-write: append a truncated JSON line.
        let path = activity_path(&ledger_path);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path.as_std_path())
            .expect("open for truncated append");
        file.write_all(b"{\"record\":\"activity\",\"at-ep")
            .expect("write truncated line");
        let (records, skipped) = read_activity(&ledger_path);
        assert_eq!(records.len(), 2);
        assert_eq!(skipped, 1);
    }

    /// The title fallback returns the LAST parked title, reads `None` when
    /// no title record exists, and coexists with other record kinds.
    #[test]
    fn session_title_reads_back_last_parked_title() {
        let dir = scratch_dir("session-title");
        let ledger_path = dir.join("session-fixture.json");
        assert_eq!(read_session_title(&ledger_path), None);
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        writer.append_session_title("First title".to_string());
        writer.append_session_title("Refined title".to_string());
        assert_eq!(
            read_session_title(&ledger_path),
            Some("Refined title".to_string())
        );
        let (records, skipped) = read_activity(&ledger_path);
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn round_trips_narration_record_alongside_other_kinds() {
        let dir = scratch_dir("narration");
        let ledger_path = dir.join("session-fixture.json");
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        writer.append_narration("frame-1".to_string(), "Reading the config file".to_string());
        let (records, skipped) = read_activity(&ledger_path);
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0], ActivityRecord::Activity { .. }));
        match &records[1] {
            ActivityRecord::Narration { frame_id, text, .. } => {
                assert_eq!(frame_id, "frame-1");
                assert_eq!(text, "Reading the config file");
            }
            other => panic!("expected Narration record, got {other:?}"),
        }
    }

    #[test]
    fn absent_sidecar_reads_as_empty_not_an_error() {
        let dir = scratch_dir("absent");
        let ledger_path = dir.join("session-fixture.json");
        assert!(!activity_exists(&ledger_path));
        let (records, skipped) = read_activity(&ledger_path);
        assert!(records.is_empty());
        assert_eq!(skipped, 0);
    }

    #[test]
    fn symlinked_sidecar_path_is_refused() {
        let dir = scratch_dir("symlink");
        let ledger_path = dir.join("session-fixture.json");
        let path = activity_path(&ledger_path);
        std::fs::write(dir.join("real-target.jsonl").as_std_path(), "")
            .expect("write symlink target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.join("real-target.jsonl").as_std_path(),
            path.as_std_path(),
        )
        .expect("create symlink");
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        // The symlink target must remain untouched: the writer refused to open it.
        let target_contents = std::fs::read_to_string(dir.join("real-target.jsonl").as_std_path())
            .expect("read symlink target");
        assert!(target_contents.is_empty());
    }

    #[test]
    fn observer_runs_only_after_a_flushed_record() {
        let dir = scratch_dir("observer");
        let ledger_path = dir.join("session-fixture.json");
        let observed_after_write = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        let observer_path = ledger_path.clone();
        let observer_seen = observed_after_write.clone();
        writer.set_observer(move |record| {
            // The observer is the notification seam. It must only observe a
            // record after the exact JSONL line is durable and readable.
            let (records, skipped) = read_activity(&observer_path);
            observer_seen.store(
                skipped == 0 && records.last() == Some(record),
                std::sync::atomic::Ordering::Relaxed,
            );
        });
        writer.append_activity(fixture_event(1));
        assert!(observed_after_write.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn observer_is_suppressed_when_the_sidecar_is_unavailable() {
        let dir = scratch_dir("observer-write-failure");
        let ledger_path = dir.join("session-fixture.json");
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        let observer_seen = seen.clone();
        writer.set_observer(move |_| {
            observer_seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        // A failed open is the writer's best-effort failure representation and
        // must not be mistaken for a successfully flushed durable record.
        writer.file = None;
        writer.append_activity(fixture_event(1));
        assert_eq!(seen.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn remove_activity_for_ledger_deletes_sidecar() {
        let dir = scratch_dir("remove");
        let ledger_path = dir.join("session-fixture.json");
        let mut writer = ActivitySidecarWriter::open(&ledger_path);
        writer.append_activity(fixture_event(1));
        drop(writer);
        assert!(activity_exists(&ledger_path));
        remove_activity_for_ledger(&ledger_path);
        assert!(!activity_exists(&ledger_path));
    }
}
