//! Host-placement lifecycle (P441): built-in host registry, placement
//! manifest, and the install/update/remove/archive transactions built on the
//! shared [`crate::export`] managed-write/removal primitives.
//!
//! Rendering stays a CLI/core concern: this module never renders a trait. It
//! receives already-rendered content plus digest evidence from the caller
//! and owns only the placement-specific mechanics — registry resolution,
//! manifest persistence, transactional writes, and archive/removal.

use crate::file_lock::{lock_exclusive_blocking, open_lock_file_no_follow};
use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::export::{Format, Identity};
use ctx_traits_core::render::ExtendedRenderProfile;
use ctx_traits_core::r#trait::Id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Built-in host registry
// ---------------------------------------------------------------------------

/// A placement fact sheet for one host: which existing render profile and
/// export format to reuse, and where the resulting artifact goes for
/// project vs. global scope. Templates use the single `{trait}` placeholder,
/// substituted with the already-validated trait ID. `global_template` is
/// `None` when the host has no portable, filesystem-discoverable global
/// location (e.g. Settings-UI-managed configuration) — `--global` then
/// refuses explicitly instead of writing a file the host will never read.
#[derive(Debug, Clone)]
pub struct HostSpec {
    pub id: String,
    pub profile: ExtendedRenderProfile,
    pub format: Format,
    /// `None` when the host has no project-scoped discovery location at
    /// all (verified against the host's real discovery contract, not
    /// guessed) — `--host <id>` without `--global` then refuses explicitly
    /// via [`Error::ProjectUnsupported`] instead of writing a path the host
    /// will never read.
    pub project_template: Option<String>,
    pub global_template: Option<String>,
}

/// Deterministic `.mdc` frontmatter Cursor requires to discover a Project
/// Rule at all: an `.mdc` file with no frontmatter is silently ignored by
/// Cursor's rule loader. `alwaysApply: true` is a fixed constant (not
/// derived from trait content, matching the `Copilot` frontmatter's
/// injection-surface reasoning): the placed rule always applies rather than
/// requiring Cursor's (undeclared, per-project) glob/agent-requested
/// matching. Applied by the host-placement lifecycle itself (see
/// [`builtin_content_frontmatter`]), not by `ctx_traits_core::render`,
/// because it is Cursor's placement discovery contract, not new canonical
/// render vocabulary.
pub const CURSOR_RULE_FRONTMATTER: &str = "---\ndescription:\nglobs:\nalwaysApply: true\n---";

/// The fixed, host-keyed frontmatter block a built-in host's placement
/// content must open with to be discovered at all, prepended by the CLI
/// orchestration layer after rendering and before writing/archiving.
/// Distinct from `ExtendedRenderProfile`'s own frontmatter (`AgentSkills`,
/// `Copilot`): this is placement-discovery shape for a specific host ID,
/// not canonical render vocabulary, so it lives here instead of in
/// `ctx_traits_core::render`.
pub fn builtin_content_frontmatter(host: &str) -> Option<&'static str> {
    match host {
        "cursor" => Some(CURSOR_RULE_FRONTMATTER),
        _ => None,
    }
}

/// Built-in hosts covering the contract's wider tier, grounded in each
/// host's real discovery contract (not a guessed path plus a generic
/// Markdown render):
///
/// - `cursor`: Project Rules are discovered under `.cursor/rules/*.mdc`,
///   and only if the file opens with the frontmatter block
///   [`CURSOR_RULE_FRONTMATTER`] (`alwaysApply: true`, always required —
///   see `builtin_content_frontmatter`). Cursor has no portable global
///   rules file — user rules live in Settings-UI-managed state, so global
///   scope is unsupported.
/// - `copilot`: path-specific custom instructions are discovered under
///   `.github/instructions/*.instructions.md`, and are silently ignored
///   unless the file opens with `applyTo` frontmatter (`Copilot` profile
///   frontmatter, see `render_frontmatter`). VS Code has no portable global
///   filesystem location for these, so global scope is unsupported.
/// - `gemini`: skills are discovered under `.gemini/skills/<name>/SKILL.md`
///   for a project and `~/.gemini/skills/<name>/SKILL.md` for the user —
///   the same `SKILL.md` shape the existing `AgentSkills`/`Skill` profile
///   already renders.
/// - `cline`: project rules are discovered under `.clinerules/*.md`. Cline
///   has no single documented cross-platform global rules path, so global
///   scope is unsupported.
/// - `kiro`: steering documents are discovered under `.kiro/steering/*.md`
///   for a project and `~/.kiro/steering/*.md` for the user — both are
///   plain Markdown, so global scope is supported the same way as project
///   scope.
/// - `claude-code`, `opencode`, `pi`: Agent Skills-shaped hosts discovered
///   under `.claude/skills`, `.opencode/skills`, `.pi/skills` respectively
///   for a project; `claude-code` also has a documented user-level
///   `~/.claude/skills`. `opencode` and `pi` have no verified portable
///   global location, so global scope is unsupported for them (the
///   `[host.<name>] global-path` config override remains the escape).
/// - `codex`: skills are discovered under `$CODEX_HOME/skills` only — the
///   0.145 binary has no project-local `.agents/skills` (or any other
///   project-local) discovery chain, per
///   `.docs/research/HARNESS_UTILIZATION_2026-07.md:94-96` — so project
///   scope is unsupported and only global (`~/.codex/skills`) is wired.
///
/// Default format for the four skill-shaped rows above is [`Format::Stub`]:
/// a stub converges on fresh, trust-gated bytes via `ctx traits internal prompt`
/// rather than freezing a copy, and `--format skill` opts into the fully
/// rendered directory.
///
/// None of these hosts get a dedicated render profile of their own (P441
/// scope boundary) — each maps onto an existing profile/format pairing that
/// emits content the host actually accepts.
pub fn builtin_hosts() -> Vec<HostSpec> {
    vec![
        HostSpec {
            id: "cursor".to_string(),
            profile: ExtendedRenderProfile::MarkdownOnly,
            format: Format::Compat,
            project_template: Some(".cursor/rules/{trait}.mdc".to_string()),
            global_template: None,
        },
        HostSpec {
            id: "copilot".to_string(),
            profile: ExtendedRenderProfile::Copilot,
            format: Format::Compat,
            project_template: Some(".github/instructions/{trait}.instructions.md".to_string()),
            global_template: None,
        },
        HostSpec {
            id: "gemini".to_string(),
            profile: ExtendedRenderProfile::AgentSkills,
            format: Format::Skill,
            project_template: Some(".gemini/skills/{trait}/SKILL.md".to_string()),
            global_template: Some(".gemini/skills/{trait}/SKILL.md".to_string()),
        },
        HostSpec {
            id: "cline".to_string(),
            profile: ExtendedRenderProfile::MarkdownOnly,
            format: Format::Compat,
            project_template: Some(".clinerules/{trait}.md".to_string()),
            global_template: None,
        },
        HostSpec {
            id: "kiro".to_string(),
            profile: ExtendedRenderProfile::MarkdownOnly,
            format: Format::Compat,
            project_template: Some(".kiro/steering/{trait}.md".to_string()),
            global_template: Some(".kiro/steering/{trait}.md".to_string()),
        },
        HostSpec {
            id: "claude-code".to_string(),
            profile: ExtendedRenderProfile::ClaudeCode,
            format: Format::Stub,
            project_template: Some(".claude/skills/{trait}/SKILL.md".to_string()),
            global_template: Some(".claude/skills/{trait}/SKILL.md".to_string()),
        },
        HostSpec {
            id: "opencode".to_string(),
            profile: ExtendedRenderProfile::Opencode,
            format: Format::Stub,
            project_template: Some(".opencode/skills/{trait}/SKILL.md".to_string()),
            global_template: None,
        },
        HostSpec {
            id: "codex".to_string(),
            profile: ExtendedRenderProfile::Codex,
            format: Format::Stub,
            project_template: None,
            global_template: Some(".codex/skills/{trait}/SKILL.md".to_string()),
        },
        HostSpec {
            id: "pi".to_string(),
            profile: ExtendedRenderProfile::Pi,
            format: Format::Stub,
            project_template: Some(".pi/skills/{trait}/SKILL.md".to_string()),
            global_template: None,
        },
    ]
}

