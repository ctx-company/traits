//! Harness registry and runtime configuration parsing at the IO boundary.
//!
//! Trait source stays portable: this module reads machine-local harness and
//! agent-role configuration from `.ctx/config.toml`, layers `--assign`
//! overrides on top, validates the result against a loaded trait, probes
//! configured harness binaries, and produces opaque run-session assignment
//! evidence for core.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer};

/// The one registry-adjacent mapping from configured harness kind to its
/// native activity adapter. Custom harnesses intentionally have no guessed
/// adapter and report no native activity instead.
pub fn activity_adapter_kind(
    harness: &HarnessDefinition,
) -> Option<crate::harness_activity::HarnessActivityAdapterKind> {
    crate::harness_activity::HarnessActivityAdapterKind::from_harness_kind(harness.kind())
}

pub use crate::layout::{
    GLOBAL_RUNTIME_CONFIG, HARNESS_REGISTRY, LEGACY_CTX_GLOBAL_RUNTIME_CONFIG,
    LEGACY_CTX_RUNTIME_CONFIG, LEGACY_GLOBAL_RUNTIME_CONFIG, LEGACY_HARNESS_REGISTRY,
    LEGACY_RUNTIME_CONFIG, PROJECT_CONFIG, RUNTIME_CONFIG,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessRegistry {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub harness: BTreeMap<String, HarnessDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessDefinition {
    /// P568: `None` means "not stated here". For a built-in id that inherits
    /// the compiled-in value; for a custom harness it falls back to
    /// [`Self::kind`]'s `"custom"` default. Optional so a config table can
    /// state ONLY its deltas — see [`built_in_harness_definition`].
    #[serde(default)]
    pub kind: Option<String>,
    /// P568: `None` inherits the built-in's binary. Validation still requires
    /// a non-empty resolved bin, so a custom harness must state one.
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub transports: Vec<RunTransport>,
    #[serde(default)]
    pub version_probe: Vec<String>,
    #[serde(default)]
    pub cli: Option<HarnessCliConvention>,
    #[serde(default)]
    pub mcp: Option<HarnessMcpConvention>,
}

/// The kind a harness gets when nothing states one: a user-defined harness
/// with no declared dialect. P568 moved this from a serde default (which could
/// not tell "omitted" from "stated as custom") to a resolution-time fallback.
const CUSTOM_HARNESS_KIND: &str = "custom";

impl HarnessDefinition {
    /// The resolved kind: what was stated, else the `"custom"` default a
    /// user-defined harness gets.
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or(CUSTOM_HARNESS_KIND)
    }

    /// The resolved binary. Empty when nothing stated it, which validation
    /// rejects — a harness with no binary cannot be spawned.
    pub fn bin(&self) -> &str {
        self.bin.as_deref().unwrap_or_default()
    }

    /// P568: overlay `self` (a config table) onto `base` (the compiled-in
    /// definition) field by field.
    ///
    /// SHALLOW by design: a stated field REPLACES the base's value outright —
    /// `narrator-argv` never concatenates with the built-in's, it supersedes
    /// it. Only a field the config does not state at all is inherited. That
    /// keeps a config table readable as "the deltas", while leaving each
    /// individual value exactly what the author wrote.
    ///
    /// A list field is "not stated" when empty, matching how every other
    /// config layer in this file treats list overrides.
    fn merged_onto(&self, base: &HarnessDefinition) -> HarnessDefinition {
        HarnessDefinition {
            kind: self.kind.clone().or_else(|| base.kind.clone()),
            bin: self.bin.clone().or_else(|| base.bin.clone()),
            transports: if self.transports.is_empty() {
                base.transports.clone()
            } else {
                self.transports.clone()
            },
            version_probe: if self.version_probe.is_empty() {
                base.version_probe.clone()
            } else {
                self.version_probe.clone()
            },
            cli: match (self.cli.as_ref(), base.cli.as_ref()) {
                (Some(over), Some(under)) => Some(over.merged_onto(under)),
                (Some(over), None) => Some(over.clone()),
                (None, under) => under.cloned(),
            },
            mcp: match (self.mcp.as_ref(), base.mcp.as_ref()) {
                (Some(over), Some(under)) => Some(over.merged_onto(under)),
                (Some(over), None) => Some(over.clone()),
                (None, under) => under.cloned(),
            },
        }
    }
}

/// P568: resolve one optional string field under merge.
///
/// `None` inherits. `Some("")` is an EXPLICIT UNSET — it suppresses the
/// built-in's value instead of inheriting it. Without this, a field the
/// built-in declares could never be turned off, and a config that legitimately
/// conflicts with an inherited field (the `warm-argv` x `json-schema-flag`
/// exclusion is the live case) would be unfixable rather than merely wrong.
fn merge_flag(over: &Option<String>, under: &Option<String>) -> Option<String> {
    match over {
        Some(value) if value.is_empty() => None,
        Some(value) => Some(value.clone()),
        None => under.clone(),
    }
}

impl HarnessCliConvention {
    /// Whether this harness streams. `None` is not `false`: it means the
    /// config did not state it, and the built-in's value stands.
    pub fn stream(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    /// Field-wise overlay; see [`HarnessDefinition::merged_onto`].
    fn merged_onto(&self, base: &HarnessCliConvention) -> HarnessCliConvention {
        HarnessCliConvention {
            argv: if self.argv.is_empty() {
                base.argv.clone()
            } else {
                self.argv.clone()
            },
            narrator_argv: self
                .narrator_argv
                .clone()
                .or_else(|| base.narrator_argv.clone()),
            warm_argv: self.warm_argv.clone().or_else(|| base.warm_argv.clone()),
            json_schema_flag: merge_flag(&self.json_schema_flag, &base.json_schema_flag),
            model_flag: merge_flag(&self.model_flag, &base.model_flag),
            reasoning_effort_flag: merge_flag(
                &self.reasoning_effort_flag,
                &base.reasoning_effort_flag,
            ),
            system_prompt_flag: merge_flag(&self.system_prompt_flag, &base.system_prompt_flag),
            resume_flag: merge_flag(&self.resume_flag, &base.resume_flag),
            session_flag: merge_flag(&self.session_flag, &base.session_flag),
            dir_flag: merge_flag(&self.dir_flag, &base.dir_flag),
            prompt_via: merge_flag(&self.prompt_via, &base.prompt_via),
            stream: self.stream.or(base.stream),
            output: merge_flag(&self.output, &base.output),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessCliConvention {
    #[serde(default)]
    pub argv: Vec<String>,
    /// Optional lightweight argv for one-shot narrator calls, used instead of
    /// `argv` (which is tuned for streaming worker runs). Keep it minimal — e.g.
    /// `["-p"]` for claude so it prints plain text — so narration stays small and
    /// fast and does not blow past the capture limit or timeout.
    #[serde(default)]
    pub narrator_argv: Option<Vec<String>>,
    /// Optional resident argv for warm process mode. This is currently a
    /// claude-stream-json-only protocol: callers write one JSON user message per
    /// turn and read until a `type:"result"` event.
    #[serde(default)]
    pub warm_argv: Option<Vec<String>>,
    #[serde(default)]
    pub json_schema_flag: Option<String>,
    #[serde(default)]
    pub model_flag: Option<String>,
    #[serde(default)]
    pub reasoning_effort_flag: Option<String>,
    #[serde(default)]
    pub system_prompt_flag: Option<String>,
    #[serde(default)]
    pub resume_flag: Option<String>,
    #[serde(default)]
    pub session_flag: Option<String>,
    /// Flag that pins the harness to an execution directory (e.g. opencode's
    /// `--dir`). Needed when the harness resolves its project from a running
    /// server instead of the spawned process cwd; without it, worktree runs
    /// execute against the wrong checkout.
    #[serde(default)]
    pub dir_flag: Option<String>,
    #[serde(default)]
    pub prompt_via: Option<String>,
    /// P568: `None` inherits; it is NOT the same as `false`. A bare `bool`
    /// here meant that omitting the key silently disabled streaming, which is
    /// the exact silent-drop this merge model exists to end.
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HarnessMcpConvention {
    #[serde(default)]
    pub mcp_config_flag: Option<String>,
    #[serde(default)]
    pub allowed_tools_flag: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub system_prompt_flag: Option<String>,
    #[serde(default)]
    pub reasoning_effort_flag: Option<String>,
    #[serde(default)]
    pub config_via: Option<String>,
}

impl HarnessMcpConvention {
    /// Ordered tool lists replace as a complete declaration while omitted
    /// scalar leaves inherit from the farther convention.
    fn merged_onto(&self, base: &HarnessMcpConvention) -> HarnessMcpConvention {
        HarnessMcpConvention {
            mcp_config_flag: merge_flag(&self.mcp_config_flag, &base.mcp_config_flag),
            allowed_tools_flag: merge_flag(&self.allowed_tools_flag, &base.allowed_tools_flag),
            allowed_tools: if self.allowed_tools.is_empty() {
                base.allowed_tools.clone()
            } else {
                self.allowed_tools.clone()
            },
            system_prompt_flag: merge_flag(&self.system_prompt_flag, &base.system_prompt_flag),
            reasoning_effort_flag: merge_flag(
                &self.reasoning_effort_flag,
                &base.reasoning_effort_flag,
            ),
            config_via: merge_flag(&self.config_via, &base.config_via),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum RunTransport {
    Cli,
    Mcp,
    Api,
}

impl RunTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Api => "api",
        }
    }
}

/// Which wire format a `transport = "api"` seat's endpoint speaks (0079):
/// OpenAI-compatible `/chat/completions` (the baseline — OpenRouter, proxies,
/// local servers) or Anthropic's `/v1/messages`. Declared on the seat as
/// `wire`, never inferred from `base-url`, so a self-hosted or unfamiliar
/// host name never has to be pattern-matched to guess its shape.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderWire {
    OpenaiCompat,
    Anthropic,
}

impl ProviderWire {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompat => "openai-compat",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum RunAssignmentMode {
    #[default]
    Harness,
    Attach,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum RunSessionMode {
    #[default]
    PerFrame,
    Persistent,
}

impl RunSessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerFrame => "per-frame",
            Self::Persistent => "persistent",
        }
    }
}

/// `[worktree]` seed and setup configuration for `--worktree` runs:
/// repository-relative gitignored context roots to copy into the created
/// worktree after checkout, followed by ordered literal-argv setup commands
/// (e.g. `["make", "setup"]`) executed in the new
/// worktree once seeding finishes. Tracked package-relative resources need no
/// seed entry — `git worktree add` already checks them out. Setup commands
/// are literal argv lists, never shell strings, and only run while creating a
/// fresh worktree; resuming an already-registered worktree reruns neither
/// seeding nor setup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorktreeConfig {
    #[serde(default)]
    pub seed: Vec<String>,
    #[serde(default)]
    pub setup: Vec<Vec<String>>,
    /// Opt-in, generic environment overlay applied to every child process
    /// launched *inside* a run worktree (setup commands, default-input and
    /// procedure command steps, harness probes, cold/persistent harnesses,
    /// narrators, and merger dispatch). Deterministic `BTreeMap` ordering is
    /// load-bearing: it drives merge order, the resolved overlay's application
    /// sequence, and the persistent-process pool identity key. Values whose
    /// text starts with `.ctx/`, `./`, or `../` are treated as
    /// repository-relative paths and resolved against the invocation
    /// repository root (never the generated worktree) when the effective
    /// overlay is computed; every other value is an opaque scalar. These bytes
    /// are operational only — they never enter canonical documents, digests,
    /// ledgers, or reports. Empty by default, in which case behavior is
    /// byte-identical to a run with no overlay.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Effective `[run.build-cache.<name>]` declarations (P428), folded in
    /// by `resolve_runtime_assignments_impl` so every consumer that already
    /// threads `WorktreeConfig` (run, drive/resume, merge dispatch) picks up
    /// named build-cache exports for free. Never itself an authored
    /// `[worktree]` TOML field -- `deny_unknown_fields` on this struct still
    /// rejects a stray `[worktree] build-cache` table, since declarations
    /// only belong under `[run.build-cache.<name>]`.
    #[serde(skip)]
    #[schemars(skip)]
    pub build_cache: BTreeMap<String, BuildCacheConfig>,
    /// P478 harness-native write confinement, generated and injected per
    /// spawn for every process launched inside a run worktree. Defaults to
    /// enabled with the OS sandbox layer on; see
    /// [`crate::confinement::WorktreeConfinementConfig`] for field semantics.
    #[serde(default)]
    pub confinement: crate::confinement::WorktreeConfinementConfig,
    /// P479: out-of-tree mutation tripwire, snapshotting the invocation
    /// repository around each `--worktree` frame boundary. Defaults to
    /// `policy = "park"` with no extra `sentinel` entries; see
    /// [`crate::tripwire::WorktreeTripwireConfig`] for field semantics.
    #[serde(default)]
    pub tripwire: crate::tripwire::WorktreeTripwireConfig,
    /// P489: per-command wall-clock ceiling, in seconds, for every declared
    /// `[worktree] setup` command. `None` resolves to
    /// [`crate::worktree::DEFAULT_SETUP_TIMEOUT_MS`].
    #[serde(default)]
    pub setup_seconds: Option<u64>,
    /// P489: stdout/stderr capture ceiling, in bytes, for every declared
    /// `[worktree] setup` command. `None` resolves to
    /// [`crate::worktree::DEFAULT_SETUP_CAPTURE_BYTES`] — generous enough that
    /// a failing installer's own diagnostic output survives.
    #[serde(default)]
    pub setup_capture_bytes: Option<u64>,
    /// Declared regenerable worktree paths. Only these paths may be removed
    /// automatically after a run; branches and uncommitted source are never
    /// candidates for retention cleanup.
    #[serde(default)]
    pub retention: WorktreeRetentionConfig,
    /// P564: repository-relative directories copy-on-write cloned from the
    /// invocation checkout into the freshly created worktree, at the same
    /// relative path, so a per-worktree build cache starts warm instead of
    /// cold. Distinct from [`Self::seed`] in both mechanism and intent: a
    /// seed is CONTEXT (copied byte-for-byte, baselined, and harvestable
    /// back), a warm entry is a REGENERABLE ARTIFACT (cloned, never
    /// baselined, never harvested, and safe to lose). Cloning is refused
    /// rather than degraded to a byte copy on a filesystem without
    /// copy-on-write support — see [`crate::worktree::warm_worktree_paths`].
    #[serde(default)]
    pub warm: Vec<String>,
}

/// P511 retention policy for derived worktree artifacts. A missing table is
/// deliberately conservative: no path is inferred from a tool name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorktreeRetentionConfig {
    /// Cheap, high-volume artifacts deleted at every drive exit.
    #[serde(default)]
    pub cheap: Vec<String>,
    /// Expensive artifacts retained for a warm resume; expiry sweeping is
    /// deliberately separate from the terminal cheap-tier cleanup.
    #[serde(default)]
    pub expensive: Vec<String>,
    /// Optional override for the default seven-day expensive-tier grace.
    #[serde(default)]
    pub expensive_grace_days: Option<u64>,
}

/// One named build-artifact cache declaration
/// (`[run.build-cache.<name>] env = "..."`, P428): the environment variable
/// a build tool reads to find its cache directory (an ecosystem-specific
/// value). The directory itself is never authored here -- it is
/// always `<global-per-repository-cache-root>/build/<name>`, resolved
/// through the same `.ctx/cache/...` overlay redirect every other
/// repository-relative `[worktree.env]` value uses (see
/// `resolve_worktree_env_value`), so a declared cache never needs its own
/// path-resolution code path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildCacheConfig {
    pub env: String,
}

/// Resolve a `[worktree].env` overlay for execution inside a run worktree.
///
/// A `.ctx/cache` (or `.ctx/cache/<rest>`) value is redirected through the
/// P426 global per-repository cache root (`crate::state::global_cache_root`)
/// instead of the repository checkout, so a named build-artifact path such
/// as `.ctx/cache/build/<name>` (P428's per-declaration export target) lands
/// under `~/.config/ctx/cache/<repo-key>/build/<name>` rather than
/// repo-local `.ctx/cache`. Every other repository-relative path value — text starting
/// with `.ctx/`, `./`, or `../` — is resolved against `repo_root` (the
/// invocation repository root discovered via
/// [`crate::repository::discover_repo_root`]). Absolute values and every
/// non-path scalar pass through verbatim: only the three explicit relative
/// prefixes are treated as paths, so an arbitrary literal is never
/// guess-detected as a filesystem path. Iterating a `BTreeMap` preserves
/// deterministic key ordering.
///
/// P564: a value opening with the `{worktree}` token instead resolves
/// against `worktree_root` — THIS run's generated worktree, not the shared
/// invocation checkout. It is the only form that yields a per-run path, and
/// it is how a build-artifact cache stops being shared across concurrent
/// runs. When `worktree_root` is `None` (a host-side spawn with no worktree
/// in play) the entry names a directory that does not exist for this run, so
/// it is DROPPED from the overlay rather than resolved against a fallback:
/// a host-side `cargo` then falls back to its own default, which is correct,
/// where silently pointing it at some other run's worktree would not be.
pub fn resolve_worktree_env_overlay(
    env: &BTreeMap<String, String>,
    repo_root: &Utf8Path,
    worktree_root: Option<&Utf8Path>,
) -> crate::Result<BTreeMap<String, String>> {
    env.iter()
        .filter_map(|(key, value)| {
            resolve_worktree_env_value(value, repo_root, worktree_root)
                .transpose()
                .map(|resolved| Ok((key.clone(), resolved?)))
        })
        .collect()
}

/// The `[worktree.env]` token that resolves against the run's own generated
/// worktree rather than the invocation checkout (P564).
const WORKTREE_ENV_TOKEN: &str = "{worktree}";

/// The `[worktree.env]` token that resolves to a STABLE build-cache directory
/// leased to this run's worktree (0057).
///
/// `{worktree}` gives every run its own path, which is what keeps concurrent
/// runs from colliding — but a path that is new every run is a build cache
/// that is cold every run, because every toolchain anchors freshness to
/// absolute paths. This token keeps the isolation and drops the coldness: it
/// resolves to one of a small fixed set of directories under the repository's
/// global cache root, leased so that no two LIVE worktrees hold the same one
/// and a finished run's warmed directory is handed to the next run. See
/// [`crate::target_slot`].
const CACHE_SLOT_ENV_TOKEN: &str = "{cache-slot}";

fn resolve_worktree_env_value(
    value: &str,
    repo_root: &Utf8Path,
    worktree_root: Option<&Utf8Path>,
) -> crate::Result<Option<String>> {
    if value == WORKTREE_ENV_TOKEN || value.starts_with(&format!("{WORKTREE_ENV_TOKEN}/")) {
        let rest = value
            .strip_prefix(WORKTREE_ENV_TOKEN)
            .unwrap_or_default()
            .trim_start_matches('/');
        // A traversal would escape the worktree and land back in the shared
        // checkout — reintroducing exactly the cross-run collision this token
        // exists to end. Rejected at resolution, not merely discouraged.
        if rest.split('/').any(|segment| segment == "..") {
            return invalid_config(
                "worktree.env",
                format!("{WORKTREE_ENV_TOKEN} value {value:?} must not contain a '..' segment"),
            );
        }
        let Some(worktree_root) = worktree_root else {
            return Ok(None);
        };
        return Ok(Some(if rest.is_empty() {
            worktree_root.to_string()
        } else {
            worktree_root.join(rest).to_string()
        }));
    }
    if value == CACHE_SLOT_ENV_TOKEN || value.starts_with(&format!("{CACHE_SLOT_ENV_TOKEN}/")) {
        let rest = value
            .strip_prefix(CACHE_SLOT_ENV_TOKEN)
            .unwrap_or_default()
            .trim_start_matches('/');
        if rest.split('/').any(|segment| segment == "..") {
            return invalid_config(
                "worktree.env",
                format!("{CACHE_SLOT_ENV_TOKEN} value {value:?} must not contain a '..' segment"),
            );
        }
        // No worktree means no lease to take: a host-side command falls back
        // to its own default rather than borrowing a slot some run may hold.
        let Some(worktree_root) = worktree_root else {
            return Ok(None);
        };
        let main_root = crate::repository::discover_main_repo_root(repo_root)?;
        let canonical = crate::state::canonical_repo_root(&main_root)?;
        let key = crate::state::repo_key(&canonical);
        let slots_root = crate::state::global_cache_root(&key)?.join("build-slots");
        let slot = crate::target_slot::resolve(
            &slots_root,
            worktree_root,
            crate::target_slot::DEFAULT_TARGET_SLOTS,
        )?;
        return Ok(Some(if rest.is_empty() {
            slot.to_string()
        } else {
            slot.join(rest).to_string()
        }));
    }
    resolve_shared_worktree_env_value(value, repo_root).map(Some)
}

fn resolve_shared_worktree_env_value(value: &str, repo_root: &Utf8Path) -> crate::Result<String> {
    if value == ".ctx/cache" || value.starts_with(".ctx/cache/") {
        // Build caches belong to the checkout that owns linked worktrees, not
        // to each generated worktree's distinct Git top-level path.
        let repo_root = crate::repository::discover_main_repo_root(repo_root)?;
        let canonical = crate::state::canonical_repo_root(&repo_root)?;
        let key = crate::state::repo_key(&canonical);
        let cache_root = crate::state::global_cache_root(&key)?;
        let rest = value.strip_prefix(".ctx/cache").unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return Ok(if rest.is_empty() {
            cache_root.to_string()
        } else {
            cache_root.join(rest).to_string()
        });
    }
    if value.starts_with(".ctx/") || value.starts_with("./") || value.starts_with("../") {
        return Ok(repo_root.join(value).to_string());
    }
    Ok(value.to_string())
}

/// Resolve the ONE effective worktree environment overlay for a subprocess
/// that runs inside a run worktree: `[worktree].env` plus, for every P428
/// `[run.build-cache.<name>]` declaration folded into `worktree.build_cache`
/// (see [`WorktreeConfig::build_cache`]), that cache's environment variable
/// pointing at `.ctx/cache/build/<name>` -- which [`resolve_worktree_env_value`]
/// already redirects through the global per-repository cache root exactly
/// like any other `.ctx/cache/...` overlay value, so named build caches need
/// no separate path-resolution code path. An explicit `[worktree.env]` entry
/// for the same variable name wins over a same-named cache export. Empty
/// overlays never discover the invocation repository root -- so runs that
/// declare no overlay stay byte-identical and pay no extra filesystem probe
/// -- and a non-empty overlay is resolved via [`resolve_worktree_env_overlay`]
/// against the invocation repository root (never the generated worktree).
/// Every caller that needs the effective overlay (run start, drive/resume,
/// merge dispatch) shares this one resolution path so repo-root discovery
/// and path-resolution semantics cannot drift between call sites.
pub fn resolve_effective_worktree_env(
    worktree: &WorktreeConfig,
    worktree_root: Option<&Utf8Path>,
) -> crate::Result<BTreeMap<String, String>> {
    let raw = combined_worktree_env(worktree);
    if raw.is_empty() {
        return Ok(BTreeMap::new());
    }
    let repo_root = crate::repository::discover_repo_root()?;
    resolve_worktree_env_overlay(&raw, &repo_root, worktree_root)
}

/// The raw (unresolved) `[worktree.env]` overlay plus one
/// `.ctx/cache/build/<name>` pseudo-path entry per declared build-cache
/// export, before [`resolve_worktree_env_overlay`] redirects any of it
/// through the global cache root.
fn combined_worktree_env(worktree: &WorktreeConfig) -> BTreeMap<String, String> {
    if worktree.build_cache.is_empty() {
        return worktree.env.clone();
    }
    let mut combined = worktree.env.clone();
    for (name, cache) in &worktree.build_cache {
        combined
            .entry(cache.env.clone())
            .or_insert_with(|| format!(".ctx/cache/build/{name}"));
    }
    combined
}

/// Optional package-root `config.toml` sidecar (P312): committed run
/// defaults that travel with the trait package. Scope is strictly
/// budget-only — `deny_unknown_fields` makes an `[assign]`, `[worktree]`, or
/// any harness/model/session field a hard decode error naming the field,
/// rather than a silently ignored table, so a vendored trait can never bind
/// lifecycle, harness, or model selection; those stay machine-private in
/// `.ctx/config.toml [agent.role.<role>]`. Not canonical bytes: tuning it
/// never re-baselines the trait's digest.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TraitRunConfig {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub budget: RunProfileBudget,
    #[serde(default)]
    pub defaults: PortDefaults,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PortDefaults {
    #[serde(default)]
    pub port: BTreeMap<String, String>,
}

/// Committed per-package `runtime.toml` (0036): the AUTHOR's budget-only
/// successor to [`TraitRunConfig`]'s `config.toml` sidecar and to a family
/// manifest's per-variant `run-config` declarations. Top-level budget keys
/// are the package default; a `[variant.<vid>]` table overlays them (stated
/// replaces, omitted inherits — [`overlay_budget`]).
///
/// Decoded manually rather than via `#[serde(flatten)]`, because serde
/// silently disables `deny_unknown_fields` under `flatten` — the schema must
/// keep rejecting `[assign]`, `[worktree]`, harness, or model exactly as the
/// legacy sidecar did. **Permission narrows as authority moves away from the
/// machine owner**: the machine tier (`.ctx/traits/runtime.toml`) gets the
/// full schema, the package tier gets `[budget]` (plus `[defaults.port]`)
/// only — do not widen this for symmetry with the machine tier.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackageRuntimeConfig {
    pub schema_version: Option<String>,
    pub budget: RunProfileBudget,
    pub defaults: PortDefaults,
    /// `[variant.<vid>]`: budget-only overlay tables, keyed by variant id.
    /// Never contains an entry for the family's default variant — its
    /// budget is expressed at the top level.
    pub variant: BTreeMap<String, RunProfileBudget>,
}

impl PackageRuntimeConfig {
    fn decode(text: &str, path: &Utf8Path) -> crate::Result<Self> {
        let table: toml::Table =
            toml::from_str(text).map_err(|source| crate::parse::Error::TomlDecode {
                context: path.to_string(),
                source,
            })?;
        let mut schema_version = None;
        let mut defaults = PortDefaults::default();
        let mut variant = BTreeMap::new();
        let mut budget_table = toml::Table::new();
        for (key, value) in table {
            match key.as_str() {
                "schema-version" => {
                    schema_version = value.as_str().map(str::to_string);
                }
                "defaults" => {
                    defaults =
                        value
                            .try_into()
                            .map_err(|source| crate::parse::Error::TomlDecode {
                                context: format!("{path} [defaults]"),
                                source,
                            })?;
                }
                "variant" => {
                    let table = value
                        .as_table()
                        .cloned()
                        .ok_or_else(|| crate::Error::Usage {
                            message: format!("{path}: `variant` must be a table"),
                        })?;
                    for (name, entry) in table {
                        let entry_table =
                            entry
                                .as_table()
                                .cloned()
                                .ok_or_else(|| crate::Error::Usage {
                                    message: format!("{path}: [variant.{name}] must be a table"),
                                })?;
                        let budget: RunProfileBudget =
                            toml::Value::Table(entry_table)
                                .try_into()
                                .map_err(|source| crate::parse::Error::TomlDecode {
                                    context: format!("{path} [variant.{name}]"),
                                    source,
                                })?;
                        variant.insert(name, budget);
                    }
                }
                _ => {
                    budget_table.insert(key, value);
                }
            }
        }
        let budget: RunProfileBudget =
            toml::Value::Table(budget_table)
                .try_into()
                .map_err(|source| crate::parse::Error::TomlDecode {
                    context: path.to_string(),
                    source,
                })?;
        Ok(Self {
            schema_version,
            budget,
            defaults,
            variant,
        })
    }
}

/// Which tier supplied the resolved package-level run config, most-current
/// first. Surfaced to `ctx traits check` (`run-config-sidecar-active`) so an
/// author can tell whether a package is already on the current `runtime.toml`
/// shape or still needs migrating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageRunConfigTier {
    /// Committed `runtime.toml` ([`PackageRuntimeConfig`]).
    Runtime,
    /// Legacy family-manifest-declared per-variant `run-config` file.
    LegacyDeclared,
    /// Legacy package-root `config.toml` sidecar ([`TraitRunConfig`]).
    LegacySidecar,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TraitDefaults {
    #[serde(default)]
    pub defaults: PortDefaults,
    /// 0034: `[trait.<id>.agent…]` — a trait-scoped seat, same
    /// [`AgentDefaults`] shape `RepoOverride` already holds. Beats a matching
    /// `[repo.<key>]` qualifier (the more specific scope) and folds
    /// field-wise onto the global seat via [`fold_role`]/[`combine_role_level`],
    /// the one shared fold every consumer reads the flattened result of. A
    /// non-empty `agent.variant` here is a hard config error — the canonical
    /// spelling for a trait-scoped variant is `variant.<vid>.agent`, not
    /// `agent.variant.<vid>`, so the grammar has exactly one spelling.
    #[serde(default)]
    pub agent: AgentDefaults,
    /// 0034: `[trait.<id>.variant.<vid>.agent…]` — one variant of this
    /// trait. Reusable by 0037 for `[trait.<id>.variant.<vid>.budget]`.
    #[serde(default)]
    pub variant: BTreeMap<String, TraitVariantDefaults>,
}

/// One `[trait.<id>.variant.<vid>]` block (0034): a trait-and-variant-scoped
/// `AgentDefaults`. A non-empty `agent.variant` here is likewise a hard
/// config error — see [`TraitDefaults::agent`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TraitVariantDefaults {
    #[serde(default)]
    pub agent: AgentDefaults,
}

/// Narrow caller-selected runtime profile for `ctx traits import
/// --run-profile <PATH>` (P403): scoped to `[assign.<role>]` and `[budget]`
/// only. `deny_unknown_fields` makes a `[worktree]` (or any other) table a
/// hard decode error naming the field, so this file can never revive
/// caller-selected worktree authority or other profile infrastructure P314
/// retired. This affects only the harness-backed `--llm-assisted` path of
/// import; it is unrelated to the `--profile` source-format selector.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RunProfileDocument {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub assign: BTreeMap<String, RunProfileAssignment>,
    #[serde(default)]
    pub budget: RunProfileBudget,
}

/// One role's routing entry inside a `--run-profile` document: harness,
/// transport, session-mode, and model-tier routing only. `deny_unknown_fields`
/// makes a concrete `model`, `system-prompt`, `extra-args`, `reasoning-effort`,
/// or `mode` (attach) a hard decode error, so a profile can never carry a
/// field the narrower Family Axis profile boundary excludes — those stay
/// machine-local in `.ctx/config.toml [agent.role.<role>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RunProfileAssignment {
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub transport: Option<RunTransport>,
    #[serde(default)]
    pub session_mode: Option<RunSessionMode>,
    #[serde(default)]
    pub model_tier: Option<ctx_traits_core::r#trait::AgentModelTier>,
}

impl RunProfileAssignment {
    /// Convert into the generic assignment representation the shared
    /// merge/model-resolution path consumes, always in harness mode (a
    /// run profile has no attach-mode concept) with no concrete model,
    /// system-prompt, reasoning-effort, or extra-args of its own.
    pub fn into_profile_assignment(self) -> ProfileAssignment {
        ProfileAssignment {
            replace_inherited: false,
            model_selector: None,
            model_resolution_reason: None,
            mode: RunAssignmentMode::Harness,
            mode_authored: true,
            harness: self.harness,
            transport: self.transport,
            session_mode: self.session_mode,
            model: None,
            model_tier: self.model_tier,
            reasoning_effort: None,
            system_prompt: None,
            extra_args: Vec::new(),
            budget: RoleBudget::default(),
            api: Box::default(),
            count: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub harness: BTreeMap<String, HarnessDefinition>,
    #[serde(default)]
    pub agent: AgentDefaults,
    #[serde(default, rename = "trait")]
    pub trait_defaults: BTreeMap<String, TraitDefaults>,
    /// P342 worktree preparation: gitignored seed roots, ordered literal-argv
    /// setup commands, and the environment overlay applied inside a run
    /// worktree. `.ctx/config.toml [worktree]` is the single source for
    /// worktree creation, drive/resume, and merge — all three read this one
    /// resolved `WorktreeConfig`.
    #[serde(default)]
    pub worktree: WorktreeConfig,
    /// P441 host-placement overrides, keyed by host ID. Merges global-to-project
    /// like every other `BTreeMap` field: a project-scoped `[host.<name>]`
    /// table field-by-field overrides the same host's global-scoped table,
    /// and an entry present in only one scope survives untouched.
    #[serde(default)]
    pub host: BTreeMap<String, HostOverride>,
    /// Project-fact execution policy. Values remain optional while config
    /// layers are merged so an absent table preserves the historical default.
    #[serde(default)]
    pub run: Option<RunTable>,
    /// Project-fact landing policy.
    #[serde(default)]
    pub merge: Option<MergeTable>,
    /// 0063.4 board-dispatch policy: the trait id the board's `d` dispatch
    /// seeds into the spawn modal by default.
    #[serde(default)]
    pub tasks: Option<TasksTable>,
    /// P489 git process timeout policy.
    #[serde(default)]
    pub git: Option<GitTable>,
    /// P489 publish pack-exclude policy.
    #[serde(default)]
    pub publish: Option<PublishTable>,
    /// P492 registry base URL policy.
    #[serde(default)]
    pub registry: Option<RegistryTable>,
    /// P451: `[repo."<key>"]` blocks, keyed by the P426 repo registry key.
    /// Accepted only in the carried GLOBAL config file — a non-empty entry
    /// declared in any other layer is a hard config error (see
    /// [`resolve_config_report`]).
    #[serde(default)]
    pub repo: BTreeMap<String, RepoOverride>,
    /// Requirement declarations captured from the authored TOML document.
    /// Serde defaults erase the distinction between an absent `false`/empty
    /// value and an explicitly authored one, but repository requirements need
    /// that distinction when protecting individual leaves from CTX_CONFIG.
    #[serde(skip)]
    #[schemars(skip)]
    authored_requirements: BTreeMap<ConfigLeaf, AuthoredConfigLeaf>,
    /// `$CTX_CONFIG` agent defaults are applied after the matching personal
    /// qualifier has been flattened. Keeping them transient prevents a
    /// personal `[repo.*]` assignment from incorrectly beating the explicit
    /// environment layer.
    #[serde(skip)]
    #[schemars(skip)]
    environment_agent: AgentDefaults,
    /// Agent defaults before the final environment layer. Assignment
    /// resolution needs this base to apply repo qualifiers before CTX_CONFIG,
    /// while the report's public `agent` value remains fully collapsed.
    #[serde(skip)]
    #[schemars(skip)]
    pre_environment_agent: AgentDefaults,
}

/// The policy assigned to an authored runtime-config leaf. Keeping this next
/// to parse-time presence prevents serde defaults from turning an explicitly
/// authored requirement into an ordinary fallback value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigSemantic {
    Default,
    Requirement,
    Additive,
}

#[derive(Debug, Clone, PartialEq)]
struct AuthoredConfigLeaf {
    semantic: ConfigSemantic,
    value: toml::Value,
}

impl Eq for AuthoredConfigLeaf {}

/// Every stable, authored RuntimeConfig leaf. Dynamic map entries are
/// deliberately represented by their table shape; their existing per-key
/// additive resolution remains outside this catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ConfigLeaf {
    SchemaVersion,
    TraitDynamic,
    WorktreeSeed,
    WorktreeWarm,
    WorktreeSetup,
    WorktreeSetupSeconds,
    WorktreeSetupCaptureBytes,
    WorktreeConfinementEnabled,
    WorktreeConfinementSandbox,
    WorktreeConfinementAllow,
    WorktreeEnv,
    WorktreeTripwirePolicy,
    WorktreeTripwireSentinel,
    WorktreeRetentionCheap,
    WorktreeRetentionExpensive,
    WorktreeRetentionExpensiveGraceDays,
    RunWorktree,
    RunMaxFrames,
    RunFrameSeconds,
    RunTotalSeconds,
    RunMaxRetries,
    RunAttachWaitSeconds,
    RunIdleSeconds,
    /// 0058 `[run] command-seconds`: absolute backstop for command steps.
    RunCommandSeconds,
    /// 0058 `[run] command-idle-seconds`: silence window for command steps.
    RunCommandIdleSeconds,
    RunMaxInFlight,
    RunWait,
    RunStrictLoops,
    RunBuildCache,
    RunInlinePromptBytes,
    RunStory,
    MergeWait,
    MergeOverlap,
    MergeAuto,
    MergeDeep,
    MergeBranch,
    MergeGate,
    MergeGateSeconds,
    MergeGenerated,
    MergeDiskFloorMb,
    MergeRetryAttempts,
    MergeRetryBackoffMs,
    GitLongSeconds,
    PublishExclude,
    RegistryBase,
    TasksDispatchTrait,
    TasksAutoClose,
    HarnessDynamic,
    AgentDynamic,
    HostDynamic,
    RepoDynamic,
}

impl ConfigLeaf {
    const ALL: &[Self] = &[
        Self::SchemaVersion,
        Self::TraitDynamic,
        Self::WorktreeSeed,
        Self::WorktreeWarm,
        Self::WorktreeSetup,
        Self::WorktreeSetupSeconds,
        Self::WorktreeSetupCaptureBytes,
        Self::WorktreeConfinementEnabled,
        Self::WorktreeConfinementSandbox,
        Self::WorktreeConfinementAllow,
        Self::WorktreeEnv,
        Self::WorktreeTripwirePolicy,
        Self::WorktreeTripwireSentinel,
        Self::WorktreeRetentionCheap,
        Self::WorktreeRetentionExpensive,
        Self::WorktreeRetentionExpensiveGraceDays,
        Self::RunWorktree,
        Self::RunMaxFrames,
        Self::RunFrameSeconds,
        Self::RunTotalSeconds,
        Self::RunMaxRetries,
        Self::RunAttachWaitSeconds,
        Self::RunIdleSeconds,
        Self::RunCommandSeconds,
        Self::RunCommandIdleSeconds,
        Self::RunMaxInFlight,
        Self::RunWait,
        Self::RunStrictLoops,
        Self::RunBuildCache,
        Self::RunInlinePromptBytes,
        Self::RunStory,
        Self::MergeWait,
        Self::MergeOverlap,
        Self::MergeAuto,
        Self::MergeDeep,
        Self::MergeBranch,
        Self::MergeGate,
        Self::MergeGateSeconds,
        Self::MergeGenerated,
        Self::MergeDiskFloorMb,
        Self::MergeRetryAttempts,
        Self::MergeRetryBackoffMs,
        Self::GitLongSeconds,
        Self::PublishExclude,
        Self::RegistryBase,
        Self::TasksDispatchTrait,
        Self::TasksAutoClose,
        Self::HarnessDynamic,
        Self::AgentDynamic,
        Self::HostDynamic,
        Self::RepoDynamic,
    ];

    fn path(self) -> &'static str {
        match self {
            Self::SchemaVersion => "schema-version",
            Self::TraitDynamic => "trait",
            Self::WorktreeSeed => "worktree.seed",
            Self::WorktreeWarm => "worktree.warm",
            Self::WorktreeSetup => "worktree.setup",
            Self::WorktreeSetupSeconds => "worktree.setup-seconds",
            Self::WorktreeSetupCaptureBytes => "worktree.setup-capture-bytes",
            Self::WorktreeConfinementEnabled => "worktree.confinement.enabled",
            Self::WorktreeConfinementSandbox => "worktree.confinement.sandbox",
            Self::WorktreeConfinementAllow => "worktree.confinement.allow",
            Self::WorktreeEnv => "worktree.env",
            Self::WorktreeTripwirePolicy => "worktree.tripwire.policy",
            Self::WorktreeTripwireSentinel => "worktree.tripwire.sentinel",
            Self::WorktreeRetentionCheap => "worktree.retention.cheap",
            Self::WorktreeRetentionExpensive => "worktree.retention.expensive",
            Self::WorktreeRetentionExpensiveGraceDays => "worktree.retention.expensive-grace-days",
            Self::RunWorktree => "run.worktree",
            Self::RunMaxFrames => "run.max-frames",
            Self::RunFrameSeconds => "run.frame-seconds",
            Self::RunTotalSeconds => "run.total-seconds",
            Self::RunMaxRetries => "run.max-retries",
            Self::RunAttachWaitSeconds => "run.attach-wait-seconds",
            Self::RunIdleSeconds => "run.idle-seconds",
            Self::RunCommandSeconds => "run.command-seconds",
            Self::RunCommandIdleSeconds => "run.command-idle-seconds",
            Self::RunMaxInFlight => "run.max-in-flight",
            Self::RunWait => "run.wait",
            Self::RunStrictLoops => "run.strict-loops",
            Self::RunBuildCache => "run.build-cache",
            Self::RunInlinePromptBytes => "run.inline-prompt-bytes",
            Self::RunStory => "run.story",
            Self::MergeWait => "merge.wait",
            Self::MergeOverlap => "merge.overlap",
            Self::MergeAuto => "merge.auto",
            Self::MergeDeep => "merge.deep",
            Self::MergeBranch => "merge.branch",
            Self::MergeGate => "merge.gate",
            Self::MergeGateSeconds => "merge.gate-seconds",
            Self::MergeGenerated => "merge.generated",
            Self::MergeDiskFloorMb => "merge.disk-floor-mb",
            Self::MergeRetryAttempts => "merge.retry-attempts",
            Self::MergeRetryBackoffMs => "merge.retry-backoff-ms",
            Self::GitLongSeconds => "git.long-seconds",
            Self::PublishExclude => "publish.exclude",
            Self::RegistryBase => "registry.base",
            Self::TasksDispatchTrait => "tasks.dispatch-trait",
            Self::TasksAutoClose => "tasks.auto-close",
            Self::HarnessDynamic => "harness.*",
            Self::AgentDynamic => "agent.*",
            Self::HostDynamic => "host.*",
            Self::RepoDynamic => "repo.*",
        }
    }

    fn semantic(self) -> ConfigSemantic {
        match self {
            Self::WorktreeSeed
            | Self::WorktreeWarm
            | Self::WorktreeEnv
            | Self::WorktreeTripwireSentinel
            | Self::RunBuildCache
            | Self::PublishExclude => ConfigSemantic::Additive,
            Self::RunWait
            | Self::RunStory
            | Self::MergeWait
            | Self::MergeAuto
            | Self::MergeDeep
            | Self::GitLongSeconds
            | Self::RegistryBase
            | Self::TasksDispatchTrait
            | Self::TasksAutoClose
            // These maps are resolved by machine-scoped qualifier handling,
            // not by repository requirement precedence.
            | Self::HarnessDynamic
            | Self::AgentDynamic
            | Self::HostDynamic
            | Self::RepoDynamic
            | Self::TraitDynamic => ConfigSemantic::Default,
            Self::SchemaVersion
            | Self::WorktreeSetup
            | Self::WorktreeSetupSeconds
            | Self::WorktreeSetupCaptureBytes
            | Self::WorktreeConfinementEnabled
            | Self::WorktreeConfinementSandbox
            | Self::WorktreeConfinementAllow
            | Self::WorktreeTripwirePolicy
            | Self::WorktreeRetentionCheap
            | Self::WorktreeRetentionExpensive
            | Self::WorktreeRetentionExpensiveGraceDays
            | Self::RunWorktree
            | Self::RunMaxFrames
            | Self::RunFrameSeconds
            | Self::RunTotalSeconds
            | Self::RunMaxRetries
            | Self::RunAttachWaitSeconds
            | Self::RunIdleSeconds
            | Self::RunCommandSeconds
            | Self::RunCommandIdleSeconds
            | Self::RunMaxInFlight
            | Self::RunStrictLoops
            | Self::RunInlinePromptBytes
            | Self::MergeOverlap
            | Self::MergeBranch
            | Self::MergeGate
            | Self::MergeGateSeconds
            | Self::MergeGenerated
            | Self::MergeDiskFloorMb => ConfigSemantic::Requirement,
            Self::MergeRetryAttempts | Self::MergeRetryBackoffMs => ConfigSemantic::Requirement,
        }
    }
}

impl From<&str> for ConfigLeaf {
    fn from(path: &str) -> Self {
        *ConfigLeaf::ALL
            .iter()
            .find(|leaf| leaf.path() == path)
            .unwrap_or_else(|| panic!("unknown RuntimeConfig leaf {path}"))
    }
}

/// `[git]` process-timeout policy (P489): the long-running side of git
/// operations that plumbing's [`crate::git_process::PLUMBING_TIMEOUT_MS`]
/// would starve (`rebase`, `rebase --continue`, `worktree add`, `clone`,
/// `fetch`). `None` resolves to [`crate::git_process::LONG_TIMEOUT_MS`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GitTable {
    #[serde(default)]
    pub long_seconds: Option<u64>,
}

/// `[publish] exclude` (P489): directory names never published to the pack
/// tarball, at any depth. Entries combine across configuration layers in
/// stable first-occurrence order; duplicate entries are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PublishTable {
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
}

/// `[registry] base` (P492): the base URL npm-registry reads resolve
/// against, beneath `CTX_TRAITS_REGISTRY_BASE` (the top override — see
/// [`crate::distribution::resolve_registry_options`]) and above
/// [`crate::registry::DEFAULT_REGISTRY_BASE`]. Unauthenticated only — the
/// registry client sends no `Authorization` header and reads no `.npmrc`;
/// `publish` is unaffected, since it delegates to the caller's own npm.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegistryTable {
    #[serde(default)]
    pub base: Option<String>,
}

/// `[tasks] dispatch-trait` (0063.4): the trait id the TASKS board's `d`
/// dispatch seeds into the spawn modal's first line by default, so
/// dispatching a task never requires the owner to recall a trait id from
/// memory. `None` when unconfigured — the modal opens as today, its leading
/// comment naming this key as the missing setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TasksTable {
    #[serde(default)]
    pub dispatch_trait: Option<String>,
    /// `[tasks] auto-close` (0144): how a task's declared checks translate
    /// into a close action, beneath each document's own `auto_close`
    /// override. `None` when unconfigured.
    #[serde(default)]
    pub auto_close: Option<ctx_traits_core::task::AutoClosePolicy>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RunTable {
    #[serde(default)]
    pub worktree: Option<bool>,
    #[serde(flatten)]
    pub budget: RunProfileBudget,
    #[serde(default)]
    pub max_in_flight: Option<usize>,
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub strict_loops: Option<bool>,
    /// Named build-artifact cache declarations (P428), keyed by cache name.
    /// A `BTreeMap` so merge order and doctor reporting stay deterministic;
    /// each layer's `[run.build-cache.<name>]` table replaces the same name
    /// wholesale rather than merging its single `env` field (see
    /// `overlay_run_table`).
    #[serde(default)]
    pub build_cache: BTreeMap<String, BuildCacheConfig>,
    /// P489: inline-body ceiling, in bytes, for a resolved frame prompt
    /// (`ctx_traits_cli::app::frame_prompt`). `None` resolves to
    /// `frame_prompt::DEFAULT_MAX_INLINE_PROMPT_BYTES` (128 KiB).
    #[serde(default)]
    pub inline_prompt_bytes: Option<u64>,
    /// P550: the story level a driven `run`/`session start` opens at
    /// termination when neither `--story`/`--no-story` overrides it. `None`
    /// (absent everywhere) means the run-termination story pane is off by
    /// default — bare `--story` still opens it at `default` regardless of
    /// this key. `Some(StoryLevel::Assisted)` is the only value that spends
    /// a narrator model call; `doctor --config` prints that cost note
    /// whenever this resolves to `assisted`.
    #[serde(default)]
    pub story: Option<ctx_traits_core::procedure::story::StoryLevel>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum MergeOverlap {
    Land,
    Park,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MergeTable {
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub overlap: Option<MergeOverlap>,
    /// P460: request automatic post-drive landing by default for a driven
    /// `run`/`session start` that supplies neither `--merge` nor
    /// `--no-merge`. `None`/`false` preserves the pre-P460 default of never
    /// landing automatically.
    #[serde(default)]
    pub auto: Option<bool>,
    /// P460: the rung an automatic or bare `--merge` request resolves to
    /// when the CLI does not itself pin `standard`/`deep`. `None`/`false`
    /// resolves to `standard`.
    #[serde(default)]
    pub deep: Option<bool>,
    /// P488: the explicit landing-branch override. `None` means discover the
    /// default branch (`origin/HEAD` symref, then `init.defaultBranch`, then
    /// a literal `"main"` fallback) rather than assuming a name.
    #[serde(default)]
    pub branch: Option<String>,
    /// P477: the declared pre-landing gate — an ordered list of literal argv
    /// arrays, executed without a shell in the worktree on every landing
    /// path. `None` inherits a nearer-absent outer declaration; an explicit
    /// `gate = []` clears it. A present declaration replaces the whole
    /// ordered list wholesale (see `merge_project_config`) — commands are
    /// never concatenated across layers. Absent everywhere resolves to the
    /// product default: an empty gate (see `effective_merge_policy`).
    #[serde(default)]
    pub gate: Option<Vec<Vec<String>>>,
    /// P477: the per-command wall-clock ceiling, in seconds, applied to
    /// every command in `gate`. `None` resolves to
    /// [`DEFAULT_MERGE_GATE_SECONDS`].
    #[serde(default)]
    pub gate_seconds: Option<u64>,
    /// P463: declared generated-artifact reconciliation — every conflicted
    /// path matching one of these entries (exact path, or under a declared
    /// directory prefix) is never sent to the merger; ctx resolves it
    /// mechanically and re-runs `rebuild` after the rebase instead. `None`
    /// inherits a nearer-absent outer declaration; an explicit `generated =
    /// []` clears it. A present declaration replaces the whole ordered list
    /// wholesale (see `overlay_merge_table`), exactly like `gate`. Absent
    /// everywhere resolves to the product default: an empty list —
    /// classification off, behavior byte-identical to a build with no
    /// `[[merge.generated]]` declarations at all.
    #[serde(default)]
    pub generated: Option<Vec<GeneratedArtifact>>,
    /// P462: the minimum free disk space, in MiB, required on the worktree's
    /// volume before any declared `gate` command runs. `None` resolves to
    /// [`DEFAULT_MERGE_DISK_FLOOR_MB`]; `0` disables the preflight probe
    /// entirely. `deny_unknown_fields` means a binary predating this key
    /// refuses to parse a config that sets it (see risks in P462's draft).
    #[serde(default)]
    pub disk_floor_mb: Option<u64>,
    /// Total mechanical landing attempts permitted for a merge race. The
    /// first attempt counts toward this bound.
    #[serde(default)]
    pub retry_attempts: Option<u64>,
    /// Base delay in milliseconds for exponential merge-race backoff.
    #[serde(default)]
    pub retry_backoff_ms: Option<u64>,
}

/// One `[[merge.generated]]` declaration (P463): a set of repository-relative
/// paths whose conflicted content is never reconciled by the merger, plus the
/// literal argv command(s) that regenerate them after the rebase replays.
/// Matching is exact path or path under a declared directory prefix — no
/// glob engine, no pattern language, mirroring `[merge] gate`'s ordered-
/// literal-argv, anti-shell posture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GeneratedArtifact {
    pub paths: Vec<String>,
    pub rebuild: Vec<Vec<String>>,
}

/// Default per-command wall-clock ceiling (seconds) for a declared `[merge]
/// gate` command, used when `[merge] gate-seconds` is absent. Generous
/// enough to cover a full workspace build plus test suite for a repository
/// that declares one, without hardcoding what that repository's build/test
/// tooling actually is. Also the per-command ceiling `[[merge.generated]]`
/// `rebuild` commands run under (P463) — reusing `gate-seconds` rather than
/// minting a second timeout key, since a rebuild command is the same class
/// of repository-declared command as a gate command.
pub const DEFAULT_MERGE_GATE_SECONDS: u64 = 1_800;

/// Default minimum free disk space, in MiB, required on the worktree's
/// volume before a declared `[merge] gate` command runs (P462). A judgment
/// call: high enough to catch an ENOSPC-class failure before it wastes a
/// build, low enough to not false-park a small VM. Config always wins.
pub const DEFAULT_MERGE_DISK_FLOOR_MB: u64 = 2_048;
/// Default total number of attempts for a retryable merge race.
pub const DEFAULT_MERGE_RETRY_ATTEMPTS: u64 = 5;
/// Default base delay for exponential merge-race backoff.
pub const DEFAULT_MERGE_RETRY_BACKOFF_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigLayer {
    BuiltIn,
    UserGlobal,
    Repo,
    Environment,
    Flag,
}

/// Why a resolved value is effective. This is resolver metadata, rather than
/// presentation policy, so text and JSON consumers cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigReason {
    Default,
    RepoDefault,
    RepoRequirement,
    PersonalRepoOverride,
    EnvironmentOverride,
    Additive,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigContributor {
    pub layer: ConfigLayer,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigWinner {
    pub layer: ConfigLayer,
    pub source: Option<String>,
    pub reason: ConfigReason,
    /// Every document that supplied an additive value, in merge order.
    pub contributors: Vec<ConfigContributor>,
}

impl ConfigReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::RepoDefault => "repo default",
            Self::RepoRequirement => "repo requirement",
            Self::PersonalRepoOverride => "your per-repo override",
            Self::EnvironmentOverride => "environment override",
            Self::Additive => "additive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig<T> {
    pub value: T,
    pub winner: ConfigWinner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReport {
    pub runtime: RuntimeConfig,
    pub winners: BTreeMap<String, ConfigWinner>,
    pub tier_warnings: Vec<String>,
    /// Non-fatal attempts to override repository-owned requirements. Kept in
    /// the report so presentation layers, rather than this library, decide
    /// how and where to display them.
    pub requirement_conflicts: Vec<ConfigRequirementConflict>,
    pub foreign_config: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigRequirementConflict {
    pub field: String,
    pub rejected_source: String,
    pub repo_source: String,
}

/// Runtime defaults after all configuration layers have been classified.
/// Keeping this projection here prevents doctor and command dispatch from
/// inventing different defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveRunPolicy {
    pub worktree: bool,
    pub max_frames: Option<u64>,
    pub frame_seconds: Option<u64>,
    pub total_seconds: Option<u64>,
    pub max_retries: Option<u64>,
    pub attach_wait_seconds: Option<u64>,
    pub idle_seconds: Option<u64>,
    /// 0058: absolute wall-clock backstop for command/check steps, seconds.
    pub command_seconds: Option<u64>,
    /// 0058: silence window for command/check steps, seconds.
    pub command_idle_seconds: Option<u64>,
    pub max_in_flight: usize,
    pub wait: bool,
    pub strict_loops: bool,
    /// P489 `[run] inline-prompt-bytes` override for the resolved-prompt
    /// inline ceiling. `None` means the caller's own documented default
    /// applies (`frame_prompt::DEFAULT_MAX_INLINE_PROMPT_BYTES`).
    pub inline_prompt_bytes: Option<u64>,
    /// P550 `[run] story`: the story level a driven run opens at termination
    /// when the CLI supplies neither `--story` nor `--no-story`. `None`
    /// means the pane is off by default.
    pub story: Option<ctx_traits_core::procedure::story::StoryLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveMergePolicy {
    pub wait: bool,
    pub overlap: MergeOverlap,
    /// P460 `[merge] auto`: whether a driven run with no `--merge`/
    /// `--no-merge` flag requests automatic landing by default.
    pub auto: bool,
    /// P460 `[merge] deep`: whether a bare `--merge` (or an auto-enabled)
    /// request resolves to the deep rung instead of standard.
    pub deep: bool,
    /// P488 `[merge] branch`: the explicit landing-branch override. `None`
    /// means discover the default branch rather than assuming a name.
    pub branch: Option<String>,
    /// P477 `[merge] gate`: the declared ordered pre-landing gate. Empty
    /// (the product default) when no layer declares one.
    pub gate: Vec<Vec<String>>,
    /// P477 `[merge] gate-seconds`: the resolved per-command wall-clock
    /// ceiling, in seconds, applied to every command in `gate`.
    pub gate_seconds: u64,
    /// P463 `[[merge.generated]]`: declared generated-artifact reconciliation
    /// entries. Empty (the product default) when no layer declares any.
    pub generated: Vec<GeneratedArtifact>,
    /// P462 `[merge] disk-floor-mb`: the resolved minimum free disk space,
    /// in MiB, required before a declared gate command runs. `0` disables
    /// the preflight probe.
    pub disk_floor_mb: u64,
    pub retry_attempts: u64,
    pub retry_backoff_ms: u64,
}

impl RuntimeConfig {
    pub fn effective_run_policy(&self) -> EffectiveRunPolicy {
        let run = self.run.as_ref();
        let budget = run.map(|value| &value.budget);
        EffectiveRunPolicy {
            worktree: run.and_then(|value| value.worktree).unwrap_or(false),
            max_frames: budget.and_then(|value| value.max_frames),
            frame_seconds: budget.and_then(|value| value.frame_seconds),
            total_seconds: budget.and_then(|value| value.total_seconds),
            max_retries: budget.and_then(|value| value.max_retries),
            attach_wait_seconds: budget.and_then(|value| value.attach_wait_seconds),
            idle_seconds: budget.and_then(|value| value.idle_seconds),
            command_seconds: budget.and_then(|value| value.command_seconds),
            command_idle_seconds: budget.and_then(|value| value.command_idle_seconds),
            max_in_flight: run.and_then(|value| value.max_in_flight).unwrap_or(1),
            wait: run.and_then(|value| value.wait).unwrap_or(false),
            strict_loops: run.and_then(|value| value.strict_loops).unwrap_or(false),
            inline_prompt_bytes: run.and_then(|value| value.inline_prompt_bytes),
            story: run.and_then(|value| value.story),
        }
    }

    pub fn effective_merge_policy(&self) -> EffectiveMergePolicy {
        let merge = self.merge.as_ref();
        EffectiveMergePolicy {
            wait: merge.and_then(|value| value.wait).unwrap_or(true),
            overlap: merge
                .and_then(|value| value.overlap)
                .unwrap_or(MergeOverlap::Land),
            auto: merge.and_then(|value| value.auto).unwrap_or(false),
            deep: merge.and_then(|value| value.deep).unwrap_or(false),
            branch: merge.and_then(|value| value.branch.clone()),
            gate: merge
                .and_then(|value| value.gate.clone())
                .unwrap_or_default(),
            gate_seconds: merge
                .and_then(|value| value.gate_seconds)
                .unwrap_or(DEFAULT_MERGE_GATE_SECONDS),
            generated: merge
                .and_then(|value| value.generated.clone())
                .unwrap_or_default(),
            disk_floor_mb: merge
                .and_then(|value| value.disk_floor_mb)
                .unwrap_or(DEFAULT_MERGE_DISK_FLOOR_MB),
            retry_attempts: merge
                .and_then(|value| value.retry_attempts)
                .unwrap_or(DEFAULT_MERGE_RETRY_ATTEMPTS),
            retry_backoff_ms: merge
                .and_then(|value| value.retry_backoff_ms)
                .unwrap_or(DEFAULT_MERGE_RETRY_BACKOFF_MS),
        }
    }

    /// P489 `[git] long-seconds` override, in milliseconds, for the
    /// long-running side of git operations. `None` when unconfigured, so
    /// callers apply their own documented [`crate::git_process::LONG_TIMEOUT_MS`]
    /// default.
    pub fn effective_git_long_timeout_ms(&self) -> Option<u64> {
        self.git
            .as_ref()
            .and_then(|git| git.long_seconds)
            .map(|seconds| seconds * 1000)
    }

    /// 0063.4 `[tasks] dispatch-trait`: the trait id the board's `d`
    /// dispatch seeds into the spawn modal by default. `None` when
    /// unconfigured.
    pub fn effective_dispatch_trait(&self) -> Option<String> {
        self.tasks
            .as_ref()
            .and_then(|tasks| tasks.dispatch_trait.clone())
    }

    /// 0144 `[tasks] auto-close`: how a task's declared checks translate
    /// into a close action, beneath each document's own `auto_close`
    /// override. `None` when unconfigured.
    pub fn effective_auto_close(&self) -> Option<ctx_traits_core::task::AutoClosePolicy> {
        self.tasks.as_ref().and_then(|tasks| tasks.auto_close)
    }
}

/// Resolve the effective long-running git timeout (`[git] long-seconds`,
/// P489) for a caller that has no other config already in hand — mirrors the
/// ad hoc `resolve_runtime_config(Utf8Path::new("."))` pattern used at other
/// command entry points, shared here once so every caller (run start,
/// drive/resume, merge dispatch) applies the identical override-then-default
/// resolution rather than restating it. Falls back to
/// [`crate::git_process::LONG_TIMEOUT_MS`] both when unconfigured and when
/// config resolution itself fails, since a config read error must never
/// block a git operation that would otherwise proceed under the documented
/// default.
pub fn resolve_git_long_timeout_ms(start_dir: &Utf8Path) -> u64 {
    resolve_runtime_config(start_dir)
        .ok()
        .and_then(|config| config.effective_git_long_timeout_ms())
        .unwrap_or(crate::git_process::LONG_TIMEOUT_MS)
}

/// Resolve the effective pack exclude set (`[publish] exclude`, P489) for a
/// caller that has no other config already in hand — same ad hoc
/// `resolve_runtime_config` pattern as [`resolve_git_long_timeout_ms`].
/// Falls back to [`crate::publish::PACK_DEFAULT_EXCLUDES`] both when
/// unconfigured and when config resolution itself fails, since a config read
/// error must never block a publish that would otherwise proceed under the
/// documented default.
pub fn resolve_pack_excludes(start_dir: &Utf8Path) -> Vec<String> {
    resolve_runtime_config(start_dir)
        .ok()
        .and_then(|config| config.publish.and_then(|publish| publish.exclude))
        .unwrap_or_else(|| {
            crate::publish::PACK_DEFAULT_EXCLUDES
                .iter()
                .map(|name| name.to_string())
                .collect()
        })
}

/// Resolve the declared `[registry] base` (P492) for a caller that has no
/// other config already in hand — same ad hoc `resolve_runtime_config`
/// pattern as [`resolve_git_long_timeout_ms`]/[`resolve_pack_excludes`].
/// `None` when unconfigured (including when config resolution itself
/// fails, since a config read error must never block an operation that
/// would otherwise proceed under the documented default); the caller falls
/// back to [`crate::registry::DEFAULT_REGISTRY_BASE`].
pub fn resolve_registry_base(start_dir: &Utf8Path) -> Option<String> {
    resolve_runtime_config(start_dir)
        .ok()
        .and_then(|config| config.registry)
        .and_then(|registry| registry.base)
}

/// A `.ctx/config.toml [host.<name>]` override for one built-in host, or a
/// fully-specified addition of a new host not in the built-in table. Every
/// field is optional so a config entry may override only the fields it
/// cares about (e.g. just `project-path`) while leaving the rest of a
/// built-in host's defaults in place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HostOverride {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub global_path: Option<String>,
}

impl HostOverride {
    /// Merge `next` on top of `self`, field by field (`next`'s `Some` wins).
    pub(crate) fn merge(&mut self, next: HostOverride) {
        if next.profile.is_some() {
            self.profile = next.profile;
        }
        if next.format.is_some() {
            self.format = next.format;
        }
        if next.project_path.is_some() {
            self.project_path = next.project_path;
        }
        if next.global_path.is_some() {
            self.global_path = next.global_path;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AgentDefaults {
    #[serde(default)]
    pub model_tier: BTreeMap<ctx_traits_core::r#trait::AgentModelTier, ProfileAssignment>,
    /// `[agent.role.<role>]` (single table) or `[[agent.role.<role>]]`
    /// (ordered list of seats, P456). One namespace (P476) for every seat:
    /// `default` (the DRIVER seat, renamed from the old `master`, and the
    /// fallback for any role with no table of its own), `narrator`, `guide`,
    /// `merger`, `merger-deep`, and every trait-declared role — see
    /// [`STANDING_SEATS`]/[`is_standing_seat`] for the standing seats that never
    /// inherit `default`'s value themselves and are restricted to exactly one
    /// seat. A nearer config scope's whole value — table or list — replaces
    /// the same role key entirely in [`merge_agent_defaults`]; seats are
    /// never merged individually across scopes.
    #[serde(default)]
    pub role: BTreeMap<String, RoleAssignmentValue>,
    /// P451: `[agent.variant.<variant>.role.<role>]` — a variant-qualified
    /// override keyed on the run's derived variant name (see
    /// [`RunScope`]/[`flatten_agent_defaults`]). A single-table entry is a
    /// *partial* override of the base `role` table (field-wise inherit); a
    /// list-form entry wins WHOLE. Never itself nestable (`VariantOverride`
    /// carries only `role`), so `[agent.variant.<v>.variant.<w>]` cannot be
    /// authored.
    #[serde(default)]
    pub variant: BTreeMap<String, VariantOverride>,
}

/// One `(role, variant)` qualifier level (P451): `[agent.variant.<v>.role.<r>]`
/// or its repo-scoped counterpart `[repo."<key>".agent.variant.<v>.role.<r>]`.
/// Carries only `role` so a `variant`-inside-`variant` table is structurally
/// unrepresentable — no anti-nesting validation is needed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct VariantOverride {
    #[serde(default)]
    pub role: BTreeMap<String, RoleAssignmentValue>,
}

/// One `[repo."<key>"]` block in the GLOBAL `~/.config/ctx/config.toml` only
/// (P451): a repo-scoped `AgentDefaults`, reused wholesale so `(repo,role)`
/// and `(repo,role,variant)` both come from the one type. A non-empty
/// `[repo.*]` declared in any config layer other than the carried global
/// file is a hard error (checked in [`resolve_config_report`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoOverride {
    #[serde(default)]
    pub agent: AgentDefaults,
    #[serde(default)]
    pub harness: BTreeMap<String, HarnessDefinition>,
    #[serde(default)]
    pub host: BTreeMap<String, HostOverride>,
    /// Only additive worktree values are accepted in a personal repo block;
    /// setup and confinement remain repository-owned requirements.
    #[serde(default)]
    pub worktree: RepoWorktreeOverride,
    #[serde(default)]
    pub run: RepoRunOverride,
    #[serde(default)]
    pub merge: RepoMergeOverride,
    #[serde(default)]
    pub git: RepoGitOverride,
    #[serde(default)]
    pub registry: RepoRegistryOverride,
    #[serde(default)]
    pub publish: RepoPublishOverride,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoWorktreeOverride {
    #[serde(default)]
    pub seed: Vec<String>,
    #[serde(default)]
    pub warm: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub tripwire: RepoTripwireOverride,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoTripwireOverride {
    #[serde(default)]
    pub sentinel: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoRunOverride {
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub story: Option<ctx_traits_core::procedure::story::StoryLevel>,
    #[serde(default)]
    pub build_cache: BTreeMap<String, BuildCacheConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoMergeOverride {
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub auto: Option<bool>,
    #[serde(default)]
    pub deep: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoGitOverride {
    #[serde(default)]
    pub long_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoRegistryOverride {
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RepoPublishOverride {
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// The DRIVER seat's role name: `[agent.role.default]`. Renamed from the
/// pre-P476 `master`; also the whole-seat fallback for any role (other than
/// the other three standing seats) with no table of its own.
pub const DEFAULT_SEAT: &str = "default";

/// One standing seat's config policy, replacing the pre-P476 dedicated
/// `master`/`narrator`/`merger`/`merger-deep` struct fields and their
/// bespoke accessors/validation with one table every consumer (resolution,
/// validation, the `--assign` parser) reads.
struct StandingSeat {
    name: &'static str,
    /// `merger`/`merger-deep`: absence means the feature is unavailable, and
    /// a present table must declare its own harness/model/reasoning-effort —
    /// it is never left to resolve implicitly the way a trait role's harness
    /// can. `default`/`narrator` are usable from an `--assign` override alone
    /// with no table at all.
    requires_full_declaration: bool,
    /// P475 D3/D4: whether this seat is dispatched as a one-shot harness call
    /// (`run_one_shot`) outside the drive frame loop — `narrator`, `merger`,
    /// `merger-deep`. `default` is NOT one-shot: it IS the drive frame loop.
    /// A one-shot seat has no idle timeout and no retry loop, so its budget
    /// resolves from its own `budget.frame-seconds` alone (never the
    /// `[run]`/CLI-flag chain frame seats use), and a declared
    /// `budget.idle-seconds`/`budget.max-retries` on it is a decode error
    /// (`validate_role_budget`) rather than an accepted-and-ignored no-op.
    one_shot: bool,
}

const STANDING_SEATS: &[StandingSeat] = &[
    StandingSeat {
        name: DEFAULT_SEAT,
        requires_full_declaration: false,
        one_shot: false,
    },
    StandingSeat {
        name: "narrator",
        requires_full_declaration: false,
        one_shot: true,
    },
    StandingSeat {
        name: "guide",
        requires_full_declaration: false,
        one_shot: true,
    },
    StandingSeat {
        name: "merger",
        requires_full_declaration: true,
        one_shot: true,
    },
    StandingSeat {
        name: "merger-deep",
        requires_full_declaration: true,
        one_shot: true,
    },
];

/// Whether `role` is one of the four standing seats (`default`, `narrator`,
/// `merger`, `merger-deep`): self-described, restricted to exactly one seat,
/// and — unlike a trait role — never resolved by falling back to
/// `[agent.role.default]` when it has no table of its own.
fn is_standing_seat(role: &str) -> bool {
    STANDING_SEATS.iter().any(|seat| seat.name == role)
}

/// The standing seat names, in declaration order — the single source for any
/// caller (currently `doctor --config`, P475) needing the union of every
/// configured role and the standing seats. Backed by [`STANDING_SEATS`]
/// itself (not a second hard-coded list), so a seat added or removed there
/// is reflected here with no second edit.
pub fn standing_seat_names() -> impl Iterator<Item = &'static str> {
    STANDING_SEATS.iter().map(|seat| seat.name)
}

fn standing_seat_requires_full_declaration(role: &str) -> bool {
    STANDING_SEATS
        .iter()
        .any(|seat| seat.name == role && seat.requires_full_declaration)
}

/// Whether `role` is dispatched as a one-shot harness call outside the drive
/// frame loop (`narrator`, `merger`, `merger-deep`) — see
/// [`StandingSeat::one_shot`]. `false` for `default` and for every
/// trait-declared role (both are frame-dispatched).
pub fn standing_seat_is_one_shot(role: &str) -> bool {
    STANDING_SEATS
        .iter()
        .any(|seat| seat.name == role && seat.one_shot)
}

/// One role's configured assignment shape: the legacy single `[agent.role.<role>]`
/// table, or an ordered `[[agent.role.<role>]]` list of seats (P456). Untagged
/// so a legacy single-table role decodes and serializes byte-identically to
/// before this type existed; only a TOML array-of-tables input takes the list
/// branch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum RoleAssignmentValue {
    Single(ProfileAssignment),
    List(Vec<ProfileAssignment>),
}

impl RoleAssignmentValue {
    /// Ordered seat entries: one element for a legacy single table (no seat
    /// evidence attaches to it), or the authored list order for
    /// `[[agent.role.<role>]]`.
    pub fn entries(&self) -> &[ProfileAssignment] {
        match self {
            RoleAssignmentValue::Single(assignment) => std::slice::from_ref(assignment),
            RoleAssignmentValue::List(entries) => entries,
        }
    }

    pub fn is_list(&self) -> bool {
        matches!(self, RoleAssignmentValue::List(_))
    }
}

/// Optional evidence identifying which seat of a list-backed role an
/// assignment was resolved for. Absent entirely for legacy single-table
/// roles, so their serialized assignments stay byte-for-byte unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatInfo {
    /// 1-based seat position within the configured list.
    pub seat_index: u32,
    pub list_length: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProfileAssignment {
    /// JSON overrides emitted from an already resolved assignment replace the
    /// inherited defaults rather than merging their accumulated arguments.
    #[serde(skip)]
    #[schemars(skip)]
    replace_inherited: bool,
    #[serde(skip)]
    #[schemars(skip)]
    model_selector: Option<String>,
    #[serde(skip)]
    #[schemars(skip)]
    model_resolution_reason: Option<ctx_traits_core::agent_model::ResolutionReason>,
    #[serde(default)]
    pub mode: RunAssignmentMode,
    /// Kept private so authored presence survives deserialization without
    /// changing the public resolved `mode` shape.
    #[serde(skip)]
    #[schemars(skip)]
    mode_authored: bool,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub transport: Option<RunTransport>,
    #[serde(default)]
    pub session_mode: Option<RunSessionMode>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<ctx_traits_core::r#trait::AgentModelTier>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// P475: this seat's own time/retry limits. Empty (never serialized) for
    /// every assignment that declares none, so existing evidence/ledger
    /// bytes are unaffected until an operator actually declares one.
    #[serde(default, skip_serializing_if = "RoleBudget::is_empty")]
    pub budget: RoleBudget,
    /// 0079 `transport = "api"` endpoint fields, boxed and flattened: boxed
    /// so this rarely-populated cluster does not grow every
    /// [`ProfileAssignment`] (and, through it, [`RoleAssignmentValue`]'s
    /// `Single` variant vs. `List`'s `Vec` pointer — clippy's
    /// `large_enum_variant`) by six fields' worth of bytes; flattened so the
    /// wire shape stays the flat `base-url`/`wire`/`api-key-env`/... seat
    /// fields the draft specifies, not a nested `[agent.role.<role>.api]`
    /// sub-table. Every field skip-serializes when absent, so a config that
    /// never declares any of them keeps byte-identical serialized
    /// assignments.
    #[serde(flatten)]
    pub api: Box<ApiEndpoint>,
    /// 0025: a `Single`-form role table declaring `count = N` expands to `N`
    /// addressable seats (`<role>-1` … `<role>-N`) in [`expand_role_seats`],
    /// run after scope merging so a trait-scoped override (0034) can still
    /// narrow it. `None` (un-authored) never expands, keeping every existing
    /// config byte-identical through this enum's untagged round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// See [`ProfileAssignment::api`].
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub struct ApiEndpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<ProviderWire>,
    /// The environment variable's NAME, never its value — resolved at
    /// dispatch/doctor time by [`crate::env_reference::resolve_env_var_reference`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
}

impl<'de> Deserialize<'de> for ProfileAssignment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct WireAssignment {
            #[serde(default)]
            mode: Option<RunAssignmentMode>,
            #[serde(default)]
            harness: Option<String>,
            #[serde(default)]
            transport: Option<RunTransport>,
            #[serde(default)]
            session_mode: Option<RunSessionMode>,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            model_tier: Option<ctx_traits_core::r#trait::AgentModelTier>,
            #[serde(default)]
            reasoning_effort: Option<String>,
            #[serde(default)]
            system_prompt: Option<String>,
            #[serde(default)]
            extra_args: Vec<String>,
            #[serde(default)]
            budget: RoleBudget,
            #[serde(default)]
            base_url: Option<String>,
            #[serde(default)]
            wire: Option<ProviderWire>,
            #[serde(default)]
            api_key_env: Option<String>,
            #[serde(default)]
            connect_timeout_ms: Option<u64>,
            #[serde(default)]
            read_timeout_ms: Option<u64>,
            #[serde(default)]
            retries: Option<u32>,
            #[serde(default)]
            count: Option<u32>,
        }

        let raw = WireAssignment::deserialize(deserializer)?;
        Ok(Self {
            replace_inherited: false,
            model_selector: None,
            model_resolution_reason: None,
            mode: raw.mode.unwrap_or_default(),
            mode_authored: raw.mode.is_some(),
            harness: raw.harness,
            transport: raw.transport,
            session_mode: raw.session_mode,
            model: raw.model,
            model_tier: raw.model_tier,
            reasoning_effort: raw.reasoning_effort,
            system_prompt: raw.system_prompt,
            extra_args: raw.extra_args,
            budget: raw.budget,
            api: Box::new(ApiEndpoint {
                base_url: raw.base_url,
                wire: raw.wire,
                api_key_env: raw.api_key_env,
                connect_timeout_ms: raw.connect_timeout_ms,
                read_timeout_ms: raw.read_timeout_ms,
                retries: raw.retries,
            }),
            count: raw.count,
        })
    }
}

impl ProfileAssignment {
    pub fn model_resolution_evidence(&self) -> Option<String> {
        Some(format!(
            "selector={} model={} reason={}",
            self.model_selector.as_deref()?,
            self.model.as_deref()?,
            self.model_resolution_reason?.as_str()
        ))
    }
}

/// A seat's own time/retry limits (P475), declared as `[agent.role.<name>.budget]`
/// next to that seat's model/harness. Distinct from [`RunProfileBudget`],
/// which also carries the drive-shaped `max-frames`/`total-seconds`/
/// `attach-wait-seconds` knobs that have no meaning per seat —
/// `deny_unknown_fields` turns a role budget table naming one of those into
/// a decode error instead of a silently-ignored key.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RoleBudget {
    #[serde(default)]
    pub frame_seconds: Option<u64>,
    #[serde(default)]
    pub idle_seconds: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u64>,
}

impl RoleBudget {
    pub fn is_empty(&self) -> bool {
        self.frame_seconds.is_none() && self.idle_seconds.is_none() && self.max_retries.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RunProfileBudget {
    #[serde(default)]
    pub max_frames: Option<u64>,
    #[serde(default)]
    pub frame_seconds: Option<u64>,
    #[serde(default)]
    pub total_seconds: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u64>,
    #[serde(default)]
    pub attach_wait_seconds: Option<u64>,
    #[serde(default)]
    pub idle_seconds: Option<u64>,
    /// 0058 `[run] command-seconds`: absolute wall-clock backstop for a
    /// command/check step (the repo gate), in seconds. A backstop, not an
    /// estimate — `command-idle-seconds` is what decides hung-ness. `None`
    /// resolves to `crate::command::DEFAULT_COMMAND_WALL_MS`.
    #[serde(default)]
    pub command_seconds: Option<u64>,
    /// 0058 `[run] command-idle-seconds`: how long a command/check step may
    /// produce NO output before the runtime treats it as hung, in seconds.
    /// Liveness generalises across ecosystems where a fixed duration cannot:
    /// a command still printing is working however long it takes. `None`
    /// resolves to `crate::command::DEFAULT_COMMAND_IDLE_MS`.
    #[serde(default)]
    pub command_idle_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRunAssignments {
    pub assignments: Option<Vec<ctx_traits_core::procedure::session::AgentAssignment>>,
    pub harness_probes: Vec<ctx_traits_core::procedure::session::HarnessProbeEvidence>,
    pub warnings: Vec<String>,
    pub capability_reports: Vec<ctx_traits_core::response::CapabilityReport>,
    pub worktree: WorktreeConfig,
    pub port_defaults: BTreeMap<String, ConfiguredPortDefault>,
}

/// A selected trait-config port default with the exact file and TOML field
/// that supplied it. This provenance is persisted with the accepted port value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredPortDefault {
    pub value: String,
    /// The configuration layer that supplied the winning leaf. Kept separate
    /// from the rendered receipt so callers can retain structured provenance.
    pub layer: ConfigLayer,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeAssignments {
    pub registry: HarnessRegistry,
    pub assignments: BTreeMap<String, ProfileAssignment>,
    /// `--assign role.N=...` per-seat overrides (P456), keyed by role and
    /// 1-based seat. Applied after `assignments`' whole-role overlay, on top
    /// of the matching seat only.
    pub seat_assignments: BTreeMap<(String, u32), ProfileAssignment>,
    pub agent_defaults: AgentDefaults,
    /// P451: for every role a variant/repo qualifier actually contributed to
    /// (after [`flatten_agent_defaults`]), the winning qualifier label
    /// (`variant:<v>`, `repo:<key>`, or `repo:<key>+variant:<v>`) — threaded
    /// into [`assignment_evidence`]'s `config-qualifier=` field. Empty for an
    /// unqualified config, so evidence bytes stay identical.
    pub qualifier_by_role: BTreeMap<String, String>,
    pub budget: RunProfileBudget,
    pub worktree: WorktreeConfig,
    pub port_defaults: BTreeMap<String, ConfiguredPortDefault>,
    model_catalogs: BTreeMap<String, ModelCatalogState>,
    model_catalog_capability_reports: Vec<ctx_traits_core::response::CapabilityReport>,
    /// P427 zero-config fallback: one cached PATH-detection pass over the
    /// compiled-in built-in harnesses, populated lazily on first use so a
    /// resolver that never needs automatic selection never probes anything.
    /// Shared by role/master fallback selection, [`Self::builtin_detection`]
    /// (doctor reporting), and [`no_builtin_harness_message`] so every
    /// consumer sees exactly one probe per candidate per resolver instance.
    builtin_detection: Option<Vec<BuiltinHarnessDetection>>,
    /// Roles (and `"master"`) automatically assigned a built-in harness,
    /// grouped by the selected harness id, in the order fallback selection
    /// happened. Rendered as one grouped, deterministically sorted
    /// announcement per harness by [`Self::builtin_fallback_warnings`].
    builtin_fallback_selections: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelCatalogState {
    Available(ctx_traits_core::agent_model::Catalog),
    Unavailable(String),
}

impl ResolvedRuntimeAssignments {
    /// Resolve any role — a declared trait role or one of the four standing
    /// seats — by exact name: its own `[agent.role.<role>]` table if present
    /// (self-described, inheriting nothing), else, for every role except the
    /// four standing seats themselves, the whole `[agent.role.default]` seat;
    /// an `--assign role=...` override wins last. See [`is_standing_seat`]
    /// for why `narrator`/`merger`/`merger-deep` never fall back to
    /// `default` this way.
    pub fn assignment_for_role(&self, role: &str) -> Option<ProfileAssignment> {
        resolved_assignment_for_role(&self.agent_defaults, role, self.assignments.get(role))
    }

    pub fn narrator_assignment(&self) -> Option<ProfileAssignment> {
        self.assignment_for_role("narrator")
    }

    /// The optional live-TUI guide. Unlike narrator, an `--assign guide=...`
    /// override cannot create it: the read-only ask surface is available only
    /// when its exact-name config table explicitly opts in.
    pub fn guide_assignment(&self) -> Option<ProfileAssignment> {
        single_role_table(&self.agent_defaults, "guide")?;
        self.assignment_for_role("guide")
    }

    /// Merge is available if and only if `[agent.role.merger]` itself is
    /// present: unlike a trait role assignment, an `--assign merger=...`
    /// override on its own must never make the merge command available,
    /// since the merger's required model/effort/harness only get validated
    /// when the table exists (see `validate_agent_defaults`). An override
    /// still layers onto a present table as usual.
    pub fn merger_assignment(&self) -> Option<ProfileAssignment> {
        single_role_table(&self.agent_defaults, "merger")?;
        self.assignment_for_role("merger")
    }

    /// The `--deep` judgment-capable merger, present if and only if
    /// `[agent.role.merger-deep]` itself is present — same absence rule as
    /// [`merger_assignment`]. Fallback to the ordinary merger when
    /// `[agent.role.merger-deep]` is absent is the caller's responsibility
    /// (see `ctx-traits-cli`'s `resolve_merger`), not this accessor's, since
    /// a caller not running `--deep` must never resolve this table at all.
    pub fn merger_deep_assignment(&self) -> Option<ProfileAssignment> {
        single_role_table(&self.agent_defaults, "merger-deep")?;
        self.assignment_for_role("merger-deep")
    }

    /// `true` when `[agent.role.merger-deep]` itself is configured — the
    /// caller's only use is choosing the display role label
    /// ([`merger_deep_or_fallback_assignment`]'s base) between `merger-deep`
    /// and the `merger` fallback.
    pub fn merger_deep_table_present(&self) -> bool {
        single_role_table(&self.agent_defaults, "merger-deep").is_some()
    }

    /// The `--deep` merger's base table — `[agent.role.merger-deep]` if
    /// present, falling back to `[agent.role.merger]` when it is absent —
    /// with the deep invocation's own `--assign merger-deep=...` override
    /// layered onto *that selected base*, so an override is never silently
    /// dropped just because `[agent.role.merger-deep]` itself is
    /// unconfigured. Distinct from [`merger_deep_assignment`], which only
    /// ever resolves the `[agent.role.merger-deep]` table and is unused by a
    /// `--deep` merge (see `ctx-traits-cli`'s `resolve_merger`).
    pub fn merger_deep_or_fallback_assignment(&self) -> Option<ProfileAssignment> {
        let base = single_role_table(&self.agent_defaults, "merger-deep")
            .or_else(|| single_role_table(&self.agent_defaults, "merger"));
        base?;
        let mut assignment = base.cloned().unwrap_or_default();
        if let Some(explicit) = self.assignments.get("merger-deep") {
            merge_assignment(&mut assignment, explicit);
        }
        finalize_assignment(assignment)
    }

    pub fn resolved_assignment_for_role(
        &mut self,
        role: &str,
    ) -> crate::Result<Option<ProfileAssignment>> {
        let assignment = self.assignment_for_role(role);
        self.resolve_assignment_model(assignment)
    }

    /// [`Self::assignment_for_role`]`("default")` plus the P427 built-in
    /// fallback for the implicit `default` (driver) seat: when
    /// `[agent.role.default]`/`--assign default=...` leave the harness
    /// unset (including when `default` is entirely unconfigured), the first
    /// available built-in fills it. Still `Ok(None)` — never an error — when
    /// no built-in candidate is available, exactly as an unconfigured
    /// `default` always was, so agentless/attach-only runs that never reach
    /// this accessor, and drive's session-provenance fallthrough for
    /// `default` seats that do, stay unchanged.
    pub fn resolved_default_assignment(&mut self) -> crate::Result<Option<ProfileAssignment>> {
        let raw = raw_assignment_for_role(
            &self.agent_defaults,
            DEFAULT_SEAT,
            single_role_table(&self.agent_defaults, DEFAULT_SEAT),
            self.assignments.get(DEFAULT_SEAT),
        );
        let assignment = self.apply_builtin_fallback(DEFAULT_SEAT, raw);
        self.resolve_assignment_model(assignment)
    }

    pub fn resolved_narrator_assignment(&mut self) -> crate::Result<Option<ProfileAssignment>> {
        let assignment = self.narrator_assignment();
        self.resolve_assignment_model(assignment)
    }

    pub fn resolved_guide_assignment(&mut self) -> crate::Result<Option<ProfileAssignment>> {
        self.resolve_assignment_model(self.guide_assignment())
    }

    pub fn resolved_merger_assignment(&mut self) -> crate::Result<Option<ProfileAssignment>> {
        let assignment = self.merger_assignment();
        self.resolve_assignment_model(assignment)
    }

    pub fn resolved_merger_deep_assignment(&mut self) -> crate::Result<Option<ProfileAssignment>> {
        let assignment = self.merger_deep_assignment();
        self.resolve_assignment_model(assignment)
    }

    pub fn resolved_merger_deep_or_fallback_assignment(
        &mut self,
    ) -> crate::Result<Option<ProfileAssignment>> {
        let assignment = self.merger_deep_or_fallback_assignment();
        self.resolve_assignment_model(assignment)
    }

    /// The resolved [`RoleBudget`] (P475) for `role` at `structural_seat` —
    /// the same seat [`Self::configured_seats_for_role`]/drive's own seat
    /// selection (`index = structural_seat.unwrap_or(0) % seats.len()`)
    /// would pick, so a list-backed role's per-seat budget declarations are
    /// never averaged or mismatched against the harness actually dispatched
    /// for that seat. Empty (every field `None`) for a role with no
    /// declared budget anywhere in its resolution chain — callers apply
    /// built-in defaults on top.
    pub fn budget_for_seat(&self, role: &str, structural_seat: Option<u32>) -> RoleBudget {
        let seats = self.seats_for_role_raw(role);
        if seats.is_empty() {
            return RoleBudget::default();
        }
        let index = match seats.len() {
            1 => 0,
            len => (structural_seat.unwrap_or(0) as usize) % len,
        };
        seats[index].0.budget.clone()
    }

    pub fn model_catalog_capability_reports(
        &self,
    ) -> &[ctx_traits_core::response::CapabilityReport] {
        &self.model_catalog_capability_reports
    }

    /// Layer `--assign role=...`/`--assign role.N=...` overrides onto
    /// `role`'s configured entries WITHOUT resolving a model-tier/catalog
    /// probe — the pure configuration-layering half of
    /// [`resolved_seats_for_role`]. One entry with no [`SeatInfo`] for a
    /// legacy single-table (or wholly unconfigured, catch-all-only) role, or
    /// one entry per authored `[[agent.role.<role>]]` list item, in order,
    /// each carrying its 1-based seat index and the list length. A
    /// `--assign role=...` whole-role override merges onto every seat before
    /// a `--assign role.N=...` per-seat override merges onto the matching
    /// seat last. Empty when the role has no usable assignment at all (the
    /// caller reports "no runtime assignment" exactly as for a single-table
    /// role).
    ///
    /// Split out (P456) so a caller that only needs to enumerate/display
    /// configured seats (run-info) or apply a higher-precedence override
    /// before resolution (built-in `--model`) never triggers a model-catalog
    /// probe it does not need — [`resolved_seats_for_role`] is the only
    /// caller that additionally resolves models.
    pub fn configured_seats_for_role(
        &self,
        role: &str,
    ) -> crate::Result<Vec<(ProfileAssignment, Option<SeatInfo>)>> {
        let raw = self.seats_for_role_raw(role);
        let is_list = matches!(
            self.agent_defaults.role.get(role),
            Some(RoleAssignmentValue::List(_))
        );
        if !is_list {
            let (assignment, _) = raw
                .into_iter()
                .next()
                .expect("seats_for_role_raw always yields at least one entry for a non-list role");
            return Ok(match finalize_assignment(assignment) {
                Some(resolved) => vec![(resolved, None)],
                None => Vec::new(),
            });
        }
        let mut out = Vec::with_capacity(raw.len());
        for (assignment, seat_info) in raw {
            let seat_index = seat_info
                .expect("list-backed role entries always carry seat info")
                .seat_index;
            match finalize_assignment(assignment) {
                Some(resolved) => out.push((resolved, seat_info)),
                None => {
                    return invalid_config(
                        format!("assign.{role}.{seat_index}"),
                        format!("seat {seat_index} of role {role:?} has no usable assignment"),
                    );
                }
            }
        }
        Ok(out)
    }

    /// The pure configuration-layering half shared by [`configured_seats_for_role`]
    /// and [`resolved_seats_for_role`]: one raw (possibly harness-less) entry
    /// per seat, never collapsed to "no assignment" — collapsing, and the P427
    /// built-in fallback that runs before collapsing, are each caller's own
    /// decision.
    fn seats_for_role_raw(&self, role: &str) -> Vec<(ProfileAssignment, Option<SeatInfo>)> {
        let role_value = self.agent_defaults.role.get(role).cloned();
        let whole_override = self.assignments.get(role).cloned();
        let Some(RoleAssignmentValue::List(entries)) = role_value else {
            let role_default = match &role_value {
                Some(RoleAssignmentValue::Single(assignment)) => Some(assignment),
                _ => None,
            };
            let raw = raw_assignment_for_role(
                &self.agent_defaults,
                role,
                role_default,
                whole_override.as_ref(),
            );
            return vec![(raw, None)];
        };
        let list_length = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(entries.len());
        for (offset, entry) in entries.iter().enumerate() {
            let seat_index = u32::try_from(offset + 1).unwrap_or(u32::MAX);
            let seat_override = self
                .seat_assignments
                .get(&(role.to_string(), seat_index))
                .cloned();
            let combined = match (whole_override.clone(), seat_override) {
                (Some(mut base), Some(seat)) => {
                    merge_assignment(&mut base, &seat);
                    Some(base)
                }
                (Some(base), None) => Some(base),
                (None, Some(seat)) => Some(seat),
                (None, None) => None,
            };
            let raw =
                raw_assignment_for_role(&self.agent_defaults, role, Some(entry), combined.as_ref());
            out.push((
                raw,
                Some(SeatInfo {
                    seat_index,
                    list_length,
                }),
            ));
        }
        out
    }

    /// [`configured_seats_for_role`] plus model-tier/catalog resolution for
    /// every seat — the caller for actual dispatch, which needs a concrete
    /// resolved model rather than just the configured selector. Unlike
    /// [`configured_seats_for_role`], a seat that configuration alone leaves
    /// without a harness is filled from the P427 built-in fallback
    /// ([`Self::apply_builtin_fallback`]) before being dropped; a seat that
    /// still has no harness after fallback (no candidate detected) is
    /// omitted here exactly as an unconfigured seat always was — the caller
    /// (`prepare_run_assignments`) reports the no-candidate remediation when
    /// the whole role comes back empty.
    pub fn resolved_seats_for_role(
        &mut self,
        role: &str,
    ) -> crate::Result<Vec<(ProfileAssignment, Option<SeatInfo>)>> {
        let raw = self.seats_for_role_raw(role);
        let mut out = Vec::with_capacity(raw.len());
        for (assignment, seat_info) in raw {
            let Some(assignment) = self.apply_builtin_fallback(role, assignment) else {
                continue;
            };
            let resolved = self
                .resolve_assignment_model(Some(assignment))?
                .expect("resolve_assignment_model must return Some for a Some input assignment");
            out.push((resolved, seat_info));
        }
        Ok(out)
    }

    /// Whether `role` (or `"master"`) was itself filled by the P427 built-in
    /// fallback rather than a real CLI/config assignment during the most
    /// recent [`Self::resolved_seats_for_role`]/[`Self::resolved_master_assignment`]
    /// call for it. Lets a caller that already holds a resolved profile
    /// assignment for `role` tell "explicit configuration selected this"
    /// from "automatic PATH detection selected this" without re-deriving it.
    pub fn used_builtin_fallback(&self, role: &str) -> bool {
        self.builtin_fallback_selections
            .values()
            .any(|roles| roles.contains(role))
    }

    /// Undo one role's [`Self::apply_builtin_fallback`] warning bookkeeping
    /// for `harness_id` — used when a caller (drive's `assignment_for_role`)
    /// decides, AFTER already computing the live automatic-fallback plan,
    /// that a persisted session assignment should be dispatched instead: the
    /// live selection never actually happened as far as the operator-facing
    /// grouped announcement is concerned, so its bookkeeping must not survive
    /// into [`Self::builtin_fallback_warnings`]. A no-op if `role` was never
    /// recorded under `harness_id` (defensive, not expected in practice).
    pub fn discard_builtin_selection(&mut self, harness_id: &str, role: &str) {
        if let Some(roles) = self.builtin_fallback_selections.get_mut(harness_id) {
            roles.remove(role);
            if roles.is_empty() {
                self.builtin_fallback_selections.remove(harness_id);
            }
        }
    }

    /// Fill `assignment.harness` from the P427 built-in fallback registry
    /// when configuration alone leaves it unset — the lowest-precedence
    /// runtime layer, applied only after every explicit/configured layer
    /// ([`raw_assignment_from_default`]) has already had its chance. Returns
    /// the already-finalized assignment (`Some`) whether or not fallback ran;
    /// `None` only when `assignment` needed a harness and no built-in
    /// candidate is available on `PATH`. A successful fallback selection is
    /// recorded (grouped by harness id) in `builtin_fallback_selections` for
    /// [`Self::builtin_fallback_warnings`].
    fn apply_builtin_fallback(
        &mut self,
        role_label: &str,
        assignment: ProfileAssignment,
    ) -> Option<ProfileAssignment> {
        if let Some(resolved) = finalize_assignment(assignment.clone()) {
            // The harness was already assigned by a real CLI/config layer.
            // If it names a built-in id with no matching `[harness.<id>]`
            // table, the registry lookup dispatch relies on
            // (`prepare_run_assignments`, `resolved.registry.harness.get`)
            // still needs the compiled-in definition made available — the
            // same requirement automatic selection has, just triggered by an
            // explicit assignment instead of a missing one.
            if let Some(id) = resolved.harness.as_deref() {
                self.ensure_builtin_registered(id);
            }
            return Some(resolved);
        }
        let id = self.select_builtin_harness()?;
        self.builtin_fallback_selections
            .entry(id.clone())
            .or_default()
            .insert(role_label.to_string());
        let mut filled = assignment;
        filled.harness = Some(id);
        finalize_assignment(filled)
    }

    /// The first built-in harness detected as available on `PATH`, in fixed
    /// candidate order, registering its (possibly configuration-overridden)
    /// definition via [`Self::ensure_builtin_registered`]. `None` when no
    /// built-in candidate is available; the caller decides what that means
    /// for the role it was resolving.
    fn select_builtin_harness(&mut self) -> Option<String> {
        let row = self
            .ensure_builtin_detection()
            .iter()
            .find(|row| row.available)
            .cloned()?;
        self.ensure_builtin_registered(&row.id);
        Some(row.id)
    }

    /// Make a built-in harness id's (possibly configuration-overridden)
    /// definition available through `self.registry` so every downstream
    /// dispatch/model lookup keyed by harness id finds it exactly as it
    /// would a machine-configured harness — regardless of whether `id` got
    /// there via automatic PATH selection or an explicit `--assign`/
    /// `[agent.*]` assignment that merely names a built-in id with no
    /// matching `[harness.<id>]` table. A non-built-in id (an explicit
    /// assignment naming a real configured-only harness, or one that will
    /// correctly fail as unknown) is left untouched. `pub`: a resumed
    /// dispatch's `ResolvedRuntimeAssignments` is a fresh instance that
    /// never itself ran automatic selection, so drive's `assignment_for_role`
    /// must call this directly for a persisted-session assignment that
    /// names a built-in id — the definition still has to come from
    /// somewhere before dispatch can use it.
    pub fn ensure_builtin_registered(&mut self, id: &str) {
        if self.registry.harness.contains_key(id) {
            return;
        }
        if !built_in_harness_ids().contains(&id) {
            return;
        }
        let definition = built_in_harness_definition(id, &self.registry);
        self.registry.harness.insert(id.to_string(), definition);
    }

    /// The cached P427 built-in-harness PATH-detection table, probed at most
    /// once per `ResolvedRuntimeAssignments` regardless of how many roles or
    /// callers (fallback selection, `ctx traits doctor --config`) need it.
    pub fn builtin_detection(&mut self) -> &[BuiltinHarnessDetection] {
        self.ensure_builtin_detection()
    }

    fn ensure_builtin_detection(&mut self) -> &[BuiltinHarnessDetection] {
        if self.builtin_detection.is_none() {
            self.builtin_detection = Some(detect_builtin_harnesses(&self.registry));
        }
        self.builtin_detection
            .as_deref()
            .expect("just populated above")
    }

    /// One grouped, deterministically ordered plain-output line per built-in
    /// harness automatically selected during this resolution — reusing the
    /// existing warnings channel rather than adding a second output path, per
    /// P427. Empty when no role used automatic fallback.
    pub fn builtin_fallback_warnings(&self) -> Vec<String> {
        self.builtin_fallback_selections
            .iter()
            .map(|(id, roles)| {
                let bin = self
                    .builtin_detection
                    .as_ref()
                    .and_then(|rows| rows.iter().find(|row| &row.id == id))
                    .map(|row| row.bin.as_str())
                    .unwrap_or(id.as_str());
                let roles = roles
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("automatic harness selection: {id} ({bin}) for role(s): {roles}")
            })
            .collect()
    }

    fn resolve_assignment_model(
        &mut self,
        assignment: Option<ProfileAssignment>,
    ) -> crate::Result<Option<ProfileAssignment>> {
        let Some(assignment) = assignment else {
            return Ok(None);
        };
        resolve_assignment_model(
            &self.registry,
            &mut self.model_catalogs,
            &mut self.model_catalog_capability_reports,
            assignment,
        )
        .map(Some)
    }
}

/// Resolve machine-local harness registry/agent defaults from
/// `.ctx/config.toml` and layer `--assign` overrides on top. Callers with no
/// trait package context (merge/generate/run-info) use this; it never
/// consults a package sidecar.
pub fn resolve_runtime_assignments(
    overrides: &[String],
) -> crate::Result<ResolvedRuntimeAssignments> {
    resolve_runtime_assignments_impl(None, None, overrides)
}

/// Trait-aware variant of [`resolve_runtime_assignments`]: also loads the
/// trait package's optional budget-only `config.toml` sidecar, and derives
/// the run's variant identity (P451) from `trait_ref` for `[agent.variant.*]`
/// qualifier resolution. Assignments resolve exactly as in
/// [`resolve_runtime_assignments`] plus the variant/repo qualifier fold (see
/// [`RunScope`]/[`flatten_agent_defaults`]); the sidecar never contributes
/// assignments. Budget precedence is CLI flags > sidecar `[budget]` >
/// built-in defaults (P312; see `ctx-traits-cli`'s `budget_from`).
pub fn resolve_trait_runtime_assignments(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    overrides: &[String],
) -> crate::Result<ResolvedRuntimeAssignments> {
    resolve_runtime_assignments_impl(Some(trait_ref), Some(trait_root), overrides)
}

fn resolve_runtime_assignments_impl(
    trait_ref: Option<&ctx_traits_core::Trait>,
    trait_root: Option<&Utf8Path>,
    overrides: &[String],
) -> crate::Result<ResolvedRuntimeAssignments> {
    let config_report = resolve_config_report(Utf8Path::new("."))?;
    let runtime_config = config_report.runtime;
    let registry = HarnessRegistry {
        schema_version: runtime_config.schema_version,
        harness: runtime_config.harness,
    };
    validate_registry(&registry)?;
    validate_agent_defaults(&runtime_config.agent)?;
    validate_repo_overrides(&runtime_config.repo)?;
    validate_trait_overrides(&runtime_config.trait_defaults)?;
    let mut budget = runtime_config
        .run
        .as_ref()
        .map(|run| run.budget.clone())
        .unwrap_or_default();
    // `resolve_runtime_config` (via `resolve_config_report`) already folded
    // declared named build caches into `worktree.build_cache` (P428).
    let worktree = runtime_config.worktree;
    // 0034: the running trait's own `[trait.<id>]` block, if declared —
    // shared by the port-defaults loop below and the qualifier fold further
    // down, so both read the same lookup.
    let trait_defaults_entry =
        trait_ref.and_then(|trait_ref| runtime_config.trait_defaults.get(trait_ref.id.as_str()));
    let mut port_defaults = BTreeMap::new();
    if let Some(defaults) = trait_defaults_entry {
        for (port, value) in &defaults.defaults.port {
            let field = format!(
                "trait.{}.defaults.port.{port}",
                trait_ref.expect("trait exists").id
            );
            let winner = config_report.winners.get(&field);
            let source = winner
                .and_then(|winner| winner.source.as_deref())
                .unwrap_or("runtime config");
            port_defaults.insert(
                port.clone(),
                ConfiguredPortDefault {
                    value: value.clone(),
                    layer: winner
                        .map(|winner| winner.layer)
                        .unwrap_or(ConfigLayer::Repo),
                    evidence: format!("{source}:{field}"),
                },
            );
        }
    }

    if let Some(trait_root) = trait_root
        && let Some((sidecar, sidecar_path, _tier)) =
            load_selected_trait_run_config(trait_ref, trait_root)?
    {
        // The package sidecar remains a compatibility fallback. Project
        // `[run]` values are nearer and therefore win over it.
        let configured = budget.clone();
        let mut sidecar_budget = sidecar.budget;
        overlay_budget(&mut sidecar_budget, &configured);
        budget = sidecar_budget;
        // Package sidecars are the lowest config layer. Runtime trait-scoped
        // values above remain authoritative.
        for (port, value) in sidecar.defaults.port {
            port_defaults
                .entry(port.clone())
                .or_insert(ConfiguredPortDefault {
                    value,
                    layer: ConfigLayer::BuiltIn,
                    evidence: format!("{}:defaults.port.{port}", sidecar_path),
                });
        }
    }
    if let Some(trait_ref) = trait_ref {
        validate_port_defaults(trait_ref, &port_defaults)?;
    }

    // P451: fold the declared variant/repo qualifier tables into one
    // effective `AgentDefaults` for this run before any override/validation
    // step downstream consumes it — everything after this point (seat
    // overlays, `--assign` layering, standing-seat rules, P475 budgets)
    // operates on the flattened result and needs no further change.
    // Only shell out to `git rev-parse` for the active repo key when a
    // `[repo.*]` block actually exists to match against — every assignment
    // resolution (merge, generate, doctor, worktree prep, run, resume) hits
    // this path, and an empty `repo` map can never match regardless of key.
    let repo_key = if runtime_config.repo.is_empty() {
        None
    } else {
        resolve_run_repo_key()
    };
    let repo_override = repo_key
        .as_deref()
        .and_then(|key| runtime_config.repo.get(key));
    let scope = RunScope {
        variant: trait_ref
            .and_then(|trait_ref| {
                resolve_run_variant(
                    trait_ref,
                    &runtime_config.agent,
                    repo_override,
                    trait_defaults_entry,
                )
            })
            .map(std::borrow::Cow::Owned),
        repo_key: repo_key.map(std::borrow::Cow::Owned),
        trait_id: trait_ref.map(|trait_ref| std::borrow::Cow::Borrowed(trait_ref.id.as_str())),
    };
    let (mut agent_defaults, qualifier_by_role) = flatten_agent_defaults(
        &runtime_config.pre_environment_agent,
        &runtime_config.repo,
        trait_defaults_entry,
        &scope,
    );
    // `$CTX_CONFIG` is the final default layer. Apply it only after the
    // personal repo qualifier fold so its role and variant leaves win. No
    // trait-scope fold here: `$CTX_CONFIG` already wins over every scope by
    // being merged in last.
    let (environment_agent, _) = flatten_agent_defaults(
        &runtime_config.environment_agent,
        &BTreeMap::new(),
        None,
        &scope,
    );
    merge_agent_defaults(&mut agent_defaults, environment_agent);
    // 0025: expand `count`/list-form roles into stable seat aliases now that
    // every scope layer (variant, repo, $CTX_CONFIG) has folded in — the
    // exact point 0034's trait-scope fold must land before, or a
    // trait-scoped `count` would arrive too late to change the expansion.
    expand_role_seats(&mut agent_defaults);
    if !qualifier_by_role.is_empty() {
        validate_role_map(&agent_defaults.role, "agent.role", true)?;
    }

    let overrides = parse_assignment_overrides(overrides)?;
    for (role, assignment) in &overrides.whole {
        reject_tier_override(role, assignment)?;
    }
    for ((role, seat), assignment) in &overrides.seat {
        reject_tier_override(&format!("{role}.{seat}"), assignment)?;
    }
    validate_assignments(&overrides.whole)?;
    validate_seat_overrides(&agent_defaults, &overrides.seat)?;

    Ok(ResolvedRuntimeAssignments {
        registry,
        assignments: overrides.whole,
        seat_assignments: overrides.seat,
        agent_defaults,
        qualifier_by_role,
        budget,
        worktree,
        port_defaults,
        model_catalogs: BTreeMap::new(),
        model_catalog_capability_reports: Vec::new(),
        builtin_detection: None,
        builtin_fallback_selections: BTreeMap::new(),
    })
}

/// The run scope a config qualifier level resolves against (P451): the
/// derived variant name and/or the invocation repository's registry key.
/// Either or both may be absent (ad-hoc invocation, or a caller with no
/// trait context such as merge/generate/run-info).
struct RunScope<'a> {
    variant: Option<std::borrow::Cow<'a, str>>,
    repo_key: Option<std::borrow::Cow<'a, str>>,
    /// 0034: the running trait's id, for `trait:<id>`/`trait:<id>+variant:<v>`
    /// qualifier labels. `None` for a caller with no trait context (merge,
    /// generate, run-info), making trait qualifiers inert exactly like repo
    /// qualifiers on those same ad-hoc invocations.
    trait_id: Option<std::borrow::Cow<'a, str>>,
}

/// The invocation repository's P426 registry key for repo-qualifier
/// resolution: `repo_key(canonical_repo_root(discover_main_repo_root(cwd)))`
/// — the MAIN checkout root, not `current_repo_key()`, since runs execute
/// inside `.ctx/worktrees/wt-*` linked worktrees whose own
/// `rev-parse --show-toplevel` yields a different key that would silently
/// stop matching a repo qualifier declared for the main checkout. `None` for
/// an ad-hoc (non-Git) invocation, making repo qualifiers inert rather than
/// an error.
fn resolve_run_repo_key() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let cwd = Utf8PathBuf::from_path_buf(cwd).ok()?;
    let main_root = crate::repository::discover_main_repo_root(&cwd).ok()?;
    let canonical = crate::state::canonical_repo_root(&main_root).ok()?;
    Some(crate::state::repo_key(&canonical))
}

/// The active P451 repo-qualifier key for the current invocation — `None`
/// for an ad-hoc (non-Git) invocation. `ctx traits doctor --config` reports
/// this so an operator copying a key from `repos.toml` (which also contains
/// worktree-derived keys) can confirm which one actually matches a
/// `[repo."<key>"]` block.
pub fn active_repo_qualifier_key() -> Option<String> {
    resolve_run_repo_key()
}

/// The run's variant identity (P451): `trait.metadata.variant` when
/// declared; else the longest operator-declared `[agent.variant.<v>]` key
/// `v` such that the resolved trait id ends with `-<v>`; else `None`. Only
/// keys the operator actually declared a qualifier for are considered, so an
/// id ending in a coincidental hyphenated suffix (`deep-research`) never
/// silently acquires a variant unless a rule for that exact suffix exists.
///
/// The candidate key set is `defaults.variant` (the base `[agent.variant.*]`
/// table) unioned with `repo_override.agent.variant` when the active repo key
/// matched a declared `[repo."<key>"]` block, and with the running trait's
/// own `[trait.<id>.variant.*]` table (0034) — a variant declared solely
/// under the repo- or trait-scoped table (with no base-level
/// `[agent.variant.<v>]` sibling) must still be derivable, or the
/// `(repo,role,variant)`/`(trait,variant,role)` rung could never be reached
/// for that config shape.
fn resolve_run_variant(
    trait_ref: &ctx_traits_core::Trait,
    defaults: &AgentDefaults,
    repo_override: Option<&RepoOverride>,
    trait_defaults: Option<&TraitDefaults>,
) -> Option<String> {
    if let Some(variant) = trait_ref
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.variant.as_ref())
    {
        return Some(variant.as_str().to_string());
    }
    let id = trait_ref.id.as_str();
    defaults
        .variant
        .keys()
        .chain(
            repo_override
                .into_iter()
                .flat_map(|repo_override| repo_override.agent.variant.keys()),
        )
        .chain(
            trait_defaults
                .into_iter()
                .flat_map(|trait_defaults| trait_defaults.variant.keys()),
        )
        .filter(|key| id.ends_with(&format!("-{key}")))
        .max_by_key(|key| key.len())
        .cloned()
}

/// Layer one `(role, variant)`/`(role, repo)`/`(role, repo, variant)`
/// qualifier level onto the chain accumulated so far (P451): a `Single` onto
/// a `Single` (or an absent chain start) inherits field-by-field via
/// [`merge_assignment_fields`]; every other transition (a `List` at either
/// side) replaces the accumulated value wholesale — the most-specific
/// declared level's list always wins WHOLE, never merged per-seat across
/// levels. When the chain has not started (`current` is `None`) and `next` is
/// itself a `Single`, the chain seeds from [`base_for_role`] — the same base
/// a role with no table of its own would otherwise resolve to — so a
/// variant/repo override with no base table of its own does not silently
/// lose the `[agent.role.default]` seat.
fn combine_role_level(
    current: Option<RoleAssignmentValue>,
    next: &RoleAssignmentValue,
    defaults: &AgentDefaults,
    role: &str,
) -> RoleAssignmentValue {
    match (current, next) {
        (None, RoleAssignmentValue::List(entries)) => RoleAssignmentValue::List(entries.clone()),
        (None, RoleAssignmentValue::Single(next_single)) => {
            let mut seed = base_for_role(defaults, role, None);
            merge_assignment_fields(&mut seed, next_single);
            RoleAssignmentValue::Single(seed)
        }
        (
            Some(RoleAssignmentValue::Single(mut base_single)),
            RoleAssignmentValue::Single(next_single),
        ) => {
            merge_assignment_fields(&mut base_single, next_single);
            RoleAssignmentValue::Single(base_single)
        }
        (Some(_), RoleAssignmentValue::List(entries)) => RoleAssignmentValue::List(entries.clone()),
        (Some(_), RoleAssignmentValue::Single(next_single)) => {
            RoleAssignmentValue::Single(next_single.clone())
        }
    }
}

/// Fold every declared level for one role — base, `(role,variant)`,
/// `(repo,role)`, `(repo,role,variant)`, `(trait,role)`, `(trait,variant,role)`
/// (0034), in that least-to-most-specific order — into the effective value
/// plus the qualifier label of whichever level actually won (`None` when only
/// the base table contributed). Trait rungs land after repo rungs so a trait
/// wins over a matching `[repo.<key>]` (the task's decision: trait is the
/// more specific qualifier).
fn fold_role(
    defaults: &AgentDefaults,
    repo_override: Option<&RepoOverride>,
    trait_defaults: Option<&TraitDefaults>,
    scope: &RunScope,
    role: &str,
) -> (Option<RoleAssignmentValue>, Option<String>) {
    let mut current = defaults.role.get(role).cloned();
    let mut qualifier = None;

    if let Some(variant) = scope.variant.as_deref()
        && let Some(level) = defaults
            .variant
            .get(variant)
            .and_then(|value| value.role.get(role))
    {
        current = Some(combine_role_level(current, level, defaults, role));
        qualifier = Some(format!("variant:{variant}"));
    }
    if let Some(repo_override) = repo_override {
        let repo_key = scope.repo_key.as_deref().unwrap_or_default();
        if let Some(level) = repo_override.agent.role.get(role) {
            current = Some(combine_role_level(current, level, defaults, role));
            qualifier = Some(format!("repo:{repo_key}"));
        }
        if let Some(variant) = scope.variant.as_deref()
            && let Some(level) = repo_override
                .agent
                .variant
                .get(variant)
                .and_then(|value| value.role.get(role))
        {
            current = Some(combine_role_level(current, level, defaults, role));
            qualifier = Some(format!("repo:{repo_key}+variant:{variant}"));
        }
    }
    if let Some(trait_defaults) = trait_defaults {
        let trait_id = scope.trait_id.as_deref().unwrap_or_default();
        if let Some(level) = trait_defaults.agent.role.get(role) {
            current = Some(combine_role_level(current, level, defaults, role));
            qualifier = Some(format!("trait:{trait_id}"));
        }
        if let Some(variant) = scope.variant.as_deref()
            && let Some(level) = trait_defaults
                .variant
                .get(variant)
                .and_then(|value| value.agent.role.get(role))
        {
            current = Some(combine_role_level(current, level, defaults, role));
            qualifier = Some(format!("trait:{trait_id}+variant:{variant}"));
        }
    }
    (current, qualifier)
}

/// The whole fold (P451, extended 0034 for trait scope): resolve every
/// declared qualifier table against `scope` into one effective
/// `AgentDefaults`, plus a `role -> qualifier` map for evidence (only for
/// roles a qualified level actually won). Returns `defaults` unchanged with
/// no extra winners when neither `defaults.variant`, `repo`, nor
/// `trait_defaults` declares anything at all — the structural guarantee
/// behind "an unqualified config resolves exactly as before".
fn flatten_agent_defaults(
    defaults: &AgentDefaults,
    repo: &BTreeMap<String, RepoOverride>,
    trait_defaults: Option<&TraitDefaults>,
    scope: &RunScope,
) -> (AgentDefaults, BTreeMap<String, String>) {
    let trait_defaults =
        trait_defaults.filter(|value| !value.agent.role.is_empty() || !value.variant.is_empty());
    if defaults.variant.is_empty() && repo.is_empty() && trait_defaults.is_none() {
        return (defaults.clone(), BTreeMap::new());
    }
    let repo_override = scope.repo_key.as_deref().and_then(|key| repo.get(key));
    if scope.variant.is_none() && repo_override.is_none() && trait_defaults.is_none() {
        return (defaults.clone(), BTreeMap::new());
    }

    let mut role_names: BTreeSet<String> = defaults.role.keys().cloned().collect();
    if let Some(variant) = scope.variant.as_deref()
        && let Some(value) = defaults.variant.get(variant)
    {
        role_names.extend(value.role.keys().cloned());
    }
    if let Some(repo_override) = repo_override {
        role_names.extend(repo_override.agent.role.keys().cloned());
        if let Some(variant) = scope.variant.as_deref()
            && let Some(value) = repo_override.agent.variant.get(variant)
        {
            role_names.extend(value.role.keys().cloned());
        }
    }
    if let Some(trait_defaults) = trait_defaults {
        role_names.extend(trait_defaults.agent.role.keys().cloned());
        if let Some(variant) = scope.variant.as_deref()
            && let Some(value) = trait_defaults.variant.get(variant)
        {
            role_names.extend(value.agent.role.keys().cloned());
        }
    }

    let mut role = BTreeMap::new();
    let mut qualifier_by_role = BTreeMap::new();
    for name in role_names {
        let (value, qualifier) = fold_role(defaults, repo_override, trait_defaults, scope, &name);
        if let Some(value) = value {
            role.insert(name.clone(), value);
        }
        if let Some(qualifier) = qualifier {
            qualifier_by_role.insert(name, qualifier);
        }
    }
    (
        AgentDefaults {
            model_tier: defaults.model_tier.clone(),
            role,
            variant: BTreeMap::new(),
        },
        qualifier_by_role,
    )
}

fn reject_tier_override(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    if assignment.model.is_none() && assignment.model_tier.is_some() {
        return invalid_config(
            format!("assign.{role}.model-tier"),
            "model tiers no longer select models; provide a concrete model or alias instead",
        );
    }
    Ok(())
}

/// Reject a `--assign role.N=...` seat override whose seat is out of range
/// for `role`'s configured `[[agent.role.<role>]]` list, or whose role is
/// not configured as a list at all (a single-table or wholly unconfigured
/// role has no seats to select) — and, equally for every seat override,
/// reject an assignment value that fails the same [`validate_assignment`]
/// contract a whole-role override must pass (P456 fix: seat overrides
/// previously checked only list membership/range, never the assignment
/// value itself, so an invalid attach/harness/reasoning-effort value could
/// slip through on a seat although the identical whole-role override would
/// be rejected).
fn validate_seat_overrides(
    defaults: &AgentDefaults,
    seat_overrides: &BTreeMap<(String, u32), ProfileAssignment>,
) -> crate::Result<()> {
    for ((role, seat), assignment) in seat_overrides {
        let seat_role = format!("{role}.{seat}");
        let field_path = format!("assign.{seat_role}");
        let list_length = match defaults.role.get(role) {
            Some(RoleAssignmentValue::List(entries)) => entries.len(),
            _ => {
                return invalid_config(
                    field_path,
                    format!(
                        "role {role:?} is not configured as a [[agent.role.{role}]] list; seat selectors require a list-backed role"
                    ),
                );
            }
        };
        if usize::try_from(*seat).unwrap_or(usize::MAX) > list_length {
            return invalid_config(
                field_path,
                format!(
                    "seat {seat} is out of range for role {role:?} (configured list has {list_length} entries)"
                ),
            );
        }
        validate_assignment(&seat_role, assignment)?;
    }
    Ok(())
}

/// Overlay one `RunProfileBudget` onto another: only fields present on
/// `next` replace `base`, so a profile/CLI budget that sets only some fields
/// does not erase the rest of a sidecar's budget.
fn validate_port_defaults(
    trait_ref: &ctx_traits_core::Trait,
    defaults: &BTreeMap<String, ConfiguredPortDefault>,
) -> crate::Result<()> {
    for (port_id, default) in defaults {
        let Some(port) = trait_ref.ports.iter().find(|port| port.id == *port_id) else {
            return invalid_config(
                default.evidence.clone(),
                format!("unknown input port {port_id:?}"),
            );
        };
        if !matches!(
            port.direction,
            ctx_traits_core::r#trait::PortDirection::Input
        ) {
            return invalid_config(
                default.evidence.clone(),
                format!("port {port_id:?} is not an input port"),
            );
        }
        if port.schema != "schema:text" {
            return invalid_config(
                default.evidence.clone(),
                format!("port {port_id:?} must use schema:text for a static config default"),
            );
        }
    }
    Ok(())
}

fn overlay_budget(base: &mut RunProfileBudget, next: &RunProfileBudget) {
    if next.max_frames.is_some() {
        base.max_frames = next.max_frames;
    }
    if next.frame_seconds.is_some() {
        base.frame_seconds = next.frame_seconds;
    }
    if next.total_seconds.is_some() {
        base.total_seconds = next.total_seconds;
    }
    if next.max_retries.is_some() {
        base.max_retries = next.max_retries;
    }
    if next.attach_wait_seconds.is_some() {
        base.attach_wait_seconds = next.attach_wait_seconds;
    }
    if next.idle_seconds.is_some() {
        base.idle_seconds = next.idle_seconds;
    }
    // 0066.4: these two were added to `RunProfileBudget` by 0058 but never
    // wired into this overlay — a repo's declared `[run] command-seconds`/
    // `command-idle-seconds` decoded into `next` and then silently vanished
    // on every merge, so `resolve_command_bounds` never saw anything but the
    // built-in default regardless of what a repo configured.
    if next.command_seconds.is_some() {
        base.command_seconds = next.command_seconds;
    }
    if next.command_idle_seconds.is_some() {
        base.command_idle_seconds = next.command_idle_seconds;
    }
}

/// Load the optional package-root `config.toml` sidecar (legacy, P312;
/// superseded by [`PackageRuntimeConfig`]). Returns `Ok(None)` only when the
/// file is absent; a malformed or out-of-scope sidecar (e.g. an `[assign]`
/// or `[worktree]` table) is always a hard structured `deny_unknown_fields`
/// decode error, never silently ignored.
pub fn load_trait_run_config(trait_root: &Utf8Path) -> crate::Result<Option<TraitRunConfig>> {
    let path = crate::layout::package_run_config_path(trait_root);
    let Some(text) = crate::read::read_optional_text(&path)? else {
        return Ok(None);
    };
    let config: TraitRunConfig =
        toml::from_str(&text).map_err(|source| crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        })?;
    Ok(Some(config))
}

/// Which package-level run-config tier is active for a resolved trait, and
/// the path it was read from — surfaced to `ctx traits check`. `Ok(None)`
/// means no package tier is active (built-in defaults apply).
pub fn describe_active_package_run_config(
    trait_ref: Option<&ctx_traits_core::Trait>,
    trait_root: &Utf8Path,
) -> crate::Result<Option<(PackageRunConfigTier, Utf8PathBuf)>> {
    Ok(load_selected_trait_run_config(trait_ref, trait_root)?.map(|(_, path, tier)| (tier, path)))
}

/// Resolve the effective package-tier run config for the selected variant,
/// in precedence order: committed `runtime.toml` ([`PackageRuntimeConfig`],
/// top-level budget overlaid by the selected `[variant.<vid>]`) beats a
/// native family's declared per-variant sidecar (part of the family
/// manifest, so a missing or malformed one is a hard configuration error),
/// which beats the legacy package-root `config.toml` sidecar. A package
/// carrying `runtime.toml` uses it exclusively — the legacy forms are never
/// consulted once it exists.
fn load_selected_trait_run_config(
    trait_ref: Option<&ctx_traits_core::Trait>,
    trait_root: &Utf8Path,
) -> crate::Result<Option<(TraitRunConfig, Utf8PathBuf, PackageRunConfigTier)>> {
    let runtime_path = crate::layout::package_runtime_config_path(trait_root);
    if let Some(text) = crate::read::read_optional_text(&runtime_path)? {
        let config = PackageRuntimeConfig::decode(&text, &runtime_path)?;
        let mut budget = config.budget;
        if let Some(variant_budget) = trait_ref
            .and_then(|trait_ref| trait_ref.variant.as_deref())
            .and_then(|variant| config.variant.get(variant))
        {
            overlay_budget(&mut budget, variant_budget);
        }
        return Ok(Some((
            TraitRunConfig {
                schema_version: config.schema_version,
                budget,
                defaults: config.defaults,
            },
            runtime_path,
            PackageRunConfigTier::Runtime,
        )));
    }

    let selected = if let Some(variant) = trait_ref.and_then(|trait_ref| trait_ref.variant.as_ref())
    {
        crate::family_manifest::read_family_table(&crate::layout::package_manifest_path(
            trait_root,
        ))?
        .and_then(|table| {
            table
                .variant(variant.as_str())
                .and_then(|(_, variant)| variant.run_config.clone())
        })
    } else {
        None
    };
    if let Some(relative_path) = selected {
        let path = trait_root.join(relative_path);
        if !path.is_file() {
            return Err(crate::Error::Usage {
                message: format!("declared native-family run config does not exist: {path}"),
            });
        }
        let config = decode_trait_run_config_at(&path)?;
        return Ok(Some((config, path, PackageRunConfigTier::LegacyDeclared)));
    }
    Ok(load_trait_run_config(trait_root)?.map(|config| {
        let path = crate::layout::package_run_config_path(trait_root);
        (config, path, PackageRunConfigTier::LegacySidecar)
    }))
}

/// Decode a [`TraitRunConfig`] (legacy budget-only sidecar shape) from an
/// explicit file path. Shared by the declared per-variant lookup above and
/// by the rebuild-time consolidation into `runtime.toml`
/// ([`render_package_runtime_config`]), which reads the same legacy files
/// before their `run-config` declarations are dropped.
pub fn decode_trait_run_config_at(path: &Utf8Path) -> crate::Result<TraitRunConfig> {
    let text = crate::read::read_text(path)?;
    let config: TraitRunConfig =
        toml::from_str(&text).map_err(|source| crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        })?;
    Ok(config)
}

/// Render a [`PackageRuntimeConfig`]-shaped document from a resolved default
/// budget plus per-variant overlays: the one-time rebuild consolidation
/// `publish_cdk_family` runs when an existing family declares `run-config`
/// files and no `runtime.toml` exists yet, so a rebuild never orphans
/// authored budgets once the declarations are dropped. `default_defaults` is
/// the default variant's `[defaults.port]` table (empty when none was
/// authored) — carried forward at top level per the same schema decision
/// that keeps `[defaults.port]` live in [`PackageRuntimeConfig`].
pub fn render_package_runtime_config(
    default_budget: &RunProfileBudget,
    default_defaults: &PortDefaults,
    variant_budgets: &BTreeMap<String, RunProfileBudget>,
) -> String {
    let mut text = String::new();
    push_budget_lines(&mut text, default_budget);
    if !default_defaults.port.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("[defaults.port]\n");
        for (port, value) in &default_defaults.port {
            text.push_str(&format!("{port} = {value:?}\n"));
        }
    }
    for (name, budget) in variant_budgets {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("[variant.{name}]\n"));
        push_budget_lines(&mut text, budget);
    }
    text
}

fn push_budget_lines(text: &mut String, budget: &RunProfileBudget) {
    let fields: [(&str, Option<u64>); 8] = [
        ("max-frames", budget.max_frames),
        ("frame-seconds", budget.frame_seconds),
        ("total-seconds", budget.total_seconds),
        ("max-retries", budget.max_retries),
        ("attach-wait-seconds", budget.attach_wait_seconds),
        ("idle-seconds", budget.idle_seconds),
        ("command-seconds", budget.command_seconds),
        ("command-idle-seconds", budget.command_idle_seconds),
    ];
    for (key, value) in fields {
        if let Some(value) = value {
            text.push_str(&format!("{key} = {value}\n"));
        }
    }
}

/// Load and validate a caller-selected narrow runtime profile from an
/// explicit file path (P403's `--run-profile`). A missing or malformed file,
/// or one declaring assignments the same `validate_assignments` contract
/// used for `--assign` rejects, fails here with a field/path-specific error
/// before any harness dispatch.
pub fn load_run_profile_document(path: &Utf8Path) -> crate::Result<RunProfileDocument> {
    let text = crate::read::read_text(path)?;
    let document: RunProfileDocument =
        toml::from_str(&text).map_err(|source| crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        })?;
    for (role, assignment) in &document.assign {
        let field_path = format!("run-profile.assign.{role}");
        normalize_role(role, &field_path)?;
        if let Some(harness) = assignment.harness.as_deref() {
            validate_bare_id(harness, &format!("{field_path}.harness"))?;
        }
    }
    Ok(document)
}

/// Reject a run profile that assigns a role the resolved trait it will route
/// into does not declare as `[[agent]]`, before any harness dispatch: a
/// `--run-profile` covers only the harness-backed built-in runner it is
/// passed to, so a stray or misspelled role must fail loudly rather than be
/// silently ignored.
pub fn validate_run_profile_roles(
    document: &RunProfileDocument,
    declared_roles: &BTreeSet<String>,
) -> crate::Result<()> {
    for role in document.assign.keys() {
        if !declared_roles.contains(role) {
            return invalid_config(
                format!("run-profile.assign.{role}"),
                format!(
                    "assignment role {role:?} is not declared as [[agent]] by the resolved trait"
                ),
            );
        }
    }
    Ok(())
}

/// The corrective action for an unassigned agent role, shared by every
/// surface that can report it (assignment preparation here, the CLI drive
/// loop, and session next-action hints) so the remediation text is written
/// once instead of drifting across three near-duplicate copies.
pub fn unassigned_role_remediation(role: &str) -> String {
    format!("pass --assign {role}=<harness-id> or add it to .ctx/config.toml [agent.role.{role}]")
}

/// 0025: guard against a trait agent id that looks like an expansion seat
/// alias (`<base>-<digits>`) but names a seat past the base role's
/// configured `count`/list length — e.g. `smart-3` when `[agent.role.smart]`
/// only declares `count = 2`. Without this check the id would silently fall
/// through to `[agent.role.default]` (the ordinary no-table-of-its-own
/// fallback), degrading a seat-identity typo/mismatch into a quietly wrong
/// assignment instead of a loud one. `None` for any id that already has its
/// own table (including a seat expansion already produced) or whose base is
/// not expansion-shaped — the ordinary resolution path handles both.
fn expansion_seat_out_of_range(defaults: &AgentDefaults, agent_id: &str) -> Option<crate::Error> {
    if defaults.role.contains_key(agent_id) {
        return None;
    }
    let (base, suffix) = agent_id.rsplit_once('-')?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seat_count = match defaults.role.get(base)? {
        RoleAssignmentValue::Single(assignment) => assignment.count?,
        RoleAssignmentValue::List(entries) => u32::try_from(entries.len()).ok()?,
    };
    let requested: u32 = suffix.parse().ok()?;
    if (1..=seat_count).contains(&requested) {
        return None;
    }
    Some(config_error(
        format!("run.assign.{agent_id}"),
        format!(
            "declared agent role {agent_id:?} has no runtime assignment; role {base:?} expands to only {seat_count} seat(s) — add [[agent.role.{base}]] entries or count = {requested} to cover seat {requested}"
        ),
    ))
}

/// True when a declared trait agent id will resolve by inheriting the whole
/// `[agent.role.default]` seat: it has no role table of its own, no
/// `--assign` override, is not a standing seat, and is not an expansion seat
/// of an authored base table. The inheritance itself is intended behavior
/// (see `base_for_role`); this predicate exists so `prepare_run_assignments`
/// can say so out loud — a silently inherited default seat is how a trait's
/// "smart" role ends up on the default model with nobody noticing.
fn inherits_default_seat(
    defaults: &AgentDefaults,
    overrides: &BTreeMap<String, ProfileAssignment>,
    agent_id: &str,
) -> bool {
    if agent_id == DEFAULT_SEAT
        || is_standing_seat(agent_id)
        || defaults.role.contains_key(agent_id)
        || overrides.contains_key(agent_id)
    {
        return false;
    }
    if let Some((base, suffix)) = agent_id.rsplit_once('-')
        && !suffix.is_empty()
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
        && defaults.role.contains_key(base)
    {
        // An expansion seat resolves from its authored base; an out-of-range
        // seat already errored in `expansion_seat_out_of_range`.
        return false;
    }
    true
}

pub fn prepare_run_assignments(
    trait_ref: &ctx_traits_core::Trait,
    trait_root: &Utf8Path,
    overrides: &[String],
) -> crate::Result<PreparedRunAssignments> {
    let mut resolved = resolve_trait_runtime_assignments(trait_ref, trait_root, overrides)?;
    // P427: a trait that declares agent roles must still get a chance at
    // automatic built-in fallback even with zero explicit config — bypassing
    // straight to the "single-agent compatible" no-assignment report (as a
    // fully agentless trait legitimately does) would silently skip fallback
    // and leave every declared role permanently unassigned.
    if overrides.is_empty()
        && resolved.assignments.is_empty()
        && resolved.agent_defaults.role.is_empty()
        && trait_ref.agents.is_empty()
    {
        return Ok(PreparedRunAssignments {
            assignments: None,
            harness_probes: Vec::new(),
            warnings: Vec::new(),
            capability_reports: Vec::new(),
            worktree: resolved.worktree,
            port_defaults: resolved.port_defaults,
        });
    }

    let mut prepared = Vec::new();
    let mut harness_ids = BTreeSet::new();
    let mut seat_warnings: Vec<String> = Vec::new();
    for agent in &trait_ref.agents {
        if let Some(err) = expansion_seat_out_of_range(&resolved.agent_defaults, &agent.id) {
            return Err(err);
        }
        let seats = resolved.resolved_seats_for_role(&agent.id)?;
        if inherits_default_seat(&resolved.agent_defaults, &resolved.assignments, &agent.id) {
            let seat_desc = seats
                .first()
                .map(|(assignment, _)| {
                    let harness = assignment.harness.as_deref().unwrap_or("unassigned");
                    match assignment.model.as_deref() {
                        Some(model) => format!("harness={harness} model={model}"),
                        None => format!("harness={harness}"),
                    }
                })
                .unwrap_or_else(|| "unassigned".to_string());
            seat_warnings.push(format!(
                "agent role {:?} has no [agent.role.{}] table and no --assign override; it inherits the whole default seat ({seat_desc}) — add a role table or rename the trait agent to a configured role",
                agent.id, agent.id
            ));
        }
        if seats.is_empty() {
            // `resolved_seats_for_role` already attempted the P427 built-in
            // fallback for every seat that reached here without a
            // configured harness; an empty result means that attempt also
            // found no candidate on `PATH`, so the no-candidate remediation
            // (naming every probed binary) is the accurate guidance now,
            // not the older "add an --assign" hint alone.
            let rows = resolved.builtin_detection().to_vec();
            return invalid_config(
                format!("run.assign.{}", agent.id),
                format!(
                    "declared agent role {:?} has no runtime assignment; {} — {}",
                    agent.id,
                    unassigned_role_remediation(&agent.id),
                    no_builtin_harness_message(&rows, &agent.id)
                ),
            );
        };
        for (assignment, seat_info) in seats {
            let qualifier = resolved
                .qualifier_by_role
                .get(&agent.id)
                .map(String::as_str);
            let evidence = assignment_evidence(&agent.id, &assignment, seat_info, qualifier);
            match assignment.mode {
                RunAssignmentMode::Attach => {
                    prepared.push(ctx_traits_core::procedure::session::AgentAssignment {
                        role: agent.id.clone(),
                        harness: "attach".to_string(),
                        transport: "attach".to_string(),
                        model: None,
                        evidence,
                        seat_index: seat_info.map(|info| info.seat_index),
                        list_length: seat_info.map(|info| info.list_length),
                        budget: role_budget_evidence(&assignment.budget),
                    });
                }
                RunAssignmentMode::Harness => {
                    let harness_id = assignment.harness.as_deref().ok_or_else(|| {
                        config_error(
                            format!("assign.{}.harness", agent.id),
                            "harness-mode assignment must declare harness",
                        )
                    })?;
                    let harness = resolved.registry.harness.get(harness_id).ok_or_else(|| {
                        config_error(
                            format!("assign.{}.harness", agent.id),
                            format!("unknown harness id {harness_id:?}"),
                        )
                    })?;
                    let transport = assignment.transport.unwrap_or(RunTransport::Cli);
                    if !harness.transports.contains(&transport) {
                        return invalid_config(
                            format!("assign.{}.transport", agent.id),
                            format!(
                                "harness {harness_id:?} does not declare transport {}",
                                transport.as_str()
                            ),
                        );
                    }
                    harness_ids.insert(harness_id.to_string());
                    prepared.push(ctx_traits_core::procedure::session::AgentAssignment {
                        role: agent.id.clone(),
                        harness: harness_id.to_string(),
                        transport: transport.as_str().to_string(),
                        model: assignment.model.clone(),
                        evidence,
                        seat_index: seat_info.map(|info| info.seat_index),
                        list_length: seat_info.map(|info| info.list_length),
                        budget: role_budget_evidence(&assignment.budget),
                    });
                }
            }
        }
    }
    for role in resolved.assignments.keys() {
        if is_standing_seat(role) {
            continue;
        }
        if !trait_ref.agents.iter().any(|agent| agent.id == *role) {
            return invalid_config(
                format!("assign.{role}"),
                format!("assignment role {role:?} is not declared as [[agent]]"),
            );
        }
    }
    for (role, _seat) in resolved.seat_assignments.keys() {
        if !trait_ref.agents.iter().any(|agent| agent.id == *role) {
            return invalid_config(
                format!("assign.{role}"),
                format!("assignment role {role:?} is not declared as [[agent]]"),
            );
        }
    }

    // Roles filled by the P427 built-in fallback were already probed once
    // for detection; reuse that evidence here instead of re-probing through
    // `probe_harnesses`, which only knows about explicitly configured
    // harnesses in `resolved.registry` (a fallback selection did insert the
    // built-in's definition there for dispatch to find, but re-probing it
    // would violate "one probe per candidate per resolver invocation").
    let fallback_ids: BTreeSet<String> = resolved
        .builtin_fallback_selections
        .keys()
        .cloned()
        .collect();
    let configured_probe_ids: BTreeSet<String> =
        harness_ids.difference(&fallback_ids).cloned().collect();
    let (mut harness_probes, mut warnings, mut capability_reports) =
        probe_harnesses(&resolved.registry, &configured_probe_ids);
    warnings.extend(seat_warnings);
    warnings.extend(resolved.builtin_fallback_warnings());
    for id in &fallback_ids {
        let Some(row) = resolved
            .builtin_detection()
            .iter()
            .find(|row| &row.id == id)
            .cloned()
        else {
            continue;
        };
        harness_probes.push(ctx_traits_core::procedure::session::HarnessProbeEvidence {
            harness_id: row.id.clone(),
            bin: row.bin.clone(),
            version: row.version.clone().unwrap_or_default(),
        });
        capability_reports.push(ctx_traits_core::response::CapabilityReport::supported(
            format!("runtime.harness-probe.{id}"),
        ));
    }
    harness_probes.sort_by(|a, b| a.harness_id.cmp(&b.harness_id));
    capability_reports.extend(resolved.model_catalog_capability_reports);
    capability_reports.sort();
    capability_reports.dedup();
    prepared.sort_by(|a, b| (&a.role, a.seat_index).cmp(&(&b.role, b.seat_index)));
    Ok(PreparedRunAssignments {
        assignments: Some(prepared),
        harness_probes,
        warnings,
        capability_reports,
        worktree: resolved.worktree,
        port_defaults: resolved.port_defaults,
    })
}

pub fn load_harness_registry(path: &Utf8Path) -> crate::Result<HarnessRegistry> {
    let text = crate::read::read_text(path)?;
    toml::from_str(&text).map_err(|source| {
        crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        }
        .into()
    })
}

pub fn resolve_runtime_config(start_dir: &Utf8Path) -> crate::Result<RuntimeConfig> {
    Ok(resolve_config_report(start_dir)?.runtime)
}

/// Resolve the runtime document with the distinct machine/project precedence
/// rules. The report is also the source of doctor provenance; callers should
/// not infer winners by comparing the final values.
pub fn resolve_config_report(start_dir: &Utf8Path) -> crate::Result<ConfigReport> {
    let layers = runtime_config_layers(start_dir)?;
    let mut documents = Vec::new();
    let mut winners = BTreeMap::new();
    let mut tier_warnings = Vec::new();

    for (layer, path) in layers {
        if crate::config_source::is_generated_config_candidate(&path) {
            crate::config_source::guard_never_built(&path)?;
        }
        if !path.exists() {
            continue;
        }
        let next = load_runtime_config(&path)?;
        if has_tier_declaration(&next.agent) {
            tier_warnings.push(format!("retired model-tier declaration in {path}"));
        }
        if layer != ConfigLayer::UserGlobal && !next.repo.is_empty() {
            return invalid_config(
                "repo",
                format!(
                    "[repo.*] blocks are only accepted in the carried global config file ({GLOBAL_RUNTIME_CONFIG}); found one in {path}"
                ),
            );
        }
        documents.push((layer, path, next));
    }

    let mut project = RuntimeConfig::default();
    for (layer, path, next) in &documents {
        if *layer == ConfigLayer::Environment {
            continue;
        }
        merge_project_config(
            &mut project,
            next.clone(),
            *layer,
            Some(path.to_string()),
            &mut winners,
        );
    }
    let mut machine = RuntimeConfig::default();
    for wanted in [ConfigLayer::UserGlobal, ConfigLayer::Repo] {
        for (layer, path, next) in &documents {
            if *layer == wanted {
                merge_machine_config(
                    &mut machine,
                    next.clone(),
                    *layer,
                    Some(path.to_string()),
                    &mut winners,
                );
            }
        }
    }

    let mut runtime = project;
    let effective_repo_requirements = effective_repo_requirements(&documents);
    // Project merging records most winners as it goes, but schema-version has
    // no generic overlay branch. Record every retained requirement here from
    // the same last-effective map that protects it from CTX_CONFIG.
    record_effective_repo_requirement_winners(&effective_repo_requirements, &mut winners);
    // Machine tables are authoritative for these facts. Keep project-only
    // fields from the project resolution and then install machine fields.
    // Schema compatibility is document-local/repository-owned, not an
    // environment-overridable runtime knob.
    runtime.harness = machine.harness;
    runtime.agent = machine.agent;
    // P451: `[repo.*]` blocks are machine-scoped (global-file-only) exactly
    // like `agent`/`harness` above — without this the repo qualifier is
    // parsed and merged into `machine` but never reaches the returned
    // runtime document, so `resolve_runtime_assignments_impl` (which reads
    // `runtime_config.repo`) and `doctor --config` never see it.
    runtime.repo = machine.repo;
    runtime.pre_environment_agent = runtime.agent.clone();
    for (layer, _, document) in &documents {
        if *layer == ConfigLayer::Environment {
            merge_agent_defaults(&mut runtime.environment_agent, document.agent.clone());
        }
    }
    // A matching global `[repo."<key>"]` is a personal qualifier. It sits
    // after repository defaults but before CTX_CONFIG defaults.
    let active_repo_key = active_repo_qualifier_key();
    // The carried global paths are a compatibility chain, not alternatives:
    // merge every matching qualifier in legacy-to-current order.
    let personal: Vec<_> = documents
        .iter()
        .filter_map(|(layer, path, document)| {
            (*layer == ConfigLayer::UserGlobal)
                .then(|| {
                    active_repo_key
                        .as_ref()
                        .and_then(|key| document.repo.get(key).map(|value| (path, value)))
                })
                .flatten()
        })
        .collect();
    // Rebuild classified additive values from every contributor. These are
    // deliberately not handled by broad table overlays: list order is stable
    // first-occurrence order and repository map keys cannot be displaced.
    apply_additive_values(&mut runtime, &documents, &personal, &mut winners);
    // A matching global qualifier is a default layer after repository config
    // and before CTX_CONFIG. Its type cannot express requirements.
    for (path, qualifier) in &personal {
        apply_repo_defaults(
            &mut runtime,
            qualifier,
            ConfigLayer::UserGlobal,
            Some(path.to_string()),
            &mut winners,
        );
    }
    for (layer, path, document) in &documents {
        if *layer == ConfigLayer::Environment {
            apply_environment_requirement_leaves(
                &mut runtime,
                document,
                &effective_repo_requirements,
                *layer,
                Some(path.to_string()),
                &mut winners,
            );
            apply_environment_defaults(
                &mut runtime,
                document,
                *layer,
                Some(path.to_string()),
                &mut winners,
            );
        }
    }
    // Validate the effective agent map even though the environment tier stays
    // transient until qualifier flattening. `doctor --config` must reject an
    // invalid CTX_CONFIG assignment just like run/session resolution does.
    let mut validation_agent = runtime.agent.clone();
    merge_agent_defaults(&mut validation_agent, runtime.environment_agent.clone());
    validate_agent_defaults(&validation_agent)?;
    let requirement_conflicts =
        requirement_conflicts_for_effective(&documents, &effective_repo_requirements);
    // P568: fold each built-in harness's config table over its compiled-in
    // definition ONCE, here, where the runtime document is finalized. Every
    // consumer reads `runtime.harness` directly — dispatch, narration,
    // doctor, validation — so merging at the lookup sites instead would mean
    // remembering to merge at each one, and the first site anyone forgets
    // silently sees a half-defined harness (the narrator lost `--model` that
    // way). One choke point, so a raw `harness.get(id)` is always effective.
    merge_built_in_harness_overrides(&mut runtime.harness);
    let foreign_config =
        foreign_config_path()?.filter(|path| !documents.iter().any(|(_, p, _)| p == path));
    if let Some(max_in_flight) = runtime.run.as_ref().and_then(|run| run.max_in_flight)
        && max_in_flight == 0
    {
        return Err(config_error("run.max-in-flight", "must be at least 1"));
    }
    if let Some(run) = runtime.run.as_ref() {
        validate_build_cache(&run.build_cache)?;
        // P428: fold declared named build caches into the one
        // `WorktreeConfig` every consumer of the effective overlay already
        // threads (run, drive/resume, merge dispatch — see
        // `resolve_effective_worktree_env`), so every entry point that
        // resolves `RuntimeConfig` (not only trait-run assignment
        // resolution) sees the same overlay.
        runtime.worktree.build_cache = run.build_cache.clone();
    }
    validate_merge_gate(&runtime.effective_merge_policy())?;
    validate_merge_retries(&runtime.effective_merge_policy())?;
    validate_merge_generated(&runtime.effective_merge_policy())?;
    for key in [
        "run.worktree",
        "run.max-frames",
        "run.frame-seconds",
        "run.total-seconds",
        "run.max-retries",
        "run.attach-wait-seconds",
        "run.idle-seconds",
        "run.max-in-flight",
        "run.wait",
        "run.strict-loops",
        "run.inline-prompt-bytes",
    ] {
        winners
            .entry(key.to_string())
            .or_insert_with(builtin_winner);
    }
    for key in [
        "merge.wait",
        "merge.overlap",
        "merge.branch",
        "merge.gate",
        "merge.gate-seconds",
        "merge.generated",
        "merge.disk-floor-mb",
        "worktree.setup-seconds",
        "worktree.setup-capture-bytes",
        "git.long-seconds",
        "publish.exclude",
        "registry.base",
    ] {
        winners
            .entry(key.to_string())
            .or_insert_with(builtin_winner);
    }
    Ok(ConfigReport {
        runtime,
        winners,
        tier_warnings,
        requirement_conflicts,
        foreign_config: foreign_config.map(|path| path.to_string()),
    })
}

fn repo_requirement(config: &RuntimeConfig, leaf: ConfigLeaf) -> bool {
    config
        .authored_requirements
        .get(&leaf)
        .is_some_and(|leaf| leaf.semantic == ConfigSemantic::Requirement)
}

type EffectiveRepoRequirements<'a> =
    BTreeMap<ConfigLeaf, (&'a Utf8PathBuf, &'a AuthoredConfigLeaf)>;

fn effective_repo_requirements(
    documents: &[(ConfigLayer, Utf8PathBuf, RuntimeConfig)],
) -> EffectiveRepoRequirements<'_> {
    let mut effective = BTreeMap::new();
    for (layer, path, document) in documents {
        if *layer == ConfigLayer::Repo {
            for (leaf, value) in &document.authored_requirements {
                if value.semantic == ConfigSemantic::Requirement {
                    effective.insert(*leaf, (path, value));
                }
            }
        }
    }
    effective
}

fn record_effective_repo_requirement_winners(
    effective: &EffectiveRepoRequirements<'_>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    for (leaf, (path, _)) in effective {
        record_winner(
            winners,
            leaf.path(),
            ConfigLayer::Repo,
            Some(path.to_string()),
        );
    }
}

fn apply_requirement_leaf(target: &mut RuntimeConfig, source: &RuntimeConfig, leaf: ConfigLeaf) {
    match leaf {
        ConfigLeaf::SchemaVersion => target.schema_version = source.schema_version.clone(),
        ConfigLeaf::WorktreeSetup => target.worktree.setup = source.worktree.setup.clone(),
        ConfigLeaf::WorktreeSetupSeconds => {
            target.worktree.setup_seconds = source.worktree.setup_seconds
        }
        ConfigLeaf::WorktreeSetupCaptureBytes => {
            target.worktree.setup_capture_bytes = source.worktree.setup_capture_bytes
        }
        ConfigLeaf::WorktreeConfinementEnabled => {
            target.worktree.confinement.enabled = source.worktree.confinement.enabled
        }
        ConfigLeaf::WorktreeConfinementSandbox => {
            target.worktree.confinement.sandbox = source.worktree.confinement.sandbox
        }
        ConfigLeaf::WorktreeConfinementAllow => {
            target.worktree.confinement.allow = source.worktree.confinement.allow.clone()
        }
        ConfigLeaf::WorktreeTripwirePolicy => {
            target.worktree.tripwire.policy = source.worktree.tripwire.policy
        }
        ConfigLeaf::WorktreeRetentionCheap => {
            target.worktree.retention.cheap = source.worktree.retention.cheap.clone()
        }
        ConfigLeaf::WorktreeRetentionExpensive => {
            target.worktree.retention.expensive = source.worktree.retention.expensive.clone()
        }
        ConfigLeaf::WorktreeRetentionExpensiveGraceDays => {
            target.worktree.retention.expensive_grace_days =
                source.worktree.retention.expensive_grace_days
        }
        ConfigLeaf::RunWorktree => {
            target.run.get_or_insert_with(RunTable::default).worktree =
                source.run.as_ref().and_then(|run| run.worktree)
        }
        ConfigLeaf::RunMaxFrames => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .max_frames = source.run.as_ref().and_then(|run| run.budget.max_frames)
        }
        ConfigLeaf::RunFrameSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .frame_seconds = source.run.as_ref().and_then(|run| run.budget.frame_seconds)
        }
        ConfigLeaf::RunTotalSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .total_seconds = source.run.as_ref().and_then(|run| run.budget.total_seconds)
        }
        ConfigLeaf::RunMaxRetries => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .max_retries = source.run.as_ref().and_then(|run| run.budget.max_retries)
        }
        ConfigLeaf::RunAttachWaitSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .attach_wait_seconds = source
                .run
                .as_ref()
                .and_then(|run| run.budget.attach_wait_seconds)
        }
        ConfigLeaf::RunIdleSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .idle_seconds = source.run.as_ref().and_then(|run| run.budget.idle_seconds)
        }
        ConfigLeaf::RunMaxInFlight => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .max_in_flight = source.run.as_ref().and_then(|run| run.max_in_flight)
        }
        ConfigLeaf::RunStrictLoops => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .strict_loops = source.run.as_ref().and_then(|run| run.strict_loops)
        }
        ConfigLeaf::RunInlinePromptBytes => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .inline_prompt_bytes = source.run.as_ref().and_then(|run| run.inline_prompt_bytes)
        }
        ConfigLeaf::RunCommandSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .command_seconds = source
                .run
                .as_ref()
                .and_then(|run| run.budget.command_seconds)
        }
        ConfigLeaf::RunCommandIdleSeconds => {
            target
                .run
                .get_or_insert_with(RunTable::default)
                .budget
                .command_idle_seconds = source
                .run
                .as_ref()
                .and_then(|run| run.budget.command_idle_seconds)
        }
        ConfigLeaf::MergeOverlap => {
            target.merge.get_or_insert_with(MergeTable::default).overlap =
                source.merge.as_ref().and_then(|merge| merge.overlap)
        }
        ConfigLeaf::MergeBranch => {
            target.merge.get_or_insert_with(MergeTable::default).branch =
                source.merge.as_ref().and_then(|merge| merge.branch.clone())
        }
        ConfigLeaf::MergeGate => {
            target.merge.get_or_insert_with(MergeTable::default).gate =
                source.merge.as_ref().and_then(|merge| merge.gate.clone())
        }
        ConfigLeaf::MergeGateSeconds => {
            target
                .merge
                .get_or_insert_with(MergeTable::default)
                .gate_seconds = source.merge.as_ref().and_then(|merge| merge.gate_seconds)
        }
        ConfigLeaf::MergeGenerated => {
            target
                .merge
                .get_or_insert_with(MergeTable::default)
                .generated = source
                .merge
                .as_ref()
                .and_then(|merge| merge.generated.clone())
        }
        ConfigLeaf::MergeDiskFloorMb => {
            target
                .merge
                .get_or_insert_with(MergeTable::default)
                .disk_floor_mb = source.merge.as_ref().and_then(|merge| merge.disk_floor_mb)
        }
        ConfigLeaf::MergeRetryAttempts => {
            target
                .merge
                .get_or_insert_with(MergeTable::default)
                .retry_attempts = source.merge.as_ref().and_then(|merge| merge.retry_attempts)
        }
        ConfigLeaf::MergeRetryBackoffMs => {
            target
                .merge
                .get_or_insert_with(MergeTable::default)
                .retry_backoff_ms = source
                .merge
                .as_ref()
                .and_then(|merge| merge.retry_backoff_ms)
        }
        ConfigLeaf::WorktreeSeed
        | ConfigLeaf::WorktreeWarm
        | ConfigLeaf::WorktreeEnv
        | ConfigLeaf::WorktreeTripwireSentinel
        | ConfigLeaf::RunWait
        | ConfigLeaf::RunBuildCache
        | ConfigLeaf::RunStory
        | ConfigLeaf::MergeWait
        | ConfigLeaf::MergeAuto
        | ConfigLeaf::MergeDeep
        | ConfigLeaf::GitLongSeconds
        | ConfigLeaf::PublishExclude
        | ConfigLeaf::RegistryBase
        | ConfigLeaf::TasksDispatchTrait
        | ConfigLeaf::TasksAutoClose
        | ConfigLeaf::HarnessDynamic
        | ConfigLeaf::AgentDynamic
        | ConfigLeaf::HostDynamic
        | ConfigLeaf::RepoDynamic
        | ConfigLeaf::TraitDynamic => {
            unreachable!("only requirement leaves are applied directly")
        }
    }
}

fn apply_environment_requirement_leaves(
    runtime: &mut RuntimeConfig,
    environment: &RuntimeConfig,
    effective: &EffectiveRepoRequirements<'_>,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    for (leaf, value) in &environment.authored_requirements {
        if value.semantic == ConfigSemantic::Requirement && !effective.contains_key(leaf) {
            apply_requirement_leaf(runtime, environment, *leaf);
            record_winner(winners, leaf.path(), layer, source.clone());
        }
    }
}

fn push_unique(values: &mut Vec<String>, additions: impl IntoIterator<Item = String>) {
    for value in additions {
        if !values.contains(&value) {
            values.push(value);
        }
    }
}

fn apply_repo_defaults(
    runtime: &mut RuntimeConfig,
    qualifier: &RepoOverride,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    // Keep the report's runtime document fully collapsed. The repo map remains
    // for qualifier evidence at dispatch time, but its matching personal
    // defaults must also be visible to non-dispatch consumers such as doctor.
    let role_keys: Vec<_> = qualifier.agent.role.iter().collect();
    let variant_role_keys = variant_role_assignments(&qualifier.agent.variant);
    let model_tier_present = !qualifier.agent.model_tier.is_empty();
    reconcile_assignment_winners(winners, &runtime.agent, &qualifier.agent, "agent");
    merge_agent_defaults(&mut runtime.agent, qualifier.agent.clone());
    for (role, assignment) in role_keys {
        record_personal_assignment_winners(
            winners,
            &format!("agent.role.{role}"),
            assignment,
            source.clone(),
        );
    }
    for (variant, role, assignment) in variant_role_keys {
        record_personal_assignment_winners(
            winners,
            &format!("agent.variant.{variant}.role.{role}"),
            assignment,
            source.clone(),
        );
    }
    if model_tier_present {
        record_personal_winner(winners, "agent.model-tier", source.clone());
    }
    for (id, harness) in &qualifier.harness {
        let merged = runtime
            .harness
            .get(id)
            .map(|base| harness.merged_onto(base))
            .unwrap_or_else(|| harness.clone());
        runtime.harness.insert(id.clone(), merged);
        record_harness_winners(
            winners,
            id,
            harness,
            layer,
            source.clone(),
            layer == ConfigLayer::UserGlobal,
        );
    }
    for (name, host) in &qualifier.host {
        runtime
            .host
            .entry(name.clone())
            .or_default()
            .merge(host.clone());
        record_host_winners(
            winners,
            name,
            host,
            layer,
            source.clone(),
            layer == ConfigLayer::UserGlobal,
        );
    }
    let run = runtime.run.get_or_insert_with(RunTable::default);
    if qualifier.run.wait.is_some() {
        run.wait = qualifier.run.wait;
        record_personal_winner(winners, "run.wait", source.clone());
    }
    if qualifier.run.story.is_some() {
        run.story = qualifier.run.story;
        record_personal_winner(winners, "run.story", source.clone());
    }
    let merge = runtime.merge.get_or_insert_with(MergeTable::default);
    if qualifier.merge.wait.is_some() {
        merge.wait = qualifier.merge.wait;
        record_personal_winner(winners, "merge.wait", source.clone());
    }
    if qualifier.merge.auto.is_some() {
        merge.auto = qualifier.merge.auto;
        record_personal_winner(winners, "merge.auto", source.clone());
    }
    if qualifier.merge.deep.is_some() {
        merge.deep = qualifier.merge.deep;
        record_personal_winner(winners, "merge.deep", source.clone());
    }
    if qualifier.git.long_seconds.is_some() {
        runtime
            .git
            .get_or_insert_with(GitTable::default)
            .long_seconds = qualifier.git.long_seconds;
        record_personal_winner(winners, "git.long-seconds", source.clone());
    }
    if qualifier.registry.base.is_some() {
        runtime
            .registry
            .get_or_insert_with(RegistryTable::default)
            .base = qualifier.registry.base.clone();
        record_personal_winner(winners, "registry.base", source);
    }
}

fn apply_environment_defaults(
    runtime: &mut RuntimeConfig,
    document: &RuntimeConfig,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    for (trait_id, defaults) in &document.trait_defaults {
        let target = runtime.trait_defaults.entry(trait_id.clone()).or_default();
        for (port, value) in &defaults.defaults.port {
            target.defaults.port.insert(port.clone(), value.clone());
            record_winner(
                winners,
                format!("trait.{trait_id}.defaults.port.{port}"),
                layer,
                source.clone(),
            );
        }
        merge_trait_agent_defaults(
            &mut runtime.trait_defaults,
            trait_id,
            &defaults.agent,
            &defaults.variant,
            layer,
            source.clone(),
            winners,
        );
    }
    let role_keys: Vec<_> = document.agent.role.iter().collect();
    let variant_role_keys = variant_role_assignments(&document.agent.variant);
    let model_tier_present = !document.agent.model_tier.is_empty();
    reconcile_assignment_winners(winners, &runtime.agent, &document.agent, "agent");
    merge_agent_defaults(&mut runtime.agent, document.agent.clone());
    for (role, assignment) in role_keys {
        record_assignment_winners(
            winners,
            &format!("agent.role.{role}"),
            assignment,
            layer,
            source.clone(),
        );
    }
    for (variant, role, assignment) in variant_role_keys {
        record_assignment_winners(
            winners,
            &format!("agent.variant.{variant}.role.{role}"),
            assignment,
            layer,
            source.clone(),
        );
    }
    if model_tier_present {
        record_winner(winners, "agent.model-tier", layer, source.clone());
    }
    for (id, harness) in &document.harness {
        let merged = runtime
            .harness
            .get(id)
            .map(|base| harness.merged_onto(base))
            .unwrap_or_else(|| harness.clone());
        runtime.harness.insert(id.clone(), merged);
        record_harness_winners(winners, id, harness, layer, source.clone(), false);
    }
    for (name, host) in &document.host {
        runtime
            .host
            .entry(name.clone())
            .or_default()
            .merge(host.clone());
        record_host_winners(winners, name, host, layer, source.clone(), false);
    }
    if let Some(next) = &document.run {
        let run = runtime.run.get_or_insert_with(RunTable::default);
        if next.wait.is_some() {
            run.wait = next.wait;
            record_winner(winners, "run.wait", layer, source.clone());
        }
        if next.story.is_some() {
            run.story = next.story;
            record_winner(winners, "run.story", layer, source.clone());
        }
    }
    if let Some(next) = &document.merge {
        let merge = runtime.merge.get_or_insert_with(MergeTable::default);
        if next.wait.is_some() {
            merge.wait = next.wait;
            record_winner(winners, "merge.wait", layer, source.clone());
        }
        if next.auto.is_some() {
            merge.auto = next.auto;
            record_winner(winners, "merge.auto", layer, source.clone());
        }
        if next.deep.is_some() {
            merge.deep = next.deep;
            record_winner(winners, "merge.deep", layer, source.clone());
        }
    }
    if let Some(next) = &document.git
        && next.long_seconds.is_some()
    {
        runtime
            .git
            .get_or_insert_with(GitTable::default)
            .long_seconds = next.long_seconds;
        record_winner(winners, "git.long-seconds", layer, source.clone());
    }
    if let Some(next) = &document.registry
        && next.base.is_some()
    {
        runtime
            .registry
            .get_or_insert_with(RegistryTable::default)
            .base = next.base.clone();
        record_winner(winners, "registry.base", layer, source.clone());
    }
    if let Some(next) = &document.tasks
        && next.dispatch_trait.is_some()
    {
        runtime
            .tasks
            .get_or_insert_with(TasksTable::default)
            .dispatch_trait = next.dispatch_trait.clone();
        record_winner(winners, "tasks.dispatch-trait", layer, source.clone());
    }
    if let Some(next) = &document.tasks
        && next.auto_close.is_some()
    {
        runtime
            .tasks
            .get_or_insert_with(TasksTable::default)
            .auto_close = next.auto_close;
        record_winner(winners, "tasks.auto-close", layer, source);
    }
}

fn apply_additive_values(
    runtime: &mut RuntimeConfig,
    documents: &[(ConfigLayer, Utf8PathBuf, RuntimeConfig)],
    personal: &[(&Utf8PathBuf, &RepoOverride)],
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    runtime.worktree.seed.clear();
    runtime.worktree.warm.clear();
    runtime.worktree.env.clear();
    runtime.worktree.tripwire.sentinel.clear();
    if let Some(run) = runtime.run.as_mut() {
        run.build_cache.clear();
    }
    if let Some(publish) = runtime.publish.as_mut() {
        publish.exclude = None;
    }
    // Product exclusions are the first additive contribution, not a fallback
    // that disappears as soon as an author adds one exclusion.
    let publish = runtime.publish.get_or_insert_with(PublishTable::default);
    publish.exclude = Some(
        crate::publish::PACK_DEFAULT_EXCLUDES
            .iter()
            .map(|entry| (*entry).to_string())
            .collect(),
    );
    let mut contributors: BTreeMap<String, Vec<ConfigContributor>> = BTreeMap::new();
    let mut effective: BTreeMap<String, ConfigContributor> = BTreeMap::new();
    contributors.insert(
        "publish.exclude".into(),
        vec![ConfigContributor {
            layer: ConfigLayer::BuiltIn,
            source: None,
        }],
    );
    for (layer, path, document) in documents {
        push_unique(&mut runtime.worktree.seed, document.worktree.seed.clone());
        push_unique(&mut runtime.worktree.warm, document.worktree.warm.clone());
        push_unique(
            &mut runtime.worktree.tripwire.sentinel,
            document.worktree.tripwire.sentinel.clone(),
        );
        for (key, value) in &document.worktree.env {
            let contributor = ConfigContributor {
                layer: *layer,
                source: Some(path.to_string()),
            };
            record_additive_contributor(
                &mut contributors,
                format!("worktree.env.{key}"),
                contributor.clone(),
            );
            // Repository-owned map entries are retained over personal/env.
            if *layer == ConfigLayer::Repo || !runtime.worktree.env.contains_key(key) {
                runtime.worktree.env.insert(key.clone(), value.clone());
                effective.insert(format!("worktree.env.{key}"), contributor);
            }
        }
        if let Some(run) = &document.run {
            let target = runtime.run.get_or_insert_with(RunTable::default);
            for (name, cache) in &run.build_cache {
                let contributor = ConfigContributor {
                    layer: *layer,
                    source: Some(path.to_string()),
                };
                record_additive_contributor(
                    &mut contributors,
                    format!("run.build-cache.{name}"),
                    contributor.clone(),
                );
                if *layer == ConfigLayer::Repo || !target.build_cache.contains_key(name) {
                    target.build_cache.insert(name.clone(), cache.clone());
                    effective.insert(format!("run.build-cache.{name}"), contributor);
                }
            }
        }
        if let Some(publish) = &document.publish
            && let Some(exclude) = &publish.exclude
        {
            let target = runtime.publish.get_or_insert_with(PublishTable::default);
            push_unique(target.exclude.get_or_insert_with(Vec::new), exclude.clone());
        }
        let contributor = || ConfigContributor {
            layer: *layer,
            source: Some(path.to_string()),
        };
        if !document.worktree.seed.is_empty() {
            record_additive_contributor(&mut contributors, "worktree.seed".into(), contributor());
        }
        if !document.worktree.warm.is_empty() {
            record_additive_contributor(&mut contributors, "worktree.warm".into(), contributor());
        }
        if !document.worktree.tripwire.sentinel.is_empty() {
            record_additive_contributor(
                &mut contributors,
                "worktree.tripwire.sentinel".into(),
                contributor(),
            );
        }
        if document
            .publish
            .as_ref()
            .and_then(|publish| publish.exclude.as_ref())
            .is_some()
        {
            record_additive_contributor(&mut contributors, "publish.exclude".into(), contributor());
        }
    }
    for (path, override_) in personal {
        push_unique(&mut runtime.worktree.seed, override_.worktree.seed.clone());
        push_unique(&mut runtime.worktree.warm, override_.worktree.warm.clone());
        push_unique(
            &mut runtime.worktree.tripwire.sentinel,
            override_.worktree.tripwire.sentinel.clone(),
        );
        for (key, value) in &override_.worktree.env {
            let contributor = ConfigContributor {
                layer: ConfigLayer::UserGlobal,
                source: Some(path.to_string()),
            };
            record_additive_contributor(
                &mut contributors,
                format!("worktree.env.{key}"),
                contributor.clone(),
            );
            let inserted = !runtime.worktree.env.contains_key(key);
            runtime
                .worktree
                .env
                .entry(key.clone())
                .or_insert_with(|| value.clone());
            if inserted {
                effective.insert(format!("worktree.env.{key}"), contributor);
            }
        }
        let run = runtime.run.get_or_insert_with(RunTable::default);
        for (name, cache) in &override_.run.build_cache {
            let contributor = ConfigContributor {
                layer: ConfigLayer::UserGlobal,
                source: Some(path.to_string()),
            };
            record_additive_contributor(
                &mut contributors,
                format!("run.build-cache.{name}"),
                contributor.clone(),
            );
            let inserted = !run.build_cache.contains_key(name);
            run.build_cache
                .entry(name.clone())
                .or_insert_with(|| cache.clone());
            if inserted {
                effective.insert(format!("run.build-cache.{name}"), contributor);
            }
        }
        if !override_.publish.exclude.is_empty() {
            let publish = runtime.publish.get_or_insert_with(PublishTable::default);
            let values = publish.exclude.get_or_insert_with(Vec::new);
            push_unique(values, override_.publish.exclude.clone());
        }
        let contributor = || ConfigContributor {
            layer: ConfigLayer::UserGlobal,
            source: Some(path.to_string()),
        };
        if !override_.worktree.seed.is_empty() {
            record_additive_contributor(&mut contributors, "worktree.seed".into(), contributor());
        }
        if !override_.worktree.warm.is_empty() {
            record_additive_contributor(&mut contributors, "worktree.warm".into(), contributor());
        }
        if !override_.worktree.tripwire.sentinel.is_empty() {
            record_additive_contributor(
                &mut contributors,
                "worktree.tripwire.sentinel".into(),
                contributor(),
            );
        }
        if !override_.publish.exclude.is_empty() {
            record_additive_contributor(&mut contributors, "publish.exclude".into(), contributor());
        }
    }
    for (key, sources) in contributors {
        let last = effective.remove(&key).unwrap_or_else(|| {
            sources
                .last()
                .cloned()
                .expect("additive contributor exists")
        });
        winners.insert(
            key,
            ConfigWinner {
                layer: last.layer,
                source: last.source.clone(),
                reason: ConfigReason::Additive,
                contributors: sources,
            },
        );
    }
}

/// A matching personal block can come from a document already traversed above.
/// It contributes values at its merge position, but must appear once per leaf.
fn record_additive_contributor(
    contributors: &mut BTreeMap<String, Vec<ConfigContributor>>,
    key: String,
    contributor: ConfigContributor,
) {
    let sources = contributors.entry(key).or_default();
    if !sources.contains(&contributor) {
        sources.push(contributor);
    }
}

#[cfg(test)]
fn requirement_conflicts(
    documents: &[(ConfigLayer, Utf8PathBuf, RuntimeConfig)],
) -> Vec<ConfigRequirementConflict> {
    requirement_conflicts_for_effective(documents, &effective_repo_requirements(documents))
}

fn requirement_conflicts_for_effective(
    documents: &[(ConfigLayer, Utf8PathBuf, RuntimeConfig)],
    effective: &EffectiveRepoRequirements<'_>,
) -> Vec<ConfigRequirementConflict> {
    let mut output = Vec::new();
    for (layer, path, candidate) in documents
        .iter()
        .filter(|(layer, _, _)| *layer == ConfigLayer::Environment)
    {
        for (leaf, rejected) in &candidate.authored_requirements {
            if let Some((repo_path, required)) = effective.get(leaf)
                && rejected.semantic == ConfigSemantic::Requirement
                && rejected.value != required.value
            {
                output.push(ConfigRequirementConflict {
                    field: leaf.path().to_string(),
                    rejected_source: path.to_string(),
                    repo_source: repo_path.to_string(),
                });
            }
        }
        let _ = layer;
    }
    // Additive maps merge distinct entries, but a repository-owned entry is a
    // per-key requirement. Track it separately from document requirements so
    // rejected personal values are visible instead of silently ignored.
    let mut repo_env = BTreeMap::new();
    let mut repo_caches = BTreeMap::new();
    for (layer, path, document) in documents {
        if *layer != ConfigLayer::Repo {
            continue;
        }
        for (key, value) in &document.worktree.env {
            repo_env.insert(key.as_str(), (path, value));
        }
        if let Some(run) = &document.run {
            for (key, value) in &run.build_cache {
                repo_caches.insert(key.as_str(), (path, value));
            }
        }
    }
    let mut check_maps = |path: &Utf8Path,
                          env: &BTreeMap<String, String>,
                          caches: &BTreeMap<String, BuildCacheConfig>| {
        for (key, value) in env {
            if let Some((repo_path, required)) = repo_env.get(key.as_str())
                && *required != value
            {
                output.push(ConfigRequirementConflict {
                    field: format!("worktree.env.{key}"),
                    rejected_source: path.to_string(),
                    repo_source: repo_path.to_string(),
                });
            }
        }
        for (key, value) in caches {
            if let Some((repo_path, required)) = repo_caches.get(key.as_str())
                && *required != value
            {
                output.push(ConfigRequirementConflict {
                    field: format!("run.build-cache.{key}"),
                    rejected_source: path.to_string(),
                    repo_source: repo_path.to_string(),
                });
            }
        }
    };
    for (layer, path, document) in documents {
        if *layer == ConfigLayer::Environment {
            if let Some(run) = &document.run {
                check_maps(path, &document.worktree.env, &run.build_cache);
            } else {
                check_maps(path, &document.worktree.env, &BTreeMap::new());
            }
        }
    }
    if let Some(key) = active_repo_qualifier_key() {
        for (layer, path, document) in documents {
            if *layer == ConfigLayer::UserGlobal
                && let Some(personal) = document.repo.get(&key)
            {
                check_maps(path, &personal.worktree.env, &personal.run.build_cache);
            }
        }
    }
    output
}

fn has_tier_declaration(agent: &AgentDefaults) -> bool {
    !agent.model_tier.is_empty()
        || agent.role.values().any(|value| {
            value
                .entries()
                .iter()
                .any(|assignment| assignment.model_tier.is_some())
        })
}

/// Pre-P476 `[agent]` keys that now decode-fail under `deny_unknown_fields`,
/// mapped to the exact replacement `[agent.role.*]` location named in the
/// clean-break migration error (§3.3 of the P476 draft). `master` alone
/// renames to `default`; every other legacy table/scalar key keeps its name
/// under `[agent.role.default]`.
const LEGACY_AGENT_TABLE_KEYS: &[&str] = &["master", "narrator", "merger", "merger-deep"];
const LEGACY_AGENT_SCALAR_KEYS: &[&str] = &[
    "harness",
    "transport",
    "session-mode",
    "model",
    "reasoning-effort",
    "system-prompt",
    "extra-args",
];

/// The single statement of the pre-P476 `master` → `default` seat rename —
/// consumed by both [`legacy_agent_key_error`] and the `--migrate-config`
/// planner ([`plan_agent_config_rewrite`]) so the two can never disagree
/// about where a legacy table lands.
fn legacy_agent_role_name(key: &str) -> &str {
    if key == "master" { "default" } else { key }
}

/// On a decode failure, re-parse `text` as a generic TOML table and check
/// whether its `[agent]` table carries a pre-P476 legacy key — if so, return
/// a message naming that key's exact new home instead of serde's generic
/// "unknown field" text. `None` when the failure is unrelated to a legacy
/// agent key (the original decode error is used instead) or `text` itself
/// does not parse as TOML at all.
fn legacy_agent_key_error(text: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let agent = table.get("agent")?.as_table()?;
    for key in LEGACY_AGENT_TABLE_KEYS {
        if agent.contains_key(*key) {
            let new_role = legacy_agent_role_name(key);
            return Some(format!(
                "[agent.{key}] moved: declare [agent.role.{new_role}]; run `ctx traits doctor --migrate-config` for the full rewrite"
            ));
        }
    }
    for key in LEGACY_AGENT_SCALAR_KEYS {
        if agent.contains_key(*key) {
            return Some(format!(
                "[agent] {key} = ... moved: declare [agent.role.default].{key}; run `ctx traits doctor --migrate-config` for the full rewrite"
            ));
        }
    }
    None
}

pub fn load_runtime_config(path: &Utf8Path) -> crate::Result<RuntimeConfig> {
    let text = crate::read::read_text(path)?;
    if crate::config_source::is_generated_config_candidate(path) {
        crate::config_source::guard_config_toml(path, &text)?;
    }
    let mut config: RuntimeConfig = toml::from_str(&text).map_err(|source| {
        if let Some(message) = legacy_agent_key_error(&text) {
            return config_error("agent", message);
        }
        crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        }
        .into()
    })?;
    let document: toml::Value = toml::from_str(&text).expect("decoded runtime TOML is valid");
    config.authored_requirements = authored_requirement_values(&document);
    Ok(config)
}

fn authored_requirement_values(document: &toml::Value) -> BTreeMap<ConfigLeaf, AuthoredConfigLeaf> {
    ConfigLeaf::ALL
        .iter()
        .filter_map(|leaf| {
            toml_value_at(document, leaf.path()).cloned().map(|value| {
                (
                    *leaf,
                    AuthoredConfigLeaf {
                        semantic: leaf.semantic(),
                        value,
                    },
                )
            })
        })
        .collect()
}

/// The authoritative semantic classifier for concrete authored paths. Dynamic
/// map entries are classified by their enclosing table at resolution time.
fn config_semantic(field: &str) -> ConfigSemantic {
    ConfigLeaf::ALL
        .iter()
        .find_map(|leaf| (leaf.path() == field).then_some(leaf.semantic()))
        .or_else(|| {
            ["worktree.env.", "run.build-cache."]
                .iter()
                .any(|prefix| field.starts_with(prefix))
                .then_some(ConfigSemantic::Additive)
        })
        .unwrap_or(ConfigSemantic::Default)
}

fn toml_value_at<'a>(value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    path.split('.')
        .try_fold(value, |value, segment| value.get(segment))
}

/// The resolved config-layer paths runtime config resolution itself reads
/// (user-global config, every ancestor `.ctx/config.toml`/`.ctx/harness.toml`
/// up to the repo root, `$CTX_CONFIG` — see [`crate::env_reference::env_reference`]
/// for `CTX_CONFIG`'s full contract) — exposed so P479's tripwire default
/// sentinel set is *the same* enumeration config resolution uses, never a
/// second hand-listed copy. Existence is not checked here; a caller that
/// digests these paths treats a missing one as "absent", not an error.
pub fn runtime_config_layer_paths(start_dir: &Utf8Path) -> crate::Result<Vec<Utf8PathBuf>> {
    Ok(runtime_config_layers(start_dir)?
        .into_iter()
        .map(|(_, path)| path)
        .collect())
}

fn runtime_config_layers(start_dir: &Utf8Path) -> crate::Result<Vec<(ConfigLayer, Utf8PathBuf)>> {
    let mut layers = Vec::new();
    let globals = global_runtime_config_paths()?;
    for path in globals {
        layers.push((ConfigLayer::UserGlobal, path));
    }
    let cwd = absolute_utf8_path(start_dir, "runtime.config.cwd")?;
    let repo_root = crate::repository::discover_repo_root().ok();
    let mut ancestors = Vec::new();
    for ancestor in cwd.ancestors() {
        ancestors.push(ancestor);
        match &repo_root {
            Some(root) if ancestor != root => continue,
            _ => break,
        }
    }
    ancestors.reverse();
    for ancestor in ancestors {
        layers.push((ConfigLayer::Repo, ancestor.join(LEGACY_HARNESS_REGISTRY)));
        layers.push((ConfigLayer::Repo, ancestor.join(LEGACY_RUNTIME_CONFIG)));
        // P569: `.ctx/config.toml` before `.ctx/traits/runtime.toml`, so a
        // checkout carrying both is governed by the new name.
        layers.push((ConfigLayer::Repo, ancestor.join(LEGACY_CTX_RUNTIME_CONFIG)));
        layers.push((ConfigLayer::Repo, ancestor.join(HARNESS_REGISTRY)));
        // 0037: the committed project tier (`.ctx/traits/config.toml`) merges
        // BEFORE the machine-local `.ctx/traits/runtime.toml`, so a local
        // field overrides the project decision field-wise and nothing else —
        // built-in < package < machine ~/.config < config.toml < runtime.toml.
        layers.push((ConfigLayer::Repo, ancestor.join(PROJECT_CONFIG)));
        layers.push((ConfigLayer::Repo, ancestor.join(RUNTIME_CONFIG)));
    }
    if let Ok(path) = std::env::var("CTX_CONFIG") {
        layers.push((ConfigLayer::Environment, Utf8PathBuf::from(path)));
    }
    Ok(layers)
}

// ---------------------------------------------------------------------------
// `ctx traits doctor --migrate-config [--apply]` (P514)
// ---------------------------------------------------------------------------

/// One legacy `[agent]` key this migration would rewrite, or already
/// rewrote: its old dotted location and its new `[agent.role.*]` home.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentConfigRewrite {
    pub from: String,
    pub to: String,
}

/// A legacy key whose destination is already occupied by a distinct,
/// independently-declared value — two operator intentions that only a human
/// can collapse, so this migration never merges or overwrites it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentConfigConflict {
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// The per-layer result of [`plan_agent_config_rewrite`]: the rewrites and
/// conflicts found, plus the fully rewritten text (identical to the input
/// when there is nothing to rewrite) — computed in one pass so a report and
/// its eventual `--apply` write can never disagree with each other. `refusal`
/// is set instead of `rewrites`/`conflicts` when the render-verification
/// pass (see [`plan_agent_config_rewrite`]) finds that the rendered text
/// does not actually carry what the plan believed it moved — in that case
/// `rewritten_text` is the original, untouched `text`.
struct AgentConfigRewriteResult {
    rewrites: Vec<AgentConfigRewrite>,
    conflicts: Vec<AgentConfigConflict>,
    rewritten_text: String,
    refusal: Option<String>,
}

/// Whether `dotted_path` (e.g. `"agent.role.default"`) resolves to a present
/// value in `rendered` when parsed as ordinary TOML. This is the ground
/// truth [`plan_agent_config_rewrite`] checks its own rendered output
/// against: the in-memory `toml_edit` tree it built can disagree with what
/// `DocumentMut::to_string()` actually serializes (a standard `Table`
/// inserted under an inline-table ancestor renders as though it were never
/// inserted at all, with no parse error to signal the loss).
fn rendered_path_exists(rendered: &str, dotted_path: &str) -> bool {
    let Ok(value) = toml::from_str::<toml::Value>(rendered) else {
        return false;
    };
    let mut current = &value;
    for segment in dotted_path.split('.') {
        match current.get(segment) {
            Some(next) => current = next,
            None => return false,
        }
    }
    true
}

/// Create `parent[key]` as an empty table when absent, marked implicit so it
/// never renders its own `[a.b]` header line — it exists only as scaffolding
/// for the dotted subtables a caller assigns into it next (mirroring how
/// `toml_edit` itself treats a table that is only ever referenced through
/// its children, e.g. `[agent.role.default]` implying `agent.role`).
/// Idempotent: a pre-existing table (implicit or not) is left as-is.
fn ensure_implicit_table(parent: &mut toml_edit::Item, key: &str) {
    if parent.get(key).is_none() {
        let mut table = toml_edit::table();
        if let Some(table) = table.as_table_mut() {
            table.set_implicit(true);
        }
        parent[key] = table;
    }
}

/// Pure prepare: parse `text` as TOML (via `toml_edit`, so every comment,
/// blank line, and key order round-trips) and, for each pre-P476 legacy
/// `[agent]` key present, either move it to its `[agent.role.*]` destination
/// or record a conflict when that destination is already occupied. Never
/// writes anything — the caller decides whether `rewritten_text` is worth
/// persisting. Reuses [`LEGACY_AGENT_TABLE_KEYS`], [`LEGACY_AGENT_SCALAR_KEYS`],
/// and [`legacy_agent_role_name`] — the same vocabulary [`legacy_agent_key_error`]
/// uses — so a fresh, drifting copy of "what counts as legacy" can never
/// exist here.
fn plan_agent_config_rewrite(text: &str) -> crate::Result<AgentConfigRewriteResult> {
    let mut document: toml_edit::DocumentMut =
        text.parse()
            .map_err(|source| crate::parse::Error::TomlEditDecode {
                context: "parse config for agent migration".to_string(),
                source: Box::new(source),
            })?;
    let mut rewrites = Vec::new();
    let mut conflicts = Vec::new();

    if document
        .get("agent")
        .and_then(|item| item.as_table_like())
        .is_none()
    {
        return Ok(AgentConfigRewriteResult {
            rewrites,
            conflicts,
            rewritten_text: text.to_string(),
            refusal: None,
        });
    }

    for key in LEGACY_AGENT_TABLE_KEYS {
        let present = document["agent"]
            .as_table_like()
            .is_some_and(|agent| agent.contains_key(key));
        if !present {
            continue;
        }
        let new_role = legacy_agent_role_name(key);
        let from = format!("agent.{key}");
        let to = format!("agent.role.{new_role}");
        let occupied = document["agent"]
            .as_table_like()
            .and_then(|agent| agent.get("role"))
            .and_then(|role| role.as_table_like())
            .is_some_and(|role| role.contains_key(new_role));
        if occupied {
            conflicts.push(AgentConfigConflict {
                from,
                to: to.clone(),
                reason: format!("[{to}] already exists"),
            });
            continue;
        }
        let value = document["agent"]
            .as_table_like_mut()
            .and_then(|agent| agent.remove(key))
            .expect("presence checked above");
        ensure_implicit_table(&mut document["agent"], "role");
        document["agent"]["role"][new_role] = value;
        rewrites.push(AgentConfigRewrite { from, to });
    }

    for key in LEGACY_AGENT_SCALAR_KEYS {
        let present = document["agent"]
            .as_table_like()
            .is_some_and(|agent| agent.contains_key(key));
        if !present {
            continue;
        }
        let from = format!("agent.{key}");
        let to = format!("agent.role.default.{key}");
        let occupied = document["agent"]
            .as_table_like()
            .and_then(|agent| agent.get("role"))
            .and_then(|role| role.as_table_like())
            .and_then(|role| role.get("default"))
            .and_then(|default| default.as_table_like())
            .is_some_and(|default| default.contains_key(key));
        if occupied {
            conflicts.push(AgentConfigConflict {
                from,
                to: to.clone(),
                reason: format!("[{to}] already set"),
            });
            continue;
        }
        let value = document["agent"]
            .as_table_like_mut()
            .and_then(|agent| agent.remove(key))
            .expect("presence checked above");
        ensure_implicit_table(&mut document["agent"], "role");
        ensure_implicit_table(&mut document["agent"]["role"], "default");
        document["agent"]["role"]["default"][key] = value;
        rewrites.push(AgentConfigRewrite { from, to });
    }

    let rewritten_text = document.to_string();

    // Verify the render actually carries what `rewrites` claims to have
    // moved before ever reporting or writing it: for every recorded
    // rewrite, its destination must resolve in the rendered text and its
    // source key must be gone. A mismatch means `to_string()` silently
    // dropped part of the tree (the inline-table-ancestor case above) —
    // report no rewrite for this layer rather than a rewrite that was
    // never actually performed, and leave the text untouched.
    let verified = rewrites.iter().all(|rewrite| {
        rendered_path_exists(&rewritten_text, &rewrite.to)
            && !rendered_path_exists(&rewritten_text, &rewrite.from)
    });
    if !verified {
        return Ok(AgentConfigRewriteResult {
            rewrites: Vec::new(),
            conflicts: Vec::new(),
            rewritten_text: text.to_string(),
            refusal: Some(
                "this layer's [agent] table could not be rewritten without data loss (its \
                 rendered text does not survive a written round trip) — migrate it by hand"
                    .to_string(),
            ),
        });
    }

    Ok(AgentConfigRewriteResult {
        rewrites,
        conflicts,
        rewritten_text,
        refusal: None,
    })
}

/// One config layer's migration plan: which legacy `[agent]` keys it
/// carries, which are safe to rewrite, which conflict, and — for a
/// P457-generated `config.toml`, or a layer whose rendered rewrite failed
/// [`plan_agent_config_rewrite`]'s own round-trip verification — the
/// `refusal` naming why nothing here is safe to write.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentConfigLayerPlan {
    pub layer: ConfigLayer,
    pub path: String,
    pub rewrites: Vec<AgentConfigRewrite>,
    pub conflicts: Vec<AgentConfigConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// Scan every existing runtime-config layer ([`runtime_config_layers`] — the
/// same enumeration config resolution itself reads, never a second hand-listed
/// copy) for pre-P476 legacy `[agent]` keys, and return one plan entry per
/// layer that actually carries at least one rewrite, conflict, or refusal. A
/// layer with nothing to report is omitted entirely — an empty return means
/// "no legacy `[agent]` keys found" anywhere.
pub fn plan_agent_config_migration(
    start_dir: &Utf8Path,
) -> crate::Result<Vec<AgentConfigLayerPlan>> {
    let mut plans = Vec::new();
    for (layer, path) in runtime_config_layers(start_dir)? {
        if !path.exists() {
            continue;
        }
        let text = crate::read::read_text(&path)?;
        if crate::config_source::is_generated_config_candidate(&path) {
            let source_path = crate::config_source::sibling_source_path(&path);
            if source_path.exists() {
                let result = plan_agent_config_rewrite(&text)?;
                let carries_legacy_keys = !result.rewrites.is_empty()
                    || !result.conflicts.is_empty()
                    || result.refusal.is_some();
                if !carries_legacy_keys {
                    continue;
                }
                plans.push(AgentConfigLayerPlan {
                    layer,
                    refusal: Some(format!(
                        "{path} is generated from {source_path} — migrate {source_path} and re-run `ctx traits config build`"
                    )),
                    path: path.to_string(),
                    rewrites: Vec::new(),
                    conflicts: Vec::new(),
                });
                continue;
            }
        }
        let result = plan_agent_config_rewrite(&text)?;
        if result.rewrites.is_empty() && result.conflicts.is_empty() && result.refusal.is_none() {
            continue;
        }
        plans.push(AgentConfigLayerPlan {
            layer,
            path: path.to_string(),
            rewrites: result.rewrites,
            conflicts: result.conflicts,
            refusal: result.refusal,
        });
    }
    Ok(plans)
}

/// One layer [`apply_agent_config_migration`] successfully rewrote.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AppliedAgentConfigLayer {
    pub path: String,
    pub rewrites: Vec<AgentConfigRewrite>,
}

/// One layer [`apply_agent_config_migration`] tried and failed to rewrite;
/// its source file is left untouched (the write is atomic — see
/// [`crate::write::write_bytes_atomically`]).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AppliedAgentConfigFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AppliedAgentConfigMigration {
    pub rewritten: Vec<AppliedAgentConfigLayer>,
    pub failed: Vec<AppliedAgentConfigFailure>,
}

/// Perform the non-conflicting, non-refused rewrites from `plans`. Re-reads
/// and re-plans each layer from its current on-disk text rather than
/// trusting anything cached from the planning pass, then writes the result
/// atomically ([`crate::write::write_bytes_atomically`]) — so a plan-to-apply
/// race or a plan built from stale text can never produce a half-applied
/// write. A conflicting or refused entry is left untouched here exactly as
/// it was left in the plan; a write failure, or a refusal newly discovered
/// by the fresh re-plan (e.g. the file changed between plan and apply), is
/// reported as a per-entry failure, source data intact.
pub fn apply_agent_config_migration(plans: &[AgentConfigLayerPlan]) -> AppliedAgentConfigMigration {
    let mut rewritten = Vec::new();
    let mut failed = Vec::new();
    for plan in plans {
        if plan.rewrites.is_empty() || plan.refusal.is_some() {
            continue;
        }
        let path = Utf8Path::new(&plan.path);
        let outcome: crate::Result<()> = (|| {
            let text = crate::read::read_text(path)?;
            let result = plan_agent_config_rewrite(&text)?;
            if let Some(refusal) = result.refusal {
                return Err(config_error("agent", refusal));
            }
            crate::write::write_bytes_atomically(path, result.rewritten_text.as_bytes())
        })();
        match outcome {
            Ok(()) => rewritten.push(AppliedAgentConfigLayer {
                path: plan.path.clone(),
                rewrites: plan.rewrites.clone(),
            }),
            Err(err) => failed.push(AppliedAgentConfigFailure {
                path: plan.path.clone(),
                error: err.to_string(),
            }),
        }
    }
    AppliedAgentConfigMigration { rewritten, failed }
}

fn foreign_config_path() -> crate::Result<Option<Utf8PathBuf>> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home else { return Ok(None) };
    let path = Utf8PathBuf::from_path_buf(std::path::PathBuf::from(home).join(".ctx/config.toml"))
        .map_err(|path| {
            config_error(
                "foreign-config",
                format!("path is not UTF-8: {}", path.display()),
            )
        })?;
    Ok(path.exists().then_some(path))
}

/// Global runtime config candidates, legacy first so the current path wins
/// the merge in [`resolve_runtime_config`]. Both live directly under the
/// `ctx` config-home directory (never under a nested `.ctx/`, which is a
/// project-relative concept — see [`GLOBAL_RUNTIME_CONFIG`]).
fn global_runtime_config_paths() -> crate::Result<Vec<Utf8PathBuf>> {
    // Absence of `HOME`/`XDG_CONFIG_HOME` is not fatal here (unlike other
    // global-state consumers): a global runtime config is optional, so a
    // missing config home just means "no global config layer" rather than
    // an error.
    let Ok(ctx_dir) = crate::state::global_ctx_root() else {
        return Ok(Vec::new());
    };
    Ok(vec![
        ctx_dir.join(LEGACY_GLOBAL_RUNTIME_CONFIG),
        ctx_dir.join(LEGACY_CTX_GLOBAL_RUNTIME_CONFIG),
        ctx_dir.join(GLOBAL_RUNTIME_CONFIG),
    ])
}

fn absolute_utf8_path(path: &Utf8Path, field_path: &str) -> crate::Result<Utf8PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|source| crate::environment::Error::Filesystem {
        path: ".".to_string(),
        source,
    })?;
    utf8_path(cwd.join(path.as_std_path()), field_path)
}

fn utf8_path(path: PathBuf, field_path: &str) -> crate::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| {
        config_error(
            field_path,
            format!("path is not valid UTF-8: {}", path.display()),
        )
    })
}

fn record_winner(
    winners: &mut BTreeMap<String, ConfigWinner>,
    key: impl Into<String>,
    layer: ConfigLayer,
    source: Option<String>,
) {
    let key = key.into();
    let reason = match (config_semantic(&key), layer) {
        (ConfigSemantic::Additive, _) => ConfigReason::Additive,
        (ConfigSemantic::Requirement, ConfigLayer::Repo) => ConfigReason::RepoRequirement,
        (_, ConfigLayer::Repo) => ConfigReason::RepoDefault,
        (_, ConfigLayer::Environment) => ConfigReason::EnvironmentOverride,
        _ => ConfigReason::Default,
    };
    winners.insert(
        key,
        ConfigWinner {
            layer,
            source,
            reason,
            contributors: Vec::new(),
        },
    );
}

fn record_personal_winner(
    winners: &mut BTreeMap<String, ConfigWinner>,
    key: impl Into<String>,
    source: Option<String>,
) {
    winners.insert(
        key.into(),
        ConfigWinner {
            layer: ConfigLayer::UserGlobal,
            source,
            reason: ConfigReason::PersonalRepoOverride,
            contributors: Vec::new(),
        },
    );
}

fn remove_winner_subtree(winners: &mut BTreeMap<String, ConfigWinner>, prefix: &str) {
    let child_prefix = format!("{prefix}.");
    winners.retain(|key, _| key != prefix && !key.starts_with(&child_prefix));
}

fn builtin_winner() -> ConfigWinner {
    ConfigWinner {
        layer: ConfigLayer::BuiltIn,
        source: None,
        reason: ConfigReason::Default,
        contributors: Vec::new(),
    }
}

fn merge_project_config(
    base: &mut RuntimeConfig,
    next: RuntimeConfig,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    for (trait_id, defaults) in &next.trait_defaults {
        let target = base.trait_defaults.entry(trait_id.clone()).or_default();
        for (port, value) in &defaults.defaults.port {
            target.defaults.port.insert(port.clone(), value.clone());
            record_winner(
                winners,
                format!("trait.{trait_id}.defaults.port.{port}"),
                layer,
                source.clone(),
            );
        }
        merge_trait_agent_defaults(
            &mut base.trait_defaults,
            trait_id,
            &defaults.agent,
            &defaults.variant,
            layer,
            source.clone(),
            winners,
        );
    }
    let setup_declared = repo_requirement(&next, ConfigLeaf::WorktreeSetup);
    let confinement_enabled = repo_requirement(&next, ConfigLeaf::WorktreeConfinementEnabled);
    let confinement_sandbox = repo_requirement(&next, ConfigLeaf::WorktreeConfinementSandbox);
    let confinement_allow = repo_requirement(&next, ConfigLeaf::WorktreeConfinementAllow);
    let tripwire_policy = repo_requirement(&next, ConfigLeaf::WorktreeTripwirePolicy);
    let retention_cheap = repo_requirement(&next, ConfigLeaf::WorktreeRetentionCheap);
    let retention_expensive = repo_requirement(&next, ConfigLeaf::WorktreeRetentionExpensive);
    if next.schema_version.is_some() {
        base.schema_version = next.schema_version;
    }
    if !next.worktree.seed.is_empty() {
        base.worktree.seed = next.worktree.seed;
        record_winner(winners, "worktree.seed", layer, source.clone());
    }
    if !next.worktree.warm.is_empty() {
        base.worktree.warm = next.worktree.warm;
        record_winner(winners, "worktree.warm", layer, source.clone());
    }
    if setup_declared {
        base.worktree.setup = next.worktree.setup;
        record_winner(winners, "worktree.setup", layer, source.clone());
    }
    for (key, value) in next.worktree.env {
        base.worktree.env.insert(key.clone(), value);
        record_winner(
            winners,
            format!("worktree.env.{key}"),
            layer,
            source.clone(),
        );
    }
    // These tables have scalar serde defaults, so comparing their decoded
    // whole values loses whether an author actually supplied each sibling.
    // Use parse-time presence and overlay only the declared requirement leaf.
    if confinement_enabled {
        base.worktree.confinement.enabled = next.worktree.confinement.enabled;
        record_winner(
            winners,
            "worktree.confinement.enabled",
            layer,
            source.clone(),
        );
    }
    if confinement_sandbox {
        base.worktree.confinement.sandbox = next.worktree.confinement.sandbox;
        record_winner(
            winners,
            "worktree.confinement.sandbox",
            layer,
            source.clone(),
        );
    }
    if confinement_allow {
        base.worktree.confinement.allow = next.worktree.confinement.allow;
        record_winner(winners, "worktree.confinement.allow", layer, source.clone());
    }
    if tripwire_policy {
        base.worktree.tripwire.policy = next.worktree.tripwire.policy;
        record_winner(winners, "worktree.tripwire.policy", layer, source.clone());
    }
    if next.worktree.setup_seconds.is_some() {
        base.worktree.setup_seconds = next.worktree.setup_seconds;
        record_winner(winners, "worktree.setup-seconds", layer, source.clone());
    }
    if next.worktree.setup_capture_bytes.is_some() {
        base.worktree.setup_capture_bytes = next.worktree.setup_capture_bytes;
        record_winner(
            winners,
            "worktree.setup-capture-bytes",
            layer,
            source.clone(),
        );
    }
    if retention_cheap {
        base.worktree.retention.cheap = next.worktree.retention.cheap;
        record_winner(winners, "worktree.retention.cheap", layer, source.clone());
    }
    if retention_expensive {
        base.worktree.retention.expensive = next.worktree.retention.expensive;
        record_winner(
            winners,
            "worktree.retention.expensive",
            layer,
            source.clone(),
        );
    }
    if next.worktree.retention.expensive_grace_days.is_some() {
        base.worktree.retention.expensive_grace_days = next.worktree.retention.expensive_grace_days;
        record_winner(
            winners,
            "worktree.retention.expensive-grace-days",
            layer,
            source.clone(),
        );
    }
    for (host, value) in next.host {
        let fields = [
            ("profile", value.profile.is_some()),
            ("format", value.format.is_some()),
            ("project-path", value.project_path.is_some()),
            ("global-path", value.global_path.is_some()),
        ];
        base.host.entry(host.clone()).or_default().merge(value);
        for (field, present) in fields {
            if present {
                record_winner(
                    winners,
                    format!("host.{host}.{field}"),
                    layer,
                    source.clone(),
                );
            }
        }
    }
    if let Some(run) = next.run {
        let target = base.run.get_or_insert_with(RunTable::default);
        overlay_run_table(target, run, layer, source.clone(), winners);
    }
    if let Some(merge) = next.merge {
        let target = base.merge.get_or_insert_with(MergeTable::default);
        if merge.wait.is_some() {
            target.wait = merge.wait;
            record_winner(winners, "merge.wait", layer, source.clone());
        }
        if merge.overlap.is_some() {
            target.overlap = merge.overlap;
            record_winner(winners, "merge.overlap", layer, source.clone());
        }
        if merge.auto.is_some() {
            target.auto = merge.auto;
            record_winner(winners, "merge.auto", layer, source.clone());
        }
        if merge.deep.is_some() {
            target.deep = merge.deep;
            record_winner(winners, "merge.deep", layer, source.clone());
        }
        if merge.branch.is_some() {
            target.branch = merge.branch;
            record_winner(winners, "merge.branch", layer, source.clone());
        }
        // P477: `gate` replaces the whole ordered list wholesale (never
        // concatenated across layers) whenever this layer declares it at
        // all — including an explicit empty `gate = []`, which must clear a
        // nearer-absent outer declaration. `Option` tells the two cases
        // apart; only truly absent (`None`) inherits the outer value.
        if merge.gate.is_some() {
            target.gate = merge.gate;
            record_winner(winners, "merge.gate", layer, source.clone());
        }
        if merge.gate_seconds.is_some() {
            target.gate_seconds = merge.gate_seconds;
            record_winner(winners, "merge.gate-seconds", layer, source.clone());
        }
        // P463: `generated` replaces the whole ordered list wholesale
        // (never concatenated across layers), exactly like `gate` above.
        if merge.generated.is_some() {
            target.generated = merge.generated;
            record_winner(winners, "merge.generated", layer, source.clone());
        }
        if merge.disk_floor_mb.is_some() {
            target.disk_floor_mb = merge.disk_floor_mb;
            record_winner(winners, "merge.disk-floor-mb", layer, source.clone());
        }
        if merge.retry_attempts.is_some() {
            target.retry_attempts = merge.retry_attempts;
            record_winner(winners, "merge.retry-attempts", layer, source.clone());
        }
        if merge.retry_backoff_ms.is_some() {
            target.retry_backoff_ms = merge.retry_backoff_ms;
            record_winner(winners, "merge.retry-backoff-ms", layer, source.clone());
        }
    }
    if let Some(git) = next.git {
        let target = base.git.get_or_insert_with(GitTable::default);
        if git.long_seconds.is_some() {
            target.long_seconds = git.long_seconds;
            record_winner(winners, "git.long-seconds", layer, source.clone());
        }
    }
    if let Some(publish) = next.publish {
        let target = base.publish.get_or_insert_with(PublishTable::default);
        // `exclude` replaces the whole list wholesale whenever this layer
        // declares it at all — including an explicit empty `exclude = []` —
        // exactly like `[merge] gate`'s replace-never-concatenate rule.
        if publish.exclude.is_some() {
            target.exclude = publish.exclude;
            record_winner(winners, "publish.exclude", layer, source.clone());
        }
    }
    if let Some(registry) = next.registry {
        let target = base.registry.get_or_insert_with(RegistryTable::default);
        if registry.base.is_some() {
            target.base = registry.base;
            record_winner(winners, "registry.base", layer, source.clone());
        }
    }
    if let Some(tasks) = next.tasks {
        let target = base.tasks.get_or_insert_with(TasksTable::default);
        if tasks.dispatch_trait.is_some() {
            target.dispatch_trait = tasks.dispatch_trait;
            record_winner(winners, "tasks.dispatch-trait", layer, source.clone());
        }
        if tasks.auto_close.is_some() {
            target.auto_close = tasks.auto_close;
            record_winner(winners, "tasks.auto-close", layer, source);
        }
    }
}

fn overlay_run_table(
    base: &mut RunTable,
    next: RunTable,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    if next.worktree.is_some() {
        base.worktree = next.worktree;
        record_winner(winners, "run.worktree", layer, source.clone());
    }
    overlay_budget(&mut base.budget, &next.budget);
    for (key, present) in [
        ("max-frames", next.budget.max_frames.is_some()),
        ("frame-seconds", next.budget.frame_seconds.is_some()),
        ("total-seconds", next.budget.total_seconds.is_some()),
        ("max-retries", next.budget.max_retries.is_some()),
        (
            "attach-wait-seconds",
            next.budget.attach_wait_seconds.is_some(),
        ),
        ("idle-seconds", next.budget.idle_seconds.is_some()),
        ("command-seconds", next.budget.command_seconds.is_some()),
        (
            "command-idle-seconds",
            next.budget.command_idle_seconds.is_some(),
        ),
    ] {
        if present {
            record_winner(winners, format!("run.{key}"), layer, source.clone());
        }
    }
    if next.max_in_flight.is_some() {
        base.max_in_flight = next.max_in_flight;
        record_winner(winners, "run.max-in-flight", layer, source.clone());
    }
    if next.wait.is_some() {
        base.wait = next.wait;
        record_winner(winners, "run.wait", layer, source.clone());
    }
    for (name, cache) in next.build_cache {
        base.build_cache.insert(name.clone(), cache);
        record_winner(
            winners,
            format!("run.build-cache.{name}"),
            layer,
            source.clone(),
        );
    }
    if next.strict_loops.is_some() {
        base.strict_loops = next.strict_loops;
        record_winner(winners, "run.strict-loops", layer, source.clone());
    }
    if next.inline_prompt_bytes.is_some() {
        base.inline_prompt_bytes = next.inline_prompt_bytes;
        record_winner(winners, "run.inline-prompt-bytes", layer, source.clone());
    }
    if next.story.is_some() {
        base.story = next.story;
        record_winner(winners, "run.story", layer, source);
    }
}

fn merge_machine_config(
    base: &mut RuntimeConfig,
    next: RuntimeConfig,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    for (trait_id, defaults) in next.trait_defaults {
        let target = base.trait_defaults.entry(trait_id.clone()).or_default();
        for (port, value) in defaults.defaults.port {
            target.defaults.port.insert(port.clone(), value);
            record_winner(
                winners,
                format!("trait.{trait_id}.defaults.port.{port}"),
                layer,
                source.clone(),
            );
        }
        merge_trait_agent_defaults(
            &mut base.trait_defaults,
            &trait_id,
            &defaults.agent,
            &defaults.variant,
            layer,
            source.clone(),
            winners,
        );
    }
    if next.schema_version.is_some() {
        base.schema_version = next.schema_version;
    }
    for (name, harness) in next.harness {
        record_harness_winners(winners, &name, &harness, layer, source.clone(), false);
        let merged = base
            .harness
            .get(&name)
            .map(|current| harness.merged_onto(current))
            .unwrap_or(harness);
        base.harness.insert(name.clone(), merged);
    }
    let agent = next.agent;
    let role_keys: Vec<_> = agent.role.iter().collect();
    let model_tier_present = !agent.model_tier.is_empty();
    let agent_variant_role_keys = variant_role_assignments(&agent.variant);
    reconcile_assignment_winners(winners, &base.agent, &agent, "agent");
    for (name, assignment) in role_keys {
        record_assignment_winners(
            winners,
            &format!("agent.role.{name}"),
            assignment,
            layer,
            source.clone(),
        );
    }
    for (variant, role, assignment) in agent_variant_role_keys {
        record_assignment_winners(
            winners,
            &format!("agent.variant.{variant}.role.{role}"),
            assignment,
            layer,
            source.clone(),
        );
    }
    if model_tier_present {
        record_winner(
            winners,
            "agent.model-tier".to_string(),
            layer,
            source.clone(),
        );
    }
    merge_agent_defaults(&mut base.agent, agent);
    for (repo_key, repo_override) in next.repo {
        let target = base.repo.entry(repo_key.clone()).or_default();
        reconcile_assignment_winners(
            winners,
            &target.agent,
            &repo_override.agent,
            &format!("repo.{repo_key}.agent"),
        );
        let role_keys: Vec<_> = repo_override.agent.role.iter().collect();
        let repo_variant_role_keys = variant_role_assignments(&repo_override.agent.variant);
        for (role, assignment) in role_keys {
            record_assignment_winners(
                winners,
                &format!("repo.{repo_key}.agent.role.{role}"),
                assignment,
                layer,
                source.clone(),
            );
        }
        for (variant, role, assignment) in repo_variant_role_keys {
            record_assignment_winners(
                winners,
                &format!("repo.{repo_key}.agent.variant.{variant}.role.{role}"),
                assignment,
                layer,
                source.clone(),
            );
        }
        merge_agent_defaults(&mut target.agent, repo_override.agent);
    }
}

/// Merge one document's `[trait.<id>.agent…]`/`[trait.<id>.variant.<vid>.agent…]`
/// blocks (0034) into `base`, recording winners under `trait.<id>.agent.role.<r>`
/// / `trait.<id>.variant.<v>.agent.role.<r>` — reusing the same
/// `merge_agent_defaults`/`record_assignment_winners`/`reconcile_assignment_winners`
/// machinery every other qualifier scope (`agent.variant.*`, `repo.<key>.agent…`)
/// already merges through, rather than a fourth reimplementation. Shared by
/// all three tier-merge sites (environment, project, machine).
fn merge_trait_agent_defaults(
    base: &mut BTreeMap<String, TraitDefaults>,
    trait_id: &str,
    agent: &AgentDefaults,
    variant: &BTreeMap<String, TraitVariantDefaults>,
    layer: ConfigLayer,
    source: Option<String>,
    winners: &mut BTreeMap<String, ConfigWinner>,
) {
    let target = base.entry(trait_id.to_string()).or_default();
    reconcile_assignment_winners(
        winners,
        &target.agent,
        agent,
        &format!("trait.{trait_id}.agent"),
    );
    for (role, assignment) in &agent.role {
        record_assignment_winners(
            winners,
            &format!("trait.{trait_id}.agent.role.{role}"),
            assignment,
            layer,
            source.clone(),
        );
    }
    merge_agent_defaults(&mut target.agent, agent.clone());
    for (variant_id, value) in variant {
        let variant_target = target.variant.entry(variant_id.clone()).or_default();
        reconcile_assignment_winners(
            winners,
            &variant_target.agent,
            &value.agent,
            &format!("trait.{trait_id}.variant.{variant_id}.agent"),
        );
        for (role, assignment) in &value.agent.role {
            record_assignment_winners(
                winners,
                &format!("trait.{trait_id}.variant.{variant_id}.agent.role.{role}"),
                assignment,
                layer,
                source.clone(),
            );
        }
        merge_agent_defaults(&mut variant_target.agent, value.agent.clone());
    }
}

fn variant_role_assignments(
    variant: &BTreeMap<String, VariantOverride>,
) -> Vec<(String, String, &RoleAssignmentValue)> {
    variant
        .iter()
        .flat_map(|(name, value)| {
            value
                .role
                .iter()
                .map(move |(role, assignment)| (name.clone(), role.clone(), assignment))
        })
        .collect()
}

/// Drop leaf provenance when assignment resolution replaces a role value
/// wholesale. Single-to-single is the only field-wise assignment merge.
fn reconcile_assignment_winners(
    winners: &mut BTreeMap<String, ConfigWinner>,
    base: &AgentDefaults,
    next: &AgentDefaults,
    prefix: &str,
) {
    for (role, next_value) in &next.role {
        if !matches!(
            (base.role.get(role), next_value),
            (
                Some(RoleAssignmentValue::Single(_)),
                RoleAssignmentValue::Single(_)
            )
        ) {
            remove_winner_subtree(winners, &format!("{prefix}.role.{role}"));
        }
    }
    for (variant, next_variant) in &next.variant {
        let base_variant = base.variant.get(variant);
        for (role, next_value) in &next_variant.role {
            if !matches!(
                (
                    base_variant.and_then(|value| value.role.get(role)),
                    next_value
                ),
                (
                    Some(RoleAssignmentValue::Single(_)),
                    RoleAssignmentValue::Single(_)
                )
            ) {
                remove_winner_subtree(winners, &format!("{prefix}.variant.{variant}.role.{role}"));
            }
        }
    }
}

fn record_assignment_winners(
    winners: &mut BTreeMap<String, ConfigWinner>,
    prefix: &str,
    value: &RoleAssignmentValue,
    layer: ConfigLayer,
    source: Option<String>,
) {
    for (index, assignment) in value.entries().iter().enumerate() {
        let prefix = if value.is_list() {
            format!("{prefix}.{}", index + 1)
        } else {
            prefix.to_string()
        };
        for (field, present) in [
            ("mode", assignment.mode_authored),
            ("harness", assignment.harness.is_some()),
            ("transport", assignment.transport.is_some()),
            ("session-mode", assignment.session_mode.is_some()),
            ("model", assignment.model.is_some()),
            ("model-tier", assignment.model_tier.is_some()),
            ("reasoning-effort", assignment.reasoning_effort.is_some()),
            ("system-prompt", assignment.system_prompt.is_some()),
            ("extra-args", !assignment.extra_args.is_empty()),
            (
                "budget.frame-seconds",
                assignment.budget.frame_seconds.is_some(),
            ),
            (
                "budget.idle-seconds",
                assignment.budget.idle_seconds.is_some(),
            ),
            (
                "budget.max-retries",
                assignment.budget.max_retries.is_some(),
            ),
        ] {
            if present {
                record_winner(winners, format!("{prefix}.{field}"), layer, source.clone());
            }
        }
    }
}

fn record_personal_assignment_winners(
    winners: &mut BTreeMap<String, ConfigWinner>,
    prefix: &str,
    value: &RoleAssignmentValue,
    source: Option<String>,
) {
    let mut authored = BTreeMap::new();
    record_assignment_winners(
        &mut authored,
        prefix,
        value,
        ConfigLayer::UserGlobal,
        source.clone(),
    );
    for key in authored.into_keys() {
        record_personal_winner(winners, key, source.clone());
    }
}

fn record_harness_winners(
    winners: &mut BTreeMap<String, ConfigWinner>,
    name: &str,
    harness: &HarnessDefinition,
    layer: ConfigLayer,
    source: Option<String>,
    personal: bool,
) {
    let prefix = format!("harness.{name}");
    let mut record = |key: String| {
        if personal {
            record_personal_winner(winners, key, source.clone())
        } else {
            record_winner(winners, key, layer, source.clone())
        }
    };
    for (field, present) in [
        ("kind", harness.kind.is_some()),
        ("bin", harness.bin.is_some()),
        ("transports", !harness.transports.is_empty()),
        ("version-probe", !harness.version_probe.is_empty()),
    ] {
        if present {
            record(format!("{prefix}.{field}"));
        }
    }
    if let Some(cli) = &harness.cli {
        for (field, present) in [
            ("argv", !cli.argv.is_empty()),
            ("narrator-argv", cli.narrator_argv.is_some()),
            ("warm-argv", cli.warm_argv.is_some()),
            ("json-schema-flag", cli.json_schema_flag.is_some()),
            ("model-flag", cli.model_flag.is_some()),
            ("reasoning-effort-flag", cli.reasoning_effort_flag.is_some()),
            ("system-prompt-flag", cli.system_prompt_flag.is_some()),
            ("resume-flag", cli.resume_flag.is_some()),
            ("session-flag", cli.session_flag.is_some()),
            ("dir-flag", cli.dir_flag.is_some()),
            ("prompt-via", cli.prompt_via.is_some()),
            ("stream", cli.stream.is_some()),
            ("output", cli.output.is_some()),
        ] {
            if present {
                record(format!("{prefix}.cli.{field}"));
            }
        }
    }
    if let Some(mcp) = &harness.mcp {
        for (field, present) in [
            ("mcp-config-flag", mcp.mcp_config_flag.is_some()),
            ("allowed-tools-flag", mcp.allowed_tools_flag.is_some()),
            ("allowed-tools", !mcp.allowed_tools.is_empty()),
            ("system-prompt-flag", mcp.system_prompt_flag.is_some()),
            ("reasoning-effort-flag", mcp.reasoning_effort_flag.is_some()),
            ("config-via", mcp.config_via.is_some()),
        ] {
            if present {
                record(format!("{prefix}.mcp.{field}"));
            }
        }
    }
}

fn record_host_winners(
    winners: &mut BTreeMap<String, ConfigWinner>,
    name: &str,
    host: &HostOverride,
    layer: ConfigLayer,
    source: Option<String>,
    personal: bool,
) {
    for (field, present) in [
        ("profile", host.profile.is_some()),
        ("format", host.format.is_some()),
        ("project-path", host.project_path.is_some()),
        ("global-path", host.global_path.is_some()),
    ] {
        if present {
            let key = format!("host.{name}.{field}");
            if personal {
                record_personal_winner(winners, key, source.clone())
            } else {
                record_winner(winners, key, layer, source.clone())
            }
        }
    }
}

fn merge_agent_defaults(base: &mut AgentDefaults, next: AgentDefaults) {
    base.model_tier.extend(next.model_tier);
    for (role, next_value) in next.role {
        match (base.role.get_mut(&role), next_value) {
            (Some(RoleAssignmentValue::Single(base)), RoleAssignmentValue::Single(next)) => {
                merge_assignment_fields(base, &next);
            }
            (_, next) => {
                base.role.insert(role, next);
            }
        }
    }
    for (variant, next_variant) in next.variant {
        let target = base.variant.entry(variant).or_default();
        for (role, next_value) in next_variant.role {
            match (target.role.get_mut(&role), next_value) {
                (Some(RoleAssignmentValue::Single(base)), RoleAssignmentValue::Single(next)) => {
                    merge_assignment_fields(base, &next);
                }
                (_, next) => {
                    target.role.insert(role, next);
                }
            }
        }
    }
}

/// 0025: expand every expansion-shaped `[agent.role.<role>]` entry into
/// stable per-seat aliases (`<role>-1` … `<role>-N`) so a trait declaring
/// those as distinct `[[agent]]` ids resolves them through the existing
/// exact-name lookup unchanged. A role is expansion-shaped as a `Single`
/// table with `count = N` (N identical seats, `count` cleared on each) or as
/// a `List` with N entries (each entry may differ). An authored exact table
/// already occupying a seat alias wins wholesale — expansion never
/// overwrites an existing key — and the base role key is always kept, so a
/// trait agent id equal to the role name itself keeps resolving to it (P456
/// rotation for `List`, single-seat behaviour for `Single`+`count`).
///
/// Must run immediately after scope merging
/// ([`merge_agent_defaults`]/`flatten_agent_defaults`) and before override
/// parsing/validation: this is the exact point 0034's trait-scope fold must
/// land BEFORE, or a trait-scoped `count` would arrive too late to change
/// the expansion.
///
/// `pub` so `doctor --config` (P475's per-seat row convention) can show the
/// expanded seats an actual run would resolve without reimplementing this
/// rule — the resolved view must match what dispatch uses.
pub fn expand_role_seats(defaults: &mut AgentDefaults) {
    let mut seats: Vec<(String, ProfileAssignment)> = Vec::new();
    for (role, value) in &defaults.role {
        match value {
            RoleAssignmentValue::Single(assignment) => {
                let Some(count) = assignment.count else {
                    continue;
                };
                for index in 1..=count {
                    let mut seat = assignment.clone();
                    seat.count = None;
                    seats.push((format!("{role}-{index}"), seat));
                }
            }
            RoleAssignmentValue::List(entries) => {
                for (offset, entry) in entries.iter().enumerate() {
                    let mut seat = entry.clone();
                    seat.count = None;
                    seats.push((format!("{role}-{}", offset + 1), seat));
                }
            }
        }
    }
    for (seat_role, seat) in seats {
        defaults
            .role
            .entry(seat_role)
            .or_insert(RoleAssignmentValue::Single(seat));
    }
}

/// The whole-role (`--assign reviewer=...`) and per-seat
/// (`--assign reviewer.2=...`) override layers parsed from repeated
/// `--assign` flags. Kept as two maps rather than one so
/// [`ResolvedRuntimeAssignments::resolved_seats_for_role`] can apply the
/// whole-role overlay to every seat before layering the more specific
/// per-seat overlay on top, deterministically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssignmentOverrides {
    pub whole: BTreeMap<String, ProfileAssignment>,
    pub seat: BTreeMap<(String, u32), ProfileAssignment>,
}

/// Parse a `--assign` target into its role and optional 1-based seat:
/// `reviewer` -> `(reviewer, None)`, `reviewer.2` -> `(reviewer, Some(2))`.
/// A trailing all-digit segment after the last `.` is always treated as a
/// seat selector, since a bare role id (`validate_bare_id`) never contains
/// `.` itself.
fn parse_assign_target(key: &str) -> crate::Result<(String, Option<u32>)> {
    if let Some((role_part, seat_part)) = key.rsplit_once('.')
        && !seat_part.is_empty()
        && seat_part.bytes().all(|b| b.is_ascii_digit())
    {
        let role = normalize_role(role_part, "run.assign.role")?;
        let seat: u32 = seat_part
            .parse()
            .map_err(|_| ())
            .and_then(|seat: u32| if seat == 0 { Err(()) } else { Ok(seat) })
            .map_err(|()| {
                config_error(
                    format!("run.assign.{key}"),
                    format!("seat selector {seat_part:?} must be an integer >= 1"),
                )
            })?;
        if is_standing_seat(&role) {
            return invalid_config(
                format!("run.assign.{key}"),
                format!("role {role:?} is a standing agent and does not support seat selectors"),
            );
        }
        return Ok((role, Some(seat)));
    }
    Ok((normalize_role(key, "run.assign.role")?, None))
}

pub fn parse_assignment_overrides(overrides: &[String]) -> crate::Result<AssignmentOverrides> {
    let mut parsed = AssignmentOverrides::default();
    let mut seen_selectors = BTreeSet::new();
    for item in overrides {
        let Some((target, value)) = item.split_once('=') else {
            return invalid_config(
                "run.assign",
                format!(
                    "--assign value {item:?} must use role[.seat]=harness[:transport[:session-mode[:model[:reasoning-effort]]]]"
                ),
            );
        };
        let (role, seat) = parse_assign_target(target)?;
        let selector = match seat {
            Some(seat) => format!("{role}.{seat}"),
            None => role.clone(),
        };
        if !seen_selectors.insert(selector.clone()) {
            return invalid_config(
                format!("run.assign.{selector}"),
                format!("duplicate assignment override for {selector:?}"),
            );
        }
        if let Some(value) = value.strip_prefix("json:") {
            let mut assignment: ProfileAssignment =
                serde_json::from_str(value).map_err(|source| {
                    crate::parse::Error::JsonDeserialize {
                        context: format!("JSON assignment override for {selector:?}"),
                        source,
                    }
                })?;
            // P475: `--assign role=json:{...}` decodes a whole
            // `ProfileAssignment`, `budget` included — route it through the
            // same validation config tables get so a budget arriving via an
            // override cannot skip the zero-seconds/one-shot-applicability
            // checks (budgets are otherwise config-only, never a documented
            // `--assign` surface, but the decoder accepts the field either
            // way, so this closes the gap rather than leaving it open).
            validate_role_budget(&role, &format!("run.assign.{selector}"), &assignment.budget)?;
            assignment.replace_inherited = true;
            match seat {
                Some(seat) => {
                    parsed.seat.insert((role, seat), assignment);
                }
                None => {
                    parsed.whole.insert(role, assignment);
                }
            }
            continue;
        }
        let mut parts = value.split(':');
        let harness = parts.next().unwrap_or_default();
        if harness.trim().is_empty() {
            return invalid_config("run.assign.harness", "assignment harness must not be empty");
        }
        let transport = match parts.next() {
            Some("cli") | None => Some(RunTransport::Cli),
            Some("mcp") => Some(RunTransport::Mcp),
            Some("api") => Some(RunTransport::Api),
            Some(other) => {
                return invalid_config(
                    "run.assign.transport",
                    format!(
                        "unsupported assignment transport {other:?}; expected cli, mcp, or api"
                    ),
                );
            }
        };
        let session_mode = match parts.next() {
            Some("per-frame") | None => Some(RunSessionMode::PerFrame),
            Some("persistent") => Some(RunSessionMode::Persistent),
            Some(other) => {
                return invalid_config(
                    "run.assign.session-mode",
                    format!(
                        "unsupported assignment session-mode {other:?}; expected per-frame or persistent"
                    ),
                );
            }
        };
        let model = parts
            .next()
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string);
        let reasoning_effort = parts
            .next()
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string);
        if parts.next().is_some() {
            return invalid_config(
                "run.assign",
                format!("--assign value {item:?} has too many ':' segments"),
            );
        }
        let assignment = ProfileAssignment {
            replace_inherited: false,
            model_selector: None,
            model_resolution_reason: None,
            mode: RunAssignmentMode::Harness,
            mode_authored: true,
            harness: Some(harness.to_string()),
            transport,
            session_mode,
            model,
            model_tier: None,
            reasoning_effort,
            system_prompt: None,
            extra_args: Vec::new(),
            budget: RoleBudget::default(),
            api: Box::default(),
            count: None,
        };
        match seat {
            Some(seat) => {
                parsed.seat.insert((role, seat), assignment);
            }
            None => {
                parsed.whole.insert(role, assignment);
            }
        }
    }
    Ok(parsed)
}

/// `role`'s own single-table `[agent.role.<role>]` entry, or `None` for an
/// absent role or one configured as a `[[agent.role.<role>]]` list — a
/// role-only lookup never silently selects a list's first entry; callers
/// that need a list-backed role go through
/// [`ResolvedRuntimeAssignments::resolved_seats_for_role`] instead.
fn single_role_table<'a>(defaults: &'a AgentDefaults, role: &str) -> Option<&'a ProfileAssignment> {
    match defaults.role.get(role) {
        Some(RoleAssignmentValue::Single(assignment)) => Some(assignment),
        Some(RoleAssignmentValue::List(_)) | None => None,
    }
}

/// Resolve any role (declared trait role or standing seat) from its own
/// single-table entry, with `explicit` (an `--assign` override) winning
/// last.
fn resolved_assignment_for_role(
    defaults: &AgentDefaults,
    role: &str,
    explicit: Option<&ProfileAssignment>,
) -> Option<ProfileAssignment> {
    let role_default = single_role_table(defaults, role);
    finalize_assignment(raw_assignment_for_role(
        defaults,
        role,
        role_default,
        explicit,
    ))
}

/// Layer one role entry (a single-table default, or one seat of a
/// `[[agent.role.<role>]]` list) with `explicit` (an `--assign` override,
/// already combined whole-role + per-seat when called from seat resolution)
/// winning last — the pure layering half, without the final
/// collapse-to-`None`-when-harness-missing step, so a caller can attempt the
/// P427 built-in fallback (or, for [`configured_seats_for_role`],
/// intentionally discard it) before deciding what "no assignment" means.
///
/// `role_default` is self-described: a role WITH its own table inherits
/// nothing. A role with NO table of its own falls back to the whole
/// `[agent.role.default]` seat instead — except the four standing seats
/// themselves ([`is_standing_seat`]), whose own absence carries its own
/// meaning (`default` has nothing to fall back to; `narrator` absence is a
/// valid passthrough mode; `merger`/`merger-deep` absence makes the feature
/// unavailable) and must never silently borrow `default`'s harness/model.
fn raw_assignment_for_role(
    defaults: &AgentDefaults,
    role: &str,
    role_default: Option<&ProfileAssignment>,
    explicit: Option<&ProfileAssignment>,
) -> ProfileAssignment {
    let mut assignment = base_for_role(defaults, role, role_default);
    if let Some(explicit) = explicit {
        merge_assignment(&mut assignment, explicit);
    }
    // Model tiers remain decodable for diagnostics, but are no longer runtime
    // selectors. A concrete model or alias must be supplied instead.
    assignment.model_tier = None;
    assignment
}

/// `role`'s own table (`role_default`) if the caller already has one, else —
/// for any role except the four standing seats themselves — the whole
/// `[agent.role.default]` seat, else an empty assignment. Extracted (P451)
/// so [`raw_assignment_for_role`] and [`combine_role_level`]'s "no base
/// table" seeding share exactly one "self-described vs. default-fallback"
/// rule rather than restating it.
fn base_for_role(
    defaults: &AgentDefaults,
    role: &str,
    role_default: Option<&ProfileAssignment>,
) -> ProfileAssignment {
    match role_default {
        Some(role_default) => role_default.clone(),
        None if role != DEFAULT_SEAT && !is_standing_seat(role) => {
            single_role_table(defaults, DEFAULT_SEAT)
                .cloned()
                .unwrap_or_default()
        }
        None => ProfileAssignment::default(),
    }
}

/// The final collapse-to-`None` step of role resolution: a Harness-mode
/// assignment with nothing to dispatch on does not exist. An api-transport
/// seat (0079) IS dispatchable without any harness — `resolve_seat_dispatch`
/// owns what happens when its key later fails to resolve (degrade to a
/// declared harness, else `Unavailable`, which every standing-seat caller
/// already treats as "no narration", never an error). Collapsing such a seat
/// here instead made a harness-less `transport = "api"` narrator silently
/// vanish from resolution — no seat, no titles, no narration, no diagnostic.
fn finalize_assignment(assignment: ProfileAssignment) -> Option<ProfileAssignment> {
    match assignment.mode {
        RunAssignmentMode::Attach => Some(assignment),
        RunAssignmentMode::Harness
            if assignment.harness.is_some() || assignment.transport == Some(RunTransport::Api) =>
        {
            Some(assignment)
        }
        RunAssignmentMode::Harness => None,
    }
}

fn merge_assignment(base: &mut ProfileAssignment, next: &ProfileAssignment) {
    if next.replace_inherited {
        *base = next.clone();
        base.replace_inherited = false;
        return;
    }
    merge_assignment_fields(base, next);
}

/// Field-by-field overlay used by [`merge_assignment`] to layer an explicit
/// `--assign`/profile entry over `.ctx/config.toml` role/tier defaults: only
/// fields `next` actually sets replace the corresponding field on `base`.
fn merge_assignment_fields(base: &mut ProfileAssignment, next: &ProfileAssignment) {
    if next.mode_authored {
        base.mode = next.mode;
        base.mode_authored = true;
    }
    if next.harness.is_some() {
        base.harness = next.harness.clone();
    }
    if next.transport.is_some() {
        base.transport = next.transport;
    }
    if next.session_mode.is_some() {
        base.session_mode = next.session_mode;
    }
    if next.model.is_some() {
        base.model = next.model.clone();
    }
    if next.model_tier.is_some() {
        base.model_tier = next.model_tier;
    }
    if next.count.is_some() {
        base.count = next.count;
    }
    if next.reasoning_effort.is_some() {
        base.reasoning_effort = next.reasoning_effort.clone();
    }
    if next.system_prompt.is_some() {
        base.system_prompt = next.system_prompt.clone();
    }
    if !next.extra_args.is_empty() {
        // argv is ordered command input, not an additive set. A nearer
        // assignment owns the complete argument vector.
        base.extra_args = next.extra_args.clone();
    }
    if next.api.base_url.is_some() {
        base.api.base_url = next.api.base_url.clone();
    }
    if next.api.wire.is_some() {
        base.api.wire = next.api.wire;
    }
    if next.api.api_key_env.is_some() {
        base.api.api_key_env = next.api.api_key_env.clone();
    }
    if next.api.connect_timeout_ms.is_some() {
        base.api.connect_timeout_ms = next.api.connect_timeout_ms;
    }
    if next.api.read_timeout_ms.is_some() {
        base.api.read_timeout_ms = next.api.read_timeout_ms;
    }
    if next.api.retries.is_some() {
        base.api.retries = next.api.retries;
    }
    overlay_role_budget(&mut base.budget, &next.budget);
}

/// Field-wise overlay for a seat's [`RoleBudget`] (P475), mirroring
/// [`overlay_budget`]: only fields `next` actually declares replace the
/// corresponding field on `base`, so an `--assign`/seat override that names
/// only one budget field never erases the rest of the inherited seat's
/// budget.
fn overlay_role_budget(base: &mut RoleBudget, next: &RoleBudget) {
    if next.frame_seconds.is_some() {
        base.frame_seconds = next.frame_seconds;
    }
    if next.idle_seconds.is_some() {
        base.idle_seconds = next.idle_seconds;
    }
    if next.max_retries.is_some() {
        base.max_retries = next.max_retries;
    }
}

/// Validate every declared `[run.build-cache.<name>]` (P428): the name must
/// be a safe single path component (reusing [`validate_bare_id`], the same
/// contract harness/role ids already use, since a cache name becomes a path
/// leaf under the global cache root), `env` must not be empty, and no two
/// declared caches may name the same environment variable -- otherwise the
/// effective worktree overlay could not tell which cache directory a build
/// tool should actually see.
fn validate_build_cache(build_cache: &BTreeMap<String, BuildCacheConfig>) -> crate::Result<()> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, cache) in build_cache {
        validate_bare_id(name, &format!("run.build-cache.{name}"))?;
        if cache.env.trim().is_empty() {
            return invalid_config(
                format!("run.build-cache.{name}.env"),
                "env must not be empty",
            );
        }
        if let Some(previous) = seen.insert(cache.env.as_str(), name.as_str()) {
            return invalid_config(
                format!("run.build-cache.{name}.env"),
                format!(
                    "environment variable {:?} is already declared by build cache {previous:?}; each declared cache must export a distinct environment variable",
                    cache.env
                ),
            );
        }
    }
    Ok(())
}

/// Validate the resolved `[merge] gate`/`gate-seconds` (P477): every declared
/// command must have a non-empty executable (its argv's first element), the
/// per-command ceiling must be positive, and the aggregate millisecond
/// budget (`gate.len() * gate-seconds * 1000`, the same product
/// `merge_lock_wait_timeout_ms` sums into the merge-lock wait) must fit in a
/// `u64` — so a pathological declaration fails fast at config-resolution
/// time rather than silently wrapping inside the merge machinery's own
/// checked calculation.
fn validate_merge_gate(policy: &EffectiveMergePolicy) -> crate::Result<()> {
    if policy.gate_seconds == 0 {
        return invalid_config("merge.gate-seconds", "must be at least 1");
    }
    for (index, command) in policy.gate.iter().enumerate() {
        match command.first() {
            Some(executable) if !executable.trim().is_empty() => {}
            _ => {
                return invalid_config(
                    format!("merge.gate[{index}]"),
                    "command must declare a non-empty executable",
                );
            }
        }
    }
    let per_command_ms = policy.gate_seconds.checked_mul(1_000).ok_or_else(|| {
        config_error("merge.gate-seconds", "too large to convert to milliseconds")
    })?;
    u64::try_from(policy.gate.len())
        .ok()
        .and_then(|len| len.checked_mul(per_command_ms))
        .ok_or_else(|| {
            config_error(
                "merge.gate",
                "declares too many commands for its per-command ceiling to be summed",
            )
        })?;
    Ok(())
}

fn validate_merge_retries(policy: &EffectiveMergePolicy) -> crate::Result<()> {
    if policy.retry_attempts == 0 {
        return invalid_config("merge.retry-attempts", "must be at least 1");
    }
    if policy.retry_backoff_ms == 0 {
        return invalid_config("merge.retry-backoff-ms", "must be at least 1");
    }
    Ok(())
}

/// A repository-relative path is well-formed for a `[[merge.generated]]`
/// declaration when it is non-empty, not absolute, and does not walk up via
/// `..` — the same shape every other repository-relative path this config
/// handles is expected to have (mirrors `is_well_formed_relative_path` in
/// `ctx traits merge`'s deep-decision-receipt validation, duplicated here
/// rather than shared across the crate boundary for one three-line check).
fn is_well_formed_declared_relative_path(path: &str) -> bool {
    !path.trim().is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment == "..")
}

/// Validate every declared `[[merge.generated]]` entry (P463): each must
/// declare at least one path (all well-formed repository-relative paths) and
/// at least one `rebuild` command (each with a non-empty executable) — a
/// malformed declaration must surface here, as a config error, rather than
/// as a mid-merge park (see the P463 draft's own risk #6).
fn validate_merge_generated(policy: &EffectiveMergePolicy) -> crate::Result<()> {
    for (index, entry) in policy.generated.iter().enumerate() {
        if entry.paths.is_empty() {
            return invalid_config(
                format!("merge.generated[{index}].paths"),
                "must declare at least one path",
            );
        }
        for path in &entry.paths {
            if !is_well_formed_declared_relative_path(path) {
                return invalid_config(
                    format!("merge.generated[{index}].paths"),
                    format!("{path:?} is not a well-formed repository-relative path"),
                );
            }
        }
        if entry.rebuild.is_empty() {
            return invalid_config(
                format!("merge.generated[{index}].rebuild"),
                "must declare at least one command",
            );
        }
        for (command_index, command) in entry.rebuild.iter().enumerate() {
            match command.first() {
                Some(executable) if !executable.trim().is_empty() => {}
                _ => {
                    return invalid_config(
                        format!("merge.generated[{index}].rebuild[{command_index}]"),
                        "command must declare a non-empty executable",
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_registry(registry: &HarnessRegistry) -> crate::Result<()> {
    for id in registry.harness.keys() {
        validate_bare_id(id, &format!("harness.{id}"))?;
        // P568: validate the EFFECTIVE definition, not the authored table. A
        // built-in id's table is merged over the compiled-in one, so a
        // constraint can be violated by the COMBINATION even when the authored
        // text alone is legal — `warm-argv` (configured) against an inherited
        // `json-schema-flag` is the live case. Validating the raw table would
        // let that reach a spawn. For a custom id the merged form is the table
        // itself, so this is a no-op there.
        let effective = built_in_harness_definition(id, registry);
        let harness = &effective;
        if harness.bin().trim().is_empty() {
            return invalid_config(format!("harness.{id}.bin"), "harness bin must not be empty");
        }
        if harness.transports.is_empty() {
            return invalid_config(
                format!("harness.{id}.transports"),
                "harness transports must not be empty",
            );
        }
        if harness.version_probe.is_empty() {
            return invalid_config(
                format!("harness.{id}.version-probe"),
                "harness version-probe must not be empty",
            );
        }
        for transport in &harness.transports {
            match transport {
                RunTransport::Cli if harness.cli.is_none() => {
                    return invalid_config(
                        format!("harness.{id}.cli"),
                        "harness declares cli transport but has no cli convention table",
                    );
                }
                RunTransport::Mcp if harness.mcp.is_none() => {
                    return invalid_config(
                        format!("harness.{id}.mcp"),
                        "harness declares mcp transport but has no mcp convention table",
                    );
                }
                _ => {}
            }
        }
        if let Some(cli) = &harness.cli {
            let output = cli.output.as_deref().unwrap_or_default();
            if output.trim().is_empty() {
                return invalid_config(
                    format!("harness.{id}.cli.output"),
                    "cli convention must declare output parser id",
                );
            }
            if !known_output_parser(output) {
                return invalid_config(
                    format!("harness.{id}.cli.output"),
                    format!("unsupported output parser id {output:?}"),
                );
            }
            if cli.warm_argv.is_some() && output != "claude-stream-json" {
                return invalid_config(
                    format!("harness.{id}.cli.warm-argv"),
                    "warm-argv currently supports only claude-stream-json output",
                );
            }
            if cli.warm_argv.is_some() && cli.json_schema_flag.is_some() {
                return invalid_config(
                    format!("harness.{id}.cli.warm-argv"),
                    "warm-argv cannot honor per-frame json-schema-flag; remove one",
                );
            }
        }
    }
    Ok(())
}

fn known_output_parser(output: &str) -> bool {
    matches!(
        output,
        "raw-json"
            | "claude-json"
            | "claude-stream-json"
            | "opencode-json"
            | "pi-json"
            | "codex-json"
    )
}

/// Validate the resolver's final merged assignment map. Every role is
/// validated with the full [`validate_assignment`] contract:
/// the trait package's `config.toml` sidecar is budget-only (P312) and never
/// contributes a partial assignment layer.
fn validate_assignments(assignments: &BTreeMap<String, ProfileAssignment>) -> crate::Result<()> {
    for (role, assignment) in assignments {
        normalize_role(role, &format!("assign.{role}"))?;
        if is_standing_seat(role) && assignment.model_tier.is_some() {
            return invalid_config(
                format!("assign.{role}.model-tier"),
                "model-tier applies only to declared trait roles",
            );
        }
        validate_assignment(role, assignment)?;
    }
    Ok(())
}

/// One validation pass over `[agent.role.*]`, driven by [`STANDING_SEATS`]:
/// every role gets the ordinary trait-role checks
/// ([`validate_assignment_defaults`]); the four standing seats additionally
/// get the self-described harness-id/reasoning-effort format checks
/// ([`validate_special_assignment`]) and are rejected as list-form (P456)
/// declarations, since each is restricted to exactly one seat; `merger`/
/// `merger-deep` additionally require a full harness+model+reasoning-effort
/// declaration when present, since their absence — unlike every other role
/// — is never filled from `[agent.role.default]`.
fn validate_agent_defaults(defaults: &AgentDefaults) -> crate::Result<()> {
    for (tier, assignment) in &defaults.model_tier {
        let path = format!("agent.model-tier.{}", tier.as_str());
        if assignment.mode != RunAssignmentMode::Harness {
            return invalid_config(path, "model-tier definition requires harness mode");
        }
        if assignment.model_tier.is_some() {
            return invalid_config(
                format!("{path}.model-tier"),
                "model-tier definition must not reference another model tier",
            );
        }
        validate_assignment_defaults(tier.as_str(), assignment)?;
    }
    // The base table is the only level whose standing seats (`merger`/
    // `merger-deep`) must satisfy the full harness+model+reasoning-effort
    // declaration on their own; a variant/repo qualifier is a legitimate
    // partial override and is checked again, in full, only after
    // [`flatten_agent_defaults`] resolves the effective per-run result.
    validate_role_map(&defaults.role, "agent.role", true)?;
    validate_variant_maps(&defaults.variant, "agent")?;
    Ok(())
}

/// Validate every `[agent.variant.<v>.role.*]` table (P451) — always in
/// partial mode, since a variant-qualified table is never required to
/// satisfy a standing seat's full-declaration rule on its own.
fn validate_variant_maps(
    variant: &BTreeMap<String, VariantOverride>,
    path_prefix: &str,
) -> crate::Result<()> {
    for (name, value) in variant {
        validate_role_map(
            &value.role,
            &format!("{path_prefix}.variant.{name}.role"),
            false,
        )?;
    }
    Ok(())
}

/// Validate every GLOBAL-only `[repo."<key>"]` block (P451): its own
/// `agent.role` table (partial mode — `(repo,role)` is itself a qualifier
/// level, never required to stand alone) and its nested
/// `agent.variant.<v>.role` tables.
fn validate_repo_overrides(repo: &BTreeMap<String, RepoOverride>) -> crate::Result<()> {
    for (key, value) in repo {
        let prefix = format!("repo.{key}.agent");
        validate_role_map(&value.agent.role, &format!("{prefix}.role"), false)?;
        validate_variant_maps(&value.agent.variant, &prefix)?;
    }
    Ok(())
}

/// Validate every declared `[trait.<id>]` block's `agent`/`variant.<v>.agent`
/// tables (0034): its own `agent.role` table (partial mode — `(trait,role)`
/// is itself a qualifier level, never required to stand alone) and its
/// nested `variant.<vid>.agent.role` tables. Unlike a repo qualifier, a
/// trait-scoped `agent.variant` (or `variant.<vid>.agent.variant`) is a hard
/// config error: the canonical spelling for a trait-scoped variant is
/// `trait.<id>.variant.<vid>.agent`, variant ABOVE agent, not `agent.variant`
/// below it — so the grammar has exactly one spelling, never two that could
/// silently disagree.
fn validate_trait_overrides(trait_defaults: &BTreeMap<String, TraitDefaults>) -> crate::Result<()> {
    for (trait_id, value) in trait_defaults {
        let prefix = format!("trait.{trait_id}.agent");
        if !value.agent.variant.is_empty() {
            return invalid_config(
                format!("{prefix}.variant"),
                format!(
                    "trait-scoped variants are declared as [trait.{trait_id}.variant.<vid>.agent…], not [trait.{trait_id}.agent.variant.<vid>…]"
                ),
            );
        }
        validate_role_map(&value.agent.role, &format!("{prefix}.role"), false)?;
        for (variant_id, variant_value) in &value.variant {
            let variant_prefix = format!("trait.{trait_id}.variant.{variant_id}.agent");
            if !variant_value.agent.variant.is_empty() {
                return invalid_config(
                    format!("{variant_prefix}.variant"),
                    format!(
                        "trait-scoped variants cannot nest another variant.<vid> under [trait.{trait_id}.variant.{variant_id}.agent…]"
                    ),
                );
            }
            validate_role_map(
                &variant_value.agent.role,
                &format!("{variant_prefix}.role"),
                false,
            )?;
        }
    }
    Ok(())
}

/// One `[agent.role.*]`-shaped table's per-role checks, shared by the base
/// table (`full = true`: standing seats requiring a full declaration
/// (`merger`/`merger-deep`) must satisfy it here) and every variant/repo
/// qualifier level (`full = false`: a partial override never has to satisfy
/// that rule on its own — only the flattened per-run result does, checked a
/// second time, in full, by the caller after [`flatten_agent_defaults`]).
/// Every other check (bare-id shape, standing-seat list rejection, empty-list
/// rejection, self-described format checks, budget validity) applies at
/// every level regardless of `full`.
fn validate_role_map(
    role_map: &BTreeMap<String, RoleAssignmentValue>,
    path_prefix: &str,
    full: bool,
) -> crate::Result<()> {
    for (role, value) in role_map {
        normalize_role(role, &format!("{path_prefix}.{role}"))?;
        if is_standing_seat(role) && value.is_list() {
            return invalid_config(
                format!("{path_prefix}.{role}"),
                format!(
                    "role {role:?} is a standing agent and accepts exactly one seat, not {} in [[{path_prefix}.{role}]]",
                    value.entries().len()
                ),
            );
        }
        if value.is_list() && value.entries().is_empty() {
            return invalid_config(
                format!("{path_prefix}.{role}"),
                format!("[[{path_prefix}.{role}]] must declare at least one seat"),
            );
        }
        for (offset, assignment) in value.entries().iter().enumerate() {
            let path = if value.is_list() {
                format!("{role}.{}", offset + 1)
            } else {
                role.clone()
            };
            if is_standing_seat(role) {
                validate_special_assignment(assignment, &format!("{path_prefix}.{path}"))?;
            }
            // 0025: `count` only means "N seats of this Single table" — a
            // list's own length is already its seat count, and a standing
            // seat is restricted to exactly one seat by definition.
            if let Some(count) = assignment.count {
                if value.is_list() {
                    return invalid_config(
                        format!("{path_prefix}.{path}.count"),
                        "count is not allowed inside a [[...]] seat list; the list length is the seat count",
                    );
                }
                if is_standing_seat(role) {
                    return invalid_config(
                        format!("{path_prefix}.{path}.count"),
                        format!(
                            "role {role:?} is a standing agent and accepts exactly one seat; count is not allowed"
                        ),
                    );
                }
                if count == 0 {
                    return invalid_config(
                        format!("{path_prefix}.{path}.count"),
                        "count must be at least 1",
                    );
                }
            }
            if full && standing_seat_requires_full_declaration(role) {
                if assignment.mode != RunAssignmentMode::Harness || assignment.harness.is_none() {
                    return invalid_config(
                        format!("{path_prefix}.{path}.harness"),
                        format!("{path_prefix}.{role} must declare harness mode and a harness id"),
                    );
                }
                if assignment.model.is_none() {
                    return invalid_config(
                        format!("{path_prefix}.{path}.model"),
                        format!("{path_prefix}.{role} must declare model"),
                    );
                }
                if assignment.reasoning_effort.is_none() {
                    return invalid_config(
                        format!("{path_prefix}.{path}.reasoning-effort"),
                        format!("{path_prefix}.{role} must declare reasoning-effort"),
                    );
                }
            }
            validate_assignment_defaults(&path, assignment)?;
            validate_role_budget(role, &format!("{path_prefix}.{path}"), &assignment.budget)?;
        }
    }
    Ok(())
}

/// Validate one seat's declared budget (P475), reached from both a config
/// table (`field_prefix` = `agent.role.<path>`) and an `--assign
/// role=json:{...}` override (`field_prefix` = `run.assign.<selector>`) —
/// the same rules either way, since a budget can arrive through either
/// decoder: `frame-seconds`/`idle-seconds` of `0` are rejected outright
/// (mirroring the `merge.gate-seconds` zero-rejection precedent), and —
/// since `narrator`/`merger`/`merger-deep` are one-shot dispatches outside
/// the drive frame loop, with no idle timeout and no retry loop
/// (`run_one_shot`) — a declared `idle-seconds` or `max-retries` on one of
/// those three seats is rejected rather than silently accepted-and-ignored,
/// which would let an operator believe a limit is in force when it never
/// takes effect.
fn validate_role_budget(role: &str, field_prefix: &str, budget: &RoleBudget) -> crate::Result<()> {
    if budget.frame_seconds == Some(0) {
        return invalid_config(
            format!("{field_prefix}.budget.frame-seconds"),
            "must be at least 1",
        );
    }
    if budget.idle_seconds == Some(0) {
        return invalid_config(
            format!("{field_prefix}.budget.idle-seconds"),
            "must be at least 1",
        );
    }
    let one_shot = standing_seat_is_one_shot(role);
    if one_shot && budget.idle_seconds.is_some() {
        return invalid_config(
            format!("{field_prefix}.budget.idle-seconds"),
            format!("agent.role.{role} is a one-shot call: no idle budget"),
        );
    }
    if one_shot && budget.max_retries.is_some() {
        return invalid_config(
            format!("{field_prefix}.budget.max-retries"),
            format!("agent.role.{role} is a one-shot call: no retry loop"),
        );
    }
    Ok(())
}

/// Validate a standing seat's (`default`/`narrator`/`merger`/`merger-deep`)
/// table: a well-formed harness id and, if present, a whitespace-free
/// reasoning effort.
fn validate_special_assignment(assignment: &ProfileAssignment, path: &str) -> crate::Result<()> {
    if assignment.model_tier.is_some() {
        return invalid_config(
            format!("{path}.model-tier"),
            "model-tier applies only to declared trait roles",
        );
    }
    if let Some(harness) = assignment.harness.as_deref() {
        validate_bare_id(harness, &format!("{path}.harness"))?;
    }
    if let Some(effort) = assignment.reasoning_effort.as_deref() {
        validate_reasoning_effort(effort, &format!("{path}.reasoning-effort"))?;
    }
    Ok(())
}

fn validate_assignment(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    match assignment.mode {
        RunAssignmentMode::Attach => validate_attach_assignment(role, assignment)?,
        // 0079: an api-transport seat needs no harness declaration of its
        // own — its endpoint is `base-url`/`model`, validated by
        // `validate_api_transport` below. A harness may still be declared
        // alongside it as the missing-key/unavailable-endpoint fallback
        // (dispatch resolution owns that precedence), so it is not rejected
        // here either — only not required.
        RunAssignmentMode::Harness if assignment.transport == Some(RunTransport::Api) => {}
        RunAssignmentMode::Harness => {
            if assignment
                .harness
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                return invalid_config(
                    format!("assign.{role}.harness"),
                    "harness-mode assignment must declare harness",
                );
            }
        }
    }
    validate_assignment_common(role, assignment)
}

fn validate_assignment_defaults(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    if assignment.mode == RunAssignmentMode::Attach {
        validate_attach_assignment(role, assignment)?;
    }
    validate_assignment_common(role, assignment)
}

fn validate_attach_assignment(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    if assignment.harness.is_some()
        || assignment.transport.is_some()
        || assignment.session_mode.is_some()
        || assignment.model.is_some()
        || assignment.model_tier.is_some()
        || assignment.reasoning_effort.is_some()
        || assignment.system_prompt.is_some()
        || !assignment.extra_args.is_empty()
    {
        return invalid_config(
            format!("assign.{role}"),
            "attach-mode assignment must not declare harness, transport, session-mode, model, model-tier, reasoning-effort, system-prompt, or extra-args",
        );
    }
    Ok(())
}

fn validate_assignment_common(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    if role == "guide" && !assignment.extra_args.is_empty() {
        return invalid_config(
            format!("assign.{role}.extra-args"),
            "guide must not declare extra-args because its argv is tool-less",
        );
    }
    if let Some(effort) = assignment.reasoning_effort.as_deref() {
        validate_reasoning_effort(effort, &format!("assign.{role}.reasoning-effort"))?;
    }
    if assignment.transport == Some(RunTransport::Api) {
        validate_api_transport(role, assignment)?;
    }
    Ok(())
}

/// 0079: `transport = "api"` validation. The worker/driver seat
/// (`[agent.role.default]`) is rejected structurally here — not by
/// convention — because it is the one seat that needs tools, a filesystem,
/// and an agentic loop, which this one-shot transport does not provide. Every
/// other role requires `base-url` and a model, since a one-shot HTTP round
/// trip has no other way to find an endpoint or a model name.
fn validate_api_transport(role: &str, assignment: &ProfileAssignment) -> crate::Result<()> {
    if role == DEFAULT_SEAT {
        return invalid_config(
            format!("assign.{role}.transport"),
            "transport = \"api\" is not available on the worker/driver seat (agent.role.default); it needs tools and an agentic loop, which is what a harness is for",
        );
    }
    if assignment
        .api
        .base_url
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return invalid_config(
            format!("assign.{role}.base-url"),
            "transport = \"api\" requires base-url",
        );
    }
    if assignment
        .model
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return invalid_config(
            format!("assign.{role}.model"),
            "transport = \"api\" requires model",
        );
    }
    Ok(())
}

fn validate_reasoning_effort(value: &str, field_path: &str) -> crate::Result<()> {
    if value.trim().is_empty() {
        return invalid_config(field_path, "reasoning effort must not be empty");
    }
    if value.contains(char::is_whitespace) {
        return invalid_config(field_path, "reasoning effort must not contain whitespace");
    }
    Ok(())
}

/// The declared `[agent.role.<role>].budget` (P475), converted to the
/// ledger-persisted evidence shape — `None` when the seat declared no
/// budget field at all, so a run with no budgets anywhere in its config
/// records zero additional evidence bytes.
fn role_budget_evidence(
    budget: &RoleBudget,
) -> Option<ctx_traits_core::procedure::session::RoleBudgetEvidence> {
    if budget.is_empty() {
        return None;
    }
    Some(ctx_traits_core::procedure::session::RoleBudgetEvidence {
        frame_seconds: budget.frame_seconds,
        idle_seconds: budget.idle_seconds,
        max_retries: budget.max_retries,
    })
}

/// `seat_info` is `Some` only for a list-backed role (P456): it appends
/// `seat-index`/`list-length` fields so the selected seat is identifiable in
/// the evidence string. Absent entirely for a legacy single-table role, so
/// its evidence text is byte-identical to before this field existed.
fn assignment_evidence(
    role: &str,
    assignment: &ProfileAssignment,
    seat_info: Option<SeatInfo>,
    qualifier: Option<&str>,
) -> String {
    match assignment.mode {
        RunAssignmentMode::Attach => {
            let mut fields = vec!["mode=attach".to_string()];
            if let Some(seat) = seat_info {
                fields.push(format!("seat-index={}", seat.seat_index));
                fields.push(format!("list-length={}", seat.list_length));
            }
            if let Some(qualifier) = qualifier {
                fields.push(format!("config-qualifier={qualifier}"));
            }
            format!("assignment role={role} {}", fields.join(" "))
        }
        RunAssignmentMode::Harness => {
            let mut fields = vec![
                format!("role={role}"),
                "mode=harness".to_string(),
                format!("harness={}", assignment.harness.as_deref().unwrap_or("")),
                format!(
                    "transport={}",
                    assignment.transport.unwrap_or(RunTransport::Cli).as_str()
                ),
                format!(
                    "session-mode={}",
                    assignment.session_mode.unwrap_or_default().as_str()
                ),
            ];
            if let Some(model) = assignment.model.as_deref() {
                fields.push(format!("model={model}"));
            }
            if let Some(selector) = assignment.model_selector.as_deref() {
                fields.push(format!("model-selector={selector}"));
            }
            if let Some(reason) = assignment.model_resolution_reason {
                fields.push(format!("model-resolution={}", reason.as_str()));
            }
            if let Some(effort) = assignment.reasoning_effort.as_deref() {
                fields.push(format!("reasoning-effort={effort}"));
            }
            if let Some(prompt) = assignment.system_prompt.as_deref() {
                fields.push(format!(
                    "system-prompt-digest={}",
                    ctx_traits_core::digest::Digest::source(prompt)
                ));
            }
            if let Some(seat) = seat_info {
                fields.push(format!("seat-index={}", seat.seat_index));
                fields.push(format!("list-length={}", seat.list_length));
            }
            if let Some(qualifier) = qualifier {
                fields.push(format!("config-qualifier={qualifier}"));
            }
            format!("assignment {}", fields.join(" "))
        }
    }
}

fn resolve_assignment_model(
    registry: &HarnessRegistry,
    catalogs: &mut BTreeMap<String, ModelCatalogState>,
    capabilities: &mut Vec<ctx_traits_core::response::CapabilityReport>,
    mut assignment: ProfileAssignment,
) -> crate::Result<ProfileAssignment> {
    if assignment.mode == RunAssignmentMode::Attach {
        return Ok(assignment);
    }
    if assignment.transport == Some(RunTransport::Api) {
        // 0079: an api-transport seat's model is an opaque string handed
        // straight to the provider endpoint over HTTP — there is no harness
        // model catalog to resolve it against, and (per
        // `validate_api_transport`) no harness need be declared at all, so
        // this must not require one.
        return Ok(assignment);
    }
    let explicit_model = assignment.model.clone();
    let Some(model) = explicit_model.as_deref() else {
        return Ok(assignment);
    };
    let selector = ctx_traits_core::agent_model::Selector::Explicit(model);
    let harness_id = assignment.harness.as_deref().ok_or_else(|| {
        config_error(
            "model-resolution.harness",
            "model resolution requires a selected harness",
        )
    })?;
    let harness = registry.harness.get(harness_id).ok_or_else(|| {
        config_error(
            "model-resolution.harness",
            format!("unknown harness id {harness_id:?}"),
        )
    })?;
    ensure_model_catalog(harness_id, harness, catalogs, capabilities);
    let catalog = match catalogs
        .get(harness_id)
        .expect("ensured model catalog state must be present")
    {
        ModelCatalogState::Available(catalog) => {
            ctx_traits_core::agent_model::CatalogAccess::Available(catalog)
        }
        ModelCatalogState::Unavailable(reason) => {
            ctx_traits_core::agent_model::CatalogAccess::Unavailable(reason)
        }
    };
    let resolution = ctx_traits_core::agent_model::resolve_model(harness.kind(), selector, catalog)
        .map_err(|error| config_error("model-resolution", error.to_string()))?;
    assignment.model = Some(resolution.canonical_model);
    assignment.model_selector = Some(resolution.requested);
    assignment.model_resolution_reason = Some(resolution.reason);
    Ok(assignment)
}

fn ensure_model_catalog(
    harness_id: &str,
    harness: &HarnessDefinition,
    catalogs: &mut BTreeMap<String, ModelCatalogState>,
    capabilities: &mut Vec<ctx_traits_core::response::CapabilityReport>,
) {
    if catalogs.contains_key(harness_id) {
        return;
    }
    let capability = format!("runtime.model-catalog.{harness_id}");
    let state = match ctx_traits_core::agent_model::catalog_probe_plan(harness.kind()) {
        Ok(plan) => {
            let mut argv = Vec::with_capacity(plan.argv.len() + 1);
            argv.push(harness.bin().to_string());
            argv.extend(plan.argv);
            match crate::command::run(crate::command::RunRequest {
                argv: &argv,
                cwd: Some("project-root"),
                exec_dir: None,
                success_exit_code: &[0],
                timeout_ms: Some(10_000),
                idle_timeout_ms: None,
                capture_limit: 1024 * 1024,
                tick_observer: None,
            }) {
                Ok(output)
                    if output.success && !output.stdout_truncated && !output.stderr_truncated =>
                {
                    match ctx_traits_core::agent_model::parse_catalog(
                        harness.kind(),
                        &output.stdout,
                    ) {
                        Ok(catalog) => {
                            capabilities.push(
                                ctx_traits_core::response::CapabilityReport::supported(capability),
                            );
                            ModelCatalogState::Available(catalog)
                        }
                        Err(error) => {
                            unavailable_catalog(capability, error.to_string(), capabilities)
                        }
                    }
                }
                Ok(output) => unavailable_catalog(
                    capability,
                    format!(
                        "catalog probe failed: exit={:?} timed-out={} truncated={} stderr={}",
                        output.exit_code,
                        output.timed_out,
                        output.stdout_truncated || output.stderr_truncated,
                        output.stderr.trim()
                    ),
                    capabilities,
                ),
                Err(error) => unavailable_catalog(
                    capability,
                    format!("catalog probe failed: {error}"),
                    capabilities,
                ),
            }
        }
        Err(error) => unavailable_catalog(capability, error.to_string(), capabilities),
    };
    capabilities.sort();
    capabilities.dedup();
    catalogs.insert(harness_id.to_string(), state);
}

fn unavailable_catalog(
    capability: String,
    reason: String,
    capabilities: &mut Vec<ctx_traits_core::response::CapabilityReport>,
) -> ModelCatalogState {
    capabilities.push(ctx_traits_core::response::CapabilityReport::unsupported(
        capability,
        reason.clone(),
    ));
    ModelCatalogState::Unavailable(reason)
}

/// Result of a single harness binary version-probe invocation — the one
/// shared argv/timeout/capture-policy call both [`probe_harnesses`] (bulk
/// preparation, downgrades a failure to a warning) and [`probe_harness_version`]
/// (a single caller, promotes the same failure to an error) build on, so the
/// probe's own request shape can't drift between the two.
struct HarnessProbeOutcome {
    argv: Vec<String>,
    success: bool,
    exit_code: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

fn run_harness_probe(harness: &HarnessDefinition) -> crate::Result<HarnessProbeOutcome> {
    let mut argv = Vec::with_capacity(harness.version_probe.len() + 1);
    argv.push(harness.bin().to_string());
    argv.extend(harness.version_probe.clone());
    let outcome = crate::command::run(crate::command::RunRequest {
        argv: &argv,
        cwd: Some("project-root"),
        exec_dir: None,
        success_exit_code: &[0],
        timeout_ms: Some(10_000),
        idle_timeout_ms: None,
        capture_limit: 4096,
        tick_observer: None,
    })?;
    Ok(HarnessProbeOutcome {
        argv,
        success: outcome.success,
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        stdout: outcome.stdout,
        stderr: outcome.stderr,
    })
}

fn probe_harnesses(
    registry: &HarnessRegistry,
    harness_ids: &BTreeSet<String>,
) -> (
    Vec<ctx_traits_core::procedure::session::HarnessProbeEvidence>,
    Vec<String>,
    Vec<ctx_traits_core::response::CapabilityReport>,
) {
    let mut probes = Vec::new();
    let mut warnings = Vec::new();
    let mut capabilities = Vec::new();
    for harness_id in harness_ids {
        let Some(harness) = registry.harness.get(harness_id) else {
            continue;
        };
        match run_harness_probe(harness) {
            Ok(outcome) if outcome.success => {
                let version = if outcome.stdout.trim().is_empty() {
                    outcome.stderr.trim().to_string()
                } else {
                    outcome.stdout.trim().to_string()
                };
                probes.push(ctx_traits_core::procedure::session::HarnessProbeEvidence {
                    harness_id: harness_id.clone(),
                    bin: harness.bin().to_string(),
                    version,
                });
                capabilities.push(ctx_traits_core::response::CapabilityReport::supported(
                    format!("runtime.harness-probe.{harness_id}"),
                ));
            }
            Ok(outcome) => {
                let reason = format!(
                    "harness {harness_id} probe failed: exit={:?} timed-out={} stderr={}",
                    outcome.exit_code,
                    outcome.timed_out,
                    outcome.stderr.trim()
                );
                warnings.push(reason.clone());
                capabilities.push(ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-probe.{harness_id}"),
                    reason,
                ));
            }
            Err(err) => {
                let reason = format!("harness {harness_id} probe failed: {err}");
                warnings.push(reason.clone());
                capabilities.push(ctx_traits_core::response::CapabilityReport::unsupported(
                    format!("runtime.harness-probe.{harness_id}"),
                    reason,
                ));
            }
        }
    }
    (probes, warnings, capabilities)
}

/// Probe a single harness's binary before any Git-mutating step, failing
/// loudly (unlike `probe_harnesses`, which only warns): a caller like
/// `ctx traits merge` must discover a missing/broken merger binary before it
/// rebases anything, not mid-mutation.
pub fn probe_harness_version(harness: &HarnessDefinition) -> crate::Result<()> {
    let outcome = run_harness_probe(harness)?;
    if !outcome.success {
        return Err(crate::environment::Error::Process {
            command: Some(outcome.argv.join(" ")),
            path: None,
            exit_status: outcome.exit_code,
            timed_out: outcome.timed_out,
            message: format!("harness probe failed: {}", outcome.stderr.trim()),
        }
        .into());
    }
    Ok(())
}

/// P427 zero-config runtime fallback: harness definitions compiled into the
/// binary, in the fixed candidate order automatic selection tries them.
///
/// `claude-code` carries this phase's verified Claude Code CLI conventions:
/// print-mode stream JSON with partial messages
/// (`-p --output-format stream-json --include-partial-messages --verbose`),
/// system-prompt injection via `--append-system-prompt`, and the Agent tool
/// disabled for a dispatched worker via `--disallowedTools Agent`.
///
/// `opencode` carries `run --format json --auto`, directory pinning via
/// `--dir` (`dir-flag`, needed because opencode resolves its project from a
/// running server rather than the spawned process cwd), and effort routing
/// via `--variant` (`reasoning-effort-flag` — opencode's per-invocation
/// agent-variant selector, the closest existing typed field to the phase's
/// named flag).
///
/// `pi` (the `@earendil-works/pi-coding-agent` CLI) carries its verified
/// headless JSON-event contract: `--mode json` selects JSON event output and
/// `--print` makes the positional prompt non-interactive. `--model` and
/// `--thinking` select model and reasoning effort, `--append-system-prompt`
/// appends text or file contents to the system prompt (verified against
/// pi's own `--help`, distinct from `--system-prompt`, which replaces it —
/// `--append-system-prompt` is what the other two built-ins' single
/// standing-instructions layer needs), and `--session <id/path>` resumes a
/// specific session. Positional arguments are delivered as the message body
/// (`prompt-via: "arg"`), matching the other two built-ins. Pi has no
/// published JSON-schema-enforcement flag — that is tracked upstream as an
/// open feature request, not a shipped contract — so `json_schema_flag`
/// stays unset rather than guessed, per this phase's scope. Pi's
/// `--mode json` stream is one JSON object per line: a `session` header
/// event (`{"type":"session",...}`), then one event per turn/tool
/// step, including `message_end` events carrying the complete
/// `{"role":"assistant","content":[{"type":"text","text":...}],...}`
/// message — the same shape Claude Code and OpenCode's own event streams
/// nest their answer text/JSON under, so
/// `modules/cli/src/app/harness_stream.rs`'s
/// `GENERIC_STREAM_JSON_OUTPUTS`-gated decoder (which already walks a
/// `message`/`content` wrapper chain looking for the requested slot keys)
/// handles it too, with no Pi-specific parser.
///
/// `codex` carries the Codex CLI 0.146.0 non-interactive contract: `exec`
/// selects the non-interactive command, `--json` emits JSONL events, and the
/// documented `approval_policy="never"` config override prevents a headless
/// frame from waiting for confirmation. Codex accepts `--model` and `--cd`
/// directly, but its
/// reasoning uses `--config model_reasoning_effort="<value>"`; the dispatch
/// renderer recognizes Codex's `--config` mapping so the resolved effort is
/// applied to every invocation. Developer-instruction settings cannot be
/// represented by this flag/value convention and use the existing prompt
/// fallback. Its `--output-schema` expects a file path, not
/// the inline schema this convention supplies, so schema enforcement likewise
/// remains in the prompt contract. `exec resume` has positional session and
/// prompt arguments, so correction retries visibly cold-start rather than
/// invoking it with a guessed argv shape. `agents.enabled=false` disables
/// legacy subagents and `features.multi_agent_v2=false` prevents the V2
/// override, leaving both multi-agent implementations disabled.
fn built_in_harness_definitions() -> Vec<(&'static str, HarnessDefinition)> {
    vec![
        (
            "claude-code",
            HarnessDefinition {
                kind: Some("claude-code".to_string()),
                bin: Some("claude".to_string()),
                transports: vec![RunTransport::Cli, RunTransport::Mcp],
                version_probe: vec!["--version".to_string()],
                cli: Some(HarnessCliConvention {
                    // `--dangerously-skip-permissions` is REQUIRED, not a
                    // convenience: a driven frame is headless and cannot answer
                    // a permission prompt, so without it every claude-code
                    // frame stalls until it times out. Safety comes from ctx's
                    // own write confinement (P478/P480), which bounds the
                    // process regardless of what the harness would have asked.
                    // `--disallowedTools Agent`: the frame IS the unit of work;
                    // a subagent re-adds a layer budgets/narration/receipts
                    // cannot see.
                    argv: [
                        "-p",
                        "--dangerously-skip-permissions",
                        "--output-format",
                        "stream-json",
                        "--include-partial-messages",
                        "--verbose",
                        "--disallowedTools",
                        "Agent",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                    // Standing one-shots (narrator and guide) have no tools.
                    // `*` denies every built-in tool and `mcp__*` every MCP
                    // tool. Keep their output compatible with the shared
                    // Claude stream parser rather than accepting plain text.
                    narrator_argv: Some(
                        [
                            "-p",
                            "--output-format",
                            "stream-json",
                            "--verbose",
                            "--disallowedTools",
                            "*",
                            "mcp__*",
                        ]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                    ),
                    // Deliberately absent. A warm process is spawned once and
                    // cannot carry a per-frame `--json-schema`, so shipping a
                    // warm convention here would silently trade typed-output
                    // enforcement away for every user. A repo that wants
                    // persistent claude-code sessions opts in by declaring
                    // `warm-argv` AND unsetting `json-schema-flag = ""`.
                    warm_argv: None,
                    json_schema_flag: Some("--json-schema".to_string()),
                    model_flag: Some("--model".to_string()),
                    // P568: folded up from repo config. Without it a seat's
                    // declared `reasoning-effort` is silently dropped — the
                    // value is resolved and then has nowhere to go, which is
                    // the same silent-omission class the merge model exists to
                    // end. The flag is claude-code's own; declaring it changes
                    // nothing for a seat that sets no effort.
                    reasoning_effort_flag: Some("--effort".to_string()),
                    system_prompt_flag: Some("--append-system-prompt".to_string()),
                    resume_flag: Some("--resume".to_string()),
                    session_flag: None,
                    dir_flag: None,
                    prompt_via: Some("arg".to_string()),
                    stream: Some(true),
                    output: Some("claude-stream-json".to_string()),
                }),
                mcp: Some(HarnessMcpConvention {
                    mcp_config_flag: Some("--mcp-config".to_string()),
                    allowed_tools_flag: Some("--allowedTools".to_string()),
                    allowed_tools: vec!["mcp__ctx__*".to_string()],
                    system_prompt_flag: Some("--append-system-prompt".to_string()),
                    reasoning_effort_flag: None,
                    config_via: None,
                }),
            },
        ),
        (
            "opencode",
            HarnessDefinition {
                kind: Some("opencode".to_string()),
                bin: Some("opencode".to_string()),
                transports: vec![RunTransport::Cli],
                version_probe: vec!["--version".to_string()],
                cli: Some(HarnessCliConvention {
                    // `--auto` is opencode's headless parity with claude's
                    // permission bypass, and required for the same reason;
                    // explicit denies in opencode's own config still win.
                    // `--thinking` surfaces reasoning in the stream, which the
                    // live view and narration read.
                    argv: ["run", "--format", "json", "--thinking", "--auto"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                    // `--pure` drops tools, MCP and plugins for the one-shot
                    // narration call. Deliberately NOT `--agent <name>`: that
                    // would bind every user to a repo-local agent file, which
                    // is exactly the dependency this default exists to remove.
                    narrator_argv: Some(
                        ["run", "--format", "json", "--pure"]
                            .into_iter()
                            .map(String::from)
                            .collect(),
                    ),
                    warm_argv: None,
                    json_schema_flag: None,
                    model_flag: Some("--model".to_string()),
                    reasoning_effort_flag: Some("--variant".to_string()),
                    system_prompt_flag: None,
                    resume_flag: None,
                    session_flag: Some("--session".to_string()),
                    dir_flag: Some("--dir".to_string()),
                    prompt_via: Some("arg".to_string()),
                    stream: Some(true),
                    output: Some("opencode-json".to_string()),
                }),
                mcp: None,
            },
        ),
        (
            "pi",
            HarnessDefinition {
                kind: Some("pi".to_string()),
                bin: Some("pi".to_string()),
                transports: vec![RunTransport::Cli],
                version_probe: vec!["--version".to_string()],
                cli: Some(HarnessCliConvention {
                    argv: ["--mode", "json", "--print"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                    narrator_argv: None,
                    warm_argv: None,
                    json_schema_flag: None,
                    model_flag: Some("--model".to_string()),
                    reasoning_effort_flag: Some("--thinking".to_string()),
                    system_prompt_flag: Some("--append-system-prompt".to_string()),
                    resume_flag: None,
                    session_flag: Some("--session".to_string()),
                    dir_flag: None,
                    prompt_via: Some("arg".to_string()),
                    stream: Some(true),
                    // Pi emits NDJSON events, including a `session` header and
                    // a completed assistant message. `raw-json` accepts only
                    // one document and would discard that stream structure.
                    output: Some("pi-json".to_string()),
                }),
                mcp: None,
            },
        ),
        (
            "codex",
            HarnessDefinition {
                kind: Some("codex".to_string()),
                bin: Some("codex".to_string()),
                transports: vec![RunTransport::Cli],
                version_probe: vec!["--version".to_string()],
                cli: Some(HarnessCliConvention {
                    argv: [
                        "exec",
                        "--json",
                        "--config",
                        "approval_policy=\"never\"",
                        "--config",
                        "agents.enabled=false",
                        "--config",
                        "features.multi_agent_v2=false",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                    narrator_argv: None,
                    warm_argv: None,
                    json_schema_flag: None,
                    model_flag: Some("--model".to_string()),
                    reasoning_effort_flag: Some("--config".to_string()),
                    system_prompt_flag: None,
                    resume_flag: None,
                    session_flag: None,
                    dir_flag: Some("--cd".to_string()),
                    prompt_via: Some("arg".to_string()),
                    stream: Some(true),
                    output: Some("codex-json".to_string()),
                }),
                mcp: None,
            },
        ),
    ]
}

/// Fixed candidate order the built-in registry is tried in, shared by
/// detection and doctor reporting so both agree on `order` without
/// duplicating the id list.
/// P568: replace every configured built-in harness entry with its merge over
/// the compiled-in definition. Idempotency is NOT guaranteed, so this runs
/// exactly once, at config resolution.
fn merge_built_in_harness_overrides(harness: &mut BTreeMap<String, HarnessDefinition>) {
    for (id, base) in built_in_harness_definitions() {
        // Merge an override onto the built-in, or MATERIALIZE the built-in
        // when nothing overrides it. Merging only the configured ids left the
        // resolved registry holding just what the file named, so once the
        // conventions moved into the built-ins an empty `[harness]` section
        // meant every lookup failed with "unknown harness id" — the registry
        // is the one place that knows a harness exists, so it has to carry
        // all of them.
        let resolved = match harness.get(id) {
            Some(configured) => configured.merged_onto(&base),
            None => base,
        };
        harness.insert(id.to_string(), resolved);
    }
}

/// The compiled-in harness ids, in registry order. Public so a reporting
/// surface can tell a built-in id (whose config table MERGES over a compiled-in
/// definition, P568) from a purely custom one (whose table stands alone).
pub fn built_in_harness_ids() -> Vec<&'static str> {
    built_in_harness_definitions()
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// The effective definition for a built-in harness id: a same-id
/// `[harness.<id>]` entry in `configured` is MERGED over the compiled-in
/// definition field by field, otherwise the compiled-in definition is used
/// as-is.
///
/// P568 replaces P427's wholesale-replace precedence. Under replace, changing
/// one flag meant restating the entire definition, and any field you forgot
/// was silently dropped — a config could disable streaming or a model flag by
/// omission alone. Merge means a table states only what differs.
///
/// The merge is SHALLOW (see [`HarnessDefinition::merged_onto`]): a stated
/// field replaces the built-in's value outright rather than combining with
/// it, so `narrator-argv` never concatenates.
///
/// An inherited field can now contradict one the config states — the
/// `warm-argv` × `json-schema-flag` exclusion is the live example. That is a
/// HARD ERROR from the ordinary validation pass over the merged definition,
/// never a silent drop of either side: the config must say which it wants.
pub fn built_in_harness_definition(id: &str, configured: &HarnessRegistry) -> HarnessDefinition {
    // Entries arriving here are already merged by
    // `merge_built_in_harness_overrides` at config-resolution time, so this is
    // a plain lookup. Merging again would NOT be idempotent: an explicit unset
    // (`flag = ""`) has already been applied to `None`, and a second pass would
    // re-inherit the built-in's value.
    if let Some(overridden) = configured.harness.get(id) {
        return overridden.clone();
    }
    built_in_harness_definitions()
        .into_iter()
        .find(|(candidate, _)| *candidate == id)
        .map(|(_, definition)| definition)
        .unwrap_or_else(|| {
            unreachable!("built_in_harness_definition called with a non-built-in id")
        })
}

/// One row of the P427 built-in-harness PATH-detection table: shared by
/// automatic fallback selection and `ctx traits doctor --config`'s
/// plain/JSON detection table, so both surfaces report exactly the same
/// evidence from the same probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinHarnessDetection {
    pub id: String,
    pub bin: String,
    /// 0-based position in the fixed candidate order.
    pub order: usize,
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
    /// `true` when a same-id `[harness.<id>]` table in `configured` replaces
    /// this built-in's compiled-in definition (P427 precedence).
    pub overridden: bool,
}

/// Probe every built-in harness candidate once, in fixed order, using the
/// same bounded version-probe execution [`probe_harnesses`] uses for
/// configured harnesses. Detection alone never writes or mutates
/// configuration; it only reports what is already true of `PATH` and
/// `configured`.
pub fn detect_builtin_harnesses(configured: &HarnessRegistry) -> Vec<BuiltinHarnessDetection> {
    built_in_harness_ids()
        .into_iter()
        .enumerate()
        .map(|(order, id)| {
            let overridden = configured.harness.contains_key(id);
            let definition = built_in_harness_definition(id, configured);
            let bin = definition.bin().to_string();
            match run_harness_probe(&definition) {
                Ok(outcome) if outcome.success => {
                    let version = if outcome.stdout.trim().is_empty() {
                        outcome.stderr.trim().to_string()
                    } else {
                        outcome.stdout.trim().to_string()
                    };
                    BuiltinHarnessDetection {
                        id: id.to_string(),
                        bin,
                        order,
                        available: true,
                        version: Some(version),
                        error: None,
                        overridden,
                    }
                }
                Ok(outcome) => BuiltinHarnessDetection {
                    id: id.to_string(),
                    bin,
                    order,
                    available: false,
                    version: None,
                    error: Some(format!(
                        "exit={:?} timed-out={} stderr={}",
                        outcome.exit_code,
                        outcome.timed_out,
                        outcome.stderr.trim()
                    )),
                    overridden,
                },
                Err(err) => BuiltinHarnessDetection {
                    id: id.to_string(),
                    bin,
                    order,
                    available: false,
                    version: None,
                    error: Some(err.to_string()),
                    overridden,
                },
            }
        })
        .collect()
}

/// The single-line, copy-pasteable no-candidate remediation (P427): every
/// probed built-in binary, plus a `printf`-and-append shell command that
/// writes a concrete, valid `.ctx/config.toml` snippet pinning `role` to the
/// first probed candidate's built-in id. Uses dotted-key TOML
/// (`agent.role.<role>.harness = "<id>"`) rather than a `[table]` header
/// followed by a key on the same line — the latter is not valid single-line
/// TOML — so running the emitted command actually clears the error it names.
/// Deliberately emits NO `[harness.<id>]` table: naming a built-in id with no
/// matching table leaves [`built_in_harness_definition`]'s compiled-in
/// definition in effect exactly as-is, so the pinned role dispatches through
/// the SAME verified argv/output convention this phase's other built-ins
/// prove end to end — never an invented custom-harness stand-in that only
/// looks complete. The remaining, real prerequisite — the named binary
/// actually being reachable on `PATH` — is the "install {names}" half of the
/// same message. Kept to one shell line (no literal `\n`; `printf`'s own
/// `\n` escapes become real newlines in the file it writes) so the emitted
/// command stays copy-pasteable.
pub fn no_builtin_harness_message(rows: &[BuiltinHarnessDetection], role: &str) -> String {
    let probed = rows
        .iter()
        .map(|row| format!("{}({})", row.id, row.bin))
        .collect::<Vec<_>>()
        .join(", ");
    let names = rows
        .iter()
        .map(|row| row.bin.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let example_id = rows
        .first()
        .map(|row| row.id.as_str())
        .unwrap_or("claude-code");
    let role_key = format!("agent.role.{role}.harness");
    format!(
        "no built-in harness found on PATH (probed {probed}); install {names}, then pin the role to it, or run: printf '{role_key} = \"{example_id}\"\\n' >> .ctx/config.toml"
    )
}

fn normalize_role(role: &str, field_path: &str) -> crate::Result<String> {
    if role.trim().is_empty() {
        return invalid_config(field_path, "assignment role must not be empty");
    }
    if let Some(stripped) = role.strip_prefix("agent:") {
        validate_bare_id(stripped, field_path)?;
        return Ok(stripped.to_string());
    }
    validate_bare_id(role, field_path)?;
    Ok(role.to_string())
}

fn validate_bare_id(id: &str, field_path: &str) -> crate::Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return invalid_config(
            field_path,
            "id must contain only letters, digits, '-' or '_'",
        );
    }
    Ok(())
}

fn invalid_config<T>(
    field_path: impl Into<String>,
    message: impl Into<String>,
) -> crate::Result<T> {
    Err(config_error(field_path, message))
}

fn config_error(field_path: impl Into<String>, message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: field_path.into(),
            message: message.into(),
        }
        .into(),
    )
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn pi_built_in_definition_uses_valid_ndjson_convention() {
        let definition = built_in_harness_definition("pi", &HarnessRegistry::default());
        let cli = definition.cli.as_ref().expect("pi has a CLI convention");

        assert_eq!(definition.kind(), "pi");
        assert_eq!(definition.bin(), "pi");
        assert_eq!(
            cli.argv,
            ["--mode", "json", "--print"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(cli.model_flag.as_deref(), Some("--model"));
        assert_eq!(cli.reasoning_effort_flag.as_deref(), Some("--thinking"));
        assert_eq!(
            cli.system_prompt_flag.as_deref(),
            Some("--append-system-prompt")
        );
        assert_eq!(cli.session_flag.as_deref(), Some("--session"));
        assert_eq!(cli.prompt_via.as_deref(), Some("arg"));
        assert_eq!(cli.output.as_deref(), Some("pi-json"));
        assert!(cli.json_schema_flag.is_none());
        assert!(cli.dir_flag.is_none());

        let registry = HarnessRegistry {
            harness: BTreeMap::from([("pi".to_string(), definition)]),
            ..HarnessRegistry::default()
        };
        validate_registry(&registry).expect("the Pi built-in convention is valid");
    }

    #[test]
    fn codex_built_in_definition_uses_verified_exec_convention() {
        let definition = built_in_harness_definition("codex", &HarnessRegistry::default());
        let cli = definition.cli.as_ref().expect("Codex has a CLI convention");

        assert_eq!(
            built_in_harness_ids(),
            ["claude-code", "opencode", "pi", "codex"]
        );
        assert_eq!(definition.kind(), "codex");
        assert_eq!(definition.bin(), "codex");
        assert_eq!(
            cli.argv,
            [
                "exec",
                "--json",
                "--config",
                "approval_policy=\"never\"",
                "--config",
                "agents.enabled=false",
                "--config",
                "features.multi_agent_v2=false",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
        assert_eq!(cli.model_flag.as_deref(), Some("--model"));
        assert_eq!(cli.dir_flag.as_deref(), Some("--cd"));
        assert_eq!(cli.prompt_via.as_deref(), Some("arg"));
        assert_eq!(cli.output.as_deref(), Some("codex-json"));
        assert!(cli.json_schema_flag.is_none());
        assert_eq!(cli.reasoning_effort_flag.as_deref(), Some("--config"));
        assert!(cli.system_prompt_flag.is_none());
        assert!(cli.resume_flag.is_none());
        assert!(cli.session_flag.is_none());

        let registry = HarnessRegistry {
            harness: BTreeMap::from([("codex".to_string(), definition)]),
            ..HarnessRegistry::default()
        };
        validate_registry(&registry).expect("the Codex built-in convention is valid");
    }

    /// P564: the `{worktree}` token is the only overlay form that yields a
    /// per-run path. Everything else in this test asserts the forms it must
    /// NOT disturb — the shared-cache and repo-relative rules P342/P426/P428
    /// depend on.
    #[test]
    fn worktree_token_resolves_per_run_and_leaves_shared_forms_alone() {
        let repo_root = Utf8Path::new("/repo");
        let worktree = Utf8Path::new("/repo/.ctx/worktrees/wt-abc");
        let mut env = BTreeMap::new();
        env.insert(
            "CARGO_TARGET_DIR".to_string(),
            "{worktree}/target".to_string(),
        );
        env.insert("WORKTREE_ROOT".to_string(), "{worktree}".to_string());
        env.insert("REPO_RELATIVE".to_string(), "./tools".to_string());
        env.insert("PLAIN".to_string(), "verbatim".to_string());

        let resolved = resolve_worktree_env_overlay(&env, repo_root, Some(worktree))
            .expect("overlay resolves");
        assert_eq!(
            resolved.get("CARGO_TARGET_DIR").map(String::as_str),
            Some("/repo/.ctx/worktrees/wt-abc/target")
        );
        assert_eq!(
            resolved.get("WORKTREE_ROOT").map(String::as_str),
            Some("/repo/.ctx/worktrees/wt-abc")
        );
        assert_eq!(
            resolved.get("REPO_RELATIVE").map(String::as_str),
            Some("/repo/./tools"),
            "a repository-relative value still resolves against the invocation checkout"
        );
        assert_eq!(
            resolved.get("PLAIN").map(String::as_str),
            Some("verbatim"),
            "a non-path scalar is never guess-detected as a path"
        );

        // With no worktree in play the token names nothing, so the entry is
        // dropped rather than resolved against a fallback that would point a
        // host-side build at some other run's worktree.
        let without =
            resolve_worktree_env_overlay(&env, repo_root, None).expect("overlay resolves");
        assert!(!without.contains_key("CARGO_TARGET_DIR"));
        assert!(!without.contains_key("WORKTREE_ROOT"));
        assert_eq!(without.get("PLAIN").map(String::as_str), Some("verbatim"));
    }

    /// 0057: with no worktree there is no lease to take, so the entry is
    /// DROPPED rather than resolved — a host-side command must fall back to
    /// its own default, never borrow a slot a live run may be building in.
    #[test]
    fn a_cache_slot_overlay_is_dropped_when_no_worktree_is_in_play() {
        let mut env = BTreeMap::new();
        env.insert("CARGO_TARGET_DIR".to_string(), "{cache-slot}".to_string());
        let resolved =
            resolve_worktree_env_overlay(&env, Utf8Path::new("/repo"), None).expect("resolve");
        assert!(
            resolved.is_empty(),
            "a slot must never be leased for a spawn that has no worktree: {resolved:?}"
        );
    }

    /// A traversal would climb out of the leased slot and back into a shared
    /// path — reintroducing the cross-run collision the lease exists to end.
    #[test]
    fn a_cache_slot_overlay_refuses_a_traversal_segment() {
        let mut env = BTreeMap::new();
        env.insert(
            "CARGO_TARGET_DIR".to_string(),
            "{cache-slot}/../shared".to_string(),
        );
        let error = resolve_worktree_env_overlay(&env, Utf8Path::new("/repo"), None)
            .expect_err("traversal must be refused at resolution");
        assert!(
            error.to_string().contains(".."),
            "the refusal must name the traversal: {error}"
        );
    }

    /// A traversal out of the worktree lands back in the shared checkout,
    /// which is the collision this token exists to end.
    #[test]
    fn worktree_token_refuses_traversal_out_of_the_worktree() {
        let mut env = BTreeMap::new();
        env.insert(
            "CARGO_TARGET_DIR".to_string(),
            "{worktree}/../target".to_string(),
        );
        let error = resolve_worktree_env_overlay(
            &env,
            Utf8Path::new("/repo"),
            Some(Utf8Path::new("/repo/.ctx/worktrees/wt-abc")),
        )
        .expect_err("traversal is refused");
        assert!(
            error.to_string().contains(".."),
            "the refusal names the offending segment: {error}"
        );
    }

    #[test]
    fn validate_build_cache_rejects_duplicate_env_and_bad_names() {
        let mut caches = BTreeMap::new();
        caches.insert(
            "cargo".to_string(),
            BuildCacheConfig {
                env: "CARGO_TARGET_DIR".to_string(),
            },
        );
        assert!(validate_build_cache(&caches).is_ok());

        caches.insert(
            "cargo2".to_string(),
            BuildCacheConfig {
                env: "CARGO_TARGET_DIR".to_string(),
            },
        );
        let error = validate_build_cache(&caches).unwrap_err().to_string();
        assert!(error.contains("CARGO_TARGET_DIR"), "{error}");

        let mut bad_name = BTreeMap::new();
        bad_name.insert(
            "../escape".to_string(),
            BuildCacheConfig {
                env: "X".to_string(),
            },
        );
        assert!(validate_build_cache(&bad_name).is_err());

        let mut empty_env = BTreeMap::new();
        empty_env.insert("cargo".to_string(), BuildCacheConfig { env: String::new() });
        assert!(validate_build_cache(&empty_env).is_err());
    }

    #[test]
    fn overlay_run_table_merges_named_build_caches_by_name() {
        let mut base = RunTable::default();
        let mut winners = BTreeMap::new();
        let mut repo_layer = RunTable::default();
        repo_layer.build_cache.insert(
            "cargo".to_string(),
            BuildCacheConfig {
                env: "CARGO_TARGET_DIR".to_string(),
            },
        );
        overlay_run_table(
            &mut base,
            repo_layer,
            ConfigLayer::Repo,
            Some("repo".into()),
            &mut winners,
        );
        let mut global_layer = RunTable::default();
        global_layer.build_cache.insert(
            "pnpm".to_string(),
            BuildCacheConfig {
                env: "PNPM_HOME".to_string(),
            },
        );
        overlay_run_table(
            &mut base,
            global_layer,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );

        assert_eq!(base.build_cache.len(), 2);
        assert_eq!(base.build_cache["cargo"].env, "CARGO_TARGET_DIR");
        assert_eq!(base.build_cache["pnpm"].env, "PNPM_HOME");
        assert_eq!(
            winners["run.build-cache.cargo"].source.as_deref(),
            Some("repo")
        );
        assert_eq!(
            winners["run.build-cache.pnpm"].source.as_deref(),
            Some("global")
        );
    }

    #[test]
    fn combined_worktree_env_derives_cache_paths_and_prefers_explicit_overlay() {
        let mut worktree = WorktreeConfig::default();
        worktree.build_cache.insert(
            "cargo".to_string(),
            BuildCacheConfig {
                env: "CARGO_TARGET_DIR".to_string(),
            },
        );
        worktree.build_cache.insert(
            "pnpm".to_string(),
            BuildCacheConfig {
                env: "PNPM_HOME".to_string(),
            },
        );
        let combined = combined_worktree_env(&worktree);
        assert_eq!(
            combined.get("CARGO_TARGET_DIR").map(String::as_str),
            Some(".ctx/cache/build/cargo")
        );
        assert_eq!(
            combined.get("PNPM_HOME").map(String::as_str),
            Some(".ctx/cache/build/pnpm")
        );

        worktree.env.insert(
            "CARGO_TARGET_DIR".to_string(),
            "./custom-target".to_string(),
        );
        let combined = combined_worktree_env(&worktree);
        assert_eq!(
            combined.get("CARGO_TARGET_DIR").map(String::as_str),
            Some("./custom-target"),
            "an explicit [worktree.env] entry must win over a same-named cache export"
        );
    }

    #[test]
    fn project_retention_policy_survives_absent_layers_and_records_winners() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        let mut configured = RuntimeConfig::default();
        configured.worktree.retention = WorktreeRetentionConfig {
            cheap: vec!["target/incremental".to_string()],
            expensive: vec!["target".to_string()],
            expensive_grace_days: Some(3),
        };
        configured.authored_requirements = [
            ConfigLeaf::WorktreeRetentionCheap,
            ConfigLeaf::WorktreeRetentionExpensive,
            ConfigLeaf::WorktreeRetentionExpensiveGraceDays,
        ]
        .into_iter()
        .map(|leaf| {
            (
                leaf,
                AuthoredConfigLeaf {
                    semantic: ConfigSemantic::Requirement,
                    value: toml::Value::String(String::new()),
                },
            )
        })
        .collect();
        merge_project_config(
            &mut effective,
            configured,
            ConfigLayer::Repo,
            Some("repo".to_string()),
            &mut winners,
        );
        merge_project_config(
            &mut effective,
            RuntimeConfig::default(),
            ConfigLayer::UserGlobal,
            Some("global".to_string()),
            &mut winners,
        );
        assert_eq!(
            effective.worktree.retention.cheap,
            vec!["target/incremental".to_string()]
        );
        assert_eq!(
            effective.worktree.retention.expensive,
            vec!["target".to_string()]
        );
        assert_eq!(effective.worktree.retention.expensive_grace_days, Some(3));
        assert_eq!(
            winners["worktree.retention.cheap"].source.as_deref(),
            Some("repo")
        );
    }

    /// 0063.4 regression: `[tasks] dispatch-trait` parsed from a repo config
    /// file must survive the project merge — it was only applied on the
    /// `CTX_CONFIG` environment path, so a configured board dispatch still
    /// opened the modal as unconfigured.
    #[test]
    fn tasks_dispatch_trait_survives_the_project_merge_from_a_repo_layer() {
        let configured: RuntimeConfig =
            toml::from_str("[tasks]\ndispatch-trait = \"implement:quick\"\n").unwrap();
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_project_config(
            &mut effective,
            configured,
            ConfigLayer::Repo,
            Some("repo".to_string()),
            &mut winners,
        );
        assert_eq!(
            effective.effective_dispatch_trait().as_deref(),
            Some("implement:quick")
        );
        assert_eq!(
            winners["tasks.dispatch-trait"].source.as_deref(),
            Some("repo")
        );

        // A later layer without a `[tasks]` table must not clear it.
        merge_project_config(
            &mut effective,
            RuntimeConfig::default(),
            ConfigLayer::UserGlobal,
            Some("global".to_string()),
            &mut winners,
        );
        assert_eq!(
            effective.effective_dispatch_trait().as_deref(),
            Some("implement:quick")
        );
    }

    /// 0144, following the `tasks.dispatch-trait` precedent above: `[tasks]
    /// auto-close` parsed from a repo config file must survive the project
    /// merge from the repo layer.
    #[test]
    fn tasks_auto_close_survives_the_project_merge_from_a_repo_layer() {
        let configured: RuntimeConfig =
            toml::from_str("[tasks]\nauto-close = \"checked\"\n").unwrap();
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_project_config(
            &mut effective,
            configured,
            ConfigLayer::Repo,
            Some("repo".to_string()),
            &mut winners,
        );
        assert_eq!(
            effective.effective_auto_close(),
            Some(ctx_traits_core::task::AutoClosePolicy::Checked)
        );
        assert_eq!(winners["tasks.auto-close"].source.as_deref(), Some("repo"));

        // A later layer without a `[tasks]` table must not clear it.
        merge_project_config(
            &mut effective,
            RuntimeConfig::default(),
            ConfigLayer::UserGlobal,
            Some("global".to_string()),
            &mut winners,
        );
        assert_eq!(
            effective.effective_auto_close(),
            Some(ctx_traits_core::task::AutoClosePolicy::Checked)
        );
    }

    #[test]
    fn machine_global_role_replaces_repo_suggestion() {
        let mut repo = RuntimeConfig::default();
        repo.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(ProfileAssignment {
                harness: Some("repo".into()),
                ..ProfileAssignment::default()
            }),
        );
        let mut global = RuntimeConfig::default();
        global.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(ProfileAssignment {
                harness: Some("global".into()),
                ..ProfileAssignment::default()
            }),
        );
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_machine_config(
            &mut effective,
            repo,
            ConfigLayer::Repo,
            Some("repo".into()),
            &mut winners,
        );
        merge_machine_config(
            &mut effective,
            global,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );
        assert_eq!(
            effective.agent.role["worker"].entries()[0]
                .harness
                .as_deref(),
            Some("global")
        );
        assert_eq!(
            winners["agent.role.worker.harness"].layer,
            ConfigLayer::UserGlobal
        );
    }

    #[test]
    fn project_run_values_replace_global_values() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        for (layer, value) in [(ConfigLayer::UserGlobal, 2), (ConfigLayer::Repo, 4)] {
            merge_project_config(
                &mut effective,
                RuntimeConfig {
                    run: Some(RunTable {
                        max_in_flight: Some(value),
                        ..RunTable::default()
                    }),
                    ..RuntimeConfig::default()
                },
                layer,
                Some(format!("{layer:?}")),
                &mut winners,
            );
        }
        assert_eq!(effective.run.unwrap().max_in_flight, Some(4));
        assert_eq!(winners["run.max-in-flight"].layer, ConfigLayer::Repo);
    }

    #[test]
    fn repo_requirements_preserve_explicit_defaults_and_unrelated_fallbacks() {
        let mut runtime = RuntimeConfig {
            worktree: WorktreeConfig {
                confinement: crate::confinement::WorktreeConfinementConfig {
                    enabled: true,
                    sandbox: false,
                    allow: vec!["global-allow".into()],
                },
                retention: WorktreeRetentionConfig {
                    cheap: vec!["global-cheap".into()],
                    expensive: vec!["global-expensive".into()],
                    expensive_grace_days: Some(7),
                },
                ..WorktreeConfig::default()
            },
            ..RuntimeConfig::default()
        };
        let repo = RuntimeConfig {
            worktree: WorktreeConfig {
                confinement: crate::confinement::WorktreeConfinementConfig {
                    enabled: false,
                    ..crate::confinement::WorktreeConfinementConfig::default()
                },
                retention: WorktreeRetentionConfig {
                    cheap: vec!["repo-cheap".into()],
                    ..WorktreeRetentionConfig::default()
                },
                ..WorktreeConfig::default()
            },
            authored_requirements: BTreeMap::from([
                (
                    "worktree.confinement.enabled".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Boolean(false),
                    },
                ),
                (
                    "worktree.retention.cheap".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Array(vec![toml::Value::String("repo-cheap".into())]),
                    },
                ),
            ]),
            ..RuntimeConfig::default()
        };

        for leaf in repo.authored_requirements.keys().copied() {
            apply_requirement_leaf(&mut runtime, &repo, leaf);
        }

        assert!(!runtime.worktree.confinement.enabled);
        assert!(!runtime.worktree.confinement.sandbox);
        assert_eq!(runtime.worktree.confinement.allow, vec!["global-allow"]);
        assert_eq!(runtime.worktree.retention.cheap, vec!["repo-cheap"]);
        assert_eq!(
            runtime.worktree.retention.expensive,
            vec!["global-expensive"]
        );
        assert_eq!(runtime.worktree.retention.expensive_grace_days, Some(7));
    }

    #[test]
    fn partial_requirement_table_keeps_undeclared_fallback_siblings() {
        let leaf = |value: &str| AuthoredConfigLeaf {
            semantic: ConfigSemantic::Requirement,
            value: toml::Value::String(value.into()),
        };
        let global = RuntimeConfig {
            worktree: WorktreeConfig {
                confinement: crate::confinement::WorktreeConfinementConfig {
                    sandbox: false,
                    ..crate::confinement::WorktreeConfinementConfig::default()
                },
                ..WorktreeConfig::default()
            },
            authored_requirements: BTreeMap::from([(
                "worktree.confinement.sandbox".into(),
                leaf("false"),
            )]),
            ..RuntimeConfig::default()
        };
        let repo = RuntimeConfig {
            worktree: WorktreeConfig {
                confinement: crate::confinement::WorktreeConfinementConfig {
                    enabled: false,
                    ..crate::confinement::WorktreeConfinementConfig::default()
                },
                ..WorktreeConfig::default()
            },
            authored_requirements: BTreeMap::from([(
                "worktree.confinement.enabled".into(),
                leaf("false"),
            )]),
            ..RuntimeConfig::default()
        };
        let mut runtime = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_project_config(
            &mut runtime,
            global,
            ConfigLayer::UserGlobal,
            Some("global.toml".into()),
            &mut winners,
        );
        merge_project_config(
            &mut runtime,
            repo,
            ConfigLayer::Repo,
            Some("repo.toml".into()),
            &mut winners,
        );

        assert!(!runtime.worktree.confinement.enabled);
        assert!(!runtime.worktree.confinement.sandbox);
    }

    #[test]
    fn requirement_conflicts_only_report_differing_authored_values() {
        let repo = RuntimeConfig {
            authored_requirements: BTreeMap::from([
                (
                    "run.strict-loops".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Boolean(false),
                    },
                ),
                (
                    "merge.gate".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Array(Vec::new()),
                    },
                ),
            ]),
            ..RuntimeConfig::default()
        };
        let environment = RuntimeConfig {
            authored_requirements: BTreeMap::from([
                (
                    "run.strict-loops".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Boolean(true),
                    },
                ),
                (
                    "merge.gate".into(),
                    AuthoredConfigLeaf {
                        semantic: ConfigSemantic::Requirement,
                        value: toml::Value::Array(Vec::new()),
                    },
                ),
            ]),
            ..RuntimeConfig::default()
        };
        let conflicts = requirement_conflicts(&[
            (ConfigLayer::Repo, Utf8PathBuf::from("repo.toml"), repo),
            (
                ConfigLayer::Environment,
                Utf8PathBuf::from("environment.toml"),
                environment,
            ),
        ]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].field, "run.strict-loops");
    }

    #[test]
    fn requirement_conflict_names_only_the_effective_repository_source() {
        let leaf = |value: &str| AuthoredConfigLeaf {
            semantic: ConfigSemantic::Requirement,
            value: toml::Value::String(value.into()),
        };
        let repository = |value| RuntimeConfig {
            authored_requirements: BTreeMap::from([("merge.gate".into(), leaf(value))]),
            ..RuntimeConfig::default()
        };
        let environment = repository("environment");

        let conflicts = requirement_conflicts(&[
            (
                ConfigLayer::Repo,
                Utf8PathBuf::from("legacy-repo.toml"),
                repository("legacy"),
            ),
            (
                ConfigLayer::Repo,
                Utf8PathBuf::from("current-repo.toml"),
                repository("current"),
            ),
            (
                ConfigLayer::Environment,
                Utf8PathBuf::from("environment.toml"),
                environment,
            ),
        ]);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].repo_source, "current-repo.toml");
    }

    #[test]
    fn additive_maps_keep_repository_keys_and_report_personal_and_environment_conflicts() {
        let active_key = active_repo_qualifier_key()
            .expect("the in-repository test process has an active repository qualifier");
        let cache = |env: &str| BuildCacheConfig {
            env: env.to_string(),
        };
        let global_path = Utf8PathBuf::from("global.toml");
        let repo_path = Utf8PathBuf::from("repo.toml");
        let environment_path = Utf8PathBuf::from("environment.toml");
        let personal = RepoOverride {
            worktree: RepoWorktreeOverride {
                env: BTreeMap::from([
                    ("CONFLICT".into(), "personal".into()),
                    ("PERSONAL_ONLY".into(), "personal".into()),
                ]),
                ..RepoWorktreeOverride::default()
            },
            run: RepoRunOverride {
                build_cache: BTreeMap::from([
                    ("shared".into(), cache("PERSONAL_CACHE")),
                    ("personal-only".into(), cache("PERSONAL_ONLY_CACHE")),
                ]),
                ..RepoRunOverride::default()
            },
            ..RepoOverride::default()
        };
        let global = RuntimeConfig {
            worktree: WorktreeConfig {
                env: BTreeMap::from([("GLOBAL_ONLY".into(), "global".into())]),
                ..WorktreeConfig::default()
            },
            run: Some(RunTable {
                build_cache: BTreeMap::from([("global-only".into(), cache("GLOBAL_ONLY_CACHE"))]),
                ..RunTable::default()
            }),
            repo: BTreeMap::from([(active_key, personal)]),
            ..RuntimeConfig::default()
        };
        let repo = RuntimeConfig {
            worktree: WorktreeConfig {
                env: BTreeMap::from([
                    ("CONFLICT".into(), "repo".into()),
                    ("REPO_ONLY".into(), "repo".into()),
                ]),
                ..WorktreeConfig::default()
            },
            run: Some(RunTable {
                build_cache: BTreeMap::from([
                    ("shared".into(), cache("REPO_CACHE")),
                    ("repo-only".into(), cache("REPO_ONLY_CACHE")),
                ]),
                ..RunTable::default()
            }),
            ..RuntimeConfig::default()
        };
        let environment = RuntimeConfig {
            worktree: WorktreeConfig {
                env: BTreeMap::from([
                    ("CONFLICT".into(), "environment".into()),
                    ("ENV_ONLY".into(), "environment".into()),
                ]),
                ..WorktreeConfig::default()
            },
            run: Some(RunTable {
                build_cache: BTreeMap::from([
                    ("shared".into(), cache("ENV_CACHE")),
                    ("environment-only".into(), cache("ENV_ONLY_CACHE")),
                ]),
                ..RunTable::default()
            }),
            ..RuntimeConfig::default()
        };
        let documents = vec![
            (ConfigLayer::UserGlobal, global_path.clone(), global),
            (ConfigLayer::Repo, repo_path.clone(), repo),
            (
                ConfigLayer::Environment,
                environment_path.clone(),
                environment,
            ),
        ];
        let personal = documents[0].2.repo.values().next().unwrap();
        let mut runtime = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        apply_additive_values(
            &mut runtime,
            &documents,
            &[(&global_path, personal)],
            &mut winners,
        );

        assert_eq!(
            runtime.worktree.env,
            BTreeMap::from([
                ("CONFLICT".into(), "repo".into()),
                ("ENV_ONLY".into(), "environment".into()),
                ("GLOBAL_ONLY".into(), "global".into()),
                ("PERSONAL_ONLY".into(), "personal".into()),
                ("REPO_ONLY".into(), "repo".into()),
            ])
        );
        assert_eq!(
            runtime.run.unwrap().build_cache,
            BTreeMap::from([
                ("environment-only".into(), cache("ENV_ONLY_CACHE")),
                ("global-only".into(), cache("GLOBAL_ONLY_CACHE")),
                ("personal-only".into(), cache("PERSONAL_ONLY_CACHE")),
                ("repo-only".into(), cache("REPO_ONLY_CACHE")),
                ("shared".into(), cache("REPO_CACHE")),
            ])
        );
        assert_eq!(
            winners["worktree.env.CONFLICT"].source.as_deref(),
            Some("repo.toml")
        );
        assert_eq!(
            winners["run.build-cache.shared"].source.as_deref(),
            Some("repo.toml")
        );

        let conflicts = requirement_conflicts(&documents);
        let actual: BTreeSet<_> = conflicts
            .iter()
            .map(|conflict| {
                (
                    conflict.field.as_str(),
                    conflict.rejected_source.as_str(),
                    conflict.repo_source.as_str(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            BTreeSet::from([
                ("worktree.env.CONFLICT", "global.toml", "repo.toml"),
                ("worktree.env.CONFLICT", "environment.toml", "repo.toml"),
                ("run.build-cache.shared", "global.toml", "repo.toml"),
                ("run.build-cache.shared", "environment.toml", "repo.toml"),
            ])
        );
    }

    #[test]
    fn config_semantics_classify_requirement_additive_and_default_leaves() {
        assert_eq!(config_semantic("merge.gate"), ConfigSemantic::Requirement);
        assert_eq!(config_semantic("publish.exclude"), ConfigSemantic::Additive);
        assert_eq!(config_semantic("merge.auto"), ConfigSemantic::Default);
    }

    #[test]
    fn runtime_config_semantic_catalog_covers_every_authored_static_leaf() {
        let expected = runtime_config_schema_leaves();
        assert_eq!(
            ConfigLeaf::ALL
                .iter()
                .map(|leaf| leaf.path().to_string())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            config_semantic("worktree.env.TOKEN"),
            ConfigSemantic::Additive
        );
        assert_eq!(
            config_semantic("run.build-cache.target"),
            ConfigSemantic::Additive
        );
    }

    /// Derive the authored RuntimeConfig surface from schemars rather than a
    /// second hand-maintained field list. Dynamic maps are one schema-level
    /// shape and machine-scoped tables are deliberately one classified prefix.
    fn runtime_config_schema_leaves() -> BTreeSet<String> {
        fn visit<'a>(
            root: &'a serde_json::Value,
            node: &'a serde_json::Value,
            path: &mut Vec<String>,
            leaves: &mut BTreeSet<String>,
        ) {
            if let Some(reference) = node.get("$ref").and_then(serde_json::Value::as_str) {
                let name = reference
                    .strip_prefix("#/$defs/")
                    .expect("RuntimeConfig schema references local definitions");
                visit(root, &root["$defs"][name], path, leaves);
                return;
            }
            if let Some(branches) = node.get("anyOf").and_then(serde_json::Value::as_array) {
                for branch in branches {
                    if branch.get("type").and_then(serde_json::Value::as_str) != Some("null") {
                        visit(root, branch, path, leaves);
                    }
                }
                return;
            }
            if let Some(branches) = node.get("allOf").and_then(serde_json::Value::as_array) {
                for branch in branches {
                    visit(root, branch, path, leaves);
                }
                return;
            }
            if path.len() == 1 && matches!(path[0].as_str(), "harness" | "agent" | "host" | "repo")
            {
                leaves.insert(format!("{}.*", path[0]));
                return;
            }
            if let Some(properties) = node
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                for (name, child) in properties {
                    path.push(name.clone());
                    visit(root, child, path, leaves);
                    path.pop();
                }
                return;
            }
            if node.get("additionalProperties").is_some() {
                leaves.insert(path.join("."));
                return;
            }
            leaves.insert(path.join("."));
        }

        let schema = serde_json::to_value(schemars::schema_for!(RuntimeConfig))
            .expect("RuntimeConfig schema serializes");
        let mut leaves = BTreeSet::new();
        visit(&schema, &schema, &mut Vec::new(), &mut leaves);
        leaves
    }

    #[test]
    fn every_requirement_leaf_rejects_a_differing_environment_declaration() {
        let requirements: BTreeMap<ConfigLeaf, AuthoredConfigLeaf> = ConfigLeaf::ALL
            .iter()
            .filter(|leaf| leaf.semantic() == ConfigSemantic::Requirement)
            .map(|leaf| {
                (
                    *leaf,
                    AuthoredConfigLeaf {
                        semantic: leaf.semantic(),
                        value: toml::Value::String("repository".into()),
                    },
                )
            })
            .collect();
        let environment: BTreeMap<ConfigLeaf, AuthoredConfigLeaf> = requirements
            .iter()
            .map(|(field, leaf)| {
                (
                    *field,
                    AuthoredConfigLeaf {
                        semantic: leaf.semantic,
                        value: toml::Value::String("environment".into()),
                    },
                )
            })
            .collect();
        let conflicts = requirement_conflicts(&[
            (
                ConfigLayer::Repo,
                Utf8PathBuf::from("repo.toml"),
                RuntimeConfig {
                    authored_requirements: requirements,
                    ..RuntimeConfig::default()
                },
            ),
            (
                ConfigLayer::Environment,
                Utf8PathBuf::from("environment.toml"),
                RuntimeConfig {
                    authored_requirements: environment,
                    ..RuntimeConfig::default()
                },
            ),
        ]);
        assert_eq!(
            conflicts.len(),
            ConfigLeaf::ALL
                .iter()
                .filter(|leaf| leaf.semantic() == ConfigSemantic::Requirement)
                .count()
        );
        assert!(conflicts.iter().all(|conflict| {
            conflict.rejected_source == "environment.toml" && conflict.repo_source == "repo.toml"
        }));
    }

    #[test]
    fn every_requirement_leaf_retains_its_repository_value_during_resolution() {
        let values = [
            (ConfigLeaf::SchemaVersion, "\"repo\"", "\"environment\""),
            (
                ConfigLeaf::WorktreeSetup,
                "[[\"repo\"]]",
                "[[\"environment\"]]",
            ),
            (ConfigLeaf::WorktreeSetupSeconds, "1", "2"),
            (ConfigLeaf::WorktreeSetupCaptureBytes, "1", "2"),
            (ConfigLeaf::WorktreeConfinementEnabled, "false", "true"),
            (ConfigLeaf::WorktreeConfinementSandbox, "false", "true"),
            (
                ConfigLeaf::WorktreeConfinementAllow,
                "[\"repo\"]",
                "[\"environment\"]",
            ),
            (ConfigLeaf::WorktreeTripwirePolicy, "\"park\"", "\"warn\""),
            (
                ConfigLeaf::WorktreeRetentionCheap,
                "[\"repo\"]",
                "[\"environment\"]",
            ),
            (
                ConfigLeaf::WorktreeRetentionExpensive,
                "[\"repo\"]",
                "[\"environment\"]",
            ),
            (ConfigLeaf::WorktreeRetentionExpensiveGraceDays, "1", "2"),
            (ConfigLeaf::RunWorktree, "false", "true"),
            (ConfigLeaf::RunMaxFrames, "1", "2"),
            (ConfigLeaf::RunFrameSeconds, "1", "2"),
            (ConfigLeaf::RunTotalSeconds, "1", "2"),
            (ConfigLeaf::RunMaxRetries, "1", "2"),
            (ConfigLeaf::RunAttachWaitSeconds, "1", "2"),
            (ConfigLeaf::RunIdleSeconds, "1", "2"),
            (ConfigLeaf::RunCommandSeconds, "1", "2"),
            (ConfigLeaf::RunCommandIdleSeconds, "1", "2"),
            (ConfigLeaf::RunMaxInFlight, "1", "2"),
            (ConfigLeaf::RunStrictLoops, "false", "true"),
            (ConfigLeaf::RunInlinePromptBytes, "1", "2"),
            (ConfigLeaf::MergeOverlap, "\"land\"", "\"park\""),
            (ConfigLeaf::MergeBranch, "\"repo\"", "\"environment\""),
            (ConfigLeaf::MergeGate, "[[\"repo\"]]", "[[\"environment\"]]"),
            (ConfigLeaf::MergeGateSeconds, "1", "2"),
            (
                ConfigLeaf::MergeGenerated,
                "[]",
                "[{ paths = [\"environment\"], rebuild = [[\"echo\", \"environment\"]] }]",
            ),
            (ConfigLeaf::MergeDiskFloorMb, "0", "2"),
            (ConfigLeaf::MergeRetryAttempts, "1", "2"),
            (ConfigLeaf::MergeRetryBackoffMs, "1", "2"),
        ];

        assert_eq!(
            values
                .iter()
                .map(|(leaf, _, _)| *leaf)
                .collect::<BTreeSet<_>>(),
            ConfigLeaf::ALL
                .iter()
                .copied()
                .filter(|leaf| leaf.semantic() == ConfigSemantic::Requirement)
                .collect(),
            "the resolution table must exercise every requirement leaf"
        );

        for (leaf, repo_value, environment_value) in values {
            let parse = |value: &str| {
                let text = format!("{} = {value}", leaf.path());
                let document: toml::Value = toml::from_str(&text).expect("test declaration parses");
                let mut config: RuntimeConfig =
                    toml::from_str(&text).expect("test declaration decodes");
                config.authored_requirements = authored_requirement_values(&document);
                config
            };
            let repo = parse(repo_value);
            let environment = parse(environment_value);
            let repo_path = Utf8PathBuf::from("repo.toml");
            let effective = BTreeMap::from([(
                leaf,
                (
                    &repo_path,
                    repo.authored_requirements
                        .get(&leaf)
                        .expect("repo leaf is authored"),
                ),
            )]);
            let mut runtime = RuntimeConfig::default();
            apply_requirement_leaf(&mut runtime, &repo, leaf);
            let expected = runtime.clone();
            apply_environment_requirement_leaves(
                &mut runtime,
                &environment,
                &effective,
                ConfigLayer::Environment,
                Some("environment.toml".into()),
                &mut BTreeMap::new(),
            );
            assert_eq!(
                runtime,
                expected,
                "{} must retain the repository value",
                leaf.path()
            );
        }
    }

    #[test]
    fn repository_schema_version_wins_over_environment_with_provenance() {
        let parse = |text: &str| {
            let document: toml::Value = toml::from_str(text).expect("test config parses");
            let mut config: RuntimeConfig = toml::from_str(text).expect("test config decodes");
            config.authored_requirements = authored_requirement_values(&document);
            config
        };
        let repo_path = Utf8PathBuf::from("repository/.ctx/config.toml");
        let environment_path = Utf8PathBuf::from("environment.toml");
        let repo = parse("schema-version = \"repository\"");
        let environment = parse("schema-version = \"environment\"");
        let documents = vec![
            (ConfigLayer::Repo, repo_path.clone(), repo.clone()),
            (
                ConfigLayer::Environment,
                environment_path,
                environment.clone(),
            ),
        ];
        let mut runtime = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_project_config(
            &mut runtime,
            repo,
            ConfigLayer::Repo,
            Some(repo_path.to_string()),
            &mut winners,
        );
        let effective = effective_repo_requirements(&documents);
        record_effective_repo_requirement_winners(&effective, &mut winners);
        apply_environment_requirement_leaves(
            &mut runtime,
            &environment,
            &effective,
            ConfigLayer::Environment,
            Some("environment.toml".into()),
            &mut winners,
        );

        assert_eq!(runtime.schema_version.as_deref(), Some("repository"));
        assert_eq!(winners["schema-version"].layer, ConfigLayer::Repo);
        assert_eq!(
            winners["schema-version"].source.as_deref(),
            Some("repository/.ctx/config.toml")
        );
        assert_eq!(
            winners["schema-version"].reason,
            ConfigReason::RepoRequirement
        );
    }

    #[test]
    fn inherits_default_seat_flags_only_unmatched_trait_roles() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([
                (
                    DEFAULT_SEAT.to_string(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        harness: Some("harness".into()),
                        ..ProfileAssignment::default()
                    }),
                ),
                (
                    "smart-1".to_string(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        harness: Some("harness".into()),
                        ..ProfileAssignment::default()
                    }),
                ),
                (
                    "expandable".to_string(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        harness: Some("harness".into()),
                        count: Some(2),
                        ..ProfileAssignment::default()
                    }),
                ),
            ]),
            ..AgentDefaults::default()
        };
        let mut overrides: BTreeMap<String, ProfileAssignment> = BTreeMap::new();
        // A bare id with no table of its own inherits the default seat.
        assert!(inherits_default_seat(&defaults, &overrides, "smart"));
        // Its own table, a standing seat, or an expansion of an authored
        // base all resolve without inheriting.
        assert!(!inherits_default_seat(&defaults, &overrides, "smart-1"));
        assert!(!inherits_default_seat(&defaults, &overrides, "narrator"));
        assert!(!inherits_default_seat(&defaults, &overrides, DEFAULT_SEAT));
        assert!(!inherits_default_seat(
            &defaults,
            &overrides,
            "expandable-2"
        ));
        // An explicit --assign override covers a table-less role.
        overrides.insert("smart".to_string(), ProfileAssignment::default());
        assert!(!inherits_default_seat(&defaults, &overrides, "smart"));
    }

    #[test]
    fn model_tier_is_diagnostic_only() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                DEFAULT_SEAT.to_string(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    harness: Some("harness".into()),
                    ..ProfileAssignment::default()
                }),
            )]),
            ..AgentDefaults::default()
        };
        defaults.model_tier.insert(
            ctx_traits_core::r#trait::AgentModelTier::Fast,
            ProfileAssignment {
                model: Some("tier-model".into()),
                ..ProfileAssignment::default()
            },
        );
        let assignment = ProfileAssignment {
            model_tier: Some(ctx_traits_core::r#trait::AgentModelTier::Fast),
            ..ProfileAssignment::default()
        };
        let resolved =
            resolved_assignment_for_role(&defaults, "worker", Some(&assignment)).unwrap();
        assert_eq!(resolved.model, None);
        assert_eq!(resolved.model_tier, None);
    }

    #[test]
    fn machine_role_lists_replace_as_one_value() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        let repo = RuntimeConfig {
            agent: AgentDefaults {
                role: BTreeMap::from([(
                    "worker".into(),
                    RoleAssignmentValue::List(vec![ProfileAssignment {
                        harness: Some("repo-seat".into()),
                        model: Some("stale-repo-model".into()),
                        ..ProfileAssignment::default()
                    }]),
                )]),
                ..AgentDefaults::default()
            },
            ..RuntimeConfig::default()
        };
        let global = RuntimeConfig {
            agent: AgentDefaults {
                role: BTreeMap::from([(
                    "worker".into(),
                    RoleAssignmentValue::List(vec![
                        ProfileAssignment {
                            harness: Some("global-one".into()),
                            ..ProfileAssignment::default()
                        },
                        ProfileAssignment {
                            harness: Some("global-two".into()),
                            ..ProfileAssignment::default()
                        },
                    ]),
                )]),
                ..AgentDefaults::default()
            },
            ..RuntimeConfig::default()
        };
        merge_machine_config(
            &mut effective,
            repo,
            ConfigLayer::Repo,
            Some("repo".into()),
            &mut winners,
        );
        merge_machine_config(
            &mut effective,
            global,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );

        let RoleAssignmentValue::List(seats) = &effective.agent.role["worker"] else {
            panic!("expected list-backed role");
        };
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].harness.as_deref(), Some("global-one"));
        assert_eq!(
            winners["agent.role.worker.1.harness"].layer,
            ConfigLayer::UserGlobal
        );
        assert!(!winners.contains_key("agent.role.worker.1.model"));
    }

    #[test]
    fn parsed_single_tables_overlay_leaves_without_resetting_attach_mode() {
        let parse =
            |text: &str| toml::from_str::<RuntimeConfig>(text).expect("test config decodes");
        let global = parse(
            "[harness.layered]\nbin = 'global-bin'\nversion-probe = ['global-version']\n\
             [harness.layered.cli]\nprompt-via = 'stdin'\nargv = ['global-argv']\n\
             [harness.layered.mcp]\nmcp-config-flag = '--global-mcp'\nallowed-tools = ['global-tool']\n\
             [agent.role.worker]\nmode = 'attach'\nmodel = 'global-model'\nextra-args = ['--global']\n\
             [agent.variant.smart.role.worker]\nreasoning-effort = 'low'\n\
             [repo.key.agent.role.worker]\nsystem-prompt = 'personal-prompt'\n",
        );
        let repo = parse(
            "[harness.layered]\nbin = 'repo-bin'\nversion-probe = ['repo-version']\n\
             [harness.layered.cli]\nargv = ['repo-argv']\n\
             [harness.layered.mcp]\nallowed-tools-flag = '--repo-tools'\nallowed-tools = ['repo-tool']\n\
             [agent.role.worker]\nreasoning-effort = 'medium'\n\
             [agent.variant.smart.role.worker]\nsystem-prompt = 'variant-prompt'\n",
        );
        let environment = parse(
            "[harness.layered.mcp]\nmcp-config-flag = ''\nconfig-via = 'environment-file'\n\
             [agent.role.worker]\nextra-args = ['--environment']\n",
        );
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_machine_config(
            &mut effective,
            global,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );
        merge_machine_config(
            &mut effective,
            repo,
            ConfigLayer::Repo,
            Some("repo".into()),
            &mut winners,
        );
        let personal = effective.repo["key"].clone();
        apply_repo_defaults(
            &mut effective,
            &personal,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );
        apply_environment_defaults(
            &mut effective,
            &environment,
            ConfigLayer::Environment,
            Some("environment".into()),
            &mut winners,
        );

        let harness = &effective.harness["layered"];
        assert_eq!(harness.bin(), "repo-bin");
        assert_eq!(harness.version_probe, ["repo-version"]);
        assert_eq!(harness.cli.as_ref().unwrap().argv, ["repo-argv"]);
        let mcp = harness.mcp.as_ref().unwrap();
        assert_eq!(mcp.mcp_config_flag, None);
        assert_eq!(mcp.allowed_tools_flag.as_deref(), Some("--repo-tools"));
        assert_eq!(mcp.allowed_tools, ["repo-tool"]);
        assert_eq!(mcp.config_via.as_deref(), Some("environment-file"));
        assert_eq!(
            winners["harness.layered.mcp.mcp-config-flag"].layer,
            ConfigLayer::Environment
        );
        assert_eq!(
            winners["harness.layered.mcp.allowed-tools-flag"].layer,
            ConfigLayer::Repo
        );
        assert_eq!(
            winners["harness.layered.mcp.config-via"].layer,
            ConfigLayer::Environment
        );

        let RoleAssignmentValue::Single(worker) = &effective.agent.role["worker"] else {
            panic!("expected single-table worker assignment");
        };
        assert_eq!(worker.mode, RunAssignmentMode::Attach);
        assert_eq!(worker.model.as_deref(), Some("global-model"));
        assert_eq!(worker.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(worker.system_prompt.as_deref(), Some("personal-prompt"));
        assert_eq!(worker.extra_args, ["--environment"]);
        assert_eq!(
            winners["agent.role.worker.mode"].layer,
            ConfigLayer::UserGlobal
        );
        assert_eq!(
            winners["agent.role.worker.system-prompt"].reason,
            ConfigReason::PersonalRepoOverride
        );
        assert_eq!(
            winners["agent.role.worker.extra-args"].layer,
            ConfigLayer::Environment
        );

        let RoleAssignmentValue::Single(variant) = &effective.agent.variant["smart"].role["worker"]
        else {
            panic!("expected single-table variant assignment");
        };
        assert_eq!(variant.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(variant.system_prompt.as_deref(), Some("variant-prompt"));
    }

    #[test]
    fn tier_diagnostic_scans_role_lists() {
        let agent = AgentDefaults {
            role: BTreeMap::from([(
                "worker".into(),
                RoleAssignmentValue::List(vec![ProfileAssignment {
                    model_tier: Some(ctx_traits_core::r#trait::AgentModelTier::Fast),
                    ..ProfileAssignment::default()
                }]),
            )]),
            ..AgentDefaults::default()
        };
        assert!(has_tier_declaration(&agent));
    }

    // P427 explicit-built-in-id-assignment, explicit-master-assignment, and
    // same-id-override precedence are proved end to end by
    // `scripts/byte_compare.rs`'s `--zero-config-harness-proof` (Scenarios
    // D, J, K) — the one harness this codebase's current-stage validation
    // rule permits for this behavior — rather than duplicated here.

    // -----------------------------------------------------------------
    // P451: variant/repo qualifier fold
    // -----------------------------------------------------------------

    fn single(harness: &str) -> ProfileAssignment {
        ProfileAssignment {
            harness: Some(harness.into()),
            ..ProfileAssignment::default()
        }
    }

    /// 0079 regression: a seat declaring ONLY `transport = "api"` (no
    /// harness fallback) is dispatchable and must survive resolution —
    /// `finalize_assignment` collapsing it to `None` made an api-only
    /// narrator silently vanish (no titles, no narration, no diagnostic),
    /// even though `resolve_seat_dispatch` was built to own exactly that
    /// seat's key-missing degrade path.
    #[test]
    fn an_api_only_seat_survives_resolution_without_a_harness() {
        let mut seat = ProfileAssignment {
            transport: Some(RunTransport::Api),
            model: Some("deepseek/deepseek-v4-flash".into()),
            ..ProfileAssignment::default()
        };
        seat.api.base_url = Some("https://openrouter.ai/api/v1".into());
        seat.api.wire = Some(ProviderWire::OpenaiCompat);
        seat.api.api_key_env = Some("OPENROUTER_API_KEY".into());
        let mut defaults = AgentDefaults::default();
        defaults
            .role
            .insert("narrator".into(), RoleAssignmentValue::Single(seat));

        let resolved = resolved_assignment_for_role(&defaults, "narrator", None)
            .expect("an api-only seat must survive resolution without a harness");
        assert_eq!(resolved.transport, Some(RunTransport::Api));
        assert!(resolved.harness.is_none());

        // A harness-less seat with NO api transport still collapses — the
        // pre-0079 rule is unchanged for ordinary Cli seats.
        let mut defaults = AgentDefaults::default();
        defaults.role.insert(
            "narrator".into(),
            RoleAssignmentValue::Single(ProfileAssignment::default()),
        );
        assert!(resolved_assignment_for_role(&defaults, "narrator", None).is_none());
    }

    fn scope<'a>(variant: Option<&'a str>, repo_key: Option<&'a str>) -> RunScope<'a> {
        scope_with_trait(variant, repo_key, None)
    }

    fn scope_with_trait<'a>(
        variant: Option<&'a str>,
        repo_key: Option<&'a str>,
        trait_id: Option<&'a str>,
    ) -> RunScope<'a> {
        RunScope {
            variant: variant.map(std::borrow::Cow::Borrowed),
            repo_key: repo_key.map(std::borrow::Cow::Borrowed),
            trait_id: trait_id.map(std::borrow::Cow::Borrowed),
        }
    }

    #[test]
    fn flatten_agent_defaults_is_structural_identity_when_unqualified() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "reviewer".into(),
                RoleAssignmentValue::Single(single("base")),
            )]),
            ..AgentDefaults::default()
        };
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            None,
            &scope(Some("smart"), Some("repo-x")),
        );
        assert_eq!(flattened, defaults);
        assert!(winners.is_empty());
    }

    #[test]
    fn flatten_agent_defaults_variant_single_table_inherits_fields() {
        let mut base = single("base-harness");
        base.reasoning_effort = Some("low".into());
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([("reviewer".into(), RoleAssignmentValue::Single(base))]),
            ..AgentDefaults::default()
        };
        defaults.variant.insert(
            "smart".into(),
            VariantOverride {
                role: BTreeMap::from([(
                    "reviewer".into(),
                    RoleAssignmentValue::Single(single("smart-harness")),
                )]),
            },
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            None,
            &scope(Some("smart"), None),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["reviewer"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("smart-harness"));
        // Inherited from the base table: the variant override only named a
        // harness.
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(winners["reviewer"], "variant:smart");
    }

    #[test]
    fn flatten_agent_defaults_variant_list_wins_whole() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                "reviewer".into(),
                RoleAssignmentValue::Single(single("base-harness")),
            )]),
            ..AgentDefaults::default()
        };
        defaults.variant.insert(
            "quick".into(),
            VariantOverride {
                role: BTreeMap::from([(
                    "reviewer".into(),
                    RoleAssignmentValue::List(vec![single("seat-one"), single("seat-two")]),
                )]),
            },
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            None,
            &scope(Some("quick"), None),
        );
        let RoleAssignmentValue::List(seats) = &flattened.role["reviewer"] else {
            panic!("expected list-backed role");
        };
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].harness.as_deref(), Some("seat-one"));
        assert_eq!(winners["reviewer"], "variant:quick");
    }

    #[test]
    fn flatten_agent_defaults_repo_qualifier_only_applies_to_matching_key() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "worker".into(),
                RoleAssignmentValue::Single(single("base-harness")),
            )]),
            ..AgentDefaults::default()
        };
        let mut repo = BTreeMap::new();
        repo.insert(
            "repo-a".to_string(),
            RepoOverride {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("repo-a-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
                ..RepoOverride::default()
            },
        );

        let (matching, matching_winners) =
            flatten_agent_defaults(&defaults, &repo, None, &scope(None, Some("repo-a")));
        let RoleAssignmentValue::Single(resolved) = &matching.role["worker"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("repo-a-harness"));
        assert_eq!(matching_winners["worker"], "repo:repo-a");

        let (other, other_winners) =
            flatten_agent_defaults(&defaults, &repo, None, &scope(None, Some("repo-b")));
        assert_eq!(other, defaults);
        assert!(other_winners.is_empty());
    }

    #[test]
    fn flatten_agent_defaults_repo_and_variant_compose() {
        let defaults = AgentDefaults::default();
        let mut repo = BTreeMap::new();
        repo.insert(
            "repo-a".to_string(),
            RepoOverride {
                agent: AgentDefaults {
                    variant: BTreeMap::from([(
                        "smart".into(),
                        VariantOverride {
                            role: BTreeMap::from([(
                                "reviewer".into(),
                                RoleAssignmentValue::Single(single("repo-smart-harness")),
                            )]),
                        },
                    )]),
                    ..AgentDefaults::default()
                },
                ..RepoOverride::default()
            },
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &repo,
            None,
            &scope(Some("smart"), Some("repo-a")),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["reviewer"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("repo-smart-harness"));
        assert_eq!(winners["reviewer"], "repo:repo-a+variant:smart");
    }

    #[test]
    fn flatten_agent_defaults_seeds_from_default_seat_when_role_has_no_base_table() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                DEFAULT_SEAT.into(),
                RoleAssignmentValue::Single(single("default-harness")),
            )]),
            ..AgentDefaults::default()
        };
        defaults.variant.insert(
            "smart".into(),
            VariantOverride {
                role: BTreeMap::from([(
                    "reviewer".into(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        reasoning_effort: Some("high".into()),
                        ..ProfileAssignment::default()
                    }),
                )]),
            },
        );
        let (flattened, _) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            None,
            &scope(Some("smart"), None),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["reviewer"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("default-harness"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn flatten_agent_defaults_standing_seat_never_seeded_from_default() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                DEFAULT_SEAT.into(),
                RoleAssignmentValue::Single(single("default-harness")),
            )]),
            ..AgentDefaults::default()
        };
        defaults.variant.insert(
            "smart".into(),
            VariantOverride {
                role: BTreeMap::from([(
                    "narrator".into(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        reasoning_effort: Some("high".into()),
                        ..ProfileAssignment::default()
                    }),
                )]),
            },
        );
        let (flattened, _) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            None,
            &scope(Some("smart"), None),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["narrator"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness, None);
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn resolve_run_variant_prefers_metadata_over_suffix_match() {
        let mut defaults = AgentDefaults::default();
        defaults
            .variant
            .insert("smart".into(), VariantOverride::default());
        defaults
            .variant
            .insert("phase-smart".into(), VariantOverride::default());

        let trait_ref = minimal_trait("implement-phase-smart", Some("declared"));
        assert_eq!(
            resolve_run_variant(&trait_ref, &defaults, None, None),
            Some("declared".to_string())
        );

        let trait_ref = minimal_trait("implement-phase-smart", None);
        // Both "smart" and "phase-smart" match the id's suffix; the longer
        // declared key wins.
        assert_eq!(
            resolve_run_variant(&trait_ref, &defaults, None, None),
            Some("phase-smart".to_string())
        );
    }

    #[test]
    fn resolve_run_variant_none_when_no_declared_suffix_matches() {
        let defaults = AgentDefaults::default();
        let trait_ref = minimal_trait("implement", None);
        assert_eq!(resolve_run_variant(&trait_ref, &defaults, None, None), None);
    }

    // -----------------------------------------------------------------
    // 0034: `[trait.<id>]` seat overrides
    // -----------------------------------------------------------------

    #[test]
    fn fold_role_trait_scope_single_table_inherits_fields() {
        let mut base = single("global-harness");
        base.reasoning_effort = Some("low".into());
        let defaults = AgentDefaults {
            role: BTreeMap::from([("worker".into(), RoleAssignmentValue::Single(base))]),
            ..AgentDefaults::default()
        };
        let mut trait_defaults = TraitDefaults::default();
        trait_defaults.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(single("opencode")),
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            Some(&trait_defaults),
            &scope_with_trait(None, None, Some("deep-research")),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["worker"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("opencode"));
        // Inherited from the global table: the trait override only named a
        // harness (the task's own deep-research example).
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(winners["worker"], "trait:deep-research");
    }

    #[test]
    fn fold_role_trait_scope_narrows_count_before_seat_expansion() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(2),
                    ..single("global-harness")
                }),
            )]),
            ..AgentDefaults::default()
        };
        let mut trait_defaults = TraitDefaults::default();
        trait_defaults.agent.role.insert(
            "smart".into(),
            RoleAssignmentValue::Single(ProfileAssignment {
                count: Some(1),
                ..ProfileAssignment::default()
            }),
        );
        let (mut flattened, _) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            Some(&trait_defaults),
            &scope_with_trait(None, None, Some("plan")),
        );
        expand_role_seats(&mut flattened);
        assert!(flattened.role.contains_key("smart"));
        assert!(!flattened.role.contains_key("smart-2"));
    }

    #[test]
    fn fold_role_trait_wins_over_repo() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "worker".into(),
                RoleAssignmentValue::Single(single("global-harness")),
            )]),
            ..AgentDefaults::default()
        };
        let mut repo = BTreeMap::new();
        repo.insert(
            "repo-a".to_string(),
            RepoOverride {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("repo-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
                ..RepoOverride::default()
            },
        );
        let mut trait_defaults = TraitDefaults::default();
        trait_defaults.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(single("trait-harness")),
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &repo,
            Some(&trait_defaults),
            &scope_with_trait(None, Some("repo-a"), Some("deep-research")),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["worker"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("trait-harness"));
        assert_eq!(winners["worker"], "trait:deep-research");
    }

    #[test]
    fn fold_role_trait_variant_wins_over_plain_trait() {
        let defaults = AgentDefaults::default();
        let mut trait_defaults = TraitDefaults::default();
        trait_defaults.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(single("trait-harness")),
        );
        trait_defaults.variant.insert(
            "quick".into(),
            TraitVariantDefaults {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("trait-quick-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
            },
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            Some(&trait_defaults),
            &scope_with_trait(Some("quick"), None, Some("implement")),
        );
        let RoleAssignmentValue::Single(resolved) = &flattened.role["worker"] else {
            panic!("expected single table");
        };
        assert_eq!(resolved.harness.as_deref(), Some("trait-quick-harness"));
        assert_eq!(winners["worker"], "trait:implement+variant:quick");
    }

    #[test]
    fn fold_role_trait_list_wins_whole() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "worker".into(),
                RoleAssignmentValue::Single(single("global-harness")),
            )]),
            ..AgentDefaults::default()
        };
        let mut trait_defaults = TraitDefaults::default();
        trait_defaults.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::List(vec![single("seat-one"), single("seat-two")]),
        );
        let (flattened, winners) = flatten_agent_defaults(
            &defaults,
            &BTreeMap::new(),
            Some(&trait_defaults),
            &scope_with_trait(None, None, Some("deep-research")),
        );
        let RoleAssignmentValue::List(seats) = &flattened.role["worker"] else {
            panic!("expected list-backed role");
        };
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].harness.as_deref(), Some("seat-one"));
        assert_eq!(winners["worker"], "trait:deep-research");
    }

    #[test]
    fn validate_trait_overrides_rejects_nested_agent_variant() {
        let mut trait_defaults = BTreeMap::new();
        let mut value = TraitDefaults::default();
        value
            .agent
            .variant
            .insert("quick".into(), VariantOverride::default());
        trait_defaults.insert("implement".into(), value);
        assert!(validate_trait_overrides(&trait_defaults).is_err());

        let mut trait_defaults = BTreeMap::new();
        let mut value = TraitDefaults::default();
        let mut variant_value = TraitVariantDefaults::default();
        variant_value
            .agent
            .variant
            .insert("nested".into(), VariantOverride::default());
        value.variant.insert("quick".into(), variant_value);
        trait_defaults.insert("implement".into(), value);
        assert!(validate_trait_overrides(&trait_defaults).is_err());
    }

    #[test]
    fn validate_trait_overrides_accepts_plain_role_and_variant_tables() {
        let mut trait_defaults = BTreeMap::new();
        let mut value = TraitDefaults::default();
        value.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(single("opencode")),
        );
        value.variant.insert(
            "quick".into(),
            TraitVariantDefaults {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("quick-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
            },
        );
        trait_defaults.insert("implement".into(), value);
        assert!(validate_trait_overrides(&trait_defaults).is_ok());
    }

    #[test]
    fn sidecar_config_rejects_agent_and_trait_tables() {
        assert!(
            toml::from_str::<TraitRunConfig>("[agent.role.worker]\nharness = \"x\"\n").is_err()
        );
        assert!(
            toml::from_str::<TraitRunConfig>(
                "[trait.example.agent.role.worker]\nharness = \"x\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn package_runtime_config_decodes_top_level_budget_and_variant_overlay() {
        let config = PackageRuntimeConfig::decode(
            "frame-seconds = 1200\ntotal-seconds = 3600\n\n[variant.quick]\nframe-seconds = 900\n",
            Utf8Path::new("runtime.toml"),
        )
        .expect("runtime.toml decodes");
        assert_eq!(config.budget.frame_seconds, Some(1200));
        assert_eq!(config.budget.total_seconds, Some(3600));
        let quick = &config.variant["quick"];
        assert_eq!(quick.frame_seconds, Some(900));
        // Variant overlay leaves everything it doesn't state unset — the
        // overlay itself happens via `overlay_budget`, not at decode time.
        assert_eq!(quick.total_seconds, None);
    }

    #[test]
    fn package_runtime_config_variant_overlay_inherits_omitted_keys_via_overlay_budget() {
        let config = PackageRuntimeConfig::decode(
            "frame-seconds = 1200\ntotal-seconds = 3600\nmax-retries = 3\n\n[variant.quick]\nframe-seconds = 900\n",
            Utf8Path::new("runtime.toml"),
        )
        .expect("runtime.toml decodes");
        let mut effective = config.budget.clone();
        overlay_budget(&mut effective, &config.variant["quick"]);
        assert_eq!(effective.frame_seconds, Some(900));
        assert_eq!(effective.total_seconds, Some(3600));
        assert_eq!(effective.max_retries, Some(3));
    }

    #[test]
    fn package_runtime_config_rejects_unknown_top_level_field() {
        for text in [
            "[assign.worker]\nharness = \"x\"\n",
            "[worktree]\nbranch = \"x\"\n",
            "harness = \"opencode\"\n",
            "model = \"x\"\n",
        ] {
            let error = PackageRuntimeConfig::decode(text, Utf8Path::new("runtime.toml"))
                .expect_err("out-of-scope field must be a hard decode error");
            let message = error.to_string();
            assert!(
                message.contains("unknown field"),
                "expected an unknown-field error, got: {message}"
            );
        }
    }

    #[test]
    fn package_runtime_config_rejects_unknown_variant_field() {
        assert!(
            PackageRuntimeConfig::decode(
                "[variant.quick]\nharness = \"opencode\"\n",
                Utf8Path::new("runtime.toml"),
            )
            .is_err()
        );
        // A variant table is budget-only: no nested `variant` or `defaults`.
        assert!(
            PackageRuntimeConfig::decode(
                "[variant.quick.variant]\nframe-seconds = 900\n",
                Utf8Path::new("runtime.toml"),
            )
            .is_err()
        );
    }

    #[test]
    fn package_runtime_config_decodes_top_level_defaults() {
        let config = PackageRuntimeConfig::decode(
            "[defaults.port]\nplan = \"sidecar\"\n",
            Utf8Path::new("runtime.toml"),
        )
        .expect("defaults.port decodes");
        assert_eq!(config.defaults.port["plan"], "sidecar");
    }

    #[test]
    fn render_package_runtime_config_carries_default_defaults_forward() {
        let mut defaults = PortDefaults::default();
        defaults
            .port
            .insert("plan".to_string(), "sidecar".to_string());
        let text = render_package_runtime_config(
            &RunProfileBudget {
                max_frames: Some(10),
                ..RunProfileBudget::default()
            },
            &defaults,
            &BTreeMap::new(),
        );
        let config = PackageRuntimeConfig::decode(&text, Utf8Path::new("runtime.toml"))
            .expect("rendered runtime.toml decodes");
        assert_eq!(config.budget.max_frames, Some(10));
        assert_eq!(config.defaults.port["plan"], "sidecar");
    }

    #[test]
    fn decodes_trait_scoped_seat_toml_shapes() {
        let runtime: RuntimeConfig = toml::from_str(concat!(
            "[trait.deep-research.agent.role.worker]\n",
            "harness = \"opencode\"\n",
            "\n",
            "[trait.plan.agent.role.smart]\n",
            "count = 1\n",
        ))
        .expect("trait-scoped seat decodes");
        assert_eq!(
            runtime.trait_defaults["deep-research"].agent.role["worker"],
            RoleAssignmentValue::Single(single("opencode"))
        );
        let RoleAssignmentValue::Single(smart) =
            &runtime.trait_defaults["plan"].agent.role["smart"]
        else {
            panic!("expected single table");
        };
        assert_eq!(smart.count, Some(1));

        let variant: RuntimeConfig = toml::from_str(
            "[trait.implement.variant.quick.agent.role.worker]\nharness = \"opencode\"\n",
        )
        .expect("trait+variant seat decodes");
        assert_eq!(
            variant.trait_defaults["implement"].variant["quick"]
                .agent
                .role["worker"],
            RoleAssignmentValue::Single(single("opencode"))
        );
    }

    #[test]
    fn merge_machine_config_records_trait_agent_winners() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        let mut next = RuntimeConfig::default();
        let mut trait_value = TraitDefaults::default();
        trait_value.agent.role.insert(
            "worker".into(),
            RoleAssignmentValue::Single(single("trait-harness")),
        );
        trait_value.variant.insert(
            "quick".into(),
            TraitVariantDefaults {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("trait-quick-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
            },
        );
        next.trait_defaults.insert("implement".into(), trait_value);
        merge_machine_config(
            &mut effective,
            next,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );
        assert!(winners.contains_key("trait.implement.agent.role.worker.harness"));
        assert!(winners.contains_key("trait.implement.variant.quick.agent.role.worker.harness"));
        assert_eq!(
            effective.trait_defaults["implement"].agent.role["worker"],
            RoleAssignmentValue::Single(single("trait-harness"))
        );
        assert_eq!(
            effective.trait_defaults["implement"].variant["quick"]
                .agent
                .role["worker"],
            RoleAssignmentValue::Single(single("trait-quick-harness"))
        );
    }

    /// 0037: at every ancestor, the committed project tier
    /// (`.ctx/traits/config.toml`) merges immediately before the
    /// machine-local `.ctx/traits/runtime.toml`, so a local field wins the
    /// field-wise merge and nothing else does.
    #[test]
    fn project_config_layers_immediately_before_machine_runtime_config() {
        let layers = runtime_config_layers(Utf8Path::new(".")).expect("layers enumerate");
        let paths: Vec<String> = layers.iter().map(|(_, path)| path.to_string()).collect();
        let mut pairs = 0;
        for (index, path) in paths.iter().enumerate() {
            if path.ends_with(PROJECT_CONFIG) {
                let next = paths.get(index + 1).expect("project tier is never last");
                assert!(
                    next.ends_with(RUNTIME_CONFIG),
                    "expected {RUNTIME_CONFIG} directly after {path}, found {next}"
                );
                pairs += 1;
            }
        }
        assert!(pairs > 0, "no project-tier layer enumerated: {paths:?}");
    }

    /// 0037: a machine-local `runtime.toml` stating ONE field overrides that
    /// field of the committed `config.toml` and nothing else, and the winner
    /// map names each field's actual source document.
    #[test]
    fn machine_runtime_config_overrides_project_config_field_wise() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();

        let mut project = RuntimeConfig::default();
        let mut committed_seat = single("project-harness");
        committed_seat.model = Some("project-model".into());
        project
            .agent
            .role
            .insert("worker".into(), RoleAssignmentValue::Single(committed_seat));
        merge_machine_config(
            &mut effective,
            project,
            ConfigLayer::Repo,
            Some(".ctx/traits/config.toml".into()),
            &mut winners,
        );

        let mut mine = RuntimeConfig::default();
        let local_seat = ProfileAssignment {
            model: Some("mine-model".into()),
            ..ProfileAssignment::default()
        };
        mine.agent
            .role
            .insert("worker".into(), RoleAssignmentValue::Single(local_seat));
        merge_machine_config(
            &mut effective,
            mine,
            ConfigLayer::Repo,
            Some(".ctx/traits/runtime.toml".into()),
            &mut winners,
        );

        let RoleAssignmentValue::Single(resolved) = &effective.agent.role["worker"] else {
            panic!("expected a single-table assignment");
        };
        assert_eq!(
            resolved.harness.as_deref(),
            Some("project-harness"),
            "an omitted field inherits the project tier"
        );
        assert_eq!(
            resolved.model.as_deref(),
            Some("mine-model"),
            "a stated field replaces the project tier"
        );
        assert_eq!(
            winners["agent.role.worker.model"].source.as_deref(),
            Some(".ctx/traits/runtime.toml")
        );
        assert_eq!(
            winners["agent.role.worker.harness"].source.as_deref(),
            Some(".ctx/traits/config.toml")
        );
    }

    #[test]
    fn merge_machine_config_records_variant_and_repo_winners() {
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        let mut next = RuntimeConfig::default();
        next.agent.variant.insert(
            "smart".into(),
            VariantOverride {
                role: BTreeMap::from([(
                    "reviewer".into(),
                    RoleAssignmentValue::Single(single("smart-harness")),
                )]),
            },
        );
        next.repo.insert(
            "repo-a".into(),
            RepoOverride {
                agent: AgentDefaults {
                    role: BTreeMap::from([(
                        "worker".into(),
                        RoleAssignmentValue::Single(single("repo-harness")),
                    )]),
                    ..AgentDefaults::default()
                },
                ..RepoOverride::default()
            },
        );
        merge_machine_config(
            &mut effective,
            next,
            ConfigLayer::UserGlobal,
            Some("global".into()),
            &mut winners,
        );
        assert!(winners.contains_key("agent.variant.smart.role.reviewer.harness"));
        assert!(winners.contains_key("repo.repo-a.agent.role.worker.harness"));
        assert_eq!(
            effective.agent.variant["smart"].role["reviewer"],
            RoleAssignmentValue::Single(single("smart-harness"))
        );
        assert_eq!(
            effective.repo["repo-a"].agent.role["worker"],
            RoleAssignmentValue::Single(single("repo-harness"))
        );
    }

    fn minimal_trait(id: &str, metadata_variant: Option<&str>) -> ctx_traits_core::Trait {
        let variant_line = metadata_variant
            .map(|value| format!("variant = \"{value}\"\n"))
            .unwrap_or_default();
        let text = format!(
            "id = \"{id}\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Fixture\"\nsummary = \"Minimal fixture.\"\n{}",
            if metadata_variant.is_some() {
                format!("[metadata]\n{variant_line}")
            } else {
                String::new()
            }
        );
        ctx_traits_core::encoding::decode_trait(ctx_traits_core::encoding::Encoding::Toml, &text)
            .expect("minimal trait decodes")
    }

    #[test]
    fn guide_tool_less_rejects_assignment_extra_args() {
        let assignment = ProfileAssignment {
            extra_args: vec!["--enable-tools".to_string()],
            ..ProfileAssignment::default()
        };
        assert!(validate_assignment_common("guide", &assignment).is_err());
    }

    #[test]
    fn decodes_port_defaults_in_authorized_runtime_and_sidecar_shapes() {
        let runtime: RuntimeConfig =
            toml::from_str("[trait.example.defaults.port]\nplan = \"scoped\"\n")
                .expect("runtime defaults decode");
        assert_eq!(
            runtime
                .trait_defaults
                .get("example")
                .and_then(|value| value.defaults.port.get("plan")),
            Some(&"scoped".to_string())
        );
        let sidecar: TraitRunConfig = toml::from_str("[defaults.port]\nplan = \"sidecar\"\n")
            .expect("sidecar defaults decode");
        assert_eq!(
            sidecar.defaults.port.get("plan"),
            Some(&"sidecar".to_string())
        );
        assert!(toml::from_str::<RuntimeConfig>("[defaults.port]\nplan = \"base\"\n").is_err());
        assert!(
            toml::from_str::<RuntimeConfig>("[trait.example.defaults.port]\nplan = 42\n").is_err()
        );
        assert!(toml::from_str::<TraitRunConfig>("[defaults.port]\nplan = 42\n").is_err());
    }

    #[test]
    fn selected_runtime_port_defaults_override_sidecar_defaults() {
        let runtime: RuntimeConfig = toml::from_str(
            "[trait.example.defaults.port]\nplan = \"runtime-plan\"\nnotes = \"runtime-notes\"\n",
        )
        .expect("runtime defaults decode");
        let sidecar: TraitRunConfig =
            toml::from_str("[defaults.port]\nplan = \"sidecar-plan\"\nrule = \"sidecar-rule\"\n")
                .expect("sidecar defaults decode");
        let mut defaults = BTreeMap::new();
        for (port, value) in &runtime.trait_defaults["example"].defaults.port {
            defaults.insert(
                port.clone(),
                ConfiguredPortDefault {
                    value: value.clone(),
                    layer: ConfigLayer::Repo,
                    evidence: format!(".ctx/config.toml:trait.example.defaults.port.{port}"),
                },
            );
        }
        for (port, value) in sidecar.defaults.port {
            defaults
                .entry(port.clone())
                .or_insert(ConfiguredPortDefault {
                    value,
                    layer: ConfigLayer::BuiltIn,
                    evidence: format!("package/config.toml:defaults.port.{port}"),
                });
        }

        assert_eq!(defaults["plan"].value, "runtime-plan");
        assert_eq!(defaults["plan"].layer, ConfigLayer::Repo);
        assert_eq!(defaults["notes"].value, "runtime-notes");
        assert_eq!(defaults["rule"].value, "sidecar-rule");
        assert_eq!(defaults["rule"].layer, ConfigLayer::BuiltIn);
    }

    #[test]
    fn configured_port_defaults_reject_unknown_ports_with_origin() {
        let trait_ref = ctx_traits_core::encoding::decode_trait(
            ctx_traits_core::encoding::Encoding::Toml,
            "id = \"example\"\nschema-version = \"0.3\"\nversion = \"0.1.0\"\nname = \"Fixture\"\nsummary = \"Fixture.\"\n\n[[port]]\nid = \"plan\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Plan path\"\n",
        )
        .expect("trait decodes");
        let defaults = BTreeMap::from([(
            "missing".to_string(),
            ConfiguredPortDefault {
                value: ".plans/MISSING.md".to_string(),
                layer: ConfigLayer::Environment,
                evidence: "$CTX_CONFIG:trait.example.defaults.port.missing".to_string(),
            },
        )]);

        let error =
            validate_port_defaults(&trait_ref, &defaults).expect_err("unknown port rejects");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("$CTX_CONFIG:trait.example.defaults.port.missing"));
        assert!(rendered.contains("unknown input port \"missing\""));
    }

    #[test]
    fn runtime_budget_overlay_preserves_unspecified_sidecar_leaves() {
        let sidecar: TraitRunConfig =
            toml::from_str("[budget]\nmax-frames = 3\nframe-seconds = 60\n")
                .expect("sidecar budget decodes");
        let runtime: RuntimeConfig =
            toml::from_str("[run]\nframe-seconds = 15\n").expect("runtime budget decodes");
        let mut effective = sidecar.budget;
        overlay_budget(&mut effective, &runtime.run.expect("run table").budget);

        assert_eq!(effective.max_frames, Some(3));
        assert_eq!(effective.frame_seconds, Some(15));
    }

    #[test]
    fn trait_port_defaults_merge_leafwise_and_environment_wins_with_provenance() {
        let parse = |text: &str| toml::from_str::<RuntimeConfig>(text).expect("config decodes");
        let mut effective = RuntimeConfig::default();
        let mut winners = BTreeMap::new();
        merge_machine_config(
            &mut effective,
            parse(
                "[trait.example.defaults.port]\nplan = \"global-plan\"\nnotes = \"global-notes\"\n",
            ),
            ConfigLayer::UserGlobal,
            Some("global.toml".into()),
            &mut winners,
        );
        merge_machine_config(
            &mut effective,
            parse("[trait.example.defaults.port]\nplan = \"repo-plan\"\n"),
            ConfigLayer::Repo,
            Some(".ctx/config.toml".into()),
            &mut winners,
        );
        apply_environment_defaults(
            &mut effective,
            &parse("[trait.example.defaults.port]\nplan = \"environment-plan\"\n"),
            ConfigLayer::Environment,
            Some("$CTX_CONFIG".into()),
            &mut winners,
        );

        let ports = &effective.trait_defaults["example"].defaults.port;
        assert_eq!(ports["plan"], "environment-plan");
        assert_eq!(ports["notes"], "global-notes");
        let winner = &winners["trait.example.defaults.port.plan"];
        assert_eq!(winner.layer, ConfigLayer::Environment);
        assert_eq!(winner.source.as_deref(), Some("$CTX_CONFIG"));
        assert_eq!(
            winners["trait.example.defaults.port.notes"].layer,
            ConfigLayer::UserGlobal
        );
    }

    #[test]
    fn guide_requires_its_own_config_table_before_an_override_can_apply() {
        let mut profile = ResolvedRuntimeAssignments {
            registry: HarnessRegistry::default(),
            assignments: BTreeMap::from([("guide".to_string(), single("override-harness"))]),
            seat_assignments: BTreeMap::new(),
            agent_defaults: AgentDefaults {
                role: BTreeMap::from([(
                    DEFAULT_SEAT.to_string(),
                    RoleAssignmentValue::Single(single("default-harness")),
                )]),
                ..AgentDefaults::default()
            },
            qualifier_by_role: BTreeMap::new(),
            budget: RunProfileBudget::default(),
            worktree: WorktreeConfig::default(),
            port_defaults: BTreeMap::new(),
            model_catalogs: BTreeMap::new(),
            model_catalog_capability_reports: Vec::new(),
            builtin_detection: None,
            builtin_fallback_selections: BTreeMap::new(),
        };
        assert_eq!(profile.guide_assignment(), None);

        profile.agent_defaults.role.insert(
            "guide".to_string(),
            RoleAssignmentValue::Single(single("configured-harness")),
        );
        assert_eq!(
            profile
                .guide_assignment()
                .and_then(|assignment| assignment.harness),
            Some("override-harness".to_string())
        );
    }

    // -- 0079: `RunTransport::Api` -----------------------------------------

    #[test]
    fn transport_api_parses_from_toml() {
        let config: RuntimeConfig = toml::from_str(
            "[agent.role.narrator]\ntransport = \"api\"\nmodel = \"gpt-4o-mini\"\nbase-url = \"https://openrouter.ai/api/v1\"\nwire = \"openai-compat\"\napi-key-env = \"OPENROUTER_API_KEY\"\n",
        )
        .expect("api transport decodes");
        let RoleAssignmentValue::Single(assignment) = &config.agent.role["narrator"] else {
            panic!("expected a single-table assignment");
        };
        assert_eq!(assignment.transport, Some(RunTransport::Api));
        assert_eq!(
            assignment.api.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(assignment.api.wire, Some(ProviderWire::OpenaiCompat));
        assert_eq!(
            assignment.api.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
    }

    // -----------------------------------------------------------------
    // 0025: role-array seat expansion
    // -----------------------------------------------------------------

    #[test]
    fn expand_role_seats_count_form_expands_and_keeps_base() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(2),
                    ..single("shared-harness")
                }),
            )]),
            ..AgentDefaults::default()
        };
        expand_role_seats(&mut defaults);

        assert!(defaults.role.contains_key("smart"), "base key retained");
        let seat_1 = defaults.role["smart-1"].entries()[0].clone();
        let seat_2 = defaults.role["smart-2"].entries()[0].clone();
        assert_eq!(seat_1.harness.as_deref(), Some("shared-harness"));
        assert_eq!(seat_2.harness.as_deref(), Some("shared-harness"));
        assert_eq!(seat_1.count, None, "count cleared on the expanded seat");
        assert!(!defaults.role["smart-1"].is_list());
        assert!(!defaults.role.contains_key("smart-3"));
    }

    #[test]
    fn expand_role_seats_list_form_expands_with_differing_entries() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::List(vec![single("harness-a"), single("harness-b")]),
            )]),
            ..AgentDefaults::default()
        };
        expand_role_seats(&mut defaults);

        assert_eq!(
            defaults.role["smart-1"].entries()[0].harness.as_deref(),
            Some("harness-a")
        );
        assert_eq!(
            defaults.role["smart-2"].entries()[0].harness.as_deref(),
            Some("harness-b")
        );
        assert!(defaults.role.contains_key("smart"), "base key retained");
    }

    #[test]
    fn expand_role_seats_never_overwrites_an_authored_exact_table() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([
                (
                    "smart".into(),
                    RoleAssignmentValue::Single(ProfileAssignment {
                        count: Some(2),
                        ..single("shared-harness")
                    }),
                ),
                (
                    "smart-1".into(),
                    RoleAssignmentValue::Single(single("authored-harness")),
                ),
            ]),
            ..AgentDefaults::default()
        };
        expand_role_seats(&mut defaults);

        assert_eq!(
            defaults.role["smart-1"].entries()[0].harness.as_deref(),
            Some("authored-harness"),
            "authored exact table wins wholesale"
        );
        assert_eq!(
            defaults.role["smart-2"].entries()[0].harness.as_deref(),
            Some("shared-harness")
        );
    }

    #[test]
    fn transport_api_round_trips_through_serialize() {
        let mut assignment = ProfileAssignment {
            transport: Some(RunTransport::Api),
            model: Some("claude-haiku-4-5".into()),
            ..ProfileAssignment::default()
        };
        assignment.api.base_url = Some("https://api.anthropic.com".into());
        assignment.api.wire = Some(ProviderWire::Anthropic);
        assignment.api.api_key_env = Some("ANTHROPIC_API_KEY".into());
        let text = toml::to_string(&assignment).expect("assignment serializes");
        assert!(text.contains("transport = \"api\""));
        assert!(text.contains("base-url = \"https://api.anthropic.com\""));
        assert!(text.contains("wire = \"anthropic\""));
        assert!(text.contains("api-key-env = \"ANTHROPIC_API_KEY\""));
    }

    #[test]
    fn assignment_with_no_api_fields_serializes_without_them() {
        let assignment = single("claude");
        let text = toml::to_string(&assignment).expect("assignment serializes");
        assert!(!text.contains("base-url"));
        assert!(!text.contains("api-key-env"));
        assert!(!text.contains("\nwire "));
    }

    fn api_assignment(model: Option<&str>, base_url: Option<&str>) -> ProfileAssignment {
        ProfileAssignment {
            mode: RunAssignmentMode::Harness,
            transport: Some(RunTransport::Api),
            model: model.map(String::from),
            api: Box::new(ApiEndpoint {
                base_url: base_url.map(String::from),
                wire: Some(ProviderWire::OpenaiCompat),
                api_key_env: Some("SOME_API_KEY".into()),
                ..ApiEndpoint::default()
            }),
            ..ProfileAssignment::default()
        }
    }

    #[test]
    fn transport_api_requires_base_url() {
        let assignment = api_assignment(Some("gpt-4o-mini"), None);
        let error = validate_assignment("narrator", &assignment).expect_err("must reject");
        assert!(error.to_string().contains("base-url"));
    }

    #[test]
    fn transport_api_requires_model() {
        let assignment = api_assignment(None, Some("https://example.com/v1"));
        let error = validate_assignment("narrator", &assignment).expect_err("must reject");
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn transport_api_accepts_a_fully_declared_one_shot_seat() {
        let assignment = api_assignment(Some("gpt-4o-mini"), Some("https://example.com/v1"));
        validate_assignment("narrator", &assignment).expect("must accept");
    }

    #[test]
    fn transport_api_is_rejected_on_the_worker_seat() {
        let assignment = api_assignment(Some("gpt-4o-mini"), Some("https://example.com/v1"));
        let error =
            validate_assignment(DEFAULT_SEAT, &assignment).expect_err("must reject worker seat");
        assert!(error.to_string().contains("default"));
    }

    #[test]
    fn transport_api_does_not_require_a_harness_declaration() {
        // A seat may still declare a harness fallback (dispatch resolution
        // owns the precedence), but config validation must not force one.
        let assignment = api_assignment(Some("gpt-4o-mini"), Some("https://example.com/v1"));
        assert!(assignment.harness.is_none());
        validate_assignment("narrator", &assignment).expect("api transport needs no harness");
    }

    #[test]
    fn transport_api_key_env_stays_a_name_never_a_value() {
        let config: RuntimeConfig = toml::from_str(
            "[agent.role.narrator]\ntransport = \"api\"\nmodel = \"m\"\nbase-url = \"https://example.com\"\napi-key-env = \"MY_SECRET_KEY_NAME\"\n",
        )
        .expect("decodes");
        let RoleAssignmentValue::Single(assignment) = &config.agent.role["narrator"] else {
            panic!("expected a single-table assignment");
        };
        // The declared value is a variable NAME, never resolved/interpreted
        // by config decoding itself.
        assert_eq!(
            assignment.api.api_key_env.as_deref(),
            Some("MY_SECRET_KEY_NAME")
        );
    }

    #[test]
    fn resolve_assignment_model_skips_harness_catalog_for_api_transport() {
        // A harness-declared model requires a harness to resolve its catalog
        // against; an api-transport model is an opaque string sent straight
        // to the endpoint, so resolving it must not require a harness even
        // though `assignment.harness` is unset.
        let assignment = api_assignment(Some("gpt-4o-mini"), Some("https://example.com/v1"));
        assert!(assignment.harness.is_none());
        let mut catalogs = BTreeMap::new();
        let mut capabilities = Vec::new();
        let resolved = resolve_assignment_model(
            &HarnessRegistry::default(),
            &mut catalogs,
            &mut capabilities,
            assignment,
        )
        .expect("api-transport model resolution must not require a harness");
        assert_eq!(resolved.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn merge_assignment_fields_overlays_api_fields_independently() {
        let mut base = api_assignment(Some("base-model"), Some("https://base.example.com"));
        let next = ProfileAssignment {
            api: Box::new(ApiEndpoint {
                base_url: Some("https://override.example.com".into()),
                ..ApiEndpoint::default()
            }),
            ..ProfileAssignment::default()
        };
        merge_assignment_fields(&mut base, &next);
        assert_eq!(
            base.api.base_url.as_deref(),
            Some("https://override.example.com")
        );
        // Fields `next` left unset survive from `base` untouched.
        assert_eq!(base.model.as_deref(), Some("base-model"));
        assert_eq!(base.api.wire, Some(ProviderWire::OpenaiCompat));
    }

    #[test]
    fn expand_role_seats_runs_after_scope_merge_so_a_qualifier_count_wins() {
        // Regression for the ordering contract: `expand_role_seats` must be
        // called on the flattened (post-merge) result, or a nearer scope's
        // `count` would arrive too late to change the expansion.
        let mut base = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(2),
                    ..single("base-harness")
                }),
            )]),
            ..AgentDefaults::default()
        };
        let nearer = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(1),
                    ..ProfileAssignment::default()
                }),
            )]),
            ..AgentDefaults::default()
        };
        merge_agent_defaults(&mut base, nearer);
        expand_role_seats(&mut base);

        assert!(base.role.contains_key("smart-1"));
        assert!(
            !base.role.contains_key("smart-2"),
            "the merged count = 1 must have already won before expansion runs"
        );
    }

    #[test]
    fn expand_role_seats_zero_count_expands_no_seats() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(0),
                    ..single("shared-harness")
                }),
            )]),
            ..AgentDefaults::default()
        };
        expand_role_seats(&mut defaults);
        assert!(!defaults.role.contains_key("smart-1"));
    }

    #[test]
    fn expansion_seat_out_of_range_errors_naming_base_and_seat_count() {
        let mut defaults = AgentDefaults {
            role: BTreeMap::from([(
                "smart".into(),
                RoleAssignmentValue::Single(ProfileAssignment {
                    count: Some(2),
                    ..single("shared-harness")
                }),
            )]),
            ..AgentDefaults::default()
        };
        expand_role_seats(&mut defaults);

        assert!(expansion_seat_out_of_range(&defaults, "smart-1").is_none());
        assert!(expansion_seat_out_of_range(&defaults, "smart-2").is_none());
        let err = expansion_seat_out_of_range(&defaults, "smart-3")
            .expect("seat 3 exceeds the configured count of 2");
        let message = err.to_string();
        assert!(message.contains("smart"), "names the base role: {message}");
        assert!(message.contains('2'), "names the seat count: {message}");
    }

    #[test]
    fn expansion_seat_out_of_range_is_none_for_a_non_expansion_shaped_role() {
        let defaults = AgentDefaults {
            role: BTreeMap::from([(
                "worker".into(),
                RoleAssignmentValue::Single(single("shared-harness")),
            )]),
            ..AgentDefaults::default()
        };
        assert!(expansion_seat_out_of_range(&defaults, "worker-1").is_none());
        assert!(expansion_seat_out_of_range(&defaults, "unrelated").is_none());
    }

    #[test]
    fn validate_role_map_rejects_count_on_a_standing_seat() {
        let role = BTreeMap::from([(
            "narrator".into(),
            RoleAssignmentValue::Single(ProfileAssignment {
                count: Some(2),
                ..ProfileAssignment::default()
            }),
        )]);
        let err = validate_role_map(&role, "agent.role", true)
            .expect_err("count on a standing seat must be rejected");
        assert!(err.to_string().contains("count"));
    }

    #[test]
    fn validate_role_map_rejects_count_inside_a_list_entry() {
        let role = BTreeMap::from([(
            "smart".into(),
            RoleAssignmentValue::List(vec![ProfileAssignment {
                count: Some(1),
                ..single("a")
            }]),
        )]);
        let err = validate_role_map(&role, "agent.role", true)
            .expect_err("count inside a [[...]] seat is rejected; the list length is the count");
        assert!(err.to_string().contains("count"));
    }

    #[test]
    fn validate_role_map_rejects_zero_count() {
        let role = BTreeMap::from([(
            "smart".into(),
            RoleAssignmentValue::Single(ProfileAssignment {
                count: Some(0),
                ..single("a")
            }),
        )]);
        let err =
            validate_role_map(&role, "agent.role", true).expect_err("count must be at least 1");
        assert!(err.to_string().contains("count"));
    }

    #[test]
    fn profile_assignment_count_round_trips_and_stays_absent_when_unauthored() {
        let without_count = single("harness-only");
        let serialized = toml::to_string(&without_count).expect("serializes");
        assert!(
            !serialized.contains("count"),
            "un-authored count must not appear on the wire: {serialized}"
        );

        let with_count = ProfileAssignment {
            count: Some(3),
            ..single("harness-only")
        };
        let serialized = toml::to_string(&with_count).expect("serializes");
        assert!(serialized.contains("count = 3"));
        let decoded: ProfileAssignment =
            toml::from_str(&serialized).expect("round-trips through TOML");
        assert_eq!(decoded.count, Some(3));
    }

    // P568/0035 invariant 1: layered config is collapsed at ONE resolution
    // point (`merge_built_in_harness_overrides`, called once inside
    // `resolve_runtime_config`), so a raw `.get(id)` afterwards is always
    // correct. These three tests each lock one of the named casualties.

    #[test]
    fn raw_lookup_on_resolved_registry_equals_merged_value() {
        let mut registry: HarnessRegistry = toml::from_str(
            r#"
            [harness.claude-code]
            bin = "custom-claude"
            "#,
        )
        .expect("partial override decodes");

        merge_built_in_harness_overrides(&mut registry.harness);

        let raw = registry
            .harness
            .get("claude-code")
            .expect("claude-code is materialized after the merge");
        // The stated field won...
        assert_eq!(raw.bin(), "custom-claude");
        // ...and an untouched field still carries the built-in's value: a
        // half-defined harness (P568's narrator casualty) would have lost
        // this instead of inheriting it.
        assert_eq!(
            raw.cli.as_ref().and_then(|cli| cli.model_flag.as_deref()),
            Some("--model")
        );
        // A plain lookup on the resolved document is the merged value —
        // exactly the property the choke point exists to guarantee.
        assert_eq!(raw, &built_in_harness_definition("claude-code", &registry));
    }

    #[test]
    fn empty_harness_section_materializes_every_built_in() {
        let mut harness: BTreeMap<String, HarnessDefinition> = BTreeMap::new();

        merge_built_in_harness_overrides(&mut harness);

        for (id, definition) in built_in_harness_definitions() {
            assert_eq!(
                harness.get(id),
                Some(&definition),
                "built-in `{id}` must be materialized even with no `[harness]` section \
                 stated for it, or every lookup fails with \"unknown harness id\""
            );
        }
    }

    #[test]
    fn explicit_unset_survives_resolution_and_is_not_reinherited() {
        let mut registry: HarnessRegistry = toml::from_str(
            r#"
            [harness.claude-code.cli]
            model-flag = ""
            "#,
        )
        .expect("explicit-unset override decodes");

        merge_built_in_harness_overrides(&mut registry.harness);
        let resolved = registry
            .harness
            .get("claude-code")
            .expect("claude-code is materialized after the merge");
        assert_eq!(
            resolved
                .cli
                .as_ref()
                .and_then(|cli| cli.model_flag.as_deref()),
            None,
            "an explicit `model-flag = \"\"` must resolve to None, not inherit the built-in's flag"
        );

        // A second merge pass over the already-resolved map is NOT
        // idempotent: it re-inherits the built-in's value, because the
        // explicit unset has already been applied to `None` and a `None`
        // looks identical to "not stated" to the merge. This is why
        // resolution runs exactly once and lookup sites must never
        // "merge defensively".
        merge_built_in_harness_overrides(&mut registry.harness);
        let reinherited = registry
            .harness
            .get("claude-code")
            .expect("claude-code is still present after the second pass");
        assert_eq!(
            reinherited
                .cli
                .as_ref()
                .and_then(|cli| cli.model_flag.as_deref()),
            Some("--model"),
            "a second merge pass re-inherits the built-in's flag, demonstrating why \
             merging must happen exactly once"
        );
    }
}
