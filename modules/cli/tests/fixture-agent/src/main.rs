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
//! Only the roles consuming proof suites actually need are implemented — no
//! speculative roles. The concurrency slice (P461 deferred slice 2) extends
//! this same binary with marker/wait/fail worker roles rather than forking a
//! second one; 0153 (`proof_research_family.rs`) adds the `planner` role the
//! same way, for the research family's typed-plan cardinality gate.

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
            Some("worker") => role_worker(args.next()),
            Some("reviewer") => role_reviewer(args.next(), args.next()),
            Some("planner") => role_planner(args.next()),
            Some("scribe") => role_scribe(),
            Some("generator") => role_generator(args.next()),
            Some("refiner") => role_refiner(args.next()),
            Some("explainer") => role_explainer(args.next()),
            Some("proposer") => role_proposer(args.next()),
            Some("smart1") => role_smart1(args.next()),
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
///
/// 0163's `optimize` family binds this same `worker` agent role to two
/// distinct prompts (the readiness preflight and the per-round apply
/// step), each with a different output slot/schema. Rather than adding a
/// second harness role, this dispatches on the requested output field:
/// `readiness` (a typed status/detail object, honoring
/// `CTX_FIXTURE_READINESS_STATUS`, default `ready`) versus everything else
/// (the marker-file touch plus a plain text receipt).
fn role_worker(prompt: Option<String>) -> ExitCode {
    let prompt = prompt.unwrap_or_default();
    let field = requested_output_field(&prompt).unwrap_or_else(|| "work-summary".to_string());
    if field == "readiness" {
        let status =
            env::var("CTX_FIXTURE_READINESS_STATUS").unwrap_or_else(|_| "ready".to_string());
        let detail = if status == "ready" {
            "fixture workbench ready"
        } else {
            "fixture-forced abort"
        };
        println!("{{\"{field}\":{{\"status\":\"{status}\",\"detail\":\"{detail}\"}}}}");
        return ExitCode::SUCCESS;
    }
    let marker = "fixture-work-output.txt";
    let previous = fs::read_to_string(marker).unwrap_or_default();
    let updated = format!("{previous}fixture worker touch\n");
    if let Err(error) = fs::write(marker, updated) {
        eprintln!("ctx-fixture-agent: cannot write {marker}: {error}");
        return ExitCode::FAILURE;
    }
    println!("{{\"{field}\":\"Fixture work summary.\"}}");
    ExitCode::SUCCESS
}

