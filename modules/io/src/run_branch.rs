//! Durable per-unit sub-ledgers for P402 concurrent dispatch.
//!
//! `ctx traits drive`'s durable CLI/IO supervisor persists one JSON "sidecar"
//! record per speculatively-dispatched `parallel` branch or concurrent
//! `for-each` item, so a wave's outcomes survive process interruption
//! (graceful `SIGINT`) or restart, without ever becoming a second parent
//! ledger writer: the [`crate::run_session`] ledger stays the single
//! authoritative parent cursor, and sidecars are purely operational records
//! consumed by the drive loop's existing sequential replay path — never
//! merged directly into canonical trait/runtime digests.
//!
//! Sidecars are always resolved *relative to the parent ledger's own
//! resolved path* (see [`sidecars_root`]) — never a hardcoded
//! `.ctx/runs` — so a custom `--session-store` or an explicit ledger path
//! (`ctx traits drive --session ./somewhere/ledger.json`) keeps its sidecars
//! alongside that same ledger, not the default store. They live nested under
//! `<ledger-parent>/<ledger-stem>.branches/<activation-digest>/<ordinal>.json`
//! (never a flat `*.json` scan of the session store) so repeated activations
//! of the same panel/`for-each` (loops, nested constructs) never collide, and
//! so a directory listing for one activation is a cheap, targeted `read_dir`
//! instead of a full-store scan. Reuses [`crate::run_session`]'s no-follow
//! atomic write/symlink-rejection discipline instead of copying it.

use camino::{Utf8Path, Utf8PathBuf};

/// Which control construct a sidecar's unit was dispatched from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchScopeKind {
    Parallel,
    ForEach,
}

/// A sidecar's lifecycle. Set by the conductor (the sole sidecar/parent
/// writer for `Applied`) and by the worker that owns the unit for every
/// other status; a worker never writes any other unit's sidecar and never
/// touches the parent ledger directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchSidecarStatus {
    /// Slot reserved (offset claimed, dispatch about to start).
    Reserved,
    /// Harness call in flight.
    Running,
    /// Harness call returned an outcome (success or application-level
    /// failure) captured in `outcome`.
    Completed,
    /// Retries exhausted; a typed terminal-command-failure transition is
    /// what the parent will submit (P264 routing applies once the
    /// authoritative cursor reaches this ordinal).
    TerminalFailure,
    /// A graceful `SIGINT` observed this unit still in flight, or a sibling
    /// in the same wave became unreachable once the wave outcome was known;
    /// never redispatched automatically.
    Interrupted,
    /// The authoritative parent write that consumed this outcome has
    /// succeeded, AND that write's content was accepted (not routed through
    /// a P264 branch-failure policy): kept on disk as an audit record, never
    /// consumed again.
    Applied,
    /// The authoritative parent write that consumed this outcome has
    /// succeeded, but the consumed content was itself ultimately rejected —
    /// resolved through the SAME P264 `skip`/`park`/`panel-fail`
    /// branch-failure policy a serial rejection already triggers (see
    /// `reject_step_output`'s docs), rather than accepted as-is. Distinct
    /// from `Applied` so an audit reader (or a later resume) can tell "this
    /// cached outcome's content was consumed and then rejected" apart from
    /// "this cached outcome's content was consumed and accepted" — both are
    /// terminal and never replayed again, but they are not the same outcome.
    RejectedAttempt,
}

/// A captured harness outcome, or typed failure evidence, for one dispatched
/// unit. Mirrors what the sequential drive path would have produced from a
/// live call, so replay through the ordinary consumption path
/// (`take_cached_wave_run`-style logic in `ctx-cli`) is exact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchOutcomeRecord {
    Success(crate::harness::HarnessRunOutcome),
    Failure { message: String },
}

/// One durable per-unit sub-ledger record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BranchSidecar {
    pub parent_session_id: String,
    pub parent_run_id: String,
    pub scope_kind: BranchScopeKind,
    /// Absolute authored ordinal (branch offset / item index) within the
    /// owning panel/`for-each` activation.
    pub ordinal: usize,
    /// The same stable per-activation key the drive loop already computes
    /// (`parallel_wave_activation_key` or its `for-each` sibling) — identifies
    /// *this* activation of the construct, not just its `control_item_id`, so
    /// a repeated loop/`for-each` re-entry never collides with a prior one.
    pub activation_key: String,
    /// Digest of the peeked frame/position this unit was dispatched against,
    /// so a resumed reader can detect "the trait or position changed since
    /// this sidecar was written" and refuse to replay it blindly.
    pub position_digest: String,
    /// Digest of the session state this unit's dispatch was speculated
    /// against (P344/P402's "one frame of speculation" boundary).
    pub base_state_digest: String,
    pub attempt: u64,
    pub status: BranchSidecarStatus,
    pub outcome: Option<BranchOutcomeRecord>,
    pub recorded_at_epoch: u64,
}

