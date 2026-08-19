//! Instantiating a first-party teaching template into a draft package
//! (`ctx traits create`).
//!
//! The templates themselves live in `ctx-traits-builtin`, which embeds their
//! authoring files at compile time and holds no logic. What lives here is the
//! one operation over them that needs core: rewriting a template's identity
//! into a new package's `trait.toml` and `source/index.ts`, which decodes and
//! re-encodes a [`PackageManifest`].
//!
//! Core performs no IO here, matching [`crate::builtin_trait_packages`]:
//! `modules/io` and `modules/cli` own claiming the package root and writing
//! the instantiated text to disk.

pub use ctx_traits_builtin::templates::{BuiltinTemplate, template, templates};

use crate::manifest::{PackageManifest, PackageStatus, decode_package_manifest};

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
        "template {id:?} source/index.ts does not contain exactly one `defineTrait({anchor:?}` call site"
    )]
    MissingNameAnchor { id: &'static str, anchor: String },
}

/// A template's authoring files rewritten for a freshly requested trait id
/// and display name. `extra_source_files` passes through byte-identical —
/// the one anchored rewrite (`defineTrait(<name>`) lives only in `source_ts`,
/// per doctrine (`index.ts` declares; the other modules don't carry package
/// identity).
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
/// parser (out of scope per P271), so it is rewritten through one explicit,
/// anchored source marker instead of a broad text replace: the template's own
/// committed display name as it appears in the `defineTrait(<name>, {` call
/// site. A trait function derives its canonical id by kebab-casing that same
/// name, so rewriting the name rewrites the id with it — there is no separate
/// id literal to keep in step. The anchor must occur exactly once; a template
/// whose source doesn't match this shape is an authoring defect caught here
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

    // The anchor carries the opening brace so the replacement can inject a
    // `name` field beside the id when the two differ.
    //
    // `defineTrait`'s first argument is the NAME, and the canonical id is its
    // kebab-casing — so passing a display name alone cannot produce an id
    // that differs from it. `create daily --name "Daily Work"` would build
    // `daily-work` and then fail against a manifest declaring `daily`. The
    // id therefore goes in the positional, where it kebab-cases to itself,
    // and a display name that differs from it is passed explicitly.
    let original_name = template_display_name(template)?;
    let original_name_literal = ts_string_literal(&original_name);
    let name_anchor = format!("defineTrait({original_name_literal}, {{");
    let name_replacement = if display_name == trait_id {
        format!("defineTrait({}, {{", ts_string_literal(trait_id))
    } else {
        format!(
            "defineTrait({}, {{ name: {},",
            ts_string_literal(trait_id),
            ts_string_literal(display_name)
        )
    };
    let name_occurrences = template.source_ts.matches(name_anchor.as_str()).count();
    if name_occurrences != 1 {
        return Err(InstantiateError::MissingNameAnchor {
            id: template.id,
            anchor: name_anchor,
        });
    }
    let source_ts = template
        .source_ts
        .replacen(&name_anchor, &name_replacement, 1);

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
