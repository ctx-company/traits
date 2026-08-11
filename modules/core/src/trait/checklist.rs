//! The contract between a checklist resource and its verdict schema.
//!
//! A `variant = "checklist"` resource declares the criteria; a companion
//! `[[schema]]` declares the shape of one verdict about one criterion. The
//! schema is *generated* by the CDK rather than invented here, for one
//! reason: the canonical document is the thing reviewers read and digests
//! lock. A schema conjured at decode time would constrain the model without
//! appearing in the artifact anyone approved, and would either break
//! byte-stability on the way out or hide itself from `explain`.
//!
//! So core does not synthesize — it *verifies*. The generator and the checker
//! are independent, which is what makes a hand-edited canonical document, a
//! stale build, or a reworded item fail loudly instead of quietly reviewing
//! the wrong criteria.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::resource::Resource;
use super::schema::Schema;

/// The statuses a verdict may carry, in canonical order.
pub const VERDICT_STATUSES: [&str; 3] = ["pass", "fail", "waived"];

/// The field naming the item a verdict answers.
pub const VERDICT_ITEM_FIELD: &str = "item";
/// The field carrying the verdict itself.
pub const VERDICT_STATUS_FIELD: &str = "status";
/// The field carrying supporting evidence.
pub const VERDICT_EVIDENCE_FIELD: &str = "evidence";

/// The statuses a produced checklist item may carry, in canonical order.
///
/// Reuses the declared mechanism's `waived` rather than inventing a second
/// vocabulary for "this item does not apply."
pub const ITEM_STATUSES: [&str; 3] = ["todo", "done", "waived"];

/// The field carrying a produced item's stable id, minted at production time.
pub const ITEM_ID_FIELD: &str = "id";
/// The field carrying a produced item's text.
pub const ITEM_TEXT_FIELD: &str = "text";
/// The field carrying a produced item's optional detail.
pub const ITEM_DETAIL_FIELD: &str = "detail";
/// The field carrying a produced item's status.
pub const ITEM_STATUS_FIELD: &str = "status";
/// The field carrying a produced item's optional evidence.
pub const ITEM_EVIDENCE_FIELD: &str = "evidence";

const ITEM_FIELDS: [&str; 5] = [
    ITEM_ID_FIELD,
    ITEM_TEXT_FIELD,
    ITEM_DETAIL_FIELD,
    ITEM_STATUS_FIELD,
    ITEM_EVIDENCE_FIELD,
];

/// Structural check for one `schema:checklist-item` value.
///
/// The shape is closed: `id` (non-empty string), `text` (non-empty string),
/// optional `detail` (string), `status` (one of [`ITEM_STATUSES`]), optional
/// `evidence` (string). Unknown fields are rejected — the canonical item
/// shape lives in Rust, not in whatever a model happens to emit.
pub fn validate_checklist_item_value(value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err("expected a JSON object for schema:checklist-item".to_string());
    };
    for key in object.keys() {
        if !ITEM_FIELDS.contains(&key.as_str()) {
            return Err(format!(
                "schema:checklist-item does not declare field {key:?}"
            ));
        }
    }
    let non_empty_string = |field: &str| -> Result<(), String> {
        match object.get(field).and_then(Value::as_str) {
            Some(text) if !text.trim().is_empty() => Ok(()),
            _ => Err(format!(
                "schema:checklist-item.{field} must be a non-empty string"
            )),
        }
    };
    non_empty_string(ITEM_ID_FIELD)?;
    non_empty_string(ITEM_TEXT_FIELD)?;
    if let Some(detail) = object.get(ITEM_DETAIL_FIELD)
        && !detail.is_string()
    {
        return Err(format!(
            "schema:checklist-item.{ITEM_DETAIL_FIELD} must be a string"
        ));
    }
    match object.get(ITEM_STATUS_FIELD).and_then(Value::as_str) {
        Some(status) if ITEM_STATUSES.contains(&status) => {}
        _ => {
            return Err(format!(
                "schema:checklist-item.{ITEM_STATUS_FIELD} must be one of {ITEM_STATUSES:?}"
            ));
        }
    }
    if let Some(evidence) = object.get(ITEM_EVIDENCE_FIELD)
        && !evidence.is_string()
    {
        return Err(format!(
            "schema:checklist-item.{ITEM_EVIDENCE_FIELD} must be a string"
        ));
    }
    Ok(())
}

/// The outcome of checking a set of submitted item ids against a universe.
pub struct CoverageCheck<'a> {
    /// Universe ids absent from the write.
    pub missing: Vec<&'a str>,
    /// Ids answered more than once in the write.
    pub duplicated: Vec<&'a str>,
}

