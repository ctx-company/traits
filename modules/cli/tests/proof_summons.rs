//! 0253.4 proof: a signal-gated `ask` step (`step.ask`) parks a run
//! `awaiting-owner` with the outcome durable in the ledger, and the summon
//! point is listed in `ctx traits internal preview`'s SUMMONS section — a
//! scratch CDK source, built through `ctx traits build`, driven through the
//! `--no-drive` + `internal call` pattern `proof_signal_payloads.rs` uses to
//! submit an unassigned agentless prompt's on-complete signal (a command
//! step's local-runtime dispatch never submits `on-complete` signals itself
//! — only a caller-submitted frame result does), then resumed to the park
//! point, its persisted ledger re-read off disk to confirm the file on disk
//! agrees with what the run reported live.
//!
//! `ask_step_parks_awaiting_owner_and_ledger_agrees` also exercises `ctx
//! traits answer`: a bare call prints the question, and `--value` submits
//! the answer and drives the run to completion, consuming the answer
//! through the trailing command step's captured argv.

use std::fs;
use std::time::{Duration, Instant};

use camino::Utf8Path;
use ctx_traits_io::answer::{AnswerDeliveryVerdict, AnswerEnvelope};
use ctx_traits_io::run_control::{self, AnswerDeliveryResult, DriverProbe};
use support::{
    ScratchRoot, call_session_frame, git_init, require_success, require_success_with_env, run_ctx,
    spawn_ctx, symlink_node_modules, utf8,
};

const DRIVER_DEADLINE: Duration = Duration::from_secs(30);
const DRIVER_POLL: Duration = Duration::from_millis(20);

/// Keeps a detached driver from surviving a failing assertion in a process
/// proof, where an unanswered Ask otherwise waits indefinitely by design.
struct DriveChild(std::process::Child);

impl Drop for DriveChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn fixture_source() -> &'static str {
    // The `ask` step is authored through the newly exposed FUNCTIONAL surface
    // (`step.ask`, 0253.4 slice C), proving the fixture exercises the new
    // authoring path rather than only the pre-existing object-layer
    // `sequence.ask` constructor. `step.ask` requires an open `procedure.from`
    // build scope (it is guard-scope-checked, unlike the object-layer
    // `sequence.*` constructors), so it is placed inside a throwaway
    // `procedure.from` purely to satisfy that scope check — the RAW handle it
    // returns (`ask`, captured by closure, not the throwaway procedure's own
    // materialized `.sequence` copy) still carries every declaration
    // (the `prompt:ask-owner` resource included), so placing it directly in
    // the real, object-layer `procedure({ sequence })` array below resolves
    // cleanly. `emit` stays an object-layer, AGENTLESS `sequence.prompt`
    // deliberately: the functional `step.prompt` requires an `agent`, and an
    // agentless prompt is what forces the frame to pause for an external call
    // (`call_session_frame`, below) instead of dispatching locally — exactly
    // the pause point this proof drives through.
    "import { input, procedure, schema, sequence, signal, step, trait } from \"@ctx-traits/cdk\";\n\
\n\
const needsOwner = signal({ id: \"needs-owner\", description: \"Owner input is needed.\" });\n\
\n\
let ask;\n\
procedure.from({ description: \"throwaway scope for step.ask\" }, () => {\n\
  ask = step.ask(\"ask-owner\", {\n\
    when: needsOwner,\n\
    input: input.prompt`What should I do next?`,\n\
    output: schema.text(),\n\
  });\n\
});\n\
\n\
const emit = sequence.prompt(\"emit\", {\n\
  text: input.prompt`Emit the needs-owner signal.`,\n\
  onComplete: [needsOwner],\n\
});\n\
\n\
const consume = sequence.command(\"consume-answer\", {\n\
  input: input.command`echo ${ask.result}`,\n\
  output: schema.text(),\n\
});\n\
\n\
export const draft = trait({\n\
  id: \"summons-fixture\",\n\
  name: \"summons-fixture\",\n\
  description: \"Ask parks awaiting-owner, with a SUMMONS entry in preview.\",\n\
  signal: [needsOwner],\n\
  procedure: procedure({\n\
    description: \"Emit a signal, ask the owner a guarded question, consume the answer.\",\n\
    sequence: [emit, ask, consume],\n\
  }),\n\
});\n"
}

fn value_json(stdout: &str) -> serde_json::Value {
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'));
    let json_text = match start {
        Some(index) => stdout.lines().skip(index).collect::<Vec<_>>().join("\n"),
        None => stdout.to_string(),
    };
    serde_json::from_str(&json_text).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\nstdout:\n{stdout}")
    })
}

struct Fixture {
    // Held for its `Drop` — the scratch directory backing every path below
    // must outlive this fixture, or it is deleted the moment `build_and_run`
    // returns and every subsequent subprocess sees a missing cwd (ENOENT).
    _scratch: ScratchRoot,
    proj: std::path::PathBuf,
    home: std::path::PathBuf,
    canonical_path: String,
    ledger_path: std::path::PathBuf,
}

