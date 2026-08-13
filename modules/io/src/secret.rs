//! Redacted in-memory credential wrapper (0069's channel-secrets doctrine,
//! made a type). Config stores only the NAME of the environment variable
//! holding a credential; the VALUE, once resolved, lives in a [`Secret`] —
//! whose every incidental surface renders `[REDACTED]` — instead of a bare
//! `String` that any `{:?}` in a log line, panic payload, error context, or
//! serialized report would happily print.

use std::fmt;

/// A credential ctx holds in memory (an API key resolved from the
/// environment). `Debug`, `Display`, and serde serialization all render
/// `[REDACTED]`; the one deliberate read is [`Secret::expose`], so every
/// place the raw value escapes is greppable. There is deliberately no
/// `Deserialize` and no `PartialEq<str>`: secrets enter through the
/// environment, never through a document, and are spent, never compared.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The raw credential — call only where the value is spent (an
    /// `Authorization`/`x-api-key` header), never to log or store it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl serde::Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn every_incidental_surface_redacts_and_only_expose_reveals() {
        let secret = Secret::new("sk-live-do-not-print".to_string());
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(secret.to_string(), "[REDACTED]");
        assert_eq!(
            serde_json::to_string(&secret).expect("serializes"),
            "\"[REDACTED]\""
        );
        assert_eq!(secret.expose(), "sk-live-do-not-print");
        assert!(!secret.is_empty());
        assert!(Secret::new(String::new()).is_empty());
    }
}
