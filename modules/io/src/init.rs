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
    entries.push(ensure_project_config(repo_root)?);
    entries.push(ensure_runtime_example(repo_root)?);
    entries.push(ensure_authoring_manifest(repo_root)?);
    // `.ctx/.gitignore`, so the machine-local half of `.ctx` — worktrees, runs,
    // caches, and now `node_modules/` — is ignored from the first commit
    // rather than after someone notices. Idempotent, and it only ever appends
    // its own canonical entries.
    let ignore = crate::gitignore::ensure_nested_gitignore(repo_root)?;
    entries.push(if ignore.created {
        InitEntry::Created(ignore.path.to_string())
    } else {
        InitEntry::Preserved(ignore.path.to_string())
    });

    if let Some(name) = name {
        entries.extend(ensure_starter_package(repo_root, name)?);
    }

    entries.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(InitReport { entries })
}

/// The npm package version `ctx traits init` pins the authoring packages to.
///
/// Pinned, not `latest`: the CLI and the CDK agree on a canonical schema
/// version, so an `init` that floats would eventually scaffold a package the
/// installed binary cannot read. **This must move with every published release
/// of the authoring packages** — there is no gate proving it, and a stale value
/// here is only discovered by an author whose first build fails.
const AUTHORING_PACKAGE_VERSION: &str = "0.1.0-alpha.0";

/// Scaffold `.ctx/package.json`, the manifest for authoring-time npm packages.
///
/// Trait sources import `@ctx-traits/cdk` by bare specifier, and Node resolves
/// that by walking up from the importing file: `.ctx/traits/packages/<id>/
/// source/` → … → `.ctx/` → the repository root. So packages installed here are
/// found without any resolver configuration, and a project that already has its
/// own root `node_modules` keeps resolving through the next step up, unchanged.
///
/// Private and never published: this manifest exists to hold dependencies, not
/// to be a package. It is also the reason authoring works in a repository with
/// no JavaScript project of its own — previously the only advice on offer was
/// "run pnpm install", which assumed a workspace the author may never have.
///
/// Running an installed trait still needs no Node at all: a generated
/// `index.toml` is self-contained, and nothing here is on the run path.
fn ensure_authoring_manifest(repo_root: &Utf8Path) -> crate::Result<InitEntry> {
    let path = repo_root.join(".ctx").join("package.json");
    let content = format!(
        r#"{{
  "name": "ctx-traits-authoring",
  "private": true,
  "description": "Authoring-time dependencies for this project's ctx.traits packages. Created by `ctx traits init`; not published, and not needed to RUN a trait.",
  "type": "module",
  "dependencies": {{
    "@ctx-traits/cdk": "{AUTHORING_PACKAGE_VERSION}"
  }}
}}
"#
    );
    match crate::package_scaffold::create_new_file(&path, "authoring manifest", &content)? {
        crate::package_scaffold::CreateOutcome::Created => Ok(InitEntry::Created(path.to_string())),
        crate::package_scaffold::CreateOutcome::AlreadyExists => {
            Ok(InitEntry::Preserved(path.to_string()))
        }
    }
}

/// 0037: scaffold the committed `.ctx/traits/config.toml` — what the PROJECT
/// decides, for everyone.
///
/// Populated with the project-facing sections (worktree policy, run budgets,
/// the merge gate) and comments explaining each; the knobs themselves ship
/// COMMENTED OUT showing their defaults, per task 0019's rule — a written
/// value freezes today's default into the file forever, a commented one keeps
/// inheriting. This is a file the project fills in.
fn ensure_project_config(repo_root: &Utf8Path) -> crate::Result<InitEntry> {
    let path = repo_root.join(crate::layout::PROJECT_CONFIG);
    match crate::package_scaffold::create_new_file(
        &path,
        "project config",
        PROJECT_CONFIG_TEMPLATE,
    )? {
        crate::package_scaffold::CreateOutcome::Created => Ok(InitEntry::Created(path.to_string())),
        crate::package_scaffold::CreateOutcome::AlreadyExists => {
            Ok(InitEntry::Preserved(path.to_string()))
        }
    }
}

