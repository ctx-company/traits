//! Run-session ledger persistence at the IO boundary.
//!
//! Core run/session transitions are pure. This module owns reading and writing
//! JSON session ledgers and rejects symlinked paths instead of silently
//! following them. Bare session IDs default to the invocation repository's
//! global per-repository runs root (P426; `~/.config/ctx/runs/<repo-key>/`)
//! unless a session store is supplied, with a one-release fallback to the
//! legacy repo-local `.ctx/runs` for sessions written before P426.

use camino::{Utf8Path, Utf8PathBuf};
use std::collections::HashMap;
use std::sync::Arc;

pub const EXPLICIT_RUN_SESSION_PATH_MESSAGE: &str =
    "run-session out must be an explicit ledger path; use ./ledger or ledger.json";

pub fn explicit_run_session_path(session: &str) -> Option<&str> {
    (session.contains('/') || session.ends_with(".json")).then_some(session)
}

/// Active default session-ledger store: the invocation repository's global
/// per-repository runs root (P426). An explicit `store` override always
/// wins over this default.
pub fn default_session_store() -> crate::Result<Utf8PathBuf> {
    crate::state::current_global_runs_root()
}

/// Legacy repo-local session-ledger store (`.ctx/runs`), retained as a
/// one-release dual-read fallback for bare session IDs written before the
/// global default took effect.
fn legacy_session_store() -> crate::Result<Utf8PathBuf> {
    crate::state::current_legacy_runs_root()
}

pub fn session_store_path(store: Option<&str>, session_id: &str) -> crate::Result<Utf8PathBuf> {
    validate_bare_session_id(session_id)?;
    let root = match store {
        Some(store) => Utf8PathBuf::from(store),
        None => default_session_store()?,
    };
    Ok(root.join(format!("{session_id}.json")))
}

/// Resolve a bare session ID to its ledger path. An explicit store override
/// always wins; otherwise the global path is preferred, falling back to the
/// legacy repo-local path only when a ledger already exists there (so a
/// session written before P426 keeps resolving), and defaulting to the
/// global path for a session about to be created.
///
/// `session` may also be an unambiguous prefix of a persisted session ID
/// (P421): an exact-ID ledger always wins over a prefix match, and a prefix
/// matching more than one ledger is an ambiguity error rather than a
/// first-match guess. Store precedence for prefix scanning follows the same
/// global-then-legacy order as exact resolution.
pub fn resolve_session_path(session: &str, store: Option<&str>) -> crate::Result<Utf8PathBuf> {
    if let Some(path) = explicit_run_session_path(session) {
        return Ok(Utf8PathBuf::from(path));
    }
    if let Some(store) = store {
        let exact = session_store_path(Some(store), session)?;
        if exact.is_file() {
            return Ok(exact);
        }
        return resolve_session_prefix_in_stores(session, &[Utf8PathBuf::from(store)])
            .map(|resolved| resolved.unwrap_or(exact));
    }
    validate_bare_session_id(session)?;
    let global = default_session_store()?.join(format!("{session}.json"));
    if global.is_file() {
        return Ok(global);
    }
    if let Ok(legacy_root) = legacy_session_store() {
        let legacy = legacy_root.join(format!("{session}.json"));
        if legacy.is_file() {
            return Ok(legacy);
        }
    }
    let mut roots = vec![default_session_store()?];
    if let Ok(legacy_root) = legacy_session_store() {
        roots.push(legacy_root);
    }
    Ok(resolve_session_prefix_in_stores(session, &roots)?.unwrap_or(global))
}

/// Scan `roots` in order for ledgers whose session ID starts with `prefix`.
/// Returns `Ok(None)` when no ledger matches (the caller falls back to the
/// default not-yet-created path so `start`/`create` flows are unaffected).
/// A prefix is only meaningful once it fails to name an exact session, so
/// `prefix` itself is never treated as a valid single match unless it is the
/// full ID of exactly one ledger.
fn resolve_session_prefix_in_stores(
    prefix: &str,
    roots: &[Utf8PathBuf],
) -> crate::Result<Option<Utf8PathBuf>> {
    if prefix.is_empty() {
        return Ok(None);
    }
    let mut matches: Vec<(String, Utf8PathBuf)> = Vec::new();
    for root in roots {
        for name in session_ledger_names(root)? {
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if id.starts_with(prefix) && !matches.iter().any(|(matched, _)| matched == id) {
                matches.push((id.to_string(), root.join(&name)));
            }
        }
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.into_iter().next().unwrap().1)),
        _ => {
            let mut ids: Vec<String> = matches.into_iter().map(|(id, _)| id).collect();
            ids.sort();
            Err(crate::Error::Usage {
                message: format!(
                    "session ID prefix '{prefix}' is ambiguous; matches: {}",
                    ids.join(", ")
                ),
            })
        }
    }
}

/// Short display form of a session id (P506 §3.4): `session-` plus a prefix
/// of the remainder long enough to be unique against `all_ids`, extended one
/// character at a time. Pure, no IO — the caller decides which id set counts
/// as "the whole store" (default scope: the whole store; ALL mode: each
/// row's own repository).
///
/// An id that does not start with `session-`, has fewer than 12 characters
/// after that prefix, or whose remainder is not all hex digits, renders in
/// full — a host-supplied id (e.g. `session-implement-FMSPmRRt`, seen in the
/// wild beside minted `session-<64 hex>` ids) cannot be assumed to be hex at
/// all. The shortener also refuses to emit a candidate that collides with
/// (is a prefix of, including being exactly equal to) any other id in
/// `all_ids`, since [`resolve_session_path`] gives an exact match precedence
/// over a prefix match and a badly chosen short form would therefore resolve
/// to the wrong session silently.
pub fn short_session_display(id: &str, all_ids: &[String]) -> String {
    const PREFIX: &str = "session-";
    let Some(rest) = id.strip_prefix(PREFIX) else {
        return id.to_string();
    };
    let rest_chars: Vec<char> = rest.chars().collect();
    if rest_chars.len() < 12 || !rest_chars.iter().all(|ch| ch.is_ascii_hexdigit()) {
        return id.to_string();
    }
    let mut take = 12;
    loop {
        let candidate = format!("{PREFIX}{}", rest_chars[..take].iter().collect::<String>());
        let collides = all_ids
            .iter()
            .any(|other| other != id && other.starts_with(&candidate));
        if !collides {
            return candidate;
        }
        if take >= rest_chars.len() {
            return id.to_string();
        }
        take += 1;
    }
}

pub fn ensure_explicit_run_session_output(path: &str) -> crate::Result<()> {
    if explicit_run_session_path(path).is_some() {
        return Ok(());
    }
    Err(crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            EXPLICIT_RUN_SESSION_PATH_MESSAGE,
        ),
    }
    .into())
}

pub fn read_run_session(
    path: &Utf8Path,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    reject_symlink_ancestors(path)?;
    reject_symlink_leaf(path)?;
    let text = crate::read::read_text(path)?;
    Ok(
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse run-session JSON at {path}"),
            source,
        })?,
    )
}

