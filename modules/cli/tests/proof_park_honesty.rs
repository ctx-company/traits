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
//! - `on-exhausted = "abort"`: the loop's own native abort behavior
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
//! `core.excludesFile`, which once silently decided whether fixture board
//! files landed in the fixture repo's first commit on a machine whose
//! personal global gitignore happens to exclude them.

use std::fs;
use std::path::Path;

use support::{
    ScratchRoot, fixture_agent_bin, git_init, require_success, require_success_with_env, run_ctx,
    run_git, utf8,
};

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
id = "repo-gates-result"

[schema.fields.argv]
schema = "[schema:text]"
required = true

[schema.fields.ok]
schema = "schema:boolean"
required = true

[schema.fields.timed-out]
schema = "schema:boolean"

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
text = "Implement {port:task}."
input = ["port:task"]
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
/// Every dispatch in this suite passes `--task-dispatch` — wall preflight
/// participation is explicitly requested per dispatch
/// (`modules/io/src/dispatch_preflight.rs`), never inferred from the trait
/// id.
fn fixture_trait_toml(id: &str, max_iterations: u32, on_exhausted: &str) -> String {
    format!(
        r#"id = "{id}"
schema-version = "0.2"
version = "0.1.0"
name = "Park Honesty Fixture"
summary = "P461 park-honesty proof fixture: a single-review loop whose on-exhausted policy is the fixture's own axis."

[[resource]]
id = "task-board"
path = ".internal/tasks"
root = "repo"
trigger = "on-demand"

[[port]]
id = "task"
direction = "input"
schema = "schema:text"
description = "Task to implement, named by its file in .internal/tasks/."

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
id = "repo-gates-passed"
schema = "schema:repo-gates-result"
description = "Deterministic gate evidence used by the guarded exit and stop paths."

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
text = "Review {{port:task}}: {{slot:work-summary}}."
input = ["port:task", "slot:work-summary"]
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
input = ["port:task", "slot:work-summary"]
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

[[sequence.review-loop.sequence]]
id = "repo-gates"
title = "Record repository gate"
kind = "project"
output = ["slot:repo-gates-passed"]

[[sequence.review-loop.sequence.projection]]
destination = "slot:repo-gates-passed"

[sequence.review-loop.sequence.projection.source]
literal = {{ argv = ["fixture-gate"], ok = true, timed-out = false }}

[procedure]
description = "Implement, review in a bounded loop, then commit when the tree is dirty."
input = ["port:task"]
output = ["port:park-report"]

[[procedure.sequence]]
id = "implement"
title = "Implement"
agent = "agent:worker"
prompt = "prompt:implement"
input = ["port:task"]
output = ["slot:work-summary"]

[[procedure.sequence]]
id = "single-review"
title = "Single reviewed pass"
kind = "loop"
sequence = "sequence:review-loop"
max-iterations = {max_iterations}
on-exhausted = "{on_exhausted}"

[[procedure.sequence.until.all]]
slot = "slot:review-verdict"
field = "status"
equals = "approved"

[[procedure.sequence.until.all]]
slot = "slot:repo-gates-passed"
field = "ok"
equals = true

[procedure.sequence.abort-if]
slot = "slot:repo-gates-passed"
field = "timed-out"
equals = true

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
/// [`dual_review_trait_toml`]), and one `.internal/tasks/<task>.toml`
/// canonical task document per `(task, wall)` pair — reviewed and ready to
/// drive. The board is committed here, under the controlled environment
/// (never the invoking machine's own `~/.gitconfig`), so whether it lands
/// in the fixture's tracked set never depends on ambient developer config.
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
        "/.ctx/runs/*.json\n/.ctx/traits/worktrees/\n/.ctx/debug/\n",
    );
    for (task, wall) in phases {
        let content = "Why: fixture.\nApproach: fixture.\n".to_string();
        let validation = "## Done when\n\nFixture gates pass.\n".to_string();
        let wall_line = wall
            .map(|wall| format!("wall = \"{wall}\"\n"))
            .unwrap_or_default();
        let body = format!(
            "schema-version = \"0.2\"\nkey = \"{task}\"\ntitle = \"Park-honesty fixture task\"\nstatus = \"ready\"\n{wall_line}\ncontent = \"\"\"\n{content}\"\"\"\nvalidation = \"\"\"\n{validation}\"\"\"\n"
        );
        write_file(&repo.join(format!(".internal/tasks/{task}.toml")), &body);
    }
    commit_all(&repo, &home, "initial commit");

    require_success("`ctx traits init`", &["traits", "init"], &repo, &home);

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
/// `on-exhausted = "abort"` path — `Status::Blocked`, no commit, and a
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
        &fixture_trait_toml(id, 2, "abort"),
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
            "--task-dispatch",
            "--set",
            "task=P900",
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
            "--task-dispatch",
            "--set",
            "task=P901",
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
            "--task-dispatch",
            "--set",
            "task=P950",
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
id = "task-board"
path = ".internal/tasks"
root = "repo"
trigger = "on-demand"

[[port]]
id = "task"
direction = "input"
schema = "schema:text"
description = "Task to implement, named by its file in .internal/tasks/."

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
text = "Review (verdict 1) {{port:task}}: {{slot:work-summary}}."
input = ["port:task", "slot:work-summary"]
output = ["slot:review-verdict-1"]

[prompt.review-2]
text = "Review (verdict 2) {{port:task}}: {{slot:work-summary}}."
input = ["port:task", "slot:work-summary"]
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
input = ["port:task", "slot:work-summary"]
output = ["slot:review-verdict-1"]

[[sequence.review-loop-dual.sequence]]
id = "review-2"
title = "Review work (verdict 2)"
agent = "agent:smart-2"
prompt = "prompt:review-2"
input = ["port:task", "slot:work-summary"]
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
input = ["port:task"]
output = ["port:park-report"]

[[procedure.sequence]]
id = "implement"
title = "Implement"
agent = "agent:worker"
prompt = "prompt:implement"
input = ["port:task"]
output = ["slot:work-summary"]

[[procedure.sequence]]
id = "dual-review"
title = "Dual reviewed pass"
kind = "loop"
sequence = "sequence:review-loop-dual"
max-iterations = {max_iterations}
on-exhausted = "abort"

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
            "--task-dispatch",
            "--set",
            &format!("task={phase}"),
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

/// Submit one deterministic agent frame through the public call boundary.
/// Keeping this below the fixture helpers makes the guarded-loop regression
/// exercise the same persisted-session validation that caught the stale port
/// row, rather than an in-process transition shortcut.
fn call_fixture_frame(
    repo: &Path,
    home: &Path,
    ledger_path: &Path,
    agent: &str,
    produced_slots: serde_json::Value,
) {
    let ledger: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ledger_path).expect("fixture session ledger readable"),
    )
    .expect("fixture session ledger is JSON");
    let template = &ledger["next-frame"]["call-template"];
    let mut data = serde_json::json!({
        "session-id": template["session-id"],
        "run-id": template["run-id"],
        "state-digest": template["state-digest"],
        "expected-sequence-item-id": template["expected-sequence-item-id"],
        "expected-run-index": template["expected-run-index"],
        "expected-source-index": template["expected-source-index"],
        "produced-slots": produced_slots,
    });
    if let Some(position_path) = template["expected-position-path"].as_array() {
        data.as_object_mut()
            .expect("fixture call payload is an object")
            .insert(
                "expected-position-path".to_string(),
                position_path.clone().into(),
            );
    }
    let data_path = ledger_path.with_extension(format!("{agent}.call.json"));
    fs::write(&data_path, data.to_string()).expect("fixture call payload writable");
    let output = run_ctx(
        &[
            "traits",
            "call",
            "--session",
            ledger_path.to_str().unwrap(),
            "--agent",
            agent,
            "--data",
            data_path.to_str().unwrap(),
            "--json",
        ],
        repo,
        home,
    );
    assert!(
        output.status.success(),
        "{agent} call must preserve the output-port ledger contract: {:?}",
        utf8(&output)
    );
}

