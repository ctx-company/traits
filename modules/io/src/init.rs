//! Deterministic `ctx traits init` scaffolding for the current v2 layout.
//!
//! Initializes the repo-local trait roots and, optionally, a starter
//! package. Never touches the retired `.agents/traits/` layout, never
//! invokes a model, and never overwrites an existing authored file: a path
//! that already exists is reported as preserved and left untouched.

use camino::Utf8Path;

use ctx_traits_core::manifest::{PackageManifest, PackageMetadata, ProjectManifest};

/// One path `ctx traits init` touched, and whether it was newly created or
/// already existed and was left untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitEntry {
    Created(String),
    Preserved(String),
}

impl InitEntry {
    pub fn path(&self) -> &str {
        match self {
            InitEntry::Created(path) | InitEntry::Preserved(path) => path,
        }
    }
}

/// Deterministic, sorted report of every path `ctx traits init` touched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitReport {
    pub entries: Vec<InitEntry>,
}

/// Initialize `.ctx/traits/` and `.ctx/traits.toml` under `repo_root`, and,
/// when `name` is supplied, an additional starter package at
/// `.ctx/traits/<slugified-name>/` (`trait.toml` + `source/index.ts`).
///
/// Fails only on a genuine ambiguity (multiple existing project manifest
/// encodings) or an unsafe path (symlink ancestor/leaf); an existing path
/// that would otherwise be created is simply reported as preserved.
pub fn init(repo_root: &Utf8Path, name: Option<&str>) -> crate::Result<InitReport> {
    let mut entries = Vec::new();

    let authoring_root = crate::layout::trait_authoring_root_path(repo_root);
    entries.push(ensure_dir(&authoring_root, "trait authoring root")?);

    entries.push(ensure_project_manifest(repo_root)?);
    entries.push(ensure_runtime_config(repo_root)?);

    if let Some(name) = name {
        entries.extend(ensure_starter_package(repo_root, name)?);
    }

    entries.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(InitReport { entries })
}

/// P569: scaffold `.ctx/traits/runtime.toml` as a commented map of the
/// execution surface.
///
/// Every knob is present but COMMENTED OUT, showing its default. That is the
/// point: a written value freezes today's default into the file forever, so a
/// later ctx release that improves a default would never reach this repo — the
/// file-level twin of the wholesale-replace problem the merge model exists to
/// end. Commented keys document what exists while still inheriting.
///
/// The file is machine-local and gitignored; `ctx traits doctor --config`
/// remains the authority on what is actually in effect, with provenance.
fn ensure_runtime_config(repo_root: &Utf8Path) -> crate::Result<InitEntry> {
    let path = repo_root.join(crate::layout::RUNTIME_CONFIG);
    match crate::package_scaffold::create_new_file(
        &path,
        "runtime config",
        RUNTIME_CONFIG_TEMPLATE,
    )? {
        crate::package_scaffold::CreateOutcome::Created => Ok(InitEntry::Created(path.to_string())),
        crate::package_scaffold::CreateOutcome::AlreadyExists => {
            Ok(InitEntry::Preserved(path.to_string()))
        }
    }
}

/// Commented scaffold written by [`ensure_runtime_config`]. Values shown are
/// the built-in defaults at the time of writing; uncomment only what this
/// machine actually needs to differ on.
const RUNTIME_CONFIG_TEMPLATE: &str = r#"# ctx.traits runtime config — HOW traits execute on this machine.
#
# Machine-local and gitignored. The committed half lives beside it:
# `vendor.toml` (what this project depends on) and `packages/` (the traits).
#
# Every key below is commented out and shows its default. Uncomment only what
# you need to change: a commented key keeps inheriting future defaults, while a
# written one pins today's value forever.
#
# `ctx traits doctor --config` prints what is actually in effect, and where
# each value came from.

schema-version = "0.1.0"

