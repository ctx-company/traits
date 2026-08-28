//! Persisted-call proof for schema-carrying signal emissions.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use support::{ScratchRoot, call_session_frame, git_init, require_success, run_ctx, utf8};

const ID: &str = "fixture-signal-payload";
const MANIFEST: &str = r#"[package]
id = "fixture-signal-payload"
version = "0.1.0"
name = "Signal payload fixture"
status = "draft"
"#;
const TRAIT: &str = r#"id = "fixture-signal-payload"
schema-version = "0.4"
version = "0.1.0"
name = "Signal payload fixture"
description = "Exercises persisted signal payload validation."

[[signal]]
id = "review"
description = "A review result."
schema = "schema:review-payload"

[[schema]]
id = "review-payload"

[schema.fields.reason]
schema = "schema:text"
required = true

[procedure]
description = "A schema-carrying signal guarded inside a loop."

[[sequence.review-body.sequence]]
id = "emit"
title = "Emit review"
prompt = "Emit the review signal {signal:review}."
on-complete = ["signal:review"]

[[sequence.review-body.sequence]]
id = "review-guard"
title = "Review guard"
kind = "branch"
sequence = "sequence:consume-body"
when = "signal:review"

[[sequence.consume-body.sequence]]
id = "consume"
title = "Consume review"
prompt = "The review is {signal:review}. Reason: {signal:review.reason}. Unknown: {signal:review.unknown}."

[[procedure.sequence]]
id = "review-loop"
title = "Review loop"
kind = "loop"
sequence = "sequence:review-body"
max-iterations = 2
"#;

const BARE_TRAIT: &str = r#"id = "fixture-signal-payload"
schema-version = "0.4"
version = "0.1.0"
name = "Bare signal payload fixture"
description = "Exercises legacy signal compatibility."

[[signal]]
id = "review"
description = "A legacy review signal."

[procedure]
description = "One bare signal emission."

[[procedure.sequence]]
id = "emit"
title = "Emit review"
prompt = "Emit the review signal {signal:review}."
on-complete = ["signal:review"]
"#;

const DRIVE_TRAIT: &str = r#"id = "fixture-signal-payload"
schema-version = "0.4"
version = "0.1.0"
name = "Driven signal payload fixture"
description = "Exercises signal correction through the drive boundary."

[[agent]]
id = "worker"
description = "Fixture worker."

[[signal]]
id = "review"
description = "A review result."
schema = "schema:review-payload"

[[schema]]
id = "review-payload"

[schema.fields.reason]
schema = "schema:text"
required = true

[procedure]
description = "One driven schema-carrying signal."

[[procedure.sequence]]
id = "emit"
title = "Emit review"
agent = "agent:worker"
prompt = "Emit the review signal."
on-complete = ["signal:review"]
"#;

const MIXED_DRIVE_TRAIT: &str = r#"id = "fixture-signal-payload"
schema-version = "0.4"
version = "0.1.0"
name = "Mixed driven signal payload fixture"
description = "Exercises missing required slot correction with an optional signal."

[[agent]]
id = "worker"
description = "Fixture worker."

[[slot]]
id = "answer"
schema = "schema:text"
description = "A required answer."

[[signal]]
id = "review"
description = "A review result."
schema = "schema:review-payload"

[[schema]]
id = "review-payload"

[schema.fields.reason]
schema = "schema:text"
required = true

[procedure]
description = "One driven slot and schema-carrying signal."

[[procedure.sequence]]
id = "emit"
title = "Emit answer and review"
agent = "agent:worker"
prompt = "Emit the answer and optional review signal."
output = ["slot:answer"]
on-complete = ["signal:review"]
"#;

fn setup(trait_text: &str) -> (ScratchRoot, std::path::PathBuf, std::path::PathBuf) {
    let scratch = ScratchRoot::new("signal-payloads");
    let home = scratch.home().to_path_buf();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(format!(".ctx/traits/{ID}/generated"))).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    fs::write(repo.join(format!(".ctx/traits/{ID}/trait.toml")), MANIFEST).unwrap();
    let trait_path = repo.join(format!(".ctx/traits/{ID}/generated/index.toml"));
    fs::write(&trait_path, trait_text).unwrap();
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            trait_path.to_str().unwrap(),
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &[
            "traits",
            "state",
            "--active",
            "--file",
            trait_path.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    (scratch, repo, home)
}

