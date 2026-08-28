//! Signal declarations: named events that procedure sequence items may emit.
//!
//! `[[signal]]` declares a signal identity. `procedure.sequence[].on-complete` lists
//! declared `signal:*` refs. Local signals must resolve to a local declaration;
//! dependency-qualified refs must resolve to a dependency alias and declared
//! dependency signal, or remain dependency-pending.
//!
//! Validated signal trace evidence is modeled as pure data structures. These
//! structures are records only — they do not read files, call models, execute
//! tools, or parse freeform LLM prose. Free text in prompts, model responses,
//! or CLI arguments is not a signal unless parsed and validated into these
//! structures.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reference::Reference;
use crate::schema::form::Schema;

/// A `[[signal]]` declaration: a named event identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Signal {
    /// Signal identifier (e.g. `"needs-tests"`, `"missing-credentials"`).
    pub id: String,

    /// Human-readable description of when this signal fires.
    pub description: String,

    /// Optional schema required for an emitted payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
}

/// Validate a list of signal declarations.
pub fn validate_signals(
    signals: &[Signal],
    declared_schema_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let mut seen_ids = BTreeSet::new();

    for (i, signal) in signals.iter().enumerate() {
        let id_path = format!("signal[{i}].id");
        crate::shared::validate_slug_shape(&signal.id, &id_path)?;
        if !seen_ids.insert(signal.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate signal id {:?}", signal.id),
            }
            .into());
        }

        if signal.description.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("signal[{i}].description"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
        if let Some(schema) = &signal.schema {
            crate::schema::form::validate(
                schema,
                &format!("signal[{i}].schema"),
                declared_schema_ids,
            )?;
        }
    }

    Ok(())
}

// ===========================================================================
// Signal trace evidence (pure data structures)
// ===========================================================================

/// The scope of a signal trace event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SignalTraceScope {
    /// The signal is scoped to the current run.
    Run,
}

/// Grouped endpoint evidence for a defining/emitting signal side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SignalTraceEndpoint {
    pub trait_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_item_id: Option<String>,
}

/// A validated signal trace evidence record.
///
/// This is a pure data record: it does not read files, call models, execute
/// tools, or parse freeform LLM prose. It is produced by runtime/host layers
/// that have validated the signal event, not by the pure core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SignalTraceEvent {
    /// The signal ref string (e.g. `signal:needs-tests`).
    pub signal_ref: Reference,
    /// Defining signal endpoint.
    pub defining: SignalTraceEndpoint,
    /// Emitting procedure endpoint.
    pub emitting: SignalTraceEndpoint,
    /// The scope of this trace event (default: `run`).
    pub scope: SignalTraceScope,
    /// Optional evidence ref or checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    /// Optional turn number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// The source of this trace event (e.g. `"runtime"`, `"host"`).
    pub source: String,
}

/// A collection of validated signal trace events for relation evaluation.
///
/// Signal facts are the set of signal ref strings that have validated trace
/// evidence. They are consumed by relation `when` matching — a local
/// `signal:<id>` in a relation matches only a validated signal fact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SignalTraceEvidence {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<SignalTraceEvent>,
}

impl SignalTraceEvidence {
    /// Build a set of signal ref strings from the trace events.
    pub fn signal_facts(&self) -> BTreeSet<String> {
        self.events
            .iter()
            .map(|e| e.signal_ref.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Signal, validate_signals};

    #[test]
    fn schema_signal_validates_declared_schema_reference() {
        let signals: Vec<Signal> = serde_json::from_value(serde_json::json!([{
            "id": "needs-review",
            "description": "Review is needed.",
            "schema": "schema:review-note"
        }]))
        .expect("signal fixture decodes");
        let schemas = BTreeSet::from(["review-note"]);

        validate_signals(&signals, &schemas).expect("declared schema is accepted");
    }

    #[test]
    fn schema_signal_rejects_undeclared_schema_reference_at_signal_path() {
        let signals: Vec<Signal> = serde_json::from_value(serde_json::json!([{
            "id": "needs-review",
            "description": "Review is needed.",
            "schema": "schema:missing"
        }]))
        .expect("signal fixture decodes");

        let error = validate_signals(&signals, &BTreeSet::new())
            .expect_err("undeclared signal schema must fail");
        assert!(error.to_string().contains("signal[0].schema"));
    }
}
