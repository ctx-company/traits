//! P491 done-when proofs: the CLI error boundary prints an error's full
//! `Display` (multi-line, never first-line-only), exits are typed for
//! `doctor`/`check`/standalone `merge`, a genuine usage error never wears a
//! `filesystem error` costume, `crate::Error::Command` carries no
//! `"command error:"` prefix, and `ctx traits run <unmatched-query>` prints
//! its own header rather than reusing `run-info`'s.

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init, repo_root, run_ctx, utf8};

/// P491's findings exit, distinct from 0 (nothing to report) and 1 (could
/// not run) — mirrored here rather than imported so a drift in
/// `crate::app::error::EXIT_FINDINGS` breaks this proof loudly instead of
/// silently tracking whatever the binary happens to emit.
const EXIT_FINDINGS: i32 = 6;
const EXIT_MERGE_PARKED: i32 = 4;

/// A config file with a field type serde/toml will reject, producing a
/// genuinely multi-line `toml::de::Error` `Display` (the P477 shape: a
/// "TOML parse error at line N, column M" header followed by a distinct
/// detail line naming the actual problem).
#[test]
fn command_error_prints_full_multiline_reason_not_just_first_line() {
    let scratch = ScratchRoot::new("p491-full-reason");
    let config = scratch.home().join("repo.toml");
    fs::write(&config, "[agent.role.worker]\nharness = 42\n").unwrap();

    let mut command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo_root(),
        &scratch.home(),
    );
    command.env("CTX_CONFIG", &config);
    let output = command.output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);

    assert!(
        stderr.lines().count() > 1,
        "a toml decode error's full multi-line Display must survive to stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("toml decode error"),
        "first line must still name the error class: {stderr}"
    );
}

/// `crate::Error::Command`'s `{message}` display carries no `"command
/// error:"` prefix — every constructor already writes a full-sentence
/// message naming its own subject.
#[test]
fn command_error_has_no_command_error_prefix() {
    let scratch = ScratchRoot::new("p491-no-command-error-prefix");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    // `ctx traits set` with no `--session` is a genuine `Error::Command`
    // usage refusal.
    let output = run_ctx(&["traits", "set", "foo", "bar"], &repo, &scratch.home());
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(
        !stderr.contains("command error:"),
        "the generic prefix must be gone: {stderr}"
    );
}

/// `ctx traits next` with neither `--session` nor `--agent` is a genuine
/// usage error, not a filesystem effect — it must not wear a `filesystem
/// error at run-next.agent` costume.
#[test]
fn usage_error_wears_no_filesystem_path_costume() {
    let scratch = ScratchRoot::new("p491-usage-no-path-costume");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    let output = run_ctx(&["traits", "next"], &repo, &scratch.home());
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(
        !stderr.contains("filesystem error"),
        "a genuine usage error must not be reported as a filesystem effect: {stderr}"
    );
    assert!(
        stderr.contains("--agent"),
        "the refusal must still name the missing flag: {stderr}"
    );
}

/// `ctx traits run <unmatched-query>` prints its own `ctx traits run`
/// header (not `ctx traits run-info`, which `run_format::print_run_selection`
/// used to hardcode regardless of caller).
#[test]
fn query_run_refusal_prints_run_header_not_run_info_header() {
    let scratch = ScratchRoot::new("p491-run-header");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    let output = run_ctx(
        &[
            "traits", "run", "--", "no", "such", "trait", "matches", "this", "query",
        ],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
    let (stdout, _) = utf8(&output);
    assert!(stdout.contains("ctx traits run"), "{stdout}");
    assert!(
        !stdout.contains("ctx traits run-info"),
        "a query-run refusal must not print run-info's own header: {stdout}"
    );
}

/// A `SKILL.md` carrying one critical hidden-content finding (an HTML
/// comment), for `doctor`'s typed findings-exit proof.
fn write_critical_finding_skill(dir: &Path, relative: &str) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: Broken Skill\ndescription: A demo skill with hidden content.\n---\n\n\
         # Broken Skill\n\n<!-- secret instructions for the agent -->\n",
    )
    .unwrap();
}

