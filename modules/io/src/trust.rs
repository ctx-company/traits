//! Machine-local dependency trust store.
//!
//! Trust is the consumer machine's judgment about exact digest evidence. It is
//! stored outside canonical manifests under the user's config directory, so a
//! package cannot self-assert trust by editing its own manifest.
//! Updates are flock-serialized atomic read-modify-write operations. Records
//! are events: historical approval and block evidence is never rewritten.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustState {
    Verified,
    Blocked,
}

impl TrustState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Blocked => "blocked",
        }
    }
}

impl From<TrustState> for ctx_traits_core::r#trait::TrustVerdict {
    fn from(state: TrustState) -> Self {
        match state {
            TrustState::Verified => Self::Verified,
            TrustState::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TrustRecord {
    pub digest: String,
    pub state: TrustState,
    /// Trait ID this record was recorded against, when the update came from
    /// a named `trust approve <trait>`/`trust block <trait>` (or a package
    /// approval, which stamps every member trait's ID). `None` for raw
    /// `--digest` records and for records written before P419, which decode
    /// unchanged and remain authoritative exact-digest evidence — legacy
    /// absence of an ID is never inferred or backfilled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    /// The approval ACT this record was written in: every record from one
    /// `update_digests_locked` call shares it. It preserves append-only
    /// history metadata; executable authority is always exact-digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Monotonic store-local evidence sequence. Older stores did not have a
    /// sequence; they remain readable and are assigned one on their first
    /// subsequent mutation under the store lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Document {
    #[serde(rename = "digest", default, skip_serializing_if = "Vec::is_empty")]
    pub digests: Vec<TrustRecord>,
}

impl Document {
    /// The latest exact-digest decision applicable to `trait_id`. Raw digest
    /// evidence remains applicable to every trait with those exact bytes.
    /// Store order resolves ties in legacy unsequenced evidence, matching the
    /// append-only order used when sequences are later assigned.
    pub fn exact_record(&self, trait_id: &str, digest: &str) -> Option<&TrustRecord> {
        self.digests
            .iter()
            .filter(|record| {
                record.digest == digest
                    && (record.trait_id.is_none() || record.trait_id.as_deref() == Some(trait_id))
            })
            .max_by_key(|record| record.seq.unwrap_or(0))
    }

    pub fn record(&self, digest: &str) -> Option<&TrustRecord> {
        self.digests
            .iter()
            .filter(|record| record.digest == digest)
            .max_by_key(|record| record.seq.unwrap_or(0))
    }

    /// The latest identity-bound record for `trait_id`, retained as history
    /// for reporting only. Start authority is always exact-digest evidence.
    pub fn record_for_trait(&self, trait_id: &str) -> Option<&TrustRecord> {
        self.digests
            .iter()
            .filter(|record| record.trait_id.as_deref() == Some(trait_id))
            .max_by_key(|record| record.seq.unwrap_or(0))
    }

    /// The prior approval for a named trait. Blocks remain historical events,
    /// but cannot be reported as evidence an approval superseded.
    pub fn verified_record_for_trait(&self, trait_id: &str) -> Option<&TrustRecord> {
        self.digests
            .iter()
            .filter(|record| {
                record.trait_id.as_deref() == Some(trait_id) && record.state == TrustState::Verified
            })
            .max_by_key(|record| record.seq.unwrap_or(0))
    }

    pub fn latest_named_for_digest(&self, trait_id: &str, digest: &str) -> Option<&TrustRecord> {
        self.digests
            .iter()
            .filter(|record| {
                record.trait_id.as_deref() == Some(trait_id) && record.digest == digest
            })
            .max_by_key(|record| record.seq.unwrap_or(0))
    }

    /// Reporting selection: exact current-digest evidence (named or raw)
    /// wins, then the latest identity history supplies stale context.
    pub fn record_for_current(&self, trait_id: &str, digest: &str) -> Option<&TrustRecord> {
        self.exact_record(trait_id, digest)
            .or_else(|| self.record_for_trait(trait_id))
    }
}

/// Start-time decision derived from exact-digest append-only evidence.
/// Raw digest records intentionally remain exact-digest evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartTrust {
    Verified(TrustRecord),
    Blocked(TrustRecord),
    Unreviewed,
}

impl Document {
    pub fn start_trust(&self, trait_id: &str, digest: &str) -> StartTrust {
        // Trust is a decision about exact canonical bytes. Identity history is
        // retained for reporting, but a later decision for another digest does
        // not invalidate this digest's last decision.
        let candidate = self.exact_record(trait_id, digest);
        match candidate {
            Some(record) if record.state == TrustState::Verified => {
                StartTrust::Verified(record.clone())
            }
            Some(record) => StartTrust::Blocked(record.clone()),
            None => StartTrust::Unreviewed,
        }
    }
}

/// Where a [`TrustRecord`] stands relative to the trait it names, once
/// joined against that trait's current resolved canonical digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustFreshness {
    /// The recorded digest equals the trait's current resolved digest
    /// (identity-bound), or a legacy digest-only record matches some
    /// currently visible trait's digest.
    Current,
    /// An identity-bound record whose trait still resolves, but to a
    /// different (rebuilt) digest than the one recorded.
    Stale,
    /// No currently visible trait can be associated with this record, by
    /// identity or by exact digest. Never guessed from name similarity or
    /// history.
    Orphaned,
}