/// A quick-shaped guarded exit writes the optional park-report port through a
/// project, then blocks at loop exhaustion. Before the refresh at the
/// terminal control-flow boundary, the persisted session still held the
/// entry-time output-port snapshot and `ctx traits call` rejected it.
#[test]
fn guarded_exhaustion_refreshes_project_written_park_report_output() {
    let id = "implement-fixture-park-guarded-call";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-guarded-call",
        id,
        &fixture_trait_toml(id, 1, "abort"),
        &[("P980", None)],
    );
    let ledger_path = scratch.path().join("guarded-call.json");
    require_success(
        "guarded fixture `ctx traits run --no-drive`",
        &[
            "traits",
            "run",
            id,
            "--task-dispatch",
            "--set",
            "task=P980",
            "--no-drive",
            "--out",
            ledger_path.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "worker",
        serde_json::json!({ "slot:work-summary": "implemented" }),
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "smart-1",
        serde_json::json!({
            "slot:review-verdict": {
                "status": "revise",
                "blockers": [],
                "wall-id": "WALL-P461-GUARDED",
            },
        }),
    );

    let ledger_text = fs::read_to_string(&ledger_path).expect("guarded ledger readable");
    let ledger: serde_json::Value =
        serde_json::from_str(&ledger_text).expect("guarded ledger is JSON");
    assert_eq!(ledger["ledger"]["final-state"], "blocked", "{ledger:#?}");
    let park_slot = ledger["ledger"]["accepted-slot-values"]
        .as_array()
        .expect("guarded ledger carries accepted slot values")
        .iter()
        .find(|value| value["ref-text"] == "slot:park-report")
        .expect("guarded loop records slot:park-report");
    let park_port = ledger["ledger"]["output-ports"]
        .as_array()
        .expect("guarded ledger carries output ports")
        .iter()
        .find(|value| value["port-ref"] == "port:park-report")
        .expect("guarded loop accepts port:park-report");
    assert_eq!(park_port["status"], "accepted");
    assert_eq!(park_port["value-digest"], park_slot["value-digest"]);

    let _ = scratch;
}

