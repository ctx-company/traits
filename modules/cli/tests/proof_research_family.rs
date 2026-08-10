//! 0153: scratch-fixture behavioral proofs for the `research` family's
//! genuinely new structural mechanisms — never the checked-in `research`
//! package itself (2026-08-08 ruling: no test may depend on committed repo
//! content). This suite owns a bespoke, minimal fixture trait reproducing
//! exactly the pieces 0153 introduced: a deterministic topic-slug/report-
//! path derivation (plain command steps, never agent prose) and a
//! deterministic stream-cardinality gate (`condition.count`, never an agent
//! judgment) ahead of the family's shared `familyCommitTail` two-step
//! staging recipe (promoted to `@ctx-traits/agents` by this same task).
//!
//! Two forks:
//! - a valid plan clears the cardinality gate on round 1, the deterministic
//!   slug/path derivation produces the exact predicted values, and the
//!   commit lands.
//! - an always-invalid plan never clears the gate; the bounded loop
//!   exhausts under `on-exhausted = "abort"` and the run blocks with no
//!   commit — the same honest-park contract P461 proves for review loops,
//!   here proven for a deterministic produce/gate loop instead.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, fixture_agent_bin, git_init, require_success_with_env, run_git, utf8};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("cannot create directory {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

fn commit_all(repo: &Path, home: &Path, message: &str) {
    for args in [vec!["add", "-A"], vec!["commit", "--quiet", "-m", message]] {
        let output = run_git(&args, repo, home);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn git_rev_parse_head(repo: &Path, home: &Path) -> String {
    let output = run_git(&["rev-parse", "HEAD"], repo, home);
    assert!(output.status.success(), "git rev-parse HEAD failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Every role bound to `ctx-fixture-agent`, matching the fixture-agent
/// binary's `planner`/`worker`/`scribe` roles (`modules/cli/tests/
/// fixture-agent/src/main.rs`).
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

[harness.stub-planner]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-planner.cli]
argv = ["--role", "planner"]
output = "raw-json"

[harness.stub-worker]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-worker.cli]
argv = ["--role", "worker"]
output = "raw-json"

[harness.stub-scribe]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-scribe.cli]
argv = ["--role", "scribe"]
output = "raw-json"

[agent.role.planner]
harness = "stub-planner"
transport = "cli"
session-mode = "per-frame"

[agent.role.worker]
harness = "stub-worker"
transport = "cli"
session-mode = "per-frame"

[agent.role.scribe]
harness = "stub-scribe"
transport = "cli"
session-mode = "per-frame"
"#
    );
    write_file(&repo.join("ctx.toml"), &ctx_toml);
}

