//! P461 slice 2: park-honesty proofs for P414's contract, re-derived against
//! a bespoke, minimal `implement-*`-family fixture trait this suite owns
//! outright — never the checked-in `implement-quick`/`implement-default`
//! packages. That checked-in coupling is exactly what broke the frozen
//! `scripts/byte_compare.rs` proof: d0e9169 (an ordinary owner edit,
//! `implement-quick`'s loop `on-exhausted: "block" -> "continue"`) flipped
//! the shipped package's exhaustion policy out from under a proof that had
//! frozen the OLD policy's native runtime behavior (`on-exhausted = "block"`
//! stops the run with `Status::Blocked`, matching the byte_compare assertion
//! "a revise verdict must never commit"). Under the shipped package's
//! CURRENT `"continue"` policy that assertion is simply false: an exhausted
//! loop proceeds to the commit tail regardless of the final verdict, which
//! is d0e9169's own intended behavior and is nowhere else under proof.
//!
//! This suite therefore asserts the real fork P414's contract actually
//! describes, driven by the runtime's native loop-exhaustion mechanism
//! (`on-exhausted`), never a fixture-authored escalation branch:
//!
//! - `on-exhausted = "block"`: the loop's own native block behavior
//!   (`Status::Blocked`, `stop-reason.reason = "max-iterations-exhausted"`)
//!   IS the P414 "parked" case — no commit, a typed park-report evidence
//!   entry citing the reviewer's wall id verbatim, and (only when a merge
//!   intent is in play) a distinct non-zero exit.
//! - `on-exhausted = "continue"`: exhaustion is transparent, the run
//!   proceeds past the loop to the commit tail, and lands — the shipped
//!   package's actual current behavior, guarded here for the first time.
//!
//! Reviewer behavior is configured per invocation via `CTX_FIXTURE_*`
//! environment variables layered onto the `ctx traits run` call
//! (`support::run_ctx_with_env`) — `ctx`'s own harness dispatch never clears
//! its process environment before spawning a custom-kind harness child
//! (`modules/io/src/harness.rs`), so the SAME `ctx.toml` role wiring serves
//! every scenario below.
//!
//! Every subprocess this suite spawns — `ctx` and `git` alike — runs under
//! `support`'s one controlled environment (`env_clear`, `HOME`/`XDG_*`
//! rooted at this test's own scratch `home`): a proof's outcome must never
//! depend on the invoking machine's ambient `~/.gitconfig`/
//! `core.excludesFile`, which is exactly what silently decided whether
//! `.docs/`/`.plans/` landed in the fixture repo's first commit on a
//! machine whose personal global gitignore happens to exclude them.

use std::fs;
use std::path::Path;

use serial_test::serial;
use support::{
    ScratchRoot, controlled_command, ctx_bin, fixture_agent_bin, git_init, require_success,
    require_success_with_env, run_ctx, run_git, utf8,
};

/// P460's typed exit for a merge-intent run that never reached a completed
/// drive — mirrored here rather than imported so a drift in
/// `crate::app::error`'s constant breaks this proof loudly instead of
/// silently tracking whatever the binary happens to emit.
const EXIT_RUN_NOT_COMPLETED: i32 = 3;

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

fn extract_quoted_after(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)? + marker.len();
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

/// Resolve this repository's global run-ledger root (P426) by asking the
/// compiled binary itself, rather than re-deriving the repo-key hash here
/// and risking it drifting from `modules/io/src/state.rs`'s real algorithm.
fn global_runs_root(repo: &Path, home: &Path) -> std::path::PathBuf {
    let stdout = require_success(
        "`ctx traits doctor --migrate-state --json`",
        &["traits", "doctor", "--migrate-state", "--json"],
        repo,
        home,
    );
    let at = stdout
        .find("\"runs\"")
        .expect("doctor --migrate-state --json reports a runs field");
    let global = extract_quoted_after(&stdout[at..], "\"global\": \"")
        .expect("doctor --migrate-state --json reports runs.global");
    std::path::PathBuf::from(global)
}

/// The fixture trait's `ctx.toml`: every role backed by `ctx-fixture-agent`
/// (P461), wired identically regardless of scenario — only the
/// `CTX_FIXTURE_REVIEWER<N>_*` env vars on each `ctx traits run` invocation
/// vary a reviewer's verdict. Two independent reviewer harnesses
/// (`stub-reviewer1`/`stub-reviewer2`, each passing its own fixed slot
/// number as trailing argv) are always declared so the same `ctx.toml`
/// serves both the single-reviewer forks (which bind only `smart-1`) and
/// the dual-reviewer fork (which binds `smart-1` and `smart-2` to
/// independently configurable verdicts) — an unused `agent.role.smart-2`
/// binding is inert for a trait that never declares a `smart-2` agent.
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