/// The same write can be followed by quick's distinct timed-out stop guard.
/// Its terminal return must also retain the derived park-report port row.
#[test]
fn guarded_timed_out_stop_refreshes_project_written_park_report_output() {
    let id = "implement-fixture-park-guarded-stop";
    let trait_toml =
        fixture_trait_toml(id, 1, "abort").replace("timed-out = false", "timed-out = true");
    let (scratch, repo, home) =
        setup_fixture("p461-park-guarded-stop", id, &trait_toml, &[("P981", None)]);
    let ledger_path = scratch.path().join("guarded-stop.json");
    require_success(
        "timed-out fixture `ctx traits run --no-drive`",
        &[
            "traits",
            "run",
            id,
            "--task-dispatch",
            "--set",
            "task=P981",
            "--no-drive",
            "--out",
            ledger_path.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "worker",
        serde_json::json!({ "slot:work-summary": "implemented" }),
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "smart-1",
        serde_json::json!({
            "slot:review-verdict": {
                "status": "revise",
                "blockers": [],
                "wall-id": "WALL-P461-TIMED-OUT",
            },
        }),
    );

    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger_path).expect("timed-out ledger readable"))
            .expect("timed-out ledger is JSON");
    assert_eq!(ledger["ledger"]["final-state"], "blocked", "{ledger:#?}");
    let park_slot = ledger["ledger"]["accepted-slot-values"]
        .as_array()
        .expect("timed-out ledger carries accepted slot values")
        .iter()
        .find(|value| value["ref-text"] == "slot:park-report")
        .expect("timed-out stop records slot:park-report");
    let park_port = ledger["ledger"]["output-ports"]
        .as_array()
        .expect("timed-out ledger carries output ports")
        .iter()
        .find(|value| value["port-ref"] == "port:park-report")
        .expect("timed-out stop accepts port:park-report");
    assert_eq!(park_port["status"], "accepted");
    assert_eq!(park_port["value-digest"], park_slot["value-digest"]);

    let _ = scratch;
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

/// 0049 correction repro: the dual-reviewer loop body clears
/// `slot:park-report`, then conditionally appends it TWICE in the same
/// transition (`record-1`'s branch, then `record-2`'s branch) before the
/// loop's own exhaustion check makes the run terminal. Driven step by step
/// through the persisted-ledger `ctx traits call` boundary (the same
/// boundary `guarded_exhaustion_refreshes_project_written_park_report_output`
/// already regression-tests for a SINGLE append), smart-2's call is the one
/// transition that must refresh `output-ports` after BOTH conditional
/// appends land, not just the first.
#[test]
fn dual_review_call_boundary_refreshes_both_park_report_appends() {
    let id = "implement-fixture-park-dual-call";
    let (scratch, repo, home) = setup_fixture(
        "p461-park-dual-call",
        id,
        &dual_review_trait_toml(id, 1),
        &[("P972", None)],
    );
    let ledger_path = scratch.path().join("dual-call.json");
    require_success(
        "dual-review fixture `ctx traits run --no-drive`",
        &[
            "traits",
            "run",
            id,
            "--task-dispatch",
            "--set",
            "task=P972",
            "--no-drive",
            "--out",
            ledger_path.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "worker",
        serde_json::json!({ "slot:work-summary": "implemented" }),
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "smart-1",
        serde_json::json!({
            "slot:review-verdict-1": {
                "status": "revise",
                "blockers": [{
                    "id": "fixture-both-defect-one",
                    "where": "fixture",
                    "what": "fixture blocker one",
                    "root-cause": "fixture",
                    "required-fix": "fixture",
                    "done-when": "fixture",
                }],
                "wall-id": "WALL-P461-DUAL-CALL-A",
            },
        }),
    );
    call_fixture_frame(
        &repo,
        &home,
        &ledger_path,
        "smart-2",
        serde_json::json!({
            "slot:review-verdict-2": {
                "status": "revise",
                "blockers": [{
                    "id": "fixture-both-defect-two",
                    "where": "fixture",
                    "what": "fixture blocker two",
                    "root-cause": "fixture",
                    "required-fix": "fixture",
                    "done-when": "fixture",
                }],
                "wall-id": "WALL-P461-DUAL-CALL-B",
            },
        }),
    );

    let ledger_text = fs::read_to_string(&ledger_path).expect("dual-call ledger readable");
    let ledger: serde_json::Value =
        serde_json::from_str(&ledger_text).expect("dual-call ledger is JSON");
    assert_eq!(ledger["ledger"]["final-state"], "blocked", "{ledger:#?}");
    assert_park_entries(
        &ledger_text,
        &[
            ("WALL-P461-DUAL-CALL-A", "fixture-both-defect-one"),
            ("WALL-P461-DUAL-CALL-B", "fixture-both-defect-two"),
        ],
    );

    let _ = scratch;
}