fn builtin_host(id: &str) -> Option<HostSpec> {
    builtin_hosts().into_iter().find(|spec| spec.id == id)
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "unknown host {host:?}: expected one of the built-in hosts ({available}) or a fully-specified [host.{host}] config override (profile, format, project-path, global-path)"
    )]
    UnknownHost { host: String, available: String },
    #[error("host {host:?} config override has an invalid profile: {profile:?}")]
    InvalidProfile { host: String, profile: String },
    #[error("host {host:?} config override has an invalid format: {format:?}")]
    InvalidFormat { host: String, format: String },
    #[error("host {host:?} placement template resolved to an unsafe path: {source}")]
    UnsafeTemplate {
        host: String,
        #[source]
        source: crate::export::Error,
    },
    #[error("host {host:?} placement template references an unknown placeholder: {template:?}")]
    UnknownPlaceholder { host: String, template: String },
    #[error("no {scope} placement recorded for host {host:?} and trait {trait_id:?}")]
    PlacementNotFound {
        scope: &'static str,
        host: String,
        trait_id: String,
    },
    #[error(
        "a {scope} placement for host {host:?} and trait {trait_id:?} already sources from {existing:?}; refusing to silently adopt {requested:?} as a replacement"
    )]
    SourceMismatch {
        scope: &'static str,
        host: String,
        trait_id: String,
        existing: String,
        requested: String,
    },
    #[error(
        "a {scope} placement for host {host:?} and trait {trait_id:?} already targets {existing_path:?}; the resolved target is now {new_path:?} — run `ctx traits internal host-remove` for the existing placement first, then reinstall"
    )]
    TargetChanged {
        scope: &'static str,
        host: String,
        trait_id: String,
        existing_path: String,
        new_path: String,
    },
    #[error(
        "host {host:?} has no portable global filesystem location; add a `global-path` under [host.{host}] in .ctx/traits/runtime.toml or the global config to override"
    )]
    GlobalUnsupported { host: String },
    #[error(
        "host {host:?} has no project-scoped discovery location; add a `project-path` under [host.{host}] in .ctx/traits/runtime.toml to override, or pass --global"
    )]
    ProjectUnsupported { host: String },
    #[error(
        "{scope} placement record for host {host:?} and trait {trait_id:?} has {paths} path(s) but {content_digests} content digest(s); refusing to remove without a matching per-path digest"
    )]
    RecordCardinalityMismatch {
        scope: &'static str,
        host: String,
        trait_id: String,
        paths: usize,
        content_digests: usize,
    },
    #[error("host placement write failed: {0}")]
    Write(#[from] crate::export::Error),
    #[error("host placement archive failed: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error(
        "{context} failed ({source}), and rolling the pre-command state back afterward also failed: {rollback_source}; the placement manifest and/or artifact bytes may not match either state — inspect before retrying"
    )]
    RollbackFailed {
        context: &'static str,
        source: Box<Error>,
        rollback_source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] Box<crate::Error>),
}

impl From<crate::Error> for Error {
    fn from(source: crate::Error) -> Self {
        Self::Io(Box::new(source))
    }
}

fn available_builtin_ids() -> String {
    builtin_hosts()
        .iter()
        .map(|spec| spec.id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the effective [`HostSpec`] for `host`, applying any
/// `[host.<name>]` config override field-by-field on top of a built-in
/// entry, or building a spec entirely from a fully-specified override for a
/// host not in the built-in table. A fully-specified override must supply
/// `project-path`; `global-path` is optional (omitting it keeps the host's
/// global scope unsupported, matching a built-in with no portable global
/// location).
pub fn resolve_host_spec(
    host: &str,
    overrides: &BTreeMap<String, crate::harness_config::HostOverride>,
) -> Result<HostSpec, Error> {
    let override_ = overrides.get(host);
    let Some(builtin) = builtin_host(host) else {
        let over = override_.ok_or_else(|| Error::UnknownHost {
            host: host.to_string(),
            available: available_builtin_ids(),
        })?;
        let (Some(profile), Some(format), Some(project_path)) = (
            over.profile.as_deref(),
            over.format.as_deref(),
            over.project_path.as_deref(),
        ) else {
            return Err(Error::UnknownHost {
                host: host.to_string(),
                available: available_builtin_ids(),
            });
        };
        return Ok(HostSpec {
            id: host.to_string(),
            profile: parse_profile(host, profile)?,
            format: parse_format(host, format)?,
            project_template: Some(project_path.to_string()),
            global_template: over.global_path.clone(),
        });
    };
    let mut spec = builtin;
    if let Some(over) = override_ {
        if let Some(profile) = over.profile.as_deref() {
            spec.profile = parse_profile(host, profile)?;
        }
        if let Some(format) = over.format.as_deref() {
            spec.format = parse_format(host, format)?;
        }
        if let Some(project_path) = over.project_path.as_deref() {
            spec.project_template = Some(project_path.to_string());
        }
        if let Some(global_path) = over.global_path.as_deref() {
            spec.global_template = Some(global_path.to_string());
        }
    }
    Ok(spec)
}

fn parse_profile(host: &str, value: &str) -> Result<ExtendedRenderProfile, Error> {
    ExtendedRenderProfile::parse(value).ok_or_else(|| Error::InvalidProfile {
        host: host.to_string(),
        profile: value.to_string(),
    })
}

fn parse_format(host: &str, value: &str) -> Result<Format, Error> {
    Format::parse(value).ok_or_else(|| Error::InvalidFormat {
        host: host.to_string(),
        format: value.to_string(),
    })
}

/// Select `spec`'s project or global placement template, refusing explicitly
/// when `global` is requested but the host has no portable global location.
pub fn target_template(spec: &HostSpec, global: bool) -> Result<&str, Error> {
    if global {
        spec.global_template
            .as_deref()
            .ok_or_else(|| Error::GlobalUnsupported {
                host: spec.id.clone(),
            })
    } else {
        spec.project_template
            .as_deref()
            .ok_or_else(|| Error::ProjectUnsupported {
                host: spec.id.clone(),
            })
    }
}

/// Substitute `{trait}` in `template` with `trait_id` and validate the
/// result as a safe relative placement path (no absolute paths, `..`
/// traversal, backslashes, or empty segments — the same shape rules
/// [`crate::export`]'s managed write enforces), and reject any remaining
/// `{`/`}` as an unknown placeholder.
pub fn resolve_template(host: &str, template: &str, trait_id: &str) -> Result<Utf8PathBuf, Error> {
    let resolved = template.replace("{trait}", trait_id);
    if resolved.contains('{') || resolved.contains('}') {
        return Err(Error::UnknownPlaceholder {
            host: host.to_string(),
            template: template.to_string(),
        });
    }
    let path = Utf8PathBuf::from(resolved);
    crate::export::validate_relative_path(&path).map_err(|source| Error::UnsafeTemplate {
        host: host.to_string(),
        source,
    })?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Placement manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Project,
    Global,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }
}

/// One recorded host placement: everything `host-update`/`host-remove` need
/// to reconstruct or safely remove the artifact without re-resolving the
/// host or trait from scratch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PlacementRecord {
    pub scope: Scope,
    pub host: String,
    pub trait_id: String,
    /// Stable source locator (the trait file argument used to load the
    /// package that produced this placement), so `host-update` reloads the
    /// same package rather than a newly shadowing same-ID package.
    pub source: String,
    pub profile: String,
    pub format: String,
    /// Placed artifact paths, relative to the scope root (project repo root
    /// or the host's global root).
    pub paths: Vec<String>,
    pub source_digest: String,
    pub canonical_digest: String,
    /// Content digest per entry in `paths`, same order.
    pub content_digests: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlacementManifest {
    #[serde(default)]
    pub placements: Vec<PlacementRecord>,
}

impl PlacementManifest {
    fn find(&self, scope: Scope, host: &str, trait_id: &str) -> Option<usize> {
        self.placements.iter().position(|record| {
            record.scope == scope && record.host == host && record.trait_id == trait_id
        })
    }
}

fn load_manifest(path: &Utf8Path) -> Result<PlacementManifest, Error> {
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => {
            let manifest: PlacementManifest = toml::from_str(&text)
                .map_err(|source| crate::parse::Error::TomlDecode {
                    context: path.to_string(),
                    source,
                })
                .map_err(crate::Error::from)?;
            Ok(manifest)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PlacementManifest::default())
        }
        Err(source) => {
            let error: crate::Error = crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
            .into();
            Err(Error::from(error))
        }
    }
}

