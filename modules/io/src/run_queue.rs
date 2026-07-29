//! Role work queue over persisted run sessions.
//!
//! Ordering is an IO-layer dispatch heuristic: older session-file mtimes are
//! served first, with session ID as the tie-break. A fetched-but-never-submitted
//! frame leaves the mtime unchanged, so a poisoned session can stay at the head
//! of the v1 queue until some caller advances or edits it.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextRequest<'a> {
    /// Store-scoped queue pulls (no `session`) always require an explicit
    /// role. Session-scoped pulls (P421) may omit it: the role is inferred
    /// from the persisted frame's assigned agent.
    pub agent: Option<&'a str>,
    pub session: Option<&'a str>,
    pub session_store: Option<&'a str>,
    pub wait_seconds: u64,
    pub peek: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunNextOutput {
    pub kind: RunNextKind,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<RunQueueCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Box<ctx_traits_core::procedure::runtime::SequenceFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunQueueTerminal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction_context: Option<ctx_traits_core::procedure::runtime::StepValidationReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum RunNextKind {
    Frame,
    NoPendingWork,
    Terminal,
    Peek,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunQueueCandidate {
    pub queue_position: usize,
    pub session_id: String,
    pub run_id: String,
    pub trait_id: String,
    pub status: ctx_traits_core::procedure::session::Status,
    pub current_agent: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RunQueueTerminal {
    pub session_id: String,
    pub run_id: String,
    pub trait_id: String,
    pub status: ctx_traits_core::procedure::session::Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct CandidateRecord {
    candidate: RunQueueCandidate,
    modified: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitCacheKey {
    path: String,
    source_digest: String,
}

type TraitCache = HashMap<TraitCacheKey, crate::run::LoadedTrait>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SessionProbe {
    session_id: ctx_traits_core::procedure::session::SessionId,
    run_id: ctx_traits_core::procedure::run::Id,
    trait_id: String,
    status: ctx_traits_core::procedure::session::Status,
    #[serde(default, rename = "current-agent")]
    current_agent: Option<ctx_traits_core::procedure::runtime::AgentRole>,
}

pub fn next(request: NextRequest<'_>) -> crate::Result<RunNextOutput> {
    if request.session.is_none() && request.agent.is_none() {
        return Err(crate::Error::Usage {
            message: "store-scoped `next` requires --agent; role inference only applies to a \
                 session-scoped pull (--session)"
                .to_string(),
        });
    }
    let agent = request.agent.map(normalize_agent).transpose()?;
    let started = Instant::now();
    loop {
        let marker = poll_marker(&request)?;
        let mut trait_cache = TraitCache::new();
        let output = next_once(agent.as_deref(), &request, &mut trait_cache)?;
        if request.peek
            || output.kind != RunNextKind::NoPendingWork
            || started.elapsed() >= Duration::from_secs(request.wait_seconds)
        {
            return Ok(output);
        }
        if !wait_for_marker_change(&request, marker, started)? {
            return Ok(output);
        }
    }
}

fn next_once(
    agent: Option<&str>,
    request: &NextRequest<'_>,
    trait_cache: &mut TraitCache,
) -> crate::Result<RunNextOutput> {
    if let Some(session) = request.session {
        return session_scoped(agent, session, request, trait_cache);
    }
    // Store-scoped pulls always carry an explicit role (checked in `next`).
    let agent = agent.expect("store-scoped next requires --agent");
    let (candidates, warnings) = scan_store(agent, request.session_store)?;
    if request.peek {
        return Ok(RunNextOutput {
            kind: RunNextKind::Peek,
            agent: agent.to_string(),
            candidates,
            frame: None,
            terminal: None,
            correction_context: None,
            warnings,
        });
    }
    select_candidate(agent, candidates, warnings, trait_cache)
}

/// Resolve `next` scoped to one session (P421). `agent` is optional here:
/// when absent, the role is inferred from the persisted frame's assigned
/// agent so a cold agent operating one session does not need to already
/// know its own role name to keep polling it.
fn session_scoped(
    agent: Option<&str>,
    session: &str,
    request: &NextRequest<'_>,
    trait_cache: &mut TraitCache,
) -> crate::Result<RunNextOutput> {
    let path = crate::run_session::resolve_session_path(session, request.session_store)?;
    let mut warnings = Vec::new();
    let loaded = match load_refreshed(&path, trait_cache) {
        Ok(session) => session,
        Err(err) => {
            warnings.push(format!("{}: {err}", path));
            let agent = agent.unwrap_or("unknown");
            return Ok(no_pending(agent, Vec::new(), warnings));
        }
    };
    // A terminal session carries no pending frame/role assignment to infer
    // from; report it before role inference so `--agent`-less polling still
    // observes completion instead of failing closed on a role it will never
    // need.
    if matches!(
        loaded.status,
        ctx_traits_core::procedure::session::Status::Completed
            | ctx_traits_core::procedure::session::Status::Failed
    ) {
        return Ok(terminal(agent.unwrap_or("unknown"), loaded));
    }
    let agent = match agent {
        Some(agent) => agent.to_string(),
        None => match inferred_role(&loaded) {
            Some(role) => role,
            None => {
                return Err(crate::environment::Error::Filesystem {
                    path: session.to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "session-scoped `next` without --agent could not infer a role: the \
                         session has no persisted frame assigned to an agent",
                    ),
                }
                .into());
            }
        },
    };
    let agent = agent.as_str();
    if frame_belongs_to(agent, &loaded) {
        return Ok(frame(agent, loaded, Vec::new(), warnings));
    }
    Ok(no_pending(agent, Vec::new(), warnings))
}

/// The role a session-scoped `next` without `--agent` should serve: the
/// currently assigned agent on the persisted next frame, falling back to
/// `current_agent` when the frame itself carries no assignment.
fn inferred_role(session: &ctx_traits_core::procedure::session::Session) -> Option<String> {
    session
        .next_frame
        .as_ref()
        .and_then(|frame| frame.assigned_agent.as_ref())
        .or(session.current_agent.as_ref())
        .map(|role| role.role.clone())
}

fn select_candidate(
    agent: &str,
    candidates: Vec<RunQueueCandidate>,
    mut warnings: Vec<String>,
    trait_cache: &mut TraitCache,
) -> crate::Result<RunNextOutput> {
    for candidate in &candidates {
        let path = Utf8PathBuf::from(&candidate.path);
        match load_refreshed(&path, trait_cache) {
            Ok(session) if frame_belongs_to(agent, &session) => {
                return Ok(frame(agent, session, candidates, warnings));
            }
            Ok(_) => {}
            Err(err) => warnings.push(format!("{}: {err}", candidate.path)),
        }
    }
    Ok(no_pending(agent, candidates, warnings))
}

fn load_refreshed(
    path: &Utf8Path,
    trait_cache: &mut TraitCache,
) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    let session = crate::run_session::read_run_session(path)?;
    let loaded = load_trait_for_session_cached(trait_cache, &session)?;
    crate::run::validate_pinned_approval(&session, &loaded)?;
    ctx_traits_core::procedure::session::refresh_run_session(&loaded.trait_ref, session)
        .map_err(crate::Error::Core)
}

fn load_trait_for_session_cached(
    trait_cache: &mut TraitCache,
    session: &ctx_traits_core::procedure::session::Session,
) -> crate::Result<crate::run::LoadedTrait> {
    let key = session
        .provenance
        .trait_source
        .as_ref()
        .zip(session.source_digest.as_ref())
        .map(|(source, source_digest)| TraitCacheKey {
            path: source.path.clone(),
            source_digest: source_digest.to_string(),
        });
    if let Some(key) = key {
        if let Some(loaded) = trait_cache.get(&key) {
            verify_cached_trait_matches_session(loaded, session)?;
            return Ok(loaded.clone());
        }
        let loaded = crate::run::load_trait_for_session(None, None, session, "run-next")?;
        trait_cache.insert(key, loaded.clone());
        Ok(loaded)
    } else {
        crate::run::load_trait_for_session(None, None, session, "run-next")
    }
}

fn verify_cached_trait_matches_session(
    loaded: &crate::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
) -> crate::Result<()> {
    if loaded.trait_ref.id.as_str() != session.trait_id {
        return Err(run_next_source_error(
            "run-next.trait-id",
            format!(
                "stale run-session source: loaded trait {} does not match session trait {}",
                loaded.trait_ref.id.as_str(),
                session.trait_id
            ),
        ));
    }
    if let Some(expected) = session.source_digest.as_deref()
        && loaded.source_digest != expected
    {
        return Err(run_next_source_error(
            "run-next.source-digest",
            "stale run-session source: loaded source digest does not match session ledger",
        ));
    }
    if let Some(expected) = session.canonical_digest.as_deref()
        && loaded.canonical_digest != expected
    {
        return Err(run_next_source_error(
            "run-next.canonical-digest",
            "stale run-session source: loaded canonical digest does not match session ledger",
        ));
    }
    Ok(())
}

fn run_next_source_error(field_path: &str, message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: message.into(),
        }
        .into(),
    )
}

fn frame_belongs_to(agent: &str, session: &ctx_traits_core::procedure::session::Session) -> bool {
    matches!(
        session.status,
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput
            | ctx_traits_core::procedure::session::Status::Rejected
    ) && session
        .current_agent
        .as_ref()
        .is_some_and(|current| current.role == agent)
        && session
            .next_frame
            .as_ref()
            .and_then(|frame| frame.assigned_agent.as_ref())
            .is_some_and(|current| current.role == agent)
}

fn frame(
    agent: &str,
    session: ctx_traits_core::procedure::session::Session,
    candidates: Vec<RunQueueCandidate>,
    warnings: Vec<String>,
) -> RunNextOutput {
    RunNextOutput {
        kind: RunNextKind::Frame,
        agent: agent.to_string(),
        candidates,
        frame: session.next_frame,
        terminal: None,
        correction_context: session.last_validation_report,
        warnings,
    }
}

fn poll_marker(request: &NextRequest<'_>) -> crate::Result<Option<SystemTime>> {
    if let Some(session) = request.session {
        let path = crate::run_session::resolve_session_path(session, request.session_store)?;
        return modified_marker(&path);
    }
    let root = match request.session_store {
        Some(store) => Utf8PathBuf::from(store),
        None => crate::run_session::default_session_store()?,
    };
    modified_marker(&root)
}

fn modified_marker(path: &Utf8Path) -> crate::Result<Option<SystemTime>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.modified().ok()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

fn wait_for_marker_change(
    request: &NextRequest<'_>,
    marker: Option<SystemTime>,
    started: Instant,
) -> crate::Result<bool> {
    let wait = Duration::from_secs(request.wait_seconds);
    loop {
        let elapsed = started.elapsed();
        if elapsed >= wait {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(500).min(wait - elapsed));
        let next_marker = poll_marker(request)?;
        if marker.is_none() || next_marker != marker {
            return Ok(true);
        }
    }
}

fn terminal(agent: &str, session: ctx_traits_core::procedure::session::Session) -> RunNextOutput {
    RunNextOutput {
        kind: RunNextKind::Terminal,
        agent: agent.to_string(),
        candidates: Vec::new(),
        frame: None,
        terminal: Some(RunQueueTerminal {
            session_id: session.session_id.as_str().to_string(),
            run_id: session.run_id.as_str().to_string(),
            trait_id: session.trait_id,
            status: session.status,
            stop_reason: session.stop_reason.map(|reason| format!("{reason:?}")),
        }),
        correction_context: None,
        warnings: Vec::new(),
    }
}

fn no_pending(
    agent: &str,
    candidates: Vec<RunQueueCandidate>,
    warnings: Vec<String>,
) -> RunNextOutput {
    RunNextOutput {
        kind: RunNextKind::NoPendingWork,
        agent: agent.to_string(),
        candidates,
        frame: None,
        terminal: None,
        correction_context: None,
        warnings,
    }
}

fn scan_store(
    agent: &str,
    session_store: Option<&str>,
) -> crate::Result<(Vec<RunQueueCandidate>, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut records = Vec::new();
    for path in crate::run_session::session_store_paths(session_store)? {
        match probe_candidate(agent, &path) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(err) => warnings.push(format!("{}: {err}", path)),
        }
    }
    records.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.candidate.session_id.cmp(&right.candidate.session_id))
    });
    let candidates = records
        .into_iter()
        .enumerate()
        .map(|(index, mut record)| {
            record.candidate.queue_position = index + 1;
            record.candidate
        })
        .collect();
    Ok((candidates, warnings))
}

