//! Canonical relations: trait-to-trait requires, suggests, and targetless
//! conflicts.
//!
//! `[[relations.requires]]` and `[[relations.suggests]]` use a `target` ref.
//! `[[relations.conflicts]]` is targetless — it applies to the declaring trait
//! when `when` conditions match. All entries carry `when` conditions (`rule:*`
//! or `signal:*` refs, AND within one list, OR across multiple entries) and a
//! required `reason`.
//!
//! Model never install, download, build, trust, activate, execute, or bind
//! a trait by themselves; they only create explained requirements, suggestions,
//! conflicts, and binding proposals that pass gates.

include!("model.rs");
include!("validate.rs");
include!("graph.rs");
include!("evaluate.rs");
include!("binding.rs");
include!("port_compat.rs");
include!("binding_proposals.rs");
include!("port_compat_collect.rs");
include!("evaluate_targets.rs");

/// Validate an optional relations section.
pub fn validate_optional(relations: Option<&Model>) -> crate::Result<()> {
    if let Some(relations) = relations {
        validate(relations)?;
    }
    Ok(())
}

/// Graph-building names scoped under `relations::graph`.
pub mod graph {
    pub use super::{Cycle, Edge, EdgeKind, Graph, Node, build_graph as build};
}

/// Evaluation names scoped under `relations::evaluate`.
pub mod evaluate {
    pub use super::{
        EdgeEffect, EdgeEvaluation, Evaluation, PortTargetOutcome, PortTargetOutcomeKind, evaluate,
    };
}

/// Binding proposal names scoped under `relations::binding`.
pub mod binding {
    pub use super::{
        Compatibility, FieldMapping, Proposal, Status, produce_proposals,
        produce_proposals_for_port,
    };
}

/// Port-compatibility names scoped under `relations::port_compat`.
pub mod port_compat {
    pub use super::{ComparisonPort, Evidence, Outcome, collect_compatibility as collect, compare};
}
