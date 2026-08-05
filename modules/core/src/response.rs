//! Response, warning, and ABI envelope conventions.
//!
//! Two shapes live here:
//!
//! - `CommandOutput<T>`: the current CLI/report-oriented response carrying
//!   data, warnings, and capability reports. Used by the CLI today.
//! - `Envelope<T>`: the active WASM/plugin JSON ABI response envelope with
//!   `schema-version`, `ok`, `value` or typed `error`, `warnings`, and
//!   `capabilities`.
//!
//! Warnings carry stable codes for deterministic reporting. Capabilities
//! record what a host or target supports. Both are deterministically ordered
//! so check/diff/explain output stays stable.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable warning code string for deterministic reporting.
///
/// Codes follow a `<category>.<specific>` convention. See [`warning_code`]
/// for known constants.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, derive_more::Display,
)]
#[serde(transparent)]
pub struct WarningCode(String);

impl WarningCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for WarningCode {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}

/// Known warning code constants.
///
/// Once a code is defined here it is stable: later phases use the same string
/// for the same meaning. New codes are added as new domain areas land.
pub mod warning_code {
    pub const POLICY_ADVISORY_ONLY: &str = "policy.advisory-only";
    pub const COMPOSITION_CONFLICT: &str = "composition.conflict";
    pub const RENDER_UNSUPPORTED_FIELD: &str = "render.unsupported-field";
    pub const RUNTIME_CLAIM_GATE: &str = "runtime.claim-gate";
    pub const STEP_IMPLICIT_SLOT: &str = "step.implicit-slot";
    pub const SCHEMA_SUPPORT_ARTIFACT: &str = "schema.support-artifact";
    /// A decoded canonical trait document still carried a retired top-level
    /// `status`/`trust` key (Group 95, 2026-07-19). Native CLI/IO callers
    /// print this warning to stderr via
    /// `ctx_traits_io::decode_diagnostics::print_decode_warnings`; the WASM
    /// ABI has no stderr, so it surfaces the same warning on the response
    /// envelope instead.
    pub const TRAIT_DEPRECATED_FIELD: &str = "trait.deprecated-field";
}

/// A warning with a stable code, human-readable message, and optional field
/// path for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Warning {
    pub code: WarningCode,
    pub message: String,
    pub path: Option<String>,
}

impl Warning {
    pub fn new(code: impl Into<WarningCode>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl PartialOrd for Warning {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Warning {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort order is `(code, path, message)`, while field order is wire order.
        (&self.code, &self.path, &self.message).cmp(&(&other.code, &other.path, &other.message))
    }
}

/// Capability report for unsupported or partially supported features.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct CapabilityReport {
    pub capability: String,
    pub supported: bool,
    pub reason: Option<String>,
}

impl CapabilityReport {
    pub fn supported(capability: impl Into<String>) -> Self {
        Self {
            capability: capability.into(),
            supported: true,
            reason: None,
        }
    }

    pub fn unsupported(capability: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            capability: capability.into(),
            supported: false,
            reason: Some(reason.into()),
        }
    }
}

/// Current CLI/report-oriented response carrying data, warnings, and
/// capability reports.
///
/// This is the command output shape used by the CLI today. It is distinct
/// from `Envelope`, which is the active WASM/plugin JSON ABI response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommandOutput<T> {
    pub data: T,
    pub warnings: Vec<Warning>,
    pub capabilities: Vec<CapabilityReport>,
}

impl<T> CommandOutput<T> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            warnings: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_warning(mut self, warning: Warning) -> Self {
        self.warnings.push(warning);
        self.warnings.sort();
        self
    }

    pub fn with_capability(mut self, report: CapabilityReport) -> Self {
        self.capabilities.push(report);
        self.capabilities.sort();
        self
    }
}

/// ABI envelope schema version.
const ENVELOPE_SCHEMA_VERSION: &str = "0.1.0";

/// Typed JSON ABI error object.
///
/// Shape: `{ "code", "message", "details" }`. Details use a `BTreeMap` so
/// serialization stays deterministic. Empty details serialize as `{}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResponseError {
    pub code: String,
    pub message: String,
    pub details: BTreeMap<String, String>,
}

