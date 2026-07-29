//! Host-neutral WASM plugin runtime for `ctx.traits`.
//!
//! Answers host lifecycle events - session created, compaction, tool-after,
//! prompt append, context status - and returns host-neutral actions. It
//! performs no host side effects directly; host adapters (OpenCode, Claude
//! Code, Codex) execute actions with explicit capability reporting. It depends
//! on `ctx-traits-core` for pure behavior.
//!
//! Distinct from `ctx-traits-wasm-core`, which answers direct JSON ABI API
//! calls. This crate answers lifecycle events and translates them into
//! actions the host adapter is responsible for executing.
//!
//! The Agent Traits protocol is defined by the public docs under
//! `.protocol/agent-traits/`; this crate is `ctx.traits` runtime/tooling and owns
//! no part of the portable standard.

pub mod contracts;

pub use contracts::*;