/// Minimal fixture family: `topic`/`output-dir` input ports, the exact
/// topic-slug shell derivation `.ctx/traits/packages/research/source/
/// data.ts`'s `deriveTopicSlugStep` authors, a `printf`-only report-path
/// derivation, a bounded typed-plan cardinality-gated loop
/// (`condition.count(stream-plan).equals(2)` — 0153's deterministic gate,
/// hand-authored here in plain canonical rather than compiled through the
/// CDK, matching P461's own precedent for controlling `on-exhausted`
/// directly), and the family commit tail's two-step staging recipe
/// (`git add -A` then `git reset -q -- .agents/runs`, run-42bd7fb2's fix).
fn fixture_trait_toml(max_iterations: u32, on_exhausted: &str) -> String {
    format!(
        r#"id = "research-fixture"
schema-version = "0.2"
version = "0.1.0"
name = "Research Family Fixture"
summary = "0153 proof fixture: topic-slug/report-path determinism, plan cardinality gate, and commit-tail two-step staging."

[[port]]
id = "topic"
direction = "input"
schema = "schema:text"
description = "The research topic."

[[port]]
id = "output-dir"
direction = "input"
schema = "schema:text"
description = "Repo-relative output root."

[[port]]
id = "report-path"
direction = "output"
schema = "schema:text"
optional = true
value = "slot:report-path"
description = "Deterministic path to the delivered report."

[[slot]]
id = "topic-slug"
schema = "schema:text"
description = "Deterministic kebab-case slug for the topic."

[[slot]]
id = "report-path"
schema = "schema:text"
description = "Deterministic path to the delivered report."

[[slot]]
id = "stream-plan"
schema = "[schema:research-stream]"
description = "The typed stream plan the cardinality gate checks."

[[slot]]
id = "work-summary"
schema = "schema:text"
description = "Fixture worker output."

[[slot]]
id = "commit-message"
schema = "schema:text"
description = "Fixture scribe commit message."

[[slot]]
id = "git-status"
schema = "schema:text"
description = "git status --porcelain, captured before the commit tail."

[[slot]]
id = "stage-output"
schema = "schema:text"
description = "Output evidence from the git staging command."

[[slot]]
id = "unstage-output"
schema = "schema:text"
description = "Output evidence from the runtime-state unstage command."

[[slot]]
id = "commit-output"
schema = "schema:text"
description = "Output evidence from the git commit command."

[[schema]]
id = "research-stream"
description = "One non-overlapping research stream."

[schema.fields.id]
schema = "schema:text"
required = true

[schema.fields.focus]
schema = "schema:text"
required = true

[schema.fields.kind]
schema = "schema:text"
required = true
allowed = ["primary", "counterevidence"]

[[agent]]
id = "planner"
description = "Fixture planner."
summary = "Planning role."

[[agent]]
id = "worker"
description = "Fixture worker."
summary = "Implementation role."

[[agent]]
id = "scribe"
description = "Fixture scribe."
summary = "Commit-message role."

[prompt.plan]
text = "Plan streams for {{port:topic}}."
input = ["port:topic"]
output = ["slot:stream-plan"]

[prompt.research]
text = "Research {{port:topic}} per plan {{slot:stream-plan}}."
input = ["port:topic", "slot:stream-plan"]
output = ["slot:work-summary"]

[prompt.summarization]
text = "Write the commit message for the completed work: {{slot:work-summary}}."
input = ["slot:work-summary"]
output = ["slot:commit-message"]

[[sequence.planning-loop.sequence]]
id = "plan"
title = "Plan the streams"
agent = "agent:planner"
prompt = "prompt:plan"
input = ["port:topic"]
output = ["slot:stream-plan"]

[[sequence.commit-tail.sequence]]
id = "summarization"
title = "Write the commit message"
agent = "agent:scribe"
prompt = "prompt:summarization"
input = ["slot:work-summary"]
output = ["slot:commit-message"]

[[sequence.commit-tail.sequence]]
id = "stage"
title = "Stage all changes"
output = ["slot:stage-output"]

[sequence.commit-tail.sequence.command]
argv = ["git", "add", "-A"]

[[sequence.commit-tail.sequence]]
id = "unstage-runtime"
title = "Unstage runtime state"
output = ["slot:unstage-output"]

[sequence.commit-tail.sequence.command]
argv = ["git", "reset", "-q", "--", ".agents/runs"]

[[sequence.commit-tail.sequence]]
id = "commit"
title = "Commit the work"
input = ["slot:commit-message"]
output = ["slot:commit-output"]

[sequence.commit-tail.sequence.command]
argv = ["git", "commit", "-m", "{{slot:commit-message}}"]

[procedure]
description = "Derive the topic slug and report path deterministically, gate a typed stream plan on cardinality, research, then commit."
input = ["port:topic", "port:output-dir"]
output = ["port:report-path"]

[[procedure.sequence]]
id = "topic-slug"
title = "Derive topic slug"
input = ["port:topic"]
output = ["slot:topic-slug"]

[procedure.sequence.command]
argv = ["sh", "-c", "printf \"%s\" \"$1\" | tr \"A-Z\" \"a-z\" | tr -cs \"a-z0-9\" \"-\" | sed \"s/^-*//;s/-*$//\"", "_", "{{port:topic}}"]

[[procedure.sequence]]
id = "report-path"
title = "Derive report path"
input = ["port:output-dir", "slot:topic-slug"]
output = ["slot:report-path"]

[procedure.sequence.command]
argv = ["printf", "%s/%s/report.md", "{{port:output-dir}}", "{{slot:topic-slug}}"]

[[procedure.sequence]]
id = "planning"
title = "Plan the streams"
kind = "loop"
sequence = "sequence:planning-loop"
max-iterations = {max_iterations}
on-exhausted = "{on_exhausted}"

[procedure.sequence.until]
count = "slot:stream-plan"
equals = 2

[[procedure.sequence]]
id = "research"
title = "Research the streams"
agent = "agent:worker"
prompt = "prompt:research"
input = ["port:topic", "slot:stream-plan"]
output = ["slot:work-summary"]

[[procedure.sequence]]
id = "check-git-status"
title = "Check working tree status"
output = ["slot:git-status"]

[procedure.sequence.command]
argv = ["git", "status", "--porcelain"]

[[procedure.sequence]]
id = "maybe-commit"
title = "Maybe Commit"
kind = "branch"
sequence = "sequence:commit-tail"

[procedure.sequence.when.not]
slot = "slot:git-status"
equals = ""
"#
    )
}

fn trait_manifest(id: &str) -> String {
    format!(
        "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"Research Family Fixture\"\nstatus = \"draft\"\n"
    )
}