impl ResponseError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: BTreeMap::new(),
        }
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    pub fn from_core_error(error: &crate::Error) -> Self {
        match error {
            crate::Error::Reference(error) => match error {
                crate::reference::Error::Unresolved { ref_text } => {
                    Self::new("core.unresolved-reference", "unresolved reference")
                        .with_detail("ref-text", ref_text.clone())
                }
                crate::reference::Error::SlotNeverProduced { reader, ref_text } => {
                    Self::new("core.slot-never-produced", "slot read before production")
                        .with_detail("reader", reader.clone())
                        .with_detail("ref", ref_text.clone())
                }
                crate::reference::Error::SlotProducedLater {
                    reader,
                    ref_text,
                    producer,
                } => Self::new("core.slot-produced-later", "slot read before production")
                    .with_detail("reader", reader.clone())
                    .with_detail("ref", ref_text.clone())
                    .with_detail("producer", producer.clone()),
                crate::reference::Error::MissingSeparator { .. }
                | crate::reference::Error::EmptyKind { .. }
                | crate::reference::Error::ColonInPath { .. }
                | crate::reference::Error::UnknownKind { .. }
                | crate::reference::Error::EmptyPath { .. }
                | crate::reference::Error::EmptyPathSegment { .. }
                | crate::reference::Error::DotPathNotAllowed { .. }
                | crate::reference::Error::LocalPathQualified { .. }
                | crate::reference::Error::QualifiedComponentInvalid { .. } => {
                    Self::new("core.invalid-typed-ref", "invalid typed reference")
                        .with_detail("ref-text", error.ref_text().to_string())
                }
            },
            crate::Error::Digest(error) => Self::new("core.invalid-manifest", "invalid manifest")
                .with_detail("field-path", error.detail_field_path())
                .with_detail("message", error.detail_message()),
            crate::Error::Parse(error) => {
                if error.is_shape_mismatch() {
                    Self::new("core.shape-mismatch", "shape mismatch")
                        .with_detail("field-path", error.detail_field_path())
                        .with_detail("expected", "valid manifest shape")
                        .with_detail("actual", error.detail_message())
                } else {
                    Self::new("core.invalid-manifest", "invalid manifest")
                        .with_detail("field-path", error.detail_field_path())
                        .with_detail("message", error.detail_message())
                }
            }
            crate::Error::Encoding(error) => match error {
                crate::encoding::Error::Unsupported { encoding } => {
                    Self::new("core.unsupported-encoding", "unsupported encoding")
                        .with_detail("encoding", encoding.clone())
                }
                crate::encoding::Error::InvalidField {
                    field_path,
                    message,
                } => Self::new("core.invalid-manifest", "invalid manifest")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("message", message.clone()),
                crate::encoding::Error::ShapeMismatch {
                    field_path,
                    expected,
                    actual,
                } => Self::new("core.shape-mismatch", "shape mismatch")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("expected", expected.clone())
                    .with_detail("actual", actual.clone()),
            },
            crate::Error::Manifest(error) => match error {
                crate::manifest::Error::DuplicateManifest { path } => {
                    Self::new("core.duplicate-manifest", "duplicate manifest")
                        .with_detail("path", path.clone())
                }
                crate::manifest::Error::DuplicateId { id } => {
                    Self::new("core.duplicate-id", "duplicate identifier")
                        .with_detail("id", id.clone())
                }
                crate::manifest::Error::InvalidField {
                    field_path,
                    message,
                } => Self::new("core.invalid-manifest", "invalid manifest")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("message", message.clone()),
                crate::manifest::Error::ShapeMismatch {
                    field_path,
                    expected,
                    actual,
                } => Self::new("core.shape-mismatch", "shape mismatch")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("expected", expected.clone())
                    .with_detail("actual", actual.clone()),
            },
            crate::Error::Schema(error) => match error {
                crate::schema::Error::Invalid { message } => {
                    Self::new("core.invalid-schema", "invalid schema")
                        .with_detail("message", message.clone())
                }
            },
            crate::Error::Capability(error) => match error {
                crate::capability::Error::Unsupported { capability } => {
                    Self::new("core.unsupported-capability", "unsupported capability")
                        .with_detail("capability", capability.clone())
                }
            },
            crate::Error::Shared(error) => match error {
                crate::shared::Error::ShapeMismatch {
                    field_path,
                    expected,
                    actual,
                } => Self::new("core.shape-mismatch", "shape mismatch")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("expected", expected.clone())
                    .with_detail("actual", actual.clone()),
            },
            crate::Error::Trait(error) => match error {
                crate::r#trait::Error::InvalidField {
                    field_path,
                    message,
                } => Self::new("core.invalid-manifest", "invalid manifest")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("message", message.clone()),
            },
            crate::Error::Model(error) => match error {
                crate::procedure::Error::InvalidField {
                    field_path,
                    message,
                } => Self::new("core.invalid-manifest", "invalid manifest")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("message", message.clone()),
                crate::procedure::Error::ShapeMismatch {
                    field_path,
                    expected,
                    actual,
                } => Self::new("core.shape-mismatch", "shape mismatch")
                    .with_detail("field-path", field_path.clone())
                    .with_detail("expected", expected.clone())
                    .with_detail("actual", actual.clone()),
                crate::procedure::Error::Serialization {
                    field_path,
                    subject,
                    source,
                } => Self::new("core.invalid-manifest", "invalid manifest")
                    .with_detail("field-path", field_path.clone())
                    .with_detail(
                        "message",
                        format!("failed to serialize {subject}: {source}"),
                    ),
            },
            crate::Error::Distribution(error) => {
                Self::new("core.distribution-error", error.to_string())
            }
            crate::Error::Migrate(error) => match error {
                crate::migrate::Error::Refused { step, reason } => {
                    Self::new("core.migration-refused", "migration refused")
                        .with_detail("step", step.clone())
                        .with_detail("reason", reason.clone())
                }
            },
        }
    }
}