[harness.stub-worker]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-worker.cli]
argv = ["--role", "worker"]
output = "raw-json"

[harness.stub-reviewer1]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-reviewer1.cli]
argv = ["--role", "reviewer", "1"]
output = "raw-json"

[harness.stub-reviewer2]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-reviewer2.cli]
argv = ["--role", "reviewer", "2"]
output = "raw-json"

[harness.stub-scribe]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-scribe.cli]
argv = ["--role", "scribe"]
output = "raw-json"

[agent.role.worker]
harness = "stub-worker"
transport = "cli"
session-mode = "per-frame"

[agent.role.smart-1]
harness = "stub-reviewer1"
transport = "cli"
session-mode = "per-frame"

[agent.role.smart-2]
harness = "stub-reviewer2"
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

/// Shared by every fixture trait variant this suite authors (single- and
/// dual-reviewer alike): the `blocker`/`review-verdict` schema declarations.
const SCHEMA_BLOCKS: &str = r#"
[[schema]]
id = "blocker"
description = "One blocking defect."

[schema.fields.id]
schema = "schema:text"
required = true

[schema.fields.where]
schema = "schema:text"
required = true

[schema.fields.what]
schema = "schema:text"
required = true

[schema.fields.root-cause]
schema = "schema:text"
required = true

[schema.fields.required-fix]
schema = "schema:text"
required = true

[schema.fields.done-when]
schema = "schema:text"
required = true

[[schema]]
id = "review-verdict"
description = "Typed single-pass review verdict."

[schema.fields.status]
schema = "schema:text"
required = true
allowed = ["approved", "revise"]

[schema.fields.blockers]
schema = "[schema:blocker]"
required = true

[schema.fields.wall-id]
schema = "schema:text"
required = true
"#;

/// Shared by every fixture trait variant: the `worker`/`scribe` agents, the
/// `implement`/`summarization` prompts, and the git commit tail
/// (P397's own contract — a dirty tree gates the commit, authored here
/// exactly as `implement-quick`'s generated package authors it).
const WORKER_SCRIBE_AGENTS_AND_COMMIT_TAIL: &str = r#"
[[agent]]
id = "worker"
description = "Fixture worker."
summary = "Implementation role."

[[agent]]
id = "scribe"
description = "Fixture scribe."
summary = "Commit-message role."

[prompt.implement]
text = "Implement {port:phase}."
input = ["port:phase"]
output = ["slot:work-summary"]

[prompt.summarization]
text = "Write the commit message for the completed work: {slot:work-summary}."
input = ["slot:work-summary"]
output = ["slot:commit-message"]

[[sequence.commit-tail.sequence]]
id = "summarization"
title = "Write the commit message"
agent = "agent:scribe"
prompt = "prompt:summarization"
input = ["slot:work-summary"]
output = ["slot:commit-message"]

[[sequence.commit-tail.sequence]]
id = "git-stage"
title = "Stage all changes"
output = ["slot:stage-output"]

[sequence.commit-tail.sequence.command]
argv = ["git", "add", "-A"]

[[sequence.commit-tail.sequence]]
id = "git-commit"
title = "Commit the phase"
input = ["slot:commit-message"]
output = ["slot:commit-output"]

[sequence.commit-tail.sequence.command]
argv = ["git", "commit", "--allow-empty", "-m", "{slot:commit-message}"]
"#;

