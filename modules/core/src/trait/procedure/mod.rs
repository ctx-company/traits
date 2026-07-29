//! Model: deterministic sequence orchestration.
//!
//! `[procedure]` is an optional top-level section that declares an ordered
//! sequence of items (`[[procedure.sequence]]`) along with procedure-level
//! boundary input/output ports. The procedure wires sequence items together:
//! outputs from earlier items become available as inputs to later items.
//!
//! P25 validates procedure wiring deterministically at decode time. It does
//! not execute sequence items, resolve providers, load dependencies, or check
//! resource existence.

include!("model.rs");
include!("validate.rs");
include!("command.rs");
include!("prompt_contract.rs");
include!("command_contract.rs");

/// Command declaration names scoped under `trait::procedure::command`.
pub mod command {
    pub use super::{
        CommandDeclaration as Declaration, CommandPlan as Plan,
        command_plan_for_item as plan_for_item, parse_command_shorthand as parse_shorthand,
    };
}

/// Sequence item names scoped under `trait::procedure::sequence`.
pub mod sequence {
    pub use super::{SequenceItem as Item, SequenceKind as Kind};
}
