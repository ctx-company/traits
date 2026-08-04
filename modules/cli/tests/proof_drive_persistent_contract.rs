//! Provider-free proof that a persistent CLI harness receives a complete,
//! current output envelope on every turn, including a compact correction.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

const TRAIT: &str = r#"id = "fixture-persistent-contract"
schema-version = "0.2"
version = "0.1.0"
name = "Persistent contract fixture"
summary = "Exercises output contracts across resident turns."

[[agent]]
id = "worker"
description = "Resident fixture worker."
summary = "Fixture worker."

[[slot]]
id = "answer"
schema = "schema:boolean"
description = "First output."

[[slot]]
id = "answer2"
schema = "schema:text"
description = "Second output."

[procedure]
description = "Two worker frames."

[[procedure.sequence]]
id = "first"
title = "Produce boolean"
agent = "agent:worker"
prompt = "Return a boolean."
output = ["slot:answer"]

[[procedure.sequence]]
id = "second"
title = "Produce text"
agent = "agent:worker"
prompt = "Return text."
output = ["slot:answer2"]
"#;

const MANIFEST: &str = r#"[package]
id = "fixture-persistent-contract"
version = "0.1.0"
name = "Persistent contract fixture"
status = "draft"
"#;

fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

/// A single Claude-stream-json process records every JSON user message it
/// receives. Its first answer is deliberately schema-invalid, exercising the
/// compact correction turn before it completes the two-frame procedure.
fn resident_script(log_dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-persistent-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
printf '%s\n' "$$" > "{log_dir}/resident-pid"
COUNT=0
while IFS= read -r MESSAGE; do
  printf '%s\n' "$MESSAGE" > "{log_dir}/message-$COUNT.json"
  if [ "$COUNT" = "0" ]; then
    printf '%s\n' '{{"type":"result","session_id":"fixture-warm-session","result":"{{\"answer\":\"not-a-boolean\"}}"}}'
  elif [ "$COUNT" = "1" ]; then
    printf '%s\n' '{{"type":"result","session_id":"fixture-warm-session","result":"{{\"answer\":true}}"}}'
  else
    printf '%s\n' '{{"type":"result","session_id":"fixture-warm-session","result":"{{\"answer2\":\"done\"}}"}}'
  fi
  COUNT=$((COUNT + 1))
done
"#,
        log_dir = log_dir.display(),
    )
}

fn init_fixture_repo(repo: &Path, home: &Path, script: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/fixture-persistent-contract/generated")).unwrap();
    git_init(repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    let script = script.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        repo.join("ctx.toml"),
        format!(
            r#"schema-version = "0.2"

[harness.fixture]
kind = "custom"
bin = "{script}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.fixture.cli]
argv = []
warm-argv = ["--fixture-warm"]
session-flag = "--session"
prompt-via = "stdin"
output = "claude-stream-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
session-mode = "persistent"
"#
        ),
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-persistent-contract/trait.toml"),
        MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-persistent-contract/generated/index.toml"),
        TRAIT,
    )
    .unwrap();
    require_success("persistent fixture init", &["traits", "init"], repo, home);
    let fixture = ".ctx/traits/fixture-persistent-contract/generated/index.toml";
    require_success(
        "persistent fixture review",
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    require_success(
        "persistent fixture activate",
        &["traits", "activate", "--file", fixture],
        repo,
        home,
    );
}

fn report(output: &std::process::Output) -> serde_json::Value {
    let (stdout, stderr) = utf8(output);
    let text = stdout
        .lines()
        .skip_while(|line| !line.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\n{stdout}\n{stderr}")
    })["value"]["drive"]
        .clone()
}

fn recorded_prompt(log_dir: &Path, turn: usize) -> String {
    let message = fs::read_to_string(log_dir.join(format!("message-{turn}.json"))).unwrap();
    serde_json::from_str::<serde_json::Value>(&message).unwrap()["message"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn envelope_count(prompt: &str) -> usize {
    prompt.match_indices("<output>").count()
}

#[test]
fn resident_persistent_turns_each_receive_one_current_output_contract() {
    let scratch = ScratchRoot::new("persistent-output-contract");
    let home = scratch.home();
    let repo = home.join("repo");
    let log_dir = repo.join(".ctx/debug/persistent");
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("fixture.sh");
    write_executable(&script, &resident_script(&log_dir));
    init_fixture_repo(&repo, &home, &script);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-persistent-contract/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--max-retries",
            "1",
            "--frame-seconds",
            "2",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let drive = report(&output);
    assert_eq!(drive["status"], "completed", "drive: {drive}");

    let messages: Vec<_> = (0..3).map(|turn| recorded_prompt(&log_dir, turn)).collect();
    assert!(
        !log_dir.join("message-3.json").exists(),
        "the resident process must handle exactly the two frames plus correction"
    );
    assert!(
        log_dir.join("resident-pid").exists(),
        "warm process did not start"
    );
    for prompt in &messages {
        assert_eq!(
            envelope_count(prompt),
            1,
            "every resident turn must carry exactly one output envelope: {prompt}"
        );
        assert!(
            prompt.contains("Return ONLY one JSON object matching <format>"),
            "output envelope is incomplete: {prompt}"
        );
    }
    assert!(
        messages[0].contains("\"answer\": boolean")
            && messages[1].contains("\"answer\": boolean")
            && messages[2].contains("\"answer2\": string"),
        "each turn must receive its frame's current format: {messages:?}"
    );
    assert!(
        messages[1].contains("schema schema:boolean")
            && !messages[2].contains("\"answer\": boolean"),
        "the correction must retain its boolean schema and the next frame must not inherit it: {messages:?}"
    );
    assert!(
        !messages[1].contains("<input>"),
        "the rejected-answer retry must use the compact correction path: {}",
        messages[1]
    );
    assert!(
        drive["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["event"] != "harness-warm-fallback"),
        "persistent proof must not silently fall back to cold dispatch: {drive}"
    );
}
