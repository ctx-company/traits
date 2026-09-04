//! Public-path proofs for command-attempt evidence: durable paired records,
//! restart-derived attempt numbers, and the deliberate privacy/lazy-open bounds.

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, git_init_on_branch, require_success, run_ctx, utf8};

const ENV_SECRET: &str = "env-private-token";
const ARGV_SECRET: &str = "argv-private-token";

fn commit_all(repo: &Path, message: &str) {
    let status = Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .expect("stage fixture");
    assert!(status.success());
    let status = Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(repo)
        .status()
        .expect("commit fixture");
    assert!(status.success());
}

fn review_and_activate(repo: &Path, home: &Path, fixture: &str) {
    require_success(
        "review fixture",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        repo,
        home,
    );
    require_success(
        "activate fixture",
        &["traits", "state", "--active", "--file", fixture],
        repo,
        home,
    );
    commit_all(repo, "activate");
}

fn init_command_fixture(repo: &Path, home: &Path) {
    let package = repo.join(".ctx/traits/demo");
    fs::create_dir_all(package.join("generated")).expect("create fixture package");
    git_init_on_branch(repo, "main");
    fs::write(
        repo.join(".gitignore"),
        ".ctx/traits/worktrees/\n.ctx/runs/\n",
    )
    .unwrap();
    fs::write(
        package.join("trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(
        package.join("generated/index.toml"),
        format!(
            r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "Command journal fixture."

[[setting]]
id = "private-argv"
schema = "text"
description = "Never journal this resolved command argument."
default = "{ARGV_SECRET}"

[procedure]
description = "Run a failing command."

[[slot]]
id = "result"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Fail with bounded output"
kind = "command"
output = ["slot:result"]

[procedure.sequence.command]
argv = ["sh", "-c", "i=0; while [ $i -lt 5000 ]; do printf o; i=$((i + 1)); done; i=0; while [ $i -lt 5000 ]; do printf e >&2; i=$((i + 1)); done; exit 7", "{{setting:private-argv}}"]
"#
        ),
    )
    .unwrap();
    fs::create_dir_all(repo.join(".ctx/traits")).unwrap();
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!("[worktree.env]\nPRIVATE_ENV = \"{ENV_SECRET}\"\n"),
    )
    .unwrap();
    commit_all(repo, "init");
    review_and_activate(repo, home, ".ctx/traits/demo/generated/index.toml");
}

fn init_commandless_fixture(repo: &Path, home: &Path) {
    let package = repo.join(".ctx/traits/model-only");
    fs::create_dir_all(package.join("generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/runs/\n").unwrap();
    fs::write(
        package.join("trait.toml"),
        "[package]\nid = \"model-only\"\nversion = \"0.1.0\"\nname = \"Model only\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(
        package.join("generated/index.toml"),
        r#"id = "model-only"
schema-version = "0.4"
version = "0.1.0"
name = "Model only"
description = "No command frames."

[[agent]]
id = "worker"
description = "A model-only fixture worker."

[[slot]]
id = "result"
schema = "schema:text"

[procedure]
description = "Ask a worker."

[[procedure.sequence]]
id = "prompt"
title = "Prompt"
agent = "agent:worker"
prompt = "Return text."
output = ["slot:result"]
"#,
    )
    .unwrap();
    commit_all(repo, "init");
    review_and_activate(repo, home, ".ctx/traits/model-only/generated/index.toml");
}

fn command_attempts(sidecar: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(sidecar)
        .expect("command run writes sidecar")
        .lines()
        .map(|line| serde_json::from_str(line).expect("sidecar line is JSON"))
        .filter(|record: &serde_json::Value| {
            record["record"]
                .as_str()
                .unwrap_or_default()
                .starts_with("command-attempt-")
        })
        .collect()
}

#[test]
fn failing_command_attempts_are_persisted_across_cli_restarts_without_secrets() {
    let scratch = ScratchRoot::new("command-attempt-journal");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_command_fixture(&repo, &home);
    let ledger = repo.join(".ctx/runs/fixture.json");
    let sidecar = ledger.with_extension("json.activity.jsonl");

    for attempt in 1..=2 {
        let output = if attempt == 1 {
            run_ctx(
                &[
                    "traits",
                    "run",
                    "--file",
                    ".ctx/traits/demo/generated/index.toml",
                    "--out",
                    &ledger.to_string_lossy(),
                    "--worktree",
                    "--json",
                    "--progress",
                    "none",
                ],
                &repo,
                &home,
            )
        } else {
            run_ctx(
                &[
                    "traits",
                    "internal",
                    "drive",
                    "--session",
                    &ledger.to_string_lossy(),
                    "--json",
                    "--progress",
                    "none",
                ],
                &repo,
                &home,
            )
        };
        assert!(
            sidecar.exists(),
            "command run did not create its sidecar: {:?}",
            utf8(&output)
        );
        let records = command_attempts(&sidecar);
        assert_eq!(records.len(), attempt as usize * 2);
        let started = &records[records.len() - 2];
        let ended = &records[records.len() - 1];
        assert_eq!(started["record"], "command-attempt-started");
        assert_eq!(ended["record"], "command-attempt-ended");
        for record in [started, ended] {
            assert_eq!(record["item_id"], "command");
            assert_eq!(record["source_index"], 0);
            assert_eq!(record["run_index"], 0);
            assert_eq!(record["position_path"], serde_json::json!([]));
            assert_eq!(record["attempt"], attempt);
        }
        assert!(ended["at_epoch_ms"].as_u64().unwrap() >= started["at_epoch_ms"].as_u64().unwrap());
        assert!(
            ended["started_at_epoch_ms"].as_u64().unwrap()
                >= started["at_epoch_ms"].as_u64().unwrap(),
            "the runner observation starts after the durable pre-spawn record"
        );
        assert_eq!(ended["exit_code"], 7);
        // The shared tail clipper adds a short omission marker around its
        // 4096-byte retained payload, so prove the bounded tail and suffix
        // rather than freezing that presentation marker here.
        assert!(ended["stdout_tail"].as_str().unwrap().len() <= 4200);
        assert!(ended["stderr_tail"].as_str().unwrap().len() <= 4200);
        assert!(ended["stdout_tail"].as_str().unwrap().ends_with('o'));
        assert!(ended["stderr_tail"].as_str().unwrap().ends_with('e'));
        assert_eq!(ended["stdout_tail_truncated"], true);
        assert_eq!(ended["stderr_tail_truncated"], true);
    }

    let sidecar_bytes = fs::read(&sidecar).expect("read sidecar bytes");
    assert!(
        !sidecar_bytes
            .windows(ENV_SECRET.len())
            .any(|window| window == ENV_SECRET.as_bytes())
            && !sidecar_bytes
                .windows(ARGV_SECRET.len())
                .any(|window| window == ARGV_SECRET.as_bytes()),
        "resolved environment and argv secrets must not enter command evidence"
    );
}

#[test]
fn commandless_session_never_creates_a_command_attempt_sidecar() {
    let scratch = ScratchRoot::new("commandless-journal");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_commandless_fixture(&repo, &home);
    let ledger = repo.join(".ctx/runs/model-only.json");
    let output = run_ctx(
        &[
            "traits",
            "--session",
            &ledger.to_string_lossy(),
            "run",
            "--file",
            ".ctx/traits/model-only/generated/index.toml",
            "--no-drive",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert!(output.status.success(), "commandless start must succeed");
    assert!(
        !ledger.with_extension("json.activity.jsonl").exists(),
        "a session with no command frames must leave no command-attempt sidecar"
    );
}