/// A [`TrustRecord`] joined against current trait resolution: the shared
/// classification `trust <trait>`, `trust list`, and `doctor` all read from,
/// so none of them re-derive current/stale/orphan independently.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TrustReportRow {
    pub trait_id: Option<String>,
    pub digest: String,
    /// The named trait's current resolved digest, when `trait_id` is known
    /// and still resolves. `None` for a legacy digest-only record, or an
    /// identity-bound record whose trait no longer resolves (orphaned).
    pub current_digest: Option<String>,
    pub state: TrustState,
    pub freshness: TrustFreshness,
    pub updated_at: Option<String>,
    pub reason: Option<String>,
    /// This record's append-only store sequence, `None` for a legacy record
    /// that has never been through the write lock's `assign_legacy_sequences`
    /// pass. The only authority `superseded` and any caller-side currency
    /// selection may use — never `updated_at`, which is wall-clock and not
    /// serialized with the lock.
    pub seq: Option<u64>,
    /// `true` when another record for the same `trait_id` (any state) carries
    /// a higher sequence, i.e. this row is not the lineage's latest event.
    /// Distinct from `freshness`, which compares against the trait's current
    /// *build* rather than its recorded history: a superseded row can still
    /// be `Current` (re-approving the same digest bumps the sequence without
    /// changing the digest), and a non-superseded row can still be `Stale`
    /// (the trait rebuilt but no new trust decision was ever recorded).
    /// `false` for digest-only records, which have no identity lineage.
    pub superseded: bool,
}

impl TrustReportRow {
    /// Whether this row is a *stale approval* — P419's specific meaning of
    /// `trust list --stale` and doctor's stale finding: a VERIFIED record
    /// whose identity-bound trait rebuilt to a different digest, so a human
    /// who thinks they already approved the current bytes has not. A moved
    /// BLOCKED record is never a stale approval: the trait's new digest
    /// already reads as unreviewed (the fail-safe default) with no stale
    /// approval creating a false sense of trust, so there is nothing to
    /// warn about or re-approve.
    pub fn is_stale_approval(&self) -> bool {
        self.freshness == TrustFreshness::Stale && self.state == TrustState::Verified
    }
}