fn start(repo: &Path, home: &Path, ledger: &Path) {
    require_success(
        "start payload fixture",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--no-drive",
            "--out",
            ledger.to_str().unwrap(),
            "--json",
        ],
        repo,
        home,
    );
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn call(
    repo: &Path,
    home: &Path,
    ledger: &Path,
    signal: &str,
    payload: Option<serde_json::Value>,
) -> serde_json::Value {
    let submission = payload.map_or_else(
        || serde_json::json!({}),
        |payload| serde_json::json!({ "payload": payload }),
    );
    call_session_frame(
        repo,
        home,
        ledger,
        None,
        serde_json::json!({ "signals": { signal: submission } }),
    )
}

fn call_without_signals(repo: &Path, home: &Path, ledger: &Path) {
    let _ = call_session_frame(
        repo,
        home,
        ledger,
        None,
        serde_json::json!({ "signals": {} }),
    );
}

#[test]
fn persisted_call_records_valid_payload_and_rejects_invalid_payload_without_advancing() {
    let (_scratch, repo, home) = setup(TRAIT);
    let valid_ledger = home.join("valid.json");
    start(&repo, &home, &valid_ledger);
    call(
        &repo,
        &home,
        &valid_ledger,
        "signal:review",
        Some(serde_json::json!({ "reason": "approved" })),
    );
    let valid: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&valid_ledger).unwrap()).unwrap();
    let emission = &valid["ledger"]["emitted-signals"][0];
    assert_eq!(
        emission["payload"],
        serde_json::json!({ "reason": "approved" })
    );
    assert!(emission["payload-digest"].as_str().is_some());
    assert_eq!(valid["next-frame"]["item-id"], "consume");
    assert_eq!(
        valid["next-frame"]["signal-payloads"][0]["payload"],
        serde_json::json!({ "reason": "approved" }),
        "the following frame exposes the accepted payload"
    );

    let preview = run_ctx(
        &[
            "traits",
            "internal",
            "preview",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--session",
            valid_ledger.to_str().unwrap(),
            "--step",
            "consume",
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(
        preview.status.success(),
        "preview failed: {:?}",
        utf8(&preview)
    );
    let (stdout, _) = utf8(&preview);
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let prompt = preview["frames"][0]["prompt"].as_str().unwrap();
    assert_eq!(
        prompt
            .matches("<review hint=\"schema:review-payload\">")
            .count(),
        1,
        "the interpolated signal must have exactly one schema-hinted spec entry: {prompt}"
    );
    assert!(
        prompt.contains("A review result."),
        "signal description must travel: {prompt}"
    );
    assert!(
        prompt.contains("Reason: approved."),
        "an accepted object payload field must be interpolated: {prompt}"
    );
    assert!(
        prompt.contains("Unknown: {signal:review.unknown}."),
        "an unavailable signal payload field must remain literal: {prompt}"
    );

    // Completing the guarded body starts iteration two. Its repeated scope
    // differs from the emission's, so the payload cannot leak forward.
    call_without_signals(&repo, &home, &valid_ledger);
    let next_iteration: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&valid_ledger).unwrap()).unwrap();
    assert_eq!(next_iteration["next-frame"]["item-id"], "emit");
    assert!(
        next_iteration["next-frame"]
            .get("signal-payloads")
            .is_none(),
        "iteration N+1 must not see iteration N's signal payload"
    );

    let rejected_ledger = home.join("rejected.json");
    start(&repo, &home, &rejected_ledger);
    let rejected_response = call(
        &repo,
        &home,
        &rejected_ledger,
        "signal:review",
        Some(serde_json::json!({ "wrong": false })),
    );
    let response = rejected_response.get("value").unwrap_or(&rejected_response);
    assert_eq!(
        response["response-kind"], "rejected-correction-required",
        "persisted call response: {rejected_response}"
    );
    assert!(
        response["correction"]
            .as_str()
            .is_some_and(|correction| correction.contains("reason")),
        "the persisted rejection must name the invalid payload field: {rejected_response}"
    );
    let rejected: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&rejected_ledger).unwrap()).unwrap();
    assert_eq!(
        rejected["last-validation-report"]["signal-validation"][0]["signal-ref"],
        "signal:review"
    );
    let reason = rejected["last-validation-report"]["signal-validation"][0]["reason"]
        .as_str()
        .unwrap();
    assert!(
        reason.contains("reason"),
        "schema rejection must name the field: {reason}"
    );
    assert_eq!(
        rejected["next-frame"]["item-id"], "emit",
        "rejected submission must not advance"
    );
}

