//! Deterministic synth and LLM-generate boundary contracts.
//!
//! `synth` accepts draft JSON data and normalizes it through the same decode and
//! validation path as hand-authored JSON/TOML/YAML. It does not execute authoring
//! helpers, generators, providers, filesystem operations, or host hooks.
//!
//! `generate` is intentionally separate: it describes the LLM-assisted boundary
//! and provenance needed before any provider adapter exists. Core records the
//! candidate boundary and digests, but never calls a model.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::encoding::{self, Encoding};
use crate::manifest::ProjectManifest;
use crate::r#trait::Trait;

/// Draft document kind accepted by synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DocumentKind {
    /// Infer trait vs project manifest from the JSON object shape.
    Infer,
    /// Canonical trait package root document.
    Trait,
    /// Repo-level `.ctx/traits.*` project manifest.
    ProjectManifest,
}

/// Canonical output encoding requested from synth.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum OutputFormat {
    Toml,
    Json,
    Yaml,
}

impl OutputFormat {
    pub fn parse(value: &str) -> Option<Self> {
        if value == "yml" {
            return Some(Self::Yaml);
        }

        value.parse().ok()
    }

    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn extension(self) -> &'static str {
        self.as_str()
    }

    pub fn encoding(self) -> Encoding {
        match self {
            Self::Toml => Encoding::Toml,
            Self::Json => Encoding::Json,
            Self::Yaml => Encoding::Yaml,
        }
    }
}

/// Provenance fields supplied by the caller before synth normalization.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProvenanceSeed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Stable synth provenance carried by synth reports, lock evidence, and checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path_digest: Option<Digest>,
    pub draft_digest: Digest,
    pub canonical_digest: Digest,
    pub output_digest: Digest,
    pub output_format: OutputFormat,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Pure synth request. The draft must already be parsed JSON data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Request {
    pub document_kind: DocumentKind,
    pub draft_json: serde_json::Value,
    pub output_format: OutputFormat,
    #[serde(default)]
    pub provenance: ProvenanceSeed,
}

/// Normalized canonical document returned by synth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "document-kind", content = "value")]
pub enum CanonicalDocument {
    Trait(Box<Trait>),
    ProjectManifest(Box<ProjectManifest>),
}

/// Complete synth result for CLI, WASM, lock, and check surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Response {
    pub document_kind: DocumentKind,
    pub canonical: CanonicalDocument,
    pub canonical_json: String,
    pub output_text: String,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Normalize draft JSON into a canonical trait or project manifest document.
pub fn synthesize(request: Request) -> crate::Result<Response> {
    let draft_text = crate::digest::canonical_json(&request.draft_json)?;
    let draft_digest = Digest::source(&draft_text);
    let mut warnings = request.provenance.warnings.clone();
    let document_kind =
        resolve_document_kind(request.document_kind, &request.draft_json, &mut warnings)?;

    let output_encoding = request.output_format.encoding();
    let (canonical, canonical_json, canonical_digest, output_text) = match document_kind {
        DocumentKind::Trait => {
            let (trait_ref, decode_warnings) =
                encoding::decode_trait_with_warnings(Encoding::Json, &draft_text)?;
            warnings.extend(decode_warnings);
            let output_text = encoding::encode(output_encoding, &trait_ref)?;
            // The canonical writer never emits `status`/`trust` (Group 95),
            // so the round-trip re-decode is expected to be warning-free;
            // any warning it did produce would mean the writer regressed and
            // started emitting a removed field, which is worth surfacing too.
            let (_, roundtrip_warnings) =
                encoding::decode_trait_with_warnings(output_encoding, &output_text)?;
            warnings.extend(roundtrip_warnings);
            let canonical_json = crate::digest::canonical_json(&trait_ref)?;
            let canonical_digest = crate::digest::canonical_digest(&trait_ref)?;
            (
                CanonicalDocument::Trait(Box::new(trait_ref)),
                canonical_json,
                canonical_digest,
                output_text,
            )
        }
        DocumentKind::ProjectManifest => {
            let manifest = encoding::decode_manifest(Encoding::Json, &draft_text)?;
            let output_text = encoding::encode(output_encoding, &manifest)?;
            encoding::decode_manifest(output_encoding, &output_text)?;
            let canonical_json = crate::digest::canonical_json(&manifest)?;
            let canonical_digest = crate::digest::canonical_digest(&manifest)?;
            (
                CanonicalDocument::ProjectManifest(Box::new(manifest)),
                canonical_json,
                canonical_digest,
                output_text,
            )
        }
        DocumentKind::Infer => {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "document-kind".to_string(),
                message: "internal error: synth document kind was not resolved".to_string(),
            }
            .into());
        }
    };

    warnings.sort();
    warnings.dedup();

    let output_digest = Digest::source(&output_text);
    let source_path_digest = request
        .provenance
        .source_path
        .as_deref()
        .map(Digest::source);
    let provenance = Provenance {
        generator_package: request.provenance.generator_package,
        generator_version: request.provenance.generator_version,
        source_path: request.provenance.source_path,
        source_path_digest,
        draft_digest,
        canonical_digest,
        output_digest,
        output_format: request.output_format,
        warnings: warnings.clone(),
    };

    Ok(Response {
        document_kind,
        canonical,
        canonical_json,
        output_text,
        provenance,
        warnings,
    })
}