/// Classify every record in `document` against `current`, the caller's
/// resolved `(trait_id, current_canonical_digest)` pairs for every trait
/// currently visible in the repository (built from the same trait inventory
/// scan `list`/`doctor` already run — this function never re-resolves
/// traits itself).
pub fn classify_records(document: &Document, current: &[(String, String)]) -> Vec<TrustReportRow> {
    let mut current_by_id: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> =
        std::collections::BTreeMap::new();
    for (id, digest) in current {
        current_by_id
            .entry(id.as_str())
            .or_default()
            .insert(digest.as_str());
    }
    let current_digests: std::collections::HashSet<&str> =
        current.iter().map(|(_, digest)| digest.as_str()).collect();
    // Per-trait-lineage max sequence, the sole authority `superseded` reads
    // from — never wall-clock `updated_at` (P534 review blocker 2).
    let mut max_seq_by_trait: std::collections::HashMap<&str, u64> =
        std::collections::HashMap::new();
    for record in &document.digests {
        if let Some(id) = record.trait_id.as_deref() {
            let seq = record.seq.unwrap_or(0);
            let entry = max_seq_by_trait.entry(id).or_insert(0);
            if seq > *entry {
                *entry = seq;
            }
        }
    }
    let superseded_for = |trait_id: Option<&str>, seq: Option<u64>| -> bool {
        match trait_id {
            Some(id) => max_seq_by_trait
                .get(id)
                .is_some_and(|&max_seq| seq.unwrap_or(0) < max_seq),
            None => false,
        }
    };

    document
        .digests
        .iter()
        .map(|record| match &record.trait_id {
            Some(id) => match current_by_id.get(id.as_str()) {
                Some(current_digests) => {
                    let is_current = current_digests.contains(record.digest.as_str());
                    let current_digest = if is_current {
                        record.digest.as_str()
                    } else {
                        current_digests
                            .first()
                            .expect("current trait digest set is non-empty")
                    };
                    TrustReportRow {
                        trait_id: Some(id.clone()),
                        digest: record.digest.clone(),
                        current_digest: Some(current_digest.to_string()),
                        state: record.state,
                        freshness: if is_current {
                            TrustFreshness::Current
                        } else {
                            TrustFreshness::Stale
                        },
                        updated_at: record.updated_at.clone(),
                        reason: record.reason.clone(),
                        seq: record.seq,
                        superseded: superseded_for(Some(id.as_str()), record.seq),
                    }
                }
                None => TrustReportRow {
                    trait_id: Some(id.clone()),
                    digest: record.digest.clone(),
                    current_digest: None,
                    state: record.state,
                    freshness: TrustFreshness::Orphaned,
                    updated_at: record.updated_at.clone(),
                    reason: record.reason.clone(),
                    seq: record.seq,
                    superseded: superseded_for(Some(id.as_str()), record.seq),
                },
            },
            None => TrustReportRow {
                trait_id: None,
                digest: record.digest.clone(),
                current_digest: None,
                state: record.state,
                freshness: if current_digests.contains(record.digest.as_str()) {
                    TrustFreshness::Current
                } else {
                    TrustFreshness::Orphaned
                },
                updated_at: record.updated_at.clone(),
                reason: record.reason.clone(),
                seq: record.seq,
                superseded: false,
            },
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TrustUpdate {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    pub digest: String,
    pub state: TrustState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub seq: u64,
    /// Guard (b): the prior verified record this write's identity lineage
    /// carried before this write, when it named a *different* digest —
    /// derived from the locked read inside the same critical section as the
    /// append, so it can never race a concurrent writer (P534 review
    /// blocker 1). `None` for a digest-only write, a fresh lineage, or a
    /// re-approval of the same digest (nothing was superseded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<SupersededEvidence>,
}

/// Guard (b) evidence: the digest and recorded timestamp of the prior
/// verified record a named approval superseded, read under the store lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SupersededEvidence {
    pub digest: String,
    pub approved_at: Option<String>,
}

/// One digest trust write, as applied by [`update_digests_locked`]. `trait_id`
/// is `Some` for a named `trust approve <trait>`/`trust block <trait>` or a
/// package-member approval, `None` for a raw `--digest` write.
#[derive(Debug, Clone)]
pub struct DigestTrustUpdate {
    pub trait_id: Option<String>,
    pub digest: String,
    pub state: TrustState,
    pub reason: Option<String>,
}

impl DigestTrustUpdate {
    pub fn digest_only(digest: String, state: TrustState, reason: Option<String>) -> Self {
        Self {
            trait_id: None,
            digest,
            state,
            reason,
        }
    }

    pub fn named(
        trait_id: String,
        digest: String,
        state: TrustState,
        reason: Option<String>,
    ) -> Self {
        Self {
            trait_id: Some(trait_id),
            digest,
            state,
            reason,
        }
    }
}

/// Filesystem-derived approval guard for one target, evaluated outside the
/// store lock (guards (a) and (c) — P534 review blocker 1). Never refuses on
/// missing or unreadable lock/git evidence: those degrade to a `warning`,
/// since adhoc/legacy packages and non-repository invocations are legitimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalGuard {
    pub trait_id: String,
    pub digest: String,
    /// Guard (a): the package's own `trait.lock` records a different
    /// canonical digest than the one being approved. Refuses the approval.
    pub refusal: Option<String>,
    /// Guard (c) and the "no lock evidence" case: informational, never
    /// refuses.
    pub warning: Option<String>,
}

impl ApprovalGuard {
    pub fn refused(&self) -> bool {
        self.refusal.is_some()
    }
}

/// Evaluate guards (a)/(c) for one `trait_id`/`digest` pair against the
/// package rooted at `trait_root`. Both checks are best-effort: an
/// unreadable `trait.lock` or a `trait_root` outside a Git working tree
/// never blocks approval, matching this guard's contract that only an
/// actual lock/computed-canonical mismatch refuses.
pub fn evaluate_approval_guard(
    trait_id: &str,
    variant: Option<&str>,
    digest: &str,
    trait_root: &Utf8Path,
) -> ApprovalGuard {
    let mut refusal = None;
    let mut warning = None;
    match crate::lockfile::read_lockfile(trait_root) {
        Ok(Some(lock)) => match lock
            .trait_entry(trait_id, variant)
            .and_then(|entry| entry.canonical_digest())
        {
            Some(locked) if locked != digest => {
                refusal = Some(format!(
                    "trait.lock records a different canonical ({locked}) than the built output \
                     ({digest}); rebuild before approving"
                ));
            }
            Some(_) => {}
            None => {
                warning = Some(
                    "no trait.lock evidence for this trait; approving without lock corroboration"
                        .to_string(),
                );
            }
        },
        Ok(None) => {
            warning = Some(
                "no trait.lock evidence for this trait; approving without lock corroboration"
                    .to_string(),
            );
        }
        // Unreadable lock content is a repair-worthy problem elsewhere, but
        // never silently blocks this approval — the guard only speaks to
        // digest evidence it could actually read.
        Err(_) => {}
    }
    if let Some(dirty) = dirty_working_tree_warning(trait_root) {
        warning = Some(match warning {
            Some(existing) => format!("{existing}; {dirty}"),
            None => dirty,
        });
    }
    ApprovalGuard {
        trait_id: trait_id.to_string(),
        digest: digest.to_string(),
        refusal,
        warning,
    }
}

/// Guard (c): `Some(warning)` when `git status --porcelain` reports
/// uncommitted content under `trait_root`. `None` both when the tree is
/// clean and when `trait_root` is not inside a Git working tree (or `git`
/// itself is unavailable) — outside a repository there is no committed
/// state to warn about drifting from.
fn dirty_working_tree_warning(trait_root: &Utf8Path) -> Option<String> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(trait_root),
        cwd: None,
        args: &["status", "--porcelain", "--", "."],
        success_exit_code: &[0],
        timeout_ms: crate::git_process::PLUMBING_TIMEOUT_MS,
        capture_limit: 1_000_000,
    })
    .ok()?;
    if !output.success || output.stdout.trim().is_empty() {
        return None;
    }
    Some(
        "approving UNCOMMITTED trait content — worktree runs load the committed version, which \
         stays superseded"
            .to_string(),
    )
}

