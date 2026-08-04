//! Provider-free public-path proofs for P464's correction-retry delivery
//! contract: a rejected/incomplete answer's retry either resumes the harness
//! conversation that produced it (when the harness convention declares a
//! session/resume flag AND this attempt observed a conversation id) or falls
//! back to a visible, reported cold start — never a silent one. Also proves
//! P464's frame-boundary guard: once the session has genuinely advanced past
//! the frame a retry targeted, drive abandons that in-loop retry instead of
//! resubmitting the old conversation against the new position.
//!
//! Both fixtures below drive the SAME two-step trait through the real core
//! `RejectedCorrectionRequired` branch, interleaving both rejection classes:
//!
//! 1. The worker step's first answer is deliberately schema-invalid (a
//!    string where `slot:answer` declares `schema:boolean`) — a real,
//!    persisted content rejection (`persist_session = true`).
//! 2. On the retry, the stub harness races ahead of drive: before replying,
//!    it shells out to `ctx traits set` directly and submits the CURRENT,
//!    schema-valid answer itself — advancing the session to the SECOND
//!    agent step out from under drive. Drive's own retry then submits
//!    against the now-stale call template it already built and gets a real
//!    non-persisting stale-identity rejection (`persist_session = false`)
//!    whose refreshed current frame is a genuinely different procedure
//!    position (the second step needs an agent dispatch, so the racer's own
//!    submission cannot auto-advance past it and complete the run outright —
//!    that would surface as a distinct `AlreadyCompleted` response instead
//!    of the `RejectedCorrectionRequired` this proof needs).
//!
//! The cold fixture's harness declares no `session-flag`; the resume
//! fixture's does. Both assert the exact retry argv/prompt recorded in the
//! `harness-run` events and the literal prompt text each stub invocation
//! received (captured via `prompt-via = "stdin"`), not just event evidence.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use support::{ScratchRoot, assert_exit_code, ctx_bin, git_init, require_success, run_ctx, utf8};

const TRAIT_CANONICAL: &str = r#"id = "fixture-p464"
schema-version = "0.2"
version = "0.1.0"
name = "Fixture P464"
summary = "P464 correction-retry proof fixture."

[[agent]]
id = "worker"
description = "Stub worker for the P464 correction-retry proof."
summary = "Fixture worker role."

[[slot]]
id = "answer"
schema = "schema:boolean"
description = "Fixture worker output; deliberately mismatched on the first attempt."

[[slot]]
id = "answer2"
schema = "schema:text"
description = "Second worker step's output, reached only once the first step's answer is accepted."

[procedure]
description = "Two worker steps in sequence."

[[procedure.sequence]]
id = "answer-step"
title = "Produce fixture answer"
agent = "agent:worker"
prompt = "Produce a fixture boolean answer."
output = ["slot:answer"]

[[procedure.sequence]]
id = "finish-step"
title = "Finish"
agent = "agent:worker"
prompt = "Produce a fixture text answer."
output = ["slot:answer2"]
"#;

const TRAIT_MANIFEST: &str = r#"[package]
id = "fixture-p464"
version = "0.1.0"
name = "Fixture P464"
status = "draft"
"#;

/// Write `contents` to `path` (creating parent directories) and mark it
/// executable — the stub harness script below is a real file rather than an
/// inline `sh -c` one-liner, so its JSON/shell quoting never has to survive
/// a second layer of TOML string escaping.
fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