fn probe_candidate(agent: &str, path: &Utf8Path) -> crate::Result<Option<CandidateRecord>> {
    let metadata =
        std::fs::metadata(path).map_err(|source| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        })?;
    let text = crate::read::read_text(path)?;
    let probe: SessionProbe =
        serde_json::from_str(&text).map_err(|source| crate::parse::Error::JsonDeserialize {
            context: format!("parse run-session probe at {path}"),
            source,
        })?;
    if !matches!(
        probe.status,
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput
            | ctx_traits_core::procedure::session::Status::Rejected
    ) {
        return Ok(None);
    }
    let Some(current_agent) = probe.current_agent else {
        return Ok(None);
    };
    if current_agent.role != agent {
        return Ok(None);
    }
    Ok(Some(CandidateRecord {
        candidate: RunQueueCandidate {
            queue_position: 0,
            session_id: probe.session_id.as_str().to_string(),
            run_id: probe.run_id.as_str().to_string(),
            trait_id: probe.trait_id,
            status: probe.status,
            current_agent: current_agent.role,
            path: path.to_string(),
        },
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    }))
}

fn normalize_agent(agent: &str) -> crate::Result<String> {
    let agent = agent.strip_prefix("agent:").unwrap_or(agent);
    if agent.is_empty()
        || agent.contains('/')
        || agent.contains('\\')
        || agent.contains("..")
        || !agent
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(crate::Error::Usage {
            message: "agent role must contain only letters, digits, '-' or '_'".to_string(),
        });
    }
    Ok(agent.to_string())
}