/// 0037: scaffold the committed `.ctx/traits/runtime.example.toml` — the
/// template for the machine-local `runtime.toml` beside it.
///
/// Fully commented, every knob present showing its default (task 0019's rule
/// again). `runtime.toml` itself is NEVER scaffolded: it is copied from this
/// example by whoever needs it, and is gitignored — `ctx traits init` writing
/// it would put one machine's file into everyone's diff.
fn ensure_runtime_example(repo_root: &Utf8Path) -> crate::Result<InitEntry> {
    let path = repo_root.join(crate::layout::RUNTIME_CONFIG_EXAMPLE);
    match crate::package_scaffold::create_new_file(
        &path,
        "runtime config example",
        RUNTIME_EXAMPLE_TEMPLATE,
    )? {
        crate::package_scaffold::CreateOutcome::Created => Ok(InitEntry::Created(path.to_string())),
        crate::package_scaffold::CreateOutcome::AlreadyExists => {
            Ok(InitEntry::Preserved(path.to_string()))
        }
    }
}

/// Committed project-tier scaffold written by [`ensure_project_config`].
/// Same schema as the runtime tier — the tier carries the meaning, not the
/// key set — so the sections here are guidance about what a PROJECT usually
/// decides, not a restriction.
const PROJECT_CONFIG_TEMPLATE: &str = r#"# ctx.traits project config — what THIS PROJECT decides, for everyone.
#
# Committed and shared. Decisions that travel with the repository belong here:
# "this project runs in worktrees", setup commands, seeds, the merge gate.
# The machine-local half lives beside it in `runtime.toml` (gitignored, copied
# from `runtime.example.toml`); a field stated there overrides the same field
# here and nothing else. Same schema in both files — the only question is
# "shared, or mine?".
#
# Every key below is commented out and shows its default. Uncomment only what
# this project actually decides: a commented key keeps inheriting future
# defaults, while a written one pins today's value forever.
#
# `ctx traits doctor --config` prints what is actually in effect, and which
# tier each value came from.

schema-version = "0.1.0"

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

# --- Run budgets --------------------------------------------------------------
# [run]
# total-seconds = 3600            # the ONLY whole-run clock
# max-frames = 200
# frame-seconds = 1800            # floor for roles declaring no budget
# max-retries = 3
# inline-prompt-bytes = 300000

# --- Post-run merge gate ------------------------------------------------------
# [merge]
# deep = false                    # use a judgment-capable merger
# gate = []                       # e.g. [["just", "test"]] — runs during post-run

# --- Agents (usually machine-local — see runtime.example.toml) -----------------
# A project MAY commit a seat decision, e.g. reviewers run at high effort:
#
# [agent.role.reviewer]
# reasoning-effort = "high"
#
# Committing a `model` here binds every collaborator to a model they may not
# have access to. Allowed — but models, credentials and machine paths usually
# belong in your own `runtime.toml`.
"#;

/// Fully-commented machine-tier template written by [`ensure_runtime_example`].
/// Values shown are the built-in defaults at the time of writing; copy to
/// `runtime.toml` and uncomment only what this machine actually needs to
/// differ on.
const RUNTIME_EXAMPLE_TEMPLATE: &str = r#"# ctx.traits runtime config — HOW traits execute on THIS machine.
#
# This is the committed EXAMPLE. Copy it to `runtime.toml` beside it (which is
# gitignored and never scaffolded) and uncomment what your machine needs:
# seats, models, credentials, absolute paths, and any budget you want
# different. A field stated in `runtime.toml` overrides the same field in the
# committed `config.toml` and nothing else — same schema in both files.
#
# Every key below is commented out and shows its default. Uncomment only what
# you need to change: a commented key keeps inheriting future defaults, while a
# written one pins today's value forever.
#
# `ctx traits doctor --config` prints what is actually in effect, and which
# tier each value came from.

schema-version = "0.1.0"