pub fn trust_store_path() -> crate::Result<Utf8PathBuf> {
    Ok(crate::state::global_ctx_root()?.join("trust.toml"))
}

pub fn read_store() -> crate::Result<Document> {
    let path = trust_store_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|source| {
                crate::parse::Error::TomlDecode {
                    context: format!(
                        "decode trust store at {path}; fix or remove {path} to recover"
                    ),
                    source,
                }
                .into()
            })
            .and_then(|document: Document| {
                validate_sequence_evidence(&document)?;
                Ok(document)
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Document::default()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

/// Return store evidence suitable for a durable session receipt. Legacy rows
/// gain their deterministic sequence under the same flock used by all writes.
pub fn read_store_with_sequences() -> crate::Result<Document> {
    let document = read_store()?;
    if document.digests.iter().any(|record| record.seq.is_none()) {
        update_digests_locked(&[])?;
        return read_store();
    }
    Ok(document)
}

/// Single-digest trust update. Delegates to [`update_digests_locked`] so
/// every trust-store mutation — single-digest or package-bulk — shares the
/// same cross-process-locked read-modify-write boundary; no caller writes
/// `trust.toml` outside that lock.
pub fn update_digest(
    digest: &str,
    state: TrustState,
    reason: Option<String>,
) -> crate::Result<TrustUpdate> {
    let mut updates = update_digests_locked(&[DigestTrustUpdate::digest_only(
        digest.to_string(),
        state,
        reason,
    )])?;
    Ok(updates.remove(0))
}

/// Single named-trait trust update (`trust approve <trait>` / `trust block
/// <trait>`): a convenience wrapper over [`update_digests_locked`] so the
/// caller does not have to build a one-element slice by hand.
pub fn update_named_digest(
    trait_id: &str,
    digest: &str,
    state: TrustState,
    reason: Option<String>,
) -> crate::Result<TrustUpdate> {
    let mut updates = update_digests_locked(&[DigestTrustUpdate::named(
        trait_id.to_string(),
        digest.to_string(),
        state,
        reason,
    )])?;
    Ok(updates.remove(0))
}

/// Atomically apply a set of digest trust updates as one cross-process
/// `flock`-serialized read-modify-write (P439: package-granular `trust
/// approve` must not expose a partially approved package, nor lose a
/// concurrent trust change from another `ctx` invocation, either of which a
/// loop of single-digest [`update_digest`] calls could do). Every update
/// shares the same `updated_at` timestamp and, unless overridden per entry,
/// the store is written exactly once regardless of how many digests are
/// supplied.
///
/// Every update appends evidence, including a re-approval of an older digest.
/// Legacy records receive deterministic sequences before new events are added.
pub fn update_digests_locked(updates: &[DigestTrustUpdate]) -> crate::Result<Vec<TrustUpdate>> {
    for update in updates {
        ctx_traits_core::digest::Digest::parse(&update.digest)?;
    }
    let path = trust_store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let lock_path =
        path.with_file_name(format!("{}.lock", path.file_name().unwrap_or("trust.toml")));
    let lock_file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;

    let mut document = read_store()?;
    assign_legacy_sequences(&mut document)?;
    let timestamp = epoch_seconds();
    let mut next_seq = document
        .digests
        .iter()
        .filter_map(|record| record.seq)
        .max()
        .unwrap_or(0);
    // One call is one trust-decision act. Sequences stay unique (the store's own
    // guard requires it), so the act is recorded separately: every record
    // written together carries the same `act`. A native family is the reason —
    // its leaves are one authored package with one canonical digest EACH, all
    // reported under the same trait id. Executable authority remains the latest
    // exact-digest decision; `act` only preserves the grouped history.
    let act = next_seq + 1;
    let mut results = Vec::with_capacity(updates.len());
    for update in updates {
        // Guard (b), derived from the locked read before this write's own
        // append is visible: the prior verified record this write's lineage
        // carried, only when it names a different digest — a re-approval of
        // the same digest supersedes nothing.
        let supersedes = update.trait_id.as_deref().and_then(|trait_id| {
            document
                .verified_record_for_trait(trait_id)
                .and_then(|prior| {
                    (prior.digest != update.digest).then(|| SupersededEvidence {
                        digest: prior.digest.clone(),
                        approved_at: prior.updated_at.clone(),
                    })
                })
        });
        next_seq += 1;
        document.digests.push(TrustRecord {
            digest: update.digest.clone(),
            state: update.state,
            trait_id: update.trait_id.clone(),
            act: Some(act),
            updated_at: Some(timestamp.clone()),
            reason: update.reason.clone(),
            seq: Some(next_seq),
        });
        results.push(TrustUpdate {
            path: path.to_string(),
            trait_id: update.trait_id.clone(),
            digest: update.digest.clone(),
            state: update.state,
            reason: update.reason.clone(),
            seq: next_seq,
            supersedes,
        });
    }
    write_store(&path, &document)?;
    // The lock file's `flock` releases automatically when `lock_file` drops.
    drop(lock_file);
    Ok(results)
}

fn assign_legacy_sequences(document: &mut Document) -> crate::Result<()> {
    let has_sequenced = document.digests.iter().any(|record| record.seq.is_some());
    let has_legacy = document.digests.iter().any(|record| record.seq.is_none());
    // File order alone cannot establish whether an unsequenced event predates
    // a sequenced event. Refuse that ambiguous mixed store rather than minting
    // a sequence that could promote old trust evidence.
    if has_sequenced && has_legacy {
        return Err(crate::Error::Usage {
            message: "trust store mixes sequenced and legacy records; refuse to infer their authority; repair trust.toml so every record has a unique sequence".to_string(),
        });
    }
    let mut next_seq = 1;
    for record in &mut document.digests {
        if record.seq.is_none() {
            record.seq = Some(next_seq);
            next_seq += 1;
        }
    }
    Ok(())
}

/// Validate persisted authority before any consumer derives currency. Legacy
/// stores are accepted only when every row is legacy, so their file order can
/// be migrated deterministically under the write lock. A mixed store has no
/// safe total ordering and is rejected.
fn validate_sequence_evidence(document: &Document) -> crate::Result<()> {
    let has_sequenced = document.digests.iter().any(|record| record.seq.is_some());
    let has_legacy = document.digests.iter().any(|record| record.seq.is_none());
    if has_sequenced && has_legacy {
        return Err(crate::Error::Usage {
            message: "trust store mixes sequenced and legacy records; refuse to infer their authority; repair trust.toml so every record has a unique sequence".to_string(),
        });
    }
    let mut seen = std::collections::HashSet::new();
    for record in &document.digests {
        if let Some(seq) = record.seq
            && (seq == 0 || !seen.insert(seq))
        {
            return Err(crate::Error::Usage {
                message: "trust store has zero or duplicate sequence evidence; repair trust.toml before deriving trust authority".to_string(),
            });
        }
    }
    Ok(())
}

fn write_store(path: &Utf8Path, document: &Document) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let text =
        toml::to_string_pretty(document).map_err(|source| crate::parse::Error::TomlEncode {
            context: format!("encode trust store at {path}"),
            source,
        })?;
    let file_name = path.file_name().unwrap_or("trust.toml");
    let tmp_path = path.with_file_name(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        epoch_seconds()
    ));
    std::fs::write(&tmp_path, text).map_err(|source| crate::environment::Error::Filesystem {
        path: tmp_path.to_string(),
        source,
    })?;
    if let Err(source) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into());
    }
    Ok(())
}

