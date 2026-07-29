//! Pure parse and serialization errors for text/model boundaries.
//!
//! `parse` owns failures while translating text or serde values to and from
//! Rust models. `encoding` still owns format selection and preflight manifest
//! shape checks that are not direct decoder/encoder failures.

use std::error::Error as StdError;
use std::fmt;

/// Pure text/model parse and serialization failures.
#[derive(Debug)]
pub enum Error {
    // Decode: authored text or serde values into normalized models.
    TomlDecode {
        field_path: String,
        source: toml::de::Error,
    },
    JsonDecode {
        field_path: String,
        source: serde_json::Error,
    },
    YamlDecode {
        field_path: String,
        source: serde_yaml::Error,
    },

    // Encode: normalized models into authored or canonical text.
    TomlEncode {
        field_path: String,
        source: toml::ser::Error,
    },
    JsonEncode {
        field_path: String,
        source: serde_json::Error,
    },
    YamlEncode {
        field_path: String,
        source: serde_yaml::Error,
    },
}

impl Error {
    pub(crate) fn toml_decode(field_path: impl Into<String>, source: toml::de::Error) -> Self {
        Self::TomlDecode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn json_decode(field_path: impl Into<String>, source: serde_json::Error) -> Self {
        Self::JsonDecode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn yaml_decode(field_path: impl Into<String>, source: serde_yaml::Error) -> Self {
        Self::YamlDecode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn toml_encode(field_path: impl Into<String>, source: toml::ser::Error) -> Self {
        Self::TomlEncode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn json_encode(field_path: impl Into<String>, source: serde_json::Error) -> Self {
        Self::JsonEncode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn yaml_encode(field_path: impl Into<String>, source: serde_yaml::Error) -> Self {
        Self::YamlEncode {
            field_path: field_path.into(),
            source,
        }
    }

    pub(crate) fn detail_field_path(&self) -> String {
        let message = self.source_message();
        message
            .strip_prefix("duplicate prompt key at ")
            .map_or_else(|| self.field_path().to_string(), ToString::to_string)
    }

    pub(crate) fn detail_message(&self) -> String {
        let message = self.source_message();
        if message.starts_with("duplicate prompt key at ") {
            "duplicate prompt key".to_string()
        } else {
            message
        }
    }

    pub(crate) fn is_shape_mismatch(&self) -> bool {
        let message = self.source_message();
        message.contains("invalid type") || message.contains("expected")
    }

    fn field_path(&self) -> &str {
        match self {
            Self::TomlDecode { field_path, .. }
            | Self::JsonDecode { field_path, .. }
            | Self::YamlDecode { field_path, .. }
            | Self::TomlEncode { field_path, .. }
            | Self::JsonEncode { field_path, .. }
            | Self::YamlEncode { field_path, .. } => field_path,
        }
    }

    fn source_message(&self) -> String {
        match self {
            Self::TomlDecode { source, .. } => source.to_string(),
            Self::JsonDecode { source, .. } | Self::JsonEncode { source, .. } => source.to_string(),
            Self::YamlDecode { source, .. } => source.to_string(),
            Self::TomlEncode { source, .. } => source.to_string(),
            Self::YamlEncode { source, .. } => source.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_shape_mismatch() {
            write!(
                f,
                "shape mismatch at {}: expected valid manifest shape, got {}",
                self.detail_field_path(),
                self.detail_message()
            )
        } else {
            write!(
                f,
                "invalid manifest at {}: {}",
                self.detail_field_path(),
                self.detail_message()
            )
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::TomlDecode { source, .. } => Some(source),
            Self::JsonDecode { source, .. } | Self::JsonEncode { source, .. } => Some(source),
            Self::YamlDecode { source, .. } => Some(source),
            Self::TomlEncode { source, .. } => Some(source),
            Self::YamlEncode { source, .. } => Some(source),
        }
    }
}