/// The fixture trait itself: one worker step, a single-review loop whose
/// `on-exhausted` policy is the one axis this suite varies, a commit tail
/// gated on a dirty working tree (P397's own contract, authored here
/// exactly as `implement-quick`'s generated package authors it), and a
/// `port:park-report` output carrying whatever the loop's last round wrote
/// to `slot:park-report` — the SAME `deriveParkReportStep` clear/append
/// primitive (`.ctx/traits/implement-default/source/shared.ts`) hand-authored
/// in plain TOML rather than compiled through the CDK, since this suite
/// must control `on-exhausted` and the reviewer's schema directly rather
/// than inheriting whatever the checked-in packages currently declare.
///
/// The trait id must start with `implement-` — `is_implement_family`
/// (`modules/io/src/dispatch_preflight.rs`) gates wall preflight
/// participation on that literal prefix.
fn fixture_trait_toml(id: &str, max_iterations: u32, on_exhausted: &str) -> String {
    format!(
        r#"id = "{id}"
schema-version = "0.2"
version = "0.1.0"
name = "Park Honesty Fixture"
summary = "P461 park-honesty proof fixture: a single-review loop whose on-exhausted policy is the fixture's own axis."

[[resource]]
id = "execution-plan"
path = ".plans/EXECUTION_PLAN.md"
root = "repo"
trigger = "on-demand"

[[port]]
id = "phase"
direction = "input"
schema = "schema:text"
description = "Phase or group, exactly as named in .plans/EXECUTION_PLAN.md."

[[port]]
id = "park-report"
direction = "output"
schema = "[schema:review-verdict]"
optional = true
description = "Typed park record: present in the run's persisted output-port evidence only when the single-review loop exhausted without approval."
value = "slot:park-report"

[[slot]]
id = "work-summary"
schema = "schema:text"
description = "Fixture worker output."

[[slot]]
id = "review-verdict"
schema = "schema:review-verdict"
description = "Fixture reviewer verdict."

[[slot]]
id = "park-report"
schema = "[schema:review-verdict]"
description = "This round's still-revise verdict(s); cleared and re-appended every round."

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

[[slot]]
id = "commit-output"
schema = "schema:text"

{SCHEMA_BLOCKS}

[[agent]]
id = "smart-1"
description = "Fixture reviewer."
summary = "Reviewer role."
{WORKER_SCRIBE_AGENTS_AND_COMMIT_TAIL}

[prompt.review]
text = "Review {{port:phase}}: {{slot:work-summary}}."
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict"]

[[sequence.park-report-record-then.sequence]]
id = "park-report-append"
title = "Park Report Append"
kind = "project"
input = ["slot:review-verdict"]

[[sequence.park-report-record-then.sequence.projection]]
source = "slot:review-verdict"
destination = "slot:park-report"
operation = "append"

[[sequence.park-report-record-then.sequence.output]]
slot = "slot:park-report"
operation = "append"

[[sequence.review-loop.sequence]]
id = "review"
title = "Review work"
agent = "agent:smart-1"
prompt = "prompt:review"
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict"]

[[sequence.review-loop.sequence]]
id = "park-report-clear"
title = "Park Report Clear"
kind = "project"
output = ["slot:park-report"]

[[sequence.review-loop.sequence.projection]]
destination = "slot:park-report"

[sequence.review-loop.sequence.projection.source]
literal = []

[[sequence.review-loop.sequence]]
id = "park-report-record"
title = "Park Report Record"
kind = "branch"
sequence = "sequence:park-report-record-then"

[sequence.review-loop.sequence.when]
slot = "slot:review-verdict"
field = "status"
equals = "revise"

[procedure]
description = "Implement, review in a bounded loop, then commit when the tree is dirty."
input = ["port:phase"]
output = ["port:park-report"]

[[procedure.sequence]]
id = "implement"
title = "Implement"
agent = "agent:worker"
prompt = "prompt:implement"
input = ["port:phase"]
output = ["slot:work-summary"]

[[procedure.sequence]]
id = "single-review"
title = "Single reviewed pass"
kind = "loop"
sequence = "sequence:review-loop"
max-iterations = {max_iterations}
on-exhausted = "{on_exhausted}"

[procedure.sequence.until]
slot = "slot:review-verdict"
field = "status"
equals = "approved"

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
        "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"Park Honesty Fixture\"\nstatus = \"draft\"\n"
    )
}

