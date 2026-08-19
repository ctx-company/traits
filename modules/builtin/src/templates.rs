//! The embedded first-party teaching templates consumed by `ctx traits
//! create`.
//!
//! Templates are deterministic authoring inputs, not resolvable runtime
//! traits: they are never embedded into [`crate::trait_packages`]'s catalog,
//! never materialized under the runtime built-in trait store, and never
//! appear in `ctx traits list`'s built-in section. `build.rs` embeds each
//! template's authoring files (`trait.toml`, every `source/**/*.ts`) as
//! static UTF-8 strings; this module only exposes them.
//!
//! Instantiation — rewriting a template's identity into a new draft
//! package's authoring files — lives in core, not here: it decodes and
//! re-encodes a package manifest, and this crate holds no logic that would
//! make core's dependency on it circular.

/// One embedded first-party template: its stable id, its manifest, its
/// anchored entry module (`source/index.ts`), and every other committed
/// `source/**/*.ts` file as `(relative_path, contents)` pairs, relative to
/// `source/` with forward-slash separators. A template small enough to stay
/// single-file simply has an empty `extra_source_files`.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTemplate {
    pub id: &'static str,
    pub trait_toml: &'static str,
    pub source_ts: &'static str,
    pub extra_source_files: &'static [(&'static str, &'static str)],
}

include!(concat!(env!("OUT_DIR"), "/builtin_templates.rs"));

/// List all first-party templates, in the fixed, sorted order `ctx traits
/// create` (bare) reports them.
pub fn templates() -> &'static [BuiltinTemplate] {
    BUILTIN_TEMPLATES
}

/// Look up a template by id.
pub fn template(id: &str) -> Option<&'static BuiltinTemplate> {
    BUILTIN_TEMPLATES.iter().find(|template| template.id == id)
}
