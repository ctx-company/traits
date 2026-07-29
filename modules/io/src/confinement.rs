//! P478 harness-native write confinement, generated and injected per
//! worktree spawn.
//!
//! A `--worktree` run must be unable to write outside its own worktree,
//! enforced by config the dispatcher generates mechanically — zero prompt
//! text, zero agent cooperation. This module owns the whole generator: main
//! repo/worktree resolution, deny enumeration, per-harness-kind payload
//! rendering, and the deep-merge into an existing `OPENCODE_CONFIG_CONTENT`
//! overlay value. This is defense against accidental/mechanical out-of-tree
//! writes by a cooperating harness, not a security boundary against a
//! hostile process — the enumeration only covers paths known at generation
//! time, and an interpreter one-liner through opencode's shell tool is not
//! gated by its `edit`/`external_directory` permissions at all.

use std::collections::BTreeMap;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::Deserialize;

/// `[worktree.confinement]`: the three keys this phase owns. `enabled`
/// defaults on; `sandbox` means OS-level enforcement (P480): it gates both
/// claude-code's own `sandbox.enabled` layer AND the generated
/// `sandbox-exec` wrapper every worktree spawn is run under on macOS (deny
/// rules stay on regardless of this flag — turning `sandbox` off only drops
/// the OS-enforced layer, e.g. as an escape hatch when a toolchain fights the
/// generated profile). `allow` names extra writable directories —
/// repository-relative (resolved against the main repo root), absolute, or
/// `~`/`$HOME`-prefixed — excluded from the deny enumeration and added to
/// `additionalDirectories`/the sandbox profile's allow-list, for
/// toolchain-owned directories the generator cannot infer from the resolved
/// `[worktree.env]` overlay (e.g. `~/.cargo`, a pnpm store).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorktreeConfinementConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub sandbox: bool,
    #[serde(default)]
    pub allow: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for WorktreeConfinementConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sandbox: true,
            allow: Vec::new(),
        }
    }
}

/// One denied path, discovered by the sibling-walk enumeration below.
/// Ordered so a sorted, deduped `Vec<DenyEntry>` is deterministic regardless
/// of filesystem read-dir order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DenyEntry {
    Dir(Utf8PathBuf),
    File(Utf8PathBuf),
}

/// A generated, harness-kind-agnostic confinement plan: canonical main repo
/// root, canonical writable carve-outs (the worktree itself plus any
/// out-of-worktree directory the run legitimately writes), the resulting
/// deny enumeration, and the `sandbox` flag from config. Two renderers
/// Three renderers (claude-code, opencode, codex) turn this one plan into
/// their own dialects — the
/// walk itself is written once.
#[derive(Debug, Clone)]
pub struct ConfinementPlan {
    pub main_root: Utf8PathBuf,
    /// Sorted, deduped.
    pub deny: Vec<DenyEntry>,
    /// Sorted, deduped. Always contains at least the worktree root itself.
    pub additional_directories: Vec<Utf8PathBuf>,
    pub sandbox: bool,
}

/// Build a [`ConfinementPlan`] for a worktree run: `main_root` and
/// `worktree_root` always exist (a real repository root and the actual
/// execution directory of this spawn), so they are canonicalized via
/// `fs::canonicalize` and a failure there is a genuine fault. Every entry of
/// `extra_carve_outs`, by contrast, is an *allowance* rather than a
/// precondition — a build cache or package store the toolchain creates
/// lazily on first use is normal, not a fault — so each is canonicalized
/// best-effort and falls back to its lexically-normalized absolute path when
/// it does not exist yet, via [`canonicalize_carve_out`]. Every carve-out
/// (worktree plus extras) is then deduped and split into carve-outs that
/// live inside `main_root` (subject to the sibling-walk deny enumeration)
/// and carve-outs that live outside it (already unreachable from a
/// `main_root`-anchored deny, so they need no enumeration — only
/// `additionalDirectories` membership for the sandbox layer).
///
/// When no in-root carve-out exists (e.g. the worktree itself was relocated
/// outside the repository — the P480 design lever), the deny enumeration
/// collapses to exactly one blanket `Dir(main_root)` rule rather than
/// walking at all.
pub fn build_confinement_plan(
    main_root: &Utf8Path,
    worktree_root: &Utf8Path,
    extra_carve_outs: &[Utf8PathBuf],
    config: &WorktreeConfinementConfig,
) -> crate::Result<ConfinementPlan> {
    let main_root = canonicalize(main_root)?;
    let mut carve_outs = vec![canonicalize(worktree_root)?];
    for extra in extra_carve_outs {
        carve_outs.push(canonicalize_carve_out(extra));
    }
    carve_outs.sort();
    carve_outs.dedup();

    // A carve-out that IS `main_root` or an ancestor of it would, once
    // rendered as an allow rule by a deny-overriding renderer (e.g.
    // opencode's `external_directory`), cover the entire invocation repo and
    // silently neutralize every deny in play — turned on by ordinary config
    // (`[worktree.confinement] allow = ["."]`, or an overlay value like
    // `HOME=/Users/<user>`), not a hostile input. Drop any such carve-out
    // before it ever reaches a renderer or `additional_directories`: every
    // carve-out that survives is either strictly inside `main_root` (subject
    // to the deny enumeration below) or fully disjoint from it, so widening
    // `additional_directories` can only ever add a directory the deny
    // enumeration does not also need to cover.
    carve_outs.retain(|path| !main_root.starts_with(path));

    let in_root: Vec<Utf8PathBuf> = carve_outs
        .iter()
        .filter(|path| path.starts_with(&main_root))
        .cloned()
        .collect();

    let deny = if in_root.is_empty() {
        vec![DenyEntry::Dir(main_root.clone())]
    } else {
        let components: Vec<Vec<String>> = in_root
            .iter()
            .map(|path| relative_components(&main_root, path))
            .collect::<crate::Result<Vec<_>>>()?;
        let mut denies = Vec::new();
        enumerate_denies(&main_root, &components, &mut denies)?;
        denies.sort();
        denies.dedup();
        denies
    };

    Ok(ConfinementPlan {
        main_root,
        deny,
        additional_directories: carve_outs,
        sandbox: config.sandbox,
    })
}

fn canonicalize(path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    crate::state::canonical_repo_root(path)
}

/// Canonicalize a writable carve-out, tolerating a path that does not exist
/// yet: a carve-out is an allowance to grant, not an input that must already
/// be on disk, so a not-yet-materialized build cache or package store falls
/// back to its lexically-normalized absolute path rather than failing the
/// whole plan. The fallback path still lands in `additional_directories` and
/// is still excluded from the deny enumeration (see [`build_confinement_plan`]),
/// so the sandbox permits creating it on first use. Only `main_root` and
/// `worktree_root` — which always exist — use the hard-failing
/// [`canonicalize`].
fn canonicalize_carve_out(path: &Utf8Path) -> Utf8PathBuf {
    canonicalize(path).unwrap_or_else(|_| normalize_lexically(path))
}

