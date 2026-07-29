//! Session context ledger: track context injection estimates.
//!
//! Ledger entries are operational evidence only. They are separate from
//! procedure run ledger slot states. Ledger presence is not proof of model
//! memory — it records what was planned or supplied, not what was retained.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;

// ---------------------------------------------------------------------------
// Ledger state
// ---------------------------------------------------------------------------

/// State of context-ledger persistence for a session.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum State {
    /// No persisted context ledger was found for this session.
    Missing,
    /// Context-ledger persistence is not implemented for this phase.
    Unsupported,
    /// A persisted context ledger was loaded successfully.
    Loaded,
}

impl State {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Stale reason
// ---------------------------------------------------------------------------

/// Why a context ledger entry is stale.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum StaleReason {
    DigestChanged,
    MissingHostKey,
    LoadLevelChanged,
}

impl StaleReason {
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Ledger entry
// ---------------------------------------------------------------------------

/// One context ledger entry tracking a single injected trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Entry {
    pub session_id: String,
    pub trait_id: String,
    pub version: String,
    pub source_digest: Digest,
    pub render_profile: String,
    pub load_level: String,
    pub content_digest: Digest,
    pub approximate_tokens: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injected_turn: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,

    pub stale: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<StaleReason>,
}

/// A session context ledger, persisted one-per-host-key by
/// `ctx_traits_io::context_ledger` (P498). `session_id` predates the P498
/// host-key model and is kept for backward document compatibility; new
/// callers key persistence and reconciliation off `entry.host-key`
/// (per-entry) instead, since it is the field `reconcile` actually compares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Ledger {
    pub session_id: String,

    #[serde(default, rename = "entry", skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,

    /// The reason recorded by the most recent `context clear` on this
    /// ledger file (`compact` / `clear` / `fork`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_reason: Option<String>,
}

impl Ledger {
    /// Create a new empty ledger for the given session.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            entries: Vec::new(),
            last_cleared_reason: None,
        }
    }

    /// Mark an entry stale with a reason.
    pub fn mark_stale(&mut self, trait_id: &str, reason: StaleReason) {
        for entry in &mut self.entries {
            if entry.trait_id == trait_id {
                entry.stale = true;
                entry.stale_reason = Some(reason.clone());
            }
        }
    }

    /// Count of stale entries.
    pub fn stale_count(&self) -> usize {
        self.entries.iter().filter(|e| e.stale).count()
    }

    /// Count of fresh entries.
    pub fn fresh_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.stale).count()
    }

    /// Total approximate tokens across all entries.
    pub fn total_tokens(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.approximate_tokens)
            .fold(0u64, |acc, n| acc.saturating_add(n))
    }

    /// Reconcile entries against the current render's model-view digests and
    /// load levels.
    ///
    /// Entries whose model-view content digest differs from the current
    /// render are marked `DigestChanged` (P498 decision 3: the model-view
    /// digest — the digest of the bytes actually injected — is the sole
    /// freshness key; `Entry.source_digest` remains recorded as provenance
    /// only and is never compared here). Entries whose recorded load level
    /// differs from the level actually rendered are marked
    /// `LoadLevelChanged`. Entries whose host key is absent OR differs from
    /// `expected_host_key` are marked `MissingHostKey` — this is the P498 (c)
    /// fix: the prior implementation tested absence only, so a *wrong* host
    /// key reconciled clean.
    pub fn reconcile(&mut self, current: &[CurrentRender], expected_host_key: Option<&str>) {
        let current_map: std::collections::BTreeMap<&str, &CurrentRender> = current
            .iter()
            .map(|render| (render.trait_id.as_str(), render))
            .collect();

        for entry in &mut self.entries {
            if entry.stale {
                continue;
            }

            if let Some(render) = current_map.get(entry.trait_id.as_str()) {
                if entry.load_level != render.load_level {
                    entry.stale = true;
                    entry.stale_reason = Some(StaleReason::LoadLevelChanged);
                    continue;
                }
                if entry.content_digest != render.model_view_digest {
                    entry.stale = true;
                    entry.stale_reason = Some(StaleReason::DigestChanged);
                    continue;
                }
            }

            if let Some(expected) = expected_host_key
                && entry.host_key.as_deref() != Some(expected)
            {
                entry.stale = true;
                entry.stale_reason = Some(StaleReason::MissingHostKey);
            }
        }
    }

    /// Insert a new entry or replace the existing entry for the same trait
    /// id (the `--commit` write path for `context plan`).
    pub fn upsert(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.trait_id == entry.trait_id)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    /// Drop every entry (the `context clear` semantics: the host session's
    /// context no longer exists, so the next `plan` sees a clean ledger and
    /// returns `inject`, not `reinject` — see P498 decision 5). Returns the
    /// number of entries removed.
    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }
}

// ---------------------------------------------------------------------------
// Plan actions (`ctx traits context plan`)
// ---------------------------------------------------------------------------

/// One trait's current render, as actually produced by this process's render
/// path (`ctx traits prompt`'s render path — see P498 decision 3/4: the
/// freshness key is the model-view content digest, and the recorded load
/// level is the level actually rendered, never the resolver's nominal
/// pricing decision).
#[derive(Debug, Clone)]
pub struct CurrentRender {
    pub trait_id: String,
    pub model_view_digest: Digest,
    pub load_level: String,
}