#[test]
fn preview_renders_schema_signal_as_an_optional_output_channel() {
    let (_scratch, repo, home) = setup(TRAIT);
    let output = run_ctx(
        &[
            "traits",
            "internal",
            "preview",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(
        output.status.success(),
        "preview failed: {:?}",
        utf8(&output)
    );
    let (stdout, _) = utf8(&output);
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let prompt = preview["frames"][0]["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("<format>{\"review\": \"review\"}</format>")
            && prompt.contains("<review>{\"additionalProperties\":false"),
        "schema'd signal must be an optional response property: {prompt}"
    );
    assert_eq!(
        prompt
            .matches("<review hint=\"schema:review-payload\">")
            .count(),
        1,
        "an emitting frame must receive exactly one schema-hinted signal spec before a payload exists: {prompt}"
    );
    assert!(
        prompt.contains("A review result."),
        "the emitting frame must receive the signal description: {prompt}"
    );
}

#[test]
fn persisted_bare_signal_keeps_canonical_and_ledger_payload_free() {
    let (_scratch, repo, home) = setup(BARE_TRAIT);
    let ledger = home.join("bare.json");
    start(&repo, &home, &ledger);
    let preview = run_ctx(
        &[
            "traits",
            "internal",
            "preview",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(
        preview.status.success(),
        "preview failed: {:?}",
        utf8(&preview)
    );
    let (stdout, _) = utf8(&preview);
    let preview: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        !preview["frames"][0]["prompt"]
            .as_str()
            .unwrap()
            .contains("<review"),
        "bare signals must not add a payload spec entry"
    );
    let _ = call(&repo, &home, &ledger, "signal:review", None);

    let canonical =
        fs::read_to_string(repo.join(".ctx/traits/fixture-signal-payload/generated/index.toml"))
            .unwrap();
    let decoded = ctx_traits_core::encoding::decode_trait(
        ctx_traits_core::encoding::Encoding::Toml,
        &canonical,
    )
    .expect("bare canonical decodes");
    let reencoded =
        ctx_traits_core::encoding::encode(ctx_traits_core::encoding::Encoding::Toml, &decoded)
            .expect("bare canonical re-encodes");
    assert_eq!(
        reencoded, canonical,
        "a bare signal must decode and re-encode byte-for-byte identically"
    );

    let session: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(ledger).unwrap()).unwrap();
    let emission = &session["ledger"]["emitted-signals"][0];
    assert!(emission.get("payload").is_none());
    assert!(emission.get("payload-digest").is_none());
}

#[test]
fn drive_submits_emitted_and_omitted_optional_signal_outputs() {
    for (label, response, expected_payload) in [
        (
            "emitted",
            r#"{"review":{"reason":"approved"}}"#,
            Some(serde_json::json!({ "reason": "approved" })),
        ),
        ("omitted", "{}", None),
    ] {
        let (_scratch, repo, home) = setup(DRIVE_TRAIT);
        let script = home.join(format!("optional-signal-{label}.sh"));
        write_executable(
            &script,
            &format!(
                r#"#!/bin/sh
if [ "$1" = "--probe" ]; then
  printf 'signal-fixture-1.0\n'
  exit 0
fi
printf '%s' '{response}'
"#,
            ),
        );
        fs::write(
            repo.join(".ctx/traits/runtime.toml"),
            format!(
                r#"schema-version = "0.4"

[harness.fixture]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.fixture.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
session-mode = "per-frame"
"#,
                script.display()
            ),
        )
        .unwrap();
        let ledger = home.join(format!("optional-{label}.json"));
        let output = run_ctx(
            &[
                "traits",
                "run",
                "--file",
                ".ctx/traits/fixture-signal-payload/generated/index.toml",
                "--out",
                ledger.to_str().unwrap(),
                "--json",
            ],
            &repo,
            &home,
        );
        assert!(
            output.status.success(),
            "optional {label} signal drive failed: {:?}",
            utf8(&output)
        );
        let session: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(ledger).unwrap()).unwrap();
        assert_eq!(session["status"], "completed");
        match expected_payload {
            Some(payload) => {
                assert_eq!(session["ledger"]["emitted-signals"][0]["payload"], payload)
            }
            None => assert!(
                session["ledger"]
                    .get("emitted-signals")
                    .is_none_or(|signals| signals.as_array().is_some_and(Vec::is_empty)),
                "an omitted optional signal must still reach a successful empty submission: {session}"
            ),
        }
    }
}

#[test]
fn drive_retries_an_invalid_signal_payload_and_submits_the_corrected_payload() {
    let (_scratch, repo, home) = setup(DRIVE_TRAIT);
    let script = home.join("signal-harness.sh");
    let calls = home.join("signal-harness-calls");
    write_executable(
        &script,
        &format!(
            r#"#!/bin/sh
if [ "$1" = "--probe" ]; then
  printf 'signal-fixture-1.0\n'
  exit 0
fi
mkdir -p "{calls}"
COUNT=$(ls "{calls}" | wc -l | tr -d ' ')
cat > "{calls}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"review":{{"wrong":false}}}}'
else
  printf '{{"review":{{"reason":"approved"}}}}'
fi
"#,
            calls = calls.display()
        ),
    );
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!(
            r#"schema-version = "0.4"

[harness.fixture]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.fixture.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
session-mode = "per-frame"
"#,
            script.display()
        ),
    )
    .unwrap();
    let ledger = home.join("drive.json");
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--out",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(output.status.success(), "drive failed: {:?}", utf8(&output));

    let prompt = fs::read_to_string(calls.join("prompt-1.txt")).unwrap();
    assert!(
        prompt.contains("property `review` (signal:review)")
            && prompt.contains("schema schema:review-payload")
            && prompt.contains("received object with fields: wrong")
            && prompt.contains("required shape object requiring reason"),
        "the live correction must carry typed signal evidence: {prompt}"
    );
    let session: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(ledger).unwrap()).unwrap();
    assert_eq!(
        session["ledger"]["emitted-signals"][0]["payload"],
        serde_json::json!({ "reason": "approved" })
    );
    assert_eq!(session["status"], "completed");
}

