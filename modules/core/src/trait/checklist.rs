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

use std::collections::BTreeSet;

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