/// `doctor` exits `EXIT_FINDINGS` (6) — distinct from 0 (nothing to report)
/// and 1 (could not run at all) — when it ran to completion and reports a
/// critical finding; `--migrate-state --json` (a separate, housekeeping-only
/// handler `justfile:564` depends on) still exits 0 unconditionally.
#[test]
fn doctor_exit_split_findings_vs_could_not_run_vs_migrate_state() {
    let scratch = ScratchRoot::new("p491-doctor-exit-split");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    // Derived from the canonical list, never a copy of it: an incomplete
    // duplicate makes `doctor` report repo-state housekeeping these tests were
    // written to exclude, and here that warning silently flipped a
    // source-free root from "could not run" to "ran, warnings only".
    fs::write(
        repo.join(".ctx/.gitignore"),
        ctx_traits_io::gitignore::CANONICAL_ENTRIES
            .iter()
            .map(|entry| format!("{entry}\n"))
            .collect::<String>(),
    )
    .unwrap();

    let sources = scratch.home().join("sources");
    write_critical_finding_skill(&sources, "broken/SKILL.md");
    let output = run_ctx(
        &["traits", "doctor", sources.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_FINDINGS);

    let missing = scratch.home().join("nowhere");
    fs::create_dir_all(&missing).unwrap();
    let output = run_ctx(
        &["traits", "doctor", missing.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "a source-free root must still refuse"
    );
    assert_ne!(
        output.status.code(),
        Some(EXIT_FINDINGS),
        "\"could not run\" must stay distinct from \"ran and found findings\""
    );

    let output = run_ctx(
        &["traits", "doctor", "--migrate-state", "--json"],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
}

/// A minimal provider-free command-only `demo` trait, reviewed and
/// activated, with an out-of-tree mutation seeded into its ledger after a
/// clean completion — the P479 permanent-park precondition `merge.rs`
/// already implements. Modeled on `proof_tripwire.rs`'s
/// `init_fixture_repo`/seeded-park pattern, reused rather than re-derived.
fn build_seeded_park_run(repo: &Path, home: &Path) -> String {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    support::git_init(repo);
    fs::write(
        repo.join(".gitignore"),
        ".ctx/worktrees/\n.ctx/config.toml\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        r#"id = "demo"
schema-version = "0.2"
version = "0.1.0"
name = "Demo"
summary = "A provider-free command-only trait."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
output = ["slot:notified"]

[procedure.sequence.command]
argv = ["sh", "-c", "true"]
"#,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(repo)
        .status()
        .unwrap();

    let fixture = ".ctx/traits/demo/generated/index.toml";
    let output = run_ctx(
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let output = run_ctx(&["traits", "activate", "--file", fixture], repo, home);
    assert_exit_code(&output, 0);
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "activate"])
        .current_dir(repo)
        .status()
        .unwrap();

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--worktree",
            "--json",
            "--progress",
            "none",
        ],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    let json_start = stdout.find('{').expect("run --json prints a JSON object");
    let envelope: serde_json::Value =
        serde_json::from_str(&stdout[json_start..]).expect("run --json output must parse");
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();
    let run_id = envelope["value"]["session"]["run-id"]
        .as_str()
        .expect("run reports its run-id")
        .to_string();

    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    ledger["provenance"]["out-of-tree-mutations"] = serde_json::json!([{
        "paths": [".ctx/config.toml"],
        "frame": "run 1 / source 1: Run command (item:command, kind:command)",
        "policy": "park",
        "detected-at-epoch": 1,
    }]);
    fs::write(
        &session_path,
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();
    run_id
}

/// Standalone `ctx traits merge <run>` on a run whose ledger carries a
/// permanent P479 park finding exits the typed `EXIT_MERGE_PARKED` (4) —
/// the same status→exit mapping `run --merge` already uses (both verbs
/// share `disposition_for_report_status`/`CompletionDisposition::exit_code`,
/// which `run --merge`'s own suite already exercises for its side).
#[test]
fn standalone_merge_exits_typed_parked_status() {
    let scratch = ScratchRoot::new("p491-standalone-merge-parked");
    let repo = scratch.home().join("repo");
    let run_id = build_seeded_park_run(&repo, &scratch.home());

    let output = run_ctx(
        &["traits", "merge", &run_id, "--json"],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let (stdout, _) = utf8(&output);
    assert!(
        stdout.contains("\"status\": \"parked\""),
        "the JSON report must still print before the typed exit: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Source guard: no stray Debug-formatted enum/Option reaches non-verbose
// human output. Modeled on `proof_identity_scrub.rs`'s allowlist structure.
// ---------------------------------------------------------------------------

/// The files P491's debug-leak sweep actually rewrote (draft §3.3's
/// `run_format.rs`/`run.rs`/`doctor.rs`/`generate.rs`/`preview.rs`, plus
/// `drive.rs`'s non-persisted event/status lines) — a fixed list, not a
/// whole-tree walk, since the sweep's own explicit boundary rules
/// (persisted `merge.rs` evidence strings `merge_story` parses literally;
/// `explain_inspect.rs`'s `--verbose`-only inspection dumps) exempt the
/// rest of `modules/cli/src/app` from this guard by design.
const SWEPT_FILES: &[&str] = &[
    "modules/cli/src/app/run_format.rs",
    "modules/cli/src/app/run.rs",
    "modules/cli/src/app/doctor.rs",
    "modules/cli/src/app/generate.rs",
    "modules/cli/src/app/preview.rs",
    "modules/cli/src/app/drive.rs",
];

#[test]
fn no_stray_debug_format_reaches_human_output() {
    let root = repo_root();

    let mut violations = Vec::new();
    for relative in SWEPT_FILES {
        let path = root.join(relative);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let mut in_test_mod = false;
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                in_test_mod = true;
            }
            if in_test_mod || !line.contains("{:?}") {
                continue;
            }
            // The one remaining, deliberately-kept-as-Debug site: a struct
            // with no `Serialize` impl, out of scope for `wire_name`
            // (P491 draft §3.3's kebab-case-enum rule does not apply to it).
            if relative == &"modules/cli/src/app/doctor.rs" && line.contains("merge.generated") {
                continue;
            }
            violations.push(format!("{relative}:{}: {}", index + 1, line.trim()));
        }
    }

    assert!(
        violations.is_empty(),
        "found `{{:?}}` in a swept file outside its one recorded exception or a #[cfg(test)] \
         module — route the value through `presentation::wire_name`/`presentation::optional`, \
         or record a new exception with a reason:\n{}",
        violations.join("\n")
    );
}