/// Shared setup for every scenario below: init, build, trust, activate, start
/// `--no-drive`, and submit the "emit" prompt's on-complete signal as an
/// unassigned-agent caller would (the same surface the dashboard/CLI answer
/// path and a live harness both go through) — a command step's local-runtime
/// dispatch never submits its own `on-complete` signals (P253.4 finding:
/// `advance_command_frames` always submits an empty `signals` map), so the
/// emitting step here is a plain agentless prompt. Leaves the session one
/// `internal drive` short of the `awaiting-owner` park, so a caller can drive
/// that final frame under whatever conditions the scenario needs (a plain
/// resume, or one with a fault injected).
fn start_and_signal(label: &str) -> Fixture {
    start_with_source_and_signal(
        label,
        "summons-fixture",
        fixture_source(),
        serde_json::json!({ "signal:needs-owner": {} }),
    )
}

/// Generalized [`start_and_signal`]: build a caller-supplied CDK `source`
/// under `trait_id`, activate it, start `--no-drive`, and submit
/// `signals_payload` (a `{ "signal:<ref>": <payload> }` map, as
/// `call_session_frame`'s `signals` field expects) as an unassigned-agent
/// caller would. Leaves the session one `internal drive` short of the park,
/// exactly like [`start_and_signal`].
fn start_with_source_and_signal(
    label: &str,
    trait_id: &str,
    source: &str,
    signals_payload: serde_json::Value,
) -> Fixture {
    let scratch = ScratchRoot::new(label);
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    require_success(
        &format!("`ctx traits init {trait_id}`"),
        &["traits", "init", trait_id],
        &proj,
        &home,
    );

    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, source)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    require_success(
        &format!("`ctx traits build` for {trait_id}"),
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );

    let canonical_path = format!(".ctx/traits/authored/{trait_id}/generated/index.toml");
    let canonical_text = fs::read_to_string(proj.join(&canonical_path)).unwrap();
    assert!(
        canonical_text.contains("kind = \"ask\""),
        "expected the CDK ask step to lower to kind = \"ask\", got:\n{canonical_text}"
    );

    require_success(
        &format!("`ctx traits trust --approved` for {trait_id}"),
        &["traits", "trust", "--approved", &canonical_path],
        &proj,
        &home,
    );
    require_success(
        &format!("`ctx traits state --active {trait_id}`"),
        &["traits", "state", "--active", trait_id],
        &proj,
        &home,
    );

    let ledger_path = proj.join("run.json");
    require_success(
        "`ctx traits run --no-drive` (start only)",
        &[
            "traits",
            "run",
            "--file",
            &canonical_path,
            "--no-drive",
            "--out",
            ledger_path.to_str().unwrap(),
            "--json",
        ],
        &proj,
        &home,
    );
    call_session_frame(
        &proj,
        &home,
        &ledger_path,
        None,
        serde_json::json!({ "signals": signals_payload }),
    );

    Fixture {
        _scratch: scratch,
        proj,
        home,
        canonical_path,
        ledger_path,
    }
}

fn await_live_park(
    ledger: &Utf8Path,
) -> (
    ctx_traits_core::procedure::session::Session,
    ctx_traits_io::run_control::DriverHolder,
) {
    let deadline = Instant::now() + DRIVER_DEADLINE;
    loop {
        let session =
            ctx_traits_io::run_session::read_run_session(ledger).expect("read parked ledger");
        if session.last_drive_outcome.as_ref().is_some_and(|outcome| {
            outcome.outcome.as_str() == "awaiting-owner" && outcome.summons.is_some()
        }) && let Ok(DriverProbe::Held(Some(holder))) = run_control::probe(ledger)
        {
            return (session, holder);
        }
        assert!(
            Instant::now() < deadline,
            "driver did not reach the durable awaiting-owner park before {DRIVER_DEADLINE:?}"
        );
        std::thread::sleep(DRIVER_POLL);
    }
}

