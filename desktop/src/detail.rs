//! Run-detail state for a selected center row: plain Rust, no gpui types, so
//! it is unit-testable without an `App` — the same pattern `dashboard.rs`
//! and `run_row.rs` document.
//!
//! # Resolution
//!
//! A selection resolves to `RunRow::ledger_path` — the center's own primary
//! key for a run, the absolute path the center already resolved when it
//! indexed that ledger. This is deliberately **not** re-derived from
//! `ctx_traits_io::state::global_runs_root(&row.repo_key)` plus
//! `run_session::session_store_path(..)`: the center's runs root is
//! env-overridable (`CTX_CENTER_RUNS_ROOT`), so that derivation diverges from
//! the row's real `ledger_path` whenever the center is not serving out of the
//! global runs root — including in this crate's own integration tests. It
//! would also re-perform, from a second source, a resolution the center
//! already performed and shipped on the wire, which is exactly the kind of
//! GUI-private index this task forbids. `repo_key` still travels alongside
//! the ledger path, as part of the selection key and of failure text, so two
//! same-named runs in different repositories stay distinct at this layer too.
//!
//! The one read this module performs goes through
//! `ctx_traits_io::run_session::read_run_session`, the existing IO store
//! boundary (symlink rejection, typed parse errors) — never a hand-rolled
//! `std::fs::read_to_string` + `serde_json` in this crate.

use std::collections::VecDeque;

use camino::Utf8PathBuf;
use ctx_traits_core::procedure::session::Session;
use ctx_traits_io::activity_sidecar::ActivityRecord;
use ctx_traits_io::center::{CenterDelta, CenterPublicRow};
use ctx_traits_io::run_summary::RunSummary;

use crate::center_link::LinkUpdate;
use crate::detail_tree::{self, ActivityOverlay, DetailTree};
use crate::run_row::RunRow;

/// How many activity records a `Loading`/resync-in-flight selection buffers
/// before it starts dropping the oldest. The overlay is last-write-wins per
/// `frame_id`, so the oldest buffered record is precisely the one whose loss
/// is invisible once a newer record for the same frame lands.
const PENDING_CAP: usize = 256;

/// A background read request for one selection. Carries the generation it
/// was issued under, so a caller can tell a stale outcome from the current
/// one without any extra bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    pub generation: u64,
    pub repo_key: String,
    pub ledger_path: Utf8PathBuf,
    pub session_id: String,
    /// The exact projected `RunRow` this request's read is being issued
    /// for — carried through to `DetailBaseline::row` unchanged, so the
    /// facts a resync commits (run identity, elapsed, start time,
    /// readability) can never straddle two different center rows
    /// (review-verdict-1 blocker `selected-preview-not-atomic`).
    pub row: RunRow,
}

/// The authoritative session plus its tolerant activity embellishment,
/// already folded into a bounded [`ActivityOverlay`] — the raw sidecar
/// records themselves are never retained past `load`, so a long run's
/// thousands-of-records sidecar does not stay resident on the UI thread. The
/// sidecar read has no failure mode that reaches the caller (see
/// `ctx_traits_io::activity_sidecar::read_activity`), so a bad or absent
/// sidecar can never turn a good session into [`DetailLoad::Failed`].
#[derive(Debug, Clone, PartialEq)]
pub struct DetailBaseline {
    pub session: Session,
    pub activity_overlay: ActivityOverlay,
    pub skipped_activity_lines: usize,
    /// The authoritative variant, reconstructed from the pinned trait
    /// source: `Ok(Some)` served, `Ok(None)` reconstructed with no native
    /// `variant`, `Err` reconstruction refused (digest/identity mismatch).
    /// Never fails the detail — a variant that cannot be resolved renders
    /// its own loud segment rather than turning a readable run unreadable.
    pub variant: Result<Option<String>, String>,
    /// The served claimed-task answer. `Err` is the transport/center
    /// failure; `Ok` still carries 0265.2's four typed outcomes.
    pub claimed_task: Result<ctx_traits_io::center::ClaimedTaskResult, String>,
    /// N/M for the selected run, derived once in `load` from the pinned
    /// trait's resolved plan joined to this session (0265.14). `Err` is the
    /// same resolution refusal as `variant` (source moved, digest mismatch,
    /// foreign repository) — a typed absence, never a number assembled from
    /// the row.
    pub progress: Result<ctx_traits_core::procedure::run::RunProgress, String>,
    /// The exact `RunRow` this baseline's `LoadRequest` was issued for —
    /// copied verbatim from `LoadRequest::row`, never re-derived. This is
    /// what the preview composer reads identity/elapsed/start-time/
    /// readability from, so those facts commit atomically with `variant`
    /// and `claimed_task` under the same generation-checked `apply`.
    pub row: RunRow,
}

/// The one filesystem read this module performs, plus the tolerant sidecar
/// read alongside it. Read-only; the ledger's io error is converted to
/// `String` here, inside the (future) background task, so the value
/// crossing back to the UI thread is plainly `Send`. The sidecar's records
/// are folded into an [`ActivityOverlay`] here, once, rather than carried as
/// a raw `Vec` and re-folded on every projection.
pub fn load(request: &LoadRequest) -> Result<DetailBaseline, String> {
    let session = ctx_traits_io::run_session::read_run_session(&request.ledger_path)
        .map_err(|error| error.to_string())?;
    let (activity, skipped_activity_lines) =
        ctx_traits_io::activity_sidecar::read_activity(&request.ledger_path);
    let (variant, progress) = load_variant_and_progress(&session);
    let claimed_task =
        ctx_traits_io::center::claimed_task_existing(&request.session_id, Some(&request.repo_key))
            .map_err(|error| error.to_string());
    Ok(DetailBaseline {
        session,
        activity_overlay: ActivityOverlay::from_records(&activity),
        skipped_activity_lines,
        variant,
        claimed_task,
        progress,
        row: request.row.clone(),
    })
}

/// The authoritative variant — native `Trait.variant` over legacy
/// `Metadata.variant` (documented display-only) — and the 0265.14 frame
/// counter, both from one `load_trait_for_session` resolution rather than
/// two: the desktop already resolved the trait once for `variant` alone;
/// folding `plan_procedure_run` into that same resolution avoids doubling a
/// digest-verified reconstruction per resync (`0257.3`'s advance mechanism
/// calls `load` roughly once per landed frame). Mirrors the CLI's
/// `reconstruct_projection` (`dashboard.rs`), so both faces resolve
/// identically.
fn load_variant_and_progress(
    session: &Session,
) -> (
    Result<Option<String>, String>,
    Result<ctx_traits_core::procedure::run::RunProgress, String>,
) {
    let loaded = match ctx_traits_io::run::load_trait_for_session(None, None, session, "preview") {
        Ok(loaded) => loaded,
        Err(error) => {
            let message = error.to_string();
            return (Err(message.clone()), Err(message));
        }
    };
    let variant = Ok(loaded.trait_ref.variant.clone().or_else(|| {
        loaded
            .trait_ref
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.variant.clone())
            .map(|variant| variant.as_str().to_string())
    }));
    let progress = ctx_traits_core::procedure::run::plan_procedure_run(
        &loaded.trait_ref,
        session.run_id.clone(),
    )
    .map(|plan| ctx_traits_core::procedure::run::run_progress_for(&plan, session))
    .map_err(|error| error.to_string());
    (variant, progress)
}