pub fn write_run_session(
    path: &Utf8Path,
    session: &ctx_traits_core::procedure::session::Session,
) -> crate::Result<()> {
    reject_symlink_ancestors(path)?;
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    if !parent.as_str().is_empty() && parent.as_str() != "." {
        std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
            path: parent.to_string(),
            source: e,
        })?;
    }
    reject_symlink_ancestors(path)?;
    reject_symlink_leaf(path)?;
    let text = serde_json::to_string_pretty(session).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize run-session JSON at {path}"),
            source,
        }
    })?;
    reject_symlink_leaf(path)?;
    write_text_atomically(path, &format!("{text}\n"))?;
    // Projected sidecar written after the ledger's own atomic rename
    // has landed, itself atomically renamed. Best-effort — a write failure
    // here is derived-evidence loss, never a ledger-write failure.
    let _ = crate::run_summary::write_summary(path, session);
    Ok(())
}

/// Remove a persisted session ledger by id (0066.4): an ephemeral drive
/// (built-in meta-trait runner, `run_builtin_trait_observed`) consumes its
/// session in-process and must not leave it parked in the store afterward —
/// a phantom session a later `ctx traits story`/`drive --session` would find
/// with nothing left to do. Missing-file is not an error: the session may
/// already be gone (e.g. a second cleanup attempt on the same path).
pub fn delete_run_session(session: &str, store: Option<&str>) -> crate::Result<()> {
    let path = resolve_session_path(session, store)?;
    match std::fs::remove_file(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

/// List persisted session-ledger paths (`*.json`) directly under a session
/// store directory, sorted by file name. Shared by any caller that must walk
/// the store rather than resolve a single known session id: the role work
/// queue (`run_queue`) and `ctx traits merge`'s resolve-by-run-id scan.
/// Session-ledger file names (`*.json`) directly under `root`, sorted.
fn session_ledger_names(root: &Utf8Path) -> crate::Result<Vec<String>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: root.to_string(),
                source,
            }
            .into());
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: root.to_string(),
            source,
        })?;
        let file_type =
            entry
                .file_type()
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: root.to_string(),
                    source,
                })?;
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        // `<ledger>.json.summary.json` sidecars also end in `.json`
        // and live directly under this same root — without this skip they
        // become phantom sessions everywhere this list feeds (the dashboard,
        // `stats`, `run_queue`'s candidate scan, `find_session_by_run_id`,
        // and worst of all `resolve_session_prefix_in_stores`, where every
        // real session id would instantly gain an "ambiguous prefix" twin).
        if name.ends_with(".summary.json") {
            continue;
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// List persisted session-ledger paths. An explicit `store` scans only that
/// directory; the default scans the global root first and the legacy
/// repo-local root second, deduplicating by file name so a session present
/// in both never appears twice — the global copy always wins the dedup.
pub fn session_store_paths(store: Option<&str>) -> crate::Result<Vec<Utf8PathBuf>> {
    if let Some(store) = store {
        let root = Utf8PathBuf::from(store);
        return Ok(session_ledger_names(&root)?
            .into_iter()
            .map(|name| root.join(name))
            .collect());
    }
    let global_root = default_session_store()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut paths: Vec<Utf8PathBuf> = Vec::new();
    for name in session_ledger_names(&global_root)? {
        seen.insert(name.clone());
        paths.push(global_root.join(name));
    }
    if let Ok(legacy_root) = legacy_session_store() {
        for name in session_ledger_names(&legacy_root)? {
            if seen.contains(&name) {
                continue;
            }
            paths.push(legacy_root.join(name));
        }
    }
    paths.sort();
    Ok(paths)
}

/// Resolve the single persisted session whose internal `run-id` matches, by
/// scanning the session store. `ctx traits merge <run-id>` uses this instead
/// of heuristic branch-name guessing: an unresolvable run-id (not found, or
/// only present as an unreadable/corrupt ledger) resolves to `None` so the
/// caller can report it as unresolvable with Git state untouched.
pub fn find_session_by_run_id(
    store: Option<&str>,
    run_id: &str,
) -> crate::Result<Option<(Utf8PathBuf, ctx_traits_core::procedure::session::Session)>> {
    for path in session_store_paths(store)? {
        let Ok(session) = read_run_session(&path) else {
            continue;
        };
        if session.run_id.as_str() == run_id {
            return Ok(Some((path, session)));
        }
    }
    Ok(None)
}

/// Append one typed merge frame to a session's provenance and persist it.
/// Reuses the same symlink-safe read/write path as every other ledger
/// mutation; the core `Provenance` type threads `merge_frames` through
/// unchanged on every rebuild, so appended frames survive later refreshes.
pub fn append_merge_frame(
    path: &Utf8Path,
    frame: ctx_traits_core::procedure::session::MergeFrame,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    let mut session = read_run_session(path)?;
    session.provenance.merge_frames.push(frame);
    write_run_session(path, &session)?;
    Ok(session)
}

/// Append one P479 out-of-tree-mutation finding to a session's provenance,
/// direct sibling of [`append_merge_frame`] — same symlink-safe read/write
/// pair, since ledger mutation policy must not drift between the two.
/// Timestamps at write time so callers never have to thread a clock read
/// through the drive loop themselves.
pub fn append_out_of_tree_mutation(
    path: &Utf8Path,
    paths: Vec<String>,
    frame: String,
    policy: &str,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    let mut session = read_run_session(path)?;
    session.provenance.out_of_tree_mutations.push(
        ctx_traits_core::procedure::session::OutOfTreeMutationEvidence {
            paths,
            frame,
            policy: policy.to_string(),
            detected_at_epoch: epoch_seconds(),
        },
    );
    write_run_session(path, &session)?;
    Ok(session)
}

pub const SESSION_TITLE_ATTEMPT_LIMIT: u32 = 3;

/// The host's terminal display constraints on a session title, applied at
/// the write path regardless of source (task 0110): a narrator response and
/// trait-authored sink text are both untrusted input for terminal
/// formatting. Strips ANSI escape sequences, C0/C1 control characters, and
/// Unicode bidi-formatting controls (the same class `tui::clean_live_text`
/// strips from live output — duplicated here rather than reused because
/// `ctx-traits-io` sits below `ctx-traits-cli` in the dependency graph),
/// collapses to a single line, and clamps to 60 characters.
pub const SESSION_TITLE_DISPLAY_CLAMP: usize = 60;

pub fn sanitize_session_title(title: &str) -> String {
    let mut cleaned = String::with_capacity(title.len());
    let bytes = title.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index += 1;
            match bytes.get(index).copied() {
                Some(b'[') => {
                    index += 1;
                    while index < bytes.len() {
                        let byte = bytes[index];
                        index += 1;
                        if (0x40..=0x7e).contains(&byte) {
                            break;
                        }
                    }
                }
                Some(b']') => {
                    index += 1;
                    while index < bytes.len() {
                        match bytes[index] {
                            0x07 => {
                                index += 1;
                                break;
                            }
                            0x1b if bytes.get(index + 1) == Some(&b'\\') => {
                                index += 2;
                                break;
                            }
                            _ => index += 1,
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(ch) = title[index..].chars().next() else {
            break;
        };
        index += ch.len_utf8();
        let is_bidi_control = matches!(
            ch,
            '\u{200E}' | '\u{200F}'
                | '\u{061C}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        );
        if ch.is_control() || is_bidi_control {
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }
    let single_line: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line
        .chars()
        .take(SESSION_TITLE_DISPLAY_CLAMP)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTitleClaim {
    Claimed { attempts: u32 },
    NotClaimable,
}

/// Claim one bounded title attempt under the current driver-lock owner. An
/// in-flight owner from an earlier lock acquisition has lapsed by definition.
pub fn claim_session_title_attempt(
    path: &Utf8Path,
    owner: &str,
) -> crate::Result<SessionTitleClaim> {
    let mut session = read_run_session(path)?;
    use ctx_traits_core::procedure::session::SessionTitleState;
    let attempts = match session.provenance.session_title.as_ref() {
        None => 1,
        Some(SessionTitleState::Retryable { attempts })
        | Some(SessionTitleState::InFlight { attempts, .. })
            if *attempts < SESSION_TITLE_ATTEMPT_LIMIT =>
        {
            // Same-owner reclaims would duplicate a still-running worker.
            if matches!(session.provenance.session_title.as_ref(), Some(SessionTitleState::InFlight { owner: current, .. }) if current == owner)
            {
                return Ok(SessionTitleClaim::NotClaimable);
            }
            *attempts + 1
        }
        // A new driver proves the prior owner cannot complete this final
        // attempt. Make that abandoned claim terminal rather than displaying a
        // permanent in-flight title row on resume.
        Some(SessionTitleState::InFlight {
            owner: current,
            attempts,
        }) if current != owner && *attempts >= SESSION_TITLE_ATTEMPT_LIMIT => {
            session.provenance.session_title = Some(SessionTitleState::Terminal {
                attempts: *attempts,
                reason: "attempt-limit-exhausted".to_string(),
            });
            write_run_session(path, &session)?;
            return Ok(SessionTitleClaim::NotClaimable);
        }
        _ => return Ok(SessionTitleClaim::NotClaimable),
    };
    session.provenance.session_title = Some(SessionTitleState::InFlight {
        owner: owner.to_string(),
        attempts,
    });
    write_run_session(path, &session)?;
    Ok(SessionTitleClaim::Claimed { attempts })
}

/// Persist a successful title only when `owner` still owns the current claim.
pub fn record_session_title(path: &Utf8Path, owner: &str, title: String) -> crate::Result<bool> {
    let mut session = read_run_session(path)?;
    use ctx_traits_core::procedure::session::{SessionTitleSource, SessionTitleState};
    let Some(SessionTitleState::InFlight {
        owner: current,
        attempts,
    }) = session.provenance.session_title.as_ref()
    else {
        return Ok(false);
    };
    if current != owner {
        return Ok(false);
    }
    session.provenance.session_title = Some(SessionTitleState::Resolved {
        attempts: *attempts,
        title: sanitize_session_title(&title),
        source: SessionTitleSource::NarratorDefault,
    });
    write_run_session(path, &session)?;
    Ok(true)
}

/// Persist a title from `[sink.session-title]` (task 0110), bypassing the
/// claim/attempt machine entirely: a sink write is not an "attempt" (a
/// deterministic verbatim render costs nothing to retry and a generated
/// dispatch is already claimed and resolved through the ordinary
/// [`record_session_title`] path before this function is ever reached for
/// that source). This unconditionally overrides any standing `InFlight` or
/// `Retryable` auto-title claim — "the sink wins" — because a sink write
/// only ever happens after the sink's own readiness fires, which the caller
/// serializes against the frame boundary; there is no owner to check here,
/// only a state to replace. `title` is sanitized again here regardless of
/// what the caller already did — host display constraints are enforced at
/// the write path for every source, belt-and-suspenders, so nothing reaches
/// the ledger unsanitized even if a future caller forgets to pre-sanitize.
pub fn record_session_title_from_sink(
    path: &Utf8Path,
    source: ctx_traits_core::procedure::session::SessionTitleSource,
    title: String,
) -> crate::Result<()> {
    use ctx_traits_core::procedure::session::SessionTitleState;
    let mut session = read_run_session(path)?;
    let attempts = match session.provenance.session_title.as_ref() {
        Some(SessionTitleState::InFlight { attempts, .. })
        | Some(SessionTitleState::Retryable { attempts })
        | Some(SessionTitleState::Resolved { attempts, .. })
        | Some(SessionTitleState::Terminal { attempts, .. }) => *attempts,
        None => 0,
    };
    session.provenance.session_title = Some(SessionTitleState::Resolved {
        attempts,
        title: sanitize_session_title(&title),
        source,
    });
    write_run_session(path, &session)?;
    Ok(())
}

/// Finish a claimed attempt as retryable or terminal. As with success, stale
/// workers cannot change a newer owner's lifecycle.
pub fn record_session_title_failure(
    path: &Utf8Path,
    owner: &str,
    reason: String,
) -> crate::Result<bool> {
    let mut session = read_run_session(path)?;
    use ctx_traits_core::procedure::session::SessionTitleState;
    let Some(SessionTitleState::InFlight {
        owner: current,
        attempts,
    }) = session.provenance.session_title.as_ref()
    else {
        return Ok(false);
    };
    if current != owner {
        return Ok(false);
    }
    let attempts = *attempts;
    session.provenance.session_title = Some(if attempts >= SESSION_TITLE_ATTEMPT_LIMIT {
        SessionTitleState::Terminal { attempts, reason }
    } else {
        SessionTitleState::Retryable { attempts }
    });
    write_run_session(path, &session)?;
    Ok(true)
}

/// Finish a claimed attempt as terminal unconditionally, regardless of
/// `attempts`. 0079: an api-transport title dispatch already retried
/// internally (the provider client's own bounded, transient-only retry) —
/// re-driving its failure through the ordinary [`record_session_title_failure`]
/// attempt ladder would let a later tick reclaim and re-dispatch it, stacking
/// a second retry layer on top of the client's own (the draft's "double
/// retry" risk). As with [`record_session_title_failure`], a stale (not
/// current) owner cannot change a newer owner's lifecycle.
pub fn record_session_title_failure_terminal(
    path: &Utf8Path,
    owner: &str,
    reason: String,
) -> crate::Result<bool> {
    let mut session = read_run_session(path)?;
    use ctx_traits_core::procedure::session::SessionTitleState;
    let Some(SessionTitleState::InFlight {
        owner: current,
        attempts,
    }) = session.provenance.session_title.as_ref()
    else {
        return Ok(false);
    };
    if current != owner {
        return Ok(false);
    }
    session.provenance.session_title = Some(SessionTitleState::Terminal {
        attempts: *attempts,
        reason,
    });
    write_run_session(path, &session)?;
    Ok(true)
}

/// Terminally complete an owned claim when no narrator can be resolved.
pub fn record_session_title_no_narrator(path: &Utf8Path, owner: &str) -> crate::Result<bool> {
    let mut session = read_run_session(path)?;
    use ctx_traits_core::procedure::session::SessionTitleState;
    let Some(SessionTitleState::InFlight {
        owner: current,
        attempts,
    }) = session.provenance.session_title.as_ref()
    else {
        return Ok(false);
    };
    if current != owner {
        return Ok(false);
    }
    session.provenance.session_title = Some(SessionTitleState::Terminal {
        attempts: *attempts,
        reason: "no-resolvable-narrator".to_string(),
    });
    write_run_session(path, &session)?;
    Ok(true)
}

/// Clear the P460 automatic-landing merge intent on a session's provenance
/// (`ctx traits drive --no-merge`, applied only once this invocation holds
/// the per-session driver lock). The initial intent is set once, as part of
/// the session's first persisted provenance at start — never by this
/// function — so a globally discoverable ledger is never briefly missing its
/// requested landing intent. Reuses the same symlink-safe read/write path as
/// [`append_merge_frame`]/[`record_drive_outcome`].
pub fn set_merge_intent(
    path: &Utf8Path,
    intent: Option<ctx_traits_core::procedure::session::MergeRung>,
) -> crate::Result<()> {
    let mut session = read_run_session(path)?;
    session.provenance.merge_intent = intent;
    write_run_session(path, &session)
}

/// Stamp the terminal outcome of a drive conductor on the session ledger, so
/// an inspected ledger distinguishes "conductor exited (why, when)" from
/// "conductor still running". Core transitions rebuild the session without the
/// marker, so a present marker always postdates the last accepted frame.
pub fn record_drive_outcome(
    session: &str,
    store: Option<&str>,
    outcome: &str,
    provider_credits_pause: Option<ctx_traits_core::procedure::runtime::ProviderCreditsPause>,
    evidence: ctx_traits_core::procedure::session::DriveTerminalEvidence,
) -> crate::Result<()> {
    let path = resolve_session_path(session, store)?;
    let mut loaded = read_run_session(&path)?;
    loaded.last_drive_outcome = Some(ctx_traits_core::procedure::session::DriveOutcome {
        outcome: ctx_traits_core::procedure::session::DriveOutcomeKind::from_wire(outcome),
        recorded_at_epoch: epoch_seconds(),
        provider_credits_pause,
        effective_budget: evidence.effective_budget,
        token_usage: evidence.token_usage,
        exit_code: evidence.exit_code,
        rate_limit: evidence.rate_limit,
    });
    write_run_session(&path, &loaded)
}

/// Record a typed interrupted outcome for an orphaned drive by its already
/// resolved ledger path. This deliberately shares the ordinary atomic ledger
/// writer, so the derived summary sidecar stays in sync without another status
/// model or a dashboard-owned serialization path.
pub fn record_interrupted_outcome(path: &Utf8Path) -> crate::Result<()> {
    let mut loaded = read_run_session(path)?;
    loaded.last_drive_outcome = Some(ctx_traits_core::procedure::session::DriveOutcome {
        outcome: ctx_traits_core::procedure::session::DriveOutcomeKind::Interrupted,
        recorded_at_epoch: epoch_seconds(),
        provider_credits_pause: None,
        effective_budget: None,
        token_usage: None,
        exit_code: None,
        rate_limit: None,
    });
    write_run_session(path, &loaded)
}

/// The single owner of the `port:task` ref literal (P472; `port:phase`
/// until the 2026-07-31 task-board migration): finds it among `(ref,
/// value)` pairs and returns its string value, if any. Generic over the
/// pair source rather than one container type because the two real callers
/// hold structurally-identical-but-distinct types — a session's
/// `accepted_port_values: Vec<procedure::session::Value>` and a dispatch's
/// pre-session `initial_values: Vec<procedure::runtime::StepSlotOutput>` —
/// so the literal, not a container type, is the thing kept singular.
pub fn task_value_from_pairs<'a>(
    pairs: impl Iterator<Item = (&'a str, &'a serde_json::Value)>,
) -> Option<String> {
    pairs
        .into_iter()
        .find(|(ref_text, _)| *ref_text == "port:task")
        .and_then(|(_, value)| value.as_str())
        .map(str::to_string)
}

const PARK_REPORT_SLOT_REF: &str = "slot:park-report";
const FEASIBILITY_SLOT_REF: &str = "slot:feasibility";

/// One operational step of a [`ParkBlocker`]'s fix, mirroring
/// `blockerStepSchema` (`packages/agents/src/index.ts:207`) field-for-field.
/// Tolerant: every field defaults, so a report missing an optional field
/// parses instead of vanishing whole (0064's Watch §3 — a schema-drifted
/// report demotes gracefully, it never crashes the reader).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ParkBlockerStep {
    pub step: String,
    pub status: String,
    pub evidence: String,
}

/// One blocking defect off a park report's `blockers` list, mirroring
/// `blockerSchema` (`packages/agents/src/index.ts:227`) field-for-field —
/// the input 0064's split-from-park-report reads to propose one child task
/// per open blocker. Tolerant, same rationale as [`ParkBlockerStep`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ParkBlocker {
    pub id: String,
    #[serde(rename = "where")]
    pub location: String,
    pub what: String,
    #[serde(rename = "root-cause")]
    pub root_cause: String,
    #[serde(rename = "required-fix")]
    pub required_fix: String,
    pub steps: Vec<ParkBlockerStep>,
    #[serde(rename = "done-when")]
    pub done_when: String,
}

impl ParkBlocker {
    /// Whether this blocker still has fix work outstanding — any step not
    /// `status == "done"`, or no steps recorded at all (a blocker with an
    /// empty step list is never treated as already resolved).
    pub fn is_open(&self) -> bool {
        self.steps.iter().any(|step| step.status != "done") || self.steps.is_empty()
    }
}

/// A blocked run's park report, mirroring `reviewVerdictSchema`
/// (`packages/agents/src/index.ts:418`) field-for-field but reduced to what
/// 0064's reconcile/split mechanics read. Tolerant, same rationale as
/// [`ParkBlockerStep`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ParkReportEntry {
    pub status: String,
    pub blockers: Vec<ParkBlocker>,
    #[serde(rename = "wall-id")]
    pub wall_id: String,
}

/// The kit's feasibility triage verdict, mirroring `feasibilityVerdictSchema`
/// (`packages/agents/src/feasibility.ts:12`) field-for-field. Tolerant, same
/// rationale as [`ParkBlockerStep`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct FeasibilityVerdict {
    pub verdict: String,
    pub evidence: String,
    pub missing: Vec<String>,
    #[serde(rename = "owner-action")]
    pub owner_action: String,
}

/// The typed park-report entry a blocked session's final review round
/// accepted onto `slot:park-report`, if any — read from `accepted_slot_values`
/// (the SAME evidence a completed session's `port:park-report` final output
/// is itself projected from at completion time), never a re-derivation from
/// `completion.final-outputs` (a blocked session never reaches normal
/// completion, so that projection never runs for it). A slot's LATEST
/// accepted value replaces the prior round's per `parkReport`'s own
/// "Replaced by each review step" contract, so the last matching entry in
/// this append-ordered evidence list is the one that stood when the run
/// blocked. Extracted out of `dispatch_preflight.rs` (0064) to live beside
/// [`session_task`]; that module now calls this shared function.
pub fn session_park_report(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<serde_json::Value> {
    let value = session
        .accepted_slot_values
        .iter()
        .rev()
        .find(|value| value.ref_text == PARK_REPORT_SLOT_REF)?;
    value.value.as_array()?.first().cloned()
}

/// [`session_park_report`], deserialized into the tolerant typed mirror
/// (0064). `None` when there is no park report, or the report present does
/// not even parse as a JSON object — never a hard error, per the same
/// tolerant-reader rationale as [`ParkBlockerStep`].
pub fn typed_park_report(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<ParkReportEntry> {
    serde_json::from_value(session_park_report(session)?).ok()
}

/// The typed feasibility-triage verdict a session accepted onto
/// `slot:feasibility`, if any, by the same latest-wins rule as
/// [`session_park_report`]. `None` for a session that never ran feasibility
/// triage, or whose accepted value does not parse.
pub fn typed_feasibility_verdict(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<FeasibilityVerdict> {
    let value = session
        .accepted_slot_values
        .iter()
        .rev()
        .find(|value| value.ref_text == FEASIBILITY_SLOT_REF)?;
    let entry = value.value.as_array().and_then(|arr| arr.first()).cloned();
    serde_json::from_value(entry.unwrap_or_else(|| value.value.clone())).ok()
}

/// The `port:task` value accepted by a session, if any (P472; previously
/// duplicated as a private helper in `dispatch_preflight.rs` and inlined
/// again in `run.rs` — both now delegate to [`task_value_from_pairs`]).
pub fn session_task(session: &ctx_traits_core::procedure::session::Session) -> Option<String> {
    task_value_from_pairs(
        session
            .accepted_port_values
            .iter()
            .map(|value| (value.ref_text.as_str(), &value.value)),
    )
}

/// One row of the P423 dashboard SESSIONS-screen inventory: a ledger this
/// repository's default global-then-legacy stores actually contain, resolved
/// enough to display and act on without re-parsing it a second time per
/// screen refresh.
pub struct RunInventoryRow {
    pub session_id: String,
    pub ledger_path: Utf8PathBuf,
    pub status: InventoryOutcome,
    pub modified_epoch_secs: u64,
}

/// Either a readable ledger's session state, or the reason it could not be
/// read — kept as a typed alternative rather than a `Result` so a scan of
/// many ledgers can report one unreadable row without aborting the rest.
#[derive(Clone)]
pub enum InventoryOutcome {
    Readable {
        session: Arc<ctx_traits_core::procedure::session::Session>,
        /// The latest `Parked` merge-frame entry, if the ledger's most recent
        /// merge attempt ended there (a later non-parked frame — e.g. a
        /// subsequent successful merge — supersedes it, so this is always the
        /// *last* frame, checked for `Parked`, not merely *any* parked frame).
        latest_parked_merge: Option<ctx_traits_core::procedure::session::MergeFrame>,
    },
    Unreadable {
        error: String,
    },
}

/// In-process, mtime+size-gated memo over parsed
/// ledgers, so a dashboard tick that finds nothing changed re-reads two
/// `stat`s per row instead of deep-parsing a ~190 KB ledger. No disk, no
/// invalidation protocol, no second store — purely a refcount-bump-on-hit
/// optimization over [`run_inventory_from_paths`]. Corrupt/zero-byte ledgers
/// are cached as `Unreadable` too, so a known-bad ledger is not re-read and
/// re-failed on every tick.
#[derive(Default)]
pub struct InventoryCache {
    entries: HashMap<Utf8PathBuf, CacheEntry>,
}

struct CacheEntry {
    modified: std::time::SystemTime,
    size: u64,
    outcome: InventoryOutcome,
}

impl InventoryCache {
    pub fn new() -> InventoryCache {
        InventoryCache::default()
    }

    fn hit(&self, path: &Utf8Path, modified: std::time::SystemTime, size: u64) -> bool {
        self.entries
            .get(path)
            .is_some_and(|entry| entry.modified == modified && entry.size == size)
    }
}

/// Build the SESSIONS-screen inventory for this repository's default session
/// stores (global-first, legacy-fallback, same dedup as [`session_store_paths`]).
/// Every ledger under those stores is included — reading failures become
/// `InventoryOutcome::Unreadable` rows rather than aborting the scan, so one
/// corrupt ledger never hides every other session from the dashboard.
pub fn current_repo_run_inventory() -> crate::Result<Vec<RunInventoryRow>> {
    let mut cache = InventoryCache::new();
    current_repo_run_inventory_cached(&mut cache)
}

/// [`current_repo_run_inventory`], threading a caller-owned [`InventoryCache`]
/// instead of allocating a throwaway one — the dashboard's entry point, so a
/// tick over an unchanged ledger store is cache hits, not full re-parses.
pub fn current_repo_run_inventory_cached(
    cache: &mut InventoryCache,
) -> crate::Result<Vec<RunInventoryRow>> {
    run_inventory_from_paths(session_store_paths(None)?, cache)
}

/// Build inventory rows from an already-resolved set of ledger paths —
/// shared by [`current_repo_run_inventory`] (global-first, legacy-fallback
/// for the current repository) and [`machine_wide_run_inventory`] (one
/// indexed repository's global store, P439). One-shot callers pass a
/// throwaway cache (`InventoryCache::new()`); a caller ticking repeatedly
/// (the dashboard) owns and reuses one across calls so a cache hit is a
/// refcount bump instead of a full re-parse.
pub fn run_inventory_from_paths(
    paths: Vec<Utf8PathBuf>,
    cache: &mut InventoryCache,
) -> crate::Result<Vec<RunInventoryRow>> {
    let present: std::collections::HashSet<_> = paths.iter().cloned().collect();
    cache.entries.retain(|path, _| present.contains(path));
    let mut rows = Vec::new();
    for path in paths {
        let session_id = path
            .file_stem()
            .map(str::to_string)
            .unwrap_or_else(|| path.to_string());
        let metadata = std::fs::metadata(path.as_std_path()).ok();
        let modified = metadata.as_ref().and_then(|m| m.modified().ok());
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_epoch_secs = modified
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        let status = match modified {
            Some(modified) if cache.hit(&path, modified, size) => {
                cache.entries.get(&path).expect("cache hit").outcome.clone()
            }
            _ => {
                let status = match read_run_session(&path) {
                    Ok(session) => {
                        let latest_parked_merge = session
                            .provenance
                            .merge_frames
                            .last()
                            .filter(|frame| {
                                frame.status
                                    == ctx_traits_core::procedure::session::MergeStatus::Parked
                            })
                            .cloned();
                        InventoryOutcome::Readable {
                            session: Arc::new(session),
                            latest_parked_merge,
                        }
                    }
                    Err(error) => InventoryOutcome::Unreadable {
                        error: error.to_string(),
                    },
                };
                if let Some(modified) = modified {
                    cache.entries.insert(
                        path.clone(),
                        CacheEntry {
                            modified,
                            size,
                            outcome: status.clone(),
                        },
                    );
                }
                status
            }
        };
        rows.push(RunInventoryRow {
            session_id,
            ledger_path: path,
            status,
            modified_epoch_secs,
        });
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.modified_epoch_secs));
    Ok(rows)
}

/// One repository's rows in the machine-wide run inventory (P439): its
/// indexed identity (`repo.toml` key/path — an `adhoc-`-prefixed key for a
/// non-repository invocation identity) plus every run under its global run
/// root.
pub struct MachineRunInventoryEntry {
    pub repo_key: String,
    pub repo_path: String,
    pub rows: Vec<RunInventoryRow>,
}

/// Build the machine-wide run inventory (P439): every repository or ad-hoc
/// invocation identity recorded in [`crate::state::read_repo_index`], each
/// with its runs scanned from its own [`crate::state::global_runs_root`] —
/// the consumer half of P426/P439's ad-hoc-run producer, and the dashboard
/// ALL mode's data source. Only the current repository's inventory
/// ([`current_repo_run_inventory`]) also dual-reads the legacy repo-local
/// store; every other indexed entry is read from its global root alone.
pub fn machine_wide_run_inventory() -> crate::Result<Vec<MachineRunInventoryEntry>> {
    let mut cache = InventoryCache::new();
    machine_wide_run_inventory_cached(&mut cache)
}

/// [`machine_wide_run_inventory`], threading a caller-owned [`InventoryCache`]
/// — see [`current_repo_run_inventory_cached`].
pub fn machine_wide_run_inventory_cached(
    cache: &mut InventoryCache,
) -> crate::Result<Vec<MachineRunInventoryEntry>> {
    let mut entries = Vec::new();
    for repo in crate::state::read_repo_index()? {
        let root = crate::state::global_runs_root(&repo.key)?;
        let rows = run_inventory_from_paths(session_store_paths(Some(root.as_str()))?, cache)?;
        entries.push(MachineRunInventoryEntry {
            repo_key: repo.key,
            repo_path: repo.path,
            rows,
        });
    }
    entries.sort_by(|a, b| a.repo_key.cmp(&b.repo_key));
    Ok(entries)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn write_text_atomically(path: &Utf8Path, text: &str) -> crate::Result<()> {
    crate::write::write_bytes_atomically(path, text.as_bytes())
}

fn validate_bare_session_id(session_id: &str) -> crate::Result<()> {
    crate::path_safety::validate_bare_path_component(session_id, "bare run-session ID")
}

pub(crate) fn reject_symlink_leaf(path: &Utf8Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::other("run-session path is a symlink"),
            }
            .into())
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::other("run-session path is not a regular file"),
            }
            .into())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}

pub(crate) fn reject_symlink_ancestors(path: &Utf8Path) -> crate::Result<()> {
    let mut skipped_absolute_alias = false;
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if ancestor.as_str().is_empty()
            || ancestor == Utf8Path::new(".")
            || ancestor.as_str() == "/"
            || ancestor == path
        {
            continue;
        }
        // Tolerate the OS root alias (macOS `/tmp -> /private/tmp`, `/var -> /private/var`):
        // skip exactly the immediate child of `/`. Creating a symlink directly under `/`
        // requires root; every deeper, user-writable segment is still checked below. This
        // matches the lenient policy already used by `io::write` and `io::import::support`.
        if path.is_absolute()
            && !skipped_absolute_alias
            && ancestor.parent().is_some_and(|p| p.as_str() == "/")
        {
            skipped_absolute_alias = true;
            continue;
        }
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source: std::io::Error::other(
                        "run-session parent path contains a symlink; use a non-symlinked path (on macOS, the resolved /private/... path)",
                    ),
                }
                .into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: ancestor.to_string(),
                    source: e,
                }
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod session_title_tests {
    use super::*;
    use ctx_traits_core::digest::Digest;
    use ctx_traits_core::procedure::runtime::FinalState;

    /// Minimal-but-valid [`ctx_traits_core::procedure::session::Session`] —
    /// same shape as `ctx traits merge`'s own `write_test_session` helper,
    /// duplicated here rather than shared since the two crates' test modules
    /// cannot see each other's `#[cfg(test)]` items.
    fn write_test_session(path: &Utf8Path, run_id: &str) {
        let session = ctx_traits_core::procedure::session::Session {
            schema_version: "1".to_string(),
            session_id: ctx_traits_core::procedure::session::SessionId::new(format!(
                "session-{run_id}"
            ))
            .expect("session id"),
            run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string()).expect("run id"),
            trait_id: "test-trait".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: ctx_traits_core::procedure::session::Status::AwaitingInput,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: None,
            last_validation_report: None,
            completion: None,
            last_drive_outcome: None,
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "test".to_string(),
                    caller: "run-session-test".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
            ledger: ctx_traits_core::procedure::runtime::State {
                run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string())
                    .expect("run id"),
                trait_id: "test-trait".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                active_path: Vec::new(),
                control_stack: Vec::new(),
                branch_decisions: Vec::new(),
                conditional_input_decisions: Vec::new(),
                ask_decisions: Vec::new(),
                failure_routes: Vec::new(),
                guard_evaluations: Vec::new(),
                parallel_panel_records: Vec::new(),
                stop_reason: None,
                elapsed_seconds: 0,
                final_state: FinalState::Running,
            },
            state_digest: Digest::source("test"),
        };
        write_run_session(path, &session).expect("write session");
    }

    fn scratch_session_path(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-run-session-title-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir.join("ledger.json")
    }

    #[test]
    fn old_ledger_with_no_session_title_field_deserializes_to_none() {
        let path = scratch_session_path("old-ledger");
        write_test_session(&path, "old-ledger-run");
        let text = std::fs::read_to_string(path.as_std_path()).expect("read ledger");
        assert!(
            !text.contains("session-title") && !text.contains("session_title"),
            "an unattempted title must not be serialized at all"
        );
        let session = read_run_session(&path).expect("read session back");
        assert!(session.provenance.session_title.is_none());
    }

    #[test]
    fn first_claim_records_owned_in_flight_state() {
        let path = scratch_session_path("first-claim");
        write_test_session(&path, "first-claim-run");
        let claimed = claim_session_title_attempt(&path, "owner-a").expect("claim");
        assert_eq!(claimed, SessionTitleClaim::Claimed { attempts: 1 });
        let session = read_run_session(&path).expect("read session back");
        let state = session.provenance.session_title.expect("attempt recorded");
        assert_eq!(
            state,
            ctx_traits_core::procedure::session::SessionTitleState::InFlight {
                owner: "owner-a".to_string(),
                attempts: 1
            }
        );
    }

    #[test]
    fn same_owner_refused_but_new_owner_reclaims_lapsed_attempt() {
        let path = scratch_session_path("repeat-claim");
        write_test_session(&path, "repeat-claim-run");
        assert_eq!(
            claim_session_title_attempt(&path, "owner-a").expect("first claim"),
            SessionTitleClaim::Claimed { attempts: 1 }
        );
        assert_eq!(
            claim_session_title_attempt(&path, "owner-a").expect("same owner"),
            SessionTitleClaim::NotClaimable
        );
        assert_eq!(
            claim_session_title_attempt(&path, "owner-b").expect("new owner"),
            SessionTitleClaim::Claimed { attempts: 2 }
        );
    }

    #[test]
    fn successful_result_persists_and_survives_reconstruction() {
        let path = scratch_session_path("success");
        write_test_session(&path, "success-run");
        assert_eq!(
            claim_session_title_attempt(&path, "owner-a").expect("claim"),
            SessionTitleClaim::Claimed { attempts: 1 }
        );
        assert!(
            record_session_title(&path, "owner-a", "Refactor the merge story".to_string())
                .expect("record")
        );
        let session = read_run_session(&path).expect("read session back");
        let state = session.provenance.session_title.expect("title state");
        assert_eq!(state.resolved_title(), Some("Refactor the merge story"));
        // A later claim attempt (e.g. a stray resume) must still refuse,
        // since a resolved title is read-only from here on.
        assert_eq!(
            claim_session_title_attempt(&path, "owner-b").expect("claim after success"),
            SessionTitleClaim::NotClaimable
        );
    }

    #[test]
    fn sink_verbatim_write_bypasses_the_attempt_claim_entirely() {
        use ctx_traits_core::procedure::session::SessionTitleSource;
        let path = scratch_session_path("sink-verbatim");
        write_test_session(&path, "sink-verbatim-run");
        record_session_title_from_sink(
            &path,
            SessionTitleSource::SinkVerbatim,
            "Fixed title".to_string(),
        )
        .expect("sink write");
        let session = read_run_session(&path).expect("read session back");
        let state = session.provenance.session_title.expect("title state");
        assert_eq!(state.resolved_title(), Some("Fixed title"));
        assert_eq!(
            state.resolved_source(),
            Some(SessionTitleSource::SinkVerbatim)
        );
        // No claim was ever made, so the attempt count stays untouched at 0.
        assert_eq!(
            state,
            ctx_traits_core::procedure::session::SessionTitleState::Resolved {
                attempts: 0,
                title: "Fixed title".to_string(),
                source: SessionTitleSource::SinkVerbatim,
            }
        );
    }

    #[test]
    fn sink_write_overrides_a_standing_in_flight_auto_claim() {
        use ctx_traits_core::procedure::session::SessionTitleSource;
        let path = scratch_session_path("sink-overrides-in-flight");
        write_test_session(&path, "sink-overrides-in-flight-run");
        assert_eq!(
            claim_session_title_attempt(&path, "owner-a").expect("claim"),
            SessionTitleClaim::Claimed { attempts: 1 }
        );
        record_session_title_from_sink(
            &path,
            SessionTitleSource::SinkGenerated,
            "Sink wins".to_string(),
        )
        .expect("sink write");
        let session = read_run_session(&path).expect("read session back");
        assert_eq!(
            session.provenance.session_title.unwrap().resolved_title(),
            Some("Sink wins")
        );
    }

    /// 0110: `ctx traits run-status --json`/the receipt path is a thin
    /// read-and-reserialize of the raw ledger (`ctx_traits_io::run::status`
    /// returns the session it read, unmodified) — so the source this test
    /// asserts on-disk is exactly what that surface reports, with no
    /// intermediate filtering that could silently drop it.
    #[test]
    fn a_sink_resolved_ledger_carries_its_source_in_the_raw_receipt_json() {
        use ctx_traits_core::procedure::session::SessionTitleSource;
        let path = scratch_session_path("sink-source-in-receipt-json");
        write_test_session(&path, "sink-source-in-receipt-json-run");
        record_session_title_from_sink(
            &path,
            SessionTitleSource::SinkGenerated,
            "Generated title".to_string(),
        )
        .expect("sink write");
        let raw = std::fs::read_to_string(path.as_std_path()).expect("read ledger bytes");
        assert!(
            raw.contains("\"source\": \"sink-generated\"")
                || raw.contains("\"source\":\"sink-generated\""),
            "the exact bytes a receipt/run-status reader loads must carry the resolved source: {raw}"
        );
    }

    #[test]
    fn late_auto_worker_write_after_sink_takeover_is_a_no_op() {
        use ctx_traits_core::procedure::session::SessionTitleSource;
        let path = scratch_session_path("late-auto-worker-no-op");
        write_test_session(&path, "late-auto-worker-no-op-run");
        assert_eq!(
            claim_session_title_attempt(&path, "owner-a").expect("claim"),
            SessionTitleClaim::Claimed { attempts: 1 }
        );
        record_session_title_from_sink(
            &path,
            SessionTitleSource::SinkGenerated,
            "Sink wins".to_string(),
        )
        .expect("sink write");
        // The stale auto worker (owner-a) delivers late, after the sink has
        // already resolved the title — its write must be a silent no-op,
        // never clobbering the sink's resolved state.
        let recorded = record_session_title(&path, "owner-a", "Late narrator title".to_string())
            .expect("record does not error");
        assert!(!recorded, "a late auto-worker write must be a no-op");
        let session = read_run_session(&path).expect("read session back");
        let state = session.provenance.session_title.unwrap();
        assert_eq!(state.resolved_title(), Some("Sink wins"));
        assert_eq!(
            state.resolved_source(),
            Some(SessionTitleSource::SinkGenerated)
        );
    }

    #[test]
    fn sanitize_session_title_strips_control_chars_ansi_and_clamps_to_sixty() {
        let dirty = format!(
            "\u{1b}[31mTitle\u{1b}[0m\twith\ncontrol\u{7}chars {}",
            "x".repeat(80)
        );
        let clean = sanitize_session_title(&dirty);
        assert!(!clean.contains('\u{1b}'));
        assert!(!clean.chars().any(|c| c.is_control()));
        assert!(!clean.contains('\n') && !clean.contains('\t'));
        assert_eq!(clean.chars().count(), SESSION_TITLE_DISPLAY_CLAMP);
    }

    #[test]
    fn sanitize_session_title_collapses_whitespace_to_single_line() {
        assert_eq!(
            sanitize_session_title("  Fixing   the\n\nbug  "),
            "Fixing the bug"
        );
    }

    #[test]
    fn failures_retry_until_budget_then_become_terminal() {
        let path = scratch_session_path("no-narrator");
        write_test_session(&path, "no-narrator-run");
        for attempt in 1..=SESSION_TITLE_ATTEMPT_LIMIT {
            let owner = format!("owner-{attempt}");
            assert_eq!(
                claim_session_title_attempt(&path, &owner).expect("claim"),
                SessionTitleClaim::Claimed { attempts: attempt }
            );
            assert!(
                record_session_title_failure(&path, &owner, "failed".to_string()).expect("failure")
            );
        }
        let session = read_run_session(&path).expect("read session back");
        assert!(matches!(
            session.provenance.session_title,
            Some(ctx_traits_core::procedure::session::SessionTitleState::Terminal { .. })
        ));
    }

    #[test]
    fn new_owner_terminalizes_an_abandoned_final_attempt() {
        let path = scratch_session_path("abandoned-final-attempt");
        write_test_session(&path, "abandoned-final-attempt-run");
        for attempt in 1..=SESSION_TITLE_ATTEMPT_LIMIT {
            let owner = format!("owner-{attempt}");
            assert_eq!(
                claim_session_title_attempt(&path, &owner).expect("claim"),
                SessionTitleClaim::Claimed { attempts: attempt }
            );
            if attempt < SESSION_TITLE_ATTEMPT_LIMIT {
                assert!(
                    record_session_title_failure(&path, &owner, "failed".to_string())
                        .expect("record failure")
                );
            }
        }

        assert_eq!(
            claim_session_title_attempt(&path, "owner-3").expect("refuse current owner"),
            SessionTitleClaim::NotClaimable
        );
        assert!(matches!(
            read_run_session(&path)
                .expect("read still in-flight")
                .provenance
                .session_title,
            Some(ctx_traits_core::procedure::session::SessionTitleState::InFlight {
                owner,
                attempts: SESSION_TITLE_ATTEMPT_LIMIT,
            }) if owner == "owner-3"
        ));
        assert_eq!(
            claim_session_title_attempt(&path, "owner-4").expect("lapse final attempt"),
            SessionTitleClaim::NotClaimable
        );
        let session = read_run_session(&path).expect("read terminal state");
        assert_eq!(
            session.provenance.session_title,
            Some(
                ctx_traits_core::procedure::session::SessionTitleState::Terminal {
                    attempts: SESSION_TITLE_ATTEMPT_LIMIT,
                    reason: "attempt-limit-exhausted".to_string(),
                }
            )
        );
    }
}

