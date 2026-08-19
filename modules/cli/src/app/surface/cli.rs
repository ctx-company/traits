//! clap-based CLI argument definitions.

use std::ffi::OsString;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

const AGENT_ROLE_HELP: &str =
    "Declared agent role serving this frame, e.g. reviewer or agent:reviewer.";

/// The mandated release tagline (P453 review pass). Both `ctx traits -h`
/// (via [`traits_help_text`]) and `ctx traits help --json` (its `tagline`
/// field, see `help_surface.rs`) render this exact string — one source, so
/// the two surfaces cannot drift apart, and the byte-compare structural
/// proof pins the literal itself rather than only their mutual consistency.
pub(crate) const TRAITS_TAGLINE: &str =
    "typed, digest-locked agent procedures you can prove, reproduce, and gate";

/// Hand-authored `ctx traits --help` text.
///
/// clap 4.6's derive `Subcommand` has no per-subcommand `help_heading`: a
/// `Command` built from an enum variant carries no heading slot, and
/// `Command::subcommand_help_heading` is a single global setting for the
/// whole `Commands:` block, not a per-item one (verified against
/// `clap_builder` 4.6's source — there is no other primitive here short of
/// splitting `TraitsCommand` into several flattened sub-enums, which would
/// force a rewrite of the ~40-arm hidden-command match in
/// `command_handlers.rs`). This function builds the achievable equivalent: a
/// fully custom, bounded help body replacing clap's auto-rendered flat
/// `Commands:` wall with five genuine headed sections (P453's release
/// surface, regrouped by the 2026-08-18 owner ruling: core / author /
/// manage / ai assistance / execute, commands ordered most-important-first
/// within each group). It intentionally mirrors each visible variant's doc
/// comment below; hidden variants never appear here and stay directly
/// invocable regardless. Kept at or below 40 terminal rows.
fn traits_help_text() -> String {
    format!(
        "ctx.traits — {TRAITS_TAGLINE}.

Usage: ctx traits [OPTIONS] [COMMAND] [--json]  (bare `ctx traits` opens the dashboard on a TTY)
Core:
  init        Scaffold .ctx/traits/config.toml and .ctx/traits/, and optionally a starter package
  doctor      Inspect a folder of Agent-Skills-style files before importing (or, with --migrate-config, the legacy agent-config migration)
  cache       Cache lifecycle commands
Author:
  build       Compile a named trait or explicit TypeScript/JavaScript source path into the canonical trait document
  create      Scaffold a new trait package from a template, or list available templates
  fork        Fork an installed vendored dependency into an editable authored package
  import      Import an Agent Skills SKILL.md into a draft-status, unreviewed canonical trait package
Manage:
  list        List local trait packages from .ctx/traits, plus the built-in meta-trait packages
  check       Check a trait for validation, audit, and drift
  trust       Report or record this machine's local trust decisions: <trait>, approve, block, list [--stale]
  dependency  Packages this project depends on, and publishing your own: install (all declared), add <pkg>, remove, update, outdated, info, publish
  diff        Show layer-aware diff for a trait
AI Assistance:
  generate    Use a model to draft a new trait from a brief
  refine      Use a model to revise an existing canonical trait
  critique    Use a model to write an advisory design critique of a canonical trait
Execute:
  run         Run a trait end to end through configured harnesses
  merge       Land a completed `--worktree` run (--park-on-overlap restores strict overlap handling, --deep uses a judgment-capable merger)
Options:
      --session <SESSION>  Run-session ID or ledger path for commands such as `set`
  -h, --help                Print help
Run `ctx traits <command> --help` for a command's full options, and `ctx traits <hidden-command> --help` for internals not listed above."
    )
}

/// ctx.traits reference CLI and runtime.
#[derive(Parser, Debug)]
#[command(name = "ctx", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level commands.
#[derive(Subcommand, Debug)]
// `Traits`'s subcommand tree carries clap's full ~40-command `TraitsCommand`
// enum inline (272 bytes) against `Tasks`'s three-command one (56 bytes).
// Boxing `Traits`'s field to close the gap would touch every match site
// across `command_handlers.rs`, `dashboard.rs`, and the construction call in
// tests, for a one-time top-level `Command::parse` allocation that is not
// hot — not worth the churn for this lint.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Agent traits management.
    ///
    /// `disable_help_subcommand`: an explicit hidden `Help` variant below
    /// replaces clap's auto-generated `help` subcommand (which would
    /// otherwise collide with it) while `-h`/`--help` keep working exactly
    /// as before.
    #[command(override_help = traits_help_text(), disable_help_subcommand = true)]
    Traits {
        /// Run-session ID or ledger path for commands such as `set`.
        #[arg(long)]
        session: Option<String>,

        #[command(subcommand)]
        subcommand: Option<TraitsCommand>,
    },

    /// Task board document commands (sync, list, show) — the files-backed
    /// `TaskProvider` (0060), a top-level command since a task board is
    /// project-scoped, not trait-scoped. `ctx traits task import` (a
    /// one-shot markdown-to-TOML conversion) stays under `traits`.
    Tasks {
        #[command(subcommand)]
        subcommand: Option<TasksCommand>,
    },
}

