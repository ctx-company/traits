//! P497: trust gates the render family (`prompt`/`export`/`host install`),
//! bringing it onto the same lifecycle/trust classification the run family
//! already uses. Behavioral assertions only (exit code + substring of the
//! typed message/gate code), never a stored expected output (P461).

use std::fs;
use std::path::Path;

use support::{ScratchRoot, git_init, require_success, run_ctx};

const TRAIT_ID: &str = "render-trust-fixture";

const TRAIT_MANIFEST: &str = "id = \"render-trust-fixture\"\n\
schema-version = \"0.2\"\n\
version = \"0.1.0\"\n\
name = \"Render Trust Fixture\"\n\
summary = \"P497 render-trust gate proof fixture.\"\n";

/// Write `contents` to `path`, creating parent directories as needed.
fn write_fixture_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// A fresh Git repository with a hand-written, minimal `status = "draft"`
/// package under `.ctx/traits/render-trust-fixture` — no `node`/`pnpm`
/// dependency (`generated/index.toml` is authored directly, in the style of
/// `proof_cli_surface.rs`'s `seed_demo_trait`). Under a scratch `HOME`, this
/// trait is always both draft (status) and unreviewed (trust: no record).
fn draft_repo(scratch: &ScratchRoot) -> std::path::PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture_file(
        &repo.join(".ctx/traits/render-trust-fixture/trait.toml"),
        &format!(
            "[package]\nid = {TRAIT_ID:?}\nversion = \"0.1.0\"\nname = \"Render Trust Fixture\"\nstatus = \"draft\"\n"
        ),
    );
    write_fixture_file(
        &repo.join(".ctx/traits/render-trust-fixture/generated/index.toml"),
        TRAIT_MANIFEST,
    );
    repo
}

#[test]
fn draft_and_unreviewed_prompt_refuses_flagless() {
    let scratch = ScratchRoot::new("render-trust-prompt-refuse");
    let repo = draft_repo(&scratch);

    let output = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    assert!(!output.status.success(), "prompt should have refused");
    let (_, stderr) = support::utf8(&output);
    assert!(
        stderr.contains("blocked.trust.unreviewed"),
        "refusal must name blocked.trust.unreviewed: {stderr}"
    );
    assert!(
        stderr.contains("--allow-unreviewed"),
        "refusal must name the escape flag: {stderr}"
    );
}

#[test]
fn allow_unreviewed_renders_with_draft_advisory_on_stderr() {
    let scratch = ScratchRoot::new("render-trust-prompt-allow-unreviewed");
    let repo = draft_repo(&scratch);

    let baseline_output = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--allow-unreviewed"],
        &repo,
        &scratch.home(),
    );
    assert!(
        baseline_output.status.success(),
        "prompt --allow-unreviewed should render: {:?}",
        support::utf8(&baseline_output)
    );
    let (baseline_stdout, baseline_stderr) = support::utf8(&baseline_output);
    assert!(
        baseline_stderr.contains("blocked.status.draft"),
        "draft advisory must land on stderr: {baseline_stderr}"
    );

    // Re-run without the flag but with trust approved (still draft) to
    // capture the pre-advisory stdout and prove it is byte-identical.
    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    let approved_output = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    assert!(
        approved_output.status.success(),
        "prompt should render once trust is approved even while still draft: {:?}",
        support::utf8(&approved_output)
    );
    let (approved_stdout, approved_stderr) = support::utf8(&approved_output);
    assert_eq!(
        approved_stdout, baseline_stdout,
        "draft advisory must never change prompt's model-visible stdout bytes"
    );
    assert!(
        approved_stderr.contains("blocked.status.draft"),
        "draft advisory must still land on stderr once trust is approved: {approved_stderr}"
    );
}

