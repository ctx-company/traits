//! Pure WASM JSON ABI over `ctx.traits` core.
//!
//! Exposes the pure core as a stable JSON ABI surface: validate, audit,
//! resolve, compose, render-plan, pack, and explain. It performs no filesystem,
//! Git, process, or network IO; host dynamic support exists only when proven by
//! capability reports and integration evidence. It depends on `ctx-traits-core`
//! and stays pure so the same ABI runs in any WASM host.
//!
//! Distinct from `ctx-traits-wasm-plugin`, which answers host lifecycle
//! events and returns host-neutral actions. This crate answers direct API
//! calls.
//!
//! The Agent Traits protocol is defined by the public docs under
//! `.protocol/agent-traits/`; this crate is `ctx.traits` runtime/tooling and owns
//! no part of the portable standard.

pub mod abi;

pub use abi::*;