/// A selected run's load state. `DetailBaseline` is boxed because it is
/// large enough to trip `clippy::large_enum_variant` otherwise.
#[derive(Debug, Clone, PartialEq)]
pub enum DetailLoad {
    Loading,
    Loaded(Box<DetailBaseline>),
    Failed(String),
}

/// The preview column's one honest view of the current selection: pending,
/// failed, or an atomically-accepted baseline, staleness marked explicitly
/// rather than inferred from a second source.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewState<'a> {
    Loading,
    Failed(&'a str),
    Accepted {
        baseline: &'a DetailBaseline,
        stale: Option<&'a str>,
        /// A resync (fingerprint move, or recovery from `Stale`) has been
        /// issued and has not yet landed via `apply`. `baseline` is still
        /// the last-accepted facts, not the pending replacement's — this
        /// flag is what stops a same-selection resync from being painted
        /// identically to a settled, current baseline (review-verdict-1
        /// blocker `selected-preview-not-atomic`).
        refreshing: bool,
        /// The selection's current liveness, refreshed from every
        /// `RowChanged`/`Appeared`/`Ended` delta while following — distinct
        /// from `baseline.row.live`, which is frozen at the load this
        /// baseline was accepted from. A fingerprint-identical liveness flip
        /// (e.g. a run ending with no other summary change) moves this flag
        /// without a resync, so the `in progress` block's presence follows
        /// the current center posture rather than the last-loaded snapshot.
        live: bool,
        /// The selection's current raw session title, refreshed from every
        /// `RowChanged`/`Appeared`/recovery `Snapshot` delta — distinct from
        /// `baseline.row.session_title`, which is frozen at the load this
        /// baseline was accepted from. `Fingerprint` deliberately clears
        /// `summary.title` before comparing (see [`Fingerprint::from_row`]),
        /// so a `RowChanged` right after a live `SessionTitle` sidecar line
        /// never triggers a resync; this field is what lets that rewritten
        /// title still reach the header without one.
        session_title: Option<&'a str>,
    },
}

/// Whether a selection is still tracking the live center stream. `Ended`
/// gets its own wording inside `Stale` rather than a third variant — two
/// states is enough to be honest about "current vs not".
#[derive(Debug, Clone, PartialEq)]
pub enum FollowState {
    Following,
    Stale { reason: String },
}

/// The ledger evidence a loaded-or-requested baseline corresponds to:
/// `(row.modified_epoch_secs, row.summary with title cleared)`. `title` is
/// excluded because the center rewrites it from a sidecar `SessionTitle`
/// line the overlay already carries, immediately followed by a
/// `RowChanged` that must not itself cost a ledger read. `modified_epoch_secs`
/// is 1-second resolution, so two ledger writes inside one second that leave
/// every other summary fact identical collapse to a single re-read — bounded
/// and self-healing on the next write.
#[derive(Debug, Clone, PartialEq)]
struct Fingerprint {
    modified_epoch_secs: u64,
    summary: RunSummary,
}

impl Fingerprint {
    fn from_row(row: &CenterPublicRow) -> Self {
        let mut summary = row.summary.clone();
        summary.title = None;
        Self {
            modified_epoch_secs: row.modified_epoch_secs,
            summary,
        }
    }
}

/// What a `follow` call yielded: whether the visible state changed, and an
/// authoritative re-read the caller must run on a background executor, if
/// one was warranted.
#[derive(Debug, Default)]
pub struct FollowOutcome {
    pub changed: bool,
    pub request: Option<LoadRequest>,
}

#[derive(Debug, Clone, PartialEq)]
struct Selection {
    key: String,
    repo_key: String,
    session_id: String,
    generation: u64,
    /// The row's kernel-backed liveness flag. Refreshed from every
    /// `RowChanged`/`Appeared`/`Ended` delta while `Following` — no longer a
    /// point-in-time capture once `follow` starts observing the stream.
    live: bool,
    load: DetailLoad,
    follow: FollowState,
    /// The `modified_epoch_secs` carried by the `RunRow` this selection was
    /// made from — retained so a successful *first* load can derive its own
    /// fingerprint (see `apply`) without waiting for a delta to supply one.
    modified_epoch_secs: u64,
    fingerprint: Option<Fingerprint>,
    /// `Some(generation)` while a load (seed or resync) issued under that
    /// generation has not yet settled via `apply`. Distinct from `load`,
    /// which stays `Loaded` with the previous baseline during a resync so
    /// the display never flashes back to `Loading` — this is the flag that
    /// tells `follow_delta` an activity record must also be captured into
    /// `pending`, not just applied to the (about to be replaced) displayed
    /// overlay.
    in_flight: Option<u64>,
    /// Activity records that arrived while a read (seed or resync) was in
    /// flight, replayed onto the fresh overlay once it lands.
    pending: VecDeque<ActivityRecord>,
    /// The most recently projected `RunRow` for this selection — the
    /// candidate a resync's `LoadRequest` carries, and what lands in the
    /// next accepted `DetailBaseline::row`. Updated by `select_inner` and
    /// by every `RowChanged`/`Appeared`/recovery `Snapshot` that reprojects
    /// this selection's row, never by the read outcome itself.
    row: RunRow,
    /// The selection's current raw session title — a delta-refreshed,
    /// fingerprint-exempt fact, mirroring `live`'s threading exactly.
    /// Refreshed from `row.summary.title` by `follow_delta`'s
    /// `RowChanged`/`Appeared` arm (before the fingerprint comparison) and
    /// by `follow_snapshot`'s recovery, never by the read outcome itself.
    session_title: Option<String>,
}

impl Selection {
    fn resync_request(&self, generation: u64) -> LoadRequest {
        LoadRequest {
            generation,
            repo_key: self.repo_key.clone(),
            ledger_path: Utf8PathBuf::from(self.key.clone()),
            session_id: self.session_id.clone(),
            row: self.row.clone(),
        }
    }
}

