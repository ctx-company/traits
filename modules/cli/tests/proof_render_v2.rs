//! P493: `ctx traits prompt` emits the render v2 tagged **behavior**
//! envelope (rule 3's four sections only), never authoring-surface
//! procedure/prompt/agent/port/signal/activation/scenario plumbing.
//! Behavioral assertions only — parse/inspect structure, never byte-compare
//! a frozen string (P461).
//!
//! The fixture trait declares one of every authoring-only section
//! (procedure, prompt, agent, port, signal, activation, scenario) on top of
//! intent/behavior so their absence from the injected text is a real,
//! non-vacuous assertion — never `.ctx/traits/implement-quick` or any other
//! checked-in package, so this suite is never coupled to a package's
//! evolving runtime semantics (P461's park-honesty lesson).

use std::fs;
use std::path::{Path, PathBuf};

use support::{ScratchRoot, git_init, require_success, run_ctx};

const TRAIT_ID: &str = "render-v2-fixture";

const TRAIT_MANIFEST: &str = r#"id = "render-v2-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Render V2 Fixture"
description = "P493 render-v2 proof fixture: exercises every authoring-only section so its absence from the behavior envelope is a non-vacuous assertion."

[[intent.require]]
id = "leanness"

[[intent.avoid]]
id = "accretion"

[[behavior.tone]]
id = "direct"

[activation]

[[activation.rule]]
id = "always"
reason = "matches the fixture task"
task-keyword = "render-v2-fixture-keyword"

[[port]]
id = "phase"
direction = "input"
schema = "schema:text"
description = "Fixture input port."

[[slot]]
id = "work-summary"
schema = "schema:text"
description = "Fixture worker output."

[[signal]]
id = "fixture-signal"
description = "Fires when the fixture's own condition is met."

[[scenario]]
id = "fixture-positive"
variant = "positive"
input = "A representative fixture task."
output = "The expected fixture behavior."

[[agent]]
id = "worker"
description = "Fixture worker role."
summary = "Implementation role."

[prompt.implement]
text = "Implement {port:phase}."
input = ["port:phase"]
output = ["slot:work-summary"]

[[sequence.main.sequence]]
id = "implement"
title = "Implement the fixture phase"
agent = "agent:worker"
prompt = "prompt:implement"
input = ["port:phase"]
output = ["slot:work-summary"]
"#;

fn write_fixture_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// A fresh Git repository with the fixture trait activated and trust-approved
/// so `ctx traits prompt` never refuses on the trust/status gate (P497).
fn ready_repo(scratch: &ScratchRoot) -> PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture_file(
        &repo.join(format!(".ctx/traits/authored/{TRAIT_ID}/trait.toml")),
        &format!(
            "[package]\nid = {TRAIT_ID:?}\nversion = \"0.1.0\"\nname = \"Render V2 Fixture\"\nstatus = \"draft\"\n"
        ),
    );
    write_fixture_file(
        &repo.join(format!(
            ".ctx/traits/authored/{TRAIT_ID}/generated/index.toml"
        )),
        TRAIT_MANIFEST,
    );
    require_success(
        "`ctx traits state --active` clears the draft gate",
        &["traits", "state", "--active", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    require_success(
        "`ctx traits trust --approved` clears the unreviewed gate",
        &["traits", "trust", "--approved", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    repo
}

#[test]
fn prompt_behavior_envelope_carries_zero_authoring_plumbing() {
    let scratch = ScratchRoot::new("render-v2-behavior-only");
    let repo = ready_repo(&scratch);

    let output = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    assert!(output.status.success(), "prompt should have succeeded");
    let (stdout, _) = support::utf8(&output);

    // Rule 3's four behavior sections are present.
    assert!(stdout.contains("<description>"));
    assert!(stdout.contains("<intent group=\"require\" id=\"leanness\">"));
    assert!(stdout.contains("<intent group=\"avoid\" id=\"accretion\">"));
    assert!(stdout.contains("<behavior axis=\"tone\" id=\"direct\">"));

    // No authoring-only tag, and no authoring-only prose, leaks into the
    // behavior envelope actually injected into a host model's context.
    for absent_tag in [
        "<procedure",
        "<prompt",
        "<agent",
        "<port",
        "<signal",
        "<activation",
        "<scenario",
    ] {
        assert!(
            !stdout.contains(absent_tag),
            "behavior envelope must not contain {absent_tag}:\n{stdout}"
        );
    }
    for absent_text in [
        "sequence[",
        "Runtime command",
        "Static host note",
        "Fixture worker role",
    ] {
        assert!(
            !stdout.contains(absent_text),
            "behavior envelope must not contain {absent_text:?}:\n{stdout}"
        );
    }
}

#[test]
fn json_text_is_byte_identical_to_plain_stdout_and_digest_matches_the_envelope_attribute() {
    let scratch = ScratchRoot::new("render-v2-json-parity");
    let repo = ready_repo(&scratch);

    let plain_output = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    assert!(plain_output.status.success());
    let (plain_stdout, _) = support::utf8(&plain_output);

    let json_output = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--json"],
        &repo,
        &scratch.home(),
    );
    assert!(json_output.status.success());
    let (json_stdout, _) = support::utf8(&json_output);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_stdout).expect("prompt --json output is valid JSON");

    let json_text = parsed
        .get("text")
        .and_then(|v| v.as_str())
        .expect("json output has a text field");
    assert_eq!(
        json_text, plain_stdout,
        "prompt --json text must be byte-identical to plain stdout"
    );

    let model_view_digest = parsed
        .get("digests")
        .and_then(|d| d.get("model-view"))
        .and_then(|v| v.as_str())
        .expect("json output has digests.model-view");

    // D2: the `model-view` attribute on the envelope's own `<trait>` root is
    // exactly the reported digest — the attribute and the digest can never
    // disagree because both are derived from the same value.
    let expected_attr = format!("model-view=\"{model_view_digest}\"");
    assert!(
        plain_stdout.contains(&expected_attr),
        "expected {expected_attr} in the envelope:\n{plain_stdout}"
    );
}

#[test]
fn prompt_levels_select_matching_artifacts_and_summary_is_compact() {
    let scratch = ScratchRoot::new("render-v2-prompt-levels");
    let repo = ready_repo(&scratch);

    let flagless = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    let full = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--level", "full"],
        &repo,
        &scratch.home(),
    );
    assert_eq!(
        flagless.stdout, full.stdout,
        "full is the default prompt level"
    );

    let summary = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--level", "summary"],
        &repo,
        &scratch.home(),
    );
    assert!(summary.status.success());
    let (summary_text, _) = support::utf8(&summary);
    assert!(summary_text.contains("level=\"summary\""));
    assert!(summary_text.contains("<intent group=\"require\">"));
    assert!(!summary_text.contains("id=\"leanness\""));
    assert!(!summary_text.contains("Keep the implementation"));

    let json = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--level", "summary", "--json"],
        &repo,
        &scratch.home(),
    );
    let (stdout, _) = support::utf8(&json);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["text"].as_str(), Some(summary_text.as_str()));
    let digest = parsed["digests"]["model-view"].as_str().unwrap();
    assert!(summary_text.contains(&format!("model-view=\"{digest}\"")));
    assert!(summary_text.len() < flagless.stdout.len());
}