/// What `context plan` should tell an adapter to do for one selected trait.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Action {
    /// No fresh ledger entry exists for this trait under this host key: inject.
    Inject,
    /// A fresh ledger entry already covers this exact render: skip.
    SkipFresh,
    /// A ledger entry exists but is stale: inject again.
    Reinject,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One `context plan` decision for a single trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Decision {
    pub trait_id: String,
    pub action: Action,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<StaleReason>,
}

/// Pure decision function: given the persisted `ledger`, the traits actually
/// selected and rendered this invocation (`current`), and this host
/// session's host key, decide `inject`/`skip-fresh`/`reinject` for each
/// selected trait. Touches no disk and renders nothing — `ledger` is
/// reconciled against `current` on a private clone so callers keep the
/// original ledger for their own `--commit` upsert pass.
pub fn plan_actions(ledger: &Ledger, current: &[CurrentRender], host_key: &str) -> Vec<Decision> {
    let mut reconciled = ledger.clone();
    reconciled.reconcile(current, Some(host_key));

    current
        .iter()
        .map(|render| {
            match reconciled
                .entries
                .iter()
                .find(|entry| entry.trait_id == render.trait_id)
            {
                None => Decision {
                    trait_id: render.trait_id.clone(),
                    action: Action::Inject,
                    stale_reason: None,
                },
                Some(entry) if entry.stale => Decision {
                    trait_id: render.trait_id.clone(),
                    action: Action::Reinject,
                    stale_reason: entry.stale_reason.clone(),
                },
                Some(_) => Decision {
                    trait_id: render.trait_id.clone(),
                    action: Action::SkipFresh,
                    stale_reason: None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(trait_id: &str, digest: &str, load_level: &str, host_key: Option<&str>) -> Entry {
        Entry {
            session_id: "s".to_string(),
            trait_id: trait_id.to_string(),
            version: "1.0.0".to_string(),
            source_digest: Digest::source("source"),
            render_profile: "agent-skills".to_string(),
            load_level: load_level.to_string(),
            content_digest: Digest::source(digest),
            approximate_tokens: 100,
            injected_turn: None,
            host_key: host_key.map(str::to_string),
            stale: false,
            stale_reason: None,
        }
    }

    fn render(trait_id: &str, digest: &str, load_level: &str) -> CurrentRender {
        CurrentRender {
            trait_id: trait_id.to_string(),
            model_view_digest: Digest::source(digest),
            load_level: load_level.to_string(),
        }
    }

    #[test]
    fn missing_entry_injects() {
        let ledger = Ledger::new("s");
        let decisions = plan_actions(&ledger, &[render("t1", "d1", "full")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::Inject);
        assert_eq!(decisions[0].stale_reason, None);
    }

    #[test]
    fn fresh_entry_skips() {
        let mut ledger = Ledger::new("s");
        ledger
            .entries
            .push(entry("t1", "d1", "full", Some("claude-code:h1")));
        let decisions = plan_actions(&ledger, &[render("t1", "d1", "full")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::SkipFresh);
        assert_eq!(decisions[0].stale_reason, None);
    }

    #[test]
    fn digest_changed_reinjects() {
        let mut ledger = Ledger::new("s");
        ledger
            .entries
            .push(entry("t1", "d1", "full", Some("claude-code:h1")));
        let decisions = plan_actions(&ledger, &[render("t1", "d2", "full")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::Reinject);
        assert_eq!(decisions[0].stale_reason, Some(StaleReason::DigestChanged));
    }

    #[test]
    fn load_level_changed_reinjects() {
        let mut ledger = Ledger::new("s");
        ledger
            .entries
            .push(entry("t1", "d1", "full", Some("claude-code:h1")));
        let decisions = plan_actions(&ledger, &[render("t1", "d1", "summary")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::Reinject);
        assert_eq!(
            decisions[0].stale_reason,
            Some(StaleReason::LoadLevelChanged)
        );
    }

    #[test]
    fn wrong_host_key_reinjects_not_skips() {
        let mut ledger = Ledger::new("s");
        ledger
            .entries
            .push(entry("t1", "d1", "full", Some("claude-code:other")));
        let decisions = plan_actions(&ledger, &[render("t1", "d1", "full")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::Reinject);
        assert_eq!(decisions[0].stale_reason, Some(StaleReason::MissingHostKey));
    }

    #[test]
    fn missing_host_key_reinjects() {
        let mut ledger = Ledger::new("s");
        ledger.entries.push(entry("t1", "d1", "full", None));
        let decisions = plan_actions(&ledger, &[render("t1", "d1", "full")], "claude-code:h1");
        assert_eq!(decisions[0].action, Action::Reinject);
        assert_eq!(decisions[0].stale_reason, Some(StaleReason::MissingHostKey));
    }

    #[test]
    fn clear_drops_all_entries_and_reports_count() {
        let mut ledger = Ledger::new("s");
        ledger
            .entries
            .push(entry("t1", "d1", "full", Some("claude-code:h1")));
        ledger
            .entries
            .push(entry("t2", "d2", "full", Some("claude-code:h1")));
        assert_eq!(ledger.clear(), 2);
        assert!(ledger.entries.is_empty());
    }
}