/// Nested `ctx tasks ...` subcommands.
///
/// `Update` carries one flag per editable field (0063.5) against `Sync`'s
/// two — the same large-enum-variant tradeoff `Command` above already
/// accepts rather than boxing fields for a one-time `Command::parse`
/// allocation that is not hot.
#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum TasksCommand {
    /// Re-read the board directory and report dangling edges, unparseable
    /// documents, and duplicate keys. Manual only — never runs implicitly.
    Sync {
        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// List merge-time done-proposals (0063.8): tasks a merged bound run
    /// proposes closing, non-interactively — the same derivation the
    /// dashboard TASKS screen surfaces, recomputed on each look, listed
    /// only (accepting stays `tasks update <task> --status done`).
    Proposals {
        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// The full reconcile pass (0064): stale statuses and
    /// satisfied-but-unmet dependencies against checkable repository
    /// evidence (git ancestry, ledger task binding, park reports), listed
    /// only — never writes. Accepting a proposal stays `tasks update
    /// <task> ...` on the CLI, matching `Proposals`'s own precedent.
    Reconcile {
        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// List tasks on the board.
    List {
        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// Include archived (done/cancelled) tasks.
        #[arg(long)]
        archived: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one task, fully resolved (relations included).
    Show {
        /// The task's key, filename, or filename stem.
        task: String,

        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Apply a partial update to one task: only the fields named change
    /// (0063.5). Reads the task first to capture its digest, then refuses
    /// the write if the document changed since — the read-modify-write
    /// window this command itself opens.
    Update {
        /// The task's key, filename, or filename stem.
        task: String,

        /// Board directory. Defaults to `.internal/tasks`.
        #[arg(long)]
        board: Option<String>,

        /// New title. Refused if empty or whitespace-only.
        #[arg(long)]
        title: Option<String>,

        /// New stored status.
        #[arg(long)]
        status: Option<TaskUpdateStatus>,

        /// Replace the narrative content.
        #[arg(long)]
        content: Option<String>,

        /// Replace the scope prose. Pass an empty string to clear.
        #[arg(long)]
        scope: Option<String>,

        /// Replace the validation prose. Pass an empty string to clear.
        #[arg(long)]
        validation: Option<String>,

        /// Set the wall-clock deadline.
        #[arg(long, conflicts_with = "clear_wall")]
        wall: Option<String>,

        /// Clear the wall-clock deadline.
        #[arg(long)]
        clear_wall: bool,

        /// Set the origin.
        #[arg(long, conflicts_with = "clear_origin")]
        origin: Option<String>,

        /// Clear the origin.
        #[arg(long)]
        clear_origin: bool,

        /// Set the parent (makes this task a child).
        #[arg(long, conflicts_with = "clear_parent")]
        parent: Option<String>,

        /// Clear the parent.
        #[arg(long)]
        clear_parent: bool,

        /// Add a `depends-on` edge. Repeatable.
        #[arg(long = "add-depends-on")]
        add_depends_on: Vec<String>,

        /// Remove a `depends-on` edge. Repeatable.
        #[arg(long = "remove-depends-on")]
        remove_depends_on: Vec<String>,

        /// Mark a step done by id. Repeatable.
        #[arg(long = "step-done")]
        step_done: Vec<String>,

        /// Mark a step not-done by id. Repeatable.
        #[arg(long = "step-open")]
        step_open: Vec<String>,

        /// Release every task that directly depends on this one (removes
        /// the edge). Only valid alongside `--status done` or
        /// `--status cancelled` — the dependents sweep (0063.6).
        #[arg(long = "release-dependents")]
        release_dependents: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `--status` values for `ctx tasks update` — the closed set of stored
/// statuses, spelled the same as the schema's own `TaskStatus`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum TaskUpdateStatus {
    Ready,
    Done,
    Cancelled,
}

/// Nested `ctx traits ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum TraitsCommand {
    /// Scaffold .ctx/traits/config.toml and .ctx/traits/, and optionally a starter package.
    ///
    /// With no name, initializes the project roots only. With a name, also
    /// creates `.ctx/traits/<slugified-name>/trait.toml` and `source/index.ts`.
    /// Never overwrites an existing authored file and never touches the
    /// retired `.agents/traits/` layout.
    Init {
        /// Human-readable starter trait name to slugify into a trait ID.
        /// Omitted: initialize the project roots only.
        name: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Scaffold a new trait package from a template, or list available templates.
    ///
    /// With no name, lists the three first-party teaching templates
    /// (implement, research, review) and writes nothing. With a name and `--from
    /// <template>`, materializes that template into a new draft package at
    /// `.ctx/traits/<slugified-name>/`, builds it through the same
    /// internal path `ctx traits build` uses, and locks it through the
    /// same internal path `ctx traits vendor` uses.
    ///
    /// `create` (2026-08-18 rename of `new`, no compatibility alias) is
    /// deterministic and offline except for invoking the local CDK build
    /// runtime — unlike `generate`, it never calls a model.
    /// Never overwrites an existing package: an existing
    /// `.ctx/traits/<id>` is left byte-for-byte untouched and reported as
    /// an explicit error. The result is always `status = "draft"`; `create`
    /// itself writes no trust verdict and never auto-activates or
    /// auto-approves — the machine trust for the generated canonical
    /// digest is whatever this machine already has on file for that exact
    /// digest (unreviewed, unless an identical canonical output already
    /// has a machine trust record from a prior review). Use `ctx traits
    /// check` and `ctx traits trust approve` next.
    Create {
        /// Human-readable trait name to slugify into a trait ID. Required
        /// together with `--from`; omitted entirely to list templates.
        name: Option<String>,

        /// Template to scaffold from: implement, research, or review.
        #[arg(long)]
        from: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Fork an installed vendored dependency into an editable authored package.
    ///
    /// Turns `.ctx/traits/vendored/<id>` into `.ctx/traits/authored/<id>` in
    /// one transaction: copies the authoring subset (trait.toml, source/,
    /// resources/, reference/), rebuilds through the normal CDK build path,
    /// locks the result, records forked-from provenance (the vendored
    /// package's id, version, and canonical digest) in the authored
    /// manifest, then detaches the dependency (manifest declaration,
    /// project lock entry, and vendored tree all removed) so resolution
    /// sees only the authored fork. Never overwrites an existing authored
    /// package at that path: an existing `.ctx/traits/authored/<id>` is a
    /// loud error, byte-untouched, same as `create`. A build failure after
    /// the copy leaves no authored residue and the vendored dependency
    /// untouched. A byte-identical rebuild inherits this machine's existing
    /// trust verdict for that canonical digest; any edit changes the digest
    /// and goes through normal review.
    Fork {
        /// Installed dependency's manifest alias or exact source identity
        /// (npm package name, `path:<path>`, or `git+<url>[#path=<path>]`).
        id: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// List local trait packages from .ctx/traits, plus the built-in meta-trait packages.
    List {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Append the full per-package narrative (family groupings,
        /// shadow/origin detail) after the compact panel.
        #[arg(long)]
        verbose: bool,
    },
    /// Deterministic, read-only aggregation over this repository's run
    /// session ledgers: runs per trait, observed token evidence, and
    /// truthfully classifiable outcome counts. Writes nothing.
    #[command(hide = true)]
    Stats {
        /// Inclusive Unix-epoch-seconds cutoff against each run's recorded
        /// drive-outcome timestamp. Ledgers with no recorded timestamp are
        /// excluded from the filtered count and reported separately.
        #[arg(long)]
        since: Option<u64>,

        /// Exact, case-sensitive match against the persisted canonical
        /// trait ID recorded on each run.
        #[arg(long = "trait", value_name = "TRAIT_ID")]
        trait_id: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Machine-wide "what is running": every session the local
    /// liveness index has a row for, each probed against its own driver
    /// lock and reported live, orphaned (row present, lock free — a crashed
    /// or `kill -9`'d driver), or unknown (the local runtime root could not
    /// be resolved). One small index file read plus at most one bounded
    /// probe per row — never a scan of every ledger this machine has.
    /// Read-only — writes nothing.
    #[command(hide = true)]
    Running {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Render a persisted run-session ledger as a chronological narrative:
    /// who did what, verdicts and blockers, commands run, escalations, and
    /// how the run ended. Read-only — writes nothing.
    #[command(hide = true)]
    Story {
        /// Internal run-id (as recorded in the run-session ledger), a full
        /// session ID, an unambiguous session-ID prefix, or an explicit
        /// ledger path.
        run: String,

        /// Run-session store directory to resolve the run-id/session from.
        /// Defaults to this repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Emit a PR-comment-ready markdown block instead. Mutually
        /// exclusive with `--json`.
        #[arg(long, conflicts_with = "json")]
        markdown: bool,

        /// Requested narration depth: `default` (free, always available;
        /// per-step summaries and derived bullets from persisted activity),
        /// `detailed` (every persisted activity event with timestamps), or
        /// `assisted` (spends narrator model calls — the only level that
        /// does). `detailed`/`assisted` degrade to `default` with a stated
        /// notice when no activity was recorded for this run. Absent
        /// resolves to `default`.
        #[arg(long)]
        level: Option<String>,
    },
    /// Inspect a folder of Agent-Skills-style source files
    /// (SKILL.md/AGENTS.md/CLAUDE.md) before importing, without writing anything.
    ///
    /// Runs the same deterministic import planner `ctx traits import` uses,
    /// in dry-run mode: no writes, no network, no model calls. Aggregates
    /// per-file findings (existing hidden-content severities preserved) plus
    /// doctor-specific advisories (missing verification evidence, trait-ID
    /// or summary collisions, unknown freshness) into one report, and prints
    /// copy-pasteable `ctx traits import --source <path>` suggestions.
    Doctor {
        /// File or directory to inspect. Defaults to the current directory.
        #[arg(conflicts_with_all = ["config", "migrate_config"])]
        path: Option<String>,

        /// Report effective runtime configuration and its winning layer.
        #[arg(long)]
        config: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Scan every runtime-config layer for pre-P476 legacy `[agent]`
        /// keys (`[agent.master]`/`[agent.narrator]`/`[agent.merger]`/
        /// `[agent.merger-deep]`, or a bare `agent.<scalar>`) and report the
        /// exact `[agent.role.*]` rewrite for each. Read-only unless
        /// `--apply` is also given. Mutually exclusive with the default
        /// source-inspection report and `--config`.
        #[arg(long = "migrate-config", conflicts_with_all = ["config"])]
        migrate_config: bool,

        /// Perform the plan a companion mode reports. Two mutually exclusive
        /// modes: with `--migrate-config`, rewrite nonconflicting legacy
        /// `[agent]` keys to their `[agent.role.*]` destination (never
        /// overwrites a conflict, never rewrites a P457-generated
        /// `config.toml`, leaves source data intact on a per-entry failure);
        /// alone (no `--migrate-config`, no `--config`), append any missing
        /// canonical entry to the invocation repository's nested
        /// `.ctx/.gitignore` (P446) — creating it if absent, preserving
        /// every existing byte otherwise, and never performing Git index
        /// surgery (a tracked runtime path is only ever reported with a
        /// `git rm --cached` remedy, never removed by doctor itself).
        /// Invalid together with `--config`.
        #[arg(long)]
        apply: bool,

        /// Append the full per-candidate narrative (every field, including
        /// healthy candidates) after the compact panel. Applies only to the
        /// default source-inspection report; mutually exclusive with
        /// `--config` and `--migrate-config`.
        #[arg(long, conflicts_with_all = ["config", "migrate_config"])]
        verbose: bool,
    },
    /// Show launch claim readiness and allowed public wording.
    #[command(hide = true)]
    ClaimGate {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve and vendor declared trait dependencies.
    ///
    /// npm sources (both package-local `source.package` dependencies and
    /// project `[dependencies]`) are fetched, integrity-verified, and
    /// extracted entirely in Rust from the registry into this repo's cache;
    /// no `node`, `npm`, or `pnpm` process is ever invoked on this path (P438).
    ///
    /// Formerly `sync`, kept as a hidden alias for one release.
    /// Operations on packages this project depends on (P567): `install`,
    /// `add`, `remove`, `update`, `outdated`, `info`.
    Dependency {
        /// Emit structured JSON. Applies to whichever dependency subcommand is
        /// given; equivalent to that subcommand's own `--json`.
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        subcommand: DependencyCommand,
    },
    #[command(alias = "sync", hide = true)]
    Vendor {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        /// Omitted: vendor every package under .ctx/traits.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Optional project manifest path. Defaults to shallow discovery in the current directory.
        #[arg(long)]
        manifest: Option<String>,

        /// Trait file whose package-local lock is read or written.
        #[arg(long)]
        file: Option<String>,

        /// Verify the vendor tree and lock evidence without writing.
        #[arg(long)]
        locked: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    // The six pre-P567 bare dependency verbs (`install`, `remove`, `update`,
    // `outdated`, `info`, `publish`) lived here as hidden aliases for one
    // release after the `dependency` group superseded them. Removed before
    // the surface went public (owner ruling 2026-07-29): an alias nobody has
    // ever depended on is free to delete exactly once.
    /// Report or record this machine's local trust decisions, keyed by a
    /// trait's exact current canonical digest.
    ///
    /// Bare `ctx traits trust <trait>` reports resolved status; `approve`
    /// and `block` record a decision (falling through to package resolution
    /// for `approve` when the operand is not a trait); `list` reports every
    /// recorded decision. A trait literally named `approve`, `block`, or
    /// `list` cannot be queried through the bare form — pass `--file`
    /// instead.
    Trust {
        /// Trait name (resolved from .ctx/traits, falling back to a
        /// built-in meta-trait) or explicit file path to report status for.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to report status for.
        #[arg(long)]
        file: Option<String>,

        /// Emit structured JSON. Applies to whichever trust subcommand is
        /// given; equivalent to that subcommand's own `--json`.
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        subcommand: Option<TrustCommand>,
    },
    /// Report trait hygiene, trigger inventory, and safe prune planning.
    #[command(hide = true)]
    Hygiene {
        /// Canonical trait file to include. Repeat for multiple traits.
        #[arg(long = "file", value_name = "TRAIT_FILE", required = true)]
        trait_files: Vec<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Estimate context cost by trait layer.
    #[command(hide = true)]
    Cost {
        /// Trait file to estimate.
        #[arg(long)]
        file: String,

        /// Optional token budget to compare against.
        #[arg(long)]
        budget: Option<u64>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Plan public/private publish preparation without writing public output.
    #[command(hide = true)]
    PreparePublic {
        /// Trait file to inspect.
        #[arg(long)]
        file: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Explain behavior/project-memory/context contract boundaries.
    #[command(hide = true)]
    ContextContracts {
        /// Trait file to inspect.
        #[arg(long)]
        file: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Classify advisory policy versus enforceable host/runtime plans.
    #[command(hide = true)]
    Policy {
        /// Trait file to inspect.
        #[arg(long)]
        file: String,

        /// Target render/profile capability context.
        #[arg(long, default_value = "agent-skills")]
        profile: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Generate a compact review evidence bundle.
    #[command(hide = true)]
    Evidence {
        /// Trait file to inspect.
        #[arg(long)]
        file: String,

        /// Target render/profile context.
        #[arg(long, default_value = "agent-skills")]
        profile: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show prioritized host compatibility matrix.
    #[command(hide = true)]
    Compatibility {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Explain subagent advisory propagation for a profile.
    #[command(hide = true)]
    Subagent {
        /// Trait file to inspect.
        #[arg(long)]
        file: String,

        /// Target render/profile context.
        #[arg(long, default_value = "agent-skills")]
        profile: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Explain why a trait would or wouldn't activate for a task, or emit a
    /// deterministic explain report with --scaffold.
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Explain {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// User task text for activation-explain mode. Required unless
        /// --scaffold is supplied.
        #[arg(long)]
        task: Option<String>,

        /// Emit deterministic ExplainScaffold mode. Requires exactly one
        /// --file, rejects --task, and performs no provider/model call.
        #[arg(long)]
        scaffold: bool,

        /// Canonical trait file to load and score. Repeat for multiple traits.
        #[arg(long = "file", value_name = "TRAIT_FILE")]
        trait_files: Vec<String>,

        /// Task/source file paths used by activation matching. Repeat for
        /// multiple files. Distinct from `--file` which loads trait manifests.
        #[arg(long = "files", value_name = "FILE")]
        files: Vec<String>,

        /// Optional task mode hint.
        #[arg(long)]
        mode: Option<String>,

        /// Language hint. Repeat for multiple languages.
        #[arg(long = "language", value_name = "LANGUAGE")]
        languages: Vec<String>,

        /// Runtime signal fact. Repeat for multiple signals.
        #[arg(long = "signal", value_name = "SIGNAL")]
        signals: Vec<String>,

        /// Direct invocation evidence text for manual-activation traits.
        #[arg(long = "explicit-invocation", value_name = "TEXT")]
        explicit_invocation: Option<String>,

        /// Only show active candidates.
        #[arg(long)]
        active_only: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Optional trait ID filter. Also counts as direct invocation evidence
        /// for the matching candidate.
        #[arg(long)]
        trait_id: Option<String>,

        /// Source map sidecar for --scaffold. Repository packages default to
        /// `.ctx/traits/<id>/generated/index.map`.
        #[arg(long = "source-map", visible_alias = "map", value_name = "TRAIT_MAP")]
        source_map: Option<String>,

        /// Show the full candidate/scaffold detail report instead of the compact summary.
        #[arg(long)]
        verbose: bool,

        /// Narrate the --scaffold evidence through the explain-trait runner
        /// instead of stopping at the deterministic scaffold. No effect
        /// without --scaffold; deterministic explain is unchanged.
        #[arg(long = "llm-assisted")]
        llm_assisted: bool,

        /// Path to raw narrated explain-scaffold output (for testing gates
        /// without a provider). No effect without --llm-assisted.
        #[arg(long)]
        candidate: Option<String>,

        /// Provider/model ID for the explain-trait narrator. No effect
        /// without --llm-assisted.
        #[arg(long)]
        model: Option<String>,

        /// Path to a `[budget]` document (0176) capping the --llm-assisted
        /// explain-trait runner. Routing goes through --assign. No effect
        /// without --llm-assisted or with --candidate.
        #[arg(long, value_name = "PATH")]
        budget: Option<String>,

        /// Override the explain-trait generator agent assignment. No effect
        /// without --llm-assisted.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,
    },
    /// Inspect trait identity, lifecycle, scenarios, evals, or dry-run plan.
    #[command(hide = true)]
    Inspect {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to inspect.
        #[arg(long)]
        file: Option<String>,

        /// Show the procedure dry plan (ports, sequence items, slots).
        #[arg(long)]
        dry_plan: bool,

        /// Render profile for resource compatibility warnings in dry-plan.
        /// Defaults to agent-skills.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Discover the project manifest in the current directory.
    #[command(hide = true)]
    Manifest,
    /// P468 TUI kit demo: exercises list scroll, master-detail focus, both
    /// modal variants, and the `$EDITOR` round-trip in one screen. Requires
    /// an interactive TTY.
    #[command(hide = true)]
    TuiDemo,
    /// Open a modal alt-screen editor for one trait package: browse its
    /// sections, open its authored source in `$EDITOR`, rebuild from CDK
    /// source, and run `check` — all with the same one-keypress-per-action
    /// model as the dashboard's TRAITS screen. Requires an interactive TTY.
    #[command(hide = true)]
    Edit {
        /// Trait name or local trait file path.
        trait_arg: String,
    },
    /// Emit JSON Schema for the canonical normalized JSON shape of the Agent
    /// Traits trait-root model.
    ///
    /// This schema describes canonical serialized JSON output, not the full
    /// TOML/YAML authoring shape. Taxonomy fields that accept scalar-or-array
    /// shorthand appear as arrays (their canonical form). The schema targets
    /// `.protocol/agent-traits/` as a non-authoritative support artifact.
    #[command(hide = true)]
    Schema {
        /// Protocol to target (default: agent-traits).
        #[arg(long, default_value = "agent-traits")]
        protocol: String,

        /// Output format (default: json).
        #[arg(long, default_value = "json")]
        format: String,

        /// Output file path. If omitted, writes to stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Generate the Rust-derived TypeScript SDK type mirror.
    #[command(hide = true)]
    SdkGenerate {
        /// Verify generated output and hand-written vocabulary without writing.
        #[arg(long)]
        check: bool,
    },
    /// Deterministically synthesize canonical TOML/JSON/YAML from draft JSON.
    ///
    /// `synth` never executes TypeScript, Rust generators, model calls,
    /// provider hooks, or host code. If `--out` is omitted and `--check` is not
    /// set, stdout contains only the synthesized canonical document text.
    #[command(hide = true)]
    Synth {
        /// Draft JSON input path.
        path: String,

        /// Output format: toml, json, or yaml. Defaults to toml.
        #[arg(long, default_value = "toml")]
        format: String,

        /// Output file path. If omitted, synth writes canonical text to stdout;
        /// with --check omitted, no report text is mixed into stdout.
        #[arg(long)]
        out: Option<String>,

        /// Compare synthesized output with the target file and report drift
        /// without rewriting. Uses --out as the target, or `<path>` with the
        /// selected format extension when --out is omitted.
        #[arg(long)]
        check: bool,
    },
    /// Compile a named trait or explicit TypeScript/JavaScript authoring source
    /// path into the canonical trait document.
    ///
    /// `build` executes only at the CLI/IO boundary. The authoring module must
    /// emit draft JSON by exporting `default` or `draft`;
    /// core synth still receives parsed draft JSON only.
    Build {
        /// Trait name to rebuild. Pass an explicit .ts or .mjs source path as
        /// an escape hatch.
        #[arg(value_name = "TRAIT")]
        path: String,

        /// Output format: toml, json, or yaml. Defaults to toml.
        #[arg(long, default_value = "toml")]
        format: String,

        /// Output file path. For `.ctx/traits/<id>/trait.ts|mjs`, the default is
        /// `.ctx/traits/<id>/generated/index.<format>`; external sources write adjacent.
        #[arg(long)]
        out: Option<String>,

        /// Emit structured JSON build evidence instead of the plain report.
        #[arg(long)]
        json: bool,

        /// Rewrite an existing import-resolved-dependency lock pin instead
        /// of refusing on digest mismatch. Never a side effect of an
        /// ordinary build (task 0170).
        #[arg(long)]
        relock: bool,
    },
    /// Mechanically migrate a canonical trait from its declared
    /// `schema-version` to a newer supported version.
    ///
    /// Without `--apply`, prints a reviewable diff and digest before/after
    /// without writing. With `--apply`, writes the migrated document and
    /// reports that its canonical digest moved — trust re-approval follows.
    /// Refuses rather than guessing when a construct can't be mechanically
    /// rewritten or the migrated output fails to round-trip decode.
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Migrate {
        /// Trait name to migrate. Pass an explicit canonical trait file path
        /// as an escape hatch.
        #[arg(value_name = "TRAIT")]
        id_or_path: String,

        /// Target schema-version. Defaults to the latest version this
        /// binary supports.
        #[arg(long)]
        to: Option<String>,

        /// Write the migrated document after gates pass.
        #[arg(long)]
        apply: bool,

        /// Emit structured JSON instead of the plain report.
        #[arg(long)]
        json: bool,
    },
    /// Use a model to draft a new trait from a brief.
    ///
    /// `generate` is always LLM-assisted by default and is not a deterministic
    /// synth alias. The default candidate package path is
    /// the repo-local trait source root under `<slugified-name>/`; generated output remains
    /// package status=draft and machine trust=unreviewed until review and activation.
    Generate {
        /// Human-readable trait name to slugify into the default trait ID.
        name: String,

        /// Brief for the model-assisted candidate generator.
        brief: String,

        /// Provider/model ID for LLM-assisted generation.
        #[arg(long)]
        model: Option<String>,

        /// Override the generate-trait agent assignment.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,

        /// Candidate output path override. If omitted, uses
        /// the repo-local trait source root under `<trait-id>/trait.toml`.
        #[arg(long)]
        out: Option<String>,

        /// Path to a raw candidate output file (JSON/TOML/YAML). When
        /// supplied, gates evaluate the candidate without a provider call.
        #[arg(long)]
        candidate: Option<String>,

        /// Require provider/candidate validation success; never mutates trusted
        /// state on failure.
        #[arg(long)]
        check: bool,

        /// Emit structured JSON using the assist candidate envelope.
        #[arg(long)]
        json: bool,
    },
    /// Evaluate one authoring-source candidate through the rung ladder.
    ///
    /// Internal: the only intended caller is `generate-trait`'s in-loop
    /// evaluate step (task 0066.1). Never calls a provider, never loops —
    /// exactly one round, always exits 0 and prints the round report as
    /// JSON regardless of convergence; the meta-trait's own loop primitive
    /// decides whether to continue.
    #[command(hide = true)]
    GenerateRound {
        /// Trait ID the candidate must declare; also keys the scratch package.
        trait_id: String,

        /// Candidate authoring source (TypeScript) text.
        candidate: String,
    },
    /// Evaluate one refine scaffold candidate through the rung ladder.
    ///
    /// Internal: the only intended caller is `refine-trait`'s in-loop
    /// evaluate step (task 0066.3). Never calls a provider, never loops —
    /// exactly one round, always exits 0 and prints the round report as
    /// JSON regardless of convergence; the meta-trait's own loop primitive
    /// decides whether to continue.
    #[command(hide = true)]
    RefineRound {
        /// Filesystem path whose lines patch anchors must reference; also
        /// re-read for the source trait identity and digest.
        source_path: String,

        /// Candidate refine scaffold (JSON) text.
        candidate: String,
    },
    /// Evaluate one import trait-draft candidate through the rung ladder.
    ///
    /// Internal: the only intended caller is `import-trait`'s in-loop
    /// evaluate step (task 0066.3). Never calls a provider, never loops —
    /// exactly one round, always exits 0 and prints the round report as
    /// JSON regardless of convergence; the meta-trait's own loop primitive
    /// decides whether to continue.
    #[command(hide = true)]
    ImportRound {
        /// Trait ID the candidate must declare; also keys the scratch
        /// package and the persisted scaffold baseline.
        trait_id: String,

        /// Candidate trait draft (JSON) text.
        candidate: String,
    },
    /// LLM-assisted refinement of an existing canonical trait.
    ///
    /// `refine` loads existing canonical source and produces a candidate patch
    /// or complete canonical draft. Without `--apply`, writes only to `--out`
    /// or prints the candidate report. With `--apply`, mutates canonical source
    /// only after gates pass. Never edits generated exports directly.
    Refine {
        /// Trait name to refine. Pass an explicit canonical trait file path as
        /// an escape hatch.
        #[arg(value_name = "TRAIT")]
        id_or_path: String,

        /// Change request describing the desired refinement.
        change_request: String,

        /// Provider/model ID for LLM-assisted refinement.
        #[arg(long)]
        model: Option<String>,

        /// Override the refine-trait agent assignment.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,

        /// Output path for candidate. If omitted, prints candidate report only.
        #[arg(long)]
        out: Option<String>,

        /// Apply candidate to canonical source after gates pass.
        #[arg(long)]
        apply: bool,

        /// Path to raw candidate output (for testing gates without a provider).
        #[arg(long)]
        candidate: Option<String>,

        /// Require gate success; exit nonzero on failure.
        #[arg(long)]
        check: bool,

        /// Emit structured JSON using the assist candidate envelope.
        #[arg(long)]
        json: bool,
    },
    /// LLM-assisted advisory design critique of a canonical trait.
    Critique {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Canonical trait file to critique.
        #[arg(long)]
        file: Option<String>,

        /// Source map sidecar. Repository packages default to
        /// `.ctx/traits/<id>/generated/index.map`.
        #[arg(long = "source-map", visible_alias = "map", value_name = "TRAIT_MAP")]
        source_map: Option<String>,

        /// Provider/model ID for the critique reviewer.
        #[arg(long)]
        model: Option<String>,

        /// Override the critique-trait reviewer assignment.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,

        /// Path to raw JSON ReviewScaffold output for deterministic validation.
        #[arg(long)]
        candidate: Option<String>,

        /// Emit structured JSON using the assist candidate envelope.
        #[arg(long)]
        json: bool,
    },
    /// LLM-assisted synthesis of deferred behavioral/runtime eval declarations.
    #[command(hide = true)]
    GenerateEvals {
        /// Canonical trait file to extend in memory.
        #[arg(long)]
        file: String,

        /// Provider/model ID for the eval synthesis author.
        #[arg(long)]
        model: Option<String>,

        /// Override the generate-evals agent assignment.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,

        /// Path to raw JSON EvalSynthesisScaffold output for deterministic validation.
        #[arg(long)]
        candidate: Option<String>,

        /// Emit structured JSON using the assist candidate envelope.
        #[arg(long)]
        json: bool,
    },
    /// Import an Agent Skills SKILL.md into a draft-status, unreviewed canonical trait package.
    ///
    /// Persists the complete package by default (root status draft, canonical
    /// digest unreviewed on this machine): `check`, then team `activate` and
    /// personal `trust approve`, then `run` — same as any local trait.
    Import {
        /// Source SKILL.md file or directory containing SKILL.md.
        #[arg(long)]
        source: String,

        /// Source format hint for the deterministic import parser: what kind
        /// of document `--source` is. P59.1 supports agent-skills. Unrelated
        /// to `--budget`, which caps the `--llm-assisted` runner instead of
        /// selecting the source format.
        #[arg(long)]
        profile: Option<String>,

        /// Path to a `[budget]` document (0176) capping the `--llm-assisted`
        /// import-trait runner. Routing goes through `--assign`. Has no
        /// effect on deterministic import or an offline `--candidate`
        /// import, since neither dispatches a harness.
        #[arg(long, value_name = "PATH")]
        budget: Option<String>,

        /// Package directory or canonical-file path to write. Defaults to
        /// .ctx/traits/<id>/ (generated/index.toml) when omitted.
        #[arg(long)]
        out: Option<String>,

        /// Compare generated trait document and import-report.json against
        /// the target path without writing anything.
        #[arg(long)]
        check: bool,

        /// Enable LLM-assisted enrichment after deterministic import.
        #[arg(long)]
        llm_assisted: bool,

        /// Provider/model ID for LLM-assisted import enrichment.
        #[arg(long)]
        model: Option<String>,

        /// Override the import-trait agent assignment.
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL]]]"
        )]
        assignments: Vec<String>,

        /// Path to raw candidate output for LLM-assisted import.
        #[arg(long)]
        candidate: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Append the full per-field import narrative (inferred/unsupported
        /// fields, frontmatter mapping, warnings, review actions) after the
        /// compact panel. Applies only to a successful deterministic write.
        #[arg(long)]
        verbose: bool,
    },
    /// Refresh imported source artifacts and report dual-layer diffs.
    ///
    /// Re-reads source artifacts, builds a new import snapshot, and compares
    /// against the current package-local trait.lock snapshot.
    #[command(hide = true)]
    ImportRefresh {
        /// Trait ID or package directory path to refresh.
        trait_id_or_package: String,

        /// Source path override. If omitted, uses the stored source locator.
        #[arg(long)]
        source: Option<String>,

        /// Compare without writing canonical trait changes.
        #[arg(long)]
        check: bool,

        /// Output directory for candidate output. Writes nothing unless supplied.
        #[arg(long)]
        out: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect required inputs before starting a run. Does not create a session.
    #[command(hide = true)]
    RunInfo {
        /// Trait ID to resolve from the repo-local trait source root.
        trait_id: Option<String>,

        /// Trait file to inspect.
        #[arg(long)]
        file: Option<String>,

        /// Query text after `--` for selection preflight, e.g. ctx traits run-info -- "review this code".
        #[arg(
            value_name = "QUERY_OR_TRAIT_ARGS",
            allow_hyphen_values = true,
            last = true
        )]
        query: Vec<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run a trait end to end through configured harnesses; --no-drive
    /// starts a resumable session without driving it.
    Run {
        #[command(flatten)]
        args: Box<SessionStartArgs>,

        /// Start the session without driving it: persist the resumable
        /// ledger and exit. Advance later via the session/frame verbs or MCP.
        #[arg(long = "no-drive")]
        no_drive: bool,

        /// With --no-drive: do not persist to the default session store
        /// unless --out is supplied.
        #[arg(long)]
        ephemeral: bool,
    },
    /// Runtime-owned run sessions.
    #[command(hide = true)]
    Session {
        #[command(subcommand)]
        subcommand: SessionCommand,
    },
    /// Serve ctx.traits run-session MCP tools over line-delimited JSON-RPC stdio.
    #[command(hide = true)]
    Mcp,
    /// Drive assigned agent frames through approved CLI harnesses.
    #[command(hide = true)]
    Drive {
        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger.
        #[arg(long)]
        file: Option<String>,

        /// Run-session ID or ledger path to drive.
        #[arg(long)]
        session: String,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Override or synthesize a role assignment, role[.seat]=harness[:transport[:session-mode[:model[:reasoning-effort]]]].
        #[arg(
            long = "assign",
            value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL[:REASONING_EFFORT]]]]"
        )]
        assignments: Vec<String>,

        /// Maximum harness frames to submit in one drive invocation.
        #[arg(long = "max-frames")]
        max_frames: Option<u64>,

        /// Per-frame harness timeout in seconds.
        #[arg(long = "frame-seconds")]
        frame_seconds: Option<u64>,

        /// Total drive timeout in seconds.
        #[arg(long = "total-seconds")]
        total_seconds: Option<u64>,

        /// Correction retry budget per frame.
        #[arg(long = "max-retries")]
        max_retries: Option<u64>,

        /// Attach wait budget in seconds; defaults to total-seconds when omitted.
        #[arg(long = "attach-wait-seconds")]
        attach_wait_seconds: Option<u64>,

        /// Streaming idle timeout in seconds.
        #[arg(long = "idle-seconds")]
        idle_seconds: Option<u64>,

        /// Progress output mode. Defaults to `tui` when stdin, stdout, and
        /// stderr are all an interactive terminal; otherwise defaults to
        /// `status`. An explicit value always wins in both directions.
        #[arg(long, value_enum)]
        progress: Option<DriveProgress>,

        /// Force the current line/status progress path even when `--progress
        /// tui` is also supplied, or when the interactive default would
        /// otherwise select `tui`. Wins over `--progress tui` and over the
        /// interactive default; `--json` always wins over both.
        #[arg(long = "no-tui")]
        no_tui: bool,

        /// Execute in a dedicated `.ctx/worktrees/<id>` git worktree on branch
        /// `ctx/run/<id>` instead of the invocation checkout, creating or
        /// resuming it before driving. Bare flag derives the id from the
        /// run-session id; `--worktree=<name>` uses an explicit id. The
        /// worktree and branch are retained after every outcome.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "NAME", conflicts_with = "no_worktree")]
        worktree: Option<Option<String>>,

        /// Maximum number of independent `parallel`-panel branches or
        /// concurrent `for-each` items to dispatch concurrently. Defaults to
        /// `1`: every branch/item is driven one at a time exactly as before
        /// this flag existed — a value of `1` is a hard no-op with
        /// byte-identical behavior and creates no durable sidecars or
        /// conductor lease. Values above `1` opt into concurrently
        /// dispatching that many eligible siblings' harness calls at once,
        /// with a durable per-unit sidecar for each; ledger writes still
        /// apply strictly in authored order.
        #[arg(
            long = "max-in-flight",
            value_parser = parse_positive_max_in_flight
        )]
        max_in_flight: Option<usize>,

        /// Block for this session's P402 conductor lease within the total
        /// drive budget when another process already holds it, instead of
        /// immediately returning the typed `concurrency-conductor-busy`
        /// outcome. Only meaningful when `--max-in-flight` > 1 or this
        /// session already has durable concurrent state from a prior
        /// conductor.
        #[arg(long, conflicts_with = "no_wait")]
        wait: bool,

        /// Override `[drive].wait` and fail fast for a busy conductor lease.
        #[arg(long = "no-wait", conflicts_with = "wait")]
        no_wait: bool,

        /// Override `[worktree].enabled` and run in the invocation checkout.
        #[arg(long = "no-worktree", conflicts_with = "worktree")]
        no_worktree: bool,

        /// Clear a persisted P460 merge intent before resuming, so this
        /// drive skips automatic landing merge work even though the original
        /// `run`/`session start` requested `--merge`.
        #[arg(long = "no-merge")]
        no_merge: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Land a completed `--worktree` run. A true clean fast-forward (`main`
    /// is the run branch's merge base) lands deterministically without
    /// resolving, probing, or dispatching the standing merger agent.
    /// Otherwise (a divergent history, or `--force-merger`), the branch is
    /// rebased onto clean main and reconciled via the standing merger agent
    /// (`--deep` selects a judgment-capable merger instead) before
    /// fast-forwarding. Every landing path then runs the declared `[merge]
    /// gate` — an ordered list of repository commands configured in
    /// `.ctx/traits/runtime.toml`, empty by default — before touching `main`; merge
    /// machinery executes only what a repository declares and judges its
    /// outcome, it never inspects the repository for a Justfile or any other
    /// tool. Any failed precondition, unresolved conflict, judgment call, red
    /// gate, or lost fast-forward race parks the run with its branch and
    /// worktree intact. A cross-process lock serializes merges: by default
    /// this queues behind a concurrent merge up to a bounded wait;
    /// `--no-wait` fails fast instead.
    Merge {
        /// Internal run-id (as recorded in the run-session ledger, not the
        /// worktree id) of the completed `--worktree` run to land.
        run_id: String,

        /// Run-session store directory to resolve the run-id from. Defaults
        /// to this repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Override or synthesize the merger assignment, merger=harness[:transport[:session-mode[:model[:reasoning-effort]]]].
        #[arg(
            long = "assign",
            value_name = "ROLE=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL[:REASONING_EFFORT]]]]"
        )]
        assignments: Vec<String>,

        /// Fail immediately with a typed `lock-unavailable` result if another
        /// merge already holds the cross-process merge lock, instead of
        /// queueing behind it for the bounded default wait.
        #[arg(long, conflicts_with = "wait_override")]
        no_wait: bool,

        /// Explicitly override `[merge].wait` and queue behind the lock.
        #[arg(long = "wait", conflicts_with = "no_wait")]
        wait_override: bool,

        /// Send this merge through the standing merger agent's confirmation
        /// path even though `main` is a true clean fast-forward for the run
        /// branch. Supports operator inspection and makes result/ledger
        /// differential proofs deterministic without a production-only test
        /// hook; has no effect on a divergent history, which always goes
        /// through the merger regardless of this flag.
        #[arg(long)]
        force_merger: bool,

        /// Park on a detected stale-base overlap — the run branch and main
        /// both changed the same paths since the branch's base — instead of
        /// the default of landing anyway. Restores the strict prior
        /// behavior: the typed `stale-base-overlap` detail is recorded on the
        /// parked frame instead of on a landed one.
        #[arg(long, conflicts_with = "land_on_overlap")]
        park_on_overlap: bool,

        /// Explicitly override `[merge].overlap` and land on overlap.
        #[arg(long = "land-on-overlap", conflicts_with = "park_on_overlap")]
        land_on_overlap: bool,

        /// Deprecated no-op, kept for one release: landing despite a
        /// detected stale-base overlap is now the default (see
        /// `--park-on-overlap` to restore the old strict behavior). Passing
        /// this flag no longer changes behavior; it only emits a deprecation
        /// warning.
        #[arg(long, hide = true)]
        allow_stale_overlap: bool,

        /// Reconcile a divergent history through a judgment-capable deep
        /// merger (`[agent.role.merger-deep]`, falling back to
        /// `[agent.role.merger]`) instead of the standard mechanical-only
        /// merger. The deep merger
        /// resolves conflicts under the five P420 phase doctrines, may make
        /// logged supporting edits outside conflicted files, and refuses a
        /// landed-fix regression (parking with the refusing rule named)
        /// rather than guessing. Has no effect on a true clean fast-forward
        /// unless combined with `--force-merger`.
        #[arg(long)]
        deep: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Submit caller output for the current run-session frame.
    #[command(hide = true)]
    Call {
        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger and verifies digests.
        #[arg(long)]
        file: Option<String>,

        /// Run-session ID or ledger path. Bare IDs resolve through --session-store or `.ctx/runs`.
        #[arg(long)]
        session: String,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// JSON call submission payload.
        #[arg(long)]
        data: String,

        /// Write the updated run-session ledger JSON. Defaults to --session when --session is a path.
        #[arg(long)]
        out: Option<String>,

        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect a run-session without advancing it.
    #[command(hide = true)]
    RunStatus {
        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger and verifies digests.
        #[arg(long)]
        file: Option<String>,

        /// Run-session ledger JSON path.
        #[arg(long)]
        session: String,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the current run-session frame without advancing it.
    #[command(hide = true)]
    RunFrame {
        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger and verifies digests.
        #[arg(long)]
        file: Option<String>,

        /// Run-session ledger JSON path.
        #[arg(long)]
        session: String,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pull the next pending frame for an attached agent role.
    ///
    /// `--agent` is required for a store-scoped pull (no `--session`); a
    /// session-scoped pull (`--session` given) may omit `--agent` and the
    /// role is inferred from the session's persisted frame assignment
    /// (P421). `--session` also accepts an unambiguous prefix of a
    /// persisted session ID.
    #[command(hide = true)]
    Next {
        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Optional run-session ID (or an unambiguous prefix of one) or
        /// ledger path to scope the pull. Required to omit `--agent`.
        #[arg(long)]
        session: Option<String>,

        /// Run-session store directory for store-scoped pulls. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Bounded wait in seconds; defaults to immediate.
        #[arg(long = "wait-seconds", default_value_t = 0)]
        wait_seconds: u64,

        /// List pre-filter candidates without recomputing or fetching a frame.
        #[arg(long)]
        peek: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Submit a simple target/value update for the current run-session frame.
    #[command(hide = true)]
    Set {
        /// Run-session ID or ledger path. May also be supplied as `ctx traits --session <id> set ...`.
        #[arg(long)]
        session: Option<String>,

        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger.
        #[arg(long)]
        file: Option<String>,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Target sequence item ID, slot ID/ref, output port ID/ref, or awaiting input port ID/ref.
        target: String,

        /// Text value for schema:text targets.
        value: String,

        /// Interpret value as JSON instead of a text string.
        #[arg(long = "value-json")]
        value_json: bool,

        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record this machine's trust verdict for a trait's canonical digest:
    /// --approve marks it verified; --deny marks it blocked. Approval is of
    /// a digest, never a name — a later canonical edit is unreviewed again.
    ///
    /// Hidden as of P419: superseded by `ctx traits trust approve`/`trust
    /// block`, which this command now routes through unchanged. Kept
    /// invocable, undocumented, for one release.
    #[command(hide = true)]
    Review {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to review.
        #[arg(long)]
        file: Option<String>,

        /// Mark this machine's trust verdict for the trait's canonical digest as verified.
        #[arg(long, visible_alias = "accept")]
        approve: bool,

        /// Mark this machine's trust verdict for the trait's canonical digest as blocked.
        #[arg(long)]
        deny: bool,

        /// Optional reason recorded with a --deny decline.
        #[arg(long)]
        reason: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Activate a trait for resolver eligibility (lifecycle transition).
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Activate {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to activate.
        #[arg(long)]
        file: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deactivate a trait, removing resolver eligibility (lifecycle transition).
    #[command(hide = true)]
    Deactivate {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to deactivate.
        #[arg(long)]
        file: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deprecate a trait with a reason (lifecycle transition).
    #[command(hide = true)]
    Deprecate {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to deprecate.
        #[arg(long)]
        file: Option<String>,

        /// Reason for deprecation.
        #[arg(long)]
        reason: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run deterministic declared evals and optionally update lock evidence.
    #[command(hide = true)]
    Eval {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to evaluate.
        #[arg(long)]
        file: Option<String>,

        /// Eval ID to run. Repeat for multiple evals.
        #[arg(long = "eval", value_name = "EVAL_ID")]
        eval_ids: Vec<String>,

        /// Eval variant filter: documentation, lint, golden-render, behavioral, or runtime.
        #[arg(long)]
        variant: Option<String>,

        /// Write generated eval report JSON.
        #[arg(long)]
        out: Option<String>,

        /// Update the package-local trait.lock with passing deterministic eval-result evidence.
        #[arg(long)]
        update_lock: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print a trait's compiled prompt text for an agent to read directly.
    /// No session/frame is created or advanced.
    ///
    /// Formerly `use`, kept as a hidden alias for one release.
    ///
    /// P497: refuses on a trust-blocked trait (no escape) or an unreviewed
    /// trait (pass `--allow-unreviewed`); a draft trait always renders, with
    /// a visible advisory.
    #[command(hide = true, alias = "use")]
    Prompt {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Render an unreviewed trait anyway (default: refuse).
        #[arg(long)]
        allow_unreviewed: bool,

        /// Model-view projection to print.
        #[arg(long, value_enum, default_value_t = PromptLevel::Full)]
        level: PromptLevel,

        /// Emit structured JSON with text and digest evidence.
        #[arg(long)]
        json: bool,
    },
    /// Check a trait for validation, audit, and drift.
    Check {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to check.
        #[arg(long)]
        file: Option<String>,

        /// Compare against lock data.
        #[arg(long)]
        locked: bool,

        /// Skip CDK source drift verification against the `.ctx` source tree.
        #[arg(long)]
        skip_cdk_drift: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Emit the byte-stable plain text report without ANSI styling or animation.
        #[arg(long)]
        plain: bool,

        /// Alias for --plain; disables the terminal decode animation and styling.
        #[arg(long = "no-animate")]
        no_animate: bool,

        /// List every flagged advisory instead of collapsing the tail into a summary line.
        #[arg(long)]
        verbose: bool,

        /// Explicit run ledger JSON to include as runtime evidence.
        #[arg(long)]
        run_ledger: Option<String>,

        /// Explicit eval report JSON to include as eval evidence. Repeat for multiple reports.
        #[arg(long = "eval-report", value_name = "REPORT_JSON")]
        eval_reports: Vec<String>,
    },
    /// Show layer-aware diff for a trait.
    Diff {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to diff.
        #[arg(long)]
        file: Option<String>,

        /// Compare against lock data.
        #[arg(long)]
        from_lock: bool,

        /// Show model-view layer.
        #[arg(long)]
        model_view: bool,

        /// Show export layer.
        #[arg(long)]
        exports: bool,

        /// Show resource and policy manifest layers.
        #[arg(long)]
        resources: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,

        /// Show every diff entry's full hunk detail instead of the compact summary.
        #[arg(long)]
        verbose: bool,
    },
    /// Read-only mirror of a trait's prompt frames: no harness dispatch, no
    /// session start/advance, no runtime state written.
    #[command(hide = true)]
    Preview {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to preview.
        #[arg(long)]
        file: Option<String>,

        /// Preview only this step (item id, or its declaration/run index).
        #[arg(long)]
        step: Option<String>,

        /// Existing run session id or path. Inlines values accepted before
        /// the current frame; without it, slot/port inputs print as pending.
        #[arg(long)]
        session: Option<String>,

        /// Run-session store directory for bare session IDs. Defaults to
        /// `.ctx/runs` relative to the current directory.
        #[arg(long)]
        session_store: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Export a trait to a compatibility profile directory.
    ///
    /// Formerly a distinct read-only `render` preview, `render` is now a
    /// hidden alias of `export`: both spellings write the same compatibility
    /// profile output.
    ///
    /// P497: refuses on a trust-blocked trait (no escape) or an unreviewed
    /// trait (pass `--allow-unreviewed`); a draft trait always exports, with
    /// a visible advisory.
    #[command(alias = "render", hide = true)]
    Export {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to export.
        #[arg(long)]
        file: Option<String>,

        /// Render/export profile. Accepted profiles are agent-skills, pi,
        /// opencode, claude-code, codex, copilot, and markdown-only. Default
        /// export directories exist only for agent-skills, pi, opencode,
        /// claude-code, and codex.
        #[arg(long, default_value = "agent-skills")]
        profile: String,

        /// Export format: compat preserves the existing profile renderer;
        /// skill emits a progressive-disclosure SKILL.md directory (plus any
        /// placeable declared resources as companion files); agents emits
        /// the same flat compat body to AGENTS.md; stub emits a body-free
        /// SKILL.md that runs `ctx traits prompt <id>`.
        #[arg(long, default_value = "compat")]
        format: String,

        /// Output directory override. Export writes `<out>/<trait-id>/SKILL.md`
        /// for compat/skill/stub formats, or `<out>/<trait-id>/AGENTS.md` for
        /// the agents format; required for copilot and markdown-only because
        /// P51 defines no default export directory for those profiles.
        #[arg(long)]
        out: Option<String>,

        /// Record static projection evidence in trait.lock after export.
        #[arg(long = "update-skill-lock")]
        update_skill_lock: bool,

        /// Add the generated static export path to .gitignore with safety guards.
        #[arg(long = "update-gitignore")]
        update_gitignore: bool,

        /// Export an unreviewed trait anyway (default: refuse).
        #[arg(long)]
        allow_unreviewed: bool,

        /// Emit structured JSON describing the write result instead of the plain report.
        #[arg(long)]
        json: bool,
    },
    /// Host-placement lifecycle: place, refresh, or remove exported traits
    /// on host tools' expected locations (see `ctx traits host --help`).
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Host {
        /// Emit structured JSON (applies to whichever subcommand is given).
        #[arg(long)]
        json: bool,

        #[command(subcommand)]
        subcommand: HostCommand,
    },
    /// Search traits by lexical query. Discovery only, not activation.
    #[command(hide = true)]
    Search {
        /// Search query text.
        query: String,

        /// Repository root to scan for trait packages. Defaults to current
        /// directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Produce a budgeted activation plan for loaded traits.
    #[command(hide = true)]
    Resolve {
        /// User task text to evaluate.
        #[arg(long)]
        task: String,

        /// Canonical trait file to load. Repeat for multiple traits. If
        /// omitted, resolves from default repo inventory.
        #[arg(long = "file", value_name = "TRAIT_FILE")]
        trait_files: Vec<String>,

        /// Repository root to scan for trait packages when no --file is
        /// supplied. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Task/source file paths used by activation matching.
        #[arg(long = "files", value_name = "FILE")]
        files: Vec<String>,

        /// Optional task mode hint.
        #[arg(long)]
        mode: Option<String>,

        /// Language hint. Repeat for multiple languages.
        #[arg(long = "language", value_name = "LANGUAGE")]
        languages: Vec<String>,

        /// Token budget for context selection.
        #[arg(long)]
        budget: Option<u64>,

        /// Session hint for resolve planning.
        #[arg(long)]
        session: Option<String>,

        /// Direct invocation evidence text.
        #[arg(long = "explicit-invocation", value_name = "TEXT")]
        explicit_invocation: Option<String>,

        /// Optional trait ID filter.
        #[arg(long)]
        trait_id: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Plan a context pack from loaded traits under a token budget.
    #[command(hide = true)]
    Pack {
        /// User task text to evaluate.
        #[arg(long)]
        task: String,

        /// Canonical trait file to load. Repeat for multiple traits. If
        /// omitted, uses default repo inventory.
        #[arg(long = "file", value_name = "TRAIT_FILE")]
        trait_files: Vec<String>,

        /// Repository root to scan for trait packages when no --file is
        /// supplied. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Render profile for context frames.
        #[arg(long, default_value = "agent-skills")]
        profile: String,

        /// Session ID for the context pack.
        #[arg(long)]
        session: Option<String>,

        /// Token budget for context selection.
        #[arg(long)]
        budget: Option<u64>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Context ledger operations.
    #[command(hide = true)]
    Context {
        #[command(subcommand)]
        subcommand: ContextCommand,
    },
    /// Hook adapter (P499/P501): reads a hook payload as JSON on stdin and
    /// writes `{"hookSpecificOutput":{...}}` on stdout, or prints the
    /// host's config snippet with `--settings`. Ships hidden pending P502's
    /// install story (D8).
    #[command(hide = true)]
    Hook {
        /// Which harness's payload/config shape to speak. Defaults to
        /// claude-code for back-compat with the settings block P499 already
        /// tells people to install.
        #[arg(long, value_enum, default_value_t = HookHost::ClaudeCode)]
        host: HookHost,

        /// Print the host's hooks config snippet instead of handling a
        /// payload on stdin.
        #[arg(long)]
        settings: bool,
    },
    /// TypeScript config authoring commands (P457).
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Config {
        /// Emit structured JSON. Applies to whichever config subcommand is
        /// given; equivalent to that subcommand's own `--json`.
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        subcommand: ConfigCommand,
    },
    /// Cache lifecycle commands.
    Cache {
        /// Emit structured JSON. Applies to whichever cache subcommand is
        /// given; equivalent to that subcommand's own `--json`.
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        subcommand: CacheCommand,
    },
    /// Task board document commands.
    ///
    /// Off the release help screen since the 2026-08-18 regroup; runnable
    /// as before.
    #[command(hide = true)]
    Task {
        /// Emit structured JSON. Applies to whichever task subcommand is
        /// given; equivalent to that subcommand's own `--json`.
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        subcommand: TaskCommand,
    },
    /// Emit the full parser-derived command surface (visible and hidden
    /// commands, groups, one-line descriptions, aliases, flags, and nested
    /// subcommands) as JSON, generated from Clap's own command tree rather
    /// than a hand-copied inventory.
    #[command(hide = true)]
    Help {
        /// Emit structured JSON. `help` currently supports only `--json`;
        /// bare `ctx traits help` behaves like `-h`.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PromptLevel {
    Full,
    Summary,
}

/// `ctx traits host` subcommands (P441 host-placement lifecycle). Folded
/// into one visible namespace row — like `trust` and `cache` — so the
/// release help keeps its one-screen ceiling while every placement
/// operation stays discoverable.
#[derive(Subcommand, Debug)]
pub enum HostCommand {
    /// Install a trait onto a host's expected project or user-level location.
    ///
    /// P497: refuses on a trust-blocked trait (no escape), an unreviewed
    /// trait (pass `--allow-unreviewed`), or a draft trait (pass
    /// `--allow-draft`).
    Install {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Trait file to install.
        #[arg(long)]
        file: Option<String>,

        /// Target host: cursor, copilot, gemini, cline, kiro, claude-code,
        /// opencode, codex, pi, or a host fully specified in
        /// `.ctx/traits/runtime.toml [host.<name>]`.
        #[arg(long)]
        host: String,

        /// Place at the host's user-level location using the global
        /// placement manifest, instead of the project location.
        #[arg(long)]
        global: bool,

        /// Export format override: `stub` (default for Agent Skills-shaped
        /// hosts) writes a body-free projection that runs
        /// `ctx traits prompt <id>`; `skill` writes the fully rendered
        /// directory (SKILL.md plus placed resource files). Ignored for
        /// hosts whose default format is not stub/skill (cursor, copilot,
        /// gemini, cline, kiro).
        #[arg(long)]
        format: Option<String>,

        /// Also write a zip archive containing the exact placed artifact(s).
        #[arg(long)]
        archive: Option<String>,

        /// Install an unreviewed trait anyway (default: refuse).
        #[arg(long)]
        allow_unreviewed: bool,

        /// Install a draft trait anyway (default: refuse).
        #[arg(long)]
        allow_draft: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Rebuild every recorded host placement in a manifest from its
    /// original source, writing only entries whose bytes changed.
    ///
    /// P497: applies the same lifecycle/trust gate as `install`, with no
    /// escape flags; a placement whose source has since gone
    /// blocked/unreviewed/draft is reported as that entry's error and its
    /// placed bytes are left untouched. A recorded path that was locally
    /// modified since it was placed is also left untouched and reported as
    /// `locally-modified`, unless `--force` is passed.
    Update {
        /// Update the global placement manifest instead of the project one.
        #[arg(long)]
        global: bool,

        /// Overwrite a locally modified placed path anyway (default: skip
        /// it and report `locally-modified`).
        #[arg(long)]
        force: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report drift/ownership state for every recorded placement without
    /// mutating anything: `current`, `stale-source` (a fresh render would
    /// change the bytes), `locally-modified` (a human edited a placed
    /// file), `missing`, `unmanaged`/`ownership-mismatch`, or `error`.
    /// `host update` is the verb that fixes what this names.
    Status {
        /// Report on the global placement manifest instead of the project one.
        #[arg(long)]
        global: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove one recorded host placement, refusing to delete a missing,
    /// unmanaged, symlinked, or locally modified artifact.
    Remove {
        /// Trait ID as recorded in the placement manifest.
        #[arg(value_name = "TRAIT")]
        trait_id: String,

        /// Host the placement was installed under.
        #[arg(long)]
        host: String,

        /// Remove from the global placement manifest instead of the project one.
        #[arg(long)]
        global: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TrustCommand {
    /// Resolve `operand`'s current canonical digest through the trait
    /// resolver and mark it locally verified. When `operand` does not
    /// resolve as a trait, falls back to the installed-package resolver
    /// (project scope first, then global) and atomically marks every one of
    /// that package's *current* trait canonical digests as locally
    /// verified — trait resolution always wins when both namespaces
    /// contain the same operand. Per-digest records remain the storage
    /// unit underneath, so a later canonical edit to any one trait reverts
    /// only that digest to unreviewed.
    Approve {
        /// Trait name/path, or installed npm package name/manifest alias.
        #[arg(required_unless_present_any = ["digest", "all_current"])]
        operand: Option<String>,

        /// Raw digest to approve, bypassing trait/package resolution (for scripts).
        #[arg(long, conflicts_with = "operand", value_name = "sha256:...")]
        digest: Option<String>,

        /// Approve every currently resolved trait atomically.
        #[arg(long, conflicts_with_all = ["operand", "digest"])]
        all_current: bool,

        /// Optional reviewer note.
        #[arg(long)]
        reason: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve `operand`'s current canonical digest through the trait
    /// resolver and mark it locally blocked.
    Block {
        /// Trait name/path to block.
        #[arg(required_unless_present = "digest")]
        operand: Option<String>,

        /// Raw digest to block, bypassing trait resolution (for scripts).
        #[arg(long, conflicts_with = "operand", value_name = "sha256:...")]
        digest: Option<String>,

        /// Optional block reason.
        #[arg(long)]
        reason: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report every named trust decision recorded on this machine.
    List {
        /// Report only stale approvals: recorded digest differs from the
        /// same trait's current canonical digest.
        #[arg(long)]
        stale: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `ctx traits session ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// Start a run session and conduct it through configured harnesses.
    Start(Box<SessionStartArgs>),
    /// Inspect a run session without advancing it.
    State(Box<SessionStateArgs>),
    /// Current frame operations.
    Frame {
        #[command(subcommand)]
        subcommand: SessionFrameCommand,
    },
}

#[derive(Args, Debug)]
pub struct SessionStartArgs {
    /// Trait ID to resolve from the repo-local trait source root.
    pub trait_id: Option<String>,

    /// Trait file to run.
    #[arg(long)]
    pub file: Option<String>,

    /// Removed (P476): use `--assign default=<harness>[:transport[:session-mode[:model[:reasoning-effort]]]]`
    /// instead. Kept only as a hidden flag so a caller still passing
    /// `--master` gets a message naming the replacement instead of clap's
    /// generic "unexpected argument" error.
    #[arg(long, hide = true)]
    pub master: Option<String>,

    /// JSON file containing initial port values.
    #[arg(long)]
    pub input: Option<String>,

    /// Set an initial input port value, e.g. --set code-diff=...
    #[arg(long = "set", value_name = "PORT=VALUE")]
    pub sets: Vec<String>,

    /// Run-session store directory for bare session IDs. Defaults to this
    /// repository's global per-repository runs root.
    #[arg(long)]
    pub session_store: Option<String>,

    /// Override or synthesize a role assignment, role[.seat]=harness[:transport[:session-mode[:model[:reasoning-effort]]]].
    #[arg(
        long = "assign",
        value_name = "ROLE[.SEAT]=HARNESS[:TRANSPORT[:SESSION_MODE[:MODEL[:REASONING_EFFORT]]]]"
    )]
    pub assignments: Vec<String>,

    /// Optional resource root used to read declared resource digest evidence.
    #[arg(long)]
    pub resource_root: Option<String>,

    /// Write the resumable run-session ledger JSON.
    #[arg(long)]
    pub out: Option<String>,

    /// Maximum harness frames to submit in one invocation.
    #[arg(long = "max-frames")]
    pub max_frames: Option<u64>,

    /// Per-frame harness timeout in seconds.
    #[arg(long = "frame-seconds")]
    pub frame_seconds: Option<u64>,

    /// Total conductor timeout in seconds.
    #[arg(long = "total-seconds")]
    pub total_seconds: Option<u64>,

    /// Correction retry budget per frame.
    #[arg(long = "max-retries")]
    pub max_retries: Option<u64>,

    /// Attach wait budget in seconds; defaults to total-seconds when omitted.
    #[arg(long = "attach-wait-seconds")]
    pub attach_wait_seconds: Option<u64>,

    /// Streaming idle timeout in seconds.
    #[arg(long = "idle-seconds")]
    pub idle_seconds: Option<u64>,

    /// Maximum number of independent `parallel`-panel branches or concurrent
    /// `for-each` items to dispatch concurrently. Defaults to `1`: every
    /// branch/item is driven one at a time exactly as before this flag
    /// existed — a value of `1` is a hard no-op with byte-identical
    /// behavior and creates no durable sidecars or conductor lease. Values
    /// above `1` opt into concurrently dispatching that many eligible
    /// siblings' harness calls at once, with a durable per-unit sidecar for
    /// each; ledger writes still apply strictly in authored order.
    #[arg(
        long = "max-in-flight",
        value_parser = parse_positive_max_in_flight
    )]
    pub max_in_flight: Option<usize>,

    /// Block for this session's P402 conductor lease within the total drive
    /// budget when another process already holds it, instead of immediately
    /// returning the typed `concurrency-conductor-busy` outcome. Only
    /// meaningful when `--max-in-flight` > 1 or this session already has
    /// durable concurrent state from a prior conductor.
    #[arg(long, conflicts_with = "no_wait")]
    pub wait: bool,

    /// Override `[drive].wait` and fail fast for a busy conductor lease.
    #[arg(long = "no-wait", conflicts_with = "wait")]
    pub no_wait: bool,

    /// Progress output mode. Defaults to `tui` when stdin, stdout, and
    /// stderr are all an interactive terminal; otherwise defaults to
    /// `status`. An explicit value always wins in both directions.
    #[arg(long, value_enum)]
    pub progress: Option<DriveProgress>,

    /// Force the current line/status progress path even when `--progress
    /// tui` is also supplied, or when the interactive default would
    /// otherwise select `tui`. Wins over `--progress tui` and over the
    /// interactive default; `--json` always wins over both.
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    /// Execute in a dedicated `.ctx/worktrees/<id>` git worktree on branch
    /// `ctx/run/<id>` instead of the invocation checkout. Bare flag derives
    /// the id from the run-session id; `--worktree=<name>` uses an explicit
    /// id. The worktree and branch are retained after every outcome.
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "NAME", conflicts_with = "no_worktree")]
    pub worktree: Option<Option<String>>,

    /// Override `[worktree].enabled` and run in the invocation checkout.
    #[arg(long = "no-worktree", conflicts_with = "worktree")]
    pub no_worktree: bool,

    /// Request automatic landing merge work (`ctx traits merge`) once this run
    /// completes, requiring an effective `--worktree`. Bare `--merge` uses
    /// `[merge].deep` (falling back to standard); `--merge=standard` or
    /// `--merge=deep` pins the rung explicitly, overriding config. `[merge]
    /// auto = true` supplies this by default when neither flag is given.
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "RUNG", conflicts_with = "no_merge")]
    pub merge: Option<Option<MergeRung>>,

    /// Override `[merge].auto` and skip automatic landing merge work.
    #[arg(long = "no-merge", conflicts_with = "merge")]
    pub no_merge: bool,

    /// Fail the run when any loop uses its full iteration budget without
    /// meeting its exit condition (e.g. a review loop ending unapproved).
    /// Default: exhaustion is a normal outcome and the run continues to the
    /// step after the loop, which is responsible for reading what the loop
    /// actually produced. Overrides every loop's own `on-exhausted` policy,
    /// including a continuing loop's declared signals, which are not
    /// emitted.
    #[arg(long, conflicts_with = "no_strict_loops")]
    pub strict_loops: bool,

    /// Override `[drive].strict-loops` and allow normal loop exhaustion.
    #[arg(long = "no-strict-loops", conflicts_with = "strict_loops")]
    pub no_strict_loops: bool,

    /// Dispatch a task whose `depends-on` is unmet anyway. Recorded in the
    /// run's provenance (typed evidence plus a human-readable warning), not
    /// left silent.
    #[arg(long = "override-dependencies")]
    pub override_dependencies: bool,

    /// Bind the `task` input through the trait's declared `task-board`
    /// resource: the value must resolve to a live task on the board, the
    /// wall/closed-status/dependency preflights run, and the task file is
    /// materialised into the run's worktree. The `[tasks] dispatch-trait`
    /// flow sets this automatically; without it a `task` input is plain
    /// text like any other port value.
    #[arg(long = "task-dispatch")]
    pub task_dispatch: bool,

    /// Board-driven dispatch (0195): run a queue of tasks through `[tasks]
    /// dispatch-trait` instead of a single trait invocation. Each value is a
    /// bare/dotted task key, or a charter key (a task with children), which
    /// expands to its own non-closed children in dotted-ordinal order.
    /// Comma-separated values and repeated `--task` flags both normalize to
    /// one flattened queue, run sequentially in the order given. Conflicts
    /// with the positional trait/query and with `--task-dispatch`, which a
    /// resolved queue member sets on its own behalf.
    #[arg(
        long = "task",
        value_delimiter = ',',
        conflicts_with_all = ["task_dispatch", "trait_id", "file", "input", "sets", "trait_args"]
    )]
    pub task: Vec<String>,

    /// With `--task`: keep running the remaining queue after a failed run
    /// or a parked merge instead of halting, reporting every task's outcome
    /// in a table at the end.
    #[arg(long = "continue-on-failure", requires = "task")]
    pub continue_on_failure: bool,

    /// Open a scrollable run-story pane at termination, after the merge
    /// report when one runs. Bare `--story` renders the free `default`
    /// level; `--story=assisted` spends narrator model calls — the only
    /// level that does (degrades to `default` with a stated notice when no
    /// activity was recorded for this run, or no narrator seat resolves).
    /// Interactive-TTY only: no-op under `--json` or off a full TTY (the
    /// story still prints as plain text there). `[drive] story` supplies this
    /// by default when neither flag is given.
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "LEVEL", conflicts_with = "no_story")]
    pub story: Option<Option<String>>,

    /// Override `[drive].story` and never open the termination pane.
    #[arg(long = "no-story", conflicts_with = "story")]
    pub no_story: bool,

    /// Emit structured JSON.
    #[arg(long)]
    pub json: bool,

    /// Show the full drive/debug report at the end instead of the compact
    /// final-output line.
    #[arg(long)]
    pub verbose: bool,

    /// Trait arguments after `--`, e.g. --goal "add oauth login", or query text when no trait/file is supplied.
    #[arg(
        value_name = "TRAIT_ARGS_OR_QUERY",
        allow_hyphen_values = true,
        last = true
    )]
    pub trait_args: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SessionStateArgs {
    /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger and verifies digests.
    #[arg(long)]
    pub file: Option<String>,

    /// Run-session ledger JSON path or bare session ID.
    #[arg(long)]
    pub session: String,

    /// Run-session store directory for bare session IDs. Defaults to this
    /// repository's global per-repository runs root.
    #[arg(long)]
    pub session_store: Option<String>,

    /// Emit structured JSON.
    #[arg(long)]
    pub json: bool,
}

/// `ctx traits session frame ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum SessionFrameCommand {
    /// Print the current run-session frame without advancing it.
    State {
        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger and verifies digests.
        #[arg(long)]
        file: Option<String>,

        /// Run-session ledger JSON path or bare session ID.
        #[arg(long)]
        session: String,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set one output value for the current frame.
    Set {
        /// Run-session ledger JSON path or bare session ID.
        #[arg(long)]
        session: String,

        /// Optional trait file override. If omitted, loads the trait file recorded in the run-session ledger.
        #[arg(long)]
        file: Option<String>,

        /// Run-session store directory for bare session IDs. Defaults to this
        /// repository's global per-repository runs root.
        #[arg(long)]
        session_store: Option<String>,

        /// Target sequence item ID, slot ID/ref, output port ID/ref, or awaiting input port ID/ref.
        #[arg(long)]
        key: String,

        /// Text value for schema:text targets.
        #[arg(long)]
        value: String,

        /// Interpret value as JSON instead of a text string.
        #[arg(long = "value-json")]
        value_json: bool,

        #[arg(long, value_name = "ROLE", help = AGENT_ROLE_HELP)]
        agent: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum DriveProgress {
    None,
    Status,
    Stream,
    Tui,
}

/// P460 `--merge[=standard|deep]` explicit rung value. CLI-local (clap's
/// `ValueEnum` cannot be implemented for `ctx_traits_core`'s persisted
/// `MergeRung` from this crate) — command dispatch converts one to the
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum MergeRung {
    Standard,
    Deep,
}

/// `ctx traits context ...` subcommands (P498). Machine/adapter surface for
/// harness hooks and plugins (P499/P500/P501); `context` itself stays
/// `#[command(hide = true)]` until an adapter phase decides to surface it.
#[derive(Subcommand, Debug)]
pub enum ContextCommand {
    /// Report known context ledger entries and stale reasons for a host key.
    Status {
        /// Harness identifier (e.g. `claude-code`), half of the host key.
        #[arg(long)]
        host: String,

        /// Host-reported session id, half of the host key.
        #[arg(long = "host-session")]
        host_session: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Decide `inject` / `skip-fresh` / `reinject` per selected trait for a
    /// host key, rendering each selected trait through the same path
    /// `ctx traits prompt` uses so the ledger's freshness claim always
    /// matches what an adapter would actually inject.
    Plan {
        /// Harness identifier (e.g. `claude-code`), half of the host key.
        #[arg(long)]
        host: String,

        /// Host-reported session id, half of the host key.
        #[arg(long = "host-session")]
        host_session: String,

        /// User task text to evaluate.
        #[arg(long)]
        task: String,

        /// Canonical trait file to load. Repeat for multiple traits. If
        /// omitted, resolves from default repo inventory.
        #[arg(long = "file", value_name = "TRAIT_FILE")]
        trait_files: Vec<String>,

        /// Repository root to scan for trait packages when no --file is
        /// supplied. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Task/source file paths used by activation matching.
        #[arg(long = "files", value_name = "FILE")]
        files: Vec<String>,

        /// Optional task mode hint.
        #[arg(long)]
        mode: Option<String>,

        /// Language hint. Repeat for multiple languages.
        #[arg(long = "language", value_name = "LANGUAGE")]
        languages: Vec<String>,

        /// Token budget for context selection.
        #[arg(long)]
        budget: Option<u64>,

        /// Persist this plan's decisions to the ledger (optimistic commit:
        /// there is no post-injection callback from any harness, so this
        /// records intent, not confirmed delivery).
        #[arg(long)]
        commit: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Drop every ledger entry for a host key (the `SessionStart` edge:
    /// compact, clear, or fork all mean the context that ledger described no
    /// longer exists).
    Clear {
        /// Harness identifier (e.g. `claude-code`), half of the host key.
        #[arg(long)]
        host: String,

        /// Host-reported session id, half of the host key.
        #[arg(long = "host-session")]
        host_session: String,

        /// Why the session's context is being cleared.
        #[arg(long, value_enum)]
        reason: ContextClearReason,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `context clear --reason` values (P498 decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ContextClearReason {
    Compact,
    Clear,
    Fork,
    /// P499: a fresh claude-code `SessionStart` with `source ∈ {startup,
    /// clear, fork}` clears the ledger through this reason rather than
    /// mapping onto `Clear`, so the ledger's own evidence names what the
    /// harness actually reported (D5).
    Startup,
}

impl ContextClearReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Clear => "clear",
            Self::Fork => "fork",
            Self::Startup => "startup",
        }
    }
}

/// `ctx traits hook --host` (P501): named `--host` rather than `--harness`
/// to match `ctx traits context plan/clear --host`, the other half of the
/// same host key. A two-value enum (not a free string) buys validation and
/// an exhaustive match in the snippet emitter — a typo'd harness id would
/// otherwise silently mint a separate ledger namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum HookHost {
    ClaudeCode,
    Codex,
}

impl HookHost {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

/// `ctx traits dependency ...` subcommands (P567).
///
/// One group for every operation on a package this project DEPENDS ON,
/// named the way the npm equivalents are, because that is the mental model
/// they already follow: `install` takes everything declared in the manifest,
/// `add` takes exactly one new package. The previous surface split those two
/// across `vendor` and `install`, which read as unrelated commands and sent
/// people to a dependency verb for work that had nothing to do with
/// dependencies.
///
/// `publish` deliberately stays top-level: publishing is what you do to a
/// package you OWN, not to one you depend on.
#[derive(Subcommand, Debug)]
pub enum DependencyCommand {
    /// Resolve and install every declared trait dependency (`npm install`
    /// with no arguments).
    ///
    /// npm sources (both package-local `source.package` dependencies and
    /// project `[dependencies]`) are fetched, integrity-verified, and
    /// extracted entirely in Rust from the registry into this repo's cache;
    /// no `node`, `npm`, or `pnpm` process is ever invoked on this path (P438).
    Install {
        /// Trait name (resolved from .ctx/traits, falling back to a built-in meta-trait) or explicit file path.
        /// Omitted: install every package under .ctx/traits.
        #[arg(value_name = "TRAIT")]
        trait_arg: Option<String>,

        /// Optional project manifest path. Defaults to shallow discovery in the current directory.
        #[arg(long)]
        manifest: Option<String>,

        /// Trait file whose package-local lock is read or written.
        #[arg(long)]
        file: Option<String>,

        /// Verify the vendor tree and lock evidence without writing.
        #[arg(long)]
        locked: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add one npm-transport trait package to this project (`npm install
    /// <pkg>`).
    ///
    /// Fetches metadata and the tarball entirely in Rust (no node/npm/pnpm on
    /// the consume path), verifies npm SHA-512 SRI before extraction, and
    /// stages every check (integrity, schema version, optional `ctx.digests`
    /// publisher claim) before touching `.ctx/traits/config.toml`, `.ctx/traits/config.lock`,
    /// or the vendor tree. `path:<relative-path>` (P535) is also accepted:
    /// project-scoped only (refused with `-g`/`--global`), it copies a
    /// sibling repository's committed trait package through the same safe
    /// staging/publication transaction and records source/tree/per-trait
    /// digest evidence instead of npm registry evidence — a source rebuild
    /// never propagates during ordinary reconciliation, only an explicit
    /// `ctx traits dependency update <alias>` accepts new source bytes.
    /// A git-transport spec (task 0191) is also accepted: `owner/repo/trait[@ref]`
    /// shorthand, a bare `owner/repo`/full URL collection combined with
    /// repeatable `--trait <id>` or `--all`, or the explicit
    /// `git+<url>#ref=...&path=...` round-trip form. GitHub codeload is
    /// fetched entirely in Rust (no `git`/`node` on this path); a bare
    /// collection spec, or a trait name not found in one, prints the
    /// collection's contents with copyable add commands instead of a bare
    /// not-found error. Never writes trust state: run `ctx traits trust
    /// approve <trait>` for each printed canonical digest before running an
    /// installed trait.
    Add {
        /// npm package spec (`name`, `@scope/name`, `name@<range-or-tag>`,
        /// etc.), `path:<relative-path>` (P535, project-scoped only), or a
        /// git-transport spec (task 0191).
        spec: String,

        /// Vendor directory / manifest alias. Defaults to the npm basename,
        /// or the path's final named component for `path:`/git. Only valid
        /// with at most one `--trait`.
        #[arg(long)]
        alias: Option<String>,

        /// Add to the per-machine global tier
        /// (`~/.config/ctx/traits.toml`) instead of this project's
        /// `.ctx/traits/config.toml`. A global trait resolves in every project (and
        /// outside any repository) whenever no nearer-tier trait shadows it.
        #[arg(short = 'g', long = "global")]
        global: bool,

        /// Select a trait by id from a bare git collection spec (repeatable).
        /// Conflicts with a spec that already names a trait
        /// (`owner/repo/trait`) and with `--all`.
        #[arg(long = "trait", value_name = "ID")]
        trait_ids: Vec<String>,

        /// Add every trait found in a git collection spec instead of one.
        /// Conflicts with `--trait` and a spec that already names a trait.
        #[arg(long)]
        all: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove a project-installed npm package: manifest entry, project-lock
    /// entry, and vendor directory.
    Remove {
        /// npm package name (e.g. `@scope/name`), `path:<relative-path>`, or
        /// manifest alias of the installed dependency.
        package: String,

        /// Remove from the per-machine global tier instead of this project.
        #[arg(short = 'g', long = "global")]
        global: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Re-resolve one (or, with no argument, every) project dependency's
    /// manifest selector and replace its lock/vendor evidence. For a `path:`
    /// dependency (P535) this is the only operation that accepts changed
    /// source bytes: ordinary `dependency install`/sync always reproduces
    /// the exact locked snapshot and refuses when the current source has
    /// moved on.
    Update {
        /// npm package name (e.g. `@scope/name`), `path:<relative-path>`, or
        /// manifest alias to update. Omitted: update every project
        /// dependency.
        package: Option<String>,

        /// Update the per-machine global tier instead of this project.
        #[arg(short = 'g', long = "global")]
        global: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report locked, wanted, and registry-latest versions for every project
    /// dependency.
    Outdated {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect an npm package's or `path:<relative-path>` source's (P535)
    /// metadata, publisher claims (npm only), canonical digests, command
    /// argv, resource roots, and agent roles without modifying the
    /// manifest, lock, vendor tree, or trust store. Downloaded npm bytes
    /// only ever land in the registry cache; a `path:` source is staged
    /// into a private temporary copy and never written to the vendor tree.
    Info {
        /// npm package spec (`name`, `@scope/name`, `name@<range-or-tag>`,
        /// etc.) or `path:<relative-path>` (P535, project-scoped only).
        spec: String,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Prepare a trait package for publication: declare its npm name,
    /// registry, and access, and write the npm wrapper beside `trait.toml`.
    ///
    /// Run this once per package before the first `dependency publish`.
    /// Without it a package has no publishable identity — the npm name would
    /// otherwise be derived as `@ctx-traits/<id>`, a scope only this project
    /// can publish to.
    Init {
        /// Trait package directory. Defaults to the current directory.
        path: Option<String>,

        /// npm package name, scope included (for example `@acme/review`).
        /// Defaults to the existing `[publish] name`, then to
        /// `@ctx-traits/<id>`.
        #[arg(long)]
        name: Option<String>,

        /// Registry URL to publish to. Omit for npm's default.
        #[arg(long)]
        registry: Option<String>,

        /// npm access for a scoped package: `public` or `restricted`. npm
        /// defaults a NEW scoped package to restricted, which is how a first
        /// publish silently lands private.
        #[arg(long)]
        access: Option<String>,

        /// Overwrite an existing `[publish]` declaration or `package.json`.
        #[arg(long)]
        force: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },

    /// Publish exactly one ready trait package to npm, without changing
    /// source files (`npm publish`).
    ///
    /// The producing half of the same dependency relationship the other
    /// subcommands consume: this is how a package BECOMES something another
    /// project can `dependency add`.
    Publish {
        /// Package or repository path. Defaults to the current directory.
        path: Option<String>,

        /// Trait ID resolved from the local trait inventory.
        #[arg(long = "trait", value_name = "TRAIT_ID", conflicts_with = "path")]
        trait_id: Option<String>,

        /// Inspect and report the immutable payload without publishing.
        #[arg(long)]
        dry_run: bool,

        /// Forward npm provenance attestation to npm publish.
        #[arg(long)]
        provenance: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `ctx traits config ...` subcommands (P457).
#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Compile a `config.ts` or `runtime.ts` authoring source into its
    /// generated `.toml` sibling under `generated/`, through the same
    /// node-invoking CDK build runner traits use — the only path in the
    /// product that touches node for config. A repo with neither source
    /// never needs this command: its `config.toml`/`runtime.toml` stay
    /// hand-authored, TOML-first, zero-drift-check, zero-node.
    Build {
        /// Source path. Defaults to `layout::CONFIG_SOURCE`
        /// (`.ctx/traits/config.ts`) resolved from the current directory;
        /// pass an explicit path (e.g. `.ctx/traits/runtime.ts`, or a
        /// config-home path) to build a different source.
        path: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Accept the repo- or global-tier `runtime.example.ts`/`.toml`
    /// (0178): shows the example's content (or a diff against the
    /// previously accepted content, on re-acceptance), materializes the
    /// machine-local `runtime.ts`/`.toml` copy, and records a digest stamp
    /// covering the example's bytes in the trust store. Never runs
    /// non-interactively without `--yes` — acceptance is the operator's
    /// step, never auto-approved by a running loop.
    Accept {
        /// Accept without an interactive confirmation prompt (non-TTY
        /// contexts, e.g. scripted setup). Still shows the content.
        #[arg(long)]
        yes: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Scaffold the config-home `traits/` directory for the global tier
    /// (0178): `--global` writes a minimal `package.json` pinning
    /// `@ctx-traits/config` there so a global `traits/runtime.ts` can
    /// resolve the authoring package. Node/package-manager install remains
    /// the operator's own step — this only makes the dependency resolvable
    /// once they run it.
    Init {
        /// Scaffold the global (config-home) tier. Currently the only
        /// supported scope — omitting it is a usage error.
        #[arg(long)]
        global: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `ctx traits cache` subcommands.
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Plan a cache rebuild for all discoverable traits.
    Rebuild {
        /// Repository root to scan. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Cache root override. Defaults to this repository's global
        /// per-repository cache root's `traits` subfamily.
        #[arg(long)]
        cache_root: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report cache freshness and staleness.
    Status {
        /// Repository root to scan. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Cache root override. Defaults to this repository's global
        /// per-repository cache root's `traits` subfamily.
        #[arg(long)]
        cache_root: Option<String>,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Plan cache pruning of stale or unreachable artifacts.
    Prune {
        /// Repository root to scan. Defaults to current directory.
        #[arg(long)]
        repo_root: Option<String>,

        /// Cache root override. Defaults to this repository's global
        /// per-repository cache root's `traits` subfamily.
        #[arg(long)]
        cache_root: Option<String>,

        /// Show what would be pruned without removing anything.
        #[arg(long)]
        dry_run: bool,

        /// Prune one declared named build cache
        /// (`[run.build-cache.<name>]`, this repository's global cache
        /// root's `build/<name>` subfamily) instead of stale metadata
        /// records. Omit NAME to prune every declared build cache.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "NAME")]
        build: Option<Option<String>>,

        /// Prune the historical repo-owned `build-target` cache, which predates
        /// declared named build caches.
        #[arg(long, hide = true)]
        build_target: bool,

        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

/// Nested `ctx traits task ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum TaskCommand {
    /// Import a markdown board file into a canonical TOML task document,
    /// writing `<stem>.toml` beside the source file. Never overwrites an
    /// existing `.toml` file at that path.
    Import {
        /// Markdown source file to import.
        #[arg(value_name = "FILE")]
        path: String,

        /// Emit structured JSON instead of the plain report.
        #[arg(long)]
        json: bool,
    },
}

/// `--max-in-flight` must be a real concurrency width: `0` has no sequential
/// meaning (unlike, say, an optional timeout) and silently treating it as
/// "sequential" would hide a typo'd flag instead of rejecting it.
fn parse_positive_max_in_flight(raw: &str) -> Result<usize, String> {
    let value: usize = raw
        .parse()
        .map_err(|_| format!("invalid digit found in string: {raw:?}"))?;
    if value == 0 {
        return Err("max-in-flight must be at least 1".to_string());
    }
    Ok(value)
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Command>, clap::Error> {
    let cli = Cli::try_parse_from(args)?;
    Ok(cli.command)
}

pub fn print_help() {
    let _ = Cli::command().print_help();
}

/// Prints the same release help shown by `ctx traits -h`: locates the
/// `traits` subcommand in the derived tree and renders its own
/// (`override_help`-driven) help, rather than the top-level `ctx` command's
/// — used by bare `ctx traits help` so it matches `-h` instead of falling
/// back to unrelated top-level `ctx` help.
pub fn print_traits_help() {
    let mut traits_command = Cli::command()
        .find_subcommand("traits")
        .cloned()
        .expect("`traits` subcommand always exists in the derived Cli tree");
    let _ = traits_command.print_help();
}

/// The full derived Clap command tree, for `ctx traits help --json`
/// (P453): the source of truth this reference is generated from, rather
/// than a hand-copied inventory.
pub fn command() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
mod task_flag_conflict_tests {
    use super::parse;

    /// 0195: `--task` supersedes the single-run invocation shape, so every
    /// `SessionStartArgs` field it cannot honor is refused at parse time —
    /// never silently discarded (the phantom-scope bug this proves against:
    /// a positional trait, `--file`, `--set`, `--input`, or trailing trait
    /// args silently ignored while `--task` drives the actual dispatch).
    /// Runs on a dedicated thread with a generous stack: clap's derived
    /// error-path rendering for this CLI's large subcommand tree needs more
    /// than the default per-test-thread stack.
    fn assert_task_conflict_refused(argv: &[&str]) {
        let owned: Vec<std::ffi::OsString> = argv.iter().map(std::ffi::OsString::from).collect();
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || parse(owned).is_err())
            .expect("spawn conflict-check thread");
        assert!(
            handle.join().expect("conflict-check thread panicked"),
            "expected a clap conflict error for {argv:?}, parsed instead"
        );
    }

    #[test]
    fn positional_trait_conflicts_with_task() {
        assert_task_conflict_refused(&["ctx", "traits", "run", "some-trait", "--task", "0001"]);
    }

    #[test]
    fn task_dispatch_conflicts_with_task() {
        assert_task_conflict_refused(&[
            "ctx",
            "traits",
            "run",
            "--task-dispatch",
            "--task",
            "0001",
        ]);
    }

    #[test]
    fn file_flag_conflicts_with_task() {
        assert_task_conflict_refused(&[
            "ctx", "traits", "run", "--file", "x.toml", "--task", "0001",
        ]);
    }

    #[test]
    fn set_flag_conflicts_with_task() {
        assert_task_conflict_refused(&[
            "ctx", "traits", "run", "--set", "foo=bar", "--task", "0001",
        ]);
    }

    #[test]
    fn input_flag_conflicts_with_task() {
        assert_task_conflict_refused(&[
            "ctx",
            "traits",
            "run",
            "--input",
            "ports.json",
            "--task",
            "0001",
        ]);
    }

    #[test]
    fn trailing_trait_args_conflict_with_task() {
        assert_task_conflict_refused(&[
            "ctx", "traits", "run", "--task", "0001", "--", "some", "args",
        ]);
    }
}