fn save_manifest(path: &Utf8Path, manifest: &PlacementManifest) -> Result<(), Error> {
    let text = toml::to_string_pretty(manifest)
        .map_err(|source| crate::parse::Error::TomlEncode {
            context: path.to_string(),
            source,
        })
        .map_err(crate::Error::from)?;
    if let Some(parent) = path.parent() {
        crate::path_safety::create_dir_all_no_symlinks(parent, "host-placement manifest root")
            .map_err(Error::from)?;
    }
    crate::write::write_bytes_atomically(path, text.as_bytes()).map_err(Error::from)
}

/// Read placements for `scope` without taking the manifest lock: used by
/// `host-update`/`host-remove` to discover what to act on before starting a
/// locked transaction. Never authoritative for a mutation decision by
/// itself — [`apply_update`] and [`remove`] both reload the manifest again
/// under the lock before mutating.
pub fn list_placements(manifest_path: &Utf8Path) -> Result<Vec<PlacementRecord>, Error> {
    Ok(load_manifest(manifest_path)?.placements)
}

/// Per-path drift/ownership state, as reported by `host status`. A plain
/// projection of [`crate::export::ManagedState`] that drops the marker
/// payload (not meaningful to a CLI reporter) and is `pub` so the CLI edge
/// can consume it without reaching into a `pub(crate)` IO type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    Ok,
    Missing,
    LeafSymlink,
    LeafNotRegularFile,
    UnmanagedTarget,
    OwnershipMismatch,
    LocallyModified,
}

/// Read-only per-path drift state for one recorded placement, without
/// taking the manifest lock or mutating anything: `host status` calls this
/// per record before deciding whether a fresh render is even worth
/// comparing.
pub fn inspect_placement_paths(
    root: &Utf8Path,
    record: &PlacementRecord,
) -> Result<Vec<PathState>, Error> {
    let profile =
        ExtendedRenderProfile::parse(&record.profile).ok_or_else(|| Error::InvalidProfile {
            host: record.host.clone(),
            profile: record.profile.clone(),
        })?;
    let format = Format::parse(&record.format).ok_or_else(|| Error::InvalidFormat {
        host: record.host.clone(),
        format: record.format.clone(),
    })?;
    let source_digest = Digest::parse(&record.source_digest)
        .map_err(crate::Error::from)
        .map_err(Error::from)?;
    let identity = Identity::new(
        Id::new(record.trait_id.as_str())
            .map_err(|source| Error::from(crate::Error::from(source)))?,
        source_digest,
        format.ownership(profile),
    );
    if record.paths.len() != record.content_digests.len() {
        return Err(Error::RecordCardinalityMismatch {
            scope: record.scope.as_str(),
            host: record.host.clone(),
            trait_id: record.trait_id.clone(),
            paths: record.paths.len(),
            content_digests: record.content_digests.len(),
        });
    }
    let mut states = Vec::with_capacity(record.paths.len());
    for (index, (path, digest)) in record
        .paths
        .iter()
        .zip(record.content_digests.iter())
        .enumerate()
    {
        let expected = Digest::parse(digest)
            .map_err(crate::Error::from)
            .map_err(Error::from)?;
        let is_leaf = index == 0;
        let (_target, state) = crate::export::inspect_managed(
            root,
            Utf8Path::new(path),
            is_leaf.then_some(&identity),
            &expected,
        )?;
        states.push(match state {
            crate::export::ManagedState::Ok(_) => PathState::Ok,
            crate::export::ManagedState::Missing => PathState::Missing,
            crate::export::ManagedState::LeafSymlink => PathState::LeafSymlink,
            crate::export::ManagedState::LeafNotRegularFile => PathState::LeafNotRegularFile,
            crate::export::ManagedState::UnmanagedTarget => PathState::UnmanagedTarget,
            crate::export::ManagedState::OwnershipMismatch(_) => PathState::OwnershipMismatch,
            crate::export::ManagedState::LocallyModified => PathState::LocallyModified,
        });
    }
    Ok(states)
}

struct ManifestLock {
    _file: std::fs::File,
}

fn acquire_manifest_lock(manifest_path: &Utf8Path) -> Result<ManifestLock, Error> {
    let lock_path = manifest_path.with_extension("toml.lock");
    if let Some(parent) = lock_path.parent() {
        crate::path_safety::create_dir_all_no_symlinks(parent, "host-placement manifest root")
            .map_err(Error::from)?;
    }
    let file = open_lock_file_no_follow(&lock_path).map_err(|source| {
        Error::from(crate::Error::from(crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }))
    })?;
    lock_exclusive_blocking(&file).map_err(|source| {
        Error::from(crate::Error::from(crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }))
    })?;
    Ok(ManifestLock { _file: file })
}

