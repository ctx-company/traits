//! Shared machine-wide JSONL audit-journal append boundary.
//!
//! One append primitive so every mutation-recording caller — P438 project
//! distribution (install/remove) and P441 host placement — writes the same
//! `<root>/<YYYY-MM>.jsonl` shape under the same `flock`-serialized,
//! symlink-guarded, fsynced append discipline, instead of growing separate
//! private journal writers. Append-only, never read back by any product
//! behavior — this is evidence for a human or downstream tool, not runtime
//! state.

use crate::file_lock::{lock_exclusive_blocking, open_lock_file_no_follow};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use std::io::Write as _;

/// One audit record for a successful project- or global-distribution
/// mutation (P438/P439).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuditRecord {
    pub action: AuditAction,
    pub package: String,
    pub requested_selector: String,
    pub resolved_version: String,
    pub trait_digests: Vec<String>,
    pub project_path: String,
    /// `"project"` or `"global"` (P439): additive field distinguishing a
    /// repo-scoped mutation from a per-machine one, since `project_path`
    /// alone is ambiguous once global installs also record a root path
    /// (the `ctx` config-home root, not a project). Never read back by any
    /// product behavior — evidence only.
    pub scope: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditAction {
    Install,
    Remove,
}

/// Root audit directory: `~/.config/ctx/audit`.
pub fn audit_root() -> crate::Result<Utf8PathBuf> {
    Ok(crate::state::global_ctx_root()?.join("audit"))
}

/// Append one P438 distribution record to this month's journal file under
/// [`audit_root`], through the same locked primitive host placement uses.
pub fn append_record(month: &str, record: &AuditRecord) -> crate::Result<()> {
    let root = audit_root()?;
    let event =
        serde_json::to_value(record).map_err(|source| crate::parse::Error::JsonSerialize {
            context: format!("encode audit record for {root}/{month}.jsonl"),
            source,
        })?;
    append_with_month(&root, month, &event)
}

/// Append one JSON value as a single line to the month-keyed journal file
/// under `root` (`<root>/<YYYY-MM>.jsonl`, month derived from `unix_seconds`
/// UTC). Creates `root` and the journal file if either is missing.
pub fn append(root: &Utf8Path, unix_seconds: u64, event: &serde_json::Value) -> crate::Result<()> {
    let (year, month, _day) = civil_from_days((unix_seconds / 86_400) as i64);
    append_with_month(root, &format!("{year:04}-{month:02}"), event)
}

/// The one low-level append: holds an exclusive `flock` on a sibling
/// `.lock` file for the duration of the append so concurrent `ctx`
/// invocations never interleave partial lines, refuses symlinked roots and
/// non-regular journal files, and fsyncs before releasing.
fn append_with_month(root: &Utf8Path, month: &str, event: &serde_json::Value) -> crate::Result<()> {
    crate::path_safety::create_dir_all_no_symlinks(root, "audit journal root")?;
    let journal_path = root.join(format!("{month}.jsonl"));
    let lock_path = root.join(".audit-journal.lock");

    let lock_file = open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;

    let mut line =
        serde_json::to_string(event).map_err(|source| crate::parse::Error::JsonSerialize {
            context: journal_path.to_string(),
            source,
        })?;
    line.push('\n');

    crate::path_safety::ensure_no_symlink_ancestors(root, "audit journal root")?;
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(&journal_path, "audit journal file")?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path.as_std_path())
        .map_err(|source| crate::environment::Error::Filesystem {
            path: journal_path.to_string(),
            source,
        })?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|source| crate::environment::Error::Filesystem {
            path: journal_path.to_string(),
            source,
        })?;

    // The lock file's `flock` releases automatically when `lock_file` drops
    // at the end of this function.
    Ok(())
}

/// The current UTC month key (`YYYY-MM`) and second-precision timestamp,
/// used both to name the journal file and to stamp each record.
pub fn epoch_month_and_timestamp() -> (String, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let time_of_day = secs % 86_400;
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );
    (
        format!("{year:04}-{month:02}"),
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
    )
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch to a proleptic
/// Gregorian (year, month, day), used to avoid pulling in a full date/time
/// dependency for a monthly journal file name.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}
