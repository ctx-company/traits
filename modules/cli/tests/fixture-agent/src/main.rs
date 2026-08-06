//! P461 slices 1-2: a multi-role stub CLI harness, replacing byte_compare's
//! self-exec fixture roles (`--fixture-p414-worker`, `--fixture-dual-review-clerk`,
//! ...) with one std-only dev binary selected by `--role`. `ctx`'s harness
//! dispatch never clears its own process environment before spawning a
//! custom-kind harness child (`modules/io/src/harness.rs`), so the proof
//! suite configures a reviewer role's verdict by setting `CTX_FIXTURE_*`
//! environment variables on the `ctx traits run` invocation itself (via
//! `support::run_ctx_with_env`) rather than by baking per-scenario argv into
//! the fixture's `ctx.toml` — the same `ctx.toml` and role wiring serves
//! every scenario. The runtime appends the resolved prompt as this
//! process's own final argv element (`PromptDelivery::Arg`, the harness
//! convention's default); it is read but never inspected, since each role
//! here is dedicated to exactly one prompt.
//!
//! Only the roles P461 slices 1-2 consume are implemented — no speculative
//! roles. The concurrency slice (P461 deferred slice 2) extends this same
//! binary with marker/wait/fail worker roles rather than forking a second
//! one.

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--probe") => {
            println!("ctx-fixture-agent-1.0");
            ExitCode::SUCCESS
        }
        Some("--role") => match args.next().as_deref() {
            Some("worker") => role_worker(),
            Some("reviewer") => role_reviewer(args.next(), args.next()),
            Some("scribe") => role_scribe(),
            Some("generator") => role_generator(),
            other => {
                eprintln!("ctx-fixture-agent: unknown --role {other:?}");
                ExitCode::FAILURE
            }
        },
        other => {
            eprintln!("ctx-fixture-agent: unrecognized invocation (first arg {other:?})");
            ExitCode::FAILURE
        }
    }
}

/// Touches a fixed file in the current directory (the harness's own
/// `exec_dir`, i.e. the run/worktree checkout — `modules/io/src/harness.rs`'s
/// `apply_exec_dir` sets the child's real process cwd there) so the run's
/// working tree is genuinely dirty by the time the trait's own
/// `check-git-status` step runs. Idempotent-enough for repeat calls within
/// one run: each call appends one more marker line.
fn role_worker() -> ExitCode {
    let marker = "fixture-work-output.txt";
    let previous = fs::read_to_string(marker).unwrap_or_default();
    let updated = format!("{previous}fixture worker touch\n");
    if let Err(error) = fs::write(marker, updated) {
        eprintln!("ctx-fixture-agent: cannot write {marker}: {error}");
        return ExitCode::FAILURE;
    }
    println!("{{\"work-summary\":\"Fixture work summary.\"}}");
    ExitCode::SUCCESS
}

/// Reviewer verdict, configured entirely by env vars set on the `ctx`
/// invocation (see module docs). `slot` selects which reviewer this
/// invocation is standing in for — `ctx.toml` wires one `stub-reviewer<N>`
/// harness per reviewer role, each passing its own fixed `N` as trailing
/// argv ahead of the runtime-appended prompt — so a dual-reviewer fixture
/// trait (two independent `agent.role.smart-*` bindings) can configure each
/// verdict independently via `CTX_FIXTURE_REVIEWER<N>_MODE`/`_WALL`/
/// `_BLOCKER`, never sharing one variable across two reviewers. `mode`
/// selects `approve` (empty blockers) or `revise` (one fixed fixture
/// blocker, citing `_WALL` verbatim — empty string when unset, which is
/// the schema's own "no wall cited" value, never inferred).
///
/// The output field name is read off `prompt`'s own "Requested outputs:"
/// line rather than hard-coded: the single-reviewer fixture trait's slot is
/// plain `review-verdict`, the dual-reviewer trait's are
/// `review-verdict-1`/`review-verdict-2`, and this one binary serves both
/// without either trait's `ctx.toml` role wiring needing to carry the
/// distinction.
fn role_reviewer(slot: Option<String>, prompt: Option<String>) -> ExitCode {
    let slot = slot.unwrap_or_else(|| "1".to_string());
    let field = requested_output_field(&prompt.unwrap_or_default())
        .unwrap_or_else(|| "review-verdict".to_string());
    let mode = env::var(format!("CTX_FIXTURE_REVIEWER{slot}_MODE")).unwrap_or_default();
    let wall = env::var(format!("CTX_FIXTURE_REVIEWER{slot}_WALL")).unwrap_or_default();
    let blocker_id = env::var(format!("CTX_FIXTURE_REVIEWER{slot}_BLOCKER"))
        .unwrap_or_else(|_| format!("fixture-defect-{slot}"));
    let verdict = match mode.as_str() {
        "approve" => r#"{"status":"approved","blockers":[],"wall-id":""}"#.to_string(),
        "revise" => format!(
            r#"{{"status":"revise","blockers":[{{"id":"{blocker_id}","where":"fixture-work-output.txt","what":"fixture-only defect for the P461 park-honesty proof","root-cause":"the fixture reviewer never fixes it, so the loop always revises","required-fix":"n/a — this fixture proves park-report/exhaustion behavior, not a real fix","done-when":"never — this fixture always revises"}}],"wall-id":"{wall}"}}"#
        ),
        other => {
            eprintln!("ctx-fixture-agent: unknown CTX_FIXTURE_REVIEWER{slot}_MODE {other:?}");
            return ExitCode::FAILURE;
        }
    };
    println!("{{\"{field}\":{verdict}}}");
    ExitCode::SUCCESS
}

/// The first output key from the frame's `<output><format>` skeleton, e.g.
/// `review-verdict-1` out of `{\n  "review-verdict-1": object\n}`. Reads the
/// format sketch rather than the retired `Requested outputs:` header, which
/// P561 removed along with the rest of the frame's runtime bookkeeping.
fn requested_output_field(prompt: &str) -> Option<String> {
    let format_start = prompt.find("<format>")? + "<format>".len();
    let format_end = prompt[format_start..].find("</format>")? + format_start;
    let block = &prompt[format_start..format_end];
    let key_start = block.find('"')? + 1;
    let rest = &block[key_start..];
    let key_end = rest.find('"')?;
    Some(rest[..key_end].to_string())
}

fn role_scribe() -> ExitCode {
    println!("{{\"commit-message\":\"P461 park-honesty proof fixture commit.\"}}");
    ExitCode::SUCCESS
}

/// `generate-trait`'s produce/revise steps (task 0066.1's guarded loop):
/// always returns the same authoring source, deliberately invalid
/// TypeScript, so every round fails at the ladder's `build` rung and the
/// loop's declared bound genuinely exhausts — proving the non-convergence
/// path (no package write, failing rung named, scratch preserved) rather
/// than a fixture escalation branch. Hand-escaped JSON string, not a
/// dependency: this binary is deliberately std-only.
fn role_generator() -> ExitCode {
    let source = "this is not valid TypeScript {{{";
    let escaped = source.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"candidate-source\":\"{escaped}\"}}");
    ExitCode::SUCCESS
}