#[test]
fn activated_and_verified_trait_renders_flagless_with_no_advisory() {
    let scratch = ScratchRoot::new("render-trust-clear");
    let repo = draft_repo(&scratch);

    require_success(
        "`ctx traits activate` clears the draft gate",
        &["traits", "activate", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let prompt_output = run_ctx(&["traits", "prompt", TRAIT_ID], &repo, &scratch.home());
    assert!(
        prompt_output.status.success(),
        "prompt should succeed flagless once activated/verified: {:?}",
        support::utf8(&prompt_output)
    );
    let (_, prompt_stderr) = support::utf8(&prompt_output);
    assert!(
        prompt_stderr.is_empty(),
        "no lifecycle advisory expected once activated/verified: {prompt_stderr}"
    );

    let export_out = repo.join("export-out");
    let export_output = run_ctx(
        &[
            "traits",
            "export",
            TRAIT_ID,
            "--out",
            export_out.to_str().unwrap(),
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        export_output.status.success(),
        "export should succeed flagless once activated/verified: {:?}",
        support::utf8(&export_output)
    );

    let install_output = run_ctx(
        &["traits", "host", "install", "--host", "cursor", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    assert!(
        install_output.status.success(),
        "host install should succeed flagless once activated/verified: {:?}",
        support::utf8(&install_output)
    );
}

#[test]
fn blocked_trait_refuses_all_three_verbs_naming_trust_list() {
    let scratch = ScratchRoot::new("render-trust-blocked");
    let repo = draft_repo(&scratch);

    require_success(
        "`ctx traits trust block` records a blocked verdict",
        &["traits", "trust", "block", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    for args in [
        vec!["traits", "prompt", TRAIT_ID, "--allow-unreviewed"],
        vec!["traits", "export", TRAIT_ID, "--allow-unreviewed"],
        vec![
            "traits",
            "host",
            "install",
            "--host",
            "cursor",
            "--allow-unreviewed",
            "--allow-draft",
            TRAIT_ID,
        ],
    ] {
        let output = run_ctx(&args, &repo, &scratch.home());
        assert!(
            !output.status.success(),
            "{args:?} must refuse a blocked trait even with every escape flag"
        );
        let (_, stderr) = support::utf8(&output);
        assert!(
            stderr.contains("ctx traits trust list"),
            "{args:?} refusal must name `ctx traits trust list`: {stderr}"
        );
        assert!(
            !stderr.contains("trust approve"),
            "{args:?} refusal must not suggest `trust approve` for a blocked decision: {stderr}"
        );
    }
}

#[test]
fn json_refusal_parses_as_an_object_carrying_the_expected_gate_code() {
    let scratch = ScratchRoot::new("render-trust-json-refusal");
    let repo = draft_repo(&scratch);

    let output = run_ctx(
        &["traits", "prompt", TRAIT_ID, "--json"],
        &repo,
        &scratch.home(),
    );
    assert!(!output.status.success(), "prompt --json should refuse");
    let (stdout, _) = support::utf8(&output);
    assert!(
        stdout.trim_start().starts_with('{'),
        "--json refusal must print a JSON object envelope: {stdout}"
    );
    assert!(
        stdout.contains("\"gates\""),
        "--json refusal envelope must carry gates: {stdout}"
    );
    assert!(
        stdout.contains("blocked.trust.unreviewed"),
        "--json refusal envelope must carry the expected gate code: {stdout}"
    );
}

#[test]
fn host_install_refuses_draft_flagless_and_succeeds_with_allow_draft() {
    let scratch = ScratchRoot::new("render-trust-host-install-draft");
    let repo = draft_repo(&scratch);

    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let refused = run_ctx(
        &["traits", "host", "install", "--host", "cursor", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    assert!(
        !refused.status.success(),
        "host install must refuse a draft trait flagless"
    );
    let (_, stderr) = support::utf8(&refused);
    assert!(
        stderr.contains("blocked.status.draft"),
        "refusal must name blocked.status.draft: {stderr}"
    );
    assert!(
        stderr.contains("--allow-draft"),
        "refusal must name the escape flag: {stderr}"
    );

    let installed = run_ctx(
        &[
            "traits",
            "host",
            "install",
            "--host",
            "cursor",
            "--allow-draft",
            TRAIT_ID,
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        installed.status.success(),
        "host install --allow-draft should succeed: {:?}",
        support::utf8(&installed)
    );
}

#[test]
fn host_update_reports_a_since_blocked_placement_as_an_error_entry_and_leaves_bytes_untouched() {
    let scratch = ScratchRoot::new("render-trust-host-update-blocked");
    let repo = draft_repo(&scratch);

    require_success(
        "`ctx traits activate` clears the draft gate",
        &["traits", "activate", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    require_success(
        "`ctx traits host install` places the trait",
        &["traits", "host", "install", "--host", "cursor", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let placed_path = repo.join(".cursor/rules").join(format!("{TRAIT_ID}.mdc"));
    let placed_before = fs::read(&placed_path)
        .unwrap_or_else(|error| panic!("cannot read placed artifact {placed_path:?}: {error}"));

    require_success(
        "`ctx traits trust block` blocks the placed trait's source",
        &["traits", "trust", "block", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let update_output = run_ctx(
        &["traits", "host", "update", "--json"],
        &repo,
        &scratch.home(),
    );
    let (update_stdout, _) = support::utf8(&update_output);
    assert!(
        !update_output.status.success(),
        "host update must exit non-zero when any recorded placement fails"
    );
    assert!(
        update_stdout.contains("\"outcome\": \"error\""),
        "the now-blocked placement must surface as an error entry: {update_stdout}"
    );

    let placed_after = fs::read(&placed_path)
        .unwrap_or_else(|error| panic!("cannot read placed artifact {placed_path:?}: {error}"));
    assert_eq!(
        placed_before, placed_after,
        "a refused host update must leave the placed artifact's bytes untouched"
    );
}

#[test]
fn run_family_still_refuses_a_blocked_trait_with_its_own_unchanged_wording() {
    let scratch = ScratchRoot::new("render-trust-run-parity");
    let repo = draft_repo(&scratch);

    require_success(
        "`ctx traits trust block` records a blocked verdict",
        &["traits", "trust", "block", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let output = run_ctx(&["traits", "run", TRAIT_ID], &repo, &scratch.home());
    assert!(
        !output.status.success(),
        "run must still refuse a blocked trait"
    );
    let (_, stderr) = support::utf8(&output);
    assert!(
        stderr.contains("blocked.trust.blocked"),
        "run's own refusal wording must be unchanged: {stderr}"
    );
}