/// Lexically resolve `.`/`..` components of an already-absolute path without
/// touching the filesystem — the non-existent-path fallback for
/// [`canonicalize_carve_out`].
fn normalize_lexically(path: &Utf8Path) -> Utf8PathBuf {
    let mut normalized = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Expand a leading `~` or `~/...` in an `allow` entry against the effective
/// spawn env's own `HOME` (falling back to the host process's `HOME` when the
/// overlay does not set one) — resolved against the child's world, not the
/// parent's. Without this, PRODUCT.md's own documented `allow = ["~/.cargo"]`
/// example resolved (via the non-absolute, `main_root`-relative branch below)
/// to a nonexistent `<main_root>/~/.cargo`, a harmless nicety under P478's
/// glob-based deny enumeration but a working-build-breaking gap once P480
/// makes `allow` the only way to open a real toolchain write path under the
/// sandbox. Any other value (already absolute, or repository-relative) passes
/// through unchanged.
fn expand_home(value: &str, env: &BTreeMap<String, String>) -> String {
    let Some(rest) = value.strip_prefix('~') else {
        return value.to_string();
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        // `~someuser/...` — not a `$HOME` expansion this function handles.
        return value.to_string();
    }
    let Some(home) = env
        .get("HOME")
        .cloned()
        .or_else(|| std::env::var("HOME").ok())
    else {
        return value.to_string();
    };
    format!("{home}{rest}")
}

fn relative_components(root: &Utf8Path, path: &Utf8Path) -> crate::Result<Vec<String>> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| crate::git_process::message_error(format!("{path} is not inside {root}")))?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            Utf8Component::Normal(name) => Some(name.to_string()),
            _ => None,
        })
        .collect())
}

/// Sibling-walk one directory level: everything not on the path to a carve-out
/// is denied (as a directory-wide `**` rule, or a single file), and every
/// entry that continues toward a carve-out is recursed into rather than
/// denied. An entry whose remaining carve-out path is exactly this entry (the
/// carve-out itself) is neither denied nor recursed into — its whole subtree
/// is left unrestricted. `read_dir` entries are sorted by name before
/// iteration so the resulting deny list never depends on filesystem order.
fn enumerate_denies(
    dir: &Utf8Path,
    carve_outs: &[Vec<String>],
    denies: &mut Vec<DenyEntry>,
) -> crate::Result<()> {
    // A directory on the path to a not-yet-materialized carve-out (e.g. an
    // `allow`-listed build cache before its first build) does not exist yet
    // either — nothing to enumerate at this level, not a fault: once it is
    // created, everything under it is on the path to the allowed carve-out
    // by construction, so there is nothing here that needs denying.
    let read_dir = match std::fs::read_dir(dir.as_std_path()) {
        Ok(read_dir) => read_dir,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(crate::environment::Error::Filesystem {
                path: dir.to_string(),
                source,
            }
            .into());
        }
    };
    let mut entries: Vec<std::fs::DirEntry> = read_dir
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let matches: Vec<&Vec<String>> = carve_outs
            .iter()
            .filter(|carve_out| carve_out.first() == Some(&name))
            .collect();

        if matches.iter().any(|carve_out| carve_out.len() == 1) {
            // This entry IS a carve-out (or the ancestor of one that also
            // terminates here): fully writable, no deny, no recursion.
            continue;
        }
        if !matches.is_empty() {
            let continuing: Vec<Vec<String>> = matches
                .into_iter()
                .map(|carve_out| carve_out[1..].to_vec())
                .collect();
            let child = dir.join(&name);
            enumerate_denies(&child, &continuing, denies)?;
            continue;
        }

        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let child = dir.join(&name);
        denies.push(if is_dir {
            DenyEntry::Dir(child)
        } else {
            DenyEntry::File(child)
        });
    }
    Ok(())
}

/// The rendered per-harness-kind payloads for one [`ConfinementPlan`],
/// plus the harness-kind-agnostic P480 OS-level spawn sandbox.
#[derive(Debug, Clone)]
pub struct ConfinementPayloads {
    pub claude_code: serde_json::Value,
    pub opencode: serde_json::Value,
    /// Codex CLI overrides. `sandbox` is `workspace-write` unless P480's
    /// outer Seatbelt wrapper is active, in which case `danger-full-access`
    /// explicitly disables Codex's inner OS sandbox.
    pub codex: serde_json::Value,
    /// The generated OS-level (`sandbox-exec`) wrapper for this worktree's
    /// spawns, applied at the spawner seam identically regardless of
    /// `harness.kind`. `None` when `[worktree.confinement] sandbox = false`,
    /// or when this platform/machine has no OS layer available (see
    /// [`spawn_sandbox_unsupported_capability`]) — in either case `claude_code`
    /// and `opencode` above are unaffected, since they are the
    /// harness-native P478 layer, not this one.
    pub spawn_sandbox: Option<SpawnSandbox>,
    /// Whether `[worktree.confinement] sandbox` was `true` for this plan —
    /// carried alongside `spawn_sandbox` so a caller can tell "OS enforcement
    /// was requested but unavailable here" (report it) apart from "OS
    /// enforcement was turned off" (say nothing) without re-reading config.
    pub sandbox_requested: bool,
}

/// One generated OS-level (macOS `sandbox-exec`) write sandbox for a worktree
/// spawn: the argv prefix a caller prepends before `Command::new`/`args`, and
/// the profile text itself (carried separately so the debug trace can show it
/// without re-parsing `argv_prefix`).
#[derive(Debug, Clone)]
pub struct SpawnSandbox {
    /// Prepended to a harness argv at the spawn seam, e.g.
    /// `["/usr/bin/sandbox-exec", "-p", "<profile>"]`. Never enters
    /// `HarnessRunOutcome.argv`, a warm-pool identity key, or any ledger/
    /// provenance surface — operational-only, per PRODUCT.md's
    /// "Default-off / byte-identical" rule.
    pub argv_prefix: Vec<String>,
    pub profile: String,
}

/// Harness state directories enumerated by name rather than inferred — a
/// short, honest static list (see the P480 draft §4.3): a harness writing
/// somewhere unlisted fails loudly (EPERM) in the live gate rather than
/// silently, and is a one-line addition or an operator `allow` entry, not a
/// design change.
const HARNESS_STATE_DIRS: &[&str] = &[
    ".claude",
    ".claude.json",
    ".config/claude",
    ".local/share/opencode",
    ".config/opencode",
    ".cache/opencode",
];

/// `/usr/bin/sandbox-exec` presence on a macOS host — the whole downgrade
/// predicate (§4.6 of the P480 draft): any other platform, or a macOS host
/// missing the (formally deprecated but live-verified) binary, gets no
/// generated OS-level sandbox and a named capability report instead of
/// silently unenforced or falsely-claimed confinement.
fn sandbox_exec_available() -> bool {
    cfg!(target_os = "macos") && Utf8Path::new("/usr/bin/sandbox-exec").exists()
}