fn drive_to_park(fixture: &Fixture) -> String {
    let ledger = Utf8Path::from_path(&fixture.ledger_path).expect("UTF-8 ledger");
    let driver = spawn_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            fixture.ledger_path.to_str().unwrap(),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let (_, holder) = await_live_park(ledger);
    assert!(
        run_control::request_interrupt(ledger, &holder).expect("interrupt parked driver"),
        "parked driver did not acknowledge the control interrupt"
    );
    let output = driver
        .wait_with_output()
        .expect("wait for parked driver after control interrupt");
    assert!(
        output.status.success(),
        "parked driver exited with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("driver stdout is UTF-8")
}

/// Some persisted ledger shapes wrap the session under a `"session"` key
/// (e.g. a recovery ledger with sibling lock metadata); others are the
/// session document itself. Every ledger-inspecting assertion below goes
/// through this one accessor rather than repeating the shape guess.
fn session_view(ledger_json: &serde_json::Value) -> &serde_json::Value {
    if ledger_json.get("session").is_some() {
        &ledger_json["session"]
    } else {
        ledger_json
    }
}

fn read_ledger_json(fixture: &Fixture) -> serde_json::Value {
    let ledger_text = fs::read_to_string(&fixture.ledger_path).unwrap_or_else(|error| {
        panic!(
            "cannot read persisted ledger {}: {error}",
            fixture.ledger_path.display()
        )
    });
    serde_json::from_str(&ledger_text).unwrap_or_else(|error| {
        panic!(
            "persisted ledger {} was not JSON: {error}",
            fixture.ledger_path.display()
        )
    })
}

fn build_and_run() -> (Fixture, serde_json::Value, serde_json::Value) {
    let fixture = start_and_signal("summons-fixture");
    let run_stdout = drive_to_park(&fixture);
    let run_json = value_json(&run_stdout);
    let ledger_json = read_ledger_json(&fixture);
    (fixture, run_json, ledger_json)
}

/// A live driver owns the flock while it waits for an Ask answer. The answer
/// travels through its authenticated control socket, is applied by that same
/// process, and parked wall time is excluded from the persisted run budget.
#[test]
fn waiting_driver_accepts_a_socket_answer_without_charging_parked_time() {
    let fixture = start_and_signal("summons-waiting-driver");
    let ledger = Utf8Path::from_path(&fixture.ledger_path).expect("UTF-8 ledger");
    let started = Instant::now();
    let mut driver = DriveChild(spawn_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            fixture.ledger_path.to_str().expect("UTF-8 ledger"),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    ));

    let (parked, holder) = await_live_park(ledger);
    let summons = parked
        .last_drive_outcome
        .as_ref()
        .and_then(|outcome| outcome.summons.as_ref())
        .expect("awaiting-owner outcome retains summons evidence");
    let parked_at = Instant::now();

    std::thread::sleep(Duration::from_secs(3));
    let delivery = run_control::request_answer(
        ledger,
        &holder,
        &AnswerEnvelope {
            target: summons.answer_slot.clone(),
            schema_ref: summons.schema_ref.clone(),
            expected_state_digest: parked.state_digest.to_string(),
            value: serde_json::Value::String("do the thing".to_string()),
        },
    )
    .expect("deliver answer to waiting driver");
    assert_eq!(
        delivery,
        AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::Accepted),
        "the live driver must acknowledge an accepted answer"
    );
    let waited = parked_at.elapsed();

    let exit_deadline = Instant::now() + DRIVER_DEADLINE;
    let status = loop {
        match driver.0.try_wait().expect("poll waiting driver") {
            Some(status) => break status,
            None if Instant::now() < exit_deadline => std::thread::sleep(DRIVER_POLL),
            None => {
                panic!("driver did not complete after accepted answer before {DRIVER_DEADLINE:?}")
            }
        }
    };
    assert!(status.success(), "accepted driver exited with {status}");

    let completed = ctx_traits_io::run_session::read_run_session(ledger)
        .expect("read completed waiting-driver ledger");
    assert_eq!(
        serde_json::to_value(&completed.status).expect("serialize completed status"),
        "completed"
    );
    assert_eq!(
        completed
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.outcome.as_str()),
        Some("completed")
    );
    let total_wall_seconds = started.elapsed().as_secs_f64();
    let parked_seconds = waited.as_secs_f64();
    assert!(
        total_wall_seconds - completed.ledger.elapsed_seconds as f64 > parked_seconds - 1.0,
        "persisted elapsed time ({}) charged most of the parked span ({parked_seconds:.2}s; total wall {total_wall_seconds:.2}s)",
        completed.ledger.elapsed_seconds
    );
}

