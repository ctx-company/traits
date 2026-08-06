//! Task document schema v0.2: the canonical TOML shape for board tasks.
//!
//! Per `.internal/tasks/0059-canonical-toml-task-format.md` and
//! `0063.1-typed-task-fields-content-scope-validation.toml`: identity,
//! status, and relations are typed fields; `content`, `scope`,
//! `validation`, and each step's `content` are opaque prose that nothing in
//! this module parses beyond splitting it at import/migration time. Steps
//! are nested inside their owning document; a subtask is a separate
//! document linked by `relations.parent`.

pub mod graph;
pub mod markdown;
pub mod provider;

use serde::{Deserialize, Serialize};

/// Schema version this module reads and writes. Bumped only when the shape
/// changes; a document declaring any other value is rejected rather than
/// guessed at.
pub const SCHEMA_VERSION: &str = "0.2";

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

/// A declared check on a task document (0144): a command that, when it can
/// run and passes, is evidence the task is actually done — not just marked
/// so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// A regex the combined stdout+stderr must match for the check to pass.
    /// Without it, a zero exit code is the whole verdict — declaring this is
    /// how a test-filter check protects itself against matching nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

/// `[tasks] auto-close` (0144): how a task's declared checks translate into
/// a close action. Per-document `TaskDocument.auto_close` overrides the
/// `[tasks]` config leaf in either direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AutoClosePolicy {
    /// Checks (if any) strengthen the existing confirm proposal; nothing
    /// closes without the owner's key press.
    Confirm,
    /// All declared checks must run and pass for the task to close itself;
    /// any failure or un-runnable check downgrades to a proposal naming why.
    Checked,
    /// Closes on any hardened `MarkDone` candidate regardless of checks;
    /// checks that did run are recorded, an empty set records `unchecked`.
    Merge,
}

/// One check's recorded outcome, stored on [`Closure`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CheckRecord {
    pub name: String,
    pub command: String,
    pub outcome: CheckOutcome,
    /// Human-readable detail: exit code, matched/unmatched `expect`, timeout,
    /// or the un-runnable reason. Never implies stronger verification than
    /// what actually ran.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// The closed set of outcomes a single check can record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    /// The check could not be executed at all (timeout, over the count cap,
    /// worktree failure) — never silently treated as a pass or a fail.
    Unrunnable,
}

/// How a task closed, recorded on the archived document (0144). `checks`
/// empty under `AutoClosePolicy::Merge` renders as `unchecked` — proof that
/// no checks ran, not that they passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Closure {
    pub mode: AutoClosePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRecord>,
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

/// A canonical task document: the typed envelope plus three opaque prose
/// fields (0063.1). `status` is `Option` because the two split-index
/// parents (0010, 0051) store none — their status is derived from children
/// in 0060.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The narrative: what this task is and why it exists.
    #[serde(default)]
    pub content: String,
    /// Absorbs the house convention's `## Decisions` and `## Scope`
    /// sections: the rulings, what is in and out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// Absorbs the house convention's `## Watch` and `## Done when`
    /// sections: what to watch while doing the work and how to know it is
    /// done.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub validation: String,
    #[serde(default, skip_serializing_if = "Relations::is_empty")]
    pub relations: Relations,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// 0144: declared checks a run's proposal can be verified against
    /// before closing. Empty means "no checks declared" — the existing
    /// confirm-only flow, byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// 0144: per-document override of the `[tasks] auto-close` config leaf,
    /// in either direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_close: Option<AutoClosePolicy>,
    /// 0144: recorded proof of how this task closed. `None` for every task
    /// closed before this feature, and for any task still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<Closure>,
}

impl TaskDocument {
    /// Steps not yet marked `done`, in document order — the derived
    /// collection a step-iterating trait runs over (0062). Single-sourced
    /// here so the CLI/provider surface and any future runtime accessor
    /// agree on what "open" means.
    pub fn open_steps(&self) -> Vec<&Step> {
        self.steps.iter().filter(|step| !step.done).collect()
    }

