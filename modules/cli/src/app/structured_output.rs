//! Declaration-driven presentation for small structured output lists.

use std::collections::BTreeMap;

use serde_json::Value;

pub(crate) struct StructuredOutput {
    pub(crate) count: usize,
    rows: Vec<Vec<(String, String)>>,
}

impl StructuredOutput {
    pub(crate) fn compact_lines(
        &self,
        status: &str,
        verdict: Option<&str>,
        receipt: Option<&str>,
    ) -> Vec<String> {
        let mut lines = vec![format!(
            "{status}{} - {} items",
            verdict_suffix(verdict),
            self.count
        )];
        let widths = field_widths(&self.rows);
        for (index, row) in self.rows.iter().enumerate() {
            let fields = row
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "evidence" | "done-when"))
                .map(|(key, value)| {
                    let value = if key == "reason" {
                        format!("({value})")
                    } else {
                        value.clone()
                    };
                    format!("{key:width$}: {value}", width = widths[key])
                })
                .collect::<Vec<_>>();
            lines.push(format!("  {}. {}", index + 1, fields.join("  ")));
        }
        if let Some(receipt) = receipt {
            lines.push(format!("  receipt: {receipt}"));
        }
        lines
    }

    pub(crate) fn verbose_lines(&self, status: &str, verdict: Option<&str>) -> Vec<String> {
        let mut lines = vec![format!(
            "{status}{} - {} items",
            verdict_suffix(verdict),
            self.count
        )];
        for (index, row) in self.rows.iter().enumerate() {
            lines.push(format!("  item {}", index + 1));
            for (key, value) in row {
                lines.push(format!("    {key}: {value}"));
            }
        }
        lines
    }
}

fn verdict_suffix(verdict: Option<&str>) -> String {
    verdict.map_or_else(String::new, |verdict| format!(" ({verdict})"))
}

fn field_widths(rows: &[Vec<(String, String)>]) -> BTreeMap<String, usize> {
    let mut widths = BTreeMap::new();
    for row in rows {
        for (key, _) in row {
            let width = widths.entry(key.clone()).or_insert(0);
            *width = (*width).max(key.len());
        }
    }
    widths
}

/// Return a presentation when the declaration and value establish a bounded
/// list of small objects.
///
/// This used to require an opt-in tag in `port.format` as well, on the
/// reasoning that shape alone was not enough to justify rendering a table.
/// `format` is gone: it accepted any slug, only `structured` and `table` ever
/// meant anything, both meant the same thing, and the tag reached a model as
/// a line of text it could not act on.
///
/// What remains is the part that was doing the work — the SHAPE bounds below.
/// A declared object schema, at most 12 fields, a list of at most 100 rows,
/// every one an object. A value that narrow is a table; anything outside it
/// keeps the plain rendering exactly as before.
pub(crate) fn resolve(
    trait_ref: &ctx_traits_core::Trait,
    port_id: &str,
    value: &Value,
) -> Option<StructuredOutput> {
    let port = trait_ref.ports.iter().find(|port| port.id == port_id)?;
    let ctx_traits_core::schema::form::Schema::List(inner) =
        ctx_traits_core::schema::form::Schema::try_from_str(&port.schema).ok()?
    else {
        return None;
    };
    let schema_id = inner.strip_prefix("schema:")?;
    let schema = trait_ref
        .schemas
        .iter()
        .find(|schema| schema.id == schema_id)?;
    let fields = schema.fields.as_ref()?;
    if fields.is_empty() || fields.len() > 12 {
        return None;
    }
    let values = value.as_array()?;
    if values.is_empty() || values.len() > 100 {
        return None;
    }
    let mut rows = Vec::with_capacity(values.len());
    for value in values {
        let object = value.as_object()?;
        if object.len() > 12 || object.keys().any(|key| !fields.contains_key(key)) {
            return None;
        }
        let mut row = Vec::new();
        for key in fields.keys() {
            let value = object.get(key)?;
            if !scalar_or_scalar_list(value) {
                return None;
            }
            let text = clean_value(value);
            row.push((key.clone(), text));
        }
        rows.push(row);
    }
    Some(StructuredOutput {
        count: rows.len(),
        rows,
    })
}