/// The desktop's run-detail state. A sibling of `CenterFace`/`Dashboard`, not
/// a member of either: a center reconnect installs a fresh `Dashboard`
/// snapshot but leaves the current selection in place — `select` still holds
/// its "exactly once" rule across reconnects — while every `LinkUpdate` also
/// reaches `follow`, which is how a selection advances and recovers without
/// polling or a second ledger read per delta.
#[derive(Default)]
pub struct RunDetail {
    selected: Option<Selection>,
    generation: u64,
}

impl RunDetail {
    /// Select `row`, returning a [`LoadRequest`] the caller must run on a
    /// background executor, or `None` when no read is needed.
    ///
    /// Re-selecting the run that is already the current selection is a
    /// no-op in every state (`Loading`, `Loaded`, `Failed`) — this is the
    /// "exactly once" rule. There is no retry-on-reclick: retry/refresh is
    /// 0257.3's live-follow concern, and a silent second full-ledger read is
    /// precisely what this task's contract forbids.
    ///
    /// A row with an empty `ledger_path` is defensive-only (the center
    /// always populates it): it installs a `Failed` selection and performs
    /// no read, since there is no path to read from.
    pub fn select(&mut self, row: &RunRow) -> Option<LoadRequest> {
        self.select_inner(row, None)
    }

    /// As [`RunDetail::select`], but the selection starts `Stale` with
    /// `reason` rather than `Following` — used when the row being selected
    /// was served from an already-`Stale` `CenterFace` (a retained row from
    /// before an outage). Without this, a selection made during an outage
    /// would appear current and would ignore the eventual recovery
    /// `Snapshot`, so a change made while disconnected could never resync.
    pub fn select_stale(&mut self, row: &RunRow, reason: String) -> Option<LoadRequest> {
        self.select_inner(row, Some(reason))
    }

    fn select_inner(&mut self, row: &RunRow, stale_reason: Option<String>) -> Option<LoadRequest> {
        let follow = match stale_reason {
            Some(reason) => FollowState::Stale { reason },
            None => FollowState::Following,
        };
        if row.ledger_path.is_empty() {
            self.selected = Some(Selection {
                key: String::new(),
                repo_key: row.repo_key.clone(),
                session_id: row.session_id.clone(),
                generation: self.generation,
                live: row.live,
                load: DetailLoad::Failed("run row carries no ledger path".to_string()),
                follow,
                modified_epoch_secs: row.modified_epoch_secs,
                fingerprint: None,
                in_flight: None,
                pending: VecDeque::new(),
                row: row.clone(),
                session_title: row.session_title.clone(),
            });
            return None;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|selection| selection.key == row.ledger_path)
        {
            return None;
        }
        self.generation += 1;
        let generation = self.generation;
        self.selected = Some(Selection {
            key: row.ledger_path.clone(),
            repo_key: row.repo_key.clone(),
            session_id: row.session_id.clone(),
            generation,
            live: row.live,
            load: DetailLoad::Loading,
            follow,
            modified_epoch_secs: row.modified_epoch_secs,
            // `None` rather than derived from `row`: `RunRow` is a flattened
            // projection with no retained `RunSummary`. `apply` derives the
            // real fingerprint from the loaded `Session` the moment the seed
            // read lands (see below), so this is only ever observed by a
            // delta that races the seed read itself.
            fingerprint: None,
            in_flight: Some(generation),
            pending: VecDeque::new(),
            row: row.clone(),
            session_title: row.session_title.clone(),
        });
        Some(LoadRequest {
            generation,
            repo_key: row.repo_key.clone(),
            ledger_path: Utf8PathBuf::from(row.ledger_path.clone()),
            session_id: row.session_id.clone(),
            row: row.clone(),
        })
    }

    /// Apply a background load's outcome. Returns whether the visible state
    /// actually changed, so the caller can `cx.notify()` only on real
    /// change — the same discipline `Dashboard::apply` established in
    /// 0256.4.
    ///
    /// An outcome tagged with a superseded generation (a stale selection's
    /// result landing late) is ignored.
    ///
    /// A resync's failure (the selection was already `Loaded` when the read
    /// was issued) keeps the previous baseline on screen and marks the
    /// selection `Stale` instead of clobbering good state with `Failed` — a
    /// *first* load's failure still lands as `Failed`, since a fresh
    /// selection is never `Loaded` when its read is issued.
    ///
    /// A successful load that lands with no fingerprint yet on record (the
    /// seed read, or a delta-free race with the seed read) establishes one
    /// from the loaded `Session` itself, so the *next* delta — even the very
    /// first one this selection ever observes — can tell "nothing changed"
    /// from "structure moved" without an unconditional confirming re-read.
    /// A fingerprint already on record (set by a delta that raced ahead of
    /// this same read, or carried over from a prior generation) is left
    /// alone; the generation check above already ensures this outcome
    /// belongs to the current selection, so there is nothing newer to
    /// preserve against.
    pub fn apply(&mut self, generation: u64, outcome: Result<DetailBaseline, String>) -> bool {
        let Some(selection) = self.selected.as_mut() else {
            return false;
        };
        if selection.generation != generation {
            return false;
        }
        selection.in_flight = None;
        match outcome {
            Ok(mut baseline) => {
                // `apply_pending_record`, not `apply_live_record`: a pending
                // record buffered while this seed/resync read was in flight
                // may be a redelivery of a record the seed's own historical
                // fold already folded (a seed/pending overlap), which
                // `apply_pending_record` reconciles by bounded
                // record-occurrence accounting before feeding a genuinely
                // new record through the ordinary span fold — see its doc
                // comment and review-verdict-1 blocker
                // `live-span-reopen-divergence`.
                for record in selection.pending.drain(..) {
                    baseline.activity_overlay.apply_pending_record(&record);
                }
                // Reconciliation is a one-shot event against this seed's own
                // historical fold; the index it uses must not survive into
                // the accepted, retained overlay (review-verdict-1 blocker
                // `live-span-reopen-divergence`).
                baseline.activity_overlay.clear_seed_occurrences();
                if selection.fingerprint.is_none() {
                    let mut summary = RunSummary::from_session(&baseline.session);
                    summary.title = None;
                    selection.fingerprint = Some(Fingerprint {
                        modified_epoch_secs: selection.modified_epoch_secs,
                        summary,
                    });
                }
                selection.load = DetailLoad::Loaded(Box::new(baseline));
            }
            Err(reason) => {
                if matches!(selection.load, DetailLoad::Loaded(_)) {
                    selection.follow = FollowState::Stale { reason };
                } else {
                    selection.load = DetailLoad::Failed(reason);
                }
            }
        }
        true
    }

