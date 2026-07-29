//! Launch, hygiene, evidence, and compatibility reports.
//!
//! These reports are pure review surfaces. They summarize canonical trait data
//! and known host/profile capabilities without reading files, calling models,
//! executing hooks, or mutating packages.

include!("claim_evidence.rs");
include!("hygiene.rs");
include!("context_cost.rs");
include!("publish_prep.rs");
include!("context_contract.rs");
include!("policy.rs");
include!("evidence.rs");
include!("compatibility.rs");
include!("subagent.rs");