/// Shared coverage core for both declared verdict lists and produced
/// checklists: every id in `universe` must appear in `counts` exactly once.
///
/// `check_duplicates_beyond_universe` extends the duplicate check to ids that
/// are not (yet) in `universe` — the produced-checklist case, where a
/// replace write may mint new ids that join the universe, but still may not
/// answer any one id twice within the same write. The declared-checklist
/// case sets this `false` to keep its landed behavior byte-identical: a
/// closed item set never has ids outside its universe to begin with.
pub fn coverage_check<'a>(
    universe: &[&'a str],
    counts: &BTreeMap<&'a str, usize>,
    check_duplicates_beyond_universe: bool,
) -> CoverageCheck<'a> {
    let universe_set: BTreeSet<&str> = universe.iter().copied().collect();
    let missing: Vec<&str> = universe
        .iter()
        .copied()
        .filter(|id| !counts.contains_key(id))
        .collect();
    let mut duplicated: Vec<&str> = universe
        .iter()
        .copied()
        .filter(|id| counts.get(id).is_some_and(|count| *count > 1))
        .collect();
    if check_duplicates_beyond_universe {
        duplicated.extend(
            counts
                .iter()
                .filter(|(id, count)| **count > 1 && !universe_set.contains(*id))
                .map(|(id, _)| *id),
        );
    }
    CoverageCheck {
        missing,
        duplicated,
    }
}

/// Render a produced checklist's accepted items as the text presented to a
/// model or a static frame — never raw JSON.
pub fn render_produced_items(items: &[Value]) -> String {
    let mut out = format!("Produced checklist — {} item(s).\n", items.len());
    for item in items {
        let id = item
            .get(ITEM_ID_FIELD)
            .and_then(Value::as_str)
            .unwrap_or("?");
        let text = item
            .get(ITEM_TEXT_FIELD)
            .and_then(Value::as_str)
            .unwrap_or("");
        let status = item
            .get(ITEM_STATUS_FIELD)
            .and_then(Value::as_str)
            .unwrap_or("todo");
        out.push_str(&format!("\n- [{id}] {text} — {status}"));
        if let Some(evidence) = item.get(ITEM_EVIDENCE_FIELD).and_then(Value::as_str) {
            out.push_str(&format!(" ({evidence})"));
        }
    }
    out.push('\n');
    out
}

/// The schema id that carries verdicts for the checklist resource `id`.
pub fn verdict_schema_id(resource_id: &str) -> String {
    format!("{resource_id}-verdict")
}

/// Render a checklist's items as the text presented to a model.
///
/// This is the only way checklist items reach a model: there is no prose copy
/// to drift from, and the rendering is a pure function of the typed items, so
/// the same declaration produces the same bytes in a run frame and in a static
/// render. Item ids are shown in brackets because the model must return them
/// verbatim — an id it cannot see is an id it will invent.
///
/// Returns an empty string for a non-checklist resource.
pub fn render_items(resource: &Resource) -> String {
    if !resource.is_checklist() {
        return String::new();
    }

    let mut out = format!(
        "Checklist resource:{} — {} item(s). Answer every item exactly once, keyed by the id in brackets.\n",
        resource.id,
        resource.items.len()
    );
    for item in &resource.items {
        out.push_str(&format!("\n- [{}] {}", item.id, item.text.trim()));
        if let Some(detail) = item.detail.as_deref() {
            for line in detail.trim().lines() {
                out.push_str(&format!("\n  {line}"));
            }
        }
        if item.requires_evidence() {
            out.push_str("\n  (evidence required)");
        }
    }
    out.push('\n');
    out
}

/// The checklist resource whose verdicts `schema_id` carries, if any.
///
/// Used to recover the item universe from a slot's schema ref, which is how a
/// coverage obligation is derived rather than authored.
pub fn checklist_for_verdict_schema<'a>(
    resources: &'a [Resource],
    schema_id: &str,
) -> Option<&'a Resource> {
    let resource_id = schema_id.strip_suffix("-verdict")?;
    resources
        .iter()
        .find(|resource| resource.is_checklist() && resource.id == resource_id)
}

/// Verify every checklist's companion verdict schema matches its items.
///
/// A checklist with no companion schema is legal — it renders as guidance and
/// collects no verdicts. What is not legal is a companion schema that has
/// drifted from the items it claims to answer.
pub fn validate_checklist_verdict_schemas(
    resources: &[Resource],
    schemas: &[Schema],
) -> crate::Result<()> {
    for resource in resources.iter().filter(|r| r.is_checklist()) {
        let schema_id = verdict_schema_id(&resource.id);
        let Some(schema) = schemas.iter().find(|schema| schema.id == schema_id) else {
            continue;
        };
        validate_verdict_schema(resource, schema, &schema_id)?;
    }
    Ok(())
}