#[cfg(test)]
mod summary_sidecar_landmine_tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-summary-sidecar-landmine-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    // `<ledger>.json.summary.json` also ends in `.json`, so an
    // unfiltered `session_ledger_names` would list it as a second, phantom
    // session — and, worse, give every real id an "ambiguous prefix" twin in
    // `resolve_session_prefix_in_stores`.
    #[test]
    fn session_ledger_names_ignores_summary_sidecars_but_keeps_real_ledgers() {
        let store = scratch_dir("names");
        let id = format!("session-{}", "a".repeat(64));
        std::fs::write(store.join(format!("{id}.json")).as_std_path(), "{}").expect("write ledger");
        std::fs::write(
            store.join(format!("{id}.json.summary.json")).as_std_path(),
            "{}",
        )
        .expect("write sidecar");

        let names = session_ledger_names(&store).expect("list ledger names");
        assert_eq!(names, vec![format!("{id}.json")]);
    }

    #[test]
    fn resolve_session_path_prefix_stays_unambiguous_with_a_sidecar_present() {
        let store = scratch_dir("resolve");
        let id = format!("session-{}", "a".repeat(64));
        std::fs::write(store.join(format!("{id}.json")).as_std_path(), "{}").expect("write ledger");
        std::fs::write(
            store.join(format!("{id}.json.summary.json")).as_std_path(),
            "{}",
        )
        .expect("write sidecar");

        // A prefix of the ledger id must resolve uniquely, not error with
        // "ambiguous prefix" against the sidecar's own name.
        let prefix = &id[.."session-".len() + 12];
        let resolved =
            resolve_session_path(prefix, Some(store.as_str())).expect("resolves uniquely");
        assert_eq!(resolved, store.join(format!("{id}.json")));
    }
}