fn resolve_document_kind(
    requested: DocumentKind,
    draft: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> crate::Result<DocumentKind> {
    if !matches!(requested, DocumentKind::Infer) {
        return Ok(requested);
    }

    let Some(object) = draft.as_object() else {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "root".to_string(),
            message: "synth draft JSON must be an object".to_string(),
        }
        .into());
    };

    if object.contains_key("id")
        && object.contains_key("name")
        && object.contains_key("summary")
        && object.contains_key("status")
        && object.contains_key("trust")
    {
        warnings.push("synth inferred document-kind=trait from root identity fields".to_string());
        return Ok(DocumentKind::Trait);
    }

    if object.contains_key("project")
        || object.contains_key("trait")
        || object.contains_key("dependency")
        || object.contains_key("extends")
    {
        warnings
            .push("synth inferred document-kind=project-manifest from manifest fields".to_string());
        return Ok(DocumentKind::ProjectManifest);
    }

    Err(crate::manifest::Error::InvalidField {
        field_path: "root".to_string(),
        message: "cannot infer synth document kind; pass draft JSON with trait identity fields or project manifest fields".to_string(),
    }.into())
}

/// Request for the LLM-assisted generate boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GenerateRequest {
    pub provided_name: String,
    pub brief: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub output_path: String,
    pub package_path: String,
    pub provider_available: bool,
}

/// Deterministic boundary/provenance report for `ctx traits generate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GenerateBoundaryReport {
    pub llm_assisted: String,
    pub provider_available: bool,
    pub provider: String,
    pub model: String,
    pub provided_name: String,
    pub slugified_trait_id: String,
    pub brief_digest: Digest,
    pub prompt_context_digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_digest: Option<Digest>,
    pub output_path: String,
    pub package_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_warnings: Vec<String>,
    pub final_result: String,
}

/// Build generate provenance without calling a model provider.
pub fn plan_generate_boundary(request: GenerateRequest) -> crate::Result<GenerateBoundaryReport> {
    let slugified_trait_id = slugify_trait_id(&request.provided_name)?;
    let model = request
        .model
        .unwrap_or_else(|| "provider-default".to_string());
    let brief_digest = Digest::source(&request.brief);
    let context = serde_json::json!({
        "brief": request.brief,
        "model": model,
        "output-path": request.output_path,
        "package-path": request.package_path,
        "provided-name": request.provided_name,
        "slugified-trait-id": slugified_trait_id,
    });
    let context_json = crate::digest::canonical_json(&context)?;
    let prompt_context_digest = Digest::canonical(&context_json);
    let final_result = if request.provider_available {
        "candidate-required-validation"
    } else {
        "unsupported-provider"
    };
    let validation_warnings = if request.provider_available {
        Vec::new()
    } else {
        vec![
            "no model provider adapter is implemented; no candidate was produced or written"
                .to_string(),
        ]
    };

    Ok(GenerateBoundaryReport {
        llm_assisted: "always".to_string(),
        provider_available: request.provider_available,
        provider: "model-provider-adapter".to_string(),
        model,
        provided_name: request.provided_name,
        slugified_trait_id,
        brief_digest,
        prompt_context_digest,
        candidate_digest: None,
        output_path: request.output_path,
        package_path: request.package_path,
        validation_warnings,
        final_result: final_result.to_string(),
    })
}

/// Slugify a human name into a trait ID candidate.
pub fn slugify_trait_id(name: &str) -> crate::Result<String> {
    let mut slug = String::new();
    let mut last_was_hyphen = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen && !slug.is_empty() {
            slug.push('-');
            last_was_hyphen = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "name".to_string(),
            message: "cannot slugify name into a trait id".to_string(),
        }
        .into());
    }

    Ok(slug)
}
