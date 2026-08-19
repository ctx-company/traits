//! The first-party assets ctx.traits ships inside its binary, embedded at
//! compile time by `build.rs`.
//!
//! Three buckets, and the split between them is the reason this crate exists
//! separately from core:
//!
//! - [`vocabulary`] — the guidance catalog (intent, and behavior's eight
//!   axes). Built unconditionally: core's renderer resolves guidance through
//!   it, and wasm-core needs it.
//! - [`trait_packages`] — the built-in meta-traits, plus the shared `spec`
//!   package they depend on. Behind the `trait-packages` feature.
//! - [`templates`] — the scaffolds `ctx traits create` materializes. Behind
//!   the same feature.
//!
//! The feature gate is not symmetry for its own sake: the WASM crates leave
//! it off because embedding the packages would grow every wasm artifact by
//! their full size, while the vocabulary is small and load-bearing everywhere.
//! Without the feature the generated tables are still present, simply empty,
//! so no caller needs `cfg` to name these modules.
//!
//! This crate is data. It holds no logic beyond lookups over its own tables,
//! and depends on nothing in the workspace — core depends on it, so anything
//! that reached back the other way would be a cycle. Operations that
//! interpret these bytes (instantiating a template, publishing a package to
//! the runtime store) live in core and io.

pub mod templates;
pub mod trait_packages;
pub mod vocabulary;