/// The shared stub harness: a deterministic version-probe branch (so the
/// harness manifest's non-empty `version-probe` requirement is satisfied
/// AND probe invocations never get counted as frame invocations), then one
/// invocation-count-keyed frame behavior. Every invocation logs the exact
/// stdin prompt it received to `<log_dir>/prompt-<n>.txt` before answering,
/// so the caller can assert on real prompt content rather than only report
/// evidence.
///
/// Invocation 0 (the cold dispatch) always answers with an `answer` VALUE
/// that fails `slot:answer`'s `schema:boolean` — a genuine, persisted core
/// content rejection. Invocation 1 (the correction retry, resumed or cold
/// per the caller's harness declaration) races ahead of drive: before
/// replying at all, it shells out to `ctx traits set` and submits the real,
/// schema-valid answer itself, advancing the session to the second step
/// (which needs its own agent dispatch, so the racer's own submission
/// cannot auto-advance past it). Drive's own retry then submits against the
/// call template it already built for the OLD (first-step) position and
/// gets a genuine non-persisting stale-identity rejection whose refreshed
/// current frame is the different, second-step position. Invocation 2 is
/// that second step's own fresh dispatch, answered directly.
fn racer_script(ctx_bin: &Path, ledger: &Path, home: &Path, log_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"answer":"not-a-boolean","sessionID":"fixed-session-token"}}'
  exit 0
fi
if [ "$COUNT" = "1" ]; then
  HOME="{home}" XDG_CONFIG_HOME="{home}" XDG_CACHE_HOME="{home}" "{ctx_bin}" traits set slot:answer true --session "{ledger}" --value-json --agent worker --json > "{log_dir}/../racer-output.json" 2>&1
  printf '{{"answer":true,"sessionID":"fixed-session-token"}}'
  exit 0
fi
printf '{{"answer2":"done"}}'
"#,
        log_dir = log_dir.display(),
        home = home.display(),
        ctx_bin = ctx_bin.display(),
        ledger = ledger.display(),
    )
}

/// The SAME-FRAME counterpart to [`racer_script`]: proves recovery from a
/// non-persisting stale-identity rejection whose response carries no usable
/// refreshed frame, on the SAME logical position — never a different-frame
/// boundary crossing.
///
/// Invocation 0: answers with a schema-mismatched `answer` (a string) — a
/// real, persisted content rejection (`persist_session = true`), exactly as
/// in `racer_script`.
///
/// Invocation 1 (the correction retry): races ahead of drive again, but this
/// time via a SECOND `ctx traits set` submission of ANOTHER schema-mismatched
/// value against the CURRENT (just-refreshed) call template. That submission
/// is itself rejected by core — content-rejected, persisted, re-templated —
/// which changes the session's state digest a second time WITHOUT advancing
/// its position (a rejection never advances the sequence cursor). Drive's own
/// retry then submits its schema-VALID answer against the call template it
/// already built for the attempt BEFORE this second race — now stale purely
/// on IDENTITY grounds (the digest moved), not content — producing a genuine
/// non-persisting stale-identity rejection whose own response, a preflight
/// identity failure, carries no usable `next_frame` at all. This is the exact
/// case `refresh_frame_for_retry`'s authoritative re-read (never
/// `response.session.next_frame`) exists for.
///
/// Invocation 2: answers with the SAME schema-valid value again, now
/// submitted against the frame drive re-read fresh via `ctx_traits_io::run::status`
/// for this SAME position — and is accepted, completing the frame with
/// exactly one accepted `slot:answer` revision (the two racer submissions
/// were both rejected, never accepted).
fn same_frame_racer_script(ctx_bin: &Path, ledger: &Path, home: &Path, log_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"answer":"not-a-boolean","sessionID":"fixed-session-token"}}'
  exit 0
fi
if [ "$COUNT" = "1" ]; then
  HOME="{home}" XDG_CONFIG_HOME="{home}" XDG_CACHE_HOME="{home}" "{ctx_bin}" traits set slot:answer "still-not-a-boolean" --session "{ledger}" --agent worker --json > "{log_dir}/../racer-output.json" 2>&1
  printf '{{"answer":true,"sessionID":"fixed-session-token"}}'
  exit 0
fi
if [ "$COUNT" = "2" ]; then
  printf '{{"answer":true,"sessionID":"fixed-session-token"}}'
  exit 0
