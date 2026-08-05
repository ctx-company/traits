//! Pure deterministic `ctx.traits` core.
//!
//! Owns every product behavior that must stay side-effect free: parsing,
//! normalization, validation, audit, reference resolution, composition
//! planning, render planning, context pack planning, cache-key derivation, and
//! procedure ledger planning.
//!
//! This crate is pure. It performs no filesystem, Git, process, host install,
//! MCP, CLI, or network IO and owns no persistent state. Environment actions
//! live in `ctx-traits-io`; command orchestration lives in `ctx-traits-cli`.
//! Unsupported capabilities surface as explicit errors or capability reports,
//! never silent no-ops.
//!
//! # Protocol vs `ctx.traits` runtime
//!
//! The Agent Traits protocol is defined by the public docs under
//! `.protocol/agent-traits/`, not by a crate. This crate implements that protocol
//! plus non-standard `ctx.traits` runtime extensions (CTX-R), keeping
//! ctx-specific fields namespaced under `[extensions.ctx]` / `[x.ctx]` so
//! portable conformance is preserved. It is the inside of the hexagon; product
//! semantics live here, not in the CLI or IO edge.

#![forbid(unsafe_code)]

pub mod agent_model;
pub mod assist;
pub mod audit;
#[cfg(feature = "builtin-trait-packages")]
pub mod builtin_templates;
#[cfg(feature = "builtin-trait-packages")]
pub mod builtin_trait_packages;
pub mod builtins;
pub mod cache;
pub mod capability;
pub mod check;
pub mod context;
pub mod dependency;
pub mod diagnostics;
pub mod diff;
pub mod digest;
pub mod discovery_index;
pub mod distribution;
pub mod encoding;
pub mod error;
pub mod eval_run;
pub mod export;
pub mod import;
pub mod launch;
pub mod lockfile;
pub mod manifest;
pub mod migrate;
pub mod model_view;
pub mod parse;
pub mod procedure;
pub mod project_lock;
pub mod reference;
pub mod render;
pub mod resolve;
pub mod resource_plan;
pub mod response;
pub mod run_info;
pub mod scaffold;
pub mod schema;
pub mod search;
pub mod shared;
pub mod source_map;
pub mod source_plan;
pub mod synth;
pub mod task;
pub mod r#trait;

pub use error::{Error, Result};
pub use r#trait::{Trait, TraitRuntimeShape};