/// One unit's immutable position/base identity, captured at original dispatch
/// time inside a [`WaveManifest`]. `position_digest`/`base_state_digest` are
/// the authoritative comparanda a resuming conductor validates every recovered
/// sidecar against — they were computed once, before any worker was spawned,
/// so a forged or stale terminal sidecar whose own digests were altered can be
/// detected even though the live session's `State` has legitimately moved on.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WaveManifestUnit {
    pub ordinal: usize,
    pub position_digest: String,
    pub base_state_digest: String,
}

/// The single authoritative, immutable record of a concurrent wave's original
/// extent (P402 `durable-sidecars-not-connected`): written exactly once,
/// atomically, BEFORE any reservation or worker, and never mutated afterward.
/// Recovery validates the complete span against this manifest — every unit's
/// terminal sidecar must exist and match the manifest's recorded
/// position/base digests — before ANY outcome from the wave is replayed, so a
/// partial, in-flight, mismatched, or forged sidecar can never advance the
/// parent cursor. Because the manifest is the sole record of the original
/// span, recovery never depends on the current invocation's `--max-in-flight`,
/// `--max-frames`, or cursor width to know which ordinals belonged to the wave.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WaveManifest {
    pub parent_session_id: String,
    pub parent_run_id: String,
    pub scope_kind: BranchScopeKind,
    pub activation_key: String,
    /// Inclusive start of the dispatched span (the base offset).
    pub span_start: usize,
    /// Exclusive end of the dispatched span.
    pub span_end: usize,
    /// One entry per ordinal in `[span_start, span_end)`, in ascending order.
    pub units: Vec<WaveManifestUnit>,
    pub recorded_at_epoch: u64,
}

impl WaveManifest {
    /// The immutable identity captured for `ordinal`, if it was part of this
    /// wave's original span.
    pub fn unit(&self, ordinal: usize) -> Option<&WaveManifestUnit> {
        self.units.iter().find(|unit| unit.ordinal == ordinal)
    }
}

/// Directory root holding every activation's sidecars for one parent ledger,
/// derived from the *resolved* ledger path (see module docs) — never a
/// hardcoded default store, so `--session-store`/explicit-path sessions keep
/// their sidecars alongside their own ledger.
pub fn sidecars_root(ledger_path: &Utf8Path) -> Utf8PathBuf {
    let parent = ledger_path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let stem = ledger_path
        .file_name()
        .and_then(|name| name.strip_suffix(".json"))
        .unwrap_or_else(|| ledger_path.as_str());
    parent.join(format!("{stem}.branches"))
}

/// Directory holding every sidecar for one activation of a `parallel` panel
/// or concurrent `for-each`, under the parent ledger's sidecars root.
pub fn activation_dir(ledger_path: &Utf8Path, activation_key: &str) -> Utf8PathBuf {
    sidecars_root(ledger_path).join(activation_digest(activation_key))
}

/// Path to the single conductor lease file for a parent run (one lease per
/// parent run: the supervisor holding it is the sole parent-ledger writer).
pub fn conductor_lease_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    sidecars_root(ledger_path).join("conductor.lock")
}

/// Filesystem-safe digest of an activation key: the key itself may contain
/// `/`, `:`, and other authored-id characters that are awkward or unsafe as a
/// bare directory component, so the directory name is always this digest's
/// hex body (never the raw key).
fn activation_digest(activation_key: &str) -> String {
    let digest = ctx_traits_core::digest::Digest::source(activation_key).to_string();
    digest
        .strip_prefix("sha256:")
        .unwrap_or(&digest)
        .to_string()
}

pub fn sidecar_path(ledger_path: &Utf8Path, activation_key: &str, ordinal: usize) -> Utf8PathBuf {
    activation_dir(ledger_path, activation_key).join(format!("{ordinal}.json"))
}

/// Path to one activation's single immutable [`WaveManifest`], under the same
/// activation directory as its per-ordinal sidecars. A leading `_` keeps it
/// lexically distinct from the numeric `<ordinal>.json` sidecars (and out of
/// [`list_activation_sidecars`], which only parses numeric stems).
pub fn wave_manifest_path(ledger_path: &Utf8Path, activation_key: &str) -> Utf8PathBuf {
    activation_dir(ledger_path, activation_key).join("_manifest.json")
}

