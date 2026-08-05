//! Task document schema v0.1: the canonical TOML shape for board tasks.
//!
//! Per `.internal/tasks/0059-canonical-toml-task-format.md`: identity,
//! status, and relations are typed fields; `content` and each step's
//! `content` are opaque prose that nothing in this module parses. Steps are
//! nested inside their owning document; a subtask is a separate document
//! linked by `relations.parent`.

pub mod markdown;

use serde::{Deserialize, Serialize};

/// Schema version this module reads and writes. Bumped only when the shape
/// changes; a document declaring any other value is rejected rather than
/// guessed at.
pub const SCHEMA_VERSION: &str = "0.1";

/// The closed set of stored statuses. Everything else (`blocked`,
/// `in-flight`, a parent's status derived from its children) is derived,
/// not stored, and belongs to 0060.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Ready,
    Done,
    Cancelled,
}

/// A single ordered work item inside one task. Has no independent
/// existence outside its owning document — a unit of work that stands on
/// its own is a subtask (`relations.parent`), not a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub content: String,
}

/// Typed relations to other task documents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Relations {
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "depends-on")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

impl Relations {
    fn is_empty(&self) -> bool {
        self.depends_on.is_empty() && self.parent.is_none()
    }
}

/// A canonical task document: the typed envelope plus opaque `content`.
///
/// `status` is `Option` because the two split-index parents (0010, 0051)
/// store none — their status is derived from children in 0060.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TaskDocument {
    #[serde(rename = "schema-version")]
    pub schema_version: String,
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Relations::is_empty")]
    pub relations: Relations,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
}

/// Parse a task document from TOML text, rejecting unknown fields and any
/// `schema-version` other than [`SCHEMA_VERSION`].
pub fn parse(text: &str) -> crate::Result<TaskDocument> {
    let document: TaskDocument =
        toml::from_str(text).map_err(|e| crate::parse::Error::toml_decode("task", e))?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(Error::UnsupportedSchemaVersion {
            found: document.schema_version.clone(),
            expected: SCHEMA_VERSION.to_string(),
        }
        .into());
    }
    Ok(document)
}

/// Import a markdown board file into a [`TaskDocument`].
pub fn import_markdown(text: &str) -> crate::Result<TaskDocument> {
    let document = markdown::import(text).map_err(Error::from)?;
    Ok(document)
}

/// Serialise a task document to canonical TOML text. `content` and step
/// bodies emit as `"""`/`'''` multi-line strings (toml_edit's default
/// string-repr chooser), never single-line strings with escaped newlines.
pub fn serialize(document: &TaskDocument) -> crate::Result<String> {
    toml_edit::ser::to_string_pretty(document).map_err(|e| Error::Serialize(e.to_string()).into())
}

/// Task-document-specific failures not already covered by [`crate::parse::Error`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported task schema-version {found:?}, expected {expected:?}")]
    UnsupportedSchemaVersion { found: String, expected: String },
    #[error("failed to serialise task document: {0}")]
    Serialize(String),
    #[error(transparent)]
    Markdown(#[from] markdown::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TaskDocument {
        TaskDocument {
            schema_version: SCHEMA_VERSION.to_string(),
            key: "0050".to_string(),
            title: "Root-cause the park-report ledger-contract failure".to_string(),
            status: Some(TaskStatus::Ready),
            raised: Some("2026-08-01".to_string()),
            content: "The contract prose — everything a run reads verbatim today.\n".to_string(),
            relations: Relations {
                depends_on: vec!["0049".to_string()],
                parent: Some("0010".to_string()),
            },
            steps: vec![Step {
                id: "s1".to_string(),
                title: "Repair the defect at its layer".to_string(),
                done: true,
                content: "Refresh `output_ports` after any projection.\n".to_string(),
            }],
        }
    }

    #[test]
    fn round_trips_through_serialize_and_parse() {
        let document = sample();
        let text = serialize(&document).expect("serialise");
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed, document);
    }

    #[test]
    fn round_trips_content_with_trailing_backslash_and_triple_quotes() {
        let mut document = sample();
        document.content = "a line ending in backslash\\\nand a \"\"\" sequence\n".to_string();
        let text = serialize(&document).expect("serialise");
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed.content, document.content);
    }

    #[test]
    fn optional_status_round_trips_as_absent() {
        let mut document = sample();
        document.status = None;
        document.relations = Relations::default();
        let text = serialize(&document).expect("serialise");
        assert!(!text.contains("status"));
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed.status, None);
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = format!(
            "schema-version = \"{SCHEMA_VERSION}\"\nkey = \"0001\"\ntitle = \"t\"\nbogus = true\n"
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let text = "schema-version = \"9.9\"\nkey = \"0001\"\ntitle = \"t\"\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn rejects_missing_schema_version() {
        let text = "key = \"0001\"\ntitle = \"t\"\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn closed_status_set_rejects_unknown_status_value() {
        let text = format!(
            "schema-version = \"{SCHEMA_VERSION}\"\nkey = \"0001\"\ntitle = \"t\"\nstatus = \"blocked\"\n"
        );
        assert!(parse(&text).is_err());
    }
}
