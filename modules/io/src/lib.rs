//! `ctx.traits` IO boundary.
//!
//! Owns every interaction with the outside environment: filesystem reads and
//! writes, Git source resolution, process execution, native host installation,
//! MCP server integration, persistent cache, and persistent ledger storage. It
//! depends on `ctx-traits-core` for pure behavior and translates core's typed
//! IO plans into concrete environment actions with explicit capability
//! reporting.
//!
//! This is the outside of the hexagon. Product semantics live in
//! `ctx-traits-core`; this crate only executes environment actions and
//! reports capability. Unsupported capabilities are explicit errors or
//! capability reports, never silent no-ops.
//!
//! The Agent Traits protocol is defined by the public docs under
//! `.protocol/agent-traits/`; this crate is `ctx.traits` runtime/tooling and owns
//! no part of the portable standard.

pub mod activity_sidecar;
pub mod audit_journal;
pub mod builtin_store;
pub mod cache;
pub mod cdk_build;
pub mod command;
pub mod config_document;
pub mod config_source;
pub mod confinement;
pub mod context_ledger;
pub mod debug_trace;
pub mod decode_diagnostics;
pub mod dependency;
pub mod discovery;
pub mod dispatch_preflight;
pub mod distribution;
pub mod env_reference;
pub mod environment;
pub mod error;
pub mod export;
pub mod family_manifest;
pub mod file_lock;
pub mod git_process;
pub mod gitignore;
pub mod harness;
pub mod harness_activity;
pub mod harness_config;
pub mod host_install;
pub mod import;
pub mod init;
pub mod inventory;
pub mod layout;
pub mod lifecycle;
pub mod lockfile;
pub mod mcp;
pub mod mcp_server;
pub mod merge_lock;
pub mod package_scaffold;
pub mod parse;
pub mod path_safety;
pub mod process;
pub mod project_lock;
pub mod provider_client;
pub mod publish;
pub mod read;
pub mod registry;
pub mod repository;
pub mod resource;
pub mod retention;
pub mod run;
pub mod run_branch;
pub mod run_control;
pub mod run_kill;
pub mod run_liveness;
pub mod run_query;
pub mod run_queue;
pub mod run_session;
pub mod run_summary;
pub mod state;
pub mod target_slot;
pub mod task_board_cache;
pub mod task_files;
pub mod tripwire;
pub mod trust;
pub mod worktree;
pub mod write;

pub use error::{Error, Result};

/// Serialises tests that change something the WHOLE PROCESS shares, against
/// tests that spawn child processes and expect an ordinary environment.
///
/// Cargo runs a crate's tests as parallel threads in one binary, so a test
/// that applies a Seatbelt sandbox is not isolated from its siblings: any
/// child spawned by another thread during that window inherits the restricted
/// environment. That is how the startup-observer fixture failed in a landing
/// gate and nowhere else — its `git add -A` staged nothing, so `git commit`
/// exited non-zero, and a bare `assert!` reported it as an anonymous failure.
///
/// Every test on either side of that boundary takes this lock. Poisoning is
/// ignored deliberately: a panicking test must not convert every later one
/// into a second failure that hides it.
#[cfg(test)]
pub(crate) static PROCESS_WIDE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_process_wide_test() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_WIDE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
