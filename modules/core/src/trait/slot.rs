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

    /// Display name, when it must differ from the id. The same field a port
    /// carries, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Whether reading this slot blocks. A slot is written by a step, so a
    /// read before any step could have written is normally refused; an
    /// optional slot says otherwise once, at the declaration, rather than at
    /// every read site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,

    /// A value the slot holds before any step writes one, so a first-pass
    /// prompt can interpolate it and a counter can start somewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
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
