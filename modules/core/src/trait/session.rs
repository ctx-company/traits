//! Session declarations: named session identities agents can share.
//!
//! `[[session]]` declares a shareable session identity. `[[agent]].session`
//! binds an agent to `session:<id>`, `per-frame`, or `persistent`. Multiple
//! agents bound to the same `session:<id>` share one ordered session
//! identity; a bare lifecycle value is always agent-local and never implies
//! sharing.
//!
//! This module validates the canonical shape only. Assignment-dependent
//! compatibility (same harness, compatible transport, serialized concurrent
//! access) is P328 runtime scope, not decode-time validation.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reference::{Kind, Reference};

use super::agent::{SESSION_LIFECYCLE_PER_FRAME, SESSION_LIFECYCLE_PERSISTENT};

/// A `[[session]]` declaration: a named, shareable session identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Session {
    /// Session identifier, referenced by agents as `session:<id>`.
    pub id: String,

    /// Optional human-readable description of what this session is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A parsed `[[agent]].session` binding value.
///
/// Shared abstraction for both decode-time validation
/// (`agent::validate_agent_session_bindings`) and runtime wiring (P328's
/// `drive::effective_session`), so the two never re-derive the same
/// `Reference::parse` arms independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionBinding {
    /// Agent-local, fresh session per frame, never shared.
    PerFrame,
    /// Agent-local, one session for the agent's whole lifetime, never shared.
    Persistent,
    /// Shared with every other agent bound to the same declared session id.
    Shared(String),
}

/// Parse a raw `[[agent]].session` value into a [`SessionBinding`].
///
/// Accepts `"per-frame"`, `"persistent"`, or a local, unqualified
/// `session:<id>` reference. `field_path` is carried into any error so
/// callers get the same precise diagnostics regardless of where the value
/// came from (e.g. `agent[2].session`). Does not check that a shared id
/// resolves to a declared `[[session]]`; callers that need that check
/// (decode-time validation) do it against their own `session_ids` set.
pub fn parse_session_binding(value: &str, field_path: &str) -> crate::Result<SessionBinding> {
    if value == SESSION_LIFECYCLE_PER_FRAME {
        return Ok(SessionBinding::PerFrame);
    }
    if value == SESSION_LIFECYCLE_PERSISTENT {
        return Ok(SessionBinding::Persistent);
    }

    let parsed = Reference::parse(value).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!(
            "invalid session binding {value:?}: expected \"per-frame\", \"persistent\", or a session:<id> reference"
        ),
    })?;

    if parsed.kind() != Kind::Session {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "session binding ref kind {:?} not allowed; expected session",
                parsed.kind()
            ),
        }
        .into());
    }

    if parsed.is_qualified() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "session binding ref must be local and unqualified".to_string(),
        }
        .into());
    }

    Ok(SessionBinding::Shared(parsed.id().to_string()))
}

/// Validate `[[session]]` declarations.
pub fn validate_sessions(sessions: &[Session]) -> crate::Result<()> {
    let mut seen_ids = BTreeSet::new();

    for (i, session) in sessions.iter().enumerate() {
        let id_path = format!("session[{i}].id");
        crate::shared::validate_slug_shape(&session.id, &id_path)?;
        if !seen_ids.insert(session.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate session id {:?}", session.id),
            }
            .into());
        }

        if session
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("session[{i}].description"),
                message: "must not be empty when supplied".to_string(),
            }
            .into());
        }
    }

    Ok(())
}