/// Active WASM/plugin JSON ABI envelope.
///
/// Success shape includes `value` and omits `error`. Failure shape includes a
/// typed `error` object and omits `value`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Envelope<T> {
    pub schema_version: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    pub warnings: Vec<Warning>,
    pub capabilities: Vec<CapabilityReport>,
}

impl<T> Envelope<T> {
    pub fn ok(value: T) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION.to_string(),
            ok: true,
            value: Some(value),
            error: None,
            warnings: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self::err_code("abi.error", message)
    }

    pub fn err_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::err_response(ResponseError::new(code, message))
    }

    pub fn err_response(error: ResponseError) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION.to_string(),
            ok: false,
            value: None,
            error: Some(error),
            warnings: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_warning(mut self, warning: Warning) -> Self {
        self.warnings.push(warning);
        self.warnings.sort();
        self
    }

    pub fn with_capability(mut self, report: CapabilityReport) -> Self {
        self.capabilities.push(report);
        self.capabilities.sort();
        self
    }
}

/// Error-code configuration for JSON ABI helper functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonAbiErrorCodes {
    pub decode_code: &'static str,
    pub decode_message: &'static str,
    pub serialize_code: &'static str,
    pub serialize_message: &'static str,
}

/// Decode a JSON request, run a pure operation, and serialize an ABI envelope.
pub fn decode_then<Request, Value, F, E>(
    input: &str,
    codes: JsonAbiErrorCodes,
    map_error: E,
    f: F,
) -> String
where
    Request: DeserializeOwned,
    Value: Serialize,
    F: FnOnce(Request) -> crate::Result<Value>,
    E: FnOnce(crate::Error) -> ResponseError,
{
    let request = match serde_json::from_str::<Request>(input) {
        Ok(request) => request,
        Err(error) => {
            return envelope_to_json(
                Envelope::<Value>::err_response(
                    ResponseError::new(codes.decode_code, codes.decode_message)
                        .with_detail("serde-error", error.to_string()),
                ),
                codes,
            );
        }
    };

    match f(request) {
        Ok(value) => envelope_to_json(Envelope::ok(value), codes),
        Err(error) => envelope_to_json(Envelope::<Value>::err_response(map_error(error)), codes),
    }
}

/// Like [`decode_then`], but `f` also returns warnings to attach to the
/// success envelope's existing `warnings` field (e.g. legacy canonical-field
/// decode-deprecation warnings that a native caller would print to stderr,
/// but the WASM ABI has no stderr to print to).
pub fn decode_then_with_warnings<Request, Value, F, E>(
    input: &str,
    codes: JsonAbiErrorCodes,
    map_error: E,
    f: F,
) -> String
where
    Request: DeserializeOwned,
    Value: Serialize,
    F: FnOnce(Request) -> crate::Result<(Value, Vec<Warning>)>,
    E: FnOnce(crate::Error) -> ResponseError,
{
    let request = match serde_json::from_str::<Request>(input) {
        Ok(request) => request,
        Err(error) => {
            return envelope_to_json(
                Envelope::<Value>::err_response(
                    ResponseError::new(codes.decode_code, codes.decode_message)
                        .with_detail("serde-error", error.to_string()),
                ),
                codes,
            );
        }
    };

    match f(request) {
        Ok((value, warnings)) => {
            let mut envelope = Envelope::ok(value);
            for warning in warnings {
                envelope = envelope.with_warning(warning);
            }
            envelope_to_json(envelope, codes)
        }
        Err(error) => envelope_to_json(Envelope::<Value>::err_response(map_error(error)), codes),
    }
}

/// Serialize an ABI envelope, returning a typed error envelope if serialization fails.
pub fn envelope_to_json<T: Serialize>(envelope: Envelope<T>, codes: JsonAbiErrorCodes) -> String {
    match serde_json::to_string(&envelope) {
        Ok(json) => json,
        Err(error) => {
            let fallback = Envelope::<()>::err_response(
                ResponseError::new(codes.serialize_code, codes.serialize_message)
                    .with_detail("serde-error", error.to_string()),
            );
            serde_json::to_string(&fallback)
                .expect("serializing unit fallback ABI envelope cannot fail")
        }
    }
}
