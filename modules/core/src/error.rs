//! Domain errors for the pure core.
//!
//! The crate-root [`enum@Error`] is pure composition: one transparent variant per
//! product domain. Concrete failures live inside their owning domain modules.

use thiserror::Error;

/// Pure core errors composed from product-domain error enums.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Reference(#[from] crate::reference::Error),

    #[error(transparent)]
    Digest(#[from] crate::digest::Error),

    #[error(transparent)]
    Parse(#[from] crate::parse::Error),

    #[error(transparent)]
    Encoding(#[from] crate::encoding::Error),

    #[error(transparent)]
    Manifest(#[from] crate::manifest::Error),

    #[error(transparent)]
    Schema(#[from] crate::schema::Error),

    #[error(transparent)]
    Capability(#[from] crate::capability::Error),

    #[error(transparent)]
    Shared(#[from] crate::shared::Error),

    #[error(transparent)]
    Trait(#[from] crate::r#trait::Error),

    #[error(transparent)]
    Model(#[from] crate::procedure::Error),

    #[error(transparent)]
    Distribution(#[from] crate::distribution::Error),

    #[error(transparent)]
    Migrate(#[from] crate::migrate::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