/// 0163's `optimize:experiment` proposer seat: always returns the same
/// fixed, valid proposal — the guard mechanism this proof suite exercises
/// (keep/discard/aggregation/iteration-cap/baseline-immutability) never
/// depends on proposal content, only on the trusted measurement command's
/// output, so a static proposal is the port-faithful fixture choice.
fn role_proposer(prompt: Option<String>) -> ExitCode {
    let field = requested_output_field(&prompt.unwrap_or_default())
        .unwrap_or_else(|| "proposal".to_string());
    println!(
        "{{\"{field}\":{{\"title\":\"fixture experiment\",\"hypothesis\":\"fixture hypothesis\",\"change\":\"fixture bounded change\"}}}}"
    );
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
            r#"{{"status":"revise","blockers":[{{"id":"{blocker_id}","where":"fixture-work-output.txt","what":"fixture-only defect for the P461 park-honesty proof","root-cause":"the fixture reviewer never fixes it, so the loop always revises","required-fix":"n/a — this fixture proves park-report/exhaustion behavior, not a real fix","steps":[{{"step":"n/a — this fixture always revises","status":"open"}}],"done-when":"never — this fixture always revises"}}],"wall-id":"{wall}"}}"#
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

/// 0153's research-family cardinality-gate proof: always returns the same
/// fixed typed list, read verbatim (already valid JSON) from
/// `CTX_FIXTURE_PLANNER_STREAMS` — a deliberately STATIC plan, since the
/// gate this proves ("does the loop accept/reject this exact plan") never
/// needs the plan to vary round over round: an always-invalid plan proves
/// exhaustion, an always-valid one proves the gate accepts round 1.
fn role_planner(prompt: Option<String>) -> ExitCode {
    let field = requested_output_field(&prompt.unwrap_or_default())
        .unwrap_or_else(|| "stream-plan".to_string());
    let streams = env::var("CTX_FIXTURE_PLANNER_STREAMS").unwrap_or_else(|_| "[]".to_string());
    println!("{{\"{field}\":{streams}}}");
    ExitCode::SUCCESS
}

/// 0163's `optimize:benchmark` single `smart-1` agent role is bound to
/// three prompts (scope/draft: plain text; review: a typed verdict), so —
/// same reasoning as `role_worker`'s dispatch — this reads the requested
/// output field to decide: `review-verdict` delegates to the same verdict
/// logic as [`role_reviewer`] (slot `"1"`), everything else is a fixed
/// plain-text receipt.
fn role_smart1(prompt: Option<String>) -> ExitCode {
    let prompt = prompt.unwrap_or_default();
    let field = requested_output_field(&prompt).unwrap_or_else(|| "text".to_string());
    if field == "review-verdict" {
        return role_reviewer(Some("1".to_string()), Some(prompt));
    }
    println!("{{\"{field}\":\"Fixture smart-1 text.\"}}");
    ExitCode::SUCCESS
}

fn role_scribe() -> ExitCode {
    println!("{{\"commit-message\":\"P461 park-honesty proof fixture commit.\"}}");
    ExitCode::SUCCESS
}

/// `generate-trait`'s and `import-trait`'s produce/revise steps (both
/// declare an agent literally named `generator`; task 0066.1/0066.3's
/// guarded loops): always returns the same deliberately invalid text, so
/// every round fails at the ladder's earliest rung (`build` for generate's
/// TypeScript candidate, `synth-normalize` for import's JSON candidate) and
/// the loop's declared bound genuinely exhausts — proving the
/// non-convergence path (no package write, failing rung named, scratch
/// preserved) rather than a fixture escalation branch. The output field is
/// read off the compiled prompt's own return-format skeleton (see
/// `requested_output_field`) rather than hard-coded, since generate's
/// candidate lands in slot `candidate-source` and import's in slot
/// `candidate`.
/// 0066.4's bound-kill fixtures reuse this role rather than adding new ones:
/// `CTX_FIXTURE_GENERATOR_SLEEP_MS` blocks before answering at all (proves a
/// frame-seconds kill), `CTX_FIXTURE_GENERATOR_CANDIDATE` substitutes the
/// returned candidate source (proves a command-idle-seconds kill, once the
/// substituted source hangs the CDK build step that consumes it) — both
/// no-ops when unset, so every other consumer of this role is unaffected.
fn role_generator(prompt: Option<String>) -> ExitCode {
    if let Ok(millis) = env::var("CTX_FIXTURE_GENERATOR_SLEEP_MS")
        && let Ok(millis) = millis.parse::<u64>()
    {
        std::thread::sleep(std::time::Duration::from_millis(millis));
    }
    let field = requested_output_field(&prompt.unwrap_or_default())
        .unwrap_or_else(|| "candidate-source".to_string());
    let candidate = match env::var("CTX_FIXTURE_GENERATOR_CANDIDATE") {
        Ok(source) => json_string(&source),
        Err(_) => invalid_candidate_json(),
    };
    println!("{{\"{field}\":{candidate}}}");
    ExitCode::SUCCESS
}

/// `refine-trait`'s produce/revise steps (task 0066.3's guarded loop):
/// always returns the same deliberately invalid text in slot `candidate`, so
/// every round fails at the ladder's `synth-normalize` rung and the loop's
/// declared bound genuinely exhausts.
fn role_refiner(prompt: Option<String>) -> ExitCode {
    let field = requested_output_field(&prompt.unwrap_or_default())
        .unwrap_or_else(|| "candidate".to_string());
    println!("{{\"{field}\":{}}}", invalid_candidate_json());
    ExitCode::SUCCESS
}

/// `explain-trait`'s single narration step (task 0124): echoes the
/// deterministic scaffold embedded in the compiled prompt verbatim, adding
/// only a fixed advisory `explanation` string, so the CLI's grounding gate
/// (`evidence_matches`) sees an unaltered echo and the run converges to a
/// gated narrated explanation without a live provider.
fn role_explainer(prompt: Option<String>) -> ExitCode {
    let prompt = prompt.unwrap_or_default();
    let field = requested_output_field(&prompt).unwrap_or_else(|| "explain-trait".to_string());
    // The compiled prompt reindents every line of interpolated multi-line
    // template text (task 0046: prompt wrap never dedents), so the markers'
    // surrounding newlines carry extra leading whitespace — search for the
    // bare marker tokens and trim the extracted content instead of matching
    // an exact `\n`-adjacent marker.
    let Some(scaffold) = extract_between(&prompt, "<<<SCAFFOLD>>>", "<<<END SCAFFOLD>>>") else {
        eprintln!("ctx-fixture-agent: explainer role could not find scaffold markers in prompt");
        return ExitCode::FAILURE;
    };
    let trimmed = scaffold.trim();
    let Some(insert_at) = trimmed.rfind('}') else {
        eprintln!("ctx-fixture-agent: explainer role scaffold is not a JSON object");
        return ExitCode::FAILURE;
    };
    let narrated = format!(
        "{}{}{}",
        &trimmed[..insert_at],
        r#","explanation":"Fixture-narrated explanation, grounded strictly in the supplied scaffold."}"#,
        &trimmed[insert_at + 1..],
    );
    println!("{{\"{field}\":{narrated}}}");
    ExitCode::SUCCESS
}

/// The substring strictly between `start_marker` and `end_marker`'s first
/// occurrence in `text`, or `None` if either marker is absent.
fn extract_between<'a>(text: &'a str, start_marker: &str, end_marker: &str) -> Option<&'a str> {
    let start = text.find(start_marker)? + start_marker.len();
    let end = text[start..].find(end_marker)? + start;
    Some(&text[start..end])
}

/// Deliberately invalid candidate text, JSON-string-encoded: fails to parse
/// as either TypeScript (generate's build rung) or JSON (refine/import's
/// synth-normalize rung), so it never converges regardless of which loop
/// evaluates it. Hand-escaped, not a dependency: this binary is deliberately
/// std-only.
fn invalid_candidate_json() -> String {
    json_string("this is not valid TypeScript or JSON {{{")
}

/// JSON-string-encode `source`, hand-escaped (this binary is deliberately
/// std-only, no `serde_json` dependency).
fn json_string(source: &str) -> String {
    let escaped = source
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}