/// Persist any sidecar-shaped JSON value atomically and symlink-safely (same
/// discipline as the parent session ledger — see
/// [`crate::run_session::write_run_session`]). Shared by every sidecar kind
/// (wave manifests, branch sidecars, harness sessions) so the write
/// discipline lives in exactly one place.
fn write_json_sidecar<T: serde::Serialize>(
    path: &Utf8Path,
    value: &T,
    label: &str,
) -> crate::Result<()> {
    crate::run_session::reject_symlink_ancestors(path)?;
    if let Some(parent) = path.parent() {
        if !parent.as_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source: e,
            })?;
        }
    }
    crate::run_session::reject_symlink_ancestors(path)?;
    crate::run_session::reject_symlink_leaf(path)?;
    let text = serde_json::to_string_pretty(value).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize {label} JSON at {path}"),
            source,
        }
    })?;
    crate::run_session::reject_symlink_leaf(path)?;
    crate::run_session::write_text_atomically(path, &format!("{text}\n"))
}

/// Read back a previously-written sidecar-shaped JSON value, if present.
/// Shared reader counterpart to [`write_json_sidecar`].
fn read_json_sidecar<T: serde::de::DeserializeOwned>(
    path: &Utf8Path,
    label: &str,
) -> crate::Result<Option<T>> {
    crate::run_session::reject_symlink_ancestors(path)?;
    crate::run_session::reject_symlink_leaf(path)?;
    match std::fs::metadata(path.as_std_path()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: e,
            }
            .into());
        }
    }
    let text = crate::read::read_text(path)?;
    let value =
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse {label} JSON at {path}"),
            source,
        })?;
    Ok(Some(value))
}

/// Persist a wave manifest, atomically and symlink-safely. Written exactly once
/// per activation, before any reservation — never rewritten (its whole value is
/// immutability), so a caller must treat an already-present manifest as proof
/// this activation's wave was already dispatched.
pub fn write_wave_manifest(path: &Utf8Path, manifest: &WaveManifest) -> crate::Result<()> {
    write_json_sidecar(path, manifest, "wave manifest")
}

/// Read back an activation's wave manifest, if one was ever written.
pub fn read_wave_manifest(path: &Utf8Path) -> crate::Result<Option<WaveManifest>> {
    read_json_sidecar(path, "wave manifest")
}

/// Persist a sidecar, atomically and symlink-safely.
pub fn write_sidecar(path: &Utf8Path, sidecar: &BranchSidecar) -> crate::Result<()> {
    write_json_sidecar(path, sidecar, "branch sidecar")
}

/// Read back a previously-written sidecar, if present.
pub fn read_sidecar(path: &Utf8Path) -> crate::Result<Option<BranchSidecar>> {
    read_json_sidecar(path, "branch sidecar")
}

/// Path to the harness-conversation-id sidecar for one parent run, under the
/// same sidecars root every other per-run sidecar lives in — so deleting a
/// run (`sidecars_root` is already removed by every existing delete path)
/// removes it for free, with no additional cleanup code.
///
/// Conversation ids are operational runtime state, not canonical evidence,
/// and must never enter `state_digest` — this sidecar, not the session
/// ledger, is where they live (P516).
pub fn harness_sessions_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    sidecars_root(ledger_path).join("harness-sessions.json")
}

/// One `RunSessionMode::Persistent` frame's harness conversation, keyed by
/// `session_key` in [`HarnessSessions`].
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessSessionEntry {
    /// The harness declaration this conversation belongs to (`[agent.*]`'s
    /// `harness` field), carried for audit — a harness mismatch cannot
    /// actually occur here since `session_key` (this entry's map key) already
    /// embeds the harness id via `effective_session`'s `scoped_key`, so the
    /// resume guard that matters at read time is `exec_dir` alone.
    pub harness_id: String,
    /// The worktree execution directory the conversation was anchored to,
    /// or `None` for a run with no worktree in play. A pruned or recreated
    /// worktree (or a resume whose worktree-in-play status changed) no
    /// longer matches, and the frame starts a clean, explicitly-reasoned
    /// cold conversation instead of resuming into a directory the harness
    /// never actually ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_dir: Option<String>,
    /// The harness-native conversation/session id to resume with.
    pub harness_session_id: String,
}

/// Every live harness conversation for one parent run, persisted so a
/// `ctx traits drive --session <id>` resume (a new process) can rejoin
/// conversations a prior process observed instead of restarting them cold.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessSessions {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub sessions: std::collections::BTreeMap<String, HarnessSessionEntry>,
}

/// Persist the harness-conversation sidecar, atomically and symlink-safely.
pub fn write_harness_sessions(path: &Utf8Path, sessions: &HarnessSessions) -> crate::Result<()> {
    write_json_sidecar(path, sessions, "harness sessions")
}

/// Read back the harness-conversation sidecar. A missing or unreadable
/// sidecar reads as empty — exactly today's cold-start behavior, so a repo
/// that predates this sidecar (or a deleted one) is unaffected.
pub fn read_harness_sessions(path: &Utf8Path) -> crate::Result<HarnessSessions> {
    Ok(read_json_sidecar(path, "harness sessions")?.unwrap_or_default())
}

