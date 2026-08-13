//! IO boundary errors.
//!
//! The crate-root [`enum@Error`] is pure composition: side effects live in
//! `environment`, persisted data parsing lives in `parse`, and layout validation
//! lives in `layout`.

use thiserror::Error;

/// IO errors composed from IO product-domain error enums.
#[derive(Debug, Error)]
pub enum Error {
    /// A Git operation exhausted only the narrowly classified repository
    /// lock/control retry policy. Callers that own a larger operation can
    /// safely consume this typed race without parsing Git diagnostics.
    #[error("transient git lock/control retry exhausted: {reason:?}")]
    TransientGitLock {
        reason: crate::worktree::TransientLockReason,
    },

    #[error(transparent)]
    Environment(#[from] crate::environment::Error),

    #[error(transparent)]
    Parse(#[from] crate::parse::Error),

    #[error(transparent)]
    Layout(#[from] crate::layout::Error),

    #[error(transparent)]
    Export(#[from] crate::export::Error),

    #[error(transparent)]
    Core(#[from] ctx_traits_core::Error),

    #[error(transparent)]
    Registry(#[from] crate::registry::Error),

    #[error(transparent)]
    GitFetch(#[from] crate::git_fetch::Error),

    #[error(transparent)]
    Publish(#[from] crate::publish::Error),

    /// A genuine usage error: not a filesystem/process/parse effect, just a
    /// caller invoking a command in a way it does not support. Distinct from
    /// `Environment::Filesystem` so callers stop wearing a `path:` costume
    /// (a session-ID prefix, an argv token) that is not a filesystem path.
    #[error("{message}")]
    Usage { message: String },

    /// A primary error whose own rollback/restore attempt also failed,
    /// leaving artifacts the caller must be told about by name (e.g. a
    /// timestamped backup directory) rather than silently. The primary
    /// error's full text prints first, one indented note per un-restored
    /// artifact follows.
    #[error("{}", format_with_rollback_notes(primary, notes))]
    RollbackIncomplete {
        #[source]
        primary: Box<Error>,
        notes: Vec<String>,
    },
}

fn format_with_rollback_notes(primary: &Error, notes: &[String]) -> String {
    let mut out = primary.to_string();
    for note in notes {
        out.push('\n');
        out.push_str("  ");
        out.push_str(note);
    }
    out
}

/// Wrap `primary` with notes naming artifacts a failed rollback could not
/// restore, so the caller never silently sits on an un-restored backup.
pub fn with_rollback_notes(primary: impl Into<Error>, notes: Vec<String>) -> Error {
    Error::RollbackIncomplete {
        primary: Box::new(primary.into()),
        notes,
    }
}

pub type Result<T> = std::result::Result<T, Error>;