/// Fresh scratch repo carrying one fixture trait (`id`, rendered as
/// `trait_toml` by the caller — [`fixture_trait_toml`] or
/// [`dual_review_trait_toml`]), a `.plans/EXECUTION_PLAN.md` naming every
/// `(phase, wall)` pair, and a `.docs/PRODUCT.md` stub — reviewed and ready
/// to drive. `.docs/`/`.plans/` are committed here, under the controlled
/// environment (never the invoking machine's own `~/.gitconfig`), so
/// whether they land in the fixture's tracked set never depends on ambient
/// developer config.
fn setup_fixture(
    label: &str,
    id: &str,
    trait_toml: &str,
    phases: &[(&str, Option<&str>)],
) -> (ScratchRoot, std::path::PathBuf, std::path::PathBuf) {
    let scratch = ScratchRoot::new(label);
    let repo = scratch.path().join("repo");
    let home = scratch.home();
    fs::create_dir_all(&repo)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", repo.display()));
    git_init(&repo);
    write_file(
        &repo.join("README.md"),
        "P461 park-honesty proof scratch repository\n",
    );
    write_file(
        &repo.join(".gitignore"),
        "/.ctx/runs/*.json\n/.ctx/worktrees/\n/.ctx/debug/\n",
    );
    let mut plan = String::from("# Execution Plan\n\n## Group 1\n\n");
    for (phase, wall) in phases {
        plan.push_str(&format!(
            "- [ ] **{phase}** Park-honesty fixture phase\n  Why: fixture.\n  Approach: fixture.\n  Done-when: fixture gates pass.\n  Deps: none\n"
        ));
        if let Some(wall) = wall {
            plan.push_str(&format!("  **Wall:** {wall}\n"));
        }
        plan.push('\n');
    }
    write_file(&repo.join(".plans/EXECUTION_PLAN.md"), &plan);
    write_file(
        &repo.join(".docs/PRODUCT.md"),
        "# Product\n\n## Validation Gates\n- fixture gate\n",
    );
    commit_all(&repo, &home, "initial commit");

    require_success("`ctx traits init`", &["traits", "init"], &repo, &home);

    write_file(
        &repo.join(format!(".ctx/traits/{id}/trait.toml")),
        &trait_manifest(id),
    );
    write_file(
        &repo.join(format!(".ctx/traits/{id}/generated/index.toml")),
        trait_toml,
    );
    write_fixture_ctx_toml(&repo);
    commit_all(&repo, &home, "add fixture trait");

    require_success(
        "`ctx traits check`",
        &["traits", "check", id, "--skip-cdk-drift", "--json"],
        &repo,
        &home,
    );
    require_success(
        "`ctx traits review --approve`",
        &["traits", "review", id, "--approve"],
        &repo,
        &home,
    );
    require_success(
        "`ctx traits activate`",
        &["traits", "activate", id],
        &repo,
        &home,
    );
    commit_all(&repo, &home, "activate fixture trait");

    (scratch, repo, home)
}

/// Fork 1 (blocked) + fork 3 (standing wall): a reviewer that always
/// revises, citing a wall, drives the fixture's own native
/// `on-exhausted = "block"` path — `Status::Blocked`, no commit, and a
/// `slot:park-report` evidence entry citing the wall id verbatim. A second
/// phase citing the SAME wall is then refused at dispatch preflight, before
/// any session exists.
#[test]
fn blocked_exhaustion_parks_and_stands_as_a_wall() {
    const WALL: &str = "WALL-P461-FIXTURE";
    let id = "implement-fixture-park-blocked";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-blocked",
        id,
        &fixture_trait_toml(id, 2, "block"),
        &[("P900", Some(WALL)), ("P901", Some(WALL))],
    );
    let runs_root = global_runs_root(&repo, &home);
    fs::create_dir_all(&runs_root).unwrap();

    let head_before = git_rev_parse_head(&repo, &home);
    let ledger_path = runs_root.join("park-honesty-p900.json");
    require_success_with_env(
        "P900 (always-revise, wall-citing) `ctx traits run`",
        &[
            "traits",
            "run",
            "implement-fixture-park-blocked",
            "--set",
            "phase=P900",
            "--out",
            ledger_path.to_str().unwrap(),
            "--max-frames",
            "20",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "120",
            "--json",
        ],
        &repo,
        &home,
        &[
            ("CTX_FIXTURE_REVIEWER1_MODE", "revise"),
            ("CTX_FIXTURE_REVIEWER1_WALL", WALL),
            ("CTX_FIXTURE_REVIEWER1_BLOCKER", "fixture-standing-defect"),
        ],
    );
    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "an exhausted, still-revise loop under on-exhausted=\"block\" must never commit"
    );
    let ledger_text = fs::read_to_string(&ledger_path).expect("P900 ledger readable");
    assert!(
        ledger_text.contains("\"final-state\": \"blocked\""),
        "P900 must reach final-state blocked:\n{ledger_text}"
    );
    assert!(
        !ledger_text.contains("\"final-state\": \"completed\""),
        "P900's ledger must not also carry final-state completed:\n{ledger_text}"
    );
    assert!(
        ledger_text.contains("\"ref-text\": \"slot:park-report\""),
        "P900's ledger must carry accepted slot:park-report evidence:\n{ledger_text}"
    );
    assert_single_park_entry(&ledger_text, WALL, "fixture-standing-defect");

    // Fork 3: P901 explicitly cites the SAME wall — dispatch must be
    // refused before any session/frame exists.
    let refusal = run_ctx(
        &[
            "traits",
            "run",
            "implement-fixture-park-blocked",
            "--set",
            "phase=P901",
            "--no-drive",
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(
        !refusal.status.success(),
        "P901 (same wall as standing P900) must be refused at dispatch preflight, not dispatched"
    );
    let (stdout, stderr) = utf8(&refusal);
    let refusal_text = format!("{stdout}{stderr}");
    assert!(
        refusal_text.contains(WALL),
        "P901's dispatch refusal must cite the standing wall id:\n{refusal_text}"
    );
    let _ = scratch;
}

/// Fork 2: a reviewer that always revises with NO wall citation, under
/// `on-exhausted = "continue"` — the loop's exhaustion is transparent, the
/// run proceeds past it to the commit tail, and lands. This is the shipped
/// `implement-quick` package's own current behavior since d0e9169; nothing
/// else guards it.
#[test]
fn continuing_exhaustion_lands_the_commit() {
    let id = "implement-fixture-park-continue";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-continue",
        id,
        &fixture_trait_toml(id, 2, "continue"),
        &[("P950", None)],
    );
    let runs_root = global_runs_root(&repo, &home);
    fs::create_dir_all(&runs_root).unwrap();

    let head_before = git_rev_parse_head(&repo, &home);
    let ledger_path = runs_root.join("park-honesty-p950.json");
    require_success_with_env(
        "P950 (always-revise, no wall) `ctx traits run`",
        &[
            "traits",
            "run",
            "implement-fixture-park-continue",
            "--set",
            "phase=P950",
            "--out",
            ledger_path.to_str().unwrap(),
            "--max-frames",
            "20",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "120",
            "--json",
        ],
        &repo,
        &home,
        &[
            ("CTX_FIXTURE_REVIEWER1_MODE", "revise"),
            (
                "CTX_FIXTURE_REVIEWER1_BLOCKER",
                "fixture-never-fixed-defect",
            ),
        ],
    );
    let head_after = git_rev_parse_head(&repo, &home);
    assert_ne!(
        head_before, head_after,
        "an exhausted loop under on-exhausted=\"continue\" must still land its commit"
    );
    let ledger_text = fs::read_to_string(&ledger_path).expect("P950 ledger readable");
    assert!(
        ledger_text.contains("\"final-state\": \"completed\""),
        "P950 must reach final-state completed despite the loop never approving:\n{ledger_text}"
    );

    let _ = scratch;
}

