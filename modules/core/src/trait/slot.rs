//! Slot definitions: procedure run ledger value contracts.
//!
//! `[[slot]]` sections declare runtime ledger values produced and consumed
//! by procedure steps. Slots are contracts (what shape a value has), not
//! external storage or mutable variables.
//!
//! Slot schemas use built-in schema refs (`schema:text`), local declared
//! schema refs (`schema:scope`), or list wrappers (`[schema:scope]`).

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::schema::form::Schema;

/// A `[[slot]]` definition: a procedure run ledger value contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Slot {
    /// Slot identifier (e.g. `"finding"`, `"scope"`, `"note-risk"`).
    pub id: String,

    /// Schema ref (e.g. `"schema:text"`, `"schema:scope"`,
    /// `"[schema:scope]"`) or omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,

    /// Human-readable description of what this slot represents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Advisory hint for the expected slot value, examples, or constraints
    /// useful to a step prompt. Metadata only — schema remains the validation
    /// contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Validate a list of slot declarations.
///
/// Checks for:
/// - Empty or invalid slot IDs (must be valid kebab-case slugs usable as
///   `slot:<id>` refs)
/// - Duplicate slot IDs
///
/// Schema-required checks are deferred; P22 only validates ID shape and
/// duplicates plus lightweight schema shape for schemas that are present.
/// Slots without schemas are permitted because simple style traits and
/// implicit static-render slots may omit them.
///
/// Returns the first error found, or `Ok(())` if all slots are valid.
/// All diagnostics use field-specific paths such as `slot[0].id`.
pub fn validate_slots(slots: &[Slot], declared_schema_ids: &BTreeSet<&str>) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, slot) in slots.iter().enumerate() {
        let id_path = format!("slot[{i}].id");
        crate::shared::validate_slug_shape(&slot.id, &id_path)?;

        if !seen.insert(slot.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate slot id {:?}", slot.id),
            }
            .into());
        }

        if let Some(ref schema) = slot.schema {
            crate::schema::form::validate(
                schema,
                &format!("slot[{i}].schema"),
                declared_schema_ids,
            )?;
        }
    }

    Ok(())
}