/// Fresh scratch repo carrying one fixture trait, reviewed and activated.
fn setup_fixture(
    label: &str,
    trait_toml: &str,
) -> (ScratchRoot, std::path::PathBuf, std::path::PathBuf) {
    let id = "research-fixture";
    let scratch = ScratchRoot::new(label);
    let repo = scratch.path().join("repo");
    let home = scratch.home();
    fs::create_dir_all(&repo)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", repo.display()));
    git_init(&repo);
    write_file(
        &repo.join("README.md"),
        "0153 research-family proof scratch repository\n",
    );
    write_file(
        &repo.join(".gitignore"),
        "/.ctx/runs/*.json\n/.ctx/traits/worktrees/\n/.ctx/debug/\n",
    );
    commit_all(&repo, &home, "initial commit");

    support::require_success("`ctx traits init`", &["traits", "init"], &repo, &home);

    write_file(
        &repo.join(format!(".ctx/traits/packages/{id}/package.toml")),
        &trait_manifest(id),
    );
    write_file(
        &repo.join(format!(".ctx/traits/packages/{id}/generated/index.toml")),
        trait_toml,
    );
    write_fixture_ctx_toml(&repo);
    commit_all(&repo, &home, "add fixture trait");

    support::require_success(
        "`ctx traits check`",
        &["traits", "check", id, "--skip-cdk-drift", "--json"],
        &repo,
        &home,
    );
    support::require_success(
        "`ctx traits review --approve`",
        &["traits", "review", id, "--approve"],
        &repo,
        &home,
    );
    support::require_success(
        "`ctx traits activate`",
        &["traits", "activate", id],
        &repo,
        &home,
    );
    commit_all(&repo, &home, "activate fixture trait");

    (scratch, repo, home)
}

const VALID_PLAN: &str = r#"[{"id":"s1","focus":"primary angle","kind":"primary"},{"id":"s2","focus":"counterevidence angle","kind":"counterevidence"}]"#;
const INVALID_PLAN: &str = r#"[{"id":"s1","focus":"only one stream","kind":"primary"}]"#;

/// A plan that clears the cardinality gate on round 1: the deterministic
/// topic-slug and report-path derivations produce exactly the predicted
/// values (never agent prose), and the run lands its commit.
#[test]
fn valid_plan_clears_gate_and_derives_deterministic_paths() {
    let (scratch, repo, home) =
        setup_fixture("p153-research-valid-plan", &fixture_trait_toml(3, "abort"));
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = require_success_with_env(
        "valid-plan `ctx traits run`",
        &[
            "traits",
            "run",
            "research-fixture",
            "--set",
            "topic=Machine Learning: A Survey! (2026)",
            "--set",
            "output-dir=.internal/research",
            "--max-frames",
            "10",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "60",
            "--json",
        ],
        &repo,
        &home,
        &[("CTX_FIXTURE_PLANNER_STREAMS", VALID_PLAN)],
    );
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "a plan that clears the cardinality gate on round 1 must complete:\n{stdout}"
    );
    assert!(
        stdout.contains(".internal/research/machine-learning-a-survey-2026/report.md"),
        "report-path must equal the deterministic slug derivation, never agent prose:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_ne!(
        head_before, head_after,
        "a completed run with a dirty tree must commit"
    );
    let log = run_git(&["log", "-1", "--format=%s"], &repo, &home);
    let (subject, _) = utf8(&log);
    assert!(
        !subject.trim().is_empty(),
        "commit subject must be the scribe's message: {subject}"
    );

    let _ = scratch;
}

/// A plan that never clears the cardinality gate (one stream, gate requires
/// exactly two) exhausts the bounded loop under `on-exhausted = "abort"`:
/// the run blocks, and — because the commit tail sits strictly after the
/// gated loop in sequence order — no commit is ever created. The same
/// honest-park contract P461 proves for review loops, proven here for a
/// deterministic produce/gate loop.
#[test]
fn invalid_plan_exhausts_the_gate_and_blocks_without_committing() {
    let (scratch, repo, home) = setup_fixture(
        "p153-research-invalid-plan",
        &fixture_trait_toml(2, "abort"),
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = require_success_with_env(
        "invalid-plan `ctx traits run`",
        &[
            "traits",
            "run",
            "research-fixture",
            "--set",
            "topic=Anything",
            "--set",
            "output-dir=.internal/research",
            "--max-frames",
            "10",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "60",
            "--json",
        ],
        &repo,
        &home,
        &[("CTX_FIXTURE_PLANNER_STREAMS", INVALID_PLAN)],
    );
    assert!(
        stdout.contains("\"final-state\": \"blocked\""),
        "a plan that never clears the cardinality gate must block on exhaustion:\n{stdout}"
    );
    assert!(
        !stdout.contains("\"final-state\": \"completed\""),
        "an exhausted gate loop must never also report completed:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "a blocked run must never reach the commit tail — the gate sits strictly before it"
    );

    let _ = scratch;
}