    /// The three prose fields, labeled, in the one canonical rendering
    /// order (0063.1) — the single place this order is defined, shared by
    /// `ctx tasks show` and (0063.2) the dashboard's detail pane.
    pub fn prose_sections(&self) -> [(&'static str, &str); 3] {
        [
            ("content", &self.content),
            ("scope", &self.scope),
            ("validation", &self.validation),
        ]
    }
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
            closed: None,
            wall: None,
            origin: None,
            content: "The contract prose — everything a run reads verbatim today.\n".to_string(),
            scope: String::new(),
            validation: String::new(),
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
            checks: Vec::new(),
            auto_close: None,
            closure: None,
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
    fn round_trips_wall_origin_closed_scope_and_validation() {
        let mut document = sample();
        document.wall = Some("wall-42".to_string());
        document.origin = Some("run-abc".to_string());
        document.closed = Some("2026-08-06".to_string());
        document.scope = "## Decisions\n\n- a ruling\n".to_string();
        document.validation = "## Done when\n\nit is done\n".to_string();
        let text = serialize(&document).expect("serialise");
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed, document);
    }

    #[test]
    fn optional_new_fields_round_trip_as_absent() {
        let document = sample();
        let text = serialize(&document).expect("serialise");
        assert!(!text.contains("wall"));
        assert!(!text.contains("origin"));
        assert!(!text.contains("closed"));
        assert!(!text.contains("scope"));
        assert!(!text.contains("validation"));
        assert!(!text.contains("checks"));
        assert!(!text.contains("auto-close") && !text.contains("auto_close"));
        assert!(!text.contains("closure"));
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed.wall, None);
        assert_eq!(parsed.origin, None);
        assert_eq!(parsed.closed, None);
        assert_eq!(parsed.scope, "");
        assert_eq!(parsed.validation, "");
        assert!(parsed.checks.is_empty());
        assert_eq!(parsed.auto_close, None);
        assert_eq!(parsed.closure, None);
    }

    #[test]
    fn round_trips_checks_auto_close_and_closure() {
        let mut document = sample();
        document.checks = vec![Check {
            name: "unit tests".to_string(),
            command: "cargo test -p ctx-traits-core".to_string(),
            timeout_ms: Some(60_000),
            expect: Some("test result: ok".to_string()),
        }];
        document.auto_close = Some(AutoClosePolicy::Checked);
        document.closure = Some(Closure {
            mode: AutoClosePolicy::Checked,
            commit: Some("abc1234".to_string()),
            checks: vec![CheckRecord {
                name: "unit tests".to_string(),
                command: "cargo test -p ctx-traits-core".to_string(),
                outcome: CheckOutcome::Passed,
                detail: "exit 0, expect matched".to_string(),
            }],
        });
        let text = serialize(&document).expect("serialise");
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed, document);
    }

    #[test]
    fn closure_with_empty_checks_round_trips_under_merge_mode() {
        let mut document = sample();
        document.closure = Some(Closure {
            mode: AutoClosePolicy::Merge,
            commit: Some("deadbee".to_string()),
            checks: Vec::new(),
        });
        let text = serialize(&document).expect("serialise");
        let parsed = parse(&text).expect("parse");
        assert_eq!(parsed, document);
        assert!(parsed.closure.unwrap().checks.is_empty());
    }

    #[test]
    fn rejects_schema_0_1_naming_both_versions() {
        let text = "schema-version = \"0.1\"\nkey = \"0001\"\ntitle = \"t\"\n";
        let error = parse(text).expect_err("0.1 documents are rejected under schema 0.2");
        let message = error.to_string();
        assert!(message.contains("0.1"), "message was {message:?}");
        assert!(message.contains("0.2"), "message was {message:?}");
    }

    #[test]
    fn prose_sections_are_labeled_in_canonical_order() {
        let mut document = sample();
        document.scope = "the scope".to_string();
        document.validation = "the validation".to_string();
        let sections = document.prose_sections();
        assert_eq!(
            sections,
            [
                ("content", document.content.as_str()),
                ("scope", "the scope"),
                ("validation", "the validation"),
            ]
        );
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

    #[test]
    fn open_steps_excludes_done_and_preserves_order() {
        let mut document = sample();
        document.steps = vec![
            Step {
                id: "s1".to_string(),
                title: "first".to_string(),
                done: true,
                content: String::new(),
            },
            Step {
                id: "s2".to_string(),
                title: "second".to_string(),
                done: false,
                content: String::new(),
            },
            Step {
                id: "s3".to_string(),
                title: "third".to_string(),
                done: false,
                content: String::new(),
            },
        ];
        let open: Vec<&str> = document
            .open_steps()
            .into_iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(open, vec!["s2", "s3"]);
    }

    #[test]
    fn open_steps_empty_when_all_done_or_stepless() {
        let mut document = sample();
        document.steps = Vec::new();
        assert!(document.open_steps().is_empty());
        document.steps = vec![Step {
            id: "s1".to_string(),
            title: "first".to_string(),
            done: true,
            content: String::new(),
        }];
        assert!(document.open_steps().is_empty());
    }
}
