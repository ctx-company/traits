//! Port declarations: boundary contracts for trait dataflow.
//!
//! `[[port]]` replaces the older `[[input]]` and `[[output.section]]` models.
//! Ports are the canonical boundary between a trait and its consumers/providers.
//! Input ports declare what the trait needs; output ports either expose internal
//! slots through `value = "slot:<id>"` or are filled directly by a terminal
//! procedure output.
//!
//! Provider/provisioning decisions (source, fallback, trigger) belong to
//! runtime binding plans, not to port declarations.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::shared::SlugList;

/// Port direction: input or output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum PortDirection {
    Input,
    Output,
}

/// Runtime default source for an input port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct PortDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<PortDefaultCommand>,
}

/// Command-backed runtime default source for an input port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct PortDefaultCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
    /// Runtime timeout in milliseconds. Absent uses the IO host's documented
    /// default-input timeout.
    #[serde(
        default,
        rename = "timeout-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
    /// Runtime stdout capture ceiling in bytes. Absent uses the IO host's
    /// documented default-input capture ceiling. A capture that exceeds this
    /// fails the run rather than landing a truncated value in the port.
    #[serde(
        default,
        rename = "capture-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub capture_bytes: Option<u64>,
}

/// A `[[port]]` declaration: a boundary contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Port {
    /// Port identifier (e.g. `"user-prompt"`, `"finding"`).
    pub id: String,

    /// Whether this is an input or output port.
    pub direction: PortDirection,

    /// Schema ref (e.g. `"schema:text"`, `"schema:scope"`).
    pub schema: String,

    /// Whether this port is optional. Defaults to `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,

    /// Human-readable description.
    pub description: String,

    /// Optional display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Optional format preferences (e.g. `["markdown"]`).
    #[serde(default, skip_serializing_if = "SlugList::is_empty")]
    pub format: SlugList,

    /// For output ports: the slot ref that exposes the value (`"slot:<id>"`).
    /// Input ports must not declare `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// Optional runtime default source for input ports. Static render/check do
    /// not execute it; CLI/IO runtime may use it only with explicit permission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PortDefault>,
}