/// Fork 4: the real Justfile `_implement-loop` recipe (the same private
/// recipe `just implement`/`just implement-lean` both call) halts a batch
/// at the first phase that ends without completing, and never dispatches
/// the phase after it.
#[test]
#[serial]
fn batch_halts_at_the_first_blocked_phase() {
    const WALL: &str = "WALL-P461-BATCH-FIXTURE";
    let id = "implement-fixture-park-batch";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-batch-exit",
        id,
        &fixture_trait_toml(id, 2, "block"),
        &[("P960", Some(WALL)), ("P961", Some(WALL))],
    );
    let source_root = support::repo_root();
    let justfile = source_root.join("Justfile");
    assert!(
        justfile.exists(),
        "source repository Justfile not found at {}",
        justfile.display()
    );

    // `just` invokes `ctx` by bare name via `PATH`, and the recipe's own
    // fixture-reviewer configuration travels as `CTX_FIXTURE_*` env vars —
    // both layered on top of `support::controlled_command`'s one
    // environment definition, never a second hand-rolled copy of it.
    let real_path = std::env::var("PATH").unwrap_or_default();
    let ctx_dir = ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .to_path_buf();
    let patched_path = format!("{}:{real_path}", ctx_dir.display());

    let head_before = git_rev_parse_head(&repo, &home);
    let mut command = controlled_command(
        Path::new("just"),
        &[
            "--justfile",
            justfile.to_str().unwrap(),
            "--working-directory",
            repo.to_str().unwrap(),
            "_implement-loop",
            "implement-fixture-park-batch",
            "P960,P961",
        ],
        &repo,
        &home,
    );
    command
        .env("PATH", patched_path)
        .env("CTX_FIXTURE_REVIEWER1_MODE", "revise")
        .env("CTX_FIXTURE_REVIEWER1_WALL", WALL)
        .env("CTX_FIXTURE_REVIEWER1_BLOCKER", "fixture-batch-defect");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run just _implement-loop: {error}"));
    let (stdout, stderr) = utf8(&output);
    let combined = format!("{stdout}{stderr}");

    // P414's contract: a distinct nonzero exit, reported as a typed park —
    // never a bare exit code. Under the P460 completion seam a drive that
    // ends without completing exits EXIT_RUN_NOT_COMPLETED = 3.
    assert_eq!(
        output.status.code(),
        Some(EXIT_RUN_NOT_COMPLETED),
        "`_implement-loop \"P960,P961\"` must exit {EXIT_RUN_NOT_COMPLETED} (EXIT_RUN_NOT_COMPLETED):\n{combined}"
    );
    assert!(
        combined.contains("PARKED"),
        "batch output must report a PARKED phase:\n{combined}"
    );
    assert!(
        !combined.contains("P961"),
        "the second phase must never be mentioned — it must never be dispatched once P960 parks:\n{combined}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "HEAD must not move — the parked phase must never merge"
    );

    let _ = scratch;
}