# --- Agents -------------------------------------------------------------------
# Which model and harness each trait role runs on. Roles come from the trait
# (worker, reviewer, narrator, ...); anything unmapped falls back to
# `[agent.role.default]`.
#
# [agent.role.default]
# harness = "opencode"            # a built-in id: claude-code, opencode, pi
# model = "openai/gpt-5.6-terra"
# reasoning-effort = "medium"     # low | medium | high  (mapped per harness)
# session-mode = "per-frame"      # per-frame | persistent
# budget = { frame-seconds = 1800 }

# --- Run budgets --------------------------------------------------------------
# [run]
# total-seconds = 3600            # the ONLY whole-run clock
# max-frames = 200
# frame-seconds = 1800            # floor for roles declaring no budget
# max-retries = 3
# inline-prompt-bytes = 300000

# --- Worktree runs (`--worktree`) ---------------------------------------------
# [worktree]
# seed = []                       # gitignored paths to copy in (e.g. [".plans"])
# warm = []                       # dirs to copy-on-write clone (e.g. ["target"])
# setup = []                      # one-time commands, e.g. [["pnpm", "install"]]
# setup-seconds = 120
#
# [worktree.env]                  # env for every process in the worktree.
# # `{worktree}/...` resolves per run; `.ctx/...` against the invocation checkout.
# # CARGO_TARGET_DIR = "{worktree}/target"
#
# [worktree.retention]
# cheap = []                      # deleted at every drive exit
# expensive = []                  # kept for a warm resume, then aged out
#
# [worktree.confinement]
# enabled = true                  # harness-native write confinement
# sandbox = true                  # plus an OS sandbox around the spawn
# allow = []                      # extra writable dirs outside the worktree
#
# [worktree.tripwire]
# policy = "park"                 # park | warn — out-of-tree mutation findings

# --- Harnesses ----------------------------------------------------------------
# claude-code, opencode and pi are built in and need no configuration. A table
# here MERGES over the built-in, so state only what differs; an omitted field is
# inherited, and `flag = ""` explicitly unsets an inherited one.
#
# [harness.claude-code.cli]
# reasoning-effort-flag = "--effort"

# --- Pre-landing merge gate ---------------------------------------------------
# [merge]
# deep = false                    # use a judgment-capable merger
# gate = []                       # e.g. [["just", "test"]] — run before landing
"#;

fn ensure_dir(path: &Utf8Path, label: &str) -> crate::Result<InitEntry> {
    if crate::package_scaffold::ensure_dir(path, label)? {
        Ok(InitEntry::Created(path.to_string()))
    } else {
        Ok(InitEntry::Preserved(path.to_string()))
    }
}