#[test]
fn claude_json_result_wrapper_submits_the_nested_result_signal_payload() {
    let trait_text = DRIVE_TRAIT.replace("review", "result");
    let (_scratch, repo, home) = setup(&trait_text);
    let script = home.join("result-wrapper-harness.sh");
    write_executable(
        &script,
        r#"#!/bin/sh
if [ "$1" = "--probe" ]; then
  printf 'signal-fixture-1.0\n'
  exit 0
fi
printf '%s' '{"type":"result","result":"{\"result\":{\"reason\":\"approved\"}}"}'
"#,
    );
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!(
            r#"schema-version = "0.4"

[harness.fixture]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.fixture.cli]
argv = []
prompt-via = "stdin"
output = "claude-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
session-mode = "per-frame"
"#,
            script.display()
        ),
    )
    .unwrap();
    let ledger = home.join("result-wrapper.json");
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--out",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(output.status.success(), "drive failed: {:?}", utf8(&output));
    let session: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(ledger).unwrap()).unwrap();
    assert_eq!(session["status"], "completed");
    assert_eq!(
        session["ledger"]["emitted-signals"][0]["payload"],
        serde_json::json!({ "reason": "approved" })
    );
}

#[test]
fn drive_corrects_a_missing_required_slot_without_naming_an_omitted_signal() {
    let (_scratch, repo, home) = setup(MIXED_DRIVE_TRAIT);
    let script = home.join("mixed-signal-harness.sh");
    let calls = home.join("mixed-signal-calls");
    write_executable(
        &script,
        &format!(
            r#"#!/bin/sh
if [ "$1" = "--probe" ]; then
  printf 'signal-fixture-1.0\n'
  exit 0
fi
mkdir -p "{calls}"
COUNT=$(ls "{calls}" | wc -l | tr -d ' ')
cat > "{calls}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"review":{{"reason":"approved"}}}}'
else
  printf '{{"answer":"completed"}}'
fi
"#,
            calls = calls.display()
        ),
    );
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!(
            r#"schema-version = "0.4"

[harness.fixture]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.fixture.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
session-mode = "per-frame"
"#,
            script.display()
        ),
    )
    .unwrap();
    let ledger = home.join("mixed.json");
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-signal-payload/generated/index.toml",
            "--out",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &home,
    );
    assert!(output.status.success(), "drive failed: {:?}", utf8(&output));
    let correction = fs::read_to_string(calls.join("prompt-1.txt")).unwrap();
    assert!(correction.contains("missing: answer"), "{correction}");
    assert!(
        !correction.contains("missing: review"),
        "an omitted optional signal must not be named missing: {correction}"
    );
    let session: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(ledger).unwrap()).unwrap();
    assert_eq!(session["status"], "completed");
}
