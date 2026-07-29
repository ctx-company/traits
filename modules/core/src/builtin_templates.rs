//! First-party teaching templates consumed by `ctx traits new` (P271).
//!
//! Templates are deterministic authoring inputs, not resolvable runtime
//! traits: they are never embedded into [`crate::builtin_trait_packages`]'s
//! catalog, never materialized under the runtime built-in trait store, and
//! never appear in `ctx traits list`'s built-in section. `build.rs` embeds
//! each template's two authoring files (`trait.toml`, `source/index.ts`) as
//! static UTF-8 strings; this module only exposes them and the pure,
//! marker-based instantiation operation `ctx traits new` calls to produce a
//! draft package's authoring files.
//!
//! Core performs no IO here, matching [`crate::builtin_trait_packages`]:
//! `modules/io` and `modules/cli` own claiming the package root and writing
//! the instantiated text to disk.

use crate::manifest::{PackageManifest, PackageStatus, decode_package_manifest};

/// One embedded first-party template: its stable id, its manifest, its
/// anchored entry module (`source/index.ts`), and every other committed
/// `source/**/*.ts` file (P533's doctrine layout — `data.ts`, `schema.ts`,
/// `agent.ts`, `sequence/<concept>.ts`) as `(relative_path, contents)`
/// pairs, relative to `source/` with forward-slash separators. A template
/// small enough to stay single-file (the doctrine's own never-oversplit
/// clause) simply has an empty `extra_source_files`.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTemplate {
    pub id: &'static str,
    pub trait_toml: &'static str,
    pub source_ts: &'static str,
    pub extra_source_files: &'static [(&'static str, &'static str)],
}

include!(concat!(env!("OUT_DIR"), "/builtin_templates.rs"));

/// List all first-party templates, in the fixed, sorted order `ctx traits
/// new` (bare) reports them.
pub fn templates() -> &'static [BuiltinTemplate] {
    BUILTIN_TEMPLATES
}

/// Look up a template by id.
pub fn template(id: &str) -> Option<&'static BuiltinTemplate> {
    BUILTIN_TEMPLATES.iter().find(|template| template.id == id)
}

/// One-line purpose text for a template, read from its committed
/// `[package].description`, so the listing never drifts from the template's
/// own authored metadata.
pub fn purpose(template: &BuiltinTemplate) -> Result<String, InstantiateError> {
    let manifest = decode_manifest(template)?;
    Ok(manifest
        .package
        .description
        .unwrap_or_else(|| manifest.package.name.unwrap_or_default()))
}

/// Instantiation error: the template's own committed authoring files did
/// not match the exact shape [`instantiate`] expects to rewrite.
#[derive(Debug, thiserror::Error)]
pub enum InstantiateError {
    #[error("template {id:?} trait.toml is not a valid package manifest: {source}")]
    Manifest {
        id: &'static str,
        #[source]
        source: Box<crate::Error>,
    },
    #[error("template {id:?} trait.toml is missing a required [package] field: {field}")]
    MissingField {
        id: &'static str,
        field: &'static str,
    },
    #[error(
        "template {id:?} source/index.ts does not contain exactly one `trait({anchor:?}` call site"
    )]
    MissingIdAnchor { id: &'static str, anchor: String },
    #[error(
        "template {id:?} source/index.ts does not contain exactly one `name: {anchor:?}` field"
    )]
    MissingNameAnchor { id: &'static str, anchor: String },
}

/// A template's authoring files rewritten for a freshly requested trait id
/// and display name. `extra_source_files` passes through byte-identical —
/// the two anchored rewrites (`trait(<id>` and `name: <name>`) live only in
/// `source_ts`, per doctrine (`index.ts` declares; the other modules don't
/// carry package identity).
#[derive(Debug, Clone)]
pub struct InstantiatedTemplate {
    pub trait_toml: String,
    pub source_ts: String,
    pub extra_source_files: Vec<(&'static str, &'static str)>,
}

/// Rewrite a template's committed `trait.toml` and `source/index.ts` for a
/// new `trait_id`/`display_name`, leaving everything else byte-identical.
///
/// `trait.toml` is rewritten structurally: decode as a [`PackageManifest`],
/// overwrite `package.id`/`package.name`/`package.status` (always reset to
/// `draft` regardless of the template's own committed status), re-encode.
///
/// `source/index.ts` cannot be rewritten structurally without a TypeScript
/// parser (out of scope per P271), so it is rewritten through two explicit,
/// anchored source markers instead of a broad text replace: the template's
/// own id as it appears in the `trait(<id>, {` call site, and the
/// template's own committed display name as it appears in the `name:
/// <name>` field. Each anchor must occur exactly once; a template whose
/// source doesn't match this shape is an authoring defect caught here
/// rather than silently mis-substituted.
pub fn instantiate(
    template: &BuiltinTemplate,
    trait_id: &str,
    display_name: &str,
) -> Result<InstantiatedTemplate, InstantiateError> {
    let mut rewritten = decode_manifest(template)?;
    rewritten.package.id = trait_id.to_string();
    rewritten.package.name = Some(display_name.to_string());
    rewritten.package.status = PackageStatus::Draft;
    let trait_toml =
        crate::encoding::encode(crate::encoding::Encoding::Toml, &rewritten).map_err(|source| {
            InstantiateError::Manifest {
                id: template.id,
                source: Box::new(source),
            }
        })?;

    let original_id_literal = ts_string_literal(template.id);
    let id_anchor = format!("trait({original_id_literal}");
    let id_replacement = format!("trait({}", ts_string_literal(trait_id));
    let id_occurrences = template.source_ts.matches(id_anchor.as_str()).count();
    if id_occurrences != 1 {
        return Err(InstantiateError::MissingIdAnchor {
            id: template.id,
            anchor: id_anchor,
        });
    }
    let with_id = template.source_ts.replacen(&id_anchor, &id_replacement, 1);

    let original_name = template_display_name(template)?;
    let original_name_literal = ts_string_literal(&original_name);
    let name_anchor = format!("name: {original_name_literal}");
    let name_replacement = format!("name: {}", ts_string_literal(display_name));
    let name_occurrences = with_id.matches(name_anchor.as_str()).count();
    if name_occurrences != 1 {
        return Err(InstantiateError::MissingNameAnchor {
            id: template.id,
            anchor: name_anchor,
        });
    }
    let source_ts = with_id.replacen(&name_anchor, &name_replacement, 1);

    Ok(InstantiatedTemplate {
        trait_toml,
        source_ts,
        extra_source_files: template.extra_source_files.to_vec(),
    })
}

fn decode_manifest(template: &BuiltinTemplate) -> Result<PackageManifest, InstantiateError> {
    decode_package_manifest(template.trait_toml, "trait.toml")
        .map_err(|source| InstantiateError::Manifest {
            id: template.id,
            source: Box::new(source),
        })?
        .ok_or(InstantiateError::MissingField {
            id: template.id,
            field: "package",
        })
}

fn template_display_name(template: &BuiltinTemplate) -> Result<String, InstantiateError> {
    let manifest = decode_manifest(template)?;
    manifest.package.name.ok_or(InstantiateError::MissingField {
        id: template.id,
        field: "package.name",
    })
}

/// Encode `text` as a double-quoted TypeScript/JSON string literal. Every
/// character a user-supplied display name can contain — quotes,
/// backslashes, newlines — round-trips through a valid escape sequence.
fn ts_string_literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}