/// List every sidecar under one activation directory, sorted by ordinal —
/// the shape a resuming conductor walks to reconstruct a wave's durable
/// state before spending any new frame budget. Missing directory reads as
/// empty (nothing dispatched for this activation yet, or nothing survived).
pub fn list_activation_sidecars(
    ledger_path: &Utf8Path,
    activation_key: &str,
) -> crate::Result<Vec<(usize, Utf8PathBuf, BranchSidecar)>> {
    let dir = activation_dir(ledger_path, activation_key);
    let entries = match std::fs::read_dir(dir.as_std_path()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: dir.to_string(),
                source: e,
            }
            .into());
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source: e,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let Ok(ordinal) = stem.parse::<usize>() else {
            continue;
        };
        let path = dir.join(&name);
        if let Some(sidecar) = read_sidecar(&path)? {
            out.push((ordinal, path, sidecar));
        }
    }
    out.sort_by_key(|(ordinal, _, _)| *ordinal);
    Ok(out)
}

/// Mark a sidecar `applied`: only the conductor calls this, and only after
/// the parent ledger write it corresponds to has already succeeded — an
/// `applied` sidecar is never replayed again on a later resume.
pub fn mark_applied(path: &Utf8Path) -> crate::Result<()> {
    let Some(mut sidecar) = read_sidecar(path)? else {
        return Ok(());
    };
    sidecar.status = BranchSidecarStatus::Applied;
    write_sidecar(path, &sidecar)
}

/// Mark a sidecar `rejected-attempt`: only the conductor calls this, and only
/// after the parent ledger write that consumed this cached outcome has
/// already succeeded AND that write's content was resolved as a P264
/// `skip`/`park`/`panel-fail` branch-failure rather than accepted — see
/// [`BranchSidecarStatus::RejectedAttempt`]. A `rejected-attempt` sidecar is,
/// like `applied`, never replayed again on a later resume.
pub fn mark_rejected_attempt(path: &Utf8Path) -> crate::Result<()> {
    let Some(mut sidecar) = read_sidecar(path)? else {
        return Ok(());
    };
    sidecar.status = BranchSidecarStatus::RejectedAttempt;
    write_sidecar(path, &sidecar)
}

/// Validate a recovered sidecar's identity against the parent
/// session/run/activation/ordinal it must belong to AND against the immutable
/// [`WaveManifest`] unit captured at original dispatch time, before it is
/// trusted for replay (P402 risk: a mismatched, stale, or forged sidecar must
/// never be silently replayed). Returns a human-readable mismatch reason on
/// failure.
///
/// `expected_position_digest`/`expected_base_state_digest` come from the
/// wave's own [`WaveManifest`] — NOT a fresh digest recomputed against the
/// current (possibly already-mutated) parent cursor. That distinction is what
/// lets this gate be sound even after an earlier sibling in the same wave was
/// applied: the live session's `State` legitimately differs from the one this
/// later ordinal was dispatched against, but the manifest recorded what the
/// digests *were* at dispatch, so a byte comparison against the manifest
/// accepts an honest resume while still rejecting a sidecar whose own recorded
/// `position_digest`/`base_state_digest` were tampered with or belong to a
/// different original dispatch.
pub fn validate_recovered_identity(
    sidecar: &BranchSidecar,
    parent_session_id: &str,
    parent_run_id: &str,
    activation_key: &str,
    ordinal: usize,
    expected_position_digest: &str,
    expected_base_state_digest: &str,
) -> Result<(), String> {
    if sidecar.parent_session_id != parent_session_id {
        return Err(format!(
            "sidecar parent-session-id {:?} does not match current session {parent_session_id:?}",
            sidecar.parent_session_id
        ));
    }
    if sidecar.parent_run_id != parent_run_id {
        return Err(format!(
            "sidecar parent-run-id {:?} does not match current run {parent_run_id:?}",
            sidecar.parent_run_id
        ));
    }
    if sidecar.activation_key != activation_key {
        return Err(format!(
            "sidecar activation-key {:?} does not match current activation {activation_key:?}",
            sidecar.activation_key
        ));
    }
    if sidecar.ordinal != ordinal {
        return Err(format!(
            "sidecar ordinal {} does not match expected ordinal {ordinal}",
            sidecar.ordinal
        ));
    }
    if sidecar.position_digest != expected_position_digest {
        return Err(format!(
            "sidecar position-digest {:?} does not match the wave manifest's recorded position for ordinal {ordinal}",
            sidecar.position_digest
        ));
    }
    if sidecar.base_state_digest != expected_base_state_digest {
        return Err(format!(
            "sidecar base-state-digest {:?} does not match the wave manifest's recorded base state for ordinal {ordinal}",
            sidecar.base_state_digest
        ));
    }
    Ok(())
}