/// Validate a list of port declarations.
pub fn validate_ports(ports: &[Port]) -> crate::Result<()> {
    let mut seen_ids = BTreeSet::new();

    for (i, port) in ports.iter().enumerate() {
        let base = format!("port[{i}]");
        let id_path = format!("{base}.id");

        crate::shared::validate_slug_shape(&port.id, &id_path)?;
        if !seen_ids.insert(port.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate port id {:?}", port.id),
            }
            .into());
        }

        if port.description.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.description"),
                message: "must not be empty".to_string(),
            }
            .into());
        }

        // Schema must be a non-empty typed ref.
        if port.schema.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.schema"),
                message: "must not be empty".to_string(),
            }
            .into());
        }

        match port.direction {
            PortDirection::Input => {
                if port.value.is_some() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.value"),
                        message: "input ports must not declare value".to_string(),
                    }
                    .into());
                }
                if let Some(default) = port.default.as_ref() {
                    validate_port_default(default, &base)?;
                }
            }
            PortDirection::Output => {
                if port.default.is_some() {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{base}.default"),
                        message: "output ports must not declare runtime defaults".to_string(),
                    }
                    .into());
                }
                if let Some(ref value) = port.value {
                    let parsed = crate::reference::Reference::parse(value).map_err(|e| {
                        crate::manifest::Error::InvalidField {
                            field_path: format!("{base}.value"),
                            message: format!("invalid output port value ref: {e}"),
                        }
                    })?;
                    if parsed.kind() != crate::reference::Kind::Slot {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("{base}.value"),
                            message: "output port value must be a slot: ref".to_string(),
                        }
                        .into());
                    }
                    if parsed.is_qualified() {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("{base}.value"),
                            message: "output port value must be a local unqualified slot ref"
                                .to_string(),
                        }
                        .into());
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_port_default(default: &PortDefault, base: &str) -> crate::Result<()> {
    let Some(command) = default.command.as_ref() else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.default"),
            message: "input port default must declare a command source".to_string(),
        }
        .into());
    };
    match (command.cmd.as_deref(), command.argv.is_empty()) {
        (Some(cmd), true) => {
            crate::r#trait::procedure::parse_command_shorthand(
                cmd,
                &format!("{base}.default.command.cmd"),
            )?;
        }
        (None, false) => {
            let plan = crate::r#trait::procedure::CommandDeclaration {
                argv: command.argv.clone(),
                argv_from: None,
                executable_digest_from: None,
                cwd: command.cwd.clone(),
                timeout_ms: command.timeout_ms,
                capture_bytes: command.capture_bytes,
                success_exit_code: Vec::new(),
            };
            let item = crate::r#trait::procedure::SequenceItem {
                id: Some("default-command-validation".to_string()),
                title: String::new(),
                kind: Some(crate::r#trait::procedure::SequenceKind::Command),
                agent: None,
                prompt: String::new(),
                cmd: None,
                command: Some(plan),
                projection: Vec::new(),
                timeout_ms: None,
                capture_bytes: None,
                success_exit_code: Vec::new(),
                input: crate::r#trait::procedure::SequenceInputList::default(),
                output: crate::r#trait::procedure::OutputSinkList::new(vec![
                    crate::r#trait::procedure::OutputSink::Ref(
                        "slot:default-command-validation".to_string(),
                    ),
                ]),
                format: None,
                emits: Vec::new(),
                sequence: None,
                when: None,
                otherwise: None,
                until: None,
                stop_if: None,
                max_iterations: None,
                max_iterations_from: None,
                on_exhausted: None,
                on_stop: None,
                over: None,
                item: None,
                max_items: None,
                on_complete: None,
                on_failure: None,
                branches: crate::r#trait::procedure::RefList::default(),
                max_branches: None,
                concurrent: false,
                join: None,
                branch_failure: Vec::new(),
            };
            let _ = crate::r#trait::procedure::command_plan_for_item(
                &item,
                &format!("{base}.default.command"),
            )?;
        }
        (Some(_), false) => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.default.command"),
                message: "default command must declare either cmd or argv, not both".to_string(),
            }
            .into());
        }
        (None, true) => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.default.command"),
                message: "default command must declare cmd or argv".to_string(),
            }
            .into());
        }
    }
    if let Some(capture) = command.capture.as_deref()
        && capture != "stdout"
    {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.default.command.capture"),
            message: "only capture = \"stdout\" is supported for input defaults".to_string(),
        }
        .into());
    }
    Ok(())
}

/// Validate port schema refs and output-port value refs against declared
/// slots and schemas.
///
/// Must be called after `validate_ports`, `validate_slots`, and
/// `validate_schemas` so the declared sets are trustworthy.
pub fn validate_port_cross_refs(
    ports: &[Port],
    slot_ids: &std::collections::BTreeSet<&str>,
    schema_ids: &std::collections::BTreeSet<&str>,
    _resource_ids: &std::collections::BTreeSet<&str>,
) -> crate::Result<()> {
    for (i, port) in ports.iter().enumerate() {
        let base = format!("port[{i}]");

        // Validate schema ref using the shared schema-form validator.
        let schema_field = format!("{base}.schema");
        match crate::schema::form::Schema::try_from_str(&port.schema) {
            Ok(schema) => {
                crate::schema::form::validate(&schema, &schema_field, schema_ids)?;
            }
            Err(msg) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: schema_field,
                    message: msg,
                }
                .into());
            }
        }

        // For output ports with a value, validate the slot ref is local and declared.
        if let Some(ref value) = port.value {
            let value_field = format!("{base}.value");
            let parsed = crate::reference::Reference::parse(value).map_err(|_| {
                crate::manifest::Error::InvalidField {
                    field_path: value_field.clone(),
                    message: format!("invalid output port value ref {value:?}"),
                }
            })?;
            if parsed.is_qualified() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: value_field,
                    message: "output port value must be a local unqualified slot ref".to_string(),
                }
                .into());
            }
            let slot_id = parsed.id();
            if !slot_ids.contains(slot_id) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: value_field,
                    message: format!(
                        "output port value {value:?} references undeclared slot {slot_id:?}"
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn is_false(value: &bool) -> bool {
    !*value
}
