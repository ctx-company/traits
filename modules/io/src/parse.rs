//! IO-edge parse and serialization errors.
//!
//! This domain owns persisted JSON/TOML decode and encode failures at the IO
//! boundary. It is separate from `ctx_traits_core::parse`, which is pure
//! text/model parsing for trait models.

use thiserror::Error;

/// JSON/TOML failures while reading or writing IO-owned files and envelopes.
#[derive(Debug, Error)]
pub enum Error {
    // JSON decode/encode.
    #[error("serialization error in {context}: {source}")]
    JsonDeserialize {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("serialization error in {context}: {source}")]
    JsonSerialize {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    // TOML decode/encode.
    #[error("toml decode error in {context}: {source}")]
    TomlDecode {
        context: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("toml encode error in {context}: {source}")]
    TomlEncode {
        context: String,
        #[source]
        source: toml::ser::Error,
    },

    // Surgical `toml_edit` manifest decode (preserves authored formatting).
    #[error("toml edit error in {context}: {source}")]
    TomlEditDecode {
        context: String,
        #[source]
        source: Box<toml_edit::TomlError>,
    },
}

impl Error {
    pub(crate) fn context(&self) -> &str {
        match self {
            Self::JsonDeserialize { context, .. }
            | Self::JsonSerialize { context, .. }
            | Self::TomlDecode { context, .. }
            | Self::TomlEncode { context, .. }
            | Self::TomlEditDecode { context, .. } => context,
        }
    }

    pub(crate) fn source_message(&self) -> String {
        match self {
            Self::JsonDeserialize { source, .. } | Self::JsonSerialize { source, .. } => {
                source.to_string()
            }
            Self::TomlDecode { source, .. } => source.to_string(),
            Self::TomlEncode { source, .. } => source.to_string(),
            Self::TomlEditDecode { source, .. } => source.to_string(),
        }
    }
}