#[cfg(test)]
mod short_session_display_tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-short-session-display-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    #[test]
    fn twelve_hex_happy_path_with_no_collision() {
        let id = format!("session-{}", "a".repeat(64));
        let other = format!("session-{}", "b".repeat(64));
        let short = short_session_display(&id, &[id.clone(), other]);
        assert_eq!(short, format!("session-{}", "a".repeat(12)));
    }

    #[test]
    fn collision_extends_by_one_character() {
        let id = format!("session-aaaaaaaaaaaab{}", "0".repeat(50));
        let collider = format!("session-aaaaaaaaaaaac{}", "0".repeat(50));
        let short = short_session_display(&id, &[id.clone(), collider]);
        // The 12-char candidate ("aaaaaaaaaaaa") is a prefix of BOTH ids, so
        // it must extend until the 13th character (`b` vs `c`) disambiguates.
        assert_eq!(short, "session-aaaaaaaaaaaab");
    }

    #[test]
    fn non_hex_host_supplied_id_renders_in_full() {
        let id = "session-implement-FMSPmRRt".to_string();
        let short = short_session_display(&id, std::slice::from_ref(&id));
        assert_eq!(short, id);
    }

    #[test]
    fn short_id_never_equals_another_full_id() {
        // The 12-char candidate for `id` collides exactly with a short,
        // fully-hex OTHER id — extending must still happen even though the
        // naive prefix check on the 12-char slice alone would call this
        // "unique" against every id longer than 12 chars.
        let id = format!("session-abcdefabcdef{}", "0".repeat(52));
        let other = "session-abcdefabcdef".to_string();
        let short = short_session_display(&id, &[id.clone(), other.clone()]);
        assert_ne!(short, other);
    }

    #[test]
    fn ids_shorter_than_twelve_hex_render_in_full() {
        let id = "session-abc123".to_string();
        assert_eq!(short_session_display(&id, std::slice::from_ref(&id)), id);
    }

    #[test]
    fn short_form_round_trips_through_resolve_session_path() {
        let store = scratch_dir("roundtrip");
        let id = format!("session-{}", "a".repeat(64));
        let other = format!("session-{}", "b".repeat(64));
        std::fs::write(store.join(format!("{id}.json")).as_std_path(), "").expect("write ledger");
        std::fs::write(store.join(format!("{other}.json")).as_std_path(), "")
            .expect("write ledger");
        let short = short_session_display(&id, &[id.clone(), other]);
        assert_ne!(short, id, "the short form is genuinely shortened here");
        let resolved =
            resolve_session_path(&short, Some(store.as_str())).expect("resolves uniquely");
        assert_eq!(resolved, store.join(format!("{id}.json")));
    }
}