// ---------------------------------------------------------------------------
// Lifecycle transactions
// ---------------------------------------------------------------------------

/// Everything an install/update transaction needs about the artifact to
/// place: already-rendered content and digest evidence, computed by the
/// caller (CLI/core render pipeline). This module never renders.
pub struct ArtifactInput<'a> {
    pub trait_id: &'a str,
    pub source: &'a str,
    pub profile: ExtendedRenderProfile,
    pub format: Format,
    pub source_digest: &'a Digest,
    pub canonical_digest: &'a Digest,
    pub content: &'a str,
    /// Companion resource files placed alongside the leaf artifact (skill
    /// directory exports only), as scope-root-relative paths paired with
    /// exact bytes. Ownership of these is digest-keyed, not marker-keyed
    /// (see [`crate::export::write_companion`]) — they cannot carry the
    /// `> GENERATED FILE ...` marker without changing their bytes.
    pub companions: &'a [(Utf8PathBuf, Vec<u8>)],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedArtifact {
    pub path: Utf8PathBuf,
    pub content_digest: String,
}

pub struct InstallOutcome {
    pub record: PlacementRecord,
    pub artifact: PlacedArtifact,
    pub reinstalled: bool,
}

/// Read the current bytes at `path`, if any, for later rollback. `Ok(None)`
/// means the path does not exist yet (a fresh install with nothing to
/// restore); any other read failure is folded in as "nothing to restore"
/// too, since a rollback is already a best-effort recovery from a mutation
/// that itself already succeeded.
fn snapshot_bytes(path: &Utf8Path) -> Option<Vec<u8>> {
    std::fs::read(path.as_std_path()).ok()
}

/// Restore `path` to its pre-transaction bytes (or absence), atomically:
/// used only to undo an artifact write already known to have succeeded, so
/// the only failures possible here are a hostile concurrent change to the
/// target or a filesystem-level error — both must be surfaced rather than
/// swallowed, since a failed rollback leaves the artifact and the manifest
/// (already rolled back or about to be) out of sync.
fn restore_bytes(path: &Utf8Path, previous: &Option<Vec<u8>>) -> std::io::Result<()> {
    match previous {
        Some(bytes) => crate::write::write_bytes_atomically_raw(path, bytes),
        None => match std::fs::remove_file(path.as_std_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

/// Restore every `(path, previous bytes-or-absence)` snapshot in `entries`,
/// in order. One loop shared by every rollback stage below, so a single
/// artifact and N companion files roll back identically instead of forking
/// into per-arity copies.
fn restore_all(entries: &[(Utf8PathBuf, Option<Vec<u8>>)]) -> std::io::Result<()> {
    for (path, previous) in entries {
        restore_bytes(path, previous)?;
    }
    Ok(())
}

/// Undo every artifact write (leaf plus companions) after a later failure
/// (`original`) in the same transaction, and turn a rollback failure into a
/// distinct [`Error::RollbackFailed`] instead of silently returning
/// `original` while the artifacts are left in an unknown state.
fn rollback_artifacts(
    context: &'static str,
    original: Error,
    snapshots: &[(Utf8PathBuf, Option<Vec<u8>>)],
) -> Error {
    match restore_all(snapshots) {
        Ok(()) => original,
        Err(rollback_source) => Error::RollbackFailed {
            context,
            source: Box::new(original),
            rollback_source,
        },
    }
}

/// Undo both a manifest replacement and every artifact write after a later
/// failure (`original`) in the same transaction — used when the audit
/// append is what failed, after the manifest and artifacts were already
/// committed. Either rollback failing (manifest first, since it is cheaper
/// to detect) surfaces as [`Error::RollbackFailed`] rather than silently
/// returning `original` over an inconsistent state.
fn rollback_manifest_and_artifacts(
    context: &'static str,
    original: Error,
    manifest_path: &Utf8Path,
    previous_manifest: &PlacementManifest,
    snapshots: &[(Utf8PathBuf, Option<Vec<u8>>)],
) -> Error {
    if let Err(manifest_rollback_error) = save_manifest(manifest_path, previous_manifest) {
        return Error::RollbackFailed {
            context,
            source: Box::new(original),
            rollback_source: std::io::Error::other(manifest_rollback_error.to_string()),
        };
    }
    rollback_artifacts(context, original, snapshots)
}

/// Fields [`rollback_archive_manifest_and_artifacts`] needs to undo an
/// already-published archive alongside the manifest/artifacts, grouped to
/// keep the function signature under clippy's argument-count lint rather
/// than reaching for `#[allow]`.
struct ArchiveRollback<'a> {
    archive_path: &'a Utf8Path,
    previous_archive_bytes: &'a Option<Vec<u8>>,
    manifest_path: &'a Utf8Path,
    previous_manifest: &'a PlacementManifest,
    snapshots: &'a [(Utf8PathBuf, Option<Vec<u8>>)],
}

/// Undo an archive publish, a manifest replacement, and every artifact write
/// after a later failure (`original`) in the same transaction — used when
/// the audit append is what failed, after the archive, manifest, and
/// artifacts were already committed. The archive is restored first (cheapest
/// to detect a hostile concurrent change to), then the manifests/artifacts
/// via [`rollback_manifest_and_artifacts`]; any restore failing surfaces as
/// [`Error::RollbackFailed`] rather than silently returning `original` over
/// an inconsistent state.
fn rollback_archive_manifest_and_artifacts(
    context: &'static str,
    original: Error,
    rollback: ArchiveRollback<'_>,
) -> Error {
    let ArchiveRollback {
        archive_path,
        previous_archive_bytes,
        manifest_path,
        previous_manifest,
        snapshots,
    } = rollback;
    if let Err(archive_rollback_error) = restore_bytes(archive_path, previous_archive_bytes) {
        return Error::RollbackFailed {
            context,
            source: Box::new(original),
            rollback_source: archive_rollback_error,
        };
    }
    rollback_manifest_and_artifacts(
        context,
        original,
        manifest_path,
        previous_manifest,
        snapshots,
    )
}

/// Fields for one audit-journal event, grouped to keep the builder function
/// under clippy's argument-count lint rather than reaching for `#[allow]`.
/// Carries both the event timestamp and the previous/new digest evidence a
/// reader needs to see what identity a placement transitioned from and to
/// (absent -> new on install, previous -> new on update, previous -> absent
/// on remove) without cross-referencing another journal row.
struct PlacementAuditEvent<'a> {
    action: &'a str,
    unix_seconds: u64,
    scope: Scope,
    host: &'a str,
    trait_id: &'a str,
    root: &'a Utf8Path,
    paths: &'a [String],
    previous_source_digest: Option<&'a str>,
    new_source_digest: Option<&'a str>,
    previous_canonical_digest: Option<&'a str>,
    new_canonical_digest: Option<&'a str>,
    previous_content_digests: Option<&'a [String]>,
    new_content_digests: Option<&'a [String]>,
    archive_path: Option<&'a str>,
}

fn placement_audit_event(event: PlacementAuditEvent<'_>) -> serde_json::Value {
    let PlacementAuditEvent {
        action,
        unix_seconds,
        scope,
        host,
        trait_id,
        root,
        paths,
        previous_source_digest,
        new_source_digest,
        previous_canonical_digest,
        new_canonical_digest,
        previous_content_digests,
        new_content_digests,
        archive_path,
    } = event;
    serde_json::json!({
        "action": action,
        "at-unix-seconds": unix_seconds,
        "scope": scope.as_str(),
        "host": host,
        "trait": trait_id,
        "root": root.as_str(),
        "paths": paths,
        "previous-source-digest": previous_source_digest,
        "new-source-digest": new_source_digest,
        "previous-canonical-digest": previous_canonical_digest,
        "new-canonical-digest": new_canonical_digest,
        "previous-content-digests": previous_content_digests,
        "new-content-digests": new_content_digests,
        "archive-path": archive_path,
    })
}

/// Everything [`install`] needs about where and under what identity to
/// place an artifact, grouped so the function signature stays under
/// clippy's argument-count lint without reaching for `#[allow]`.
pub struct InstallRequest<'a> {
    pub manifest_path: &'a Utf8Path,
    pub scope: Scope,
    pub root: &'a Utf8Path,
    pub host: &'a str,
    pub relative_target: &'a Utf8Path,
    pub audit_root: &'a Utf8Path,
    pub unix_seconds: u64,
    pub archive_path: Option<&'a str>,
}

