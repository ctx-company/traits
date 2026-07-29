//! Session context ledger persistence (P498).
//!
//! Backs `ctx_traits_core::context::ledger::Ledger` with real IO: one JSON
//! file per host key under the new `context/` state family
//! (`~/.config/ctx/context/<repo-key>/`, [`crate::state::global_context_root`]),
//! a sibling of `runs/`, `debug/`, and `cache/` — never nested under
//! `cache/`, so cache-artifact pruning can never touch it, and it has no
//! prune path of its own (see the module doc on
//! [`crate::state::global_context_root`]).
//!
//! ## Host key
//!
//! A host key is the pair `(harness id, host-reported session id)` — e.g.
//! `("claude-code", "0f1e…-uuid")`. Both halves are untrusted input from the
//! calling harness and are charset-validated through the same shared
//! [`crate::path_safety::validate_bare_path_component`] every other
//! bare-path-component in this crate goes through before use.
//!
//! The persisted file name is `<harness>-<host-session>.json`. Because a
//! harness id can itself contain `-` (`claude-code`), this filename shape is
//! theoretically ambiguous (`claude-code` + `x` vs `claude` + `code-x`). The
//! defense is not in the file name: each ledger [`Entry`][entry]'s own
//! `host-key` field records the *exact* `"<harness>:<host-session>"` string
//! it was written under, and
//! [`ctx_traits_core::context::ledger::Ledger::reconcile`] treats any
//! mismatch — including a mismatch caused by a filename collision — as
//! `MissingHostKey` staleness, which `plan_actions` turns into `reinject`
//! rather than a false `skip-fresh`. A colliding read therefore fails
//! closed: it cannot make this session adopt another session's entries as
//! fresh.
//!
//! [entry]: ctx_traits_core::context::ledger::Entry
//!
//! ## Concurrency
//!
//! Every read-modify-write cycle (`upsert_entries`, `clear`) is
//! flock-serialized on a sibling `<name>.lock` file via
//! [`crate::file_lock::open_lock_file_no_follow`] +
//! [`crate::file_lock::lock_exclusive_blocking`] — the same primitive
//! [`crate::builtin_store`] uses to guard its publish path. This is
//! mandatory, not optional: [`crate::write::write_bytes_atomically`] writes
//! through a FIXED `.<name>.tmp` sibling, so two concurrent writers without a
//! lock collide on that temp path. A plain [`read`] (no mutation) does not
//! need the lock: a torn read is impossible because writes are atomic
//! (temp-file + rename), so a reader either sees the old file or the new
//! one, never a partial one.

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::context::ledger::Ledger;

/// The validated `(harness id, host-reported session id)` pair. Constructing
/// one is the only way any part of this module accepts host-supplied
/// identity: both halves are charset-validated before either is ever joined
/// into a path or a combined comparison key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    harness: String,
    host_session: String,
}

impl HostKey {
    pub fn new(harness: impl Into<String>, host_session: impl Into<String>) -> crate::Result<Self> {
        let harness = harness.into();
        let host_session = host_session.into();
        crate::path_safety::validate_bare_path_component(&harness, "harness id")?;
        crate::path_safety::validate_bare_path_component(&host_session, "host-session id")?;
        Ok(Self {
            harness,
            host_session,
        })
    }

    pub fn harness(&self) -> &str {
        &self.harness
    }

    pub fn host_session(&self) -> &str {
        &self.host_session
    }

    /// The exact string recorded in each [`Entry`][ctx_traits_core::context::ledger::Entry]'s
    /// `host-key` field and compared by
    /// [`ctx_traits_core::context::ledger::Ledger::reconcile`].
    pub fn combined(&self) -> String {
        format!("{}:{}", self.harness, self.host_session)
    }

    fn file_stem(&self) -> String {
        format!("{}-{}", self.harness, self.host_session)
    }
}

fn ledger_path(host_key: &HostKey) -> crate::Result<Utf8PathBuf> {
    let root = crate::state::current_global_context_root()?;
    Ok(root.join(format!("{}.json", host_key.file_stem())))
}

fn lock_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    let file_name = ledger_path.file_name().unwrap_or("context-ledger.json");
    ledger_path.with_file_name(format!(".{file_name}.lock"))
}

/// Read the persisted ledger for `host_key`, without taking the lock. A
/// missing file (never persisted, or already cleared) reads back as an
/// empty ledger — never an error; callers cannot distinguish "never
/// existed" from "cleared to empty" and do not need to.
pub fn read(host_key: &HostKey) -> crate::Result<Ledger> {
    let path = ledger_path(host_key)?;
    read_unlocked(&path, host_key)
}

fn read_unlocked(path: &Utf8Path, host_key: &HostKey) -> crate::Result<Ledger> {
    if !crate::path_safety::ensure_leaf_is_regular_file_or_absent(path, "context ledger file")? {
        return Ok(Ledger::new(host_key.combined()));
    }
    let text = crate::read::read_text(path)?;
    let ledger: Ledger =
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse context ledger at {path}"),
            source,
        })?;
    Ok(ledger)
}

fn write_locked(path: &Utf8Path, ledger: &Ledger) -> crate::Result<()> {
    let json = serde_json::to_string_pretty(ledger).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize context ledger at {path}"),
            source,
        }
    })?;
    crate::write::write_bytes_atomically(path, format!("{json}\n").as_bytes())
}

/// Acquire the exclusive lock for `host_key`'s ledger file, creating the
/// `context/<repo-key>/` root (symlink-guarded) first if it does not exist
/// yet. Held for the duration of a read-modify-write cycle.
fn lock(host_key: &HostKey) -> crate::Result<(Utf8PathBuf, std::fs::File)> {
    let root = crate::state::current_global_context_root()?;
    crate::path_safety::create_dir_all_no_symlinks(&root, "context ledger root")?;
    let path = ledger_path(host_key)?;
    let lock_file_path = lock_path(&path);
    let lock_file =
        crate::file_lock::open_lock_file_no_follow(&lock_file_path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: lock_file_path.to_string(),
                source,
            }
        })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_file_path.to_string(),
            source,
        }
    })?;
    Ok((path, lock_file))
}

/// Under the flock: read the current ledger, upsert `entries` into it
/// (matching by trait id), and write it back. Returns the ledger as
/// persisted after the upsert.
pub fn upsert_entries(
    host_key: &HostKey,
    entries: Vec<ctx_traits_core::context::ledger::Entry>,
) -> crate::Result<Ledger> {
    let (path, _lock_file) = lock(host_key)?;
    // `_lock_file`'s exclusive flock releases when it drops at the end of
    // this function (fd close), after the write below has landed.
    let mut ledger = read_unlocked(&path, host_key)?;
    for entry in entries {
        ledger.upsert(entry);
    }
    write_locked(&path, &ledger)?;
    Ok(ledger)
}

/// Under the flock: read the current ledger, drop every entry, record
/// `reason` as the last-cleared reason, and write it back. Returns the
/// number of entries removed. A missing ledger file is a no-op success with
/// count `0` — a `SessionStart` on a brand-new host session must not fail.
pub fn clear(host_key: &HostKey, reason: &str) -> crate::Result<usize> {
    let (path, _lock_file) = lock(host_key)?;
    let mut ledger = read_unlocked(&path, host_key)?;
    let cleared = ledger.clear();
    ledger.last_cleared_reason = Some(reason.to_string());
    write_locked(&path, &ledger)?;
    Ok(cleared)
}