/// 0064: the tolerant park-report/feasibility mirrors deserialize a shape
/// matching `reviewVerdictSchema`/`blockerSchema`/`feasibilityVerdictSchema`
/// (`packages/agents/src/index.ts`, `packages/agents/src/feasibility.ts`)
/// field-for-field, including when optional fields are absent.
#[cfg(test)]
mod park_report_mirror_tests {
    use super::*;

    #[test]
    fn a_full_review_verdict_parses_with_its_blocker_field_names() {
        let value = serde_json::json!({
            "status": "revise",
            "blockers": [{
                "id": "leaky-cache",
                "where": "modules/io/src/cache.rs",
                "what": "cache never evicts",
                "root-cause": "no eviction policy",
                "required-fix": "bound the cache",
                "steps": [
                    {"step": "add an LRU cap", "status": "open", "evidence": ""},
                    {"step": "add a unit test", "status": "done", "evidence": "cache_evicts_test"},
                ],
                "done-when": "cache_evicts_test passes",
            }],
            "wall-id": "",
        });
        let entry: ParkReportEntry = serde_json::from_value(value).expect("parses");
        assert_eq!(entry.status, "revise");
        assert_eq!(entry.blockers.len(), 1);
        let blocker = &entry.blockers[0];
        assert_eq!(blocker.id, "leaky-cache");
        assert_eq!(blocker.location, "modules/io/src/cache.rs");
        assert_eq!(blocker.root_cause, "no eviction policy");
        assert_eq!(blocker.required_fix, "bound the cache");
        assert_eq!(blocker.done_when, "cache_evicts_test passes");
        assert_eq!(blocker.steps.len(), 2);
        assert!(blocker.is_open());
    }

