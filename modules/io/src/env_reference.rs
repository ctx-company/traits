//! P492: the single shipped inventory of every environment variable product
//! code reads, with each variable's contract. `ctx traits doctor --config`
//! renders [`env_reference`] in its `environment` section — the
//! source-scanning `proof_env_reference` test keeps this table complete, and
//! `just testhook-absence-check` keeps the four test-hook names below out of
//! release binaries. This is the one place either guard, or a reader, needs
//! to look.

/// How a variable's contract binds a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvVarKind {
    /// A documented, user-facing override or input.
    UserFacing,
    /// An internal parent→child contract, never intended for a human to set
    /// directly.
    Internal,
    /// Present only in a debug-profile binary (`#[cfg(debug_assertions)]`);
    /// compiled out of `--release` builds entirely — see
    /// `just testhook-absence-check`.
    DebugOnlyTestHook,
}

/// One `ENV_REFERENCE` row: the variable's name, its contract in prose, and
/// its [`EnvVarKind`].
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnvVarDoc {
    pub name: &'static str,
    pub contract: &'static str,
    pub kind: EnvVarKind,
}

/// P402 test-hook name: forces a checkpoint after a whole wave's outcomes
/// are durably persisted but before any is applied to the parent ledger.
/// Compiled out of release builds — see `test_only_checkpoint` in
/// `modules/cli/src/app/drive.rs`. `#[cfg(debug_assertions)]`-gated at the
/// definition itself (not just at its call sites) so the byte string cannot
/// survive into a release binary through this constant.
#[cfg(debug_assertions)]
pub const TESTHOOK_CHECKPOINT_WAVE_PERSISTED: &str =
    "CTX_INTERNAL_TESTHOOK_CHECKPOINT_WAVE_PERSISTED";

/// P402 test-hook name: forces a checkpoint after exactly one wave unit's
/// outcome has been applied to the parent ledger. Compiled out of release
/// builds — see `test_only_checkpoint` in `modules/cli/src/app/drive.rs`.
#[cfg(debug_assertions)]
pub const TESTHOOK_CHECKPOINT_ONE_APPLIED: &str = "CTX_INTERNAL_TESTHOOK_CHECKPOINT_ONE_APPLIED";

/// P402 test-hook name: forces the terminal sidecar write for the named
/// ordinal to behave as a persistence failure. Compiled out of release
/// builds — see `test_only_fail_terminal_write` in
/// `modules/cli/src/app/drive.rs`.
#[cfg(debug_assertions)]
pub const TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL: &str =
    "CTX_INTERNAL_TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL";

/// P402 test-hook name: forces the reservation write for the named ordinal
/// to behave as a persistence failure. Compiled out of release builds — see
/// `test_only_fail_reservation_write` in `modules/cli/src/app/drive.rs`.
#[cfg(debug_assertions)]
pub const TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL: &str =
    "CTX_INTERNAL_TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL";

/// Every environment variable product code reads, with its contract. Kept
/// complete by `modules/cli/tests/proof_env_reference.rs`, which walks
/// `modules/*/src/**/*.rs` for quoted `CTX_`-prefixed literals and asserts
/// each appears here. `CTX_TRAITS_PROMPT_*`/`CTX_TRAITS_INPUT_*`/
/// `CTX_TRAITS_RESOURCE_*` are prompt delimiter markers built by
/// `format!("CTX_TRAITS_{kind}_{suffix}")` (`frame_prompt.rs`), not env
/// vars, and never appear as a source literal the scan can match — they are
/// deliberately absent from this table.
///
/// A function rather than a fixed-size `static` slice: the four
/// `CTX_INTERNAL_TESTHOOK_*` rows are appended only `#[cfg(debug_assertions)]`,
/// so a release build's reference (and `doctor --config`'s rendering of it)
/// omits them entirely — matching those hooks' own compile-out, rather than
/// merely labeling them absent while still shipping their name strings.
pub fn env_reference() -> Vec<EnvVarDoc> {
    let mut entries = base_env_reference();
    entries.extend(testhook_env_reference());
    entries
}

#[cfg(not(debug_assertions))]
fn testhook_env_reference() -> [EnvVarDoc; 0] {
    []
}

/// Unit-test handoff: the scratch root a `startup_observer_tests` child
/// process reads its fixture layout from. Declared here rather than only as a
/// `#[cfg(test)]` literal in `run.rs` because `proof_env_reference` scans every
/// `CTX_`-prefixed literal under `modules/*/src/**` — a name a test invents is
/// still a name, and the reference is what makes it discoverable.
#[cfg(debug_assertions)]
pub const TESTHOOK_STARTUP_OBSERVER_TEST_ROOT: &str = "CTX_TRAITS_STARTUP_OBSERVER_TEST_ROOT";

/// Unit-test handoff: the config home a `trust` child process resolves its
/// trust store under, so the test never touches the developer's real one.
#[cfg(debug_assertions)]
pub const TESTHOOK_TRUST_TEST_CONFIG_HOME: &str = "CTX_TRAITS_TRUST_TEST_CONFIG_HOME";