fn ensure_project_manifest(repo_root: &Utf8Path) -> crate::Result<InitEntry> {
    match crate::discovery::manifest(repo_root)? {
        crate::discovery::ManifestDiscovery::Found(manifest) => {
            Ok(InitEntry::Preserved(manifest.path.to_string()))
        }
        crate::discovery::ManifestDiscovery::Conflict { found } => Err(fs_err(
            repo_root,
            format!(
                "multiple project manifests already exist ({}); resolve the conflict before running init",
                found
                    .iter()
                    .map(|m| m.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        crate::discovery::ManifestDiscovery::NotFound => {
            let path = crate::layout::project_manifest_path(repo_root, "toml");
            let manifest = ProjectManifest {
                schema_version: "0.1".to_string(),
                extends: None,
                project: None,
                trait_entries: Vec::new(),
                dependencies: Vec::new(),
                packages: std::collections::BTreeMap::new(),
            };
            let text = ctx_traits_core::encoding::encode(
                ctx_traits_core::encoding::Encoding::Toml,
                &manifest,
            )?;
            match crate::package_scaffold::create_new_file(&path, "project manifest", &text)? {
                crate::package_scaffold::CreateOutcome::Created => {
                    Ok(InitEntry::Created(path.to_string()))
                }
                crate::package_scaffold::CreateOutcome::AlreadyExists => {
                    Ok(InitEntry::Preserved(path.to_string()))
                }
            }
        }
    }
}

/// Ensure a starter package exists at `.ctx/traits/<slugified-name>/`.
///
/// `TraitPackageRoot::root()` — the `.ctx/traits/<id>/` directory itself —
/// is the single creation boundary, not any file within it: the package
/// root may hold `trait.toml`, `source/index.ts`, `source/index.mjs`,
/// generated documents, resources, a lock file, or any other human-authored
/// artifact, so no fixed set of file sentinels can stand in for "does this
/// package already exist". Claiming the root directory with an exclusive
/// `mkdir` answers that question and reserves the right to populate it in
/// the same atomic step: if the `mkdir` creates the directory, this call is
/// the sole owner of a genuinely new package and writes both starter files;
/// if the directory already exists in any form, the whole root is preserved
/// byte-for-byte and nothing is written.
fn ensure_starter_package(repo_root: &Utf8Path, name: &str) -> crate::Result<Vec<InitEntry>> {
    let requested_trait_id = ctx_traits_core::synth::slugify_trait_id(name)?;
    let package = crate::layout::TraitPackageRoot::new(repo_root, &requested_trait_id)?;

    if !crate::package_scaffold::claim_root(package.root(), "starter trait package root")? {
        return Ok(vec![InitEntry::Preserved(package.root().to_string())]);
    }

    entries_for_source(package.source_dir(), "starter trait source directory")?;

    let package_manifest_path = package.package_manifest();
    let manifest_text = package_manifest_text(&requested_trait_id, name)?;
    crate::package_scaffold::create_new_file(
        package_manifest_path,
        "starter trait package manifest",
        &manifest_text,
    )?;

    let source_path = package.source_dir().join("index.ts");
    let source_text = starter_source_text(&requested_trait_id, name);
    crate::package_scaffold::create_new_file(
        &source_path,
        "starter trait CDK source",
        &source_text,
    )?;

    Ok(vec![
        InitEntry::Created(package_manifest_path.to_string()),
        InitEntry::Created(source_path.to_string()),
    ])
}

fn entries_for_source(source_dir: &Utf8Path, label: &str) -> crate::Result<()> {
    crate::package_scaffold::ensure_dir(source_dir, label).map(|_created| ())
}

pub(crate) fn package_manifest_text(trait_id: &str, name: &str) -> crate::Result<String> {
    let manifest = PackageManifest {
        package: PackageMetadata {
            id: trait_id.to_string(),
            version: "0.1.0".to_string(),
            name: Some(name.to_string()),
            description: None,
            status: Default::default(),
        },
        family: None,
        dependencies: Default::default(),
    };
    Ok(ctx_traits_core::encoding::encode(
        ctx_traits_core::encoding::Encoding::Toml,
        &manifest,
    )?)
}

/// Encode `text` as a double-quoted TypeScript/JSON string literal. Every
/// character clap/CLI callers can pass as a starter name — quotes,
/// backslashes, newlines — round-trips through a valid escape sequence, so
/// the emitted source file is always syntactically complete regardless of
/// what the user typed.
fn ts_string_literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn starter_source_text(trait_id: &str, name: &str) -> String {
    format!(
        r#"import {{ agent, port, procedure, prompt, sequence, slot, trait }} from "@ctx-traits/cdk";

const summary = slot.text("summary");
const output = port.output.text({{ id: "summary", value: summary }});
const worker = agent("worker", {{ description: "Completes the starter task." }});

export const draft = trait({{
  id: {trait_id},
  name: {name},
  description: "Starter trait scaffolded by `ctx traits init`.",
  procedure: procedure({{
    description: "Describe what this trait should accomplish.",
    output,
    sequence: sequence.prompt({{
      id: "run",
      agent: worker,
      prompt: prompt.text`Describe the task for this trait.`,
      output: summary,
    }}),
  }}),
}});
"#,
        trait_id = ts_string_literal(trait_id),
        name = ts_string_literal(name),
    )
}

fn fs_err(path: &Utf8Path, message: impl Into<String>) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()),
    }
    .into()
}