/// A stale socket delivery cannot consume or disturb the live summons. The
/// same waiting driver accepts the matching answer, and its removed control
/// socket refuses a late duplicate without changing the completed ledger.
#[test]
fn waiting_driver_refuses_stale_and_late_socket_answers() {
    let fixture = start_and_signal("summons-stale-socket-answer");
    let ledger = Utf8Path::from_path(&fixture.ledger_path).expect("UTF-8 ledger");
    let mut driver = DriveChild(spawn_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            fixture.ledger_path.to_str().expect("UTF-8 ledger"),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    ));

    let (parked, holder) = await_live_park(ledger);
    let summons = parked
        .last_drive_outcome
        .as_ref()
        .and_then(|outcome| outcome.summons.as_ref())
        .expect("awaiting-owner outcome retains summons evidence");
    let correct_envelope = AnswerEnvelope {
        target: summons.answer_slot.clone(),
        schema_ref: summons.schema_ref.clone(),
        expected_state_digest: parked.state_digest.to_string(),
        value: serde_json::Value::String("do the thing".to_string()),
    };
    let mut stale_envelope = correct_envelope.clone();
    stale_envelope.expected_state_digest = "deliberately-stale-digest".to_string();

    assert_eq!(
        run_control::request_answer(ledger, &holder, &stale_envelope)
            .expect("deliver stale answer to waiting driver"),
        AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::Stale),
        "a wrong digest must be refused without consuming the live summons"
    );
    assert!(
        driver
            .0
            .try_wait()
            .expect("poll stale-answer driver")
            .is_none(),
        "the driver must remain waiting after a stale answer"
    );
    assert_eq!(
        run_control::probe(ledger).expect("probe stale-answer driver"),
        DriverProbe::Held(Some(holder.clone())),
        "the waiting driver must retain its flock after a stale answer"
    );
    let after_stale = ctx_traits_io::run_session::read_run_session(ledger)
        .expect("re-read ledger after stale answer");
    assert_eq!(after_stale.state_digest, parked.state_digest);
    assert_eq!(
        after_stale
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.outcome.as_str()),
        Some("awaiting-owner")
    );
    assert!(
        after_stale
            .next_frame
            .as_ref()
            .is_some_and(|frame| ctx_traits_io::answer::is_live_summons(&after_stale, frame)),
        "a stale answer must leave the durable park as a live summons"
    );

    assert_eq!(
        run_control::request_answer(ledger, &holder, &correct_envelope)
            .expect("deliver matching answer to waiting driver"),
        AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::Accepted)
    );
    let exit_deadline = Instant::now() + DRIVER_DEADLINE;
    let status = loop {
        match driver.0.try_wait().expect("poll accepted driver") {
            Some(status) => break status,
            None if Instant::now() < exit_deadline => std::thread::sleep(DRIVER_POLL),
            None => {
                panic!("driver did not complete after accepted answer before {DRIVER_DEADLINE:?}")
            }
        }
    };
    assert!(status.success(), "accepted driver exited with {status}");

    let completed_before_late_delivery = read_ledger_json(&fixture);
    assert_eq!(
        session_view(&completed_before_late_delivery)["status"],
        "completed"
    );
    let late_delivery = run_control::request_answer(ledger, &holder, &correct_envelope)
        .expect("late delivery reports an undelivered/refused result");
    assert!(
        !matches!(
            late_delivery,
            AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::Accepted)
        ),
        "a duplicate after completion must not be accepted: {late_delivery:?}"
    );
    assert_eq!(
        read_ledger_json(&fixture),
        completed_before_late_delivery,
        "a late duplicate must not disturb the completed ledger"
    );
}

/// Interrupting a parked driver releases its flock but retains the durable
/// summons, so a later drive can reacquire the session and park again.
#[test]
fn interrupting_a_waiting_driver_releases_the_lock_and_keeps_the_park() {
    let fixture = start_and_signal("summons-interrupt-waiting-driver");
    let ledger = Utf8Path::from_path(&fixture.ledger_path).expect("UTF-8 ledger");
    let mut driver = DriveChild(spawn_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            fixture.ledger_path.to_str().expect("UTF-8 ledger"),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    ));

    let (parked, holder) = await_live_park(ledger);
    assert!(
        run_control::request_interrupt(ledger, &holder).expect("interrupt waiting driver"),
        "the waiting driver must acknowledge its control interrupt"
    );
    let exit_deadline = Instant::now() + DRIVER_DEADLINE;
    let status = loop {
        match driver.0.try_wait().expect("poll interrupted driver") {
            Some(status) => break status,
            None if Instant::now() < exit_deadline => std::thread::sleep(DRIVER_POLL),
            None => panic!("driver did not exit after interrupt before {DRIVER_DEADLINE:?}"),
        }
    };
    assert!(status.success(), "interrupted driver exited with {status}");
    assert!(
        matches!(
            run_control::probe(ledger).expect("probe interrupted driver"),
            DriverProbe::Unheld { .. }
        ),
        "an interrupted driver must release its flock"
    );

    let after_interrupt = ctx_traits_io::run_session::read_run_session(ledger)
        .expect("re-read ledger after interrupt");
    assert_eq!(after_interrupt.state_digest, parked.state_digest);
    assert_eq!(
        after_interrupt
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.outcome.as_str()),
        Some("awaiting-owner"),
        "interrupting a parked driver must retain its durable park"
    );
    assert!(
        after_interrupt
            .next_frame
            .as_ref()
            .is_some_and(|frame| ctx_traits_io::answer::is_live_summons(&after_interrupt, frame)),
        "the interrupted park must remain resumable as a live summons"
    );

    let resumed_stdout = drive_to_park(&fixture);
    assert_eq!(
        value_json(&resumed_stdout)["value"]["status"],
        "awaiting-owner",
        "a later driver must resume the retained park rather than reject the session"
    );
}