/// Install (or reinstall) one host placement. Reinstalling the same
/// `(scope, host, trait_id)` updates the existing record instead of adding a
/// duplicate. Refuses to adopt a placement whose recorded source locator
/// differs from `input.source` under the same identity (`--host` config
/// pointing a config-level host add at a different repository would
/// otherwise silently retarget an existing placement), and refuses a
/// reinstall whose resolved target has moved from the previously recorded
/// one rather than silently orphaning the old artifact.
///
/// The artifact write, the optional archive publish (`request.archive_path`),
/// the manifest update, and the required audit event all commit as one unit:
/// a manifest write failure rolls back only the artifact bytes; an archive
/// publish failure rolls back the manifest and artifact (the archive itself
/// was never written, so there is nothing to restore there); and an
/// `audit_root` append failure rolls back the archive (if one was
/// published), the manifest, and the artifact, all back to their pre-call
/// state, before returning `Err`.
pub fn install(
    request: InstallRequest<'_>,
    input: &ArtifactInput<'_>,
) -> Result<InstallOutcome, Error> {
    let InstallRequest {
        manifest_path,
        scope,
        root,
        host,
        relative_target,
        audit_root,
        unix_seconds,
        archive_path,
    } = request;
    let _lock = acquire_manifest_lock(manifest_path)?;
    let mut manifest = load_manifest(manifest_path)?;
    let previous_manifest = manifest.clone();

    if let Some(index) = manifest.find(scope, host, input.trait_id) {
        let existing = &manifest.placements[index];
        if existing.source != input.source {
            return Err(Error::SourceMismatch {
                scope: scope.as_str(),
                host: host.to_string(),
                trait_id: input.trait_id.to_string(),
                existing: existing.source.clone(),
                requested: input.source.to_string(),
            });
        }
        if existing.paths.first().map(String::as_str) != Some(relative_target.as_str()) {
            return Err(Error::TargetChanged {
                scope: scope.as_str(),
                host: host.to_string(),
                trait_id: input.trait_id.to_string(),
                existing_path: existing.paths.first().cloned().unwrap_or_default(),
                new_path: relative_target.to_string(),
            });
        }
    }

    let target_path = root.join(relative_target);
    let previous_artifact_bytes = snapshot_bytes(&target_path);
    let existing_record = manifest
        .find(scope, host, input.trait_id)
        .map(|index| manifest.placements[index].clone());

    // Build the archive's bytes in memory before any mutation: a malformed
    // entry (or any other archive-building failure) must be detected before
    // the artifact write, manifest update, or audit event commit anything.
    let archive_bytes = archive_path
        .map(|_| {
            let mut entries = vec![(
                relative_target.to_owned(),
                input.content.as_bytes().to_vec(),
            )];
            entries.extend(input.companions.iter().cloned());
            build_archive_bytes(&entries)
        })
        .transpose()?;

    let identity = Identity::new(
        Id::new(input.trait_id).map_err(|source| Error::from(crate::Error::from(source)))?,
        input.source_digest.clone(),
        input.format.ownership(input.profile),
    );

    // Companion resource files sit beside the leaf, inside the leaf's own
    // directory (`skill_resource_placement`'s `resources/<id>.<ext>` is
    // relative to `SKILL.md`, not to `root`) — so both the write and the
    // recorded manifest path are prefixed with the leaf's parent, matching
    // how `paths[0]` below already records the leaf as a root-relative path.
    let relative_target_dir = relative_target.parent().unwrap_or(Utf8Path::new(""));
    let companion_relative_targets: Vec<Utf8PathBuf> = input
        .companions
        .iter()
        .map(|(relative_path, _)| relative_target_dir.join(relative_path))
        .collect();
    // Companions are owned by recorded digest rather than marker (they
    // cannot carry the marker without changing the bytes their digest
    // pins): `Fresh` (refuse an existing unmanaged file) when nothing was
    // previously recorded for this path, e.g. a companion newly declared
    // since the last install of this same placement.
    let recorded_digests: Vec<Option<Digest>> = companion_relative_targets
        .iter()
        .map(|recorded_path| {
            existing_record.as_ref().and_then(|record| {
                record
                    .paths
                    .iter()
                    .position(|path| path == recorded_path.as_str())
                    .and_then(|index| record.content_digests.get(index))
                    .and_then(|digest| Digest::parse(digest).ok())
            })
        })
        .collect();
    let companion_ownership: Vec<crate::export::control::CompanionOwnership<'_>> = recorded_digests
        .iter()
        .map(|digest| match digest {
            Some(digest) => crate::export::control::CompanionOwnership::RecordedDigest(digest),
            None => crate::export::control::CompanionOwnership::Fresh,
        })
        .collect();
    let companion_requests: Vec<crate::export::control::Companion<'_>> = companion_relative_targets
        .iter()
        .zip(input.companions.iter())
        .zip(companion_ownership.iter())
        .map(
            |((relative_target, (_, bytes)), ownership)| crate::export::control::Companion {
                relative_target,
                bytes,
                ownership: *ownership,
            },
        )
        .collect();

    // Snapshot every path's prior bytes before the single validated write
    // below: `policy::write` validates the leaf and every companion before
    // writing any of them, so an ordinary refusal leaves nothing on disk to
    // restore, but a rarer failure inside the write itself (e.g. a mid-loop
    // I/O error) still needs this pre-capture to roll back cleanly.
    let mut companion_snapshots: Vec<(Utf8PathBuf, Option<Vec<u8>>)> = companion_relative_targets
        .iter()
        .map(|relative| {
            let full_path = root.join(relative);
            let previous = snapshot_bytes(&full_path);
            (full_path, previous)
        })
        .collect();

    use crate::export::control::Interface as _;
    let response = match crate::export::fs::Service.write(
        crate::export::control::Request::new(root, &identity, input.content, input.format)
            .with_relative_target(relative_target)
            .with_companions(&companion_requests),
    ) {
        Ok(response) => response,
        Err(error) => {
            let mut snapshots = vec![(target_path.clone(), previous_artifact_bytes.clone())];
            snapshots.append(&mut companion_snapshots);
            return Err(rollback_artifacts(
                "host-install write",
                Error::from(error),
                &snapshots,
            ));
        }
    };
    let written_companions: Vec<(String, String)> = response
        .companions
        .iter()
        .map(|companion| {
            let recorded_path = companion
                .path
                .strip_prefix(root)
                .unwrap_or(&companion.path)
                .to_string();
            (recorded_path, companion.content_digest.as_str().to_string())
        })
        .collect();
    let artifact_snapshots: Vec<(Utf8PathBuf, Option<Vec<u8>>)> =
        std::iter::once((target_path.clone(), previous_artifact_bytes.clone()))
            .chain(companion_snapshots)
            .collect();

    let mut paths = vec![
        response
            .path
            .strip_prefix(root)
            .unwrap_or(&response.path)
            .to_string(),
    ];
    let mut content_digests = vec![response.content_digest.as_str().to_string()];
    for (path, digest) in &written_companions {
        paths.push(path.clone());
        content_digests.push(digest.clone());
    }

    let record = PlacementRecord {
        scope,
        host: host.to_string(),
        trait_id: input.trait_id.to_string(),
        source: input.source.to_string(),
        profile: input.profile.as_str().to_string(),
        format: input.format.as_str().to_string(),
        paths,
        source_digest: input.source_digest.as_str().to_string(),
        canonical_digest: input.canonical_digest.as_str().to_string(),
        content_digests,
    };
    let reinstalled = if let Some(index) = manifest.find(scope, host, input.trait_id) {
        manifest.placements[index] = record.clone();
        true
    } else {
        manifest.placements.push(record.clone());
        false
    };

    if let Err(error) = save_manifest(manifest_path, &manifest) {
        return Err(rollback_artifacts(
            "host-install manifest write",
            error,
            &artifact_snapshots,
        ));
    }

    // Publish the archive, if requested, inside this same transaction: a
    // failure here must undo the manifest/artifact commit above too, and a
    // later audit-append failure must undo the archive publish below.
    let mut previous_archive_bytes = None;
    if let (Some(archive_path), Some(bytes)) = (archive_path, &archive_bytes) {
        let archive_path = Utf8Path::new(archive_path);
        previous_archive_bytes = snapshot_bytes(archive_path);
        if let Err(error) = publish_archive_bytes(archive_path, bytes) {
            return Err(rollback_manifest_and_artifacts(
                "host-install archive publish",
                error,
                manifest_path,
                &previous_manifest,
                &artifact_snapshots,
            ));
        }
    }

    let previous_record = previous_manifest
        .find(scope, host, input.trait_id)
        .map(|index| &previous_manifest.placements[index]);
    let event = placement_audit_event(PlacementAuditEvent {
        action: "install",
        unix_seconds,
        scope,
        host,
        trait_id: input.trait_id,
        root,
        paths: &record.paths,
        previous_source_digest: previous_record.map(|record| record.source_digest.as_str()),
        new_source_digest: Some(record.source_digest.as_str()),
        previous_canonical_digest: previous_record.map(|record| record.canonical_digest.as_str()),
        new_canonical_digest: Some(record.canonical_digest.as_str()),
        previous_content_digests: previous_record.map(|record| record.content_digests.as_slice()),
        new_content_digests: Some(&record.content_digests),
        archive_path,
    });
    if let Err(error) = crate::audit_journal::append(audit_root, unix_seconds, &event) {
        let error = Error::from(error);
        return Err(match archive_path {
            Some(archive_path) => rollback_archive_manifest_and_artifacts(
                "host-install audit append",
                error,
                ArchiveRollback {
                    archive_path: Utf8Path::new(archive_path),
                    previous_archive_bytes: &previous_archive_bytes,
                    manifest_path,
                    previous_manifest: &previous_manifest,
                    snapshots: &artifact_snapshots,
                },
            ),
            None => rollback_manifest_and_artifacts(
                "host-install audit append",
                error,
                manifest_path,
                &previous_manifest,
                &artifact_snapshots,
            ),
        });
    }

    Ok(InstallOutcome {
        artifact: PlacedArtifact {
            path: response.path,
            content_digest: response.content_digest.as_str().to_string(),
        },
        record,
        reinstalled,
    })
}

