//! CLI app-edge errors.
//!
//! This is the app edge: a catch-all `Command` variant is acceptable here
//! alongside structured wrappers for core and IO errors. `miette::Diagnostic`
//! provides human-friendly diagnostic output at the CLI boundary.

use thiserror::Error;

/// CLI errors: structured core/IO propagation plus a command-level catch-all.
#[derive(Debug, Error, miette::Diagnostic)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] Box<ctx_traits_core::Error>),

    #[error(transparent)]
    Io(#[from] Box<ctx_traits_io::Error>),

    #[error("json error in {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("{message}")]
    Command { message: String },

    /// A typed, non-zero exit whose report (JSON or plain) has already been
    /// printed by the caller — e.g. P460's combined run/drive+merge report.
    /// The generic `run()` dispatcher must map this straight to `exit_code`
    /// without printing `message` again.
    #[error("{message}")]
    AlreadyReported { message: String, exit_code: u8 },
}

/// P460 centralized non-zero exit statuses for the combined run/drive+merge
/// report, distinct from the generic `ExitCode::FAILURE` every other command
/// error still uses.
pub const EXIT_RUN_NOT_COMPLETED: u8 = 3;
pub const EXIT_MERGE_PARKED: u8 = 4;
/// A completed drive's automatic-landing attempt reached a terminal outcome
/// that is neither a landing nor a park — cross-process lock contention/
/// timeout, or a post-fast-forward cleanup/recovery failure. Distinct from
/// `EXIT_MERGE_PARKED` because those outcomes must never claim the park
/// invariant (branch and worktree left intact) that only an actual
/// `MergeStatus::Parked` promises.
pub const EXIT_MERGE_FAILED: u8 = 5;
/// The command ran to completion and reports blocking findings (`doctor`
/// critical findings, a non-`passed` `check` report) — distinct from exit 1,
/// which means the command could not run at all (load/parse/resolve
/// failure). Outside the 3/4/5 run/merge range and distinct from clap's own
/// usage-error exit 2.
pub const EXIT_FINDINGS: u8 = 6;
/// A run session that ended `Failed` — an authored `flow.error` terminal, a
/// `no-exit-reached` fall-through past every declared success exit, or any
/// other failure-shaped final state — on a run with no merge intent (0189).
/// Distinct from `EXIT_RUN_NOT_COMPLETED`: the drive completed; the RUN
/// declared itself failed.
pub const EXIT_RUN_FAILED: u8 = 7;

impl From<ctx_traits_core::Error> for Error {
    fn from(error: ctx_traits_core::Error) -> Self {
        Self::Core(Box::new(error))
    }
}

impl From<ctx_traits_io::Error> for Error {
    fn from(error: ctx_traits_io::Error) -> Self {
        Self::Io(Box::new(error))
    }
}

impl Error {
    pub(crate) fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Json {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