/// Render the P480 macOS Seatbelt profile for a [`ConfinementPlan`]:
/// `(allow default)` + `(deny file-write*)`, an allow-list built from
/// `plan.additional_directories` plus the child's effective `TMPDIR`, the
/// enumerated harness state directories, the global ctx root, and `/dev`
/// (wholesale — not a source-mutation surface, and enumerating device nodes
/// is fragile for no security benefit), and finally a carve/re-deny pair for
/// the worktree's shared `<main>/.git` (git needs to write `objects/`,
/// `refs/`, `packed-refs` there; `hooks/` and `config` are re-denied LAST so
/// SBPL's later-rule-wins semantics keep them out of reach — the two files
/// that turn a repo write into arbitrary code execution or identity forgery).
/// No `(deny network*)` and no claim about network confinement — out of
/// scope for this phase.
pub fn render_macos_sandbox_profile(
    plan: &ConfinementPlan,
    env: &BTreeMap<String, String>,
) -> crate::Result<SpawnSandbox> {
    let home = match env.get("HOME") {
        Some(value) => Utf8PathBuf::from(value.as_str()),
        None => crate::state::home_dir()?,
    };

    let mut allow_dirs: Vec<Utf8PathBuf> = plan.additional_directories.clone();
    let tmp_dir = env
        .get("TMPDIR")
        .map(|value| canonicalize_carve_out(Utf8Path::new(value)))
        .unwrap_or_else(|| Utf8PathBuf::from("/private/tmp"));
    allow_dirs.push(tmp_dir);
    allow_dirs.push(Utf8PathBuf::from("/private/tmp"));
    allow_dirs.push(Utf8PathBuf::from("/private/var/folders"));
    for state_dir in HARNESS_STATE_DIRS {
        allow_dirs.push(canonicalize_carve_out(&home.join(state_dir)));
    }
    allow_dirs.push(canonicalize_carve_out(&crate::state::global_ctx_root()?));
    allow_dirs.sort();
    allow_dirs.dedup();

    let git_dir = plan.main_root.join(".git");
    let git_hooks = git_dir.join("hooks");
    let git_config = git_dir.join("config");

    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    for dir in &allow_dirs {
        profile.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            escape_sbpl(dir.as_str())
        ));
    }
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    profile.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        escape_sbpl(git_dir.as_str())
    ));
    profile.push_str(&format!(
        "(deny file-write* (subpath \"{}\") (literal \"{}\"))\n",
        escape_sbpl(git_hooks.as_str()),
        escape_sbpl(git_config.as_str())
    ));

    Ok(SpawnSandbox {
        argv_prefix: vec![
            "/usr/bin/sandbox-exec".to_string(),
            "-p".to_string(),
            profile.clone(),
        ],
        profile,
    })
}