fn epoch_seconds() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TRUST_STORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn record(digest: &str, state: TrustState, seq: Option<u64>) -> TrustRecord {
        TrustRecord {
            digest: digest.to_string(),
            state,
            trait_id: Some("example".to_string()),
            act: None,
            updated_at: None,
            reason: None,
            seq,
        }
    }

    #[test]
    fn start_trust_uses_latest_exact_digest_decision() {
        let mut document = Document {
            digests: vec![
                record("sha256:x", TrustState::Verified, Some(1)),
                record("sha256:y", TrustState::Verified, Some(2)),
                record("sha256:x", TrustState::Verified, Some(3)),
                record("sha256:x", TrustState::Blocked, Some(4)),
                record("sha256:x", TrustState::Verified, Some(5)),
            ],
        };

        assert!(matches!(
            document.start_trust("example", "sha256:y"),
            StartTrust::Verified(record) if record.seq == Some(2)
        ));
        assert!(matches!(
            document.start_trust("example", "sha256:x"),
            StartTrust::Verified(record) if record.seq == Some(5)
        ));

        document.digests.pop();
        assert!(matches!(
            document.start_trust("example", "sha256:x"),
            StartTrust::Blocked(record) if record.seq == Some(4)
        ));

        document.digests.push(TrustRecord {
            digest: "sha256:raw".to_string(),
            state: TrustState::Verified,
            trait_id: None,
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(6),
        });
        assert!(matches!(
            document.start_trust("example", "sha256:raw"),
            StartTrust::Verified(record) if record.seq == Some(6)
        ));
        assert!(matches!(
            document.start_trust("example", "sha256:unseen"),
            StartTrust::Unreviewed
        ));
    }

    #[test]
    fn reporting_prefers_exact_digest_before_identity_history() {
        let mut document = Document {
            digests: vec![
                record("sha256:a", TrustState::Verified, Some(1)),
                record("sha256:b", TrustState::Verified, Some(2)),
            ],
        };
        assert!(matches!(
            document.record_for_current("example", "sha256:a"),
            Some(record) if record.digest == "sha256:a"
        ));
        document
            .digests
            .push(record("sha256:b", TrustState::Blocked, Some(3)));
        assert!(matches!(
            document.record_for_current("example", "sha256:unseen"),
            Some(record) if record.digest == "sha256:b" && record.state == TrustState::Blocked
        ));
    }

    #[test]
    fn unsequenced_exact_evidence_has_one_authoritative_record() {
        let document = Document {
            digests: vec![
                record("sha256:a", TrustState::Verified, None),
                record("sha256:a", TrustState::Blocked, None),
            ],
        };

        assert!(matches!(
            document.exact_record("example", "sha256:a"),
            Some(record) if record.state == TrustState::Blocked
        ));
        assert!(matches!(
            document.record_for_current("example", "sha256:a"),
            Some(record) if record.state == TrustState::Blocked
        ));
        assert!(matches!(
            document.start_trust("example", "sha256:a"),
            StartTrust::Blocked(record) if record.state == TrustState::Blocked
        ));
    }

    #[test]
    fn mixed_legacy_and_sequenced_evidence_is_rejected() {
        let mut document = Document {
            digests: vec![
                record("sha256:old-a", TrustState::Verified, None),
                record("sha256:known", TrustState::Verified, Some(3)),
                record("sha256:old-b", TrustState::Verified, None),
            ],
        };

        assert!(assign_legacy_sequences(&mut document).is_err());
        assert_eq!(document.digests[0].seq, None);
        assert_eq!(document.digests[1].seq, Some(3));
        assert_eq!(document.digests[2].seq, None);
    }

    #[test]
    fn zero_and_duplicate_sequences_are_rejected_before_classification() {
        for records in [
            vec![record("sha256:a", TrustState::Verified, Some(0))],
            vec![
                record("sha256:a", TrustState::Verified, Some(1)),
                record("sha256:b", TrustState::Verified, Some(1)),
            ],
        ] {
            assert!(validate_sequence_evidence(&Document { digests: records }).is_err());
        }
    }

    #[test]
    fn classify_records_marks_only_the_max_sequence_row_as_not_superseded() {
        let document = Document {
            digests: vec![
                record("sha256:x", TrustState::Verified, Some(1)),
                record("sha256:y", TrustState::Verified, Some(2)),
                record("sha256:x", TrustState::Verified, Some(3)),
            ],
        };
        let rows = classify_records(
            &document,
            &[("example".to_string(), "sha256:x".to_string())],
        );
        let by_seq = |seq: u64| rows.iter().find(|row| row.seq == Some(seq)).unwrap();

        assert!(by_seq(1).superseded);
        assert!(by_seq(2).superseded);
        assert!(!by_seq(3).superseded);
        // Current build digest (sha256:x) but re-approved after an
        // intervening decision: still current freshness, still superseded
        // in the lineage — the two are independent axes.
        assert_eq!(by_seq(1).freshness, TrustFreshness::Current);
    }

    #[test]
    fn classify_records_never_marks_digest_only_rows_superseded() {
        let mut document = Document {
            digests: vec![record("sha256:x", TrustState::Verified, Some(1))],
        };
        document.digests[0].trait_id = None;
        document.digests.push(TrustRecord {
            digest: "sha256:x".to_string(),
            state: TrustState::Verified,
            trait_id: None,
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(2),
        });
        let rows = classify_records(&document, &[]);
        assert!(rows.iter().all(|row| !row.superseded));
    }

    #[test]
    fn classify_records_treats_every_current_family_digest_as_current() {
        let document = Document {
            digests: vec![
                record("sha256:default", TrustState::Verified, Some(1)),
                record("sha256:quick", TrustState::Verified, Some(2)),
                record("sha256:old", TrustState::Verified, Some(3)),
            ],
        };
        let rows = classify_records(
            &document,
            &[
                ("example".to_string(), "sha256:default".to_string()),
                ("example".to_string(), "sha256:quick".to_string()),
            ],
        );

        assert_eq!(rows[0].freshness, TrustFreshness::Current);
        assert_eq!(rows[0].current_digest.as_deref(), Some("sha256:default"));
        assert_eq!(rows[1].freshness, TrustFreshness::Current);
        assert_eq!(rows[1].current_digest.as_deref(), Some("sha256:quick"));
        assert_eq!(rows[2].freshness, TrustFreshness::Stale);
    }

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-trust-guard-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    #[test]
    fn update_digests_locked_computes_supersession_only_for_a_different_digest() {
        let _lock = TRUST_STORE_TEST_LOCK.lock().unwrap();
        let dir = scratch_dir("locked-write");
        // Isolate the global config root this process resolves the trust
        // store under — parallel tests mutating this env var would race, so
        // this is deliberately the one test in this module that touches it
        // (P534 review blocker 1's noted env-global test-isolation risk).
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.as_std_path());
        }

        let digest_x = format!("sha256:{}", "a".repeat(64));
        let digest_y = format!("sha256:{}", "b".repeat(64));

        let first = update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_x.clone(),
            TrustState::Verified,
            None,
        )])
        .expect("first write");
        assert!(first[0].supersedes.is_none());

        // Re-approving the same digest supersedes nothing.
        let reapprove = update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_x.clone(),
            TrustState::Verified,
            None,
        )])
        .expect("reapprove write");
        assert!(reapprove[0].supersedes.is_none());

        let second = update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_y,
            TrustState::Verified,
            None,
        )])
        .expect("second write");
        let supersedes = second[0].supersedes.as_ref().expect("supersedes digest_x");
        assert_eq!(supersedes.digest, digest_x);

        unsafe {
            match previous {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn update_after_block_supersedes_prior_verified_digest() {
        let _lock = TRUST_STORE_TEST_LOCK.lock().unwrap();
        let dir = scratch_dir("locked-write-after-block");
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.as_std_path());
        }

        let digest_a = format!("sha256:{}", "a".repeat(64));
        let digest_b = format!("sha256:{}", "b".repeat(64));
        update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_a.clone(),
            TrustState::Verified,
            None,
        )])
        .expect("approve A");
        update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_b.clone(),
            TrustState::Blocked,
            None,
        )])
        .expect("block B");
        let approval = update_digests_locked(&[DigestTrustUpdate::named(
            "example".to_string(),
            digest_b,
            TrustState::Verified,
            None,
        )])
        .expect("approve B");
        assert_eq!(
            approval[0]
                .supersedes
                .as_ref()
                .map(|evidence| &evidence.digest),
            Some(&digest_a),
            "a block is not approval evidence that a later approval supersedes"
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn evaluate_approval_guard_refuses_on_lock_canonical_mismatch() {
        let root = scratch_dir("lock-mismatch");
        std::fs::write(
            crate::layout::package_lock_path(root.as_ref()).as_std_path(),
            "[[trait]]\nid = \"example\"\n\n[trait.digests]\ncanonical = \"sha256:locked\"\n",
        )
        .expect("write trait.lock");

        let mismatch = evaluate_approval_guard("example", None, "sha256:computed", &root);
        assert!(mismatch.refused());
        assert!(
            mismatch
                .refusal
                .as_deref()
                .unwrap()
                .contains("sha256:locked")
        );

        let matched = evaluate_approval_guard("example", None, "sha256:locked", &root);
        assert!(!matched.refused());
    }

    #[test]
    fn evaluate_approval_guard_warns_without_refusing_when_lock_is_missing() {
        let root = scratch_dir("lock-missing");

        let guard = evaluate_approval_guard("example", None, "sha256:computed", &root);
        assert!(!guard.refused());
        assert!(
            guard
                .warning
                .as_deref()
                .unwrap()
                .contains("no trait.lock evidence")
        );
    }

    #[test]
    fn evaluate_approval_guard_selects_the_requested_family_variant() {
        let root = scratch_dir("family-lock");
        std::fs::write(
            crate::layout::package_lock_path(root.as_ref()).as_std_path(),
            "[[trait]]\nid = \"example\"\nvariant = \"default\"\n[trait.digests]\ncanonical = \"sha256:default\"\n\n[[trait]]\nid = \"example\"\nvariant = \"quick\"\n[trait.digests]\ncanonical = \"sha256:quick\"\n",
        )
        .expect("write family trait.lock");

        assert!(
            !evaluate_approval_guard("example", Some("quick"), "sha256:quick", &root).refused()
        );
        assert!(
            evaluate_approval_guard("example", Some("quick"), "sha256:changed", &root).refused()
        );
    }
}
