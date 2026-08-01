//! P5049: a resumable ledger executes its pinned source even when an explicit
//! resume caller still supplies the source path after that path has rebuilt.

use std::{fs, io::Write, process::Stdio};

use camino::Utf8Path;
use ctx_traits_io::run::{TraitSourceDrift, trait_source_drift_from};
use support::{ScratchRoot, controlled_command, ctx_bin, git_init, require_success, run_ctx, utf8};

const FILE: &str = ".ctx/traits/pin/generated/index.toml";
const FLAT_FILE: &str = ".ctx/traits/pin/trait.toml";

fn manifest(summary: &str) -> String {
    format!(
        r#"id = "pin"
schema-version = "0.2"
version = "0.1.0"
name = "Pin"
summary = {summary:?}

[procedure]
description = "provider-free pin fixture"

[[slot]]
id = "result"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Command"
kind = "command"
cmd = "test -f session-package"
output = ["slot:result"]
"#
    )
}

fn resource_manifest(summary: &str) -> String {
    manifest(summary).replace(
        "[[slot]]\nid = \"result\"",
        "[[resource]]\nid = \"session-package\"\npath = \"resources/session-package\"\ndigest = \"sha256:d2643db503140a01fbb99a4445ddbcd480fded4a29fd0445ccfe3536d069acfe\"\n\n[[slot]]\nid = \"result\"",
    ).replace(
        "cmd = \"test -f session-package\"\noutput = [\"slot:result\"]",
        "command = { argv = [\"test\", \"-f\", \"{resource:session-package}\"] }\ninput = [\"resource:session-package\"]\noutput = [\"slot:result\"]",
    )
}