/// Fork 5: the dual-reviewer park-report round trip. Two independent
/// reviewers (`smart-1`/`smart-2`) each append/clear the SAME
/// `slot:park-report` every round — this is the exact
/// `deriveParkReportStep` clear/append primitive
/// (`.ctx/traits/implement-default/source/shared.ts`) the single-reviewer
/// forks above exercise once per round, now exercised twice per round.
///
/// `only_the_blocking_reviewer_is_named` proves an approved verdict from
/// one reviewer never masks a still-revise verdict from the other — the
/// park report must name only the blocking reviewer's own blocker/wall.
///
/// `simultaneous_revise_keeps_both_entries` proves the SAME round's TWO
/// per-verdict appends both survive — the only shape that can catch a
/// `park-report-clear` step silently re-running between them, which would
/// silently drop whichever verdict's entry landed first.
fn dual_review_trait_toml(id: &str, max_iterations: u32) -> String {
    format!(
        r#"id = "{id}"
schema-version = "0.2"
version = "0.1.0"
name = "Park Honesty Dual-Review Fixture"
summary = "P461 park-honesty proof fixture: two independent reviewers append/clear the same park-report slot every round."

[[resource]]
id = "execution-plan"
path = ".plans/EXECUTION_PLAN.md"
root = "repo"
trigger = "on-demand"

[[port]]
id = "phase"
direction = "input"
schema = "schema:text"
description = "Phase or group, exactly as named in .plans/EXECUTION_PLAN.md."

[[port]]
id = "park-report"
direction = "output"
schema = "[schema:review-verdict]"
optional = true
description = "Typed park record: present only when the dual-review loop exhausted without both verdicts approved."
value = "slot:park-report"

[[slot]]
id = "work-summary"
schema = "schema:text"
description = "Fixture worker output."

[[slot]]
id = "review-verdict-1"
schema = "schema:review-verdict"
description = "smart-1's independent verdict."

[[slot]]
id = "review-verdict-2"
schema = "schema:review-verdict"
description = "smart-2's independent verdict."

[[slot]]
id = "park-report"
schema = "[schema:review-verdict]"
description = "This round's still-revise verdict(s) from either reviewer; cleared and re-appended every round."

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

[[slot]]
id = "commit-output"
schema = "schema:text"
{SCHEMA_BLOCKS}

[[agent]]
id = "smart-1"
description = "Fixture reviewer one."
summary = "Reviewer role."

[[agent]]
id = "smart-2"
description = "Fixture reviewer two."
summary = "Reviewer role."
{WORKER_SCRIBE_AGENTS_AND_COMMIT_TAIL}

[prompt.review-1]
text = "Review (verdict 1) {{port:phase}}: {{slot:work-summary}}."
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict-1"]

[prompt.review-2]
text = "Review (verdict 2) {{port:phase}}: {{slot:work-summary}}."
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict-2"]

[[sequence.park-report-record-1-then.sequence]]
id = "park-report-append-1"
title = "Park Report Append (verdict 1)"
kind = "project"
input = ["slot:review-verdict-1"]

[[sequence.park-report-record-1-then.sequence.projection]]
source = "slot:review-verdict-1"
destination = "slot:park-report"
operation = "append"

[[sequence.park-report-record-1-then.sequence.output]]
slot = "slot:park-report"
operation = "append"

[[sequence.park-report-record-2-then.sequence]]
id = "park-report-append-2"
title = "Park Report Append (verdict 2)"
kind = "project"
input = ["slot:review-verdict-2"]

[[sequence.park-report-record-2-then.sequence.projection]]
source = "slot:review-verdict-2"
destination = "slot:park-report"
operation = "append"

[[sequence.park-report-record-2-then.sequence.output]]
slot = "slot:park-report"
operation = "append"

[[sequence.review-loop-dual.sequence]]
id = "review-1"
title = "Review work (verdict 1)"
agent = "agent:smart-1"
prompt = "prompt:review-1"
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict-1"]

[[sequence.review-loop-dual.sequence]]
id = "review-2"
title = "Review work (verdict 2)"
agent = "agent:smart-2"
prompt = "prompt:review-2"
input = ["port:phase", "slot:work-summary"]
output = ["slot:review-verdict-2"]

[[sequence.review-loop-dual.sequence]]
id = "park-report-clear"
title = "Park Report Clear"
kind = "project"
output = ["slot:park-report"]

[[sequence.review-loop-dual.sequence.projection]]
destination = "slot:park-report"

[sequence.review-loop-dual.sequence.projection.source]
literal = []

[[sequence.review-loop-dual.sequence]]
id = "park-report-record-1"
title = "Park Report Record (verdict 1)"
kind = "branch"
sequence = "sequence:park-report-record-1-then"

[sequence.review-loop-dual.sequence.when]
slot = "slot:review-verdict-1"
field = "status"
equals = "revise"

[[sequence.review-loop-dual.sequence]]
id = "park-report-record-2"
title = "Park Report Record (verdict 2)"
kind = "branch"
sequence = "sequence:park-report-record-2-then"

[sequence.review-loop-dual.sequence.when]
slot = "slot:review-verdict-2"
field = "status"
equals = "revise"

[procedure]
description = "Implement, review with two independent reviewers in a bounded loop, then commit when the tree is dirty."
input = ["port:phase"]
output = ["port:park-report"]

[[procedure.sequence]]
id = "implement"
title = "Implement"
agent = "agent:worker"
prompt = "prompt:implement"
input = ["port:phase"]
output = ["slot:work-summary"]

[[procedure.sequence]]
id = "dual-review"
title = "Dual reviewed pass"
kind = "loop"
sequence = "sequence:review-loop-dual"
max-iterations = {max_iterations}
on-exhausted = "block"

[[procedure.sequence.until.all]]
slot = "slot:review-verdict-1"
field = "status"
equals = "approved"

[[procedure.sequence.until.all]]
slot = "slot:review-verdict-2"
field = "status"
equals = "approved"

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

/// Drive one dual-review fixture phase to exhaustion (both reviewers always
/// revise or are otherwise unable to reach mutual approval within
/// `max_iterations = 2`) and return the persisted ledger text.
fn drive_dual_review_phase(
    repo: &Path,
    home: &Path,
    runs_root: &Path,
    trait_id: &str,
    phase: &str,
    extra_env: &[(&str, &str)],
) -> String {
    let ledger_path = runs_root.join(format!("park-honesty-dual-{}.json", phase.to_lowercase()));
    require_success_with_env(
        &format!("{phase} (dual-review) `ctx traits run`"),
        &[
            "traits",
            "run",
            trait_id,
            "--set",
            &format!("phase={phase}"),
            "--out",
            ledger_path.to_str().unwrap(),
            "--max-frames",
            "20",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "120",
            "--json",
        ],
        repo,
        home,
        extra_env,
    );
    let ledger_text = fs::read_to_string(&ledger_path).expect("dual-review ledger readable");
    assert!(
        ledger_text.contains("\"final-state\": \"blocked\""),
        "{phase} must reach final-state blocked (both reviewers never simultaneously approve):\n{ledger_text}"
    );
    ledger_text
}

/// The persisted, accepted `slot:park-report` value — a `[schema:review-verdict]`
/// list — read out of the ledger's own `accepted-slot-values` evidence,
/// never off a whole-document substring match: `slot:review-verdict-1`/
/// `slot:review-verdict-2` carry the SAME blocker/wall text independently
/// of whether the park-report clear/append primitive actually kept it, so
/// a substring check against the whole ledger cannot tell "the reviewer
/// said it" apart from "the report kept it".
fn park_report_entries(ledger_text: &str) -> Vec<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(ledger_text).expect("ledger is valid JSON");
    doc["accepted-slot-values"]
        .as_array()
        .expect("ledger carries accepted-slot-values")
        .iter()
        .find(|entry| entry["ref-text"] == "slot:park-report")
        .map(|entry| {
            entry["value"]
                .as_array()
                .cloned()
                .expect("slot:park-report's accepted value is a list")
        })
        .unwrap_or_default()
}

/// Assert the park report holds exactly one entry, carrying `expected_wall`
/// and a blocker named `expected_blocker` — structural fields, read off the
/// isolated park-report value, never a substring match.
fn assert_single_park_entry(ledger_text: &str, expected_wall: &str, expected_blocker: &str) {
    let entries = park_report_entries(ledger_text);
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one park-report entry:\n{entries:#?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["wall-id"], expected_wall,
        "park-report entry's wall-id does not match:\n{entry:#?}"
    );
    let blocker_ids: Vec<&str> = entry["blockers"]
        .as_array()
        .expect("park-report entry carries blockers")
        .iter()
        .map(|blocker| blocker["id"].as_str().expect("blocker id is a string"))
        .collect();
    assert_eq!(
        blocker_ids,
        vec![expected_blocker],
        "park-report entry's blocker id does not match:\n{entry:#?}"
    );
}

/// Assert the park report holds exactly `expected.len()` entries, one per
/// `(wall, blocker)` pair, each pair found somewhere in the isolated
/// park-report value regardless of append order.
fn assert_park_entries(ledger_text: &str, expected: &[(&str, &str)]) {
    let entries = park_report_entries(ledger_text);
    assert_eq!(
        entries.len(),
        expected.len(),
        "expected {} park-report entries:\n{entries:#?}",
        expected.len()
    );
    for (wall, blocker) in expected {
        let found = entries.iter().any(|entry| {
            entry["wall-id"] == *wall
                && entry["blockers"]
                    .as_array()
                    .is_some_and(|blockers| blockers.iter().any(|b| b["id"] == *blocker))
        });
        assert!(
            found,
            "park report missing entry wall={wall:?} blocker={blocker:?}:\n{entries:#?}"
        );
    }
}

#[test]
fn dual_review_names_only_the_blocking_reviewer() {
    let id = "implement-fixture-park-dual-single";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-dual-single",
        id,
        &dual_review_trait_toml(id, 2),
        &[("P970", None)],
    );
    let runs_root = global_runs_root(&repo, &home);
    fs::create_dir_all(&runs_root).unwrap();

    // smart-1 always revises, citing its own wall/blocker; smart-2 always
    // approves. Only smart-1's entry may appear in the park report.
    let ledger_text = drive_dual_review_phase(
        &repo,
        &home,
        &runs_root,
        id,
        "P970",
        &[
            ("CTX_FIXTURE_REVIEWER1_MODE", "revise"),
            ("CTX_FIXTURE_REVIEWER1_WALL", "WALL-P461-DUAL-SINGLE"),
            (
                "CTX_FIXTURE_REVIEWER1_BLOCKER",
                "fixture-verdict-1-only-defect",
            ),
            ("CTX_FIXTURE_REVIEWER2_MODE", "approve"),
        ],
    );
    assert_single_park_entry(
        &ledger_text,
        "WALL-P461-DUAL-SINGLE",
        "fixture-verdict-1-only-defect",
    );

    let _ = scratch;
}