fn validate_verdict_schema(
    resource: &Resource,
    schema: &Schema,
    schema_id: &str,
) -> crate::Result<()> {
    let field_base = format!("schema:{schema_id}");

    let Some(fields) = schema.fields.as_ref() else {
        return Err(invalid(
            &field_base,
            format!(
                "must declare fields: it carries verdicts for checklist resource:{}",
                resource.id
            ),
        ));
    };

    let item_ids: Vec<&str> = resource.checklist_item_ids();
    let declared = allowed_strings(
        fields
            .get(VERDICT_ITEM_FIELD)
            .and_then(|f| f.allowed.as_ref()),
    );
    let Some(declared) = declared else {
        return Err(invalid(
            &format!("{field_base}.fields.{VERDICT_ITEM_FIELD}"),
            format!(
                "must declare allowed = {item_ids:?}: the item field is the closed set of checklist resource:{} item ids",
                resource.id
            ),
        ));
    };

    if declared != item_ids {
        let missing: Vec<&&str> = item_ids
            .iter()
            .filter(|id| !declared.contains(id))
            .collect();
        let extra: Vec<&&str> = declared
            .iter()
            .filter(|id| !item_ids.contains(id))
            .collect();
        return Err(invalid(
            &format!("{field_base}.fields.{VERDICT_ITEM_FIELD}.allowed"),
            format!(
                "drifted from checklist resource:{}: expected {item_ids:?}, got {declared:?}{}{}",
                resource.id,
                format_ids(" — missing", &missing),
                format_ids(" — not declared as items", &extra),
            ),
        ));
    }

    let statuses = allowed_strings(
        fields
            .get(VERDICT_STATUS_FIELD)
            .and_then(|f| f.allowed.as_ref()),
    );
    if statuses.as_deref() != Some(&VERDICT_STATUSES[..]) {
        return Err(invalid(
            &format!("{field_base}.fields.{VERDICT_STATUS_FIELD}"),
            format!("must declare allowed = {VERDICT_STATUSES:?}"),
        ));
    }

    // Evidence is required for the whole verdict shape as soon as any single
    // item asks for it: a per-item requirement cannot be expressed in one
    // object schema, so the strictest item sets the contract.
    let evidence_required = fields
        .get(VERDICT_EVIDENCE_FIELD)
        .is_some_and(|field| field.required);
    if resource.checklist_requires_evidence() && !evidence_required {
        let demanding: Vec<&str> = resource
            .items
            .iter()
            .filter(|item| item.requires_evidence())
            .map(|item| item.id.as_str())
            .collect();
        return Err(invalid(
            &format!("{field_base}.fields.{VERDICT_EVIDENCE_FIELD}"),
            format!(
                "must be required: checklist resource:{} declares requires-evidence on {demanding:?}",
                resource.id
            ),
        ));
    }

    Ok(())
}

fn allowed_strings(allowed: Option<&Vec<Value>>) -> Option<Vec<&str>> {
    allowed?.iter().map(Value::as_str).collect()
}

fn format_ids(label: &str, ids: &[&&str]) -> String {
    if ids.is_empty() {
        String::new()
    } else {
        let list: BTreeSet<&str> = ids.iter().map(|id| **id).collect();
        format!("{label} {:?}", list.into_iter().collect::<Vec<_>>())
    }
}

fn invalid(field_path: &str, message: String) -> crate::Error {
    crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message,
    }
    .into()
}

#[cfg(test)]
mod produced_tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str, status: &str) -> Value {
        json!({"id": id, "text": "do the thing", "status": status})
    }

    #[test]
    fn well_formed_item_is_accepted() {
        assert!(validate_checklist_item_value(&item("a", "todo")).is_ok());
    }

    #[test]
    fn missing_text_is_rejected() {
        let value = json!({"id": "a", "status": "todo"});
        assert!(validate_checklist_item_value(&value).is_err());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let value = json!({"id": "a", "text": "x", "status": "todo", "surprise": 1});
        assert!(validate_checklist_item_value(&value).is_err());
    }

    #[test]
    fn unknown_status_is_rejected() {
        let value = json!({"id": "a", "text": "x", "status": "in-progress"});
        assert!(validate_checklist_item_value(&value).is_err());
    }

    #[test]
    fn coverage_accepts_matching_ids_with_status_updates() {
        let universe = ["a", "b"];
        let counts: BTreeMap<&str, usize> = [("a", 1), ("b", 1)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, true);
        assert!(outcome.missing.is_empty());
        assert!(outcome.duplicated.is_empty());
    }

    #[test]
    fn coverage_rejects_dropped_id() {
        let universe = ["a", "b"];
        let counts: BTreeMap<&str, usize> = [("a", 1)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, true);
        assert_eq!(outcome.missing, vec!["b"]);
    }

    #[test]
    fn coverage_rejects_duplicated_id() {
        let universe = ["a"];
        let counts: BTreeMap<&str, usize> = [("a", 2)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, true);
        assert_eq!(outcome.duplicated, vec!["a"]);
    }

    #[test]
    fn coverage_allows_new_ids_joining_the_universe() {
        let universe = ["a"];
        let counts: BTreeMap<&str, usize> = [("a", 1), ("b", 1)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, true);
        assert!(outcome.missing.is_empty());
        assert!(outcome.duplicated.is_empty());
    }

    #[test]
    fn coverage_rejects_duplicated_new_id_when_allowed_beyond_universe() {
        let universe = ["a"];
        let counts: BTreeMap<&str, usize> = [("a", 1), ("b", 2)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, true);
        assert_eq!(outcome.duplicated, vec!["b"]);
    }

    #[test]
    fn declared_mode_ignores_duplicates_outside_universe() {
        let universe = ["a"];
        let counts: BTreeMap<&str, usize> = [("a", 1), ("b", 2)].into_iter().collect();
        let outcome = coverage_check(&universe, &counts, false);
        assert!(outcome.duplicated.is_empty());
    }
}