/// Minimal SBPL string-literal escape: `\` and `"` are the only characters
/// that can break out of an SBPL string.
fn escape_sbpl(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The one gate that decides whether a plan gets a generated OS-level
/// sandbox: `[worktree.confinement] sandbox` was requested AND this host
/// actually has `sandbox-exec`. Split out from [`resolve_worktree_spawn`] so
/// the off-switch/unsupported-platform behavior is unit-testable without a
/// real git repository.
fn generate_spawn_sandbox(
    plan: &ConfinementPlan,
    env: &BTreeMap<String, String>,
) -> crate::Result<Option<SpawnSandbox>> {
    if plan.sandbox && sandbox_exec_available() {
        Ok(Some(render_macos_sandbox_profile(plan, env)?))
    } else {
        Ok(None)
    }
}

/// A named, operator-visible capability report for the one case the P480
/// generator cannot silently cover: `[worktree.confinement] sandbox = true`
/// (OS enforcement was actually requested) but this spawn has no OS-level
/// sandbox available — a non-macOS platform, or a macOS host missing
/// `/usr/bin/sandbox-exec`. `None` both when the OS layer is available (the
/// ordinary case) and when the operator turned `sandbox` off deliberately —
/// that is an explicit choice, not a gap to report.
///
/// Derives "unavailable" from `spawn_sandbox` — the same payload every
/// caller already holds and passed to `render_claude_code_settings` — rather
/// than re-probing `sandbox_exec_available()` a second time per call site;
/// one source of truth for "did the wrapper actually apply to this spawn".
pub fn spawn_sandbox_unsupported_capability(
    sandbox_requested: bool,
    spawn_sandbox: Option<&SpawnSandbox>,
) -> Option<ctx_traits_core::response::CapabilityReport> {
    if !sandbox_requested || spawn_sandbox.is_some() {
        return None;
    }
    Some(ctx_traits_core::response::CapabilityReport::unsupported(
        "worktree.spawn-sandbox",
        format!(
            "this worktree spawn has no P480 OS-level write-sandbox enforcement on {} ({}); P479's out-of-tree mutation tripwire remains the backstop",
            std::env::consts::OS,
            if cfg!(target_os = "macos") {
                "/usr/bin/sandbox-exec not found"
            } else {
                "unsupported platform"
            }
        ),
    ))
}

/// `{"permissions":{"deny":[...],"additionalDirectories":[...]},"sandbox":{"enabled":<bool>}}`,
/// injected as `--settings <compact-json>`. Anchors are always `//`-absolute
/// — the `/path` settings-relative form is a documented footgun and is never
/// emitted here.
///
/// `spawn_sandbox_applied` enforces "at most one Seatbelt sandbox per spawn"
/// (P480 blocker `nested-seatbelt-double-apply`): macOS refuses a second
/// `sandbox_apply` inside an already-sandboxed process — live-verified,
/// `sandbox-exec` nested inside `sandbox-exec` dies with `sandbox_apply:
/// Operation not permitted` regardless of how permissive the outer profile
/// is — so when the P480 spawner wrapper is already applied to this spawn,
/// claude-code's own `sandbox.enabled` must render `false` even though
/// `plan.sandbox` (the operator's OS-enforcement request) is `true`; the
/// request was honored by the outer layer, not dropped. When the wrapper is
/// NOT applied to this spawn (non-macOS, `sandbox-exec` missing, or
/// `sandbox = false`), this renders exactly `plan.sandbox` as before — those
/// hosts still get claude-code's own OS layer, unchanged.
pub fn render_claude_code_settings(
    plan: &ConfinementPlan,
    spawn_sandbox_applied: bool,
) -> serde_json::Value {
    let deny: Vec<String> = plan
        .deny
        .iter()
        .map(|entry| match entry {
            DenyEntry::Dir(path) => format!("Edit(//{path}/**)"),
            DenyEntry::File(path) => format!("Edit(//{path})"),
        })
        .collect();
    let additional_directories: Vec<String> = plan
        .additional_directories
        .iter()
        .map(Utf8PathBuf::to_string)
        .collect();
    serde_json::json!({
        "permissions": {
            "deny": deny,
            "additionalDirectories": additional_directories,
        },
        "sandbox": {
            "enabled": plan.sandbox && !spawn_sandbox_applied,
        },
    })
}

/// Codex's native workspace-write sandbox and its repeatable writable
/// carve-outs. The same canonical, ancestor-filtered directories used by the
/// other renderers are emitted here; this is not a second path policy.
pub fn render_codex_payload(
    plan: &ConfinementPlan,
    spawn_sandbox_applied: bool,
) -> serde_json::Value {
    let additional_directories: Vec<String> = plan
        .additional_directories
        .iter()
        .map(Utf8PathBuf::to_string)
        .collect();
    serde_json::json!({
        "sandbox": if plan.sandbox && !spawn_sandbox_applied {
            "workspace-write"
        } else {
            "danger-full-access"
        },
        "add-directory": additional_directories,
    })
}

/// `{"permission":{"external_directory":{"*":"deny",<carve-out-globs>:"allow"},"edit":{<deny-globs>:"deny"}}}`.
/// `"*"` sorts before any `/`-anchored path key in `serde_json`'s
/// (non-`preserve_order`) `BTreeMap`-backed object, which is also the order
/// opencode's last-match-wins glob semantics need.
///
/// `external_directory` names every plan carve-out as an `allow` glob
/// alongside the blanket `"*": "deny"` — confirmed live against an installed
/// opencode (`opencode debug config` with `OPENCODE_CONFIG_CONTENT` set)
/// that `external_directory` accepts a path-keyed glob map, not only the
/// scalar form. Without this, a legitimate out-of-worktree write path (e.g.
/// this repo's own `[worktree.env] CARGO_TARGET_DIR`) is blanket-denied here
/// while `render_claude_code_settings` allows it via
/// `additionalDirectories` — the exact asymmetry that has already killed a
/// frame in practice (`.ctx/config.toml:171-175`).
///
/// As of 2026-07-27, OpenCode 1.17.18 documents an allow default for edits;
/// `--auto` only approves requests that would otherwise ask, and explicit
/// generated denies still win. Do not manufacture a wildcard allow here.
/// `merge_opencode_config_content` preserves an operator-supplied scalar or
/// wildcard edit default while this renderer owns only generated deny paths.
pub fn render_opencode_permission(plan: &ConfinementPlan) -> serde_json::Value {
    let mut edit = serde_json::Map::new();
    for entry in &plan.deny {
        let glob = match entry {
            DenyEntry::Dir(path) => format!("{path}/**"),
            DenyEntry::File(path) => path.to_string(),
        };
        edit.insert(glob, serde_json::Value::String("deny".to_string()));
    }
    let mut external_directory = serde_json::Map::new();
    external_directory.insert(
        "*".to_string(),
        serde_json::Value::String("deny".to_string()),
    );
    for carve_out in &plan.additional_directories {
        external_directory.insert(
            format!("{carve_out}/**"),
            serde_json::Value::String("allow".to_string()),
        );
    }
    serde_json::json!({
        "permission": {
            "external_directory": serde_json::Value::Object(external_directory),
            "edit": serde_json::Value::Object(edit),
        },
    })
}

/// The one env var this phase's opencode payload is delivered through — the
/// same channel this repository already ships a `permission.task` deny
/// through (P477), so composing here must never clobber it.
pub const OPENCODE_CONFIG_CONTENT_KEY: &str = "OPENCODE_CONFIG_CONTENT";

/// Deep-merge `ours` into an existing `OPENCODE_CONFIG_CONTENT` overlay
/// value, our keys winning on any object-vs-object key collision, and return
/// the merged document re-serialized. An existing value that is not valid
/// JSON is a hard error naming the key — fail-closed rather than silently
/// clobbering whatever the overlay author intended.
pub fn merge_opencode_config_content(
    existing: Option<&str>,
    ours: &serde_json::Value,
) -> crate::Result<String> {
    let mut merged = match existing {
        Some(raw) => serde_json::from_str::<serde_json::Value>(raw).map_err(|source| {
            config_error(
                OPENCODE_CONFIG_CONTENT_KEY,
                format!("existing {OPENCODE_CONFIG_CONTENT_KEY} is not valid JSON: {source}"),
            )
        })?,
        None => serde_json::Value::Object(serde_json::Map::new()),
    };
    let mut ours = ours.clone();
    // A scalar edit default would otherwise be replaced by our generated map.
    // Keep it as the map wildcard before the normal generated-wins deep merge.
    if let Some(default) = merged
        .pointer("/permission/edit")
        .filter(|value| value.is_string())
        .cloned()
    {
        if let Some(edit) = ours
            .pointer_mut("/permission/edit")
            .and_then(serde_json::Value::as_object_mut)
        {
            edit.insert("*".to_string(), default);
        }
    }
    deep_merge(&mut merged, &ours);
    Ok(merged.to_string())
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

fn deep_merge(base: &mut serde_json::Value, ours: &serde_json::Value) {
    let (serde_json::Value::Object(base_map), serde_json::Value::Object(ours_map)) = (base, ours)
    else {
        return;
    };
    for (key, value) in ours_map {
        match base_map.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                deep_merge(existing, value);
            }
            _ => {
                base_map.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Resolve the ONE effective spawn environment for a worktree subprocess —
/// `[worktree].env` (P342/P428, via
/// [`crate::harness_config::resolve_effective_worktree_env`]) with this
/// phase's opencode confinement payload deep-merged into
/// `OPENCODE_CONFIG_CONTENT` — plus the generated [`ConfinementPayloads`]
/// (`None` when confinement is disabled or this is not a worktree spawn, i.e.
/// `execution_dir.is_none()` — the exact predicate every worktree-spawn
/// caller already gates on). Every caller that already threads the resolved
/// overlay picks up confinement for free on the env side; the argv side
/// still needs `append_confinement` at each CLI argv builder.
pub fn resolve_worktree_spawn(
    worktree: &crate::harness_config::WorktreeConfig,
    execution_dir: Option<&Utf8Path>,
) -> crate::Result<(BTreeMap<String, String>, Option<ConfinementPayloads>)> {
    let base_env = crate::harness_config::resolve_effective_worktree_env(worktree, execution_dir)?;
    let Some(worktree_root) = execution_dir else {
        return Ok((base_env, None));
    };
    if !worktree.confinement.enabled {
        return Ok((base_env, None));
    }
    let main_root = crate::repository::discover_main_repo_root(worktree_root)?;
    // Deliberate divergence from PRODUCT.md's `[worktree.env]` path-resolution
    // rule (only a `.ctx/`/`./`/`../`-prefixed value is treated as a path;
    // every other scalar, including an arbitrary absolute one, passes through
    // verbatim): here, ANY absolute overlay value is treated as a writable
    // directory to carve out, since a false positive only ever widens the
    // set of carve-outs `build_confinement_plan` is given — and that
    // function itself drops any carve-out that IS or CONTAINS `main_root`
    // before it reaches a renderer, so an over-wide carve-out here can never
    // neutralize confinement over the invocation repo — while a false
    // negative would leave a real out-of-tree write path unconfined.
    let mut extra_carve_outs: Vec<Utf8PathBuf> = base_env
        .values()
        .filter_map(|value| {
            let path = Utf8Path::new(value);
            path.is_absolute().then(|| path.to_path_buf())
        })
        .collect();
    for allow in &worktree.confinement.allow {
        let expanded = expand_home(allow, &base_env);
        let path = Utf8Path::new(&expanded);
        extra_carve_outs.push(if path.is_absolute() {
            path.to_path_buf()
        } else {
            main_root.join(path)
        });
    }
    let plan = build_confinement_plan(
        &main_root,
        worktree_root,
        &extra_carve_outs,
        &worktree.confinement,
    )?;
    // P480: the harness-kind-agnostic OS-level layer, generated from the
    // same plan the harness-native renderers above already consume.
    // Reads the effective spawn env (`HOME`, `TMPDIR`), not the parent
    // process's — must be resolved against `base_env`, before it is consumed
    // below.
    let spawn_sandbox = generate_spawn_sandbox(&plan, &base_env)?;
    let payloads = ConfinementPayloads {
        claude_code: render_claude_code_settings(&plan, spawn_sandbox.is_some()),
        opencode: render_opencode_permission(&plan),
        codex: render_codex_payload(&plan, spawn_sandbox.is_some()),
        spawn_sandbox,
        sandbox_requested: plan.sandbox,
    };
    let mut env = base_env;
    let merged = merge_opencode_config_content(
        env.get(OPENCODE_CONFIG_CONTENT_KEY).map(String::as_str),
        &payloads.opencode,
    )?;
    env.insert(OPENCODE_CONFIG_CONTENT_KEY.to_string(), merged);
    Ok((env, Some(payloads)))
}

/// The payload actually applied to a spawn of the named `harness.kind`, for
/// the debug-trace `confinement` field: claude-code's argv-delivered
/// settings, or opencode's env-delivered permission block (invisible in argv
/// and never echoed via the env map itself, so this is opencode's only
/// visible confirmation that confinement applied). `None` for an unsupported
/// harness kind — see [`confinement_unsupported_capability`], which every
/// worktree-spawn call site pairs with this function to surface that case as
/// a named capability report, per PRODUCT.md's non-negotiable that
/// unsupported runtime features are explicit reports, never silent no-ops.
pub fn confinement_trace_payload<'a>(
    payloads: &'a ConfinementPayloads,
    harness_kind: &str,
) -> Option<&'a serde_json::Value> {
    match harness_kind {
        "claude-code" => Some(&payloads.claude_code),
        "opencode" => Some(&payloads.opencode),
        "codex" => Some(&payloads.codex),
        _ => None,
    }
}

/// A named, operator-visible capability report for the one case this module
/// cannot silently cover: a worktree spawn whose `harness.kind` is neither
/// `claude-code` nor `opencode` gets no harness-native write confinement
/// (P478's generator only renders those two dialects). `None` when
/// `harness_kind` is one of the two covered kinds, so a caller that already
/// knows confinement generation is active for this spawn (worktree in play,
/// `[worktree.confinement] enabled = true`) can push this unconditionally
/// through its own dedupe path — an unsupported kind never silently degrades
/// to a no-op with nothing said.
///
/// `spawn_sandbox_applied` (P480) changes what "no confinement" actually
/// means for this spawn: on macOS with the OS-level wrapper active, a
/// `pi`/`codex`-kind spawn is still covered by the generated `sandbox-exec`
/// profile — only the harness-native deny rules are absent. Reporting the
/// pre-P480 wording ("no generated write confinement at all") in that case
/// would be a silent lie in an operator-facing report, so the message is
/// conditioned on it instead.
pub fn confinement_unsupported_capability(
    harness_kind: &str,
    spawn_sandbox_applied: bool,
) -> Option<ctx_traits_core::response::CapabilityReport> {
    if matches!(harness_kind, "claude-code" | "opencode" | "codex") {
        return None;
    }
    let reason = if spawn_sandbox_applied {
        format!(
            "harness kind {harness_kind} has no P478 harness-native write-confinement renderer; this worktree spawn is still covered by the P480 OS-level sandbox (sandbox-exec), which denies writes outside the worktree/carve-outs regardless of harness kind"
        )
    } else {
        format!(
            "harness kind {harness_kind} has no P478 write-confinement renderer and this spawn has no P480 OS-level sandbox either; this worktree spawn runs with no generated write confinement at all"
        )
    };
    Some(ctx_traits_core::response::CapabilityReport::unsupported(
        "worktree.write-confinement",
        reason,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchRoot {
        path: Utf8PathBuf,
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.path.as_std_path());
        }
    }

    fn scratch_root(tag: &str) -> ScratchRoot {
        let base = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir is UTF-8");
        let path = base.join(format!(
            "ctx-confinement-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(path.as_std_path()).expect("clear stale scratch dir");
        }
        std::fs::create_dir_all(path.as_std_path()).expect("create scratch dir");
        ScratchRoot { path }
    }

    fn mkdir(path: &Utf8Path) {
        std::fs::create_dir_all(path.as_std_path()).expect("mkdir");
    }

    fn touch(path: &Utf8Path) {
        std::fs::write(path.as_std_path(), b"").expect("touch");
    }

    fn deny_strings(plan: &ConfinementPlan) -> Vec<String> {
        plan.deny
            .iter()
            .map(|entry| match entry {
                DenyEntry::Dir(path) => {
                    format!("dir:{}", path.strip_prefix(&plan.main_root).unwrap())
                }
                DenyEntry::File(path) => {
                    format!("file:{}", path.strip_prefix(&plan.main_root).unwrap())
                }
            })
            .collect()
    }

    #[test]
    fn nested_worktree_denies_the_incident_path_and_a_sibling_but_not_its_own_worktree() {
        let root = scratch_root("nested");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/worktrees/other"));
        mkdir(&main_root.join("modules/io"));
        touch(&main_root.join(".ctx/config.toml"));

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let denies = deny_strings(&plan);
        assert!(denies.contains(&"file:.ctx/config.toml".to_string()));
        assert!(denies.contains(&"dir:.ctx/worktrees/other".to_string()));
        assert!(denies.contains(&"dir:modules".to_string()));
        assert!(
            !denies
                .iter()
                .any(|entry| entry.contains(".ctx/worktrees/self"))
        );
    }

    #[test]
    fn out_of_repo_worktree_collapses_to_one_blanket_rule() {
        let repo_root = scratch_root("out-of-repo-main");
        let worktree_root = scratch_root("out-of-repo-worktree");
        mkdir(&repo_root.path.join("modules"));

        let plan = build_confinement_plan(
            &repo_root.path,
            &worktree_root.path,
            &[],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        assert_eq!(plan.deny, vec![DenyEntry::Dir(plan.main_root.clone())]);
    }

    #[test]
    fn in_repo_carve_out_is_excluded_from_denies_and_present_in_additional_directories() {
        let root = scratch_root("carve-out");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/cache/build/target"));

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[main_root.join(".ctx/cache/build/target")],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let denies = deny_strings(&plan);
        assert!(!denies.iter().any(|entry| entry.contains("cache")));
        let canonical_cache = canonicalize(&main_root.join(".ctx/cache/build/target")).unwrap();
        assert!(plan.additional_directories.contains(&canonical_cache));
    }

    #[test]
    fn existing_opencode_config_content_survives_merge_and_ours_wins_on_collision() {
        let existing = serde_json::json!({
            "permission": {
                "task": { "*": "deny" },
                "edit": { "*": "allow" },
            }
        })
        .to_string();
        let ours = serde_json::json!({
            "permission": {
                "external_directory": { "*": "deny" },
                "edit": { "*": "allow", "/main/modules/**": "deny" },
            }
        });

        let merged = merge_opencode_config_content(Some(&existing), &ours).expect("merge");
        let merged: serde_json::Value = serde_json::from_str(&merged).unwrap();

        assert_eq!(merged["permission"]["task"]["*"], "deny");
        assert_eq!(merged["permission"]["external_directory"]["*"], "deny");
        assert_eq!(
            merged["permission"]["edit"]["/main/modules/**"], "deny",
            "our edit map must win over the existing one on collision"
        );
    }

    #[test]
    fn opencode_external_directory_allows_carve_outs_but_still_denies_incident_path_and_siblings() {
        let root = scratch_root("opencode-carve-out");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/worktrees/other"));
        mkdir(&main_root.join(".ctx/cache/build-target"));
        touch(&main_root.join(".ctx/config.toml"));

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[main_root.join(".ctx/cache/build-target")],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let permission = render_opencode_permission(&plan);
        let external_directory = &permission["permission"]["external_directory"];
        assert_eq!(external_directory["*"], "deny");

        let worktree_self = canonicalize(&main_root.join(".ctx/worktrees/self")).unwrap();
        let cache = canonicalize(&main_root.join(".ctx/cache/build-target")).unwrap();
        assert_eq!(
            external_directory[format!("{worktree_self}/**")],
            "allow",
            "the run's own worktree must be an allowed external_directory carve-out"
        );
        assert_eq!(
            external_directory[format!("{cache}/**")],
            "allow",
            "an out-of-worktree carve-out (e.g. a build cache) must be an allowed external_directory carve-out too"
        );

        let incident_path = format!("{main_root}/.ctx/config.toml");
        let sibling = format!("{main_root}/.ctx/worktrees/other");
        assert!(
            external_directory.get(&incident_path).is_none(),
            "the incident path must not gain its own allow entry — it stays covered by the blanket deny"
        );
        assert!(
            external_directory.get(&sibling).is_none(),
            "a sibling worktree must not gain its own allow entry — it stays covered by the blanket deny"
        );
    }

    #[test]
    fn a_carve_out_covering_main_root_or_its_ancestor_never_becomes_an_allow_rule() {
        let root = scratch_root("carve-out-covers-main-root");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        let parent_of_main_root = main_root
            .parent()
            .expect("scratch root has a parent")
            .to_path_buf();

        // `allow = ["."]` (resolved to `main_root` itself) and an overlay
        // value that happens to resolve to `main_root`'s parent — both
        // ordinary, not hostile, config shapes.
        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[main_root.clone(), parent_of_main_root.clone()],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        assert!(
            !plan.additional_directories.contains(main_root),
            "main_root itself must never become a carve-out"
        );
        assert!(
            !plan
                .additional_directories
                .iter()
                .any(|dir| main_root.starts_with(dir)),
            "no surviving carve-out may be main_root or one of its ancestors"
        );

        let permission = render_opencode_permission(&plan);
        let external_directory = &permission["permission"]["external_directory"];
        assert_eq!(
            external_directory["*"], "deny",
            "the blanket deny must survive"
        );
        let main_root_glob = format!("{main_root}/**");
        assert!(
            external_directory.get(&main_root_glob).is_none(),
            "no allow key covering main_root may be emitted at all"
        );

        let settings = render_claude_code_settings(&plan, false);
        let additional_directories = settings["permissions"]["additionalDirectories"]
            .as_array()
            .expect("additionalDirectories is an array");
        assert!(
            additional_directories
                .iter()
                .all(|value| value.as_str() != Some(main_root.as_str())),
            "claude-code's additionalDirectories must not name main_root either"
        );
    }

    #[test]
    fn ordering_of_wildcard_and_path_keys_is_deterministic_and_wildcard_first() {
        let ours = serde_json::json!({
            "permission": { "edit": { "/z/path": "deny", "*": "allow", "/a/path": "deny" } }
        });
        let merged = merge_opencode_config_content(None, &ours).expect("merge");
        let star = merged.find("\"*\"").expect("wildcard key present");
        let a_path = merged.find("\"/a/path\"").expect("a-path key present");
        assert!(
            star < a_path,
            "wildcard key must sort before any /-anchored path key"
        );
    }

    #[test]
    fn opencode_merge_preserves_scalar_default_but_omits_unconfigured_wildcard() {
        let ours = serde_json::json!({"permission": {"edit": {"/main/**": "deny"}}});
        let absent: serde_json::Value =
            serde_json::from_str(&merge_opencode_config_content(None, &ours).expect("merge"))
                .unwrap();
        assert!(absent["permission"]["edit"].get("*").is_none());

        let existing =
            serde_json::json!({"permission": {"edit": "ask", "task": "deny"}}).to_string();
        let merged: serde_json::Value = serde_json::from_str(
            &merge_opencode_config_content(Some(&existing), &ours).expect("merge"),
        )
        .unwrap();
        assert_eq!(merged["permission"]["edit"]["*"], "ask");
        assert_eq!(merged["permission"]["edit"]["/main/**"], "deny");
        assert_eq!(merged["permission"]["task"], "deny");
    }

    #[test]
    fn codex_payload_reuses_carve_outs_and_selects_one_sandbox() {
        let plan = ConfinementPlan {
            main_root: Utf8PathBuf::from("/main"),
            deny: Vec::new(),
            additional_directories: vec![
                Utf8PathBuf::from("/worktree"),
                Utf8PathBuf::from("/cache"),
            ],
            sandbox: true,
        };
        assert_eq!(
            render_codex_payload(&plan, false)["sandbox"],
            "workspace-write"
        );
        assert_eq!(
            render_codex_payload(&plan, true)["sandbox"],
            "danger-full-access"
        );
        assert_eq!(
            render_codex_payload(&plan, false)["add-directory"],
            serde_json::json!(["/worktree", "/cache"])
        );
        let payloads = ConfinementPayloads {
            claude_code: serde_json::json!({}),
            opencode: serde_json::json!({}),
            codex: render_codex_payload(&plan, false),
            spawn_sandbox: None,
            sandbox_requested: true,
        };
        assert!(confinement_trace_payload(&payloads, "codex").is_some());
        assert!(confinement_unsupported_capability("codex", false).is_none());
    }

    #[test]
    fn invalid_existing_opencode_config_content_is_a_hard_error_naming_the_key() {
        let ours = serde_json::json!({"permission": {}});
        let error = merge_opencode_config_content(Some("not json"), &ours).unwrap_err();
        assert!(format!("{error}").contains(OPENCODE_CONFIG_CONTENT_KEY));
    }

    #[test]
    fn disabled_confinement_yields_no_payload() {
        let root = scratch_root("disabled");
        mkdir(&root.path.join(".ctx/worktrees/self"));
        let mut worktree = crate::harness_config::WorktreeConfig::default();
        worktree.confinement.enabled = false;

        let (env, payloads) =
            resolve_worktree_spawn(&worktree, Some(&root.path.join(".ctx/worktrees/self")))
                .expect("resolve");

        assert!(env.is_empty());
        assert!(payloads.is_none());
    }

    #[test]
    fn no_execution_dir_yields_no_payload() {
        let worktree = crate::harness_config::WorktreeConfig::default();
        let (env, payloads) = resolve_worktree_spawn(&worktree, None).expect("resolve");
        assert!(env.is_empty());
        assert!(payloads.is_none());
    }

    #[test]
    fn sandbox_off_yields_no_spawn_sandbox_regardless_of_platform() {
        let root = scratch_root("sandbox-off");
        mkdir(&root.path.join(".ctx/worktrees/self"));
        let plan = build_confinement_plan(
            &root.path,
            &root.path.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig {
                sandbox: false,
                ..Default::default()
            },
        )
        .expect("build plan");
        assert!(!plan.sandbox);

        let sandbox = generate_spawn_sandbox(&plan, &BTreeMap::new()).expect("generate");
        assert!(sandbox.is_none());
    }

    /// P480 blocker `nested-seatbelt-double-apply`: macOS refuses a second
    /// `sandbox_apply` inside an already-sandboxed process, so at most one
    /// of {claude-code's own `sandbox.enabled`, the P480 spawner wrapper}
    /// may be active for the same spawn. When the spawner wrapper actually
    /// applies, claude-code's own layer must render disabled even though
    /// the plan itself requested OS enforcement (`plan.sandbox == true`) —
    /// the request was honored by the outer layer, not silently dropped.
    #[test]
    fn claude_code_settings_disable_own_sandbox_when_spawn_wrapper_applies() {
        let root = scratch_root("nested-sandbox-applied");
        mkdir(&root.path.join(".ctx/worktrees/self"));
        let plan = build_confinement_plan(
            &root.path,
            &root.path.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");
        assert!(plan.sandbox, "default plan requests OS enforcement");

        let settings = render_claude_code_settings(&plan, true);
        assert_eq!(
            settings["sandbox"]["enabled"],
            serde_json::json!(false),
            "claude-code's own sandbox must be disabled once the P480 wrapper already applies, \
             to avoid nesting one Seatbelt sandbox inside another"
        );
    }

    /// The pre-P480 behavior — `sandbox.enabled` following `plan.sandbox`
    /// unchanged — must hold exactly wherever the spawner wrapper does NOT
    /// apply to this spawn (non-macOS, `sandbox-exec` missing, or
    /// `sandbox = false`), so those hosts lose nothing.
    #[test]
    fn claude_code_settings_keep_own_sandbox_when_wrapper_does_not_apply() {
        let root = scratch_root("nested-sandbox-not-applied");
        mkdir(&root.path.join(".ctx/worktrees/self"));

        let plan_on = build_confinement_plan(
            &root.path,
            &root.path.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");
        assert_eq!(
            render_claude_code_settings(&plan_on, false)["sandbox"]["enabled"],
            serde_json::json!(true),
            "sandbox=true with no wrapper applied must render exactly the pre-P480 value"
        );

        let plan_off = build_confinement_plan(
            &root.path,
            &root.path.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig {
                sandbox: false,
                ..Default::default()
            },
        )
        .expect("build plan");
        assert_eq!(
            render_claude_code_settings(&plan_off, false)["sandbox"]["enabled"],
            serde_json::json!(false),
            "sandbox=false must render exactly the pre-P480 value regardless of the wrapper"
        );
    }

    /// End-to-end through the same entry point every caller uses
    /// (`resolve_worktree_spawn`'s two halves: `generate_spawn_sandbox` then
    /// `render_claude_code_settings`), on a host where the P480 wrapper is
    /// actually available: the wrapper applies AND the claude-code payload
    /// it is paired with never doubles up the OS layer.
    #[cfg(target_os = "macos")]
    #[test]
    fn spawn_sandbox_and_claude_code_settings_never_both_enable_seatbelt_when_available() {
        if !sandbox_exec_available() {
            eprintln!("skipping: /usr/bin/sandbox-exec not present on this host");
            return;
        }
        let root = scratch_root("nested-sandbox-e2e-macos");
        mkdir(&root.path.join(".ctx/worktrees/self"));
        let plan = build_confinement_plan(
            &root.path,
            &root.path.join(".ctx/worktrees/self"),
            &[],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), root.path.join("home").to_string());
        let spawn_sandbox = generate_spawn_sandbox(&plan, &env).expect("generate");
        assert!(
            spawn_sandbox.is_some(),
            "sandbox-exec is present, so the P480 wrapper must be generated"
        );

        let settings = render_claude_code_settings(&plan, spawn_sandbox.is_some());
        assert_eq!(
            settings["sandbox"]["enabled"],
            serde_json::json!(false),
            "claude-code's own layer must be off wherever the P480 wrapper is the active layer"
        );
    }

    #[test]
    fn unknown_harness_kind_yields_no_trace_payload() {
        let payloads = ConfinementPayloads {
            claude_code: serde_json::json!({}),
            opencode: serde_json::json!({}),
            codex: serde_json::json!({}),
            spawn_sandbox: None,
            sandbox_requested: true,
        };
        assert!(confinement_trace_payload(&payloads, "some-future-harness").is_none());
        assert!(confinement_trace_payload(&payloads, "claude-code").is_some());
        assert!(confinement_trace_payload(&payloads, "opencode").is_some());
    }

    #[test]
    fn unknown_harness_kind_reports_an_unsupported_capability() {
        assert!(confinement_unsupported_capability("claude-code", false).is_none());
        assert!(confinement_unsupported_capability("opencode", true).is_none());
        let capability =
            confinement_unsupported_capability("pi", false).expect("unsupported kind reports");
        assert!(!capability.supported);
        assert_eq!(capability.capability, "worktree.write-confinement");
        assert!(
            capability
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("pi")
        );
        assert!(
            capability
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("no generated write confinement at all"),
            "without an applied spawn sandbox the message must say so plainly"
        );

        let covered = confinement_unsupported_capability("pi", true)
            .expect("unsupported kind still reports, even when the OS layer covers it");
        assert!(
            covered
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("P480 OS-level sandbox"),
            "with an applied spawn sandbox the message must name it instead of claiming zero confinement"
        );
    }

    #[test]
    fn spawn_sandbox_unsupported_capability_only_reports_when_requested_and_unavailable() {
        let sandbox = SpawnSandbox {
            argv_prefix: vec!["/usr/bin/sandbox-exec".to_string()],
            profile: "(version 1)".to_string(),
        };
        assert!(spawn_sandbox_unsupported_capability(false, None).is_none());
        assert!(spawn_sandbox_unsupported_capability(false, Some(&sandbox)).is_none());
        assert!(spawn_sandbox_unsupported_capability(true, Some(&sandbox)).is_none());
        assert!(spawn_sandbox_unsupported_capability(true, None).is_some());
    }

    #[test]
    fn expand_home_resolves_against_effective_spawn_env_home() {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/effective/hearth".to_string());
        assert_eq!(expand_home("~/.cargo", &env), "/effective/hearth/.cargo");
        assert_eq!(expand_home("~", &env), "/effective/hearth");
        assert_eq!(expand_home("/already/absolute", &env), "/already/absolute");
        assert_eq!(expand_home("relative/path", &env), "relative/path");
        assert_eq!(expand_home("~otheruser/x", &env), "~otheruser/x");
    }

    #[test]
    fn macos_sandbox_profile_orders_carve_outs_before_git_allow_before_hooks_reveny() {
        let root = scratch_root("sandbox-profile-shape");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/cache/build-target"));
        mkdir(&main_root.join(".git"));

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[main_root.join(".ctx/cache/build-target")],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), root.path.join("home").to_string());
        let sandbox = render_macos_sandbox_profile(&plan, &env).expect("render profile");

        assert!(sandbox.profile.contains("(deny file-write*)\n"));
        let worktree_self = canonicalize(&main_root.join(".ctx/worktrees/self")).unwrap();
        let cache = canonicalize(&main_root.join(".ctx/cache/build-target")).unwrap();
        let git_dir = canonicalize(main_root).unwrap().join(".git");

        let worktree_idx = sandbox
            .profile
            .find(&format!("(subpath \"{worktree_self}\")"))
            .expect("worktree carve-out present");
        let cache_idx = sandbox
            .profile
            .find(&format!("(subpath \"{cache}\")"))
            .expect("cache carve-out present");
        let git_allow_idx = sandbox
            .profile
            .find(&format!("(allow file-write* (subpath \"{git_dir}\"))"))
            .expect("git allow present");
        let hooks_deny_idx = sandbox
            .profile
            .find(&format!(
                "(deny file-write* (subpath \"{}\")",
                git_dir.join("hooks")
            ))
            .expect("hooks re-deny present");

        assert!(worktree_idx < git_allow_idx);
        assert!(cache_idx < git_allow_idx);
        assert!(
            git_allow_idx < hooks_deny_idx,
            "hooks/config re-deny must come after the .git allow"
        );
        assert!(
            sandbox.argv_prefix
                == vec![
                    "/usr/bin/sandbox-exec".to_string(),
                    "-p".to_string(),
                    sandbox.profile.clone(),
                ]
        );
    }

    #[test]
    fn macos_sandbox_profile_realpaths_a_symlinked_carve_out() {
        let root = scratch_root("sandbox-profile-symlink");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/cache/real-build-target"));
        let symlinked = main_root.join(".ctx/cache/build-target-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            main_root.join(".ctx/cache/real-build-target").as_std_path(),
            symlinked.as_std_path(),
        )
        .expect("create symlink");
        #[cfg(not(unix))]
        mkdir(&symlinked);

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            std::slice::from_ref(&symlinked),
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), root.path.join("home").to_string());
        let sandbox = render_macos_sandbox_profile(&plan, &env).expect("render profile");

        let resolved = canonicalize(&main_root.join(".ctx/cache/real-build-target")).unwrap();
        assert!(
            sandbox
                .profile
                .contains(&format!("(subpath \"{resolved}\")")),
            "a symlinked carve-out must render as its resolved target, not the symlink path"
        );
        assert!(
            !sandbox.profile.contains("build-target-link"),
            "the symlink path itself must never appear in the rendered profile"
        );
    }

    /// A scratch root deliberately OUTSIDE `std::env::temp_dir()`: the
    /// rendered profile allows `TMPDIR`/`/private/tmp`/`/private/var/folders`
    /// wholesale (§4.3 of the P480 draft), so a `main_root` placed under the
    /// ordinary temp dir would be allowed for that reason alone and this
    /// test's "denied outside the worktree" assertion would be vacuous.
    /// Rooted under this crate's own `target/` (self-contained, writable,
    /// git-ignored) so nothing outside the test's own lifetime is touched.
    fn non_tmp_scratch_root(tag: &str) -> ScratchRoot {
        let path = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "ctx-confinement-live-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        if path.exists() {
            std::fs::remove_dir_all(path.as_std_path()).expect("clear stale scratch dir");
        }
        std::fs::create_dir_all(path.as_std_path()).expect("create scratch dir");
        ScratchRoot { path }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_seatbelt_enforces_write_boundaries_live() {
        if !sandbox_exec_available() {
            eprintln!("skipping: /usr/bin/sandbox-exec not present on this host");
            return;
        }
        // A nested Seatbelt sandbox refuses a second `sandbox_apply`
        // regardless of the outer profile's permissiveness (the exact
        // nesting failure P480 blocker `nested-seatbelt-double-apply`
        // named) — if the gate itself is already running inside a sandbox
        // (e.g. this very test suite invoked under a P480-wrapped worktree
        // spawn), every assertion below would fail for that reason, not
        // because the boundary logic is wrong. Probe with a trivial nested
        // `sandbox-exec` first and skip rather than report a false failure.
        let nested_probe = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)"])
            .arg("/usr/bin/true")
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !nested_probe {
            eprintln!(
                "skipping: sandbox-exec cannot nest inside this process's own sandbox \
                 (already running under an enclosing Seatbelt sandbox)"
            );
            return;
        }
        let root = non_tmp_scratch_root("sandbox-live-seatbelt");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join("modules"));
        mkdir(&main_root.join(".ctx/cache/build-target"));
        std::fs::write(main_root.join("modules/incident.txt").as_std_path(), b"").unwrap();

        let worktree_root = main_root.join(".ctx/worktrees/self");
        let plan = build_confinement_plan(
            main_root,
            &worktree_root,
            &[main_root.join(".ctx/cache/build-target")],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan");

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), root.path.join("home").to_string());
        let sandbox = render_macos_sandbox_profile(&plan, &env).expect("render profile");

        let run = |target: &Utf8Path| -> bool {
            let mut command = std::process::Command::new(&sandbox.argv_prefix[0]);
            command.args(&sandbox.argv_prefix[1..]);
            command.arg("/bin/sh");
            command.arg("-c");
            command.arg(format!("echo x > '{target}'"));
            command
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        };

        assert!(
            run(&worktree_root.join("inside.txt")),
            "a write inside the worktree must succeed"
        );
        assert!(
            !run(&main_root.join("modules/incident.txt")),
            "a write outside the worktree, inside the main repo, must be denied"
        );
        let cache = canonicalize(&main_root.join(".ctx/cache/build-target")).unwrap();
        assert!(
            run(&cache.join("out.txt")),
            "a write into a declared carve-out must succeed"
        );
    }

    #[test]
    fn a_not_yet_created_out_of_repo_carve_out_is_tolerated_and_still_excluded_from_deny() {
        let main_root = scratch_root("absent-carve-out-main");
        mkdir(&main_root.path.join(".ctx/worktrees/self"));

        // Never created — simulates a build cache under a repo-key directory
        // that cargo only materializes on its first build.
        let not_yet_created = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-confinement-test-absent-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ))
            .join("build-target");
        assert!(!not_yet_created.exists(), "fixture path must not exist yet");

        let plan = build_confinement_plan(
            &main_root.path,
            &main_root.path.join(".ctx/worktrees/self"),
            std::slice::from_ref(&not_yet_created),
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan tolerates an absent carve-out");

        assert!(
            plan.additional_directories
                .iter()
                .any(|dir| dir.ends_with("build-target")),
            "the absent carve-out must still land in additional_directories"
        );
        assert!(
            plan.deny.is_empty(),
            "a carve-out outside main_root needs no deny enumeration at all"
        );
    }

    #[test]
    fn a_not_yet_created_in_repo_carve_out_is_tolerated_and_excluded_from_deny() {
        let root = scratch_root("absent-in-repo-carve-out");
        let main_root = &root.path;
        mkdir(&main_root.join(".ctx/worktrees/self"));
        mkdir(&main_root.join(".ctx/cache"));
        // `.ctx/cache/build-target` itself is never created.

        let plan = build_confinement_plan(
            main_root,
            &main_root.join(".ctx/worktrees/self"),
            &[main_root.join(".ctx/cache/build-target")],
            &WorktreeConfinementConfig::default(),
        )
        .expect("build plan tolerates an absent in-repo carve-out");

        let denies = deny_strings(&plan);
        assert!(
            !denies.iter().any(|entry| entry.contains("build-target")),
            "the not-yet-created carve-out itself must never be denied"
        );
        assert!(
            plan.additional_directories
                .iter()
                .any(|dir| dir.ends_with(".ctx/cache/build-target")),
            "the absent carve-out must still land in additional_directories"
        );
    }
}