pub enum UpdateOutcome {
    Skipped {
        record: PlacementRecord,
    },
    /// A recorded path's on-disk bytes no longer match the digest recorded
    /// at the last install/update: a human edited a generated file. Nothing
    /// was written; pass `force: true` to overwrite deliberately.
    LocallyModified {
        record: PlacementRecord,
    },
    Updated {
        record: PlacementRecord,
        artifact: PlacedArtifact,
    },
}

/// Reconstruct one recorded placement from freshly rendered `input` and
/// write only if the resulting bytes differ from the recorded content
/// digest. `record` must be the exact record previously returned by
/// [`list_placements`]/[`install`] for this `(scope, host, trait_id)`.
///
/// Before writing, every recorded path (leaf and companions) is checked for
/// local modification since it was placed: `apply_update` previously
/// compared only the recorded digest to the *new* digest and never looked
/// at the file on disk, so a same-identity local edit was silently
/// overwritten. Unless `force` is set, a locally modified path now skips
/// the whole update and reports [`UpdateOutcome::LocallyModified`] instead
/// of writing anything.
///
/// Like [`install`], the artifact writes, manifest update, and audit event
/// commit as one unit with rollback of the manifest and every artifact
/// (leaf plus companions) on a later failure.
pub fn apply_update(
    manifest_path: &Utf8Path,
    root: &Utf8Path,
    record: &PlacementRecord,
    input: &ArtifactInput<'_>,
    audit_root: &Utf8Path,
    unix_seconds: u64,
    force: bool,
) -> Result<UpdateOutcome, Error> {
    let _lock = acquire_manifest_lock(manifest_path)?;
    let mut manifest = load_manifest(manifest_path)?;
    let previous_manifest = manifest.clone();
    let Some(index) = manifest.find(record.scope, &record.host, &record.trait_id) else {
        return Err(Error::PlacementNotFound {
            scope: record.scope.as_str(),
            host: record.host.clone(),
            trait_id: record.trait_id.clone(),
        });
    };

    let new_digest = Digest::from_bytes(input.content.as_bytes());
    let current = manifest.placements[index].clone();

    let relative_target: Utf8PathBuf = current
        .paths
        .first()
        .cloned()
        .ok_or_else(|| Error::PlacementNotFound {
            scope: record.scope.as_str(),
            host: record.host.clone(),
            trait_id: record.trait_id.clone(),
        })?
        .into();
    let target_path = root.join(&relative_target);

    // Scan every recorded path (leaf and companions) for local modification
    // BEFORE deciding whether there is anything to skip: a companion can
    // drift on its own even when the leaf's source content has not changed,
    // so the leaf-digest comparison below must never be the sole gate — see
    // the doc comment above for the incident this replaced.
    let recorded_source_digest = Digest::parse(&current.source_digest)
        .map_err(crate::Error::from)
        .map_err(Error::from)?;
    let recorded_identity = Identity::new(
        Id::new(current.trait_id.as_str())
            .map_err(|source| Error::from(crate::Error::from(source)))?,
        recorded_source_digest,
        input.format.ownership(input.profile),
    );
    let mut any_locally_modified = false;
    for (path, digest) in current.paths.iter().zip(current.content_digests.iter()) {
        let expected = Digest::parse(digest)
            .map_err(crate::Error::from)
            .map_err(Error::from)?;
        let is_leaf = current.paths.first().map(String::as_str) == Some(path.as_str());
        let (_target, state) = crate::export::inspect_managed(
            root,
            Utf8Path::new(path),
            is_leaf.then_some(&recorded_identity),
            &expected,
        )?;
        if matches!(state, crate::export::ManagedState::LocallyModified) {
            any_locally_modified = true;
            break;
        }
    }
    if any_locally_modified && !force {
        return Ok(UpdateOutcome::LocallyModified { record: current });
    }
    if !any_locally_modified
        && current.content_digests.first().map(String::as_str) == Some(new_digest.as_str())
    {
        return Ok(UpdateOutcome::Skipped { record: current });
    }

    let previous_artifact_bytes = snapshot_bytes(&target_path);

    let identity = Identity::new(
        Id::new(input.trait_id).map_err(|source| Error::from(crate::Error::from(source)))?,
        input.source_digest.clone(),
        input.format.ownership(input.profile),
    );

    // See the matching comment in `install`: companion paths are relative to
    // the leaf's own directory, not `root`.
    let relative_target_dir = relative_target
        .parent()
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|| Utf8PathBuf::from(""));
    let companion_relative_targets: Vec<Utf8PathBuf> = input
        .companions
        .iter()
        .map(|(relative_path, _)| relative_target_dir.join(relative_path))
        .collect();
    // `force` bypasses the digest-owned overwrite check deliberately for a
    // path this placement already has a recorded digest for (this is what
    // `--force` means); a path with no recorded digest is always `Fresh`
    // regardless of `force` — `--force` fixes drift on a known path, it
    // does not authorize clobbering an unrelated unmanaged file at a
    // brand-new companion path.
    let recorded_digests: Vec<Option<Digest>> = companion_relative_targets
        .iter()
        .map(|recorded_path| {
            current
                .paths
                .iter()
                .position(|path| path == recorded_path.as_str())
                .and_then(|index| current.content_digests.get(index))
                .and_then(|digest| Digest::parse(digest).ok())
        })
        .collect();
    let companion_ownership: Vec<crate::export::control::CompanionOwnership<'_>> = recorded_digests
        .iter()
        .map(|digest| match digest {
            Some(digest) if force => crate::export::control::CompanionOwnership::Force,
            Some(digest) => crate::export::control::CompanionOwnership::RecordedDigest(digest),
            None => crate::export::control::CompanionOwnership::Fresh,
        })
        .collect();
    let companion_requests: Vec<crate::export::control::Companion<'_>> = companion_relative_targets
        .iter()
        .zip(input.companions.iter())
        .zip(companion_ownership.iter())
        .map(
            |((relative_target, (_, bytes)), ownership)| crate::export::control::Companion {
                relative_target,
                bytes,
                ownership: *ownership,
            },
        )
        .collect();
    let mut companion_snapshots: Vec<(Utf8PathBuf, Option<Vec<u8>>)> = companion_relative_targets
        .iter()
        .map(|relative| {
            let full_path = root.join(relative);
            let previous = snapshot_bytes(&full_path);
            (full_path, previous)
        })
        .collect();

    use crate::export::control::Interface as _;
    let response = match crate::export::fs::Service.write(
        crate::export::control::Request::new(root, &identity, input.content, input.format)
            .with_relative_target(&relative_target)
            .with_companions(&companion_requests),
    ) {
        Ok(response) => response,
        Err(error) => {
            let mut snapshots = vec![(target_path.clone(), previous_artifact_bytes.clone())];
            snapshots.append(&mut companion_snapshots);
            return Err(rollback_artifacts(
                "host-update write",
                Error::from(error),
                &snapshots,
            ));
        }
    };
    let written_companions: Vec<(String, String)> = response
        .companions
        .iter()
        .map(|companion| {
            let recorded_path = companion
                .path
                .strip_prefix(root)
                .unwrap_or(&companion.path)
                .to_string();
            (recorded_path, companion.content_digest.as_str().to_string())
        })
        .collect();
    let artifact_snapshots: Vec<(Utf8PathBuf, Option<Vec<u8>>)> =
        std::iter::once((target_path.clone(), previous_artifact_bytes.clone()))
            .chain(companion_snapshots)
            .collect();

    let mut paths = vec![
        response
            .path
            .strip_prefix(root)
            .unwrap_or(&response.path)
            .to_string(),
    ];
    let mut content_digests = vec![response.content_digest.as_str().to_string()];
    for (path, digest) in &written_companions {
        paths.push(path.clone());
        content_digests.push(digest.clone());
    }

    let updated = PlacementRecord {
        source_digest: input.source_digest.as_str().to_string(),
        canonical_digest: input.canonical_digest.as_str().to_string(),
        paths,
        content_digests,
        ..current.clone()
    };
    manifest.placements[index] = updated.clone();

    if let Err(error) = save_manifest(manifest_path, &manifest) {
        return Err(rollback_artifacts(
            "host-update manifest write",
            error,
            &artifact_snapshots,
        ));
    }

    let event = placement_audit_event(PlacementAuditEvent {
        action: "update",
        unix_seconds,
        scope: updated.scope,
        host: &updated.host,
        trait_id: &updated.trait_id,
        root,
        paths: &updated.paths,
        previous_source_digest: Some(current.source_digest.as_str()),
        new_source_digest: Some(updated.source_digest.as_str()),
        previous_canonical_digest: Some(current.canonical_digest.as_str()),
        new_canonical_digest: Some(updated.canonical_digest.as_str()),
        previous_content_digests: Some(current.content_digests.as_slice()),
        new_content_digests: Some(&updated.content_digests),
        archive_path: None,
    });
    if let Err(error) = crate::audit_journal::append(audit_root, unix_seconds, &event) {
        return Err(rollback_manifest_and_artifacts(
            "host-update audit append",
            Error::from(error),
            manifest_path,
            &previous_manifest,
            &artifact_snapshots,
        ));
    }

    Ok(UpdateOutcome::Updated {
        record: updated,
        artifact: PlacedArtifact {
            path: response.path,
            content_digest: response.content_digest.as_str().to_string(),
        },
    })
}