fi
printf '{{"answer2":"done"}}'
"#,
        log_dir = log_dir.display(),
        home = home.display(),
        ctx_bin = ctx_bin.display(),
        ledger = ledger.display(),
    )
}

/// Replays the P542 incident shape: a same-position stale identity, a capture
/// truncation, a content rejection, then a valid answer. The 4 MiB payload is
/// generated by the stub, not by changing production capture limits.
fn incident_script(ctx_bin: &Path, ledger: &Path, home: &Path, log_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  HOME="{home}" XDG_CONFIG_HOME="{home}" XDG_CACHE_HOME="{home}" "{ctx_bin}" traits set slot:answer "still-not-a-boolean" --session "{ledger}" --agent worker --json > "{log_dir}/../racer-output.json" 2>&1
  printf '{{"answer":true,"sessionID":"old-session-token"}}'
  exit 0
fi
if [ "$COUNT" = "1" ]; then
  dd if=/dev/zero bs=4096 count=1025 2>/dev/null | tr '\000' x
  exit 0
fi
if [ "$COUNT" = "2" ]; then
  printf '{{"answer":"not-a-boolean","sessionID":"old-session-token"}}'
  exit 0
fi
if [ "$COUNT" = "3" ]; then
  printf '{{"answer":true}}'
  exit 0
fi
printf '{{"answer2":"done"}}'
"#,
        log_dir = log_dir.display(),
        home = home.display(),
        ctx_bin = ctx_bin.display(),
        ledger = ledger.display(),
    )
}

fn repeated_stale_script(ctx_bin: &Path, ledger: &Path, home: &Path, log_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then printf 'fixture-worker-1.0\n'; exit 0; fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
HOME="{home}" XDG_CONFIG_HOME="{home}" XDG_CACHE_HOME="{home}" "{ctx_bin}" traits set slot:answer "still-not-a-boolean" --session "{ledger}" --agent worker --json > "{log_dir}/../racer-$COUNT.json" 2>&1
printf '{{"answer":true,"sessionID":"fixed-session-token"}}'
"#,
        log_dir = log_dir.display(),
        home = home.display(),
        ctx_bin = ctx_bin.display(),
        ledger = ledger.display(),
    )
}

/// One isolated scratch repo carrying the committed, reviewed, activated
/// `fixture-p464` package (mirrors `proof_merge_on_completion.rs`'s
/// `init_fixture_repo`) plus a machine-local `ctx.toml` binding role
/// `worker` to `harness_id` (pointed at `harness_script`, a real executable
/// this caller has already written).
fn init_fixture_repo(
    repo: &Path,
    home: &Path,
    harness_id: &str,
    harness_script: &Path,
    extra_cli_toml: &str,
) {
    fs::create_dir_all(repo.join(".ctx/traits/fixture-p464/generated")).unwrap();
    git_init(repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    let script = harness_script.to_string_lossy().replace('\\', "\\\\");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

[harness.{harness_id}]
kind = "custom"
bin = "{script}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.{harness_id}.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"
{extra_cli_toml}

[agent.role.worker]
harness = "{harness_id}"
transport = "cli"
session-mode = "per-frame"
"#
    );
    fs::write(repo.join("ctx.toml"), ctx_toml).unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-p464/trait.toml"),
        TRAIT_MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-p464/generated/index.toml"),
        TRAIT_CANONICAL,
    )
    .unwrap();
    require_success(
        "p464-proof `ctx traits init`",
        &["traits", "init"],
        repo,
        home,
    );
    let fixture = ".ctx/traits/fixture-p464/generated/index.toml";
    require_success(
        "p464-proof `ctx traits review --approve`",
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    require_success(
        "p464-proof `ctx traits activate`",
        &["traits", "activate", "--file", fixture],
        repo,
        home,
    );
}