    /// Fold one `LinkUpdate` into the current selection, mirroring
    /// `CenterFace::apply`'s shape. Takes `update` by reference so `Shell`
    /// can feed detail first and then hand the owned update to the face
    /// unchanged, preserving stream order exactly.
    ///
    /// A `None` selection, or a delta for a different `ledger_path`, is a
    /// no-op — checked before anything is cloned.
    pub fn follow(&mut self, update: &LinkUpdate) -> FollowOutcome {
        match update {
            LinkUpdate::Snapshot(rows) => self.follow_snapshot(rows),
            LinkUpdate::Delta(delta) => self.follow_delta(delta),
            LinkUpdate::Down(reason) => self.follow_down(reason),
        }
    }

    fn follow_down(&mut self, reason: &str) -> FollowOutcome {
        let Some(selection) = self.selected.as_mut() else {
            return FollowOutcome::default();
        };
        let stale = FollowState::Stale {
            reason: reason.to_string(),
        };
        let changed = selection.follow != stale;
        selection.follow = stale;
        FollowOutcome {
            changed,
            request: None,
        }
    }

    fn follow_delta(&mut self, delta: &CenterDelta) -> FollowOutcome {
        let Some(selection) = self.selected.as_mut() else {
            return FollowOutcome::default();
        };
        if selection.key != delta.ledger_path() {
            return FollowOutcome::default();
        }
        // Stale is an absorbing state until a recovery `Snapshot` explicitly
        // closes it (`follow_snapshot`): every delta variant is a no-op
        // while stale, not just activity/row-change. Checked once, ahead of
        // the match, so a variant added later inherits the guard instead of
        // needing its own copy.
        if !matches!(selection.follow, FollowState::Following) {
            return FollowOutcome::default();
        }
        match delta {
            CenterDelta::ActivityLine { activity, .. } => {
                // The displayed baseline (if any) gets the record right
                // away, for immediate feedback. Independently, while a load
                // is in flight (seed or resync — `load` staying `Loaded`
                // during a resync must not be mistaken for "settled"), the
                // record is also captured into `pending` so it survives the
                // wholesale baseline replacement `apply` performs when that
                // load lands, even if the record post-dates the read.
                let mut changed = false;
                if let DetailLoad::Loaded(baseline) = &mut selection.load {
                    changed = baseline.activity_overlay.apply_live_record(activity);
                }
                if selection.in_flight.is_some() {
                    if selection.pending.len() >= PENDING_CAP {
                        selection.pending.pop_front();
                    }
                    selection.pending.push_back(activity.clone());
                }
                FollowOutcome {
                    changed,
                    request: None,
                }
            }
            CenterDelta::RowChanged { row } | CenterDelta::Appeared { row } => {
                let mut changed = false;
                if selection.live != row.live {
                    selection.live = row.live;
                    changed = true;
                }
                // Refreshed before the fingerprint comparison: the
                // fingerprint deliberately clears `summary.title` (see
                // `Fingerprint::from_row`), so a title-only rewrite must
                // still move this field even on the no-resync branch below.
                let fresh_title = row.summary.title.clone().filter(|title| !title.is_empty());
                if selection.session_title != fresh_title {
                    selection.session_title = fresh_title;
                    changed = true;
                }
                let fingerprint = Fingerprint::from_row(row);
                if selection.fingerprint.as_ref() != Some(&fingerprint) {
                    selection.fingerprint = Some(fingerprint);
                    selection.row = crate::run_row::project_one(row);
                    self.generation += 1;
                    selection.generation = self.generation;
                    selection.in_flight = Some(self.generation);
                    return FollowOutcome {
                        changed: true,
                        request: Some(selection.resync_request(self.generation)),
                    };
                }
                FollowOutcome {
                    changed,
                    request: None,
                }
            }
            CenterDelta::Ended { .. } => {
                let mut changed = false;
                if selection.live {
                    selection.live = false;
                    changed = true;
                }
                let stale = FollowState::Stale {
                    reason: "run no longer indexed by the center".to_string(),
                };
                if selection.follow != stale {
                    selection.follow = stale;
                    changed = true;
                }
                FollowOutcome {
                    changed,
                    request: None,
                }
            }
        }
    }

    /// The recovery edge: a `Stale` selection returns to `Following` and
    /// issues exactly one resync, whether or not the recovery snapshot still
    /// carries a matching row — the ledger is authoritative either way. An
    /// already-`Following` selection (the first snapshot, or a duplicate)
    /// issues nothing.
    fn follow_snapshot(&mut self, rows: &[CenterPublicRow]) -> FollowOutcome {
        let Some(selection) = self.selected.as_mut() else {
            return FollowOutcome::default();
        };
        if !matches!(selection.follow, FollowState::Stale { .. }) {
            return FollowOutcome::default();
        }
        if let Some(row) = rows.iter().find(|row| row.ledger_path == selection.key) {
            selection.live = row.live;
            selection.fingerprint = Some(Fingerprint::from_row(row));
            selection.row = crate::run_row::project_one(row);
            selection.session_title = row.summary.title.clone().filter(|title| !title.is_empty());
        }
        selection.follow = FollowState::Following;
        self.generation += 1;
        selection.generation = self.generation;
        selection.in_flight = Some(self.generation);
        FollowOutcome {
            changed: true,
            request: Some(selection.resync_request(self.generation)),
        }
    }