#[test]
fn explicit_file_resume_replays_pinned_source_after_rebuild() {
    let scratch = ScratchRoot::new("trait-shape-pinning");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/pin/generated")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/traits/pin/trait.toml"),
        "[package]\nid = \"pin\"\nversion = \"0.1.0\"\nname = \"Pin\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(repo.join(FILE), manifest("source A")).unwrap();
    fs::write(repo.join("session-package"), "pinned package root\n").unwrap();
    require_success(
        "activate fixture",
        &["traits", "activate", "--file", FILE],
        &repo,
        &scratch.home(),
    );
    require_success(
        "approve digest A",
        &["traits", "trust", "approve", "pin"],
        &repo,
        &scratch.home(),
    );
    let ledger = repo.join("pinned.json");
    require_success(
        "start pinned run",
        &[
            "traits",
            "run",
            "--file",
            FILE,
            "--no-drive",
            "--out",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );

    fs::write(repo.join(FILE), manifest("source B")).unwrap();
    let session: ctx_traits_core::procedure::session::Session =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    let owner_source = repo.join(FILE);
    let loaded = ctx_traits_io::run::load_trait_for_session(
        Some(owner_source.to_str().unwrap()),
        None,
        &session,
        "test",
    )
    .expect("the owning repository's rebuilt path supplies pinned package context");
    assert_eq!(
        loaded.trait_root,
        repo.join(".ctx/traits/pin"),
        "pinned commands and resources must resolve from the owning package"
    );
    assert_eq!(
        loaded.source_digest,
        session.source_digest.as_ref().unwrap().as_str(),
        "the owner-qualified context must not replace pinned bytes"
    );
    let output = run_ctx(
        &[
            "traits",
            "run-status",
            "--file",
            FILE,
            "--session",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "explicit resume must replay the pin: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("pinned session bytes remain resumable"),
        "status must expose source drift: {stdout}"
    );

    // The supplied path belongs to another package. It must neither replace
    // the pin nor contribute the command/resource root for pinned execution.
    let foreign = "foreign/.ctx/traits/pin/generated/index.toml";
    fs::create_dir_all(repo.join("foreign/.ctx/traits/pin/generated")).unwrap();
    fs::write(
        repo.join("foreign/.ctx/traits/pin/trait.toml"),
        "[package]\nid = \"foreign\"\nversion = \"0.1.0\"\nname = \"Foreign\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(repo.join(foreign), manifest("foreign source")).unwrap();
    let unrelated = ctx_traits_io::run::load_trait_for_session(
        Some(repo.join(foreign).to_str().unwrap()),
        None,
        &session,
        "test",
    )
    .expect("the recorded context remains a safe fallback");
    assert_ne!(
        unrelated.trait_root,
        repo.join("foreign/.ctx/traits/pin"),
        "an unrelated package must not provide package context for pinned bytes"
    );
    let foreign_resume = run_ctx(
        &[
            "traits",
            "drive",
            "--file",
            foreign,
            "--session",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&foreign_resume);
    assert!(
        foreign_resume.status.success(),
        "foreign explicit path must not replace pinned package context: stdout={stdout} stderr={stderr}"
    );

    // Dashboard inventories sessions from repositories other than its own
    // process cwd. The recorded path is relative, so it must be resolved from
    // this ledger's repository before validating the authoritative pin.
    let session: ctx_traits_core::procedure::session::Session =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    assert!(matches!(
        trait_source_drift_from(&session, Some(Utf8Path::from_path(&repo).unwrap())),
        TraitSourceDrift::Rebuilt { .. }
    ));

    let mut mcp = controlled_command(&ctx_bin(), &["traits", "mcp"], &repo, &scratch.home())
        .stdin(Stdio::piped())
        .spawn()
        .expect("start MCP server");
    writeln!(
        mcp.stdin.as_mut().unwrap(),
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ctx_traits_run_status",
                "arguments": {"trait-id": "pin", "session": ledger},
            },
        })
    )
    .unwrap();
    let mcp_output = mcp.wait_with_output().expect("read MCP response");
    let (mcp_stdout, mcp_stderr) = utf8(&mcp_output);
    assert!(
        mcp_output.status.success() && !mcp_stdout.contains("\"ok\":false"),
        "MCP trait-id resume must replay the pin: stdout={mcp_stdout} stderr={mcp_stderr}"
    );

    let resumed = run_ctx(
        &[
            "traits",
            "drive",
            "--file",
            FILE,
            "--session",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&resumed);
    assert!(
        resumed.status.success(),
        "drive with rebuilt explicit source must execute pinned A: stdout={stdout} stderr={stderr}"
    );

    fs::remove_file(repo.join(FILE)).unwrap();
    let session: ctx_traits_core::procedure::session::Session =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    assert!(matches!(
        trait_source_drift_from(&session, Some(Utf8Path::from_path(&repo).unwrap())),
        TraitSourceDrift::Missing
    ));
}

#[test]
fn legacy_status_accepts_explicit_recovery_without_persisting_a_warning() {
    let scratch = ScratchRoot::new("trait-shape-legacy-recovery");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/pin/generated")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/traits/pin/trait.toml"),
        "[package]\nid = \"pin\"\nversion = \"0.1.0\"\nname = \"Pin\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    let source_a = manifest("source A");
    fs::write(repo.join(FILE), &source_a).unwrap();
    require_success(
        "activate fixture",
        &["traits", "activate", "--file", FILE],
        &repo,
        &scratch.home(),
    );
    require_success(
        "approve digest A",
        &["traits", "trust", "approve", "pin"],
        &repo,
        &scratch.home(),
    );
    let ledger = repo.join("legacy.json");
    require_success(
        "start legacy fixture",
        &[
            "traits",
            "run",
            "--file",
            FILE,
            "--no-drive",
            "--out",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let mut ledger_value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    ledger_value
        .pointer_mut("/provenance/trait-source")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("document");
    fs::write(
        &ledger,
        serde_json::to_string_pretty(&ledger_value).unwrap(),
    )
    .unwrap();
    // A decodable but mismatching explicit file must not hide the unchanged
    // recorded-path fallback for a legacy ledger.
    let mismatch = ".ctx/traits/pin/generated/mismatch.toml";
    fs::write(repo.join(mismatch), manifest("source B")).unwrap();
    let fallback = run_ctx(
        &[
            "traits",
            "run-status",
            "--file",
            mismatch,
            "--session",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&fallback);
    assert!(
        fallback.status.success(),
        "matching recorded path must survive a mismatching explicit candidate: stdout={stdout} stderr={stderr}"
    );
    fs::write(repo.join(FILE), manifest("source B")).unwrap();
    let recovered = ".ctx/traits/pin/generated/recovered.toml";
    fs::write(repo.join(recovered), source_a).unwrap();

    let before_status = fs::read_to_string(&ledger).unwrap();
    let unavailable = run_ctx(
        &[
            "traits",
            "run-status",
            "--session",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&unavailable);
    assert!(
        unavailable.status.success(),
        "legacy status failed: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("unrecoverable"),
        "legacy drift must be visible: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(&ledger).unwrap(),
        before_status,
        "status must not write the ledger"
    );

    let output = run_ctx(
        &[
            "traits",
            "run-status",
            "--file",
            recovered,
            "--session",
            ledger.to_str().unwrap(),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "recovered legacy status failed: stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stdout.contains("unrecoverable"),
        "matching recovery bytes must not be reported unavailable: {stdout}"
    );
    let persisted = fs::read_to_string(&ledger).unwrap();
    assert!(
        !persisted.contains("unrecoverable"),
        "read-only status must not persist a derived warning: {persisted}"
    );
}

#[test]
fn malformed_pin_is_not_reported_as_a_legacy_ledger() {
    let scratch = ScratchRoot::new("trait-shape-malformed-pin");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/pin/generated")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/traits/pin/trait.toml"),
        "[package]\nid = \"pin\"\nversion = \"0.1.0\"\nname = \"Pin\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(repo.join(FILE), manifest("source A")).unwrap();
    require_success(
        "activate fixture",
        &["traits", "activate", "--file", FILE],
        &repo,
        &scratch.home(),
    );
    require_success(
        "approve digest A",
        &["traits", "trust", "approve", "pin"],
        &repo,
        &scratch.home(),
    );
    let ledger = repo.join("malformed.json");
    require_success(
        "start pinned run",
        &[
            "traits",
            "run",
            "--file",
            FILE,
            "--no-drive",
            "--out",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    fs::write(repo.join(FILE), manifest("source B")).unwrap();
    let mut ledger_value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    *ledger_value
        .pointer_mut("/provenance/trait-source/document")
        .unwrap() = serde_json::Value::String("not a trait document".to_string());
    fs::write(
        &ledger,
        serde_json::to_string_pretty(&ledger_value).unwrap(),
    )
    .unwrap();

    let session: ctx_traits_core::procedure::session::Session =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    let drift = trait_source_drift_from(&session, Some(Utf8Path::from_path(&repo).unwrap()));
    assert!(matches!(
        drift,
        TraitSourceDrift::UnrecoverableInvalidPin { .. }
    ));
    assert!(
        drift.warning().unwrap().contains("pinned session document"),
        "invalid pins must not claim the ledger has no pin"
    );
}

#[test]
fn flat_legacy_owner_context_replays_pinned_resources_from_another_repository() {
    let scratch = ScratchRoot::new("trait-shape-flat-owner-context");
    let owner = scratch.home().join("owner");
    let dashboard = scratch.home().join("dashboard");
    fs::create_dir_all(owner.join(".ctx/traits/pin/generated")).unwrap();
    fs::create_dir_all(&dashboard).unwrap();
    git_init(&owner);
    git_init(&dashboard);
    fs::write(
        owner.join(".ctx/traits/pin/trait.toml"),
        "[package]\nid = \"pin\"\nversion = \"0.1.0\"\nname = \"Pin\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(owner.join(FILE), resource_manifest("source A")).unwrap();
    fs::create_dir_all(owner.join(".ctx/traits/pin/resources")).unwrap();
    fs::write(
        owner.join(".ctx/traits/pin/resources/session-package"),
        "owner package resource\n",
    )
    .unwrap();
    require_success(
        "activate flat fixture",
        &["traits", "activate", "--file", FILE],
        &owner,
        &scratch.home(),
    );
    require_success(
        "approve flat A",
        &["traits", "trust", "approve", "pin"],
        &owner,
        &scratch.home(),
    );
    let ledger = owner.join("flat-pinned.json");
    require_success(
        "start flat pinned run",
        &[
            "traits",
            "run",
            "--file",
            FILE,
            "--no-drive",
            "--out",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &owner,
        &scratch.home(),
    );
    // Model a readable pre-package-manifest ledger source after the owning
    // checkout rebuilt it. The pin remains the authoritative A document.
    fs::remove_file(owner.join(FLAT_FILE)).unwrap();
    fs::write(owner.join(FLAT_FILE), resource_manifest("source B")).unwrap();
    let mut ledger_value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    *ledger_value
        .pointer_mut("/provenance/trait-source/path")
        .unwrap() = serde_json::Value::String(FLAT_FILE.to_string());
    fs::write(
        &ledger,
        serde_json::to_string_pretty(&ledger_value).unwrap(),
    )
    .unwrap();

    let resumed = run_ctx(
        &[
            "traits",
            "drive",
            "--file",
            owner.join(FLAT_FILE).to_str().unwrap(),
            "--session",
            ledger.to_str().unwrap(),
            "--progress",
            "none",
        ],
        &dashboard,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&resumed);
    assert!(
        resumed.status.success(),
        "owner-qualified flat context must resolve pinned resources after rebuild: stdout={stdout} stderr={stderr}"
    );

    let session: ctx_traits_core::procedure::session::Session =
        serde_json::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    let foreign = dashboard.join(".ctx/traits/foreign/trait.toml");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(
        &foreign,
        manifest("foreign source").replace("id = \"pin\"", "id = \"foreign\""),
    )
    .unwrap();
    let loaded = ctx_traits_io::run::load_trait_for_session(
        Some(foreign.to_str().unwrap()),
        None,
        &session,
        "test",
    )
    .expect("foreign flat context must leave the pinned source authoritative");
    assert_ne!(loaded.trait_root, foreign.parent().unwrap());
}

#[test]
fn trust_status_keeps_exact_digest_history_authoritative() {
    let scratch = ScratchRoot::new("trait-shape-exact-trust");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/pin/generated")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/traits/pin/trait.toml"),
        "[package]\nid = \"pin\"\nversion = \"0.1.0\"\nname = \"Pin\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    let source_a = manifest("source A");
    fs::write(repo.join(FILE), &source_a).unwrap();
    require_success(
        "activate fixture",
        &["traits", "activate", "--file", FILE],
        &repo,
        &scratch.home(),
    );
    require_success(
        "approve A",
        &["traits", "trust", "approve", "pin"],
        &repo,
        &scratch.home(),
    );
    fs::write(repo.join(FILE), manifest("source B")).unwrap();
    require_success(
        "approve B",
        &["traits", "trust", "approve", "pin"],
        &repo,
        &scratch.home(),
    );
    fs::write(repo.join(FILE), &source_a).unwrap();

    let status = |label: &str| {
        let output = run_ctx(
            &["traits", "trust", "pin", "--json"],
            &repo,
            &scratch.home(),
        );
        let (stdout, stderr) = utf8(&output);
        assert!(
            output.status.success(),
            "{label}: stdout={stdout} stderr={stderr}"
        );
        serde_json::from_str::<serde_json::Value>(&stdout).expect("trust status JSON")
    };
    let approved_a = status("A must retain its approval after B is approved");
    assert_eq!(approved_a["verdict"], "verified");
    assert_eq!(approved_a["recorded-state"], "verified");
    assert_eq!(approved_a["current-digest"], approved_a["recorded-digest"]);
    let digest_a = approved_a["current-digest"].as_str().unwrap().to_string();

    require_success(
        "block exact A",
        &["traits", "trust", "block", "pin"],
        &repo,
        &scratch.home(),
    );
    let blocked_a = status("a later exact block must win");
    assert_eq!(blocked_a["verdict"], "blocked");
    assert_eq!(blocked_a["recorded-state"], "blocked");

    require_success(
        "approve raw exact A evidence",
        &["traits", "trust", "approve", "--digest", &digest_a],
        &repo,
        &scratch.home(),
    );
    let raw_a = status("raw exact A evidence must be reportable");
    assert_eq!(raw_a["verdict"], "verified");
    assert_eq!(raw_a["recorded-state"], "verified");
    assert_eq!(raw_a["current-digest"], raw_a["recorded-digest"]);

    fs::write(repo.join(FILE), manifest("source C")).unwrap();
    let unseen_c = status("unseen C remains unreviewed");
    assert_eq!(unseen_c["verdict"], "unreviewed");
    assert_ne!(unseen_c["current-digest"], unseen_c["recorded-digest"]);
}
