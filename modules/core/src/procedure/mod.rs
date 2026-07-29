//! Model-family validation errors.

pub mod activity;
pub mod run;
pub mod runtime;
pub mod session;
pub mod stats;
pub mod story;

pub use run::{Plan, TraitPlan};
pub use session::Session;

use thiserror::Error;

/// Errors owned by procedure run, runtime, and session validation.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid manifest at {field_path}: {message}")]
    InvalidField { field_path: String, message: String },

    #[error("shape mismatch at {field_path}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field_path: String,
        expected: String,
        actual: String,
    },

    #[error("invalid manifest at {field_path}: failed to serialize {subject}: {source}")]
    Serialization {
        field_path: String,
        subject: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    pub(crate) fn invalid_field(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field_path: field_path.into(),
            message: message.into(),
        }
    }
}

pub(crate) fn invalid_field(
    field_path: impl Into<String>,
    message: impl Into<String>,
) -> crate::Error {
    Error::invalid_field(field_path, message).into()
}

pub(crate) fn serialization(
    field_path: impl Into<String>,
    subject: &'static str,
    source: serde_json::Error,
) -> crate::Error {
    Error::Serialization {
        field_path: field_path.into(),
        subject,
        source,
    }
    .into()
}
