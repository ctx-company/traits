//! `ctx.traits` CLI orchestration edge.
//!
//! Owns command parsing, output formatting, and exit codes. Product semantics
//! live in `ctx-traits-core`; environment actions and shared IO orchestration
//! live in `ctx-traits-io`.
//!
//! The app boundary lives in `app`: `app::entry` routes commands and converts
//! results to exit status, `app::surface` parses CLI arguments, and product
//! contexts keep their own formatting modules where needed.
//!
//! The Agent Traits protocol is defined by the public docs under
//! `.protocol/agent-traits/`; this crate is `ctx.traits` runtime/tooling and owns
//! no part of the portable standard.

pub mod app;

pub use app::error::{Error, Result};
