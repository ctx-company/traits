//! Model run ledger: pure model for planned procedure execution state.
//!
//! This module models what a procedure run *will* do without executing it.
//! It records planned sequence item states, slot states, producer edges
//! (sequence-item-to-slot), port requirements, output-port completion, and
//! acceptance state. The dry planner is pure and deterministic — it never
//! calls providers, tools, models, hosts, filesystem, Git, process, network,
//! or renderers.
//!
//! This ledger is distinct from any model context-retention ledger. It tracks
//! procedure orchestration state, not model-visible compiled context.

include!("model.rs");
include!("planner.rs");
