//! 0150: trust must never report verified while runnable bytes are
//! unreviewed. A native family shares one candidate id across every
//! variant, each with its own canonical digest and trust verdict —
//! `ctx traits trust <family>`, `ctx traits list`, and a run-start refusal
//! must all report per-variant truth instead of one variant's verdict
//! standing in for the whole family. Scratch-fixture only (2026-08-08
//! ruling): never asserts against the committed packages.

use std::fs;

use support::{ScratchRoot, git_init, run_ctx, symlink_node_modules, utf8};

const TRAIT_ID: &str = "family-fixture";

fn family_fixture_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait, variant } from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
const variantFixture = (name) => variant({\n\
  name,\n\
  summary: `The ${name} variant.`,\n\
  procedure: procedure({\n\
    description: \"Describe what this trait should accomplish.\",\n\
    output,\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n\
\n\
export const draft = trait(\"family-fixture\", {\n\
  variants: {\n\
    default: variantFixture(\"default\").default(),\n\
    quick: variantFixture(\"quick\"),\n\
  },\n\
});\n"
}

/// A two-variant native family (`default`, `quick`), built exactly like
/// `proof_native_family_resolve.rs`'s fixture — the harness role is pinned so
/// a later `ctx traits run` refuses at the trust gate rather than at
/// harness discovery first, on any machine.
fn build_family_fixture(proj: &std::path::Path, home: &std::path::Path) {
    fs::create_dir_all(proj).unwrap();
    git_init(proj);
    symlink_node_modules(proj);

    let init = run_ctx(&["traits", "init", TRAIT_ID], proj, home);
    assert!(
        init.status.success(),
        "`ctx traits init {TRAIT_ID}` failed: {}",
        utf8(&init).1
    );
    fs::write(
        proj.join(".ctx/traits/runtime.toml"),
        "[agent.role.worker]\nharness = \"claude-code\"\n",
    )
    .unwrap();

    let source_path = proj.join(format!(".ctx/traits/authored/{TRAIT_ID}/source/index.ts"));
    fs::write(&source_path, family_fixture_source())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{TRAIT_ID}/source/index.ts"),
        ],
        proj,
        home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "expected `ctx traits build` to publish a native family\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// One variant approved, the other left unreviewed: `ctx traits trust
/// <family>` reports both variants with distinct digests/verdicts, the
/// conservative aggregate, and never a bare `verdict: verified` line
/// standing in for the whole family.
#[test]
fn family_report_shows_every_variant_and_a_conservative_aggregate() {
    let scratch = ScratchRoot::new("trust-family-report-partial");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let approve = run_ctx(
        &["traits", "trust", "approve", &format!("{TRAIT_ID}:quick")],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&approve);
    assert!(
        approve.status.success(),
        "expected `trust approve {TRAIT_ID}:quick` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let status = run_ctx(&["traits", "trust", TRAIT_ID], &proj, &home);
    let (stdout, stderr) = utf8(&status);
    assert!(
        status.status.success(),
        "expected `ctx traits trust {TRAIT_ID}` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("variant=quick") && stdout.contains("verdict=verified"),
        "expected the quick variant to report verified: {stdout}"
    );
    assert!(
        stdout.contains("variant=default") && stdout.contains("verdict=unreviewed"),
        "expected the default variant to report unreviewed: {stdout}"
    );
    assert!(
        stdout.contains("aggregate: partial (1/2)"),
        "expected a conservative partial aggregate, not a flat verdict: {stdout}"
    );
    assert!(
        !stdout.contains("verdict: verified"),
        "a family with an unreviewed variant must never print a bare `verdict: verified` line: {stdout}"
    );
}

/// `ctx traits trust <family>:<variant>` echoes the variant it was asked
/// about in both its header and its next-action, instead of silently
/// dropping the qualifier and reporting as the bare family id.
#[test]
fn single_variant_status_echoes_the_variant_it_was_asked_about() {
    let scratch = ScratchRoot::new("trust-family-report-variant-echo");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let status = run_ctx(
        &["traits", "trust", &format!("{TRAIT_ID}:default")],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&status);
    assert!(
        status.status.success(),
        "expected `ctx traits trust {TRAIT_ID}:default` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let header = stdout.lines().next().unwrap_or("");
    assert!(
        header.contains(&format!("{TRAIT_ID}:default")),
        "header must echo the variant-qualified ref: {header}"
    );
    let next_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("next:"))
        .unwrap_or("");
    assert!(
        next_line.contains(&format!("{TRAIT_ID}:default")),
        "next-action must name the variant-qualified ref, not the bare family id: {next_line}"
    );
}

/// `ctx traits list --json` carries every family member's own trust, and the
/// family header's aggregate reads `partial` rather than a flat `verified`
/// once only one of two variants is approved.
#[test]
fn list_json_carries_per_variant_trust_and_a_partial_family_aggregate() {
    let scratch = ScratchRoot::new("trust-family-report-list-json");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let approve = run_ctx(
        &["traits", "trust", "approve", &format!("{TRAIT_ID}:quick")],
        &proj,
        &home,
    );
    assert!(approve.status.success(), "trust approve quick failed");

    let list = run_ctx(&["traits", "list", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&list);
    assert!(
        list.status.success(),
        "expected `ctx traits list --json` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains(&format!("\"family\": \"{TRAIT_ID}\"")),
        "expected a family entry for {TRAIT_ID}: {stdout}"
    );
    assert!(
        stdout.contains("\"trust\": \"partial (1/2)\""),
        "expected the family's own aggregate trust field to read partial: {stdout}"
    );
    assert!(
        stdout.contains("\"variant\": \"quick\"") && stdout.contains("\"variant\": \"default\""),
        "expected both variants as distinct members: {stdout}"
    );
}

/// `ctx traits run` on the unreviewed variant names the real condition — the
/// digest, the variant, and the exact approve command — never the generic
/// `invalid manifest` framing the old wrapping produced.
#[test]
fn run_on_unreviewed_variant_names_trust_and_the_approve_command() {
    let scratch = ScratchRoot::new("trust-family-report-run-refusal");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let run = run_ctx(
        &[
            "traits",
            "run",
            &format!("{TRAIT_ID}:default"),
            "--no-drive",
            "--ephemeral",
        ],
        &proj,
        &home,
    );
    assert!(
        !run.status.success(),
        "expected run on an unreviewed variant to refuse"
    );
    let (stdout, stderr) = utf8(&run);
    assert!(
        stderr.contains("trait trust refused") && stderr.contains("is unreviewed on this machine"),
        "refusal must name the unreviewed trust condition: {stderr}"
    );
    assert!(
        stderr.contains(&format!("ctx traits trust approve {TRAIT_ID}:default")),
        "refusal must name the exact approve command for this variant: {stderr}"
    );
    assert!(
        !stderr.contains("invalid manifest"),
        "refusal must not reuse the manifest-error framing: {stderr}\nstdout: {stdout}"
    );
}

/// A bare family-wide `trust approve` clears every variant's gate in one
/// act: the aggregate reads `verified` and the trust-unreviewed refusal is
/// gone for a variant that was never individually approved.
#[test]
fn bare_family_approve_clears_every_variant_and_the_refusal() {
    let scratch = ScratchRoot::new("trust-family-report-bare-approve");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let approve = run_ctx(&["traits", "trust", "approve", TRAIT_ID], &proj, &home);
    let (stdout, stderr) = utf8(&approve);
    assert!(
        approve.status.success(),
        "expected bare family `trust approve {TRAIT_ID}` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let status = run_ctx(&["traits", "trust", TRAIT_ID], &proj, &home);
    let (stdout, stderr) = utf8(&status);
    assert!(
        status.status.success(),
        "expected `ctx traits trust {TRAIT_ID}` to succeed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("aggregate: verified"),
        "expected every variant approved to read a flat verified aggregate: {stdout}"
    );

    let run = run_ctx(
        &[
            "traits",
            "run",
            &format!("{TRAIT_ID}:default"),
            "--no-drive",
            "--ephemeral",
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&run);
    assert!(
        !stderr.contains("is unreviewed on this machine"),
        "the trust-unreviewed refusal must be gone once every variant is approved\nstdout: {stdout}\nstderr: {stderr}"
    );
}
