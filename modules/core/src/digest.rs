//! Digest helpers for source and canonical digests.
//!
//! Digests are typed values in `sha256:<hex>` format. Source digest captures
//! exact authored bytes; canonical digest captures the normalized model via
//! canonical JSON text. Both are computed independently so they can diverge
//! correctly.
//!
//! Digest code lives here rather than scattered across hash call sites.

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha256Digest, Sha256};
const DIGEST_PREFIX: &str = "sha256:";

/// Digest parsing and canonicalization errors.
#[derive(Debug)]
pub enum Error {
    MissingSha256Prefix {
        value: String,
    },

    InvalidHexLength {
        actual: usize,
    },

    NonHexCharacters {
        value: String,
    },

    Canonical {
        source: serde_json::Error,
    },

    Serialization {
        field_path: String,
        subject: &'static str,
        source: serde_json::Error,
    },
}

impl Error {
    pub(crate) fn detail_field_path(&self) -> &str {
        match self {
            Self::MissingSha256Prefix { .. }
            | Self::InvalidHexLength { .. }
            | Self::NonHexCharacters { .. } => "digest",
            Self::Canonical { .. } => "root",
            Self::Serialization { field_path, .. } => field_path,
        }
    }

    pub(crate) fn detail_message(&self) -> String {
        match self {
            Self::MissingSha256Prefix { value } => {
                format!("expected {DIGEST_PREFIX} prefix, got: {value}")
            }
            Self::InvalidHexLength { actual } => {
                format!("expected 64 hex chars after {DIGEST_PREFIX}, got {actual} chars")
            }
            Self::NonHexCharacters { .. } => "digest hex contains non-hex characters".to_string(),
            Self::Canonical { source } => source.to_string(),
            Self::Serialization {
                subject, source, ..
            } => format!("failed to serialize {subject}: {source}"),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid manifest at {}: {}",
            self.detail_field_path(),
            self.detail_message()
        )
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Canonical { source } | Self::Serialization { source, .. } => Some(source),
            Self::MissingSha256Prefix { .. }
            | Self::InvalidHexLength { .. }
            | Self::NonHexCharacters { .. } => None,
        }
    }
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

/// A typed digest value in `sha256:<hex>` format.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    derive_more::Display,
)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    /// Compute a digest from raw bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = hasher.finalize();
        let hex = hex::encode(hash);
        Self(format!("{DIGEST_PREFIX}{hex}"))
    }

    /// Compute a source digest from raw authored text bytes.
    pub fn source(text: &str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    /// Compute a canonical digest from canonical JSON text.
    pub fn canonical(text: &str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    /// Parse and validate a digest string in `sha256:<hex>` format.
    pub fn parse(s: &str) -> crate::Result<Self> {
        if !s.starts_with(DIGEST_PREFIX) {
            return Err(Error::MissingSha256Prefix {
                value: s.to_string(),
            }
            .into());
        }

        let hex = &s[DIGEST_PREFIX.len()..];
        if hex.len() != 64 {
            return Err(Error::InvalidHexLength { actual: hex.len() }.into());
        }

        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::NonHexCharacters {
                value: s.to_string(),
            }
            .into());
        }

        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Preserve legacy transparent-deserialize values inside core-only sanitizers.
    pub(crate) fn from_unvalidated(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::str::FromStr for Digest {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl std::ops::Deref for Digest {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<Digest> for String {
    fn from(value: Digest) -> Self {
        value.0
    }
}

/// Generate canonical JSON text from a normalized value.
///
/// `serde_json` object maps are sorted by key unless its `preserve_order`
/// feature is enabled. The regression test below guards that invariant.
pub fn canonical_json<T: Serialize>(value: &T) -> crate::Result<String> {
    let json_value = serde_json::to_value(value).map_err(|source| Error::Canonical { source })?;

    serde_json::to_string(&json_value).map_err(|source| Error::Canonical { source }.into())
}

/// Compute the canonical digest from normalized canonical JSON.
pub fn canonical_digest<T: Serialize>(value: &T) -> crate::Result<Digest> {
    let text = canonical_json(value)?;
    Ok(Digest::canonical(&text))
}

/// Compute the source digest from raw authored text.
pub fn source_digest(text: &str) -> Digest {
    Digest::source(text)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn canonical_json_output_is_key_sorted() {
        let value = json!({"b": 1, "a": {"d": 4, "c": 3}});

        let json = super::canonical_json(&value).expect("canonical JSON serializes");

        assert_eq!(json, r#"{"a":{"c":3,"d":4},"b":1}"#);
    }
}