/// Unit-test fixture: a deliberately-unset `api-key-env` reference name used
/// by 0079's `resolve_api_seat` degrade-path tests to prove a missing key
/// reference falls back to the harness declaration rather than failing the
/// run. Never actually set in any environment.
#[cfg(debug_assertions)]
pub const TESTHOOK_API_TRANSPORT_MISSING_KEY: &str = "CTX_TEST_NONEXISTENT_API_KEY_0079";

#[cfg(debug_assertions)]
fn testhook_env_reference() -> [EnvVarDoc; 7] {
    [
        EnvVarDoc {
            name: TESTHOOK_CHECKPOINT_WAVE_PERSISTED,
            contract: "P402 fault injection: blocks the drive loop once a whole wave's outcomes are durably persisted but unapplied. A no-op unless set to a filesystem path. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_CHECKPOINT_ONE_APPLIED,
            contract: "P402 fault injection: blocks the drive loop once exactly one wave unit's outcome has been applied to the parent ledger. A no-op unless set to a filesystem path. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_FAIL_TERMINAL_WRITE_ORDINAL,
            contract: "P402 fault injection: forces the terminal sidecar write for the given ordinal to fail. A no-op unless set to an ordinal. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_FAIL_RESERVATION_WRITE_ORDINAL,
            contract: "P402 fault injection: forces the reservation write for the given ordinal to fail. A no-op unless set to an ordinal. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_STARTUP_OBSERVER_TEST_ROOT,
            contract: "Unit-test parent→child handoff: the scratch root a `startup_observer_tests` child reads its fixture layout from. Set only by that test. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_TRUST_TEST_CONFIG_HOME,
            contract: "Unit-test parent→child handoff: the config home a `trust` child resolves its trust store under, keeping the test off the developer's real store. Set only by that test. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
        EnvVarDoc {
            name: TESTHOOK_API_TRANSPORT_MISSING_KEY,
            contract: "Unit-test fixture: a deliberately-unset `api-key-env` reference name proving 0079's `resolve_api_seat` degrades instead of failing when the key does not resolve. Never actually set. Absent from release builds.",
            kind: EnvVarKind::DebugOnlyTestHook,
        },
    ]
}

/// Resolves a config-declared environment-variable *reference* by name
/// (0069's channel-secrets doctrine, reused by 0079's `api-key-env`): config
/// stores the variable's NAME only, and this is the one place its VALUE is
/// ever read. The value comes back as a [`crate::secret::Secret`], so
/// "never serialized, logged, or echoed back" is the type's guarantee, not
/// this comment's. An unset or empty-string variable resolves to `None`,
/// the same "absent" the caller degrades on either way.
pub fn resolve_env_var_reference(name: &str) -> Option<crate::secret::Secret> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(crate::secret::Secret::new)
}

fn base_env_reference() -> Vec<EnvVarDoc> {
    vec![
        EnvVarDoc {
            name: "CTX_CONFIG",
            contract: "An extra config-layer file path, appended last — it outranks every repo and global config layer.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "CTX_TRAITS_REGISTRY_BASE",
            contract: "Overrides `[registry] base`; the top layer of the npm registry base URL resolution.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "CTX_TRAITS_ELAPSED_SECONDS_BASELINE",
            contract: "Internal parent→child only: the cumulative active-drive elapsed seconds a drive loop injects into the MCP server subprocess it spawns. A bare `ctx traits mcp` honors it if set. Unparsable or absent is always treated as absent — a malformed baseline never silently starts the clock at zero.",
            kind: EnvVarKind::Internal,
        },
        EnvVarDoc {
            name: "HOME",
            contract: "Home directory root, read on Unix (with `USERPROFILE` as the Windows fallback) to locate the user-global `ctx` config/state root.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "USERPROFILE",
            contract: "Windows home directory root — the fallback `HOME` resolution reads when `HOME` itself is absent.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "XDG_CONFIG_HOME",
            contract: "XDG config-home root; when set and non-empty, relocates the user-global `ctx` config/state root beneath it instead of `~/.config`.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "NO_COLOR",
            contract: "When set (to any value, including empty), disables ANSI color/styling in terminal output.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "TERM",
            contract: "Terminal type; a value of `dumb` disables interactive/styled rendering and falls back to plain output.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "CI",
            contract: "When set (to any value), signals a non-interactive CI environment and disables interactive/styled rendering.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "COLUMNS",
            contract: "Terminal width in columns, read as a fallback when the real terminal size cannot be queried.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "LINES",
            contract: "Terminal height in lines, read as a fallback when the real terminal size cannot be queried.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: "EDITOR",
            contract: "External editor command invoked for interactive edit prompts.",
            kind: EnvVarKind::UserFacing,
        },
        EnvVarDoc {
            name: crate::run_liveness::SPAWNED_LOG_PATH_ENV,
            contract: "P510 internal parent→child only: the log path a detached-spawn parent hands its child driver, read once at `try_acquire` time into the local liveness-index row's `log_path` so `ctx traits running` can name a live run's log without guessing its spawn-time filename.",
            kind: EnvVarKind::Internal,
        },
    ]
}
