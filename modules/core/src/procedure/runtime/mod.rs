//! Pure executable procedure runtime state.
//!
//! This module advances caller-supplied procedure outputs through declared
//! contracts. It does not call models, providers, tools, filesystems, clocks,
//! UUID generators, renderers, hosts, MCP, Git, process APIs, or networks.
//! Runtime ledgers store accepted values and validation evidence only; frames
//! may expose prompt text for a caller, but frames are not persisted in the
//! ledger by this module.

include!("state.rs");
include!("frames.rs");
include!("transitions.rs");
include!("readiness.rs");
include!("transitions_rejection.rs");
include!("ledger_contract.rs");
include!("ledger_contract_maps.rs");
include!("ledger_contract_outputs.rs");
include!("schema_validation.rs");
include!("signals.rs");
include!("control_flow.rs");
include!("guards.rs");
include!("control_signals.rs");
include!("frame_builders.rs");
include!("provider_error.rs");