    /// The current selection's key (`ledger_path`), if any.
    pub fn selected_key(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|selection| selection.key.as_str())
    }

    /// The current selection's repository key, if any.
    pub fn repo_key(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|selection| selection.repo_key.as_str())
    }

    /// The current selection's load state, if any.
    pub fn load_state(&self) -> Option<&DetailLoad> {
        self.selected.as_ref().map(|selection| &selection.load)
    }

    /// The current selection's live-follow state, if any.
    pub fn follow_state(&self) -> Option<&FollowState> {
        self.selected.as_ref().map(|selection| &selection.follow)
    }

    /// The one preview-safe projection of the current selection: loading,
    /// failed, or an accepted `DetailBaseline` (optionally marked stale).
    /// Every field the Sessions preview column renders — trait/variant,
    /// run id/elapsed, task answer, footer — comes out of the same
    /// `DetailBaseline` here, which is only ever installed by a
    /// generation-checked `apply`. There is deliberately no path that lets a
    /// caller pair this baseline with a `RunRow` read fresh from
    /// `CenterFace` — that pairing is exactly what let a resync's new
    /// run/elapsed sit beside a stale variant/task (review-verdict-1 blocker
    /// `selected-preview-not-atomic`).
    pub fn preview_state(&self) -> Option<PreviewState<'_>> {
        let selection = self.selected.as_ref()?;
        match &selection.load {
            DetailLoad::Loading => Some(PreviewState::Loading),
            DetailLoad::Failed(reason) => Some(PreviewState::Failed(reason)),
            DetailLoad::Loaded(baseline) => {
                let stale = match &selection.follow {
                    FollowState::Stale { reason } => Some(reason.as_str()),
                    FollowState::Following => None,
                };
                let refreshing = selection.in_flight.is_some();
                Some(PreviewState::Accepted {
                    baseline,
                    stale,
                    refreshing,
                    live: selection.live,
                    session_title: selection.session_title.as_deref(),
                })
            }
        }
    }

    /// Project the current selection's loaded baseline into a
    /// [`DetailTree`], or `None` when there is no selection or it has not
    /// finished loading (`Loading`/`Failed`). `load` already folded the raw
    /// sidecar into `baseline.activity_overlay` once; this reprojects the
    /// tree structure from the retained session and that bounded overlay on
    /// each call rather than caching the tree, which is cheap — the overlay
    /// fold itself, the part that scales with sidecar size, does not repeat.
    pub fn tree(&self) -> Option<DetailTree> {
        let selection = self.selected.as_ref()?;
        let DetailLoad::Loaded(baseline) = &selection.load else {
            return None;
        };
        Some(detail_tree::project(
            &baseline.session,
            &baseline.activity_overlay,
            selection.live,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(repo_key: &str, ledger_path: &str, run_id: &str) -> RunRow {
        RunRow {
            ledger_path: ledger_path.to_string(),
            session_id: format!("{run_id}-session"),
            run_id: run_id.to_string(),
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            repo_label: repo_key.to_string(),
            title: run_id.to_string(),
            session_title: None,
            trait_id: "fixture-trait".to_string(),
            state: crate::run_row::RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 0,
            started_at_epoch: None,
        }
    }

    /// Built from the same minimal JSON shape `tests/live_deltas.rs` uses,
    /// via `serde_json`, rather than a hand-written `Session` literal: the
    /// struct carries far more fields than this test needs to name, and
    /// `#[serde(default, ..)]` already covers all of them.
    fn session(run_id: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": format!("{run_id}-session"),
            "run-id": run_id,
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "detail-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": run_id,
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "completed",
            },
            "state-digest": format!("sha256:fixture-{run_id}"),
        }))
        .expect("fixture session")
    }

    fn baseline(session: Session) -> DetailBaseline {
        let run_id = session.run_id.as_str().to_string();
        let fixture_row = row("repo", &format!("/repo/{run_id}.json"), &run_id);
        DetailBaseline {
            session,
            activity_overlay: ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant: Ok(None),
            claimed_task: Ok(ctx_traits_io::center::ClaimedTaskResult::Unclaimed),
            progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            row: fixture_row,
        }
    }

    /// A session whose sole `SequenceStatus` is also its current frame, so a
    /// live activity/narration record attached to `frame_id` is visible on
    /// `tree().roots[0]` for assertion.
    fn session_with_active_frame(run_id: &str, frame_id: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": format!("{run_id}-session"),
            "run-id": run_id,
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "awaiting-agent-output",
            "provenance": {
                "started-by": {"surface": "test", "caller": "detail-fixture"},
                "state-source": "test",
            },
            "active-path": [{"kind": "procedure", "id": frame_id, "index": 0}],
            "ledger": {
                "run-id": run_id,
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "running",
                "sequence-statuses": [{
                    "sequence-index": 0,
                    "run-index": 0,
                    "item-id": frame_id,
                    "title": frame_id,
                    "status": "ready",
                    "reason": "",
                    "position-path": [],
                }],
            },
            "state-digest": format!("sha256:fixture-{run_id}"),
        }))
        .expect("fixture session")
    }

    fn wire_row(
        repo_key: &str,
        ledger_path: &str,
        run_id: &str,
        live: bool,
        modified_epoch_secs: u64,
    ) -> CenterPublicRow {
        CenterPublicRow {
            summary: RunSummary {
                run_id: run_id.to_string(),
                ..RunSummary::unreadable(run_id.to_string(), "fixture".to_string())
            },
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            ledger_path: ledger_path.to_string(),
            live,
            modified_epoch_secs,
        }
    }

    /// A wire row whose `summary` is derived from `session` the same way
    /// `apply`'s just-established fingerprint is — so a delta built from
    /// this row against an already-`Loaded` `session` is genuinely
    /// "nothing changed", not merely two independently hand-built
    /// `RunSummary`s that happen to differ.
    fn matching_wire_row(
        session: &Session,
        repo_key: &str,
        ledger_path: &str,
        live: bool,
        modified_epoch_secs: u64,
    ) -> CenterPublicRow {
        CenterPublicRow {
            summary: RunSummary::from_session(session),
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            ledger_path: ledger_path.to_string(),
            live,
            modified_epoch_secs,
        }
    }

    fn narration(at_epoch_ms: u64, frame_id: &str, text: &str) -> ActivityRecord {
        ActivityRecord::Narration {
            at_epoch_ms,
            frame_id: frame_id.to_string(),
            text: text.to_string(),
        }
    }

    fn activity(at_epoch_ms: u64, frame_id: &str, sequence: u64) -> ActivityRecord {
        use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
        ActivityRecord::Activity {
            at_epoch_ms,
            event: ActivityEvent {
                sequence,
                frame_id: frame_id.to_string(),
                kind: ActivityKind::Thinking,
                text: None,
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        }
    }

    fn activity_delta(row: &CenterPublicRow, record: ActivityRecord) -> LinkUpdate {
        LinkUpdate::Delta(CenterDelta::ActivityLine {
            row: Box::new(row.clone()),
            activity: record,
        })
    }

    #[test]
    fn select_yields_a_request_and_performs_no_io() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .expect("first selection issues a request");
        assert_eq!(request.repo_key, "repo-a");
        assert_eq!(request.ledger_path, Utf8PathBuf::from("/repo-a/run.json"));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loading)));
    }

    #[test]
    fn reselecting_the_same_ledger_path_is_a_no_op_in_every_state() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(
            detail
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );

        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));
        assert!(
            detail
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );

        let mut failed = RunDetail::default();
        let failed_request = failed
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(failed.apply(failed_request.generation, Err("broken".to_string())));
        assert!(
            failed
                .select(&row("repo-a", "/repo-a/run.json", "run-a"))
                .is_none()
        );
    }

    #[test]
    fn distinct_repositories_with_identical_run_ids_are_distinct_selections() {
        let mut detail = RunDetail::default();
        let first = detail
            .select(&row("repo-a", "/repo-a/run.json", "shared-run"))
            .unwrap();
        let second = detail
            .select(&row("repo-b", "/repo-b/run.json", "shared-run"))
            .unwrap();
        assert_ne!(first.ledger_path, second.ledger_path);
        assert_ne!(first.generation, second.generation);
    }

    #[test]
    fn a_superseded_generation_outcome_is_ignored() {
        let mut detail = RunDetail::default();
        let first = detail
            .select(&row("repo-a", "/repo-a/run-a.json", "run-a"))
            .unwrap();
        let second = detail
            .select(&row("repo-a", "/repo-a/run-b.json", "run-b"))
            .unwrap();

        assert!(!detail.apply(first.generation, Ok(baseline(session("run-a")))));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loading)));

        assert!(detail.apply(second.generation, Ok(baseline(session("run-b")))));
        assert!(matches!(detail.load_state(), Some(DetailLoad::Loaded(_))));
    }

    #[test]
    fn an_error_outcome_lands_as_failed_never_loaded() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Err("bad json".to_string())));
        assert!(
            matches!(detail.load_state(), Some(DetailLoad::Failed(reason)) if reason == "bad json")
        );
    }

    #[test]
    fn an_empty_ledger_path_selects_into_failed_with_no_request() {
        let mut detail = RunDetail::default();
        let request = detail.select(&row("repo-a", "", "run-a"));
        assert!(request.is_none());
        assert!(matches!(detail.load_state(), Some(DetailLoad::Failed(_))));
    }

    #[test]
    fn tree_is_none_until_loaded_then_projects_the_baseline() {
        let mut detail = RunDetail::default();
        assert!(detail.tree().is_none());
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.tree().is_none(), "still Loading");
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));
        let tree = detail.tree().expect("loaded selection projects a tree");
        assert_eq!(tree.header.title, "fixture-trait");
    }

    #[test]
    fn activity_deltas_for_the_selected_run_mutate_the_overlay_and_issue_no_request() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(
            request.generation,
            Ok(baseline(session_with_active_frame("run-a", "the-frame")))
        ));
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        for (index, text) in ["first", "second", "third"].iter().enumerate() {
            let outcome = detail.follow(&activity_delta(
                &wire,
                narration(index as u64 + 1, "the-frame", text),
            ));
            assert!(outcome.request.is_none(), "no re-read from activity alone");
            assert!(outcome.changed);
        }
        let tree = detail.tree().unwrap();
        assert_eq!(tree.roots[0].narration.as_deref(), Some("third"));
    }

    #[test]
    fn row_changed_issues_a_request_only_when_the_fingerprint_moves() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 10);
        // This wire row's summary was hand-built (`wire_row`, not
        // `matching_wire_row`), so it genuinely differs from the fingerprint
        // `apply` just established from the loaded session — the first
        // delta below is a real structural move, not an artifact of an
        // unestablished fingerprint.
        let moving = LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(wire.clone()),
        });
        assert!(detail.follow(&moving).request.is_some());

        let same_fingerprint = LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(wire.clone()),
        });
        let outcome = detail.follow(&same_fingerprint);
        assert!(
            outcome.request.is_none(),
            "identical fingerprint issues no request"
        );

        let mut moved = wire.clone();
        moved.modified_epoch_secs = 20;
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(moved),
        }));
        assert!(
            outcome.request.is_some(),
            "a moved fingerprint issues exactly one request"
        );
    }

    #[test]
    fn first_identical_row_after_seed_issues_no_request() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let session = session("run-a");
        assert!(detail.apply(request.generation, Ok(baseline(session.clone()))));

        // Same modified_epoch_secs as `row()`'s fixture default (0) and a
        // summary derived from the very session that was just loaded: this
        // is the honest "nothing changed" case for the first delta a fresh
        // selection ever observes.
        let wire = matching_wire_row(&session, "repo-a", "/repo-a/run.json", true, 0);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(wire),
        }));
        assert!(
            outcome.request.is_none(),
            "the first row update after the seed load must not force a re-read \
             when nothing actually changed"
        );
    }

    #[test]
    fn session_title_activity_then_row_changed_issues_no_request() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let session = session("run-a");
        assert!(detail.apply(request.generation, Ok(baseline(session.clone()))));

        let wire = matching_wire_row(&session, "repo-a", "/repo-a/run.json", true, 0);

        // The center's own ordering (`center.rs:2539-2554`): a `SessionTitle`
        // sidecar line lands as an `ActivityLine` first, immediately
        // followed by a `RowChanged` carrying the rewritten title. Neither
        // must cost a ledger read — the overlay already has the title, and
        // the fingerprint clears `title` before comparing.
        let title_line = detail.follow(&activity_delta(
            &wire,
            ActivityRecord::SessionTitle {
                at_epoch_ms: 1,
                title: "a live title".to_string(),
            },
        ));
        assert!(
            title_line.request.is_none(),
            "an activity line never issues a ledger read"
        );

        let mut retitled = wire;
        retitled.summary.title = Some("a live title".to_string());
        let row_changed = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(retitled),
        }));
        assert!(
            row_changed.request.is_none(),
            "a title-only rewrite following SessionTitle must not force a re-read"
        );
    }

    #[test]
    fn deltas_for_a_different_ledger_path_never_touch_the_selection() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        let other = wire_row("repo-a", "/repo-a/other.json", "other-run", true, 999);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(other.clone()),
        }));
        assert!(!outcome.changed);
        assert!(outcome.request.is_none());

        let outcome = detail.follow(&activity_delta(&other, narration(1, "x", "text")));
        assert!(!outcome.changed);
        assert!(outcome.request.is_none());
    }

    #[test]
    fn activity_arriving_while_loading_is_replayed_after_the_baseline_lands_and_stale_ones_are_dropped()
     {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        // Arrives while still Loading: buffered.
        let outcome = detail.follow(&activity_delta(
            &wire,
            narration(5, "the-frame", "buffered-first"),
        ));
        assert!(outcome.request.is_none());
        assert!(
            !outcome.changed,
            "a buffered record has nothing to show yet"
        );

        // A second, newer record buffers too.
        detail.follow(&activity_delta(
            &wire,
            narration(10, "the-frame", "buffered-second"),
        ));

        // The seed's sidecar already carried a record at ms=8, so the
        // baseline's overlay watermark is 8 when the load lands.
        let mut seed_overlay = ActivityOverlay::default();
        seed_overlay.apply_record(&narration(8, "the-frame", "seed"));
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = seed_overlay;
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        // ms=5 predates the watermark (8) and must not roll ms=10 back; the
        // replay order (5 before 10) does not matter since apply_live_record
        // rejects on watermark, not arrival order.
        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].narration.as_deref(),
            Some("buffered-second"),
            "the newer replayed record wins; the stale one is dropped"
        );
    }

    /// Review-verdict-1 blocker `live-span-reopen-divergence`: the seed's
    /// sidecar read already contains a unique frame's two durable `Activity`
    /// stamps (1_000, 3_000), while the pending queue — buffered because the
    /// same non-tail record raced the seed read and arrived live before it
    /// landed — contains only a replay of the seed's *non-tail* 1_000
    /// record, never the 3_000 record. `apply` must still accept a 2-second
    /// span for that frame, matching a fresh reconstruction of the same
    /// seed records with no later `Activity` and no resync required to
    /// heal it.
    #[test]
    fn seed_pending_overlap_replays_a_non_tail_activity_once() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        // The seed's own non-tail record races the seed read and is
        // buffered live while the load is still in flight.
        let outcome = detail.follow(&activity_delta(&wire, activity(1_000, "the-frame", 1)));
        assert!(
            !outcome.changed,
            "a buffered record has nothing to show yet"
        );

        let seed_records = [
            activity(1_000, "the-frame", 1),
            activity(3_000, "the-frame", 2),
        ];
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = ActivityOverlay::from_records(&seed_records);
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].span,
            Some(std::time::Duration::from_millis(2_000)),
            "the seed's own non-tail record replayed live must not shorten the span"
        );

        let reopened = detail_tree::project(
            &session_with_active_frame("run-a", "the-frame"),
            &ActivityOverlay::from_records(&seed_records),
            false,
        );
        assert_eq!(
            tree.roots[0].span, reopened.roots[0].span,
            "the accepted live span must equal a fresh reconstruction of the same seed records"
        );
    }

    /// Review-verdict-1 blocker `live-span-reopen-divergence`: the seed's
    /// sidecar read already contains a unique frame's *three* durable
    /// `Activity` stamps (1_000, 2_000, 3_000), while the pending queue
    /// contains only a replay of the seed's *interior* 2_000 record — one
    /// `FrameSpans` has already folded away, since it retains only the
    /// first/last pair. A point-patch that reconciles only against the two
    /// retained endpoints cannot recognize this replay and would fold it as
    /// new, rolling the span's `last` stamp back to 2_000 and shortening the
    /// accepted live span from 2s to 1s. `apply`'s bounded
    /// record-occurrence reconciliation must still accept a 2-second span,
    /// matching a fresh reconstruction of the same seed records with no
    /// later `Activity` and no resync required to heal it.
    #[test]
    fn seed_pending_overlap_replays_an_interior_activity_once() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        // The seed's own interior record races the seed read and is
        // buffered live while the load is still in flight — a genuine
        // redelivery, so its content (including `sequence`) matches the
        // seed's own interior record exactly.
        let outcome = detail.follow(&activity_delta(&wire, activity(2_000, "the-frame", 2)));
        assert!(
            !outcome.changed,
            "a buffered record has nothing to show yet"
        );

        let seed_records = [
            activity(1_000, "the-frame", 1),
            activity(2_000, "the-frame", 2),
            activity(3_000, "the-frame", 3),
        ];
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = ActivityOverlay::from_records(&seed_records);
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].span,
            Some(std::time::Duration::from_millis(2_000)),
            "the seed's own interior record replayed live must not shorten the span"
        );

        let reopened = detail_tree::project(
            &session_with_active_frame("run-a", "the-frame"),
            &ActivityOverlay::from_records(&seed_records),
            false,
        );
        assert_eq!(
            tree.roots[0].span, reopened.roots[0].span,
            "the accepted live span must equal a fresh reconstruction of the same seed records"
        );
    }

    /// Review-verdict-1 blocker `live-span-reopen-divergence`: a genuinely
    /// new pending record can share the exact `(frame_id, at_epoch_ms)` pair
    /// of one of the seed's own non-tail records — a millisecond collision,
    /// not a replay — while carrying a distinct payload (a different
    /// `ActivityKind`). `apply` must recognize this by full record content
    /// and fold it as new evidence, never mistaking it for the seed's own
    /// record and silently dropping it. Folding a genuine third stamp at
    /// `1_000` (append order `1_000, 3_000, 1_000`) makes the span
    /// non-increasing, so the accepted span must be `None`, matching a fresh
    /// reconstruction of `[Thinking@1_000, Thinking@3_000, Stalled@1_000]`.
    #[test]
    fn seed_pending_timestamp_collision_is_not_a_replay() {
        use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        // A genuinely new record races the seed read, colliding on stamp
        // with the seed's own non-tail record but carrying a distinct kind.
        let collision = ActivityRecord::Activity {
            at_epoch_ms: 1_000,
            event: ActivityEvent {
                sequence: 99,
                frame_id: "the-frame".to_string(),
                kind: ActivityKind::Stalled,
                text: None,
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        };
        let outcome = detail.follow(&activity_delta(&wire, collision.clone()));
        assert!(
            !outcome.changed,
            "a buffered record has nothing to show yet"
        );

        let seed_records = [
            activity(1_000, "the-frame", 1),
            activity(3_000, "the-frame", 2),
        ];
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = ActivityOverlay::from_records(&seed_records);
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].span, None,
            "the distinct pending record must fold as new evidence, not be mistaken \
             for a replay of the seed's own record at the same stamp"
        );

        let all_records = [seed_records[0].clone(), seed_records[1].clone(), collision];
        let reopened = detail_tree::project(
            &session_with_active_frame("run-a", "the-frame"),
            &ActivityOverlay::from_records(&all_records),
            false,
        );
        assert_eq!(
            tree.roots[0].span, reopened.roots[0].span,
            "the accepted live span must equal a fresh reconstruction of all three records"
        );
    }

    /// Review-verdict-1 blocker `live-span-reopen-divergence`: the seed's
    /// reconciliation index must not survive past the one `apply` call that
    /// drains pending against it — the accepted, retained overlay carries
    /// only per-frame folded state, never a resident index sized to the
    /// seed's historical record count.
    #[test]
    fn accepted_overlay_drops_seed_occurrence_index() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();

        let seed_records = [
            activity(1_000, "the-frame", 1),
            activity(3_000, "the-frame", 2),
        ];
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = ActivityOverlay::from_records(&seed_records);
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        let DetailLoad::Loaded(baseline) = &detail.selected.as_ref().unwrap().load else {
            panic!("expected a loaded baseline");
        };
        assert!(
            baseline.activity_overlay.seed_occurrences_is_empty(),
            "the seed-occurrence reconciliation index must be discarded once apply \
             has drained the (empty) pending queue against it"
        );
    }

    /// Review-verdict-1 blocker `seed-pending-activity-replay-duplicates-line`,
    /// exercised end to end through `RunDetail::apply`: the seed's newest
    /// `Activity` record races the seed read and is buffered live, then
    /// replayed against the seed's own historical fold once the seed lands.
    /// The accepted tree must show exactly one visible activity line for it,
    /// not the seed's line plus a live duplicate — mirroring the
    /// ActivityOverlay-level coverage of the ambiguous and distinct-payload
    /// cases in `detail_tree`'s test module.
    #[test]
    fn seed_pending_overlap_shows_one_activity_line_not_a_duplicate() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);

        // The seed's own newest record races the seed read and is buffered
        // live while the load is still in flight — a genuine redelivery.
        let outcome = detail.follow(&activity_delta(&wire, activity(1_000, "the-frame", 1)));
        assert!(
            !outcome.changed,
            "a buffered record has nothing to show yet"
        );

        let seed_records = [activity(1_000, "the-frame", 1)];
        let mut seeded_baseline = baseline(session_with_active_frame("run-a", "the-frame"));
        seeded_baseline.activity_overlay = ActivityOverlay::from_records(&seed_records);
        assert!(detail.apply(request.generation, Ok(seeded_baseline)));

        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].activity_lines.len(),
            1,
            "a seed/pending replay of the same occurrence must not duplicate its line"
        );
    }

    #[test]
    fn down_marks_stale_a_delta_while_stale_is_dropped_and_a_recovery_snapshot_resyncs_once() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        let outcome = detail.follow(&LinkUpdate::Down("subscription closed".to_string()));
        assert!(outcome.changed);
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Stale { .. })
        ));

        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 50);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(wire.clone()),
        }));
        assert!(!outcome.changed, "a delta while stale must change nothing");
        assert!(outcome.request.is_none());

        let outcome = detail.follow(&LinkUpdate::Snapshot(vec![wire]));
        assert!(outcome.changed);
        assert_eq!(
            outcome
                .request
                .expect("exactly one resync request")
                .ledger_path,
            Utf8PathBuf::from("/repo-a/run.json")
        );
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Following)
        ));

        // A duplicate/first snapshot while already Following issues nothing.
        let again = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 50);
        let outcome = detail.follow(&LinkUpdate::Snapshot(vec![again]));
        assert!(outcome.request.is_none());
    }

    #[test]
    fn ended_for_the_selection_marks_it_not_live_and_stale_with_no_request() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::Ended {
            row: Box::new(wire),
        }));
        assert!(outcome.changed);
        assert!(outcome.request.is_none());
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Stale { .. })
        ));
        assert!(
            detail.tree().is_some(),
            "the last authoritative tree stays visible"
        );
    }

    #[test]
    fn a_resync_failure_keeps_the_previous_tree_and_reports_stale() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        let mut moved = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 99);
        moved.summary.run_id = "run-a".to_string();
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(moved),
        }));
        let resync = outcome.request.expect("fingerprint moved, resync issued");

        assert!(detail.apply(resync.generation, Err("transient read error".to_string())));
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Stale { .. })
        ));
        let tree = detail
            .tree()
            .expect("the previous Loaded tree stays visible");
        assert_eq!(tree.header.title, "fixture-trait");
    }

    #[test]
    fn a_live_flip_via_row_changed_unfreezes_the_header_with_no_ledger_read() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        let session = session("run-a");
        assert!(detail.apply(request.generation, Ok(baseline(session.clone()))));
        let before = detail.tree().unwrap().header.run_state;

        // The first row delta this selection observes: same fingerprint as
        // the just-loaded session, only `live` flips — the fact the ledger
        // does not own. No establishing delta needed; `apply` already set
        // the fingerprint from the loaded session.
        let live = matching_wire_row(&session, "repo-a", "/repo-a/run.json", true, 0);
        let mut flipped = live;
        flipped.live = false;
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(flipped),
        }));
        assert!(outcome.changed);
        assert!(
            outcome.request.is_none(),
            "a live flip alone costs no re-read"
        );
        let after = detail.tree().unwrap().header.run_state;
        assert_ne!(before, after, "the frozen live capture must unfreeze");
    }

    #[test]
    fn activity_arriving_during_loaded_resync_survives_baseline_replacement() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(
            request.generation,
            Ok(baseline(session_with_active_frame("run-a", "the-frame")))
        ));

        // Fingerprint moves, issuing a resync while `load` stays `Loaded`
        // with the old baseline (no loading flash).
        let mut moved = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 99);
        moved.summary.run_id = "run-a".to_string();
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(moved.clone()),
        }));
        let resync = outcome.request.expect("fingerprint moved, resync issued");

        // An activity record arrives while that resync is in flight.
        let outcome = detail.follow(&activity_delta(
            &moved,
            narration(50, "the-frame", "during-resync"),
        ));
        assert!(
            outcome.changed,
            "the displayed old overlay updates immediately for feedback"
        );

        // The background read lands with a fresh baseline that does not
        // carry the record above (it was read before/independently of the
        // sidecar append).
        assert!(detail.apply(
            resync.generation,
            Ok(baseline(session_with_active_frame("run-a", "the-frame")))
        ));

        let tree = detail.tree().unwrap();
        assert_eq!(
            tree.roots[0].narration.as_deref(),
            Some("during-resync"),
            "a record that arrived during a resync must survive the baseline replacement"
        );
    }

    #[test]
    fn ended_while_stale_is_ignored() {
        let mut detail = RunDetail::default();
        let request = detail
            .select(&row("repo-a", "/repo-a/run.json", "run-a"))
            .unwrap();
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));

        detail.follow(&LinkUpdate::Down("subscription closed".to_string()));
        let before_follow = detail.follow_state().cloned();
        let before_tree = detail.tree();

        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::Ended {
            row: Box::new(wire),
        }));
        assert!(
            !outcome.changed,
            "Ended must be a no-op while already stale"
        );
        assert!(outcome.request.is_none());
        assert_eq!(detail.follow_state().cloned(), before_follow);
        assert_eq!(detail.tree(), before_tree);

        let activity = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 0);
        let outcome = detail.follow(&activity_delta(&activity, narration(1, "x", "text")));
        assert!(!outcome.changed, "a delta must also be a no-op while stale");
        assert_eq!(detail.tree(), before_tree);
    }

    #[test]
    fn selection_created_while_center_is_stale_resyncs_on_recovery() {
        let mut detail = RunDetail::default();
        let request = detail
            .select_stale(
                &row("repo-a", "/repo-a/run.json", "run-a"),
                "subscription closed".to_string(),
            )
            .unwrap();
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Stale { .. })
        ));
        assert!(detail.apply(request.generation, Ok(baseline(session("run-a")))));
        assert!(
            matches!(detail.follow_state(), Some(FollowState::Stale { .. })),
            "the selection must stay stale after its own initial load lands"
        );

        // A delta arriving before recovery must not resync or un-stale it.
        let wire = wire_row("repo-a", "/repo-a/run.json", "run-a", true, 50);
        let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
            row: Box::new(wire.clone()),
        }));
        assert!(!outcome.changed);
        assert!(outcome.request.is_none());

        let outcome = detail.follow(&LinkUpdate::Snapshot(vec![wire]));
        assert!(outcome.changed);
        assert!(
            outcome.request.is_some(),
            "recovery must issue exactly one resync request"
        );
        assert!(matches!(
            detail.follow_state(),
            Some(FollowState::Following)
        ));
    }
}
