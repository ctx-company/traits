//! Capability-domain validation errors.

use thiserror::Error;

/// Capability errors surfaced by pure core planning.
#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported capability: {capability}")]
    Unsupported { capability: String },
}