pub(crate) fn clean_value(value: &Value) -> String {
    let raw = value
        .as_str()
        .map_or_else(|| value.to_string(), ToString::to_string);
    let cleaned = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.chars().count() > 200 {
        format!("{}…", cleaned.chars().take(200).collect::<String>())
    } else {
        cleaned
    }
}

pub(crate) fn verdict_for_values<'a>(
    values: impl IntoIterator<Item = &'a ctx_traits_core::procedure::runtime::Value>,
) -> Option<String> {
    values.into_iter().find_map(|value| {
        value
            .value
            .as_object()
            .and_then(|object| object.get("status"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

/// Select only values written by the accepted frame. Both the CLI call path
/// and MCP call path expose the same accepted frame contract to this helper.
pub(crate) fn accepted_frame_values<'a>(
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    accepted_slots: &'a [ctx_traits_core::procedure::runtime::Value],
    accepted_output_ports: &'a [ctx_traits_core::procedure::runtime::Value],
) -> Vec<&'a ctx_traits_core::procedure::runtime::Value> {
    accepted_slots
        .iter()
        .chain(accepted_output_ports.iter())
        .filter(|value| {
            frame
                .requested_outputs
                .iter()
                .any(|output| output.slot_ref.as_str() == value.ref_text)
        })
        .collect()
}

/// Resolve a verdict from the accepted values produced with one final output.
/// Slot revisions provide the producer position; values from unrelated frames
/// are never considered.
pub(crate) fn producer_verdict_for_output(
    session: &ctx_traits_core::procedure::session::Session,
    output: &ctx_traits_core::procedure::session::FinalOutput,
) -> Option<String> {
    let revision = session.slot_revisions.iter().find(|revision| {
        revision.slot_ref == output.value_slot_ref && revision.value_digest == output.value_digest
    });
    let values = session
        .accepted_slot_values
        .iter()
        .chain(session.accepted_output_port_values.iter());
    if let Some(revision) = revision {
        return verdict_for_values(values.filter(|value| {
            session.slot_revisions.iter().any(|candidate| {
                candidate.slot_ref.as_str() == value.ref_text
                    && candidate.position_path == revision.position_path
                    && candidate.value_digest == value.value_digest
            })
        }));
    }
    // Direct output-port values have no slot revision. Restrict the sibling
    // search to the direct-port frame values; slot-backed status values belong
    // to a different producer contract and must not be guessed here.
    let direct_values = session.accepted_output_port_values.iter();
    if direct_values.clone().any(|value| {
        value.ref_text == output.value_slot_ref.as_str()
            && value.value_digest == output.value_digest
    }) {
        return verdict_for_values(direct_values);
    }
    None
}

pub(crate) fn producer_verdict(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<String> {
    session.completion.as_ref().and_then(|completion| {
        completion
            .final_outputs
            .iter()
            .find_map(|output| producer_verdict_for_output(session, output))
    })
}

pub(crate) fn port_id_for_value(
    trait_ref: &ctx_traits_core::Trait,
    ref_text: &str,
) -> Option<String> {
    let id = ref_text.strip_prefix("port:").unwrap_or(ref_text);
    trait_ref.ports.iter().find_map(|port| {
        (port.id == id || port.value.as_deref() == Some(ref_text)).then(|| port.id.clone())
    })
}

fn scalar_or_scalar_list(value: &Value) -> bool {
    value.is_string()
        || value.is_number()
        || value.is_boolean()
        || value
            .as_array()
            .is_some_and(|items| items.len() <= 12 && items.iter().all(scalar_or_scalar_list_item))
}

fn scalar_or_scalar_list_item(value: &Value) -> bool {
    value.is_string() || value.is_number() || value.is_boolean()
}