# --- Agents -------------------------------------------------------------------
# Which model and harness each trait role runs on. Roles come from the trait
# (worker, reviewer, narrator, ...); anything unmapped falls back to
# `[agent.role.default]`.
#
# [agent.role.default]
# harness = "opencode"            # a built-in id: claude-code, opencode, pi, codex
# model = "openai/gpt-5.6-terra"
# reasoning-effort = "medium"     # low | medium | high  (mapped per harness)
# session-mode = "per-frame"      # per-frame | persistent; persistent restates each turn's output contract inline and cannot use per-frame json-schema-flag enforcement
# budget = { frame-seconds = 1800 }

# --- Run budgets (machine overrides) -------------------------------------------
# [run]
# total-seconds = 3600            # the ONLY whole-run clock
# max-frames = 200
# frame-seconds = 1800            # floor for roles declaring no budget
# max-retries = 3
# inline-prompt-bytes = 300000

# --- Worktree environment (machine paths) --------------------------------------
# [worktree.env]                  # env for every process in the worktree.
# # `{worktree}/...` resolves per run; `.ctx/...` against the invocation checkout.
# # CARGO_TARGET_DIR = "{worktree}/target"

# --- Harnesses ----------------------------------------------------------------
# claude-code, opencode, pi and codex are built in and need no configuration. A table
# here MERGES over the built-in, so state only what differs; an omitted field is
# inherited, and `flag = ""` explicitly unsets an inherited one.
#
# [harness.claude-code.cli]
# reasoning-effort-flag = "--effort"
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
        // A scaffolded package does not publish yet. `ctx traits dependency
        // init` is what fills this in, so the author states their own npm
        // name rather than inheriting a scope they cannot publish to.
        publish: None,
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
        r#"import {{ agent, input, port, procedure, sequence, slot, trait }} from "@ctx-traits/cdk";

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
      prompt: input.prompt`Describe the task for this trait.`,
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    fn scratch_repo(label: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!("ctx-init-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("scratch repo root");
        dir
    }

    /// 0037: `init` scaffolds the committed pair — a populated
    /// `config.toml` and a fully-commented `runtime.example.toml` — and
    /// NEVER `runtime.toml` itself, which is copied from the example by
    /// whoever needs it and is gitignored from the first commit.
    #[test]
    fn init_scaffolds_the_committed_config_pair_and_never_runtime_toml() {
        let repo = scratch_repo("config-pair");
        let report = super::init(&repo, None).expect("init succeeds");

        let project_config = repo.join(crate::layout::PROJECT_CONFIG);
        let example = repo.join(crate::layout::RUNTIME_CONFIG_EXAMPLE);
        let runtime = repo.join(crate::layout::RUNTIME_CONFIG);
        assert!(project_config.exists(), "config.toml scaffolded");
        assert!(example.exists(), "runtime.example.toml scaffolded");
        assert!(!runtime.exists(), "runtime.toml is never scaffolded");
        assert!(
            report
                .entries
                .iter()
                .all(|entry| !entry.path().ends_with("traits/runtime.toml")),
            "runtime.toml never appears in the init report: {report:?}"
        );

        let ignore =
            std::fs::read_to_string(crate::gitignore::nested_gitignore_path(&repo).as_std_path())
                .expect("nested gitignore written");
        assert!(
            ignore.lines().any(|line| line == "traits/runtime.toml"),
            "machine tier ignored from the first commit: {ignore}"
        );
        let _ = std::fs::remove_dir_all(repo.as_std_path());
    }

    /// Both scaffolds decode as the one shared `RuntimeConfig` schema — the
    /// tier carries the meaning, not the key set — so an author uncommenting
    /// any shown knob starts from a document the resolver already accepts.
    #[test]
    fn both_templates_decode_as_the_shared_runtime_schema() {
        for (label, template) in [
            ("config.toml", super::PROJECT_CONFIG_TEMPLATE),
            ("runtime.example.toml", super::RUNTIME_EXAMPLE_TEMPLATE),
        ] {
            let decoded: Result<crate::harness_config::RuntimeConfig, _> = toml::from_str(template);
            assert!(decoded.is_ok(), "{label} template decodes: {decoded:?}");
        }
    }
}