#[test]
fn dual_review_simultaneous_revise_keeps_both_entries() {
    let id = "implement-fixture-park-dual-both";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-dual-both",
        id,
        &dual_review_trait_toml(id, 2),
        &[("P971", None)],
    );
    let runs_root = global_runs_root(&repo, &home);
    fs::create_dir_all(&runs_root).unwrap();

    // Both reviewers revise, in the SAME round, each citing a distinct
    // blocker/wall. A `park-report-clear` that silently re-ran between the
    // two per-verdict appends would drop whichever landed first; this is
    // the only fixture shape that can catch that.
    let ledger_text = drive_dual_review_phase(
        &repo,
        &home,
        &runs_root,
        id,
        "P971",
        &[
            ("CTX_FIXTURE_REVIEWER1_MODE", "revise"),
            ("CTX_FIXTURE_REVIEWER1_WALL", "WALL-P461-DUAL-BOTH-A"),
            ("CTX_FIXTURE_REVIEWER1_BLOCKER", "fixture-both-defect-one"),
            ("CTX_FIXTURE_REVIEWER2_MODE", "revise"),
            ("CTX_FIXTURE_REVIEWER2_WALL", "WALL-P461-DUAL-BOTH-B"),
            ("CTX_FIXTURE_REVIEWER2_BLOCKER", "fixture-both-defect-two"),
        ],
    );
    assert_park_entries(
        &ledger_text,
        &[
            ("WALL-P461-DUAL-BOTH-A", "fixture-both-defect-one"),
            ("WALL-P461-DUAL-BOTH-B", "fixture-both-defect-two"),
        ],
    );

    let _ = scratch;
}