/// A signal-gated `ask` step parks the run `awaiting-owner` (not a run
/// failure), with the outcome durable in the ledger re-read off disk.
#[test]
fn ask_step_parks_awaiting_owner_and_ledger_agrees() {
    let (fixture, run_json, ledger_json) = build_and_run();

    assert_eq!(
        run_json["ok"], true,
        "a summons park must never be reported as a failed drive: {run_json}"
    );
    assert_eq!(
        run_json["value"]["status"], "awaiting-owner",
        "expected the run to park awaiting-owner on the guarded ask: {run_json}"
    );

    let session = session_view(&ledger_json);
    assert_eq!(
        session["last-drive-outcome"]["outcome"], "awaiting-owner",
        "persisted ledger disagreed with the live run on the awaiting-owner outcome: {session}"
    );
    assert_eq!(
        session["status"], "waiting-on-human",
        "the session Status vocabulary (frame readiness) is untouched by this task: {session}"
    );
    // The rendered question is durable evidence (P0253.4 blocker 1), not
    // merely re-derivable — persisted on `last-drive-outcome.summons`.
    let summons_record = &session["last-drive-outcome"]["summons"];
    assert_eq!(summons_record["step-id"], "ask-owner");
    assert_eq!(summons_record["question"], "What should I do next?");
    assert_eq!(summons_record["answer-slot"], "slot:ask-owner");

    // Static preview (no session) renders the ask as a human-owned frame,
    // never an agent JSON-response prompt.
    let static_preview_stdout = require_success(
        "`ctx traits internal preview --json` (static, no session)",
        &[
            "traits",
            "internal",
            "preview",
            "--file",
            &fixture.canonical_path,
            "--step",
            "ask-owner",
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let static_preview_json = value_json(&static_preview_stdout);
    let static_frame = &static_preview_json["frames"][0];
    assert_eq!(static_frame["transport"], "human");
    let static_prompt = static_frame["prompt"].as_str().unwrap_or_default();
    assert!(
        static_prompt.contains("Human question:"),
        "expected a human-owned prompt, got: {static_frame}"
    );
    // Negative half of the same assertion (P0253.4 blocker 3): the fix to
    // `build_static_frame`'s missing `Ask` arm must route a human frame away
    // from the agent JSON-response contract entirely, not merely append the
    // human question alongside it.
    assert!(
        !static_prompt.contains("<output>")
            && !static_prompt.contains("Return ONLY one JSON object"),
        "a human-owned ask frame must not carry the agent JSON-response contract, got: {static_frame}"
    );

    // `ctx traits internal preview` lists the summon point regardless of
    // the run's current position, with its guard and answer slot.
    let preview_stdout = require_success(
        "`ctx traits internal preview --json`",
        &[
            "traits",
            "internal",
            "preview",
            "--file",
            &fixture.canonical_path,
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let preview_json = value_json(&preview_stdout);
    let summons = preview_json["summons"]
        .as_array()
        .unwrap_or_else(|| panic!("no summons array in preview: {preview_json}"));
    assert_eq!(
        summons.len(),
        1,
        "expected exactly one summon point: {preview_json}"
    );
    assert_eq!(summons[0]["step"], "ask-owner");
    assert_eq!(summons[0]["guard"], "signal:needs-owner");
    assert_eq!(summons[0]["answer-slot"], "slot:ask-owner");

    // `ctx traits answer` with no value shows the question and refuses to
    // submit anything.
    let session_arg = fixture.ledger_path.to_str().unwrap();
    let bare_stdout = require_success(
        "`ctx traits answer <session> --json` (no value)",
        &["traits", "answer", session_arg, "--json"],
        &fixture.proj,
        &fixture.home,
    );
    let bare_json = value_json(&bare_stdout);
    assert_eq!(bare_json["ok"], true);
    assert_eq!(bare_json["value"]["submitted"], false);
    assert_eq!(bare_json["value"]["question"], "What should I do next?");
    assert_eq!(bare_json["value"]["answer-slot"], "slot:ask-owner");

    // `--value` submits the answer and resumes the run to completion,
    // consuming the answer through the trailing command step.
    let answered_stdout = require_success(
        "`ctx traits answer <session> --value --json`",
        &[
            "traits",
            "answer",
            session_arg,
            "--value",
            "do the thing",
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let answered_json = value_json(&answered_stdout);
    assert_eq!(answered_json["ok"], true);
    assert_eq!(answered_json["value"]["submitted"], true);
    assert_eq!(answered_json["value"]["resumed-status"], "completed");

    let final_ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&fixture.ledger_path).unwrap()).unwrap();
    assert_eq!(
        final_ledger["status"], "completed",
        "the answer must be consumed and the run driven to completion: {final_ledger}"
    );
    // The trailing command's captured stdout carries the submitted answer,
    // proving it was CONSUMED through the ask's slot (`echo ${ask.result}`)
    // rather than the run merely reaching `completed` for an unrelated
    // reason.
    let consumed = final_ledger["accepted-slot-values"]
        .as_array()
        .unwrap_or_else(|| panic!("no accepted-slot-values in {final_ledger}"))
        .iter()
        .find(|entry| entry["ref-text"] == "slot:consume-answer")
        .unwrap_or_else(|| {
            panic!("no slot:consume-answer in accepted-slot-values: {final_ledger}")
        })["value"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        consumed.contains("do the thing"),
        "expected the consuming command's captured output to contain the submitted answer, got: {consumed}"
    );

    drop(fixture.ledger_path);
}

/// A trait with no `ask` composes byte-identically to today: the CDK
/// widening of `AskSequenceFields.output` and the new `sequenceAsk` helper
/// touch no lowering path an ask-free trait exercises.
#[test]
fn trait_with_no_ask_is_unaffected() {
    let scratch = ScratchRoot::new("summons-fixture-no-ask");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let trait_id = "no-ask-fixture";
    require_success(
        &format!("`ctx traits init {trait_id}`"),
        &["traits", "init", trait_id],
        &proj,
        &home,
    );
    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(
        &source_path,
        "import { procedure, schema, sequence, trait } from \"@ctx-traits/cdk\";\n\
\n\
const step = sequence.command(\"only-step\", { cmd: \"echo hi\", output: schema.text() });\n\
\n\
export const draft = trait({\n\
  id: \"no-ask-fixture\",\n\
  name: \"no-ask-fixture\",\n\
  description: \"No ask step at all.\",\n\
  procedure: procedure({ description: \"d\", sequence: [step] }),\n\
});\n",
    )
    .unwrap();
    require_success(
        &format!("`ctx traits build` for {trait_id}"),
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    let canonical_path = proj.join(format!(
        ".ctx/traits/authored/{trait_id}/generated/index.toml"
    ));
    let canonical_text = fs::read_to_string(&canonical_path).unwrap();
    // Exact byte comparison against an established baseline (P0253.4 blocker
    // 3), not a `contains("\"ask\"")` substring check: the canonical TOML for
    // this fixture carries no volatile field (no digest/timestamp/absolute
    // path), so its bytes are reproducible across machines and this is a
    // genuine byte-identity proof, not merely "no ask keyword present".
    let mut expected_canonical = String::new();
    expected_canonical.push_str("id = \"no-ask-fixture\"\n");
    expected_canonical.push_str("schema-version = \"0.5\"\n");
    expected_canonical.push_str("version = \"0.1.0\"\n");
    expected_canonical.push_str("name = \"no-ask-fixture\"\n");
    expected_canonical.push_str("description = \"No ask step at all.\"\n");
    expected_canonical.push('\n');
    expected_canonical.push_str("[[slot]]\n");
    expected_canonical.push_str("id = \"only-step\"\n");
    expected_canonical.push_str("schema = \"schema:text\"\n");
    expected_canonical.push_str("description = \"Runtime slot only-step.\"\n");
    expected_canonical.push('\n');
    expected_canonical.push_str("[procedure]\n");
    expected_canonical.push_str("description = \"d\"\n");
    expected_canonical.push('\n');
    expected_canonical.push_str("[[procedure.sequence]]\n");
    expected_canonical.push_str("id = \"only-step\"\n");
    expected_canonical.push_str("title = \"Only Step\"\n");
    expected_canonical.push_str("output = [\"slot:only-step\"]\n");
    expected_canonical.push('\n');
    expected_canonical.push_str("[procedure.sequence.command]\n");
    expected_canonical.push_str("argv = [\n");
    expected_canonical.push_str("    \"echo\",\n");
    expected_canonical.push_str("    \"hi\",\n");
    expected_canonical.push_str("]\n");
    assert_eq!(
        canonical_text, expected_canonical,
        "the CDK ask widening (0253.4 slice C) must not perturb an ask-free \
         trait's composed canonical bytes"
    );
}

/// P0253.4 blocker 1 (durable-summons-evidence): the park point itself must
/// never report or persist `awaiting-owner` unless the durable outcome write
/// actually lands. Drives the exact frame that would park through with
/// `CTX_INTERNAL_TESTHOOK_FAIL_DRIVE_OUTCOME_WRITE=1` — the same fault hook
/// `proof_center.rs` uses for the same write path — and proves the drive
/// demotes itself to a failure, the ledger on disk is unchanged, and `ctx
/// traits answer` refuses the frame as an unrecorded summons rather than
/// treating the still-Ask-shaped frame as answerable.
#[test]
fn ask_park_with_failed_outcome_write_never_reports_a_live_summons() {
    let fixture = start_and_signal("summons-fixture-write-failure");
    let before = read_ledger_json(&fixture);
    let before_outcome = session_view(&before)["last-drive-outcome"].clone();

    let run_stdout = require_success_with_env(
        "`ctx traits internal drive --json` with an injected outcome-write failure",
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            fixture.ledger_path.to_str().unwrap(),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
        &[("CTX_INTERNAL_TESTHOOK_FAIL_DRIVE_OUTCOME_WRITE", "1")],
    );
    let run_json = value_json(&run_stdout);
    assert_ne!(
        run_json["value"]["status"], "awaiting-owner",
        "a failed durable write must never be reported as a successful park: {run_json}"
    );

    let after = read_ledger_json(&fixture);
    assert_eq!(
        session_view(&after)["last-drive-outcome"],
        before_outcome,
        "the persisted ledger must be unchanged by a failed outcome write: {}",
        session_view(&after)
    );

    let output = run_ctx(
        &["traits", "answer", fixture.ledger_path.to_str().unwrap()],
        &fixture.proj,
        &fixture.home,
    );
    assert!(
        !output.status.success(),
        "answer must refuse a summons that was never durably recorded"
    );
    let (_, stderr) = utf8(&output);
    assert!(
        stderr.contains("not waiting for a summons answer") || stderr.contains("was cancelled"),
        "expected a refusal naming the summons unavailable, got: {stderr}"
    );
}

/// P0253.4 blocker 1 (durable-summons-evidence): a ledger parked before the
/// `summons` field existed (simulated here by stripping the stored record
/// off an otherwise normal park) must still answer through live
/// re-resolution — the legacy-fallback half of `summons_question`
/// (`frame_prompt.rs`) exercised through the real answer surface rather than
/// inferred from its structure alone.
#[test]
fn answer_falls_back_to_live_resolution_when_the_stored_summons_record_is_absent() {
    let (fixture, run_json, _ledger_json) = build_and_run();
    assert_eq!(run_json["value"]["status"], "awaiting-owner");

    let mut ledger_json = read_ledger_json(&fixture);
    {
        let session = if ledger_json.get("session").is_some() {
            ledger_json.get_mut("session").expect("session key present")
        } else {
            &mut ledger_json
        };
        if let Some(outcome) = session.get_mut("last-drive-outcome")
            && let Some(outcome_obj) = outcome.as_object_mut()
        {
            outcome_obj.remove("summons");
        }
    }
    fs::write(
        &fixture.ledger_path,
        serde_json::to_string_pretty(&ledger_json).unwrap(),
    )
    .unwrap_or_else(|error| panic!("cannot rewrite stripped ledger: {error}"));

    let bare_stdout = require_success(
        "`ctx traits answer <session> --json` (no stored summons record)",
        &[
            "traits",
            "answer",
            fixture.ledger_path.to_str().unwrap(),
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let bare_json = value_json(&bare_stdout);
    assert_eq!(bare_json["value"]["submitted"], false);
    assert_eq!(
        bare_json["value"]["question"], "What should I do next?",
        "live re-resolution must recover the same question a stored record would have: {bare_json}"
    );
    assert_eq!(bare_json["value"]["answer-slot"], "slot:ask-owner");
}

/// P0253.4 blocker 2 (shared-answer-submission-semantics): a summons whose
/// last recorded outcome is `interrupted`/`killed` (a center repair, an
/// interrupt, a kill — simulated here by editing the outcome directly) must
/// be refused as cancelled, not shown or answered, even though its frame is
/// still shaped like a live Ask.
#[test]
fn answer_refuses_a_cancelled_summons() {
    let (fixture, run_json, _ledger_json) = build_and_run();
    assert_eq!(run_json["value"]["status"], "awaiting-owner");

    let mut ledger_json = read_ledger_json(&fixture);
    {
        let session = if ledger_json.get("session").is_some() {
            ledger_json.get_mut("session").expect("session key present")
        } else {
            &mut ledger_json
        };
        session["last-drive-outcome"]["outcome"] =
            serde_json::Value::String("interrupted".to_string());
    }
    fs::write(
        &fixture.ledger_path,
        serde_json::to_string_pretty(&ledger_json).unwrap(),
    )
    .unwrap_or_else(|error| panic!("cannot rewrite cancelled ledger: {error}"));

    let output = run_ctx(
        &["traits", "answer", fixture.ledger_path.to_str().unwrap()],
        &fixture.proj,
        &fixture.home,
    );
    assert!(
        !output.status.success(),
        "answer must refuse a cancelled summons"
    );
    let (_, stderr) = utf8(&output);
    assert!(
        stderr.contains("was cancelled"),
        "expected a cancellation refusal, got: {stderr}"
    );
}

/// P0253.4 blocker 2 (shared-answer-submission-semantics): `--no-resume`
/// records the answer without driving the trailing command step — its side
/// effect (the captured `echo`) must not appear until a later, explicit
/// resume actually runs it.
#[test]
fn answer_no_resume_records_without_driving_the_following_command() {
    let (fixture, run_json, _ledger_json) = build_and_run();
    assert_eq!(run_json["value"]["status"], "awaiting-owner");

    let session_arg = fixture.ledger_path.to_str().unwrap();
    let no_resume_stdout = require_success(
        "`ctx traits answer <session> --value --no-resume --json`",
        &[
            "traits",
            "answer",
            session_arg,
            "--value",
            "do the thing",
            "--no-resume",
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let no_resume_json = value_json(&no_resume_stdout);
    assert_eq!(no_resume_json["value"]["submitted"], true);
    assert!(
        no_resume_json["value"]["resumed-status"].is_null(),
        "no resume was requested, so no resumed status should be reported: {no_resume_json}"
    );

    let mid_ledger = read_ledger_json(&fixture);
    let mid_session = session_view(&mid_ledger);
    assert_ne!(
        mid_session["status"], "completed",
        "the trailing command must not have run before an explicit resume: {mid_session}"
    );
    assert!(
        mid_session
            .get("accepted-slot-values")
            .and_then(|values| values.as_array())
            .into_iter()
            .flatten()
            .all(|entry| entry["ref-text"] != "slot:consume-answer"),
        "the trailing command's output must not be captured before a resume: {mid_session}"
    );

    let resumed_stdout = require_success(
        "`ctx traits internal drive --json` (explicit resume after --no-resume)",
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            &fixture.canonical_path,
            "--session",
            session_arg,
            "--json",
        ],
        &fixture.proj,
        &fixture.home,
    );
    let resumed_json = value_json(&resumed_stdout);
    assert_eq!(resumed_json["value"]["status"], "completed");

    let final_ledger = read_ledger_json(&fixture);
    let final_session = session_view(&final_ledger);
    let consumed = final_session["accepted-slot-values"]
        .as_array()
        .unwrap_or_else(|| panic!("no accepted-slot-values in {final_session}"))
        .iter()
        .find(|entry| entry["ref-text"] == "slot:consume-answer")
        .unwrap_or_else(|| {
            panic!("no slot:consume-answer in accepted-slot-values: {final_session}")
        })["value"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        consumed.contains("do the thing"),
        "expected the explicit resume to consume the recorded answer, got: {consumed}"
    );
}

/// 0272 goal 5 (end-to-end): a `step.ask` guarded on the toolkit's schema'd
/// `needsOwnerSignal` interpolates its payload's `.question` field
/// (`{signal:needs-owner.question}`) directly into the ask prompt. Submits a
/// distinctive question through the emitted signal's payload and asserts it
/// appears verbatim in the durable `last-drive-outcome.summons.question` —
/// proof that a signal payload field reference resolves through drive the
/// same way a slot reference does.
#[test]
fn ask_prompt_interpolates_a_schema_carrying_signal_payload_field() {
    // A locally declared schema'd signal, deliberately not the toolkit's
    // `needsOwnerSignal`: the CDK stamps each construct's own source anchor
    // from its call site, which must resolve inside the building repo — a
    // scratch test repo's `node_modules/@ctx-traits/toolkit` symlink points
    // at this repository's real checkout, outside that scratch repo, so an
    // imported package construct's anchor cannot be made repository-relative
    // here. Declaring the same shape (`reason`/`question` fields) locally
    // exercises the identical `signal:needs-owner.question` interpolation
    // grammar `needsOwnerSignal` produces, without the anchor mismatch.
    let source = "import { input, procedure, schema, sequence, signal, step, trait } from \"@ctx-traits/cdk\";\n\
        \n\
        const needsOwner = signal(\"needs-owner\", {\n\
        \x20 description: \"The run cannot proceed without an owner decision.\",\n\
        \x20 schema: schema.object(\"needs-owner-payload\", {\n\
        \x20   reason: schema.field(schema.text(), { description: \"Why the owner must decide.\" }),\n\
        \x20   question: schema.field(schema.text(), { description: \"The question for the owner.\" }),\n\
        \x20 }),\n\
        });\n\
        \n\
        const emit = sequence.prompt(\"emit\", {\n\
        \x20 text: input.prompt`Emit the needs-owner signal.`,\n\
        \x20 onComplete: [needsOwner],\n\
        });\n\
        \n\
        let ask;\n\
        procedure.from({ description: \"throwaway scope for step.ask\" }, () => {\n\
        \x20 ask = step.ask(\"ask-owner\", {\n\
        \x20   when: needsOwner,\n\
        \x20   input: input.prompt`${needsOwner.question}`,\n\
        \x20   output: schema.text(),\n\
        \x20 });\n\
        });\n\
        \n\
        export const draft = trait({\n\
        \x20 id: \"signal-payload-summons-fixture\",\n\
        \x20 name: \"signal-payload-summons-fixture\",\n\
        \x20 description: \"An ask prompt interpolating a schema-carrying signal payload field.\",\n\
        \x20 signal: [needsOwner],\n\
        \x20 procedure: procedure({\n\
        \x20   description: \"Emit the schema'd signal, then ask the owner a guarded question.\",\n\
        \x20   sequence: [emit, ask],\n\
        \x20 }),\n\
        });\n";

    let distinctive_question = "Which of the two migration paths should this run take?";
    let fixture = start_with_source_and_signal(
        "signal-payload-summons-fixture",
        "signal-payload-summons-fixture",
        source,
        serde_json::json!({
            "signal:needs-owner": {
                "payload": {
                    "reason": "Two equally valid migration paths, no authority to pick one.",
                    "question": distinctive_question,
                }
            }
        }),
    );

    let run_stdout = drive_to_park(&fixture);
    let run_json = value_json(&run_stdout);
    assert_eq!(
        run_json["value"]["status"], "awaiting-owner",
        "expected the run to park awaiting-owner on the guarded ask: {run_json}"
    );

    let ledger_json = read_ledger_json(&fixture);
    let session = session_view(&ledger_json);
    assert_eq!(
        session["last-drive-outcome"]["summons"]["question"], distinctive_question,
        "the ask prompt's {{signal:needs-owner.question}} interpolation must resolve to the \
         submitted payload's question field verbatim: {session}"
    );
}