/// Remove exactly one recorded `(scope, host, trait_id)` placement: every
/// recorded path must verify as a regular, non-symlink, still-owned,
/// digest-matching artifact *before* anything is deleted — one refused path
/// aborts the whole removal without touching any file or the manifest.
///
/// Once every path is verified, deletion, the manifest update, and the
/// audit event commit as one unit: a mid-deletion failure restores every
/// file already deleted in this call, and a later manifest or audit failure
/// restores every deleted file plus the pre-removal manifest.
pub fn remove(
    manifest_path: &Utf8Path,
    root: &Utf8Path,
    scope: Scope,
    host: &str,
    trait_id: &str,
    audit_root: &Utf8Path,
    unix_seconds: u64,
) -> Result<PlacementRecord, Error> {
    let _lock = acquire_manifest_lock(manifest_path)?;
    let mut manifest = load_manifest(manifest_path)?;
    let previous_manifest = manifest.clone();
    let Some(index) = manifest.find(scope, host, trait_id) else {
        return Err(Error::PlacementNotFound {
            scope: scope.as_str(),
            host: host.to_string(),
            trait_id: trait_id.to_string(),
        });
    };
    let record = manifest.placements[index].clone();
    if record.paths.len() != record.content_digests.len() {
        return Err(Error::RecordCardinalityMismatch {
            scope: scope.as_str(),
            host: host.to_string(),
            trait_id: trait_id.to_string(),
            paths: record.paths.len(),
            content_digests: record.content_digests.len(),
        });
    }

    let profile =
        ExtendedRenderProfile::parse(&record.profile).ok_or_else(|| Error::InvalidProfile {
            host: host.to_string(),
            profile: record.profile.clone(),
        })?;
    let format = Format::parse(&record.format).ok_or_else(|| Error::InvalidFormat {
        host: host.to_string(),
        format: record.format.clone(),
    })?;
    let source_digest = Digest::parse(&record.source_digest)
        .map_err(crate::Error::from)
        .map_err(Error::from)?;
    let identity = Identity::new(
        Id::new(trait_id.to_string()).map_err(|source| Error::from(crate::Error::from(source)))?,
        source_digest,
        format.ownership(profile),
    );

    // Preflight: verify every recorded path before deleting any of them.
    // Only the leaf (index 0) carries the marker; every companion is
    // digest-owned instead (it cannot carry the marker without changing its
    // bytes — see `write_companion`), so it is verified with `identity: None`.
    let mut planned: Vec<(Utf8PathBuf, Vec<u8>)> = Vec::with_capacity(record.paths.len());
    for (index, (path, digest)) in record
        .paths
        .iter()
        .zip(record.content_digests.iter())
        .enumerate()
    {
        let expected = Digest::parse(digest)
            .map_err(crate::Error::from)
            .map_err(Error::from)?;
        let leaf_identity = (index == 0).then_some(&identity);
        planned.push(crate::export::verify_removable(
            root,
            Utf8Path::new(path),
            leaf_identity,
            &expected,
        )?);
    }

    // Delete, tracking progress so a mid-loop failure can restore what this
    // call already removed.
    for (deleted, (target, _bytes)) in planned.iter().enumerate() {
        if let Err(source) = std::fs::remove_file(target.as_std_path()) {
            let delete_error =
                Error::from(crate::Error::from(crate::environment::Error::Filesystem {
                    path: target.to_string(),
                    source,
                }));
            for (target, bytes) in &planned[..deleted] {
                if let Err(rollback_source) = restore_bytes(target, &Some(bytes.clone())) {
                    return Err(Error::RollbackFailed {
                        context: "host-remove mid-deletion restore",
                        source: Box::new(delete_error),
                        rollback_source,
                    });
                }
            }
            return Err(delete_error);
        }
    }

    let rollback_files = |context: &'static str, original: Error| -> Error {
        for (target, bytes) in &planned {
            if let Err(rollback_source) = restore_bytes(target, &Some(bytes.clone())) {
                return Error::RollbackFailed {
                    context,
                    source: Box::new(original),
                    rollback_source,
                };
            }
        }
        original
    };

    manifest.placements.remove(index);
    if let Err(error) = save_manifest(manifest_path, &manifest) {
        return Err(rollback_files("host-remove manifest write", error));
    }

    let event = placement_audit_event(PlacementAuditEvent {
        action: "remove",
        unix_seconds,
        scope,
        host,
        trait_id,
        root,
        paths: &record.paths,
        previous_source_digest: Some(record.source_digest.as_str()),
        new_source_digest: None,
        previous_canonical_digest: Some(record.canonical_digest.as_str()),
        new_canonical_digest: None,
        previous_content_digests: Some(record.content_digests.as_slice()),
        new_content_digests: None,
        archive_path: None,
    });
    if let Err(error) = crate::audit_journal::append(audit_root, unix_seconds, &event) {
        if let Err(save_error) = save_manifest(manifest_path, &previous_manifest) {
            return Err(Error::RollbackFailed {
                context: "host-remove audit append",
                source: Box::new(Error::from(error)),
                rollback_source: std::io::Error::other(save_error.to_string()),
            });
        }
        return Err(rollback_files(
            "host-remove audit append",
            Error::from(error),
        ));
    }

    for (target, _bytes) in &planned {
        crate::export::prune_empty_ancestors(root, target.parent().unwrap_or(root));
    }
    Ok(record)
}