    #[test]
    fn a_blocker_whose_every_step_is_done_is_not_open() {
        let value = serde_json::json!({
            "id": "x",
            "where": "",
            "what": "",
            "root-cause": "",
            "required-fix": "",
            "steps": [{"step": "a", "status": "done", "evidence": "e"}],
            "done-when": "",
        });
        let blocker: ParkBlocker = serde_json::from_value(value).expect("parses");
        assert!(!blocker.is_open());
    }

    #[test]
    fn a_blocker_report_missing_optional_fields_still_parses_tolerantly() {
        let value = serde_json::json!({
            "status": "revise",
            "blockers": [{"id": "bare", "what": "missing everything else"}],
        });
        let entry: ParkReportEntry = serde_json::from_value(value).expect("parses tolerantly");
        assert_eq!(entry.blockers.len(), 1);
        assert_eq!(entry.blockers[0].id, "bare");
        assert_eq!(entry.blockers[0].location, "");
        assert!(entry.blockers[0].steps.is_empty());
        // No steps recorded at all is still "open" — never treated as
        // already resolved for lack of a step list.
        assert!(entry.blockers[0].is_open());
    }

    #[test]
    fn an_unparseable_report_shape_never_panics() {
        let value = serde_json::json!("not an object at all");
        let entry: Option<ParkReportEntry> = serde_json::from_value(value).ok();
        assert!(entry.is_none());
    }

    #[test]
    fn a_feasibility_verdict_parses_with_its_field_names() {
        let value = serde_json::json!({
            "verdict": "oversized",
            "evidence": "checked every referenced file",
            "missing": ["a shared abstraction", "a smaller scope"],
            "owner-action": "split the task",
        });
        let verdict: FeasibilityVerdict = serde_json::from_value(value).expect("parses");
        assert_eq!(verdict.verdict, "oversized");
        assert_eq!(verdict.missing.len(), 2);
        assert_eq!(verdict.owner_action, "split the task");
    }
}
