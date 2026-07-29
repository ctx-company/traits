//! Import planning: data model and report for importing Agent Skills-compatible
//! directories into canonical trait packages.
//!
//! Import preserves raw files, infers canonical fields, records unsupported
//! fields, and recommends review actions. Imported external content always
//! defaults to package status=draft and machine trust=unreviewed. This
//! module is pure — it plans; IO executes the actual file copies and writes.

include!("source.rs");
include!("checklist.rs");
include!("agent_skills.rs");
include!("lock.rs");
include!("refresh.rs");
include!("graph.rs");
include!("remote.rs");

pub mod doctor;