// ---------------------------------------------------------------------------
// Archive
// ---------------------------------------------------------------------------

/// Build a deterministic zip archive's bytes in memory, containing exactly
/// `entries` (relative forward-slash names paired with exact bytes), in the
/// given order, with fixed metadata/timestamps and stored (uncompressed)
/// entries. Kept separate from disk publishing so [`install`] can build and
/// validate archive bytes before any transaction mutation, then publish them
/// atomically alongside the manifest/artifact commit via
/// [`publish_archive_bytes`].
fn build_archive_bytes(entries: &[(Utf8PathBuf, Vec<u8>)]) -> Result<Vec<u8>, Error> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(
                zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                    .unwrap_or_else(|_| zip::DateTime::default()),
            );
        for (name, bytes) in entries {
            let entry_name = name.as_str().replace('\\', "/");
            writer.start_file(&entry_name, options)?;
            std::io::Write::write_all(&mut writer, bytes)
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: entry_name.clone(),
                    source,
                })
                .map_err(crate::Error::from)?;
        }
        writer.finish()?;
    }
    Ok(buffer.into_inner())
}

/// Atomically publish already-built archive bytes to `archive_path`,
/// creating its parent directory if needed. The archive contains host
/// artifacts only, never the placement manifest or audit journal.
fn publish_archive_bytes(archive_path: &Utf8Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = archive_path.parent()
        && !parent.as_str().is_empty()
    {
        crate::path_safety::create_dir_all_no_symlinks(parent, "archive output directory")
            .map_err(Error::from)?;
    }
    crate::write::write_bytes_atomically(archive_path, bytes).map_err(Error::from)
}