fn value_json(output: &std::process::Output) -> serde_json::Value {
    let (stdout, stderr) = utf8(output);
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'));
    let json_text = match start {
        Some(index) => stdout.lines().skip(index).collect::<Vec<_>>().join("\n"),
        None => stdout.clone(),
    };
    serde_json::from_str(&json_text).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

fn events_of(report: &serde_json::Value, event: &str) -> Vec<serde_json::Value> {
    report["events"]
        .as_array()
        .unwrap_or_else(|| panic!("report had no events array: {report}"))
        .iter()
        .filter(|candidate| candidate["event"] == event)
        .cloned()
        .collect()
}

fn detail(event: &serde_json::Value) -> &str {
    event["detail"].as_str().unwrap_or_default()
}

/// Read `<log_dir>/prompt-<n>.txt` — the exact stdin the stub harness's
/// invocation `n` received.
fn prompt_text(log_dir: &Path, n: usize) -> String {
    fs::read_to_string(log_dir.join(format!("prompt-{n}.txt"))).unwrap_or_else(|error| {
        let listing: Vec<_> = fs::read_dir(log_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        panic!("cannot read prompt-{n}.txt: {error}; dir contents: {listing:?}")
    })
}

/// Number of `slot:answer` entries in the session's accepted-slot-values
/// history, read through `ctx traits run-status --json` (never the raw
/// ledger file) — the public-path evidence that drive's own stale retry
/// never phantom-doubled the racer's single accepted submission.
fn accepted_answer_revisions(ledger: &Path, repo: &Path, home: &Path) -> usize {
    let output = run_ctx(
        &[
            "traits",
            "run-status",
            "--session",
            &ledger.to_string_lossy(),
            "--json",
        ],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    envelope["value"]["accepted-slot-values"]
        .as_array()
        .unwrap_or_else(|| panic!("run-status had no accepted-slot-values: {envelope}"))
        .iter()
        .filter(|value| value["ref-text"] == "slot:answer")
        .count()
}

struct RunOutcome {
    // Held for the caller's whole lifetime — `ScratchRoot::drop` removes its
    // entire tree (repo, ledger, prompt log), so it must outlive every
    // assertion that reads them back, not just this function.
    _scratch: ScratchRoot,
    report: serde_json::Value,
    stderr: String,
    log_dir: std::path::PathBuf,
    ledger: std::path::PathBuf,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn run_fixture(
    label: &str,
    harness_id: &str,
    extra_cli_toml: &str,
    progress_mode: &str,
) -> RunOutcome {
    run_fixture_with_script(
        label,
        harness_id,
        extra_cli_toml,
        progress_mode,
        "2",
        racer_script,
    )
}

fn run_fixture_with_script(
    label: &str,
    harness_id: &str,
    extra_cli_toml: &str,
    progress_mode: &str,
    max_retries: &str,
    script_fn: impl Fn(&Path, &Path, &Path, &Path) -> String,
) -> RunOutcome {
    let scratch = ScratchRoot::new(label);
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let log_dir = repo.join(".ctx/debug/prompts");
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join(format!("{harness_id}.sh"));
    write_executable(&script, &script_fn(&ctx_bin(), &ledger, &home, &log_dir));
    init_fixture_repo(&repo, &home, harness_id, &script, extra_cli_toml);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-p464/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--max-retries",
            max_retries,
            "--json",
            "--progress",
            progress_mode,
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let (_, stderr) = utf8(&output);
    let envelope = value_json(&output);
    // `ctx traits run --json`'s envelope nests the drive report under
    // "drive" alongside the terminal "session" — never the drive report
    // directly.
    let report = envelope["value"]["drive"].clone();
    RunOutcome {
        _scratch: scratch,
        report,
        stderr,
        log_dir,
        ledger,
        repo,
        home,
    }
}

/// A harness with no declared `session-flag`/`resume-flag` can never resume
/// a correction retry: the content-rejection retry is a full cold resend
/// (prompt carries the full `<input>` envelope again), and once the racer advances the
/// session to a different procedure position, drive abandons the retry
/// instead of resubmitting the stale answer against it — the run still
/// completes via a fresh dispatch of the trailing command step.
#[test]
fn content_rejection_cold_retry_then_stale_identity_abandons_at_frame_boundary() {
    let outcome = run_fixture(
        "p464-cold-start",
        "cold-worker",
        "# no session-flag / resume-flag declared",
        "none",
    );
    let report = &outcome.report;
    assert_eq!(report["status"], "completed", "report: {report}");

    let retries = events_of(report, "correction-retry");
    assert_eq!(
        retries.len(),
        1,
        "expected exactly one correction-retry event (only the content-rejection retry actually dispatches a correction): {retries:?}"
    );
    assert!(
        detail(&retries[0]).contains("class=content-rejection")
            && detail(&retries[0]).contains("resumed=false")
            && detail(&retries[0]).contains("correction-ordinal=1")
            && detail(&retries[0]).contains("rung=complete-frame")
            && detail(&retries[0]).contains("schema-delivery=inline")
            && detail(&retries[0]).contains("prompt-bytes=")
            && detail(&retries[0]).contains("bare-correction-estimated-tokens="),
        "the retry must be a cold content-rejection: {}",
        detail(&retries[0])
    );

    // The stale-identity boundary crossing never dispatches a correction —
    // it must be recorded as its own event, distinct from `correction-retry`,
    // and must never claim correction text was sent.
    let boundary = events_of(report, "rejection-boundary-abandoned");
    assert_eq!(
        boundary.len(),
        1,
        "expected exactly one rejection-boundary-abandoned event: {boundary:?}"
    );
    assert!(
        detail(&boundary[0]).contains("class=stale-identity")
            && detail(&boundary[0]).contains("session advanced"),
        "boundary event must classify the stale-identity crossing: {}",
        detail(&boundary[0])
    );
    assert!(
        !detail(&boundary[0]).contains("correction="),
        "boundary event must never claim a correction was sent: {}",
        detail(&boundary[0])
    );

    // The retry (invocation 1) resent the FULL frame — cold delivery, never
    // correction-only — because this harness declares no resume mechanism.
    let retry_prompt = prompt_text(&outcome.log_dir, 1);
    assert!(
        retry_prompt.contains("<input>"),
        "cold retry prompt must resend the full frame: {retry_prompt}"
    );
    let retry_run = events_of(report, "harness-run");
    assert!(
        !detail(&retry_run[1]).contains("--session"),
        "cold retry argv must never carry --session: {}",
        detail(&retry_run[1])
    );

    assert_eq!(
        accepted_answer_revisions(&outcome.ledger, &outcome.repo, &outcome.home),
        1,
        "drive's own stale resubmission must never double the racer's single accepted answer"
    );
}

/// A harness that DOES declare `session-flag` and whose first attempt's
/// output carried an observable session id must resume that conversation on
/// the content-rejection retry: `--session <id>` in the retry argv,
/// correction-only text (never the full `<input>` envelope) in the retry prompt. Once the
/// racer advances the session to a different procedure position, drive still
/// abandons the retry rather than resubmitting the stale answer, and the run
/// completes via a fresh dispatch of the trailing command step.
#[test]
fn content_rejection_resumed_retry_then_stale_identity_abandons_at_frame_boundary() {
    let outcome = run_fixture(
        "p464-resumed",
        "resume-worker",
        "session-flag = \"--session\"",
        "none",
    );
    let report = &outcome.report;
    assert_eq!(report["status"], "completed", "report: {report}");

    let retries = events_of(report, "correction-retry");
    assert_eq!(
        retries.len(),
        1,
        "expected exactly one correction-retry event (only the content-rejection retry actually dispatches a correction): {retries:?}"
    );
    assert!(
        detail(&retries[0]).contains("class=content-rejection")
            && detail(&retries[0]).contains("resumed=true")
            && detail(&retries[0]).contains("correction-ordinal=1")
            && detail(&retries[0]).contains("rung=resumed-reshape")
            && detail(&retries[0]).contains("schema-delivery=inline"),
        "the retry must resume the harness conversation: {}",
        detail(&retries[0])
    );

    // The stale-identity boundary crossing never dispatches a correction —
    // it must be recorded as its own event, distinct from `correction-retry`,
    // and must never claim correction text was sent.
    let boundary = events_of(report, "rejection-boundary-abandoned");
    assert_eq!(
        boundary.len(),
        1,
        "expected exactly one rejection-boundary-abandoned event: {boundary:?}"
    );
    assert!(
        detail(&boundary[0]).contains("class=stale-identity")
            && detail(&boundary[0]).contains("session advanced"),
        "boundary event must classify the stale-identity crossing: {}",
        detail(&boundary[0])
    );
    assert!(
        !detail(&boundary[0]).contains("correction="),
        "boundary event must never claim a correction was sent: {}",
        detail(&boundary[0])
    );

    // The retry (invocation 1) sent ONLY correction text — never the frame
    // title — because this harness resumed the observed conversation.
    let retry_prompt = prompt_text(&outcome.log_dir, 1);
    assert!(
        !retry_prompt.contains("<input>"),
        "first resumed retry must not repeat the frame: {retry_prompt}"
    );
    assert!(
        retry_prompt.contains("<output>")
            && retry_prompt.contains("\"answer\": boolean")
            && retry_prompt.contains("Return ONLY one JSON object")
            && !retry_prompt.contains("<identity>")
            && !retry_prompt.contains("Input types (JSON Schema):")
            && !retry_prompt.contains("Resolved input values:")
            && !retry_prompt.contains("Resolved prompt instructions:"),
        "flagless resumed retry must inline the shared output contract: {retry_prompt}"
    );
    assert!(
        retry_prompt.contains("property `answer` (slot:answer)")
            && retry_prompt.contains("schema schema:boolean")
            && retry_prompt.contains("received string")
            && retry_prompt.contains("required shape boolean")
            && !retry_prompt.contains("expected JSON boolean"),
        "content correction must provide typed repair evidence without validator wording: {retry_prompt}"
    );
    let retry_run = events_of(report, "harness-run");
    assert!(
        detail(&retry_run[1]).contains("--session fixed-session-token"),
        "resumed retry argv must carry the observed session id: {}",
        detail(&retry_run[1])
    );

    assert_eq!(
        accepted_answer_revisions(&outcome.ledger, &outcome.repo, &outcome.home),
        1,
        "drive's own stale resubmission must never double the racer's single accepted answer"
    );
}

/// `--progress status` must name the rejection class and resumed/cold
/// distinction on stderr itself, not only in the JSON report's events — the
/// content-rejection retry line must say `resumed=true`, and the
/// stale-identity boundary crossing must be visibly distinct from it (never
/// claiming a resumed/cold correction retry that never happened).
#[test]
fn progress_status_names_rejection_class_and_resumed_distinction() {
    let outcome = run_fixture(
        "p464-progress-status",
        "resume-worker",
        "session-flag = \"--session\"",
        "status",
    );
    assert_eq!(outcome.report["status"], "completed", "{}", outcome.report);
    assert!(
        outcome.stderr.contains("content-rejection") && outcome.stderr.contains("resumed=true"),
        "status progress must name the content-rejection class and resumed=true: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("stale-identity")
            && outcome
                .stderr
                .contains("session advanced to a different frame"),
        "status progress must name the stale-identity boundary crossing: {}",
        outcome.stderr
    );
}

/// The stranded-same-frame case P464 exists for: a persisted content
/// rejection followed by a non-persisting stale-identity rejection whose OWN
/// response carries no usable refreshed frame (a preflight identity failure,
/// not a re-templated content rejection) — but the CURRENT position never
/// moved. Drive must re-read the ledger through `ctx_traits_io::run::status`
/// (never trust `response.session.next_frame`, which this specific rejection
/// does not even supply), recover the SAME frame's fresh call template, and
/// accept the next schema-valid answer against it — resumed via the observed
/// OpenCode-shaped `sessionID` on every attempt, never abandoned to a fresh
/// dispatch (there is no different frame to abandon to). Exactly one
/// `slot:answer` revision must exist at the end: the two racer submissions
/// were both rejected, never accepted.
#[test]
fn content_rejection_then_same_frame_stale_identity_recovers_and_completes() {
    let outcome = run_fixture_with_script(
        "p464-same-frame",
        "resume-worker",
        "session-flag = \"--session\"",
        "none",
        "1",
        same_frame_racer_script,
    );
    let report = &outcome.report;
    assert_eq!(report["status"], "completed", "report: {report}");

    // No boundary crossing here — the frame never moved, so the fix for
    // `retry-crosses-frame-boundary` must never fire.
    let boundary = events_of(report, "rejection-boundary-abandoned");
    assert!(
        boundary.is_empty(),
        "same-position recovery must never abandon a retry as a boundary crossing: {boundary:?}"
    );

    let retries = events_of(report, "correction-retry");
    assert_eq!(
        retries.len(),
        1,
        "only content rejection consumes correction budget; stale identity is runtime-owned: {retries:?}"
    );
    assert!(
        detail(&retries[0]).contains("class=content-rejection")
            && detail(&retries[0]).contains("resumed=true"),
        "first retry must be the resumed content-rejection correction: {}",
        detail(&retries[0])
    );
    let redispatches = events_of(report, "runtime-redispatch");
    assert_eq!(redispatches.len(), 1, "report: {report}");
    assert!(
        detail(&redispatches[0]).contains("class=stale-identity")
            && detail(&redispatches[0]).contains("handling=runtime-caused")
            && detail(&redispatches[0]).contains("retry-budget-used=1")
            && detail(&redispatches[0]).contains("correction=none"),
        "same-position stale identity must be runtime redispatched: {}",
        detail(&redispatches[0])
    );

    // The first resumed correction is a compact reshape with its output
    // contract inline. The stale redispatch sends the refreshed full frame
    // without any model-facing stale correction language.
    let retry_prompt_1 = prompt_text(&outcome.log_dir, 1);
    let retry_prompt_2 = prompt_text(&outcome.log_dir, 2);
    assert!(
        !retry_prompt_1.contains("<input>")
            && retry_prompt_1.contains("<output>")
            && retry_prompt_2.contains("<title>Produce fixture answer</title>")
            && retry_prompt_2.contains("<data>")
            && retry_prompt_2.contains("<identity>You are agent:")
            && retry_prompt_2.contains("<output>"),
        "only the first resumed retry may be compact: {retry_prompt_1:?} / {retry_prompt_2:?}"
    );
    assert!(
        !retry_prompt_2.contains("Session state changed before this answer was accepted")
            && !retry_prompt_2.contains("refreshed the current frame"),
        "stale redispatch must not send correction language: {retry_prompt_2}"
    );

    let retry_run = events_of(report, "harness-run");
    assert_eq!(
        retry_run.len(),
        4,
        "expected exactly four harness-run events (initial, two same-frame retries, then the second step's own fresh dispatch): {retry_run:?}"
    );
    assert!(
        detail(&retry_run[1]).contains("--session fixed-session-token")
            && detail(&retry_run[2]).contains("--session fixed-session-token"),
        "both retries must resume the observed session: {} / {}",
        detail(&retry_run[1]),
        detail(&retry_run[2])
    );

    assert_eq!(
        accepted_answer_revisions(&outcome.ledger, &outcome.repo, &outcome.home),
        1,
        "the two racer rejections must never count as accepted slot revisions — only drive's own final valid submission does"
    );
}

#[test]
fn stale_truncation_content_incident_spends_only_two_model_retry_charges() {
    let outcome = run_fixture_with_script(
        "p542-incident",
        "resume-worker",
        "session-flag = \"--session\"",
        "none",
        "2",
        incident_script,
    );
    let report = &outcome.report;
    assert_eq!(report["status"], "completed", "report: {report}");
    let redispatches = events_of(report, "runtime-redispatch");
    assert_eq!(redispatches.len(), 1, "report: {report}");
    assert!(detail(&redispatches[0]).contains("retry-budget-used=0"));
    let retries = events_of(report, "correction-retry");
    assert_eq!(retries.len(), 2, "report: {report}");
    assert!(
        detail(&retries[0]).contains("class=output-truncated")
            && detail(&retries[0]).contains("handling=condition-change=fresh-conversation")
            && detail(&retries[0]).contains("resumed=false"),
        "truncation must force a fresh conversation: {}",
        detail(&retries[0])
    );
    assert!(
        detail(&retries[1]).contains("class=content-rejection")
            && detail(&retries[1]).contains("handling=model-fixable"),
        "content rejection must consume the second retry: {}",
        detail(&retries[1])
    );
    let truncation_prompt = prompt_text(&outcome.log_dir, 2);
    assert!(
        truncation_prompt.contains("<input>")
            && truncation_prompt.contains("<identity>You are agent:")
            && truncation_prompt.contains("response ended before a complete JSON object"),
        "truncation retry must carry the complete frame: {truncation_prompt}"
    );
    let runs = events_of(report, "harness-run");
    assert!(
        !detail(&runs[2]).contains("--session old-session-token"),
        "truncation retry must omit the prior session id: {}",
        detail(&runs[2])
    );
}

#[test]
fn repeated_stale_identity_stops_at_the_drive_frame_budget() {
    let scratch = ScratchRoot::new("p542-repeated-stale");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let log_dir = repo.join(".ctx/debug/prompts");
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("repeat.sh");
    write_executable(
        &script,
        &repeated_stale_script(&ctx_bin(), &ledger, &home, &log_dir),
    );
    init_fixture_repo(
        &repo,
        &home,
        "repeat",
        &script,
        "session-flag = \"--session\"",
    );
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-p464/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--max-frames",
            "2",
            "--max-retries",
            "0",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let report = value_json(&output)["value"]["drive"].clone();
    assert_eq!(report["status"], "max-frames-exhausted", "report: {report}");
    assert_eq!(
        events_of(&report, "runtime-redispatch").len(),
        1,
        "report: {report}"
    );
    assert!(
        events_of(&report, "correction-retry").is_empty(),
        "report: {report}"
    );
    assert_eq!(
        events_of(&report, "harness-run").len(),
        2,
        "report: {report}"
    );
}

#[test]
fn schema_flag_resumed_retry_keeps_the_first_reshape_bare() {
    let outcome = run_fixture(
        "p540-schema-flag",
        "schema-worker",
        "session-flag = \"--session\"\njson-schema-flag = \"--json-schema\"",
        "none",
    );
    let report = &outcome.report;
    assert_eq!(report["status"], "completed", "report: {report}");
    let retries = events_of(report, "correction-retry");
    assert!(
        detail(&retries[0]).contains("rung=resumed-reshape")
            && detail(&retries[0]).contains("schema-delivery=flag"),
        "schema-flag retry evidence: {}",
        detail(&retries[0])
    );
    let retry_prompt = prompt_text(&outcome.log_dir, 1);
    assert!(
        !retry_prompt.contains("<input>") && !retry_prompt.contains("<output>"),
        "schema-flag first reshape must leave contract delivery to argv: {retry_prompt}"
    );
    let retry_run = events_of(report, "harness-run");
    assert!(
        detail(&retry_run[1]).contains("--json-schema"),
        "schema-flag retry argv must carry the requested-output schema: {}",
        detail(&retry_run[1])
    );
}
